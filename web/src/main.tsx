// KelpMux web client: terminals only. It signs in with a security key
// through browser-terminated OpenSSH, attaches to one KelpServer workspace
// through `kelpmux web-bridge`, and shows the workspace's panes as tabs or a
// live grid. Tabs, grid and focus are this browser's own layout; the TUI and
// other browsers see the same panes and processes with their own layouts.
import React, { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  discoverKey,
  keyFingerprint,
  registeredKeys,
} from "./terminal/connection";
import { SharedSession as Session } from "./terminal/shared-session";
import { Terminal, TerminalTile, type Store } from "./terminal/Terminal";
import type { Connection, Identity, Pane, SignInKey } from "./terminal/types";
import { gridColumns } from "./terminal/layout";
import { trimScreens } from "./terminal/screen-cache";
import "./styles/base.css";
import "./styles/shell.css";
import "./terminal/terminal.css";
import "./styles/dialogs.css";
import "./styles/login.css";
import "./styles/kelpmux.css";
import {
  fonts,
  loadAppearance,
  interfaceVariables,
  resolveMode,
  resolveTerminalMode,
  terminalVariables,
  themeVariables,
} from "./appearance";
import { ToastProvider, useToast } from "./Toast";
import { activityIcon, stateLabel } from "./activity-icon";
import { basename } from "./format";
import { Stats } from "./Stats";
import { KeySettings } from "./KeySettings";
import { LoginScreen, PopupConnecting } from "./Login";
import { ConfigPanel } from "./ConfigPanel";
import { OPEN_SETTINGS } from "./Settings";
import { PaneLabel, StatusBar, TopBar, type SessionMode } from "./Shell";
import { Icon } from "./status";
import {
  loadLayout,
  loadView,
  moveBefore,
  orderPanes,
  saveLayout,
  saveView,
} from "./layout-store";

/** Grid tiles per page. The stream allows ten subscriptions per connection;
 * eight leaves room for popup windows sharing the connection. */
const PAGE = 8;
/** The `Web` request API this client speaks (see `kelpmux web-bridge`). */
const API_VERSION = 1;

const store: Store = {
  screens: new Map(),
  events: new EventTarget(),
  leases: new Map(),
  closing: new Set(),
  session: null,
};
const storage = {
  get(key: string) {
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  },
  set(key: string, value: string) {
    try {
      localStorage.setItem(key, value);
    } catch {}
  },
  remove(key: string) {
    try {
      localStorage.removeItem(key);
    } catch {}
  },
};

function App() {
  const toast = useToast();
  const lastAppearance = useRef<string | undefined>(undefined);
  const lastCloseWarning = useRef<boolean | undefined>(undefined);
  const [signedInKey, setSignedInKey] = useState<Pick<
    Identity,
    "credentialId" | "label"
  > | null>(null);
  const popupPane =
    Number(new URLSearchParams(location.search).get("pane")) || 0;
  const resumed = useRef(false);
  const [appearance, setAppearance] = useState(loadAppearance);
  const [prefersLight, setPrefersLight] = useState(
    () => !!window.matchMedia?.("(prefers-color-scheme: light)").matches,
  );
  useEffect(() => {
    const query = window.matchMedia?.("(prefers-color-scheme: light)");
    if (!query) return;
    const changed = () => setPrefersLight(query.matches);
    changed();
    query.addEventListener("change", changed);
    return () => query.removeEventListener("change", changed);
  }, []);
  useEffect(() => {
    const root = document.documentElement;
    const style = root.style;
    const mode = resolveMode(appearance.mode, prefersLight);
    root.dataset.mode = mode;
    const variables = {
      ...themeVariables(appearance.theme, mode),
      ...interfaceVariables(appearance.theme, mode),
      ...terminalVariables(
        appearance.theme,
        resolveTerminalMode(appearance.terminal, mode),
      ),
    };
    for (const [key, value] of Object.entries(variables))
      style.setProperty(key, value);
    document
      .querySelector('meta[name="theme-color"]')
      ?.setAttribute("content", variables["--background"]);
    style.setProperty("--terminal-font", fonts[appearance.font].family);
    style.setProperty("--terminal-size", String(appearance.size));
    const serialized = JSON.stringify(appearance);
    storage.set("kelpmux-appearance", serialized);
    if (
      lastAppearance.current !== undefined &&
      lastAppearance.current !== serialized
    )
      toast("Appearance saved", { key: "appearance" });
    lastAppearance.current = serialized;
    store.events.dispatchEvent(new Event("frame"));
  }, [appearance, prefersLight, toast]);

  // Connection and security keys.
  const [connection, setConnection] = useState<Connection | null>(null),
    [identity, setIdentity] = useState<Identity | null>(null),
    [online, setOnline] = useState(false),
    [connecting, setConnecting] = useState(false);
  const [keyBusy, setKeyBusy] = useState(false);
  const [keyToDelete, setKeyToDelete] = useState("");
  const [settingsSection, setSettingsSection] = useState("appearance");
  const [popupFallback, setPopupFallback] = useState(false);
  const [findPane, setFindPane] = useState(0);
  const [keySelected, setKeySelected] = useState(false);
  const [shownKey, setShownKey] = useState("");
  const [keyModal, setKeyModal] = useState(false);
  const [signBusy, setSignBusy] = useState(false);
  const [knownKeys, setKnownKeys] = useState<Identity[]>([]);
  const [confirmClose, setConfirmClose] = useState(
    () => storage.get("kelpmux-confirm-close-v2") !== "false",
  );
  const keyDiscovery = useRef<AbortController | null>(null);
  const authProgress = useRef({
    listed: false,
    signed: false,
    welcomed: false,
    canceled: false,
  });
  function savedIdentities(): Identity[] {
    try {
      const saved = JSON.parse(storage.get("kelpmux-identities") || "[]");
      return Array.isArray(saved) ? saved.slice(-32) : [];
    } catch {
      return [];
    }
  }
  function saveIdentity(id: Identity) {
    const saved = registeredKeys(connection!, savedIdentities(), identity);
    const index = saved.findIndex(
      (key) => key.credentialId === id.credentialId,
    );
    if (index < 0) saved.push(id);
    else saved[index] = id;
    if (saved.length > 32)
      throw Error("This browser supports up to 32 saved keys.");
    setKnownKeys(saved.slice(-32));
    storage.set("kelpmux-identities", JSON.stringify(saved.slice(-32)));
    storage.set("kelpmux-identity", JSON.stringify(id));
    setIdentity(id);
    setShownKey(id.credentialId);
    setKeySelected(true);
  }
  function deleteKey(id: string) {
    const keys = knownKeys.filter((key) => key.credentialId !== id);
    setKnownKeys(keys);
    storage.set("kelpmux-identities", JSON.stringify(keys));
    storage.remove("kelpmux-proof-identity");
    if (identity?.credentialId === id) {
      const next = keys[0] || null;
      setIdentity(next);
      setKeySelected(false);
      if (next) storage.set("kelpmux-identity", JSON.stringify(next));
      else storage.remove("kelpmux-identity");
    }
    setKeyToDelete("");
    toast("Saved key removed from this browser", { key: "keys" });
  }
  function preferKey(key: Identity) {
    saveIdentity(key);
    toast("Key selected for your next sign-in", { key: "keys" });
  }
  function updateKey(
    id: string,
    patch: Partial<Pick<Identity, "label" | "emoji" | "color">>,
  ) {
    const keys = knownKeys.map((key) =>
      key.credentialId === id ? { ...key, ...patch } : key,
    );
    setKnownKeys(keys);
    storage.set("kelpmux-identities", JSON.stringify(keys));
    const current = keys.find(
      (key) => key.credentialId === identity?.credentialId,
    );
    if (current) {
      setIdentity(current);
      storage.set("kelpmux-identity", JSON.stringify(current));
    }
  }
  /** Cache the key that signed this connection and remember it. */
  async function afterSignIn(session: NonNullable<Store["session"]>) {
    const config = connection;
    const signed = session.signedIdentity();
    if (!config || !signed) return;
    try {
      const keys: Identity[] = await Promise.all(
        registeredKeys(config, savedIdentities()).map(async (key) => ({
          ...key,
          fingerprint: await keyFingerprint(key),
        })),
      );
      const index = keys.findIndex(
        (key) => key.credentialId === signed.credentialId,
      );
      const [entry] = registeredKeys(config, [
        {
          ...(keys[index] || {}),
          ...signed,
          fingerprint: await keyFingerprint(signed),
        },
      ]);
      if (index >= 0) keys[index] = entry;
      else keys.push(entry);
      const kept = keys.slice(-32);
      storage.set("kelpmux-identities", JSON.stringify(kept));
      storage.set("kelpmux-identity", JSON.stringify(entry));
      setKnownKeys(kept);
      setIdentity(entry);
    } catch (error) {
      console.warn(error);
    }
  }

  const [error, setError] = useState(""),
    [status, setStatus] = useState(""),
    [sign, setSign] = useState<{
      run: () => Promise<void>;
      cancel: () => void;
    } | null>(null),
    [panes, setPanes] = useState<Pane[]>([]),
    [panesLoaded, setPanesLoaded] = useState(false),
    [active, setActive] = useState(0),
    [view, setView] = useState<SessionMode>(() =>
      popupPane ? "single" : loadView(),
    ),
    [dialog, setDialog] = useState(""),
    [workspace, setWorkspace] = useState(""),
    [epoch, setEpoch] = useState(""),
    [order, setOrder] = useState<number[]>([]),
    [vitals, setVitals] = useState<any>({}),
    [stats, setStats] = useState<any>({}),
    [query, setQuery] = useState(""),
    [page, setPage] = useState(0);
  useEffect(() => {
    const open = (event: Event) => {
      setSettingsSection((event as CustomEvent<string>).detail);
      setDialog("config");
    };
    window.addEventListener(OPEN_SETTINGS, open);
    return () => window.removeEventListener(OPEN_SETTINGS, open);
  }, []);
  const panesRef = useRef(panes);
  panesRef.current = panes;
  const [confirmAction, setConfirmAction] = useState<{
      title: string;
      text: string;
      buttons: {
        label: string;
        action: () => unknown | Promise<unknown>;
        color?: string;
      }[];
    } | null>(null),
    [drag, setDrag] = useState<number | null>(null),
    [drop, setDrop] = useState<number | null>(null);
  const metadata = useRef(""),
    connectionAttempt = useRef<Session | null>(null),
    gridRef = useRef<HTMLDivElement>(null),
    [gridWidth, setGridWidth] = useState(() => innerWidth);
  const prefix = useRef(false);
  const dragCleanup = useRef<(() => void) | null>(null);
  const suppressClick = useRef(0);
  const overlayWasOpen = useRef(false);
  const confirmBusy = useRef(false);
  const [confirmPending, setConfirmPending] = useState(false);
  async function approve(index = 0) {
    if (!confirmAction || confirmBusy.current) return;
    confirmBusy.current = true;
    setConfirmPending(true);
    try {
      await confirmAction.buttons[index].action();
    } catch (error) {
      fail(error);
    } finally {
      confirmBusy.current = false;
      setConfirmPending(false);
      setConfirmAction(null);
    }
  }
  const overlayOpen = !!dialog || !!confirmAction || keyModal || !!findPane;
  useEffect(() => {
    if (overlayWasOpen.current && !overlayOpen)
      requestAnimationFrame(() =>
        store.events.dispatchEvent(new Event("terminal-focus")),
      );
    overlayWasOpen.current = overlayOpen;
  }, [overlayOpen]);
  useEffect(() => () => dragCleanup.current?.(), []);
  const fail = (e: any) => setError(e?.message || String(e));
  async function rpc(method: string, params: any = {}) {
    try {
      return await store.session?.rpc(method, params);
    } catch (error) {
      fail(error);
      return null;
    }
  }
  useEffect(() => {
    let canceled = false;
    fetch("/connection.json")
      .then((r) => {
        if (!r.ok) throw Error("Web relay configuration unavailable");
        return r.json();
      })
      .then((cfg) => {
        if (canceled) return;
        setConnection(cfg);
        const raw =
          storage.get("kelpmux-identity") ||
          storage.get("kelpmux-proof-identity");
        let primary: Identity | null = null;
        try {
          if (raw) primary = JSON.parse(raw);
        } catch {}
        const keys = registeredKeys(cfg, savedIdentities(), primary);
        setKnownKeys(keys);
        setIdentity(
          keys.find((key) => key.credentialId === primary?.credentialId) ||
            keys.at(-1) ||
            null,
        );
      })
      .catch(fail);
    return () => {
      canceled = true;
      keyDiscovery.current?.abort();
      connectionAttempt.current?.close();
    };
  }, []);
  useEffect(() => {
    // Clear after the commit which unmounts closed terminals, rather than
    // when the earlier RPC reply arrives ahead of the stream metadata.
    const live = new Set(panes.map((pane) => pane.id));
    for (const id of store.closing!)
      if (!live.has(id)) store.closing!.delete(id);
  }, [panes]);

  // This browser's layout: tab order and focus, per workspace and epoch.
  const live = orderPanes(panes, order);
  useEffect(() => {
    if (!online || !panesLoaded || !workspace || !epoch || popupPane) return;
    saveLayout(workspace, {
      epoch,
      order: live.map((p) => p.id),
      focus: active,
    });
  }, [online, workspace, epoch, active, live.map((p) => p.id).join(",")]);
  useEffect(() => {
    if (panesLoaded && !live.some((p) => p.id === active))
      setActive(live[0]?.id || 0);
  }, [panes, order, panesLoaded]);

  const visible = popupPane
      ? live.filter((p) => p.id === popupPane)
      : view === "single"
        ? live.filter((p) => p.id === active)
        : live.slice(page * PAGE, page * PAGE + PAGE),
    visibleKey = visible.map((p) => p.id).join(",");
  const browserActivity = activityIcon(online ? panes : null);
  useEffect(() => {
    const icon = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
    if (icon) icon.href = browserActivity.url;
    document.title = workspace
      ? `${browserActivity.title} · ${workspace}`
      : browserActivity.title;
  }, [browserActivity.url, browserActivity.title, workspace]);
  useEffect(() => {
    storage.set("kelpmux-confirm-close-v2", String(confirmClose));
    if (
      lastCloseWarning.current !== undefined &&
      lastCloseWarning.current !== confirmClose
    )
      toast("Close confirmation preference saved", { key: "close-warning" });
    lastCloseWarning.current = confirmClose;
    if (!online || !confirmClose) return;
    const warn = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = true;
    };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [online, confirmClose, toast]);
  useEffect(() => {
    setPage((previous) =>
      Math.min(previous, Math.max(0, Math.ceil(live.length / PAGE) - 1)),
    );
  }, [live.length]);
  useEffect(() => {
    if (online)
      store.session?.request("subscribe", { panes: visible.map((p) => p.id) });
  }, [online, visibleKey]);
  useEffect(() => {
    if (!connection || resumed.current || !globalThis.SharedWorker) return;
    resumed.current = true;
    if (identity && !keyModal) void connect(true);
  }, [connection, identity]);
  useEffect(() => {
    if (dialog !== "stats" || !online) return;
    let alive = true;
    let timer: ReturnType<typeof setTimeout>;
    const fetchStats = async () => {
      const result = await rpc("status");
      if (alive && result) setStats(result);
      if (alive)
        timer = setTimeout(
          fetchStats,
          result && (!result.ready || result.refreshing) ? 50 : 1200,
        );
    };
    fetchStats();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [dialog, online]);
  useEffect(() => {
    if (!gridRef.current) return;
    const obs = new ResizeObserver((entries) => {
      setGridWidth(entries[0].contentRect.width);
    });
    obs.observe(gridRef.current);
    return () => obs.disconnect();
  }, [online, view]);

  /** Shows one terminal on its own. */
  function activate(id: number) {
    if (id !== active) setFindPane(0);
    setView("single");
    setActive(id);
  }
  /** A tab's click in grid mode focuses its tile on its page. */
  function focusInGrid(pane: Pane) {
    const index = live.findIndex((p) => p.id === pane.id);
    if (index < 0) return;
    setActive(pane.id);
    setPage(Math.floor(index / PAGE));
  }
  function chooseMode(next: SessionMode) {
    if (popupPane) return;
    saveView(next);
    setView(next);
    if (next === "grid") {
      const index = live.findIndex((p) => p.id === active);
      if (index >= 0) setPage(Math.floor(index / PAGE));
    }
  }
  function switchView() {
    chooseMode(view === "grid" ? "single" : "grid");
  }
  function cycle(step: number) {
    if (!live.length) return;
    const index = live.findIndex((p) => p.id === active);
    const next = live[(index + step + live.length) % live.length];
    if (view === "grid") focusInGrid(next);
    else activate(next.id);
    focusTerminal();
  }
  useEffect(() => {
    if (view !== "grid" || !active) return;
    document
      .querySelector<HTMLElement>(`.live-tile[data-tile-pane="${active}"]`)
      ?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [active, view, page]);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (confirmAction) {
        if (e.key === "Enter") {
          e.preventDefault();
          e.stopPropagation();
          if (!e.isComposing && !e.repeat) approve();
        }
        if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          if (!confirmBusy.current) setConfirmAction(null);
        }
        return;
      }
      if (e.key === "Escape") {
        if (findPane) {
          e.preventDefault();
          e.stopPropagation();
          setFindPane(0);
          return;
        }
        if (
          (e.target as HTMLElement)?.closest?.(
            '[data-font-picker][data-open="true"]',
          )
        )
          return;
        if (dialog || keyModal || sign) {
          sign?.cancel();
          keyDiscovery.current?.abort();
          setDialog("");
          setQuery("");
          setSign(null);
          setKeyModal(false);
        }
      }
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [sign, confirmAction, findPane, dialog, keyModal]);
  const focused =
    view === "single" || popupPane
      ? live.find((p) => p.id === (popupPane || active))
      : undefined;
  function openFind(pane: number) {
    if (!pane) return;
    setFindPane(pane);
    requestAnimationFrame(() => {
      const input =
        document.querySelector<HTMLInputElement>(".pane-find input");
      input?.focus();
      input?.select();
    });
  }
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (!online) return;
      if (
        (e.metaKey || e.ctrlKey) &&
        !e.shiftKey &&
        e.key.toLowerCase() === "f" &&
        !dialog &&
        !confirmAction &&
        !keyModal
      ) {
        e.preventDefault();
        e.stopPropagation();
        if (focused) openFind(focused.id);
        return;
      }
      if (
        (e.ctrlKey || e.metaKey) &&
        e.shiftKey &&
        e.key.toLowerCase() === "g"
      ) {
        e.preventDefault();
        switchView();
        return;
      }
      if (e.altKey && !e.ctrlKey && !e.metaKey && !overlayOpen) {
        if (e.code === "Tab") {
          e.preventDefault();
          e.stopImmediatePropagation();
          switchView();
          return;
        }
        if (/^Digit[0-9]$/.test(e.code)) {
          const number = Number(e.code.slice(5)) || 10;
          const pane = live[number - 1];
          if (pane) {
            e.preventDefault();
            e.stopImmediatePropagation();
            activate(pane.id);
            focusTerminal();
          }
          return;
        }
      }
      if (
        e.ctrlKey &&
        !e.altKey &&
        !e.metaKey &&
        e.key.toLowerCase() === "b" &&
        !prefix.current
      ) {
        e.preventDefault();
        e.stopImmediatePropagation();
        prefix.current = true;
        return;
      }
      if (!prefix.current) return;
      if (["Shift", "Control", "Alt", "Meta"].includes(e.key)) return;
      prefix.current = false;
      e.preventDefault();
      e.stopImmediatePropagation();
      const target = focused?.id || active;
      if (e.ctrlKey && e.key.toLowerCase() === "b") {
        // Ctrl-B twice sends one Ctrl-B to the focused terminal.
        if (target && store.session && !store.closing?.has(target)) {
          store.session.control(target);
          store.session.request("input", { pane: target, bytes: [2] });
        }
      } else if (e.key === "z") switchView();
      else if (e.key === "g") chooseMode("grid");
      else if (e.key === "n") cycle(1);
      else if (e.key === "p") cycle(-1);
      else if (e.key === "x") {
        const pane = live.find((p) => p.id === target);
        if (pane) close(pane);
      } else if (e.key === "/") openFind(target);
      else if (e.key === "Enter") newTab();
      else if (e.key === ",") setDialog("config");
      else if (e.key === "d") store.session?.close();
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [online, panes, order, active, view, overlayOpen, dialog, confirmAction]);

  function event(event: any) {
    if (event.SignedInKey) {
      setSignedInKey(event.SignedInKey);
      return;
    }
    const e = event.Stream;
    if (!e) return;
    if (e.type === "clipboard") {
      store.events.dispatchEvent(new CustomEvent("clipboard", { detail: e }));
      return;
    }
    if (e.type === "welcome") {
      authProgress.current.welcomed = true;
      setSignedInKey(store.session?.signedInKey || null);
      if (store.session) void afterSignIn(store.session);
      store.session!.request("subscribe", { panes: [] });
      rpc("state").then((result) => {
        if (!result) {
          store.session?.close();
          return;
        }
        if (result.api_version !== API_VERSION) {
          fail(
            Error(
              "This web build does not match the server's kelpmux web-bridge. Install matching versions.",
            ),
          );
          store.session?.close();
          return;
        }
        const layout = loadLayout(result.workspace, e.epoch);
        setWorkspace(result.workspace);
        setEpoch(e.epoch);
        setOrder(layout.order);
        if (layout.focus && !popupPane) setActive(layout.focus);
        setOnline(true);
        setConnecting(false);
      });
    }
    if (e.type === "update") {
      for (const s of e.snapshots) {
        store.screens.delete(s.pane);
        store.screens.set(s.pane, s);
      }
      for (const patch of e.patches) {
        const previous = store.screens.get(patch.pane);
        if (previous) {
          for (const [index, cell] of patch.changes)
            previous.screen.cells[index] = cell;
          previous.revision = patch.revision;
          previous.screen.cursor = patch.cursor;
          previous.screen.modes = patch.modes;
          store.screens.delete(patch.pane);
          store.screens.set(patch.pane, previous);
        }
      }
      trimScreens(store.screens, e.panes, e.subscribed);
      store.leases = new Map(
        e.leases.map((lease: any) => [lease.pane, lease.owner]),
      );
      const key = JSON.stringify([e.panes, e.vitals]);
      if (key !== metadata.current) {
        metadata.current = key;
        setPanes(e.panes);
        setPanesLoaded(true);
        setVitals(e.vitals);
        if (popupPane) setActive(popupPane);
      }
      store.events.dispatchEvent(new Event("frame"));
    }
    if (e.type === "lease") {
      if (e.owner) store.leases.set(e.pane, e.owner);
      else store.leases.delete(e.pane);
      store.events.dispatchEvent(new Event("frame"));
    }
    if (e.type === "error") setError(e.message);
  }
  const latestEvent = useRef(event);
  latestEvent.current = event;
  /** Let the authenticator choose its credential, then sign in with it. */
  async function discover() {
    if (!connection || connecting || keyDiscovery.current) return;
    const abort = new AbortController();
    keyDiscovery.current = abort;
    setError("");
    setKeyBusy(true);
    setStatus("Choose your security key and touch it to identify it.");
    let key: SignInKey;
    try {
      key = await discoverKey(connection, savedIdentities(), abort.signal);
    } catch (error) {
      if (!abort.signal.aborted) fail(error);
      setStatus("");
      return;
    } finally {
      keyDiscovery.current = null;
      setKeyBusy(false);
    }
    await connect(false, key);
  }
  async function connect(resumeOnly = false, chosen?: SignInKey) {
    if (!connection || connecting || keyDiscovery.current) return;
    let selectedIdentity: SignInKey | undefined = chosen?.candidates?.length
      ? chosen
      : undefined;
    if (!selectedIdentity) {
      const keys = registeredKeys(
        connection,
        savedIdentities(),
        chosen || identity,
      );
      selectedIdentity =
        keys.find(
          (key) => key.credentialId === (chosen || identity)?.credentialId,
        ) || keys.at(-1);
    }
    if (!selectedIdentity) return;
    const progress = {
      listed: false,
      signed: false,
      welcomed: false,
      canceled: false,
    };
    authProgress.current = progress;
    setKeyModal(false);
    setDialog("");
    setConfirmAction(null);
    setStats({});
    setError("");
    setConnecting(true);
    setSignedInKey(null);
    setStatus("Starting encrypted SSH connection…");
    store.screens.clear();
    store.leases.clear();
    store.closing!.clear();
    metadata.current = "";
    setPanes([]);
    setPanesLoaded(false);
    setActive(0);
    const session = new Session(
      connection,
      selectedIdentity,
      (value) => latestEvent.current(value),
      (run, cancel) => {
        const prompt = {
          run: async () => {
            setSignBusy(true);
            try {
              await run();
              setSign(null);
            } catch (error) {
              fail(error);
              setStatus(
                "Use Sign SSH login to retry, or Cancel to stop connecting.",
              );
            } finally {
              setSignBusy(false);
            }
          },
          cancel,
        };
        setSign(prompt);
        void prompt.run();
      },
      (message) => setStatus(message.trim()),
      (event) => {
        if (event.type === "list") progress.listed = true;
        else progress.signed = true;
      },
    );
    connectionAttempt.current = session;
    store.session = session;
    try {
      await session.connect(resumeOnly);
    } catch (error) {
      fail(error);
    } finally {
      if (connectionAttempt.current === session) {
        setOnline(false);
        setConnecting(false);
        setSign(null);
        setSignBusy(false);
        store.session = null;
        // OpenSSH offered every identity and the server accepted none, so
        // no signature was requested: no candidate is authorized.
        if (
          progress.listed &&
          !progress.signed &&
          !progress.welcomed &&
          !progress.canceled
        )
          setError(
            `This security key is not authorized for ${connection.username}. Choose Register a new key, then add its public key to the account's SSH authorization.`,
          );
      }
    }
  }
  function popout(pane: Pane) {
    const url = new URL(location.href);
    url.search = "";
    url.searchParams.set("pane", String(pane.id));
    window.open(url, "_blank", "popup,noopener,width=1100,height=760");
  }
  function close(p: Pane) {
    setConfirmAction({
      title: "Close terminal",
      text: `End the shell and running jobs in “${p.title || "shell"}”? Every client showing it, the TUI included, loses it.`,
      buttons: [
        {
          label: "Close terminal",
          color: "danger",
          action: async () => {
            // A late focus/resize/input event must not reclaim this pane
            // between releasing its lease and the confirmed close request.
            store.closing!.add(p.id);
            let closed = false;
            try {
              if (
                store.session &&
                store.leases.get(p.id) === store.session.client
              ) {
                await new Promise<void>((resolve, reject) => {
                  const done = () => {
                    if (store.leases.get(p.id) !== store.session?.client) {
                      clearTimeout(timer);
                      store.events.removeEventListener("frame", done);
                      resolve();
                    }
                  };
                  const timer = setTimeout(() => {
                    store.events.removeEventListener("frame", done);
                    reject(
                      Error(
                        "Could not release control; the terminal was not closed.",
                      ),
                    );
                  }, 3000);
                  store.events.addEventListener("frame", done);
                  store.session!.request("release", { pane: p.id });
                });
              }
              closed = !!(await rpc("close", { pane: p.id, confirmed: true }));
              if (closed) toast(`Closed “${p.title || "shell"}”`);
            } catch (error) {
              fail(error);
            } finally {
              if (!closed) store.closing!.delete(p.id);
            }
          },
        },
      ],
    });
  }
  async function newTab() {
    const result = await rpc("new", { request: crypto.randomUUID() });
    if (result?.pane) {
      if (view === "grid" && !popupPane) {
        setActive(result.pane);
        setPage(Math.floor(live.length / PAGE));
      } else activate(result.pane);
    }
  }
  function move(source: number, before: number | null) {
    setOrder(
      moveBefore(
        live.map((p) => p.id),
        source,
        before,
      ),
    );
    setDrag(null);
    setDrop(null);
  }
  function dragProps(p: Pane) {
    return {
      onPointerDown: (e: React.PointerEvent) => {
        // A tab's own open button starts a drag; its other buttons do not.
        if (
          e.button !== 0 ||
          (e.target as HTMLElement).closest("button:not(.term-tab-open)")
        )
          return;
        dragCleanup.current?.();
        const state = {
          source: p.id,
          before: p.id as number | null,
          startX: e.clientX,
          startY: e.clientY,
          moving: false,
        };
        const cleanup = () => {
          window.removeEventListener("pointermove", pointerMove);
          window.removeEventListener("pointerup", pointerUp);
          window.removeEventListener("pointercancel", cancel);
          window.removeEventListener("blur", cancel);
          dragCleanup.current = null;
        };
        const cancel = () => {
          cleanup();
          setDrag(null);
          setDrop(null);
        };
        const pointerMove = (event: PointerEvent) => {
          if (
            !state.moving &&
            Math.hypot(
              event.clientX - state.startX,
              event.clientY - state.startY,
            ) < 6
          )
            return;
          event.preventDefault();
          if (!state.moving) {
            state.moving = true;
            setDrag(state.source);
            setDrop(state.source);
          }
          const target = document
            .elementFromPoint(event.clientX, event.clientY)
            ?.closest<HTMLElement>(".term-tab[data-drop-pane]");
          const targetId = Number(target?.dataset.dropPane);
          if (target && targetId && targetId !== state.source) {
            const rect = target.getBoundingClientRect();
            const after = event.clientX >= rect.left + rect.width / 2;
            const remaining = live.filter((pane) => pane.id !== state.source);
            const index = remaining.findIndex((pane) => pane.id === targetId);
            state.before = after ? remaining[index + 1]?.id || null : targetId;
            setDrop(state.before);
          } else if (
            !target &&
            (event.target as HTMLElement).closest?.(".term-tabs")
          ) {
            state.before = null;
            setDrop(null);
          }
        };
        const pointerUp = () => {
          cleanup();
          if (state.moving) {
            suppressClick.current = performance.now() + 300;
            move(state.source, state.before);
          }
        };
        dragCleanup.current = cancel;
        window.addEventListener("pointermove", pointerMove, { passive: false });
        window.addEventListener("pointerup", pointerUp);
        window.addEventListener("pointercancel", cancel);
        window.addEventListener("blur", cancel);
      },
    };
  }
  const ordered =
    drag !== null && drag !== drop
      ? orderPanes(
          live,
          moveBefore(
            live.map((p) => p.id),
            drag,
            drop,
          ),
        )
      : live;
  const gridPanes = ordered.slice(page * PAGE, page * PAGE + PAGE);
  const cols = gridColumns(gridPanes.length, gridWidth);
  const rows = Math.max(1, Math.ceil(gridPanes.length / cols));
  const keySettings = (mode: "register" | "settings") => (
    <KeySettings
      mode={mode}
      online={online}
      signedInKey={signedInKey}
      knownKeys={knownKeys}
      identity={identity}
      connection={connection}
      connecting={connecting}
      error={error}
      setError={setError}
      fail={fail}
      keyBusy={keyBusy}
      setKeyBusy={setKeyBusy}
      keySelected={keySelected}
      setKeySelected={setKeySelected}
      shownKey={shownKey}
      setShownKey={setShownKey}
      keyToDelete={keyToDelete}
      setKeyToDelete={setKeyToDelete}
      savedIdentities={savedIdentities}
      saveIdentity={saveIdentity}
      updateKey={updateKey}
      syncKey={() => {}}
      preferKey={preferKey}
      deleteKey={deleteKey}
    />
  );
  const keyManagement = keyModal && (
    <div
      className="modal-shade"
      onClick={(event) => {
        if (event.target === event.currentTarget && !keyBusy)
          setKeyModal(false);
      }}
    >
      <div
        className="modal key-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="key-modal-title"
      >
        <header>
          <h2 id="key-modal-title">Register a new key</h2>
          <button
            className="icon-btn"
            aria-label="Close key registration"
            disabled={keyBusy}
            onClick={() => setKeyModal(false)}
          >
            <Icon name="x" />
          </button>
        </header>
        {keySettings("register")}
        <div className="key-modal-actions">
          <button disabled={keyBusy} onClick={() => setKeyModal(false)}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
  if (!online && popupPane && !popupFallback && !error && !sign && !keyModal)
    return <PopupConnecting onFallback={() => setPopupFallback(true)} />;
  if (!online)
    return (
      <LoginScreen
        connection={connection}
        remembered={identity}
        error={error}
        status={status}
        connecting={connecting}
        keyBusy={keyBusy}
        keyModal={keyModal}
        keyManagement={keyManagement}
        sign={sign}
        signBusy={signBusy}
        onSignIn={() => void (identity ? connect(false, identity) : discover())}
        onDiscover={() => void discover()}
        onRegister={() => {
          setError("");
          setKeySelected(false);
          setKeyModal(true);
        }}
        onCancelConnection={() => {
          authProgress.current.canceled = true;
          sign?.cancel();
          connectionAttempt.current?.close();
          keyDiscovery.current?.abort();
          setConnecting(false);
        }}
        onRetrySign={() => {
          setError("");
          sign?.run();
        }}
        onCancelSign={() => {
          sign?.cancel();
          setSign(null);
        }}
      />
    );
  const bare = !!popupPane;
  function focusTerminal() {
    if (!overlayOpen)
      requestAnimationFrame(() =>
        store.events.dispatchEvent(new Event("terminal-focus")),
      );
  }
  const paneLabel = (p: Pane) => p.title || "shell";
  const paneHeader = (p: Pane) => (
    <header>
      <PaneLabel pane={p} />
      <span className="grid-folder" title={p.cwd}>
        {basename(p.cwd)}
      </span>
      <div className="pane-actions">
        <button
          type="button"
          className="icon-btn"
          title="Find (Ctrl/Cmd+F)"
          aria-label={`Find in ${paneLabel(p)}`}
          onClick={(event) => {
            event.stopPropagation();
            openFind(p.id);
          }}
        >
          <Icon name="text-search" size="sm" />
        </button>
        {!bare && (
          <button
            type="button"
            className="icon-btn"
            title="Open in a window"
            aria-label={`Open ${paneLabel(p)} in a window`}
            onClick={(event) => {
              event.stopPropagation();
              popout(p);
            }}
          >
            <Icon name="external-link" size="sm" />
          </button>
        )}
        <button
          type="button"
          className="icon-btn"
          title="Close"
          aria-label={`Close ${paneLabel(p)}`}
          onClick={(event) => {
            event.stopPropagation();
            close(p);
          }}
        >
          <Icon name="x" size="sm" />
        </button>
      </div>
    </header>
  );
  const owner = focused ? store.leases.get(focused.id) : undefined;
  const hint = focused
    ? owner === store.session?.client
      ? "This browser controls the focused terminal's input and size"
      : owner
        ? "Another browser controls this terminal; type or click to take it"
        : "Typing here takes this terminal from the TUI; typing there takes it back"
    : "Live grid · click a terminal to open it";
  return (
    <div className={`kelpmux-shell ${bare ? "bare" : ""}`}>
      <div
        className={`app ${view} ${popupPane ? "popup" : ""}`}
        onClick={focusTerminal}
      >
        {!bare && (
          <TopBar
            workspace={workspace}
            dev={!!connection?.dev}
            terminals={ordered}
            current={focused?.id || active}
            mode={view}
            tabProps={dragProps}
            tabClass={(p) => (drag === p.id && drag !== drop ? "landing" : "")}
            onMode={chooseMode}
            onTerminal={(p) => {
              if (performance.now() < suppressClick.current) return;
              if (view === "grid") focusInGrid(p);
              else activate(p.id);
            }}
            onClose={close}
            onNewTerminal={newTab}
            onStats={() => setDialog("stats")}
            onSettings={() => setDialog("config")}
          />
        )}
        {error && (
          <div className="notice error compact" role="alert">
            {error}
            <button aria-label="Dismiss error" onClick={() => setError("")}>
              <Icon name="x" size="sm" />
            </button>
          </div>
        )}
        <div className="workspace" ref={gridRef}>
          {focused ? (
            <div className="pane-grid tab-pane-layout">
              <section
                key={focused.id}
                className={`grid-pane first-row selected ${focused.status === "input_needed" ? "needs-input" : ""}`}
              >
                {paneHeader(focused)}
                <Terminal
                  pane={focused}
                  store={store}
                  active
                  layout="tabs"
                  onActivate={() => setActive(focused.id)}
                  onError={fail}
                  find={findPane === focused.id}
                  onCloseFind={() => setFindPane(0)}
                />
              </section>
            </div>
          ) : view === "grid" && live.length ? (
            <div
              className="pane-grid live-grid"
              aria-label="Terminal grid"
              style={{
                gridTemplateColumns: `repeat(${cols},minmax(0,1fr))`,
                gridTemplateRows: `repeat(${rows},minmax(0,1fr))`,
              }}
            >
              {Array.from({ length: cols * rows }, (_, i) => {
                const p = gridPanes[i];
                return p ? (
                  <button
                    type="button"
                    key={p.id}
                    className={`grid-pane live-tile ${p.status === "input_needed" ? "needs-input" : ""} ${p.id === active ? "focused" : ""}`}
                    aria-current={p.id === active ? "true" : undefined}
                    aria-label={`Open terminal ${paneLabel(p)} (${stateLabel[p.status] || p.status})`}
                    data-tile-pane={p.id}
                    data-state={p.status}
                    onClick={() => activate(p.id)}
                  >
                    <header>
                      <PaneLabel pane={p} />
                      <span className="grid-folder">{basename(p.cwd)}</span>
                    </header>
                    <TerminalTile pane={p} store={store} />
                  </button>
                ) : i === gridPanes.length ? (
                  <button
                    type="button"
                    className="inactive-pane new-pane"
                    key={`new-${i}`}
                    onClick={newTab}
                  >
                    <Icon name="plus" size="lg" />
                    New terminal
                  </button>
                ) : (
                  <div className="inactive-pane" key={`empty-${i}`} />
                );
              })}
            </div>
          ) : panesLoaded ? (
            <div className="empty-state">
              <Icon name="terminal" size="lg" />
              <h3>{live.length ? "Choose a terminal" : "No open terminals"}</h3>
              <button onClick={newTab}>
                <Icon name="plus" size="sm" />
                New terminal
              </button>
            </div>
          ) : null}
          {view === "grid" && !focused && live.length > PAGE && (
            <div className="pagination grid-pages">
              <button
                className="icon-btn"
                aria-label="Previous page"
                disabled={!page}
                onClick={() => setPage(page - 1)}
              >
                <Icon name="chevron-right" size="sm" className="flip" />
              </button>
              {page + 1}/{Math.ceil(live.length / PAGE)}
              <button
                className="icon-btn"
                aria-label="Next page"
                disabled={(page + 1) * PAGE >= live.length}
                onClick={() => setPage(page + 1)}
              >
                <Icon name="chevron-right" size="sm" />
              </button>
            </div>
          )}
        </div>
        {!bare && (
          <StatusBar
            username={connection?.username || ""}
            workspace={workspace}
            count={live.length}
            vitals={vitals}
            hint={hint}
            onStats={() => setDialog("stats")}
          />
        )}
        {keyManagement}
        {dialog && (
          <div
            className={`modal-shade ${dialog === "config" ? "settings-shade" : ""}`}
            onMouseDown={(e) => {
              if (e.target === e.currentTarget) setDialog("");
            }}
          >
            <section
              className={`modal ${dialog === "stats" ? "stats-modal" : ""} ${dialog === "config" ? "config-modal" : ""}`}
              role="dialog"
              aria-modal="true"
              aria-labelledby="modal-title"
            >
              <header>
                <h2 id="modal-title">
                  {{ config: "Settings", stats: "System status" }[dialog]}
                </h2>
                <button
                  className="icon-btn"
                  aria-label="Close dialog"
                  onClick={() => {
                    setDialog("");
                    setQuery("");
                  }}
                >
                  <Icon name="x" />
                </button>
              </header>
              {dialog === "config" && (
                <ConfigPanel
                  store={store}
                  connection={connection}
                  appearance={appearance}
                  setAppearance={setAppearance}
                  section={settingsSection}
                  setSection={setSettingsSection}
                  confirmClose={confirmClose}
                  setConfirmClose={setConfirmClose}
                  keyContent={keySettings("settings")}
                />
              )}
              {dialog === "stats" && (
                <Stats
                  stats={stats}
                  vitals={vitals}
                  query={query}
                  setQuery={setQuery}
                />
              )}
            </section>
          </div>
        )}
        {confirmAction && (
          <div className="modal-shade confirmation">
            <section className="confirm-dialog" role="alertdialog">
              <h2>{confirmAction.title}</h2>
              <p>{confirmAction.text}</p>
              <div className="confirm-buttons">
                <button
                  className="subtle"
                  disabled={confirmPending}
                  onClick={() => setConfirmAction(null)}
                >
                  Cancel
                </button>
                {confirmAction.buttons.map((b, index) => (
                  <button
                    key={b.label}
                    className={b.color || ""}
                    disabled={confirmPending}
                    onClick={() => approve(index)}
                  >
                    {b.label}
                  </button>
                ))}
              </div>
            </section>
          </div>
        )}
      </div>
    </div>
  );
}
createRoot(document.getElementById("root")!).render(
  <ToastProvider>
    <App />
  </ToastProvider>,
);
