// KelpMux web client: terminals only. It signs in with a security key
// through browser-terminated OpenSSH, attaches to one KelpServer workspace
// through `kelpmux web-bridge`, and shows the workspace's panes as tabs or a
// read-only live grid. Tabs, grid and focus are this browser's own layout;
// the TUI and other browsers see the same panes and processes with their
// own layouts.
import React, { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  discoverKey,
  keyFingerprint,
  registeredKeys,
} from "@kelpcore/web/connection.ts";
import { SharedSession as Session } from "@kelpcore/web/shared-session.ts";
import { Terminal, TerminalTile, type Store } from "@kelpcore/web/Terminal.tsx";
import type {
  Connection,
  Identity,
  NumberedPane,
  Pane,
  SignInKey,
} from "@kelpcore/web/types.ts";
import { LiveGrid } from "./LiveGrid";
import { trimScreens } from "@kelpcore/web/screen-cache.ts";
import "./styles/base.css";
import "./styles/shell.css";
import "@kelpcore/web/terminal.css";
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
import "./styles/state.css";
import "./styles/mobile.css";
import { ToastProvider, useToast } from "./Toast";
import { startErrorReporting } from "./errors";
import { activityIcon } from "./activity-icon";
import { basename } from "./format";
import { Stats } from "./Stats";
import { KeySettings } from "./KeySettings";
import { LoginScreen, PopupConnecting, StoppedWorkspace } from "./Login";
import { ConfigPanel } from "./ConfigPanel";
import {
  readSaveResult,
  readUserConfig,
  type ConfigChanges,
  type UserConfigState,
} from "./user-config";
import {
  IdleWarning,
  useActivityReports,
  type IdleNotice,
} from "./IdleWarning";
import { OPEN_SETTINGS } from "./Settings";
import {
  NotificationsButton,
  PaneLabel,
  UnreadMark,
  type SessionMode,
} from "./Shell";
import { Sidebar } from "./Sidebar";
import {
  HelpPanel,
  MovePanel,
  TerminalNotificationsPanel,
  PaneMenu,
  ParkedPanel,
  RecoveryPanel,
  SearchPanel,
} from "./Panels";
import { TerminalsPanel } from "./Terminals";
import type { TerminalFilter } from "./state/terminals";
import { Icon } from "./status";
import { Tooltip } from "./Tooltip";
import {
  SIDEBAR_MAX,
  SIDEBAR_MIN,
  loadLayout,
  loadPreferences,
  loadView,
  moveBefore,
  moveBy,
  numberPanes,
  orderPanes,
  saveLayout,
  savePreferences,
  saveView,
} from "./layout-store";
import {
  isFinished,
  needsAttention,
  nextPane,
  notificationMatches,
  paneState,
  readingPane,
  rollup,
  stateLabel,
  tabTitle,
  unreadFor,
  type DisplayState,
  type Notification,
} from "./state/model";
import { useReadShown } from "./state/read";
import { prefixAction } from "./prefix";
import {
  KeyBar,
  PhoneBar,
  useDevice,
  useHardwareKeys,
  useVisualViewport,
} from "./Mobile";
import { sidebarFolded } from "./device";
import {
  dialogFromParam,
  locationOf,
  useHistorySync,
  type AppLocation,
} from "./navigation";
import {
  barKeyBytes,
  consumesCtrl,
  nextInputMode,
  type BarKey,
  type HardwareKeyboard,
  type InputMode,
} from "@kelpcore/web/touch-keys.ts";
import { chime } from "./alerts";
import { AttentionToasts, useAttentionToasts } from "./AttentionToasts";
import { bellRinging } from "./toasts";
import {
  UploadDialog,
  dragHasFiles,
  droppedFiles,
  loadUploadDestination,
  saveUploadDestination,
  type UploadDrop,
} from "./Upload";
import {
  pastedPaths,
  readUploadLimits,
  type UploadLimits,
  type UploadTransport,
} from "@kelpcore/web/uploads.ts";
import {
  FilesClient,
  isFileEvent,
  readFileLimits,
  type FileLimits,
  type FilesTransport,
} from "@kelpcore/web/files/api.ts";
import {
  baseName,
  isInside,
  parentPath,
  typedPaths,
} from "@kelpcore/web/files/paths.ts";
import { FilesBrowser } from "./files/FilesBrowser";
import type { FileActionsProps } from "./files/actions";
import { FileEditor } from "./files/FileEditor";
import { FileTabs } from "./files/FileTabs";
import { neighbourTab } from "./files/tabs";
import { Downloads } from "./files/downloads";
import { SidePanel } from "./files/SidePanel";
import { usePathLinks, type PathTarget } from "./files/PathLinks";
import { useDocuments } from "./files/documents";
import { outsideRootMessage, withinRoot } from "./files/root";
import { dragHasPaths, droppedPaths, typeIntoPane } from "./files/insert";
import { ConfirmDialog, type ConfirmAction } from "./Confirm";
import "./styles/files.css";
import "./styles/titlebar.css";
import { TitleBar } from "./TitleBar";
import { titleLocation } from "./title-location";

/** A position in a file an editor jumps to. */
type FileJump = { line: number; col?: number; seq: number } | undefined;

/** Subscriptions per connection when the server's Welcome does not
 * advertise `limits` (servers before `stream-limits`). */
const DEFAULT_SUBSCRIPTIONS = 10;
/** Subscriptions left for popup windows sharing the connection: the grid
 * streams at most the server's limit less these. */
const POPUP_SUBSCRIPTIONS = 2;
/** The `Web` request API this client speaks (see `kelpmux web-bridge`). */
const API_VERSION = 3;
/** How often a window asks the bridge for the user configuration. */
const CONFIG_REFRESH_MS = 5000;

const store: Store = {
  screens: new Map(),
  events: new EventTarget(),
  leases: new Map(),
  closing: new Set(),
  session: null,
};
/** Upload requests go out on the shared stream; their events arrive as
 * `upload-event`s on the store (see `event` in App). */
const uploadTransport: UploadTransport = {
  send(type, params) {
    if (!store.session) throw Error("The SSH connection is closed");
    store.session.request(type, params);
  },
  listen(handler) {
    const listener = (event: Event) => handler((event as CustomEvent).detail);
    store.events.addEventListener("upload-event", listener);
    return () => store.events.removeEventListener("upload-event", listener);
  },
};
/** File requests (KelpServer `files`) go out on the shared stream; their
 * replies arrive as `file-event`s on the store (see `event` in App). */
const filesTransport: FilesTransport = {
  send(type, params) {
    if (!store.session) throw Error("The SSH connection is closed");
    store.session.request(type, params);
  },
  listen(handler) {
    const listener = (event: Event) => handler((event as CustomEvent).detail);
    store.events.addEventListener("file-event", listener);
    return () => store.events.removeEventListener("file-event", listener);
  },
};
/** Stream events that belong to an upload (and its correlated replies). */
function isUploadEvent(e: any) {
  return (
    typeof e.type === "string" &&
    (e.type.startsWith("upload_") ||
      (["done", "error"].includes(e.type) &&
        typeof e.request === "string" &&
        e.request.startsWith("upload-")))
  );
}
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
  /** The bridge reported this workspace's server stopped (its name). */
  const [stopped, setStopped] = useState(""),
    [starting, setStarting] = useState(false);
  /** The shared connection's idle warning (IdleWarning.tsx). */
  const [idle, setIdle] = useState<IdleNotice | null>(null);
  const reportActivity = () => {
    if (store.session instanceof Session) store.session.activity();
  };
  useActivityReports(online ? reportActivity : null, !!idle);
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
    [savedView, setView] = useState<SessionMode>(() =>
      popupPane ? "single" : loadView(),
    ),
    [dialog, setDialog] = useState(""),
    [workspace, setWorkspace] = useState(""),
    [epoch, setEpoch] = useState(""),
    [order, setOrder] = useState<number[]>([]),
    [vitals, setVitals] = useState<any>({}),
    [stats, setStats] = useState<any>({}),
    /** Mount points of the vitals' disks, from the status request. */
    [diskMounts, setDiskMounts] = useState<string[]>([]),
    [query, setQuery] = useState(""),
    /** Panes one connection may subscribe to (Welcome `limits`). */
    [subscriptionLimit, setSubscriptionLimit] = useState(DEFAULT_SUBSCRIPTIONS),
    [notifications, setNotifications] = useState<Notification[]>([]),
    [parked, setParked] = useState<number[]>([]),
    [preferences, setPreferences] = useState(loadPreferences),
    [terminalFilter, setTerminalFilter] = useState<TerminalFilter>("all"),
    [operations, setOperations] = useState(() => new Set<string>()),
    // The user configuration (config.toml) through the bridge's `config`.
    [userConfig, setUserConfig] = useState<UserConfigState | null>(null),
    // The terminal whose notifications the Notifications panel shows.
    [notifyPane, setNotifyPane] = useState(0),
    [findQuery, setFindQuery] = useState(""),
    [menu, setMenu] = useState<{
      pane: NumberedPane;
      x: number;
      y: number;
    } | null>(null);
  /** Files (KelpServer `files`, relayed by bridges of API minor 2): the
   * server's capability and limits, the client over this connection, the
   * server user's home folder, the browser's folder, the open file shown
   * (none: the browser itself) and the terminal row under dragged paths. */
  const [serverFiles, setServerFiles] = useState(false),
    [fileLimits, setFileLimits] = useState<FileLimits | null>(null),
    [filesClient, setFilesClient] = useState<FilesClient | null>(null),
    [home, setHome] = useState<string | undefined>(undefined),
    [filesDir, setFilesDir] = useState("~"),
    [shownDoc, setShownDoc] = useState(""),
    [pathDropPane, setPathDropPane] = useState(0),
    /** A position the main area's editor jumps to (a path link). */
    [mainJump, setMainJump] = useState<FileJump>(),
    /** The file in the side panel beside the terminal shown alone (by
     * document identity, so it follows renames), and where it jumps. */
    [side, setSide] = useState<{ id: number; jump?: FileJump } | null>(null);
  const jumps = useRef(0);
  const filesOffered = online && serverFiles && operations.has("files");
  /** The folder file operations are confined to (Welcome
   * `file_limits.root`; none from servers before it). */
  const filesRoot = filesClient?.limits.root;
  // An older bridge (API minor 2) relays files but not downloads.
  const relaysDownloads = operations.has("downloads");
  useEffect(() => {
    if (!filesOffered || !fileLimits) {
      setFilesClient(null);
      return;
    }
    const client = new FilesClient(filesTransport, {
      limits: fileLimits,
      downloads: relaysDownloads,
    });
    setFilesClient(client);
    client
      .stat("~")
      .then((result) => setHome(result.path))
      .catch(() => {});
    return () => client.close();
  }, [filesOffered, fileLimits, relaysDownloads]);
  const documents = useDocuments(filesClient);
  const unsavedFiles = documents.docs.some((doc) => doc.dirty);
  useEffect(() => {
    if (!unsavedFiles) return;
    const warn = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = true;
    };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [unsavedFiles]);
  // Phones and tablets (device.ts). A phone shows the terminal list, or one
  // terminal full screen; it has no grid. Touch devices show output without
  // the on-screen keyboard until the user enters input mode.
  const device = useDevice();
  const phone = device.mode === "phone" && !popupPane,
    touch = device.touch;
  // The file browser shows only on a connection that offers files.
  const view: SessionMode =
    savedView === "files"
      ? filesClient
        ? "files"
        : "single"
      : phone
        ? "single"
        : savedView;
  /** The phone shows its terminal list (its home screen). */
  /** Uploads: the server's capability and limits (from its Welcome), the
   * drop being reviewed or uploaded, the terminal under dragged files and
   * this browser's default destination folder. */
  const [serverUploads, setServerUploads] = useState(false),
    [uploadLimits, setUploadLimits] = useState<UploadLimits | null>(null),
    [uploadDrop, setUploadDrop] = useState<UploadDrop | null>(null),
    [dropPane, setDropPane] = useState(0),
    [uploadDestination, setUploadDestination] = useState(loadUploadDestination);
  const filePicker = useRef<HTMLInputElement>(null),
    pickerPane = useRef<NumberedPane | null>(null);
  const [phoneList, setPhoneList] = useState(true);
  const [inputMode, setInputMode] = useState<InputMode>("reading"),
    [ctrlArmed, setCtrlArmed] = useState(false),
    [prefixArmed, setPrefixArmed] = useState(false);
  /** A tablet in portrait starts with the sidebar folded, each time. */
  const [portraitFolded, setPortraitFolded] = useState(true);
  const tabletPortrait =
    device.mode === "tablet" && device.orientation === "portrait";
  useEffect(() => {
    if (tabletPortrait) setPortraitFolded(true);
  }, [tabletPortrait]);
  const folded = sidebarFolded(
    device.mode,
    device.orientation,
    preferences.sidebarFolded,
    portraitFolded,
  );
  function toggleFold() {
    if (tabletPortrait) setPortraitFolded(!folded);
    else
      setPreferences((current) => ({
        ...current,
        sidebarFolded: !current.sidebarFolded,
      }));
  }
  useEffect(() => savePreferences(preferences), [preferences]);
  useVisualViewport(touch && online);
  const notificationsRef = useRef(notifications);
  notificationsRef.current = notifications;
  /** Notifications this browser already acknowledged. */
  const acknowledged = useRef(new Set<number>());
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
  const [confirmAction, setConfirmAction] = useState<ConfirmAction | null>(
      null,
    ),
    [drag, setDrag] = useState<number | null>(null),
    [drop, setDrop] = useState<number | null>(null);
  const metadata = useRef(""),
    connectionAttempt = useRef<Session | null>(null);
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
  const overlayOpen =
    !!dialog ||
    !!confirmAction ||
    keyModal ||
    !!findPane ||
    !!menu ||
    !!uploadDrop;
  useEffect(() => {
    if (overlayWasOpen.current && !overlayOpen)
      requestAnimationFrame(() =>
        store.events.dispatchEvent(new Event("terminal-focus")),
      );
    overlayWasOpen.current = overlayOpen;
  }, [overlayOpen]);
  useEffect(() => () => dragCleanup.current?.(), []);
  useEffect(() => {
    // Files dropped outside a terminal must not make the browser open them
    // in place of the client; only terminals accept them.
    const outside = (event: DragEvent) => {
      if (!dragHasFiles(event)) return;
      event.preventDefault();
      const target = event.target as Element | null;
      if (
        event.dataTransfer &&
        !target?.closest?.("[data-upload-pane], [data-upload-folder]")
      )
        event.dataTransfer.dropEffect = "none";
      if (event.type === "drop") setDropPane(0);
    };
    const ended = () => {
      setDropPane(0);
      setPathDropPane(0);
    };
    window.addEventListener("dragover", outside);
    window.addEventListener("drop", outside);
    window.addEventListener("dragend", ended);
    return () => {
      window.removeEventListener("dragover", outside);
      window.removeEventListener("drop", outside);
      window.removeEventListener("dragend", ended);
    };
  }, []);
  const fail = (e: any) => setError(e?.message || String(e));
  async function rpc(method: string, params: any = {}) {
    try {
      return await store.session?.rpc(method, params);
    } catch (error) {
      fail(error);
      return null;
    }
  }
  /** A background request whose failure only leaves a view plain. */
  async function quietRpc(method: string, params: any = {}) {
    try {
      return await store.session?.rpc(method, params);
    } catch {
      return null;
    }
  }
  /** Saves Settings → Files & folders through the bridge, which validates
   * and writes config.toml. */
  async function saveUserConfig(changes: ConfigChanges) {
    const result = readSaveResult(await rpc("config_set", { changes }));
    if (result?.config) setUserConfig(result.config);
    return result;
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
        startErrorReporting(cfg);
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

  // This browser's layout: order, parking and focus, per
  // workspace and epoch. `all` is every pane in this browser's order; `live` the tabs and
  // grid (parked excluded).
  const all = numberPanes(orderPanes(panes, order));
  // Read by pasteUploaded after an upload finishes; declared here, above
  // the early returns, so the hook order never changes between renders.
  const panesNow = useRef(all);
  panesNow.current = all;
  const parkedSet = new Set(parked);
  const live = all.filter((p) => !parkedSet.has(p.id));
  const parkedPanes = all.filter((p) => parkedSet.has(p.id));
  // Navigation history (navigation.ts): the place shown, mirrored into the
  // URL; Back, Forward and a loaded URL apply theirs. Popup windows keep
  // none. Declared before the effects that call `navigation.automatic()`.
  const shownTerminal = live.find((p) => p.id === active);
  const sideDoc = side
    ? documents.docs.find((doc) => doc.id === side.id)
    : undefined;
  const mainDoc =
    view === "files"
      ? documents.docs.find(
          (doc) => doc.path === shownDoc && doc.id !== side?.id,
        )
      : undefined;
  const sideVisible =
    !!sideDoc && !phone && view === "single" && !!shownTerminal;
  const here = popupPane
    ? null
    : locationOf({
        phoneHome: phone && (phoneList || (!shownTerminal && view !== "files")),
        view,
        terminal: shownTerminal?.id || 0,
        filesDir,
        file: mainDoc?.path || "",
        fileLine: mainJump?.line,
        side: sideVisible ? sideDoc.path : "",
        sideLine: side?.jump?.line,
        dialog,
        notifyPane,
      });
  const navigation = useHistorySync({
    here,
    ready:
      online &&
      panesLoaded &&
      // Files are known: not offered, or their client is ready.
      (!(filesOffered && fileLimits) || !!filesClient),
    context: phone ? "phone" : "wide",
    apply: applyLocation,
  });
  /** Shows a location from the history or the loaded URL. What no longer
   * exists falls back (a closed terminal to the grid or the phone's list,
   * a missing file to its folder) and replaces the entry. */
  function applyLocation(target: AppLocation) {
    setMenu(null);
    const opened = dialogFromParam(target.dialog);
    setDialog(opened);
    if (!opened) setQuery("");
    if (target.of) setNotifyPane(target.of);
    const jump = (line: number): FileJump =>
      line ? { line, seq: ++jumps.current } : undefined;
    /** The side panel's file; one with unsaved changes stays (its buffer
     * lives in the panel's editor). */
    const placeSide = (path: string, line: number) => {
      if (sideDoc?.path === path && !line) return;
      if (sideDoc?.dirty && sideDoc.path !== path) return;
      // A file outside the file root is not opened.
      if (path && !withinRoot(path, filesRoot, home)) {
        toast(outsideRootMessage, { key: "navigation" });
        navigation.automatic();
        path = "";
      }
      if (!path || phone || !filesClient) {
        if (sideDoc) setSide(null);
        return;
      }
      const id = documents.open(path);
      if (shownDoc === path) setShownDoc("");
      setSide({ id, jump: jump(line) });
    };
    switch (target.view) {
      case "home":
        // A wider screen has no list: it keeps its place.
        if (phone) {
          setPhoneList(true);
          setFindPane(0);
        }
        return;
      case "grid":
        // A phone has no grid: its list is the overview.
        if (phone) setPhoneList(true);
        else setView("grid");
        return;
      case "terminal": {
        const id = target.terminal;
        if (id && !panesRef.current.some((p) => p.id === id)) {
          toast(`Terminal ${id} is closed`, { key: "navigation" });
          navigation.automatic();
          if (phone) setPhoneList(true);
          else setView("grid");
          return;
        }
        setPhoneList(false);
        setView("single");
        if (id) {
          if (parked.includes(id)) setParked(parked.filter((p) => p !== id));
          if (id !== active) setFindPane(0);
          setActive(id);
        }
        placeSide(target.side, target.line);
        return;
      }
      case "files": {
        if (!filesClient) {
          toast("Files are not available on this connection", {
            key: "navigation",
          });
          navigation.automatic();
          return;
        }
        setPhoneList(false);
        setView("files");
        const path = target.file;
        // A folder or file outside the file root: the root instead.
        if (filesRoot && !withinRoot(path || target.dir || "~", filesRoot, home)) {
          toast(outsideRootMessage, { key: "navigation" });
          navigation.automatic();
          setShownDoc("");
          setFilesDir(filesRoot);
          return;
        }
        if (!path) {
          setShownDoc("");
          if (target.dir) setFilesDir(target.dir);
          return;
        }
        if (sideDoc?.path === path) {
          if (sideDoc.dirty) return;
          setSide(null);
        }
        const opened = documents.docs.some((doc) => doc.path === path);
        documents.open(path);
        setShownDoc(path);
        setMainJump(jump(target.line));
        if (opened) return;
        // A file that no longer exists shows its folder instead.
        const shown = (folder: string) => {
          navigation.automatic();
          documents.close(path);
          setShownDoc((current) => (current === path ? "" : current));
          setFilesDir(folder);
        };
        filesClient.stat(path).then(
          (result) => {
            if (result.entry.kind === "dir") shown(result.path);
          },
          () => {
            toast(`“${baseName(path)}” is not available`, {
              key: "navigation",
            });
            shown(parentPath(path));
          },
        );
        return;
      }
    }
  }
  useEffect(() => {
    if (!online || !panesLoaded || !workspace || !epoch || popupPane) return;
    saveLayout(workspace, {
      epoch,
      order: all.map((p) => p.id),
      focus: active,
      parked,
    });
  }, [
    online,
    workspace,
    epoch,
    active,
    all.map((p) => p.id).join(","),
    parked.join(","),
  ]);
  useEffect(() => {
    // Closed terminals leave this browser's parked list.
    if (panesLoaded && parked.some((id) => !panes.some((p) => p.id === id)))
      setParked(parked.filter((id) => panes.some((p) => p.id === id)));
  }, [panes, panesLoaded]);
  useEffect(() => {
    if (panesLoaded && !popupPane && !live.some((p) => p.id === active)) {
      // A closed or parked terminal: its entry becomes the next one's.
      navigation.automatic();
      setActive(live[0]?.id || 0);
    }
  }, [panes, order, panesLoaded, parked.join(",")]);

  // The grid shows every live terminal; it streams as many as the server
  // allows one connection, less room for popups. Tiles beyond that (only
  // with a server that allows few) show their header without content.
  const streamed = Math.max(1, subscriptionLimit - POPUP_SUBSCRIPTIONS);
  const visible = popupPane
      ? all.filter((p) => p.id === popupPane)
      : (phone && phoneList) || view === "files"
        ? []
        : view === "single"
          ? live.filter((p) => p.id === active)
          : live.slice(0, streamed),
    visibleKey = visible.map((p) => p.id).join(",");
  // A ringing bell marks the favicon until it is read (Bell event on).
  const browserActivity = activityIcon(
    online ? panes : null,
    online && bellRinging(notifications, panes, preferences.notificationEvents),
  );
  const attention = rollup(
    panes.filter((p) => !p.dead),
    notifications,
  );
  const attentionTotal =
    (attention.counts.input_needed || 0) +
    (attention.counts.failed || 0) +
    (attention.counts.finished || 0);
  useEffect(() => {
    const icon = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
    if (icon) icon.href = browserActivity.url;
    document.title = tabTitle(attentionTotal);
  }, [browserActivity.url, attentionTotal]);
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
    if (online)
      store.session?.request("subscribe", {
        panes: visible.map((p) => p.id),
      });
  }, [online, visibleKey]);
  useEffect(() => {
    if (!connection || resumed.current || !globalThis.SharedWorker) return;
    resumed.current = true;
    if (identity && !keyModal) void connect(true);
  }, [connection, identity]);
  // The vitals carry disk percentages only; the status request has the
  // same disks, in the same order, with their mount points.
  const vitalDisks: number = (vitals.disks || []).length;
  const knownMounts = diskMounts.length === vitalDisks ? diskMounts : [];
  useEffect(() => {
    const mounts = (stats.disks || []).map((d: any) => String(d.mount));
    if (mounts.length) setDiskMounts(mounts);
  }, [stats]);
  // The user configuration, from a bridge of API minor 4. The bridge sends
  // a `Config` notice when the file changes, but a shared connection's
  // worker forwards only the stream's events to its windows, so every
  // window also asks again every few seconds (the bridge answers from its
  // watched copy without reading the file).
  const offersConfig = online && operations.has("config");
  useEffect(() => {
    if (!offersConfig) return;
    let alive = true;
    const load = () =>
      quietRpc("config").then((result) => {
        const config = readUserConfig(result);
        if (alive && config) setUserConfig(config);
      });
    load();
    const timer = setInterval(load, CONFIG_REFRESH_MS);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [offersConfig]);
  const mountsLoading = useRef(false);
  /** Loads the disks' mount points for the sidebar's disk tooltips. */
  async function loadDiskMounts() {
    if (!online || !vitalDisks || knownMounts.length || mountsLoading.current)
      return;
    mountsLoading.current = true;
    try {
      // A first request may find the monitor still sampling.
      for (let attempt = 0; attempt < 10; attempt++) {
        const result = await quietRpc("status");
        const mounts = (result?.disks || []).map((d: any) => String(d.mount));
        if (mounts.length) return setDiskMounts(mounts);
        if (!result || result.ready) return;
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
    } finally {
      mountsLoading.current = false;
    }
  }
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

  /** Shows one terminal on its own. */
  function activate(id: number) {
    if (id !== active) setFindPane(0);
    // Remembered like the view toggle, so a reload opens the same view.
    if (!popupPane) saveView("single");
    setView("single");
    setActive(id);
    setPhoneList(false);
  }
  function chooseMode(next: SessionMode) {
    if (popupPane) return;
    // The file browser is not restored on reload.
    if (next !== "files") saveView(next);
    setView(next);
  }
  function switchView() {
    chooseMode(view === "grid" ? "single" : "grid");
  }
  function cycle(step: number) {
    if (!live.length) return;
    const index = live.findIndex((p) => p.id === active);
    openPane(live[(index + step + live.length) % live.length]);
  }

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
  const focused = popupPane
    ? all.find((p) => p.id === popupPane)
    : view === "single"
      ? live.find((p) => p.id === active)
      : undefined;
  /** Cmd/Ctrl-click on file paths in the terminal shown alone (not in
   * popup windows, and only on a connection that offers files). */
  const pathLinks = usePathLinks({
    pane: popupPane ? undefined : focused,
    store,
    client: popupPane ? null : filesClient,
    home,
    onOpen: (target) => openPathTarget(target),
  });
  function openFind(pane: number, text = "") {
    if (!pane) return;
    setFindQuery(text);
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
        // The file editor has its own search (CodeMirror's).
        view !== "files" &&
        !(e.target as Element | null)?.closest?.(".files-editor") &&
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
      if (e.altKey && !e.ctrlKey && !e.metaKey && !overlayOpen && !popupPane) {
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
            openPane(pane);
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
        setPrefixArmed(true);
        return;
      }
      if (!prefix.current) return;
      if (["Shift", "Control", "Alt", "Meta"].includes(e.key)) return;
      // An on-screen keyboard key without a value (an IME or predictive
      // text) leaves the prefix armed for the text that follows it.
      if (e.key === "Unidentified" || e.isComposing || e.keyCode === 229)
        return;
      prefix.current = false;
      setPrefixArmed(false);
      e.preventDefault();
      e.stopImmediatePropagation();
      const action = prefixAction(e);
      if (!action) return;
      if (popupPane && action !== "send-prefix" && action !== "disconnect")
        return;
      runPrefix(action);
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [
    online,
    panes,
    order,
    parked,
    active,
    view,
    overlayOpen,
    dialog,
    confirmAction,
    notifications,
  ]);
  /** Runs a Ctrl-B prefix action (see shortcuts.ts). */
  function runPrefix(action: NonNullable<ReturnType<typeof prefixAction>>) {
    const targetPane = focusedPane();
    const target = targetPane?.id || 0;
    if (typeof action === "object") {
      const pane = live[action.select - 1];
      if (pane) openPane(pane);
      return;
    }
    switch (action) {
      case "send-prefix":
        // Ctrl-B twice sends one Ctrl-B to the focused terminal.
        if (target && store.session && !store.closing?.has(target)) {
          store.session.control(target);
          store.session.request("input", { pane: target, bytes: [2] });
        }
        return;
      case "new":
        return void newTab();
      case "reopen-last":
        return void reopenLast();
      case "grid":
        return chooseMode("grid");
      case "next":
      case "focus-right":
      case "focus-down":
        // Without grid focus, h j k l and the arrows step through the
        // terminals like Tab, each opened alone.
        return cycle(1);
      case "previous":
      case "focus-left":
      case "focus-up":
        return cycle(-1);
      case "scroll-mode":
      case "select-mode": {
        // Keyboard modes act on a terminal shown on its own; the grid has
        // no focused terminal.
        const pane = focusedPane();
        if (!pane) return;
        setTimeout(() =>
          store.events.dispatchEvent(
            new CustomEvent("terminal-key-mode", {
              detail: {
                pane: pane.id,
                kind: action === "scroll-mode" ? "scroll" : "select",
              },
            }),
          ),
        );
        return;
      }
      case "next-attention":
        return nextState(needsAttention, "No terminal needs attention");
      case "next-finished":
        return nextState(isFinished, "No finished terminal");
      // The TUI's agents and overview keys open the Terminals table with
      // the matching filter.
      case "overview":
        return openTerminals("all");
      case "agents":
        return openTerminals("agents");
      case "agents-attention":
        return openTerminals("attention");
      case "agents-finished":
        return openTerminals("finished");
      case "notifications":
        // The focused terminal's notifications (its header's bell).
        if (targetPane) openNotifications(targetPane);
        return;
      case "recovery":
      case "search":
      case "parked":
      case "help":
        return setDialog(action);
      case "move":
        // Moves the terminal shown alone; nothing in the read-only grid.
        if (targetPane) setDialog(action);
        return;
      case "settings":
        return setDialog("config");
      case "park":
        if (targetPane) park(targetPane);
        return;
      case "close":
        if (targetPane) close(targetPane);
        return;
      case "disconnect":
        return store.session?.close();
    }
  }
  /** The terminal keyboard actions address: the one shown alone. The grid
   * is read-only and has no focused terminal, so they do nothing there. */
  function focusedPane() {
    if (popupPane) return all.find((p) => p.id === popupPane);
    if (view !== "single") return undefined;
    return live.find((p) => p.id === active);
  }
  /** Opens a terminal alone, leaving the grid, and unparks a parked one.
   * Every way of opening a terminal comes here. */
  function openPane(pane: NumberedPane) {
    setMenu(null);
    if (parkedSet.has(pane.id))
      setParked(parked.filter((id) => id !== pane.id));
    activate(pane.id);
    focusTerminal();
  }
  /** The TUI's next question / next completion: the next terminal after the
   * focused one in this browser's order (parked ones included). */
  function nextState(predicate: (pane: NumberedPane) => boolean, none: string) {
    const candidates = all.filter((p) => !p.dead);
    const target = nextPane(candidates, active, predicate);
    if (target) openPane(target);
    else toast(none, { key: "next" });
  }
  /** The Terminals table, with a filter preselected. */
  function openTerminals(filter: TerminalFilter) {
    setTerminalFilter(filter);
    setDialog("terminals");
  }
  /** A sidebar summary mark: next terminal in that state. */
  function openState(state: DisplayState) {
    if (state === "working" || state === "running") {
      openTerminals(state === "working" ? "agents" : "all");
      return;
    }
    nextState(
      (p) => paneState(p, notificationsRef.current) === state,
      `No ${stateLabel[state].toLowerCase()} terminal`,
    );
  }
  function park(pane: NumberedPane) {
    if (parkedSet.has(pane.id)) return;
    setParked([...parked, pane.id]);
    toast(`Parked “${pane.title || "shell"}”; it keeps running`, {
      key: "park",
    });
  }
  function unpark(pane: NumberedPane) {
    openPane(pane);
  }
  /** The TUI's reopen-last: the newest terminal closed in this workspace
   * (closed before previous runs), restored with its first choice. */
  async function reopenLast() {
    const result = await rpc("closed");
    if (!result) return;
    const entry = (result.entries || []).find(
      (e: any) => e.workspace === result.workspace,
    );
    if (!entry) {
      toast("No recently closed terminal", { key: "restore" });
      return;
    }
    const restored = await rpc("restore", {
      key: entry.key,
      choice: entry.choices[0],
      request: crypto.randomUUID(),
      confirmed: true,
    });
    if (!restored?.pane) return;
    toast(
      restored.existing
        ? `“${entry.name}” is already open`
        : `Reopened “${entry.name}”`,
      { key: "restore" },
    );
    const pane = all.find((p) => p.id === restored.pane);
    if (pane) openPane(pane);
    else activate(restored.pane);
  }
  /** Marks notifications read in the workspace (the TUI shares the marks). */
  function acknowledge(ids: number[]) {
    attentionToasts.markRead(ids);
    for (const id of ids.slice(0, 128)) {
      if (acknowledged.current.has(id)) continue;
      acknowledged.current.add(id);
      store.session?.request("acknowledge", { notification: id });
    }
  }
  /** Opens a terminal's notifications, mainly their history (its header's
   * bell, Ctrl-B b or its menu). */
  function openNotifications(pane: NumberedPane) {
    setMenu(null);
    setNotifyPane(pane.id);
    setDialog("notifications");
  }
  /** The terminal shown on its own, if any. */
  const shownPane =
    popupPane ||
    (active && !(phone && phoneList) && view === "single" ? active : 0);
  // Showing a terminal marks its notifications read, as focusing one does
  // in the TUI: the terminal shown on its own (grid tiles are read-only and
  // keep their bells), only while this browser is signed in.
  const readPane = readingPane(
    popupPane,
    shownPane,
    visible.map((p) => p.id),
  );
  useReadShown(
    online && panesLoaded ? panes.find((p) => p.id === readPane) : undefined,
    notifications,
    acknowledge,
  );
  // In-app toasts replace desktop notifications (toasts.ts): none for the
  // terminal shown alone; the read-only grid has no focused terminal, so
  // its tiles get toasts as they keep their bells. Popups leave alerts to
  // the main window.
  const attentionToasts = useAttentionToasts({
    ready: panesLoaded && !popupPane,
    notifications,
    panes,
    events: preferences.notificationEvents,
    shown: shownPane,
    onAlert: () => {
      if (preferences.sound) chime();
    },
  });
  // Opening or switching terminals returns to reading: the on-screen
  // keyboard never appears on its own. With the hardware keyboard setting
  // On, a touch device opens terminals in input mode instead, focusing the
  // input without the on-screen keyboard.
  const hardwareKeyboard: HardwareKeyboard = touch
    ? preferences.hardwareKeyboard
    : "off";
  // Read when a terminal opens; changing the setting affects the next one.
  const hardwareOnOpen = useRef(hardwareKeyboard);
  hardwareOnOpen.current = hardwareKeyboard;
  useEffect(() => {
    const hardware = hardwareOnOpen.current;
    setInputMode((mode) => nextInputMode(mode, "open-terminal", hardware));
    setCtrlArmed(false);
    if (
      shownPane &&
      nextInputMode("reading", "open-terminal", hardware) === "input"
    )
      showKeyboard(shownPane, true, false);
  }, [shownPane]);
  // Auto and On: a key typed on a hardware keyboard while the terminal's
  // input is not focused enters input mode and reaches the terminal.
  useHardwareKeys(
    hardwareKeyboard !== "off" &&
      !!focused &&
      !(phone && phoneList) &&
      !overlayOpen,
    (key) => {
      const pane = focusedPane();
      if (!pane || store.closing?.has(pane.id)) return;
      setInputMode((mode) =>
        nextInputMode(mode, "hardware-key", hardwareKeyboard),
      );
      showKeyboard(pane.id, true, false, key);
    },
  );
  /** Shows or hides the on-screen keyboard for a terminal. The store event
   * is synchronous, so the terminal focuses its input inside this tap.
   * `virtual: false` focuses the input for a hardware keyboard without
   * showing the on-screen keyboard, and `key` (a hardware key typed while
   * the input was not focused) is then delivered to the terminal. */
  function showKeyboard(
    pane: number,
    show: boolean,
    virtual = true,
    key?: KeyboardEventInit,
  ) {
    store.events.dispatchEvent(
      new CustomEvent("terminal-keyboard", {
        detail: { pane, show, virtual, key },
      }),
    );
  }
  function terminalInput(pane: number, bytes: string, paste = false) {
    store.events.dispatchEvent(
      new CustomEvent("terminal-input", { detail: { pane, bytes, paste } }),
    );
  }
  /** The Input control: enter or leave input mode. */
  function toggleKeyboard() {
    const pane = focusedPane();
    if (!pane) return;
    const next = nextInputMode(inputMode, "input-control");
    showKeyboard(pane.id, next === "input");
    setInputMode(next);
  }
  /** A key bar tap: sends the key and enters input mode. */
  function barKey(key: BarKey) {
    const pane = focusedPane();
    if (!pane || store.closing?.has(pane.id)) return;
    showKeyboard(pane.id, true);
    setInputMode((mode) => nextInputMode(mode, "key-bar"));
    if (key === "Ctrl") return setCtrlArmed(!ctrlArmed);
    if (key === "Prefix") {
      prefix.current = !prefix.current;
      setPrefixArmed(prefix.current);
      return;
    }
    if (key === "Paste") {
      if (!navigator.clipboard?.readText) {
        toast("Paste is not available here; use the keyboard's paste", {
          key: "paste",
        });
        return;
      }
      navigator.clipboard
        .readText()
        .then((text) => {
          if (text) terminalInput(pane.id, text, true);
        })
        .catch(() =>
          toast("Paste was not allowed; use the keyboard's paste", {
            key: "paste",
          }),
        );
      return;
    }
    const modes = store.screens.get(pane.id)?.screen.modes || {};
    const bytes = barKeyBytes(key, ctrlArmed, modes, pane.kind);
    if (consumesCtrl(key)) setCtrlArmed(false);
    if (bytes) terminalInput(pane.id, bytes);
  }
  /** Text typed on the on-screen keyboard right after the key bar's Ctrl-B
   * is a prefix key. */
  function interceptText(text: string) {
    if (!prefix.current) return false;
    prefix.current = false;
    setPrefixArmed(false);
    const action = prefixAction({ key: Array.from(text)[0] || "" });
    if (action && (!popupPane || action === "send-prefix")) runPrefix(action);
    return true;
  }

  function event(event: any) {
    if (event.SignedInKey) {
      setSignedInKey(event.SignedInKey);
      return;
    }
    if (event.Workspace) {
      authProgress.current.welcomed = true;
      setStopped(event.Workspace.running ? "" : event.Workspace.workspace);
      setConnecting(false);
      return;
    }
    if (event.Config) {
      // The bridge saw config.toml change (API minor 4).
      const config = readUserConfig(event.Config);
      if (config) setUserConfig(config);
      return;
    }
    const e = event.Stream;
    if (!e) return;
    if (isUploadEvent(e)) {
      // The upload dialog shows its own errors.
      store.events.dispatchEvent(
        new CustomEvent("upload-event", { detail: e }),
      );
      return;
    }
    if (isFileEvent(e)) {
      // The file browser and editors show their own errors.
      store.events.dispatchEvent(new CustomEvent("file-event", { detail: e }));
      return;
    }
    if (e.type === "clipboard") {
      store.events.dispatchEvent(new CustomEvent("clipboard", { detail: e }));
      return;
    }
    if (e.type === "welcome") {
      setStopped("");
      setServerUploads(
        Array.isArray(e.capabilities) && e.capabilities.includes("uploads"),
      );
      setUploadLimits(readUploadLimits(e.upload_limits));
      setServerFiles(
        Array.isArray(e.capabilities) && e.capabilities.includes("files"),
      );
      setFileLimits(readFileLimits(e.file_limits));
      const subscriptions = Number(e.limits?.subscriptions);
      setSubscriptionLimit(
        Number.isSafeInteger(subscriptions) && subscriptions > 0
          ? subscriptions
          : DEFAULT_SUBSCRIPTIONS,
      );
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
        // Operations added within this API version (repos, commands) are
        // optional: a bridge without them leaves those columns plain.
        setOperations(
          new Set(Array.isArray(result.operations) ? result.operations : []),
        );
        setWorkspace(result.workspace);
        setEpoch(e.epoch);
        setOrder(layout.order);
        setParked(layout.parked);
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
      const key = JSON.stringify([e.panes, e.vitals, e.notifications]);
      if (key !== metadata.current) {
        metadata.current = key;
        setPanes(e.panes);
        setPanesLoaded(true);
        setVitals(e.vitals);
        setNotifications(e.notifications || []);
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
    setStopped("");
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
    acknowledged.current.clear();
    setNotifications([]);
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
    session.onIdle = setIdle;
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
  function popout(pane: NumberedPane) {
    const url = new URL(location.href);
    url.search = "";
    url.searchParams.set("pane", String(pane.id));
    window.open(url, "_blank", "popup,noopener,width=1100,height=760");
  }
  function close(p: NumberedPane) {
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
    // In the grid the new terminal joins the tiles; otherwise it opens.
    if (result?.pane && !(view === "grid" && !popupPane)) activate(result.pane);
  }
  function move(source: number, before: number | null) {
    setOrder(
      moveBefore(
        all.map((p) => p.id),
        source,
        before,
      ),
    );
    setDrag(null);
    setDrop(null);
  }
  function dragProps(p: NumberedPane) {
    return {
      onPointerDown: (e: React.PointerEvent) => {
        // A row or tile header starts a drag; its action buttons do not.
        if (
          e.button !== 0 ||
          (e.target as HTMLElement).closest(".term-row-actions, .pane-actions")
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
            ?.closest<HTMLElement>("[data-drop-pane]");
          const targetId = Number(target?.dataset.dropPane);
          if (target && targetId && targetId !== state.source) {
            const rect = target.getBoundingClientRect();
            // Sidebar rows reorder vertically, grid tiles horizontally.
            const after =
              target.dataset.dropAxis === "x"
                ? event.clientX >= rect.left + rect.width / 2
                : event.clientY >= rect.top + rect.height / 2;
            const remaining = live.filter((pane) => pane.id !== state.source);
            const index = remaining.findIndex((pane) => pane.id === targetId);
            state.before = after ? remaining[index + 1]?.id || null : targetId;
            setDrop(state.before);
          } else if (
            !target &&
            (event.target as HTMLElement).closest?.(".term-list")
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
  const streamedIds = new Set(visible.map((p) => p.id));
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
    return (
      <>
        <PopupConnecting onFallback={() => setPopupFallback(true)} />
        <TitleBar location="" />
      </>
    );
  if (!online && stopped && store.session)
    return (
      <>
        <StoppedWorkspace
          workspace={stopped}
          starting={starting}
          error={error}
          onStart={async () => {
            setError("");
            setStarting(true);
            // The bridge then connects and sends the usual stream welcome.
            await rpc("start", { confirmed: true });
            setStarting(false);
          }}
          onSignOut={() => {
            setStopped("");
            store.session?.close();
          }}
        />
        <TitleBar
          location={titleLocation({ view: "stopped", workspace: stopped })}
        />
      </>
    );
  if (!online)
    return (
      <>
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
          onSignIn={() =>
            void (identity ? connect(false, identity) : discover())
          }
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
        <TitleBar location={titleLocation({ view: "sign-in" })} />
      </>
    );
  const bare = !!popupPane;
  function focusTerminal() {
    if (!overlayOpen)
      requestAnimationFrame(() =>
        store.events.dispatchEvent(new Event("terminal-focus")),
      );
  }
  const showRail = !bare && !phone;
  const railWidth = folded ? 56 : preferences.sidebarWidth;
  /** The phone shows its terminal list instead of a terminal. */
  const listShown =
    phone && (phoneList || (!focused && view !== "files"));
  /** The sidebar's edge: a click folds or unfolds it, a drag resizes it
   * (folding below a minimum), like KelpHub's. Saved in this browser. */
  function resizeStart(e: React.PointerEvent) {
    e.preventDefault();
    const x = e.clientX,
      initial = railWidth;
    let moved = false,
      last = initial;
    const limit = (width: number) =>
      Math.round(
        Math.max(SIDEBAR_MIN, Math.min(SIDEBAR_MAX, innerWidth * 0.45, width)),
      );
    const move = (event: PointerEvent) => {
      moved ||= Math.abs(event.clientX - x) > 4;
      if (!moved) return;
      last = initial + event.clientX - x;
      // A tablet in portrait folds for this orientation only.
      if (tabletPortrait) setPortraitFolded(last < 110);
      setPreferences((current) =>
        last < 110
          ? tabletPortrait
            ? current
            : { ...current, sidebarFolded: true }
          : {
              ...current,
              sidebarFolded: tabletPortrait ? current.sidebarFolded : false,
              sidebarWidth: limit(last),
            },
      );
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      if (!moved) toggleFold();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }
  const paneLabel = (p: NumberedPane) => p.title || "shell";
  /** Why files cannot be uploaded on this connection, if they cannot. */
  const uploadUnavailable = !serverUploads
    ? "This workspace's terminal server cannot receive files (it lacks the uploads capability). Restart the workspace with a current KelpServer to upload files to terminals."
    : !operations.has("uploads")
      ? "The server's kelpmux web-bridge is older than this web client and cannot relay uploads. Install matching versions."
      : !uploadLimits
        ? "The terminal server did not announce its upload limits."
        : null;
  /** Opens the upload dialog for files dropped, pasted or picked onto a
   * terminal. */
  function openUpload(
    pane: NumberedPane,
    files: File[],
    folders: string[] = [],
  ) {
    if ((!files.length && !folders.length && !uploadUnavailable) || uploadDrop)
      return;
    setMenu(null);
    setUploadDrop({ pane, files, folders });
  }
  /** The terminal menu's Upload files…: a file picker (phones and tablets
   * have no drag and drop). It opens inside the menu click's gesture. */
  function pickFiles(pane: NumberedPane) {
    pickerPane.current = pane;
    const picker = filePicker.current;
    if (!picker) return;
    picker.value = "";
    picker.click();
  }
  /** Drop target props for a terminal (its view or grid tile): files from
   * the desktop upload to it, entries from the file browser type their
   * paths into it. */
  function dropProps(p: NumberedPane) {
    const paths = pathDropProps(p);
    return {
      "data-upload-pane": p.id,
      onDragOver: (event: React.DragEvent) => {
        if (dragHasPaths(event)) return paths.onDragOver(event);
        if (!dragHasFiles(event) || uploadDrop) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = "copy";
        if (dropPane !== p.id) setDropPane(p.id);
      },
      onDragLeave: (event: React.DragEvent) => {
        paths.onDragLeave(event);
        if (!event.currentTarget.contains(event.relatedTarget as Node))
          setDropPane((pane) => (pane === p.id ? 0 : pane));
      },
      onDrop: (event: React.DragEvent) => {
        if (dragHasPaths(event)) return paths.onDrop(event);
        if (!dragHasFiles(event)) return;
        event.preventDefault();
        event.stopPropagation();
        setDropPane(0);
        const { files, folders } = droppedFiles(event.dataTransfer);
        openUpload(p, files, folders);
      },
    };
  }
  /** Drop target props for a terminal's sidebar row (and its view): paths
   * dragged from the file browser are typed into it. */
  function pathDropProps(p: NumberedPane) {
    return {
      onDragOver: (event: React.DragEvent) => {
        if (!dragHasPaths(event)) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = "copy";
        if (pathDropPane !== p.id) setPathDropPane(p.id);
      },
      onDragLeave: (event: React.DragEvent) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node))
          setPathDropPane((pane) => (pane === p.id ? 0 : pane));
      },
      onDrop: (event: React.DragEvent) => {
        if (!dragHasPaths(event)) return;
        event.preventDefault();
        event.stopPropagation();
        setPathDropPane(0);
        insertPaths(droppedPaths(event.dataTransfer), p);
      },
    };
  }
  /** Types shell-quoted paths into a terminal, without Enter, as uploads
   * paste theirs. */
  function insertPaths(paths: string[], pane: NumberedPane | undefined) {
    if (!paths.length) return;
    if (!pane || pane.dead) {
      toast("Open a terminal first to insert paths into it", { key: "insert" });
      return;
    }
    const text = typedPaths(paths);
    if (text === null) {
      toast("This path holds control characters and cannot be typed safely", {
        key: "insert",
        tone: "error",
      });
      return;
    }
    if (!typeIntoPane(store, pane.id, text)) return;
    toast(
      `Inserted ${paths.length === 1 ? `“${baseName(paths[0])}”` : `${paths.length} paths`} into terminal ${pane.number}`,
      { key: "insert" },
    );
  }
  /** The terminal Insert path types into: the one last shown alone. */
  const insertPane = live.find((p) => p.id === active);
  /** The open file shown in the main area; none shows the browser. */
  const shownDocument = documents.docs.find((doc) => doc.path === shownDoc);
  /** Shows the file browser, at `dir` when given. */
  function openFiles(dir?: string) {
    if (!filesClient || popupPane) return;
    // A folder outside the file root (a terminal's): the root instead.
    if (dir && filesRoot && !withinRoot(dir, filesRoot, home)) {
      toast(outsideRootMessage, { key: "navigation" });
      dir = filesRoot;
    }
    if (dir) setFilesDir(dir);
    setShownDoc("");
    setMenu(null);
    setFindPane(0);
    setPhoneList(false);
    chooseMode("files");
  }
  /** The file in the side panel, while it is open. */
  const sideDocument = side
    ? documents.docs.find((doc) => doc.id === side.id)
    : undefined;
  /** The files open in the main area (the Files view's tabs, in opening
   * order); the side panel's file has none (one buffer per path). */
  const tabDocs = documents.docs.filter((doc) => doc.id !== side?.id);
  /** The Files view's tab strip, under the top bar of the view shown. */
  const fileTabs = (
    <FileTabs
      docs={tabDocs}
      current={shownDocument?.path || ""}
      onBrowse={() => openFiles()}
      onShow={(path) => openDoc(path)}
      onClose={closeDoc}
    />
  );
  /** The side panel shows beside the terminal shown alone (never on a
   * phone); otherwise it stays mounted but hidden. */
  const sideShown =
    !!sideDocument && !phone && !popupPane && view === "single" && !!focused;
  const jumpTo = (at?: { line?: number; col?: number }): FileJump =>
    at?.line ? { line: at.line, col: at.col, seq: ++jumps.current } : undefined;
  /** Opens a file in the editor (or shows it again), at `jump` when given.
   * The file in the side panel shows there, beside its terminal. */
  function openDoc(path: string, jump?: FileJump) {
    if (!filesClient) return;
    if (sideDocument?.path === path) {
      if (!phone && insertPane) {
        setSide({ id: sideDocument.id, jump });
        activate(insertPane.id);
        return;
      }
      // A phone has no side panel: the file moves to the main area.
      setSide(null);
    }
    documents.open(path);
    setShownDoc(path);
    setMainJump(jump);
    setPhoneList(false);
    chooseMode("files");
  }
  /** Opens a file in the side panel beside the terminal last shown alone,
   * showing that terminal; a phone (or no terminal) opens the editor in
   * the main area instead. A file with unsaved changes in the panel is
   * closed first, after asking. */
  function openBeside(path: string, jump?: FileJump) {
    if (!filesClient || popupPane) return;
    const pane = insertPane;
    if (phone || !pane) return openDoc(path, jump);
    const place = () => {
      const id = documents.open(path);
      if (shownDoc === path) setShownDoc("");
      setSide({ id, jump });
      activate(pane.id);
    };
    const current = sideDocument;
    if (!current?.dirty || current.path === path) return place();
    setConfirmAction({
      title: "Discard changes",
      text: `“${baseName(current.path)}” in the side panel has unsaved changes. Close it and discard them?`,
      buttons: [
        {
          label: "Discard changes",
          color: "danger",
          action: () => {
            documents.close(current.path);
            place();
          },
        },
      ],
    });
  }
  /** A path link: a folder opens in the browser, a file beside the
   * terminal at its position; a symbolic link opens as what it names. */
  function openPathTarget(target: PathTarget) {
    if (!filesClient) return;
    const jump = jumpTo(target);
    if (target.kind === "dir") return openFiles(target.path);
    if (target.kind !== "symlink") return openBeside(target.path, jump);
    filesClient.list(target.path).then(
      (listing) => openFiles(listing.path),
      () => openBeside(target.path, jump),
    );
  }
  /** Closes an open file, asking first when it has unsaved changes. The
   * file shown gives way to its neighbour tab (or the browser). */
  function closeDoc(path: string) {
    const doc = documents.docs.find((d) => d.path === path);
    const close = () => {
      // Closing a file is not a move: its entry becomes what shows next.
      navigation.automatic();
      documents.close(path);
      if (shownDoc === path) {
        setShownDoc(
          neighbourTab(
            tabDocs.map((d) => d.path),
            path,
          ),
        );
        setMainJump(undefined);
      }
      if (sideDocument?.path === path) setSide(null);
    };
    if (!doc?.dirty) return close();
    setConfirmAction({
      title: "Discard changes",
      text: `“${baseName(path)}” has unsaved changes. Close it and discard them?`,
      buttons: [{ label: "Discard changes", color: "danger", action: close }],
    });
  }
  /** Pastes uploaded paths into their terminal as a paste (bracketed when
   * the pane has it on), without Enter. Null once pasted, else why not. */
  function pasteUploaded(target: NumberedPane, paths: string[]): string | null {
    const paneId = target.id;
    const pane = panesNow.current.find((p) => p.id === paneId);
    if (!pane || pane.dead || store.closing?.has(paneId) || !store.session)
      return `Terminal ${target.number} closed while the files were uploading.`;
    const text = pastedPaths(paths);
    if (new TextEncoder().encode(text).length > 65536)
      return "The paths are longer than one 64 KiB paste.";
    typeIntoPane(store, paneId, text);
    toast(
      `Pasted ${paths.length === 1 ? "the path" : `${paths.length} paths`} into terminal ${pane.number}`,
      { key: "upload" },
    );
    return null;
  }
  /** A terminal's context menu (right-click on its row or tile). */
  function menuItems(p: NumberedPane): [string, () => void, boolean?][] {
    const isParked = parkedSet.has(p.id);
    const unread = unreadFor(notifications, p).length;
    // Reorders among the tabs; parked terminals keep their places after.
    const liveIds = live.map((pane) => pane.id);
    const rest = all
      .map((pane) => pane.id)
      .filter((id) => !liveIds.includes(id));
    const step = (by: -1 | 1) =>
      setOrder([...moveBy(liveIds, p.id, by), ...rest]);
    const index = live.findIndex((pane) => pane.id === p.id);
    const items: [string, () => void, boolean?][] = [
      [isParked ? "Unpark and open" : "Open", () => openPane(p)],
      ["Open in a window", () => popout(p)],
      [
        "Find…",
        () => {
          openPane(p);
          requestAnimationFrame(() => openFind(p.id));
        },
      ],
      ["Move up", () => step(-1), isParked || index <= 0],
      [
        "Move down",
        () => step(1),
        isParked || index < 0 || index >= live.length - 1,
      ],
      isParked ? ["Unpark", () => unpark(p)] : ["Park", () => park(p)],
      [
        unread ? `Notifications (${unread} unread)…` : "Notifications…",
        () => openNotifications(p),
      ],
      [
        "Upload files…",
        () => (uploadUnavailable ? openUpload(p, []) : pickFiles(p)),
      ],
      ["Close…", () => close(p)],
    ];
    // The file browser at the terminal's folder.
    if (filesClient && !popupPane)
      items.splice(items.length - 1, 0, [
        "Browse files here",
        () => openFiles(p.cwd || "~"),
      ]);
    // A touch device has no popup windows: an installed web app would open
    // them in the browser instead.
    return touch
      ? items.filter(([label]) => label !== "Open in a window")
      : items;
  }
  /** Who holds a terminal's input and size when this browser does not (the
   * former status bar's hint), for the terminal's header. */
  function leaseNote(p: NumberedPane) {
    const owner = store.leases.get(p.id);
    if (owner === store.session?.client) return null;
    return owner
      ? {
          label: "Other browser",
          text: "Another browser controls this terminal's input and size; type or click here to take it",
        }
      : {
          label: "TUI",
          text: "The TUI controls this terminal's input and size; typing here takes it, typing there takes it back",
        };
  }
  const paneHeader = (p: NumberedPane, extra?: React.ReactNode) => {
    const lease = leaseNote(p);
    return (
      <header
        onContextMenu={(event) => {
          event.preventDefault();
          setMenu({ pane: p, x: event.clientX, y: event.clientY });
        }}
      >
        <PaneLabel
          pane={p}
          state={paneState(p, notifications)}
          parked={parkedSet.has(p.id)}
        />
        <span className="grid-folder" title={p.cwd}>
          {basename(p.cwd)}
        </span>
        {lease && (
          <Tooltip content={lease.text}>
            <span
              className="badge lease-badge"
              tabIndex={0}
              aria-label={lease.text}
            >
              {lease.label}
            </span>
          </Tooltip>
        )}
        {paneActions(p, extra)}
      </header>
    );
  };
  const paneActions = (p: NumberedPane, extra?: React.ReactNode) => (
    <div className="pane-actions">
      {extra}
      {filesClient && !bare && (
        <button
          type="button"
          className="icon-btn"
          title="Files in this terminal's folder"
          aria-label={`Browse files in the folder of ${paneLabel(p)}`}
          data-files-here={p.id}
          onClick={(event) => {
            event.stopPropagation();
            openFiles(p.cwd || "~");
          }}
        >
          <Icon name="folder-open" size="sm" />
        </button>
      )}
      <NotificationsButton
        label={paneLabel(p)}
        count={unreadFor(notifications, p).length}
        onOpen={() => openNotifications(p)}
      />
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
          title="Park (Ctrl-B p)"
          aria-label={`Park ${paneLabel(p)}`}
          onClick={(event) => {
            event.stopPropagation();
            park(p);
          }}
        >
          <Icon name="archive" size="sm" />
        </button>
      )}
      {!bare && !touch && (
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
  );
  /** The terminal whose notifications the panel shows, while it lives. */
  const notified = notifyPane
    ? all.find((p) => p.id === notifyPane)
    : undefined;
  const dialogTitles: Record<string, string> = {
    config: "Settings",
    stats: "System status",
    notifications: notified
      ? `Notifications · ${paneLabel(notified)}`
      : "Notifications",
    terminals: "Terminals",
    recovery: "Recently closed",
    search: "Search terminal history",
    parked: "Parked terminals",
    move: "Move terminal",
    help: "Help",
  };
  const closeDialog = () => {
    setDialog("");
    setQuery("");
  };
  /** What the file browser does with entries. */
  const fileActions: FileActionsProps | null = filesClient && {
    client: filesClient,
    home,
    onOpenFile: (path) => openDoc(path),
    onOpenBeside: insertPane ? (path) => openBeside(path) : undefined,
    insertTarget: insertPane
      ? { number: insertPane.number, title: paneLabel(insertPane) }
      : undefined,
    onInsertPath: (path) => insertPaths([path], insertPane),
    onCopyPath: (path) =>
      navigator.clipboard
        ?.writeText(path)
        .then(() => toast("Path copied", { key: "copy" }))
        .catch(fail),
    confirm: setConfirmAction,
    toast: (message, options) => toast(message, { key: "files", ...options }),
    onRenamed: (from, to) => {
      documents.renamed(from, to);
      // The file shown follows its rename or move.
      setShownDoc((shown) =>
        isInside(shown, from) ? to + shown.slice(from.length) : shown,
      );
    },
    onRemoved: documents.removed,
    upload: {
      unavailable: uploadUnavailable,
      limits: uploadLimits,
      transport: uploadTransport,
    },
  };
  // Rows reorder by dragging with a mouse; on touch a drag scrolls the list,
  // and the row menu offers Move up / Move down instead.
  const sidebar = (
    <Sidebar
      username={connection?.username || ""}
      configProblem={!!userConfig?.errors.length}
      dev={!!connection?.dev}
      folded={folded}
      touch={touch}
      phone={phone}
      mode={view}
      diskMounts={knownMounts}
      onDisksHover={() => void loadDiskMounts()}
      onMode={chooseMode}
      terminals={ordered}
      parked={parkedPanes}
      notifications={notifications}
      current={view === "single" ? active : undefined}
      vitals={vitals}
      rowProps={(p) => ({
        ...(touch ? {} : dragProps(p)),
        ...(filesClient ? pathDropProps(p) : {}),
      })}
      rowClass={(p) =>
        `${drag === p.id && drag !== drop ? "landing" : ""} ${pathDropPane === p.id ? "drop-target" : ""}`
      }
      onFiles={filesClient && !popupPane ? () => openFiles() : undefined}
      onOpen={(p) => {
        if (performance.now() < suppressClick.current) return;
        openPane(p);
      }}
      onClose={close}
      onPark={park}
      onUnpark={unpark}
      onMenu={(pane, x, y) => setMenu({ pane, x, y })}
      onNew={newTab}
      onState={openState}
      onDialog={(name) =>
        name === "terminals" ? openTerminals("all") : setDialog(name)
      }
      onResizeStart={resizeStart}
      onToggleFold={toggleFold}
    />
  );
  /** The place the installed app's title bar names. */
  const barLocation = titleLocation(
    listShown
      ? { view: "terminals" }
      : view === "files"
        ? {
            view: "files",
            dir: filesDir,
            file: shownDocument?.path,
            home,
          }
        : view === "grid" && !popupPane
          ? { view: "grid" }
          : { view: "terminal", title: focused?.title },
  );
  return (
    <div
      className={`kelpmux-shell ${bare ? "bare" : ""} ${touch ? "touch" : ""} ${phone ? "phone" : ""} ${listShown ? "phone-home" : ""}`}
      style={{ "--rail": `${railWidth}px` } as React.CSSProperties}
    >
      {showRail && (
        <div className="rail-wrap" onClick={focusTerminal}>
          {sidebar}
        </div>
      )}
      <div
        className={`app ${view} ${popupPane ? "popup" : ""}`}
        onClick={focusTerminal}
      >
        {phone && focused && !listShown && (
          <PhoneBar
            pane={focused}
            state={paneState(focused, notifications)}
            attention={attentionTotal}
            onList={() => {
              setPhoneList(true);
              setFindPane(0);
            }}
            unread={unreadFor(notifications, focused).length}
            onNotifications={() => openNotifications(focused)}
            onMenu={(x, y) => setMenu({ pane: focused, x: Math.max(8, x), y })}
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
        <div className="workspace">
          <div className="workspace-main">
            {listShown ? (
              <div className="phone-list">{sidebar}</div>
            ) : view === "files" && fileActions ? (
              // An open file shows in its editor, mounted below.
              shownDocument ? null : (
                <FilesBrowser
                  {...fileActions}
                  dir={filesDir}
                  onDir={(dir, replace) => {
                    // `~` or a symbolic link read as its real folder,
                    // keyboard browsing, a file selected in another
                    // column: no entry of their own.
                    if (replace) navigation.automatic();
                    setFilesDir(dir);
                  }}
                  phone={phone}
                  onBack={phone ? () => setPhoneList(true) : undefined}
                  tabs={fileTabs}
                />
              )
            ) : focused ? (
              <div className="pane-grid tab-pane-layout">
                <section
                  key={focused.id}
                  className={`grid-pane first-row selected ${focused.status === "input_needed" ? "needs-input" : ""} ${dropPane === focused.id ? "drop-target" : ""} ${pathLinks.className}`}
                  {...dropProps(focused)}
                  {...pathLinks.props}
                >
                  {pathLinks.overlay}
                  {!phone && paneHeader(focused)}
                  <Terminal
                    pane={focused}
                    store={store}
                    active
                    layout="tabs"
                    onActivate={() => setActive(focused.id)}
                    onError={fail}
                    find={findPane === focused.id}
                    findQuery={findQuery}
                    onCloseFind={() => setFindPane(0)}
                    touchKeyboard={touch}
                    onInputFocus={(focus) =>
                      setInputMode((mode) =>
                        nextInputMode(mode, focus ? "focus" : "blur"),
                      )
                    }
                    stickyCtrl={ctrlArmed}
                    onCtrlUsed={() => setCtrlArmed(false)}
                    interceptText={interceptText}
                    onFiles={(files) => openUpload(focused, files)}
                  />
                </section>
              </div>
            ) : view === "grid" && live.length ? (
              // Read-only: no focus and no input (keys, mouse, paste or
              // dropped files) reach a tile's terminal.
              <LiveGrid
                panes={ordered}
                notifications={notifications}
                body={(p) =>
                  streamedIds.has(p.id) ? (
                    <TerminalTile pane={p} store={store} />
                  ) : (
                    <div className="tile-unstreamed" aria-hidden="true" />
                  )
                }
                onOpen={(p) => {
                  if (performance.now() < suppressClick.current) return;
                  openPane(p);
                }}
                onMenu={(pane, x, y) => setMenu({ pane, x, y })}
                onNew={newTab}
                onDragStart={(p, event) => {
                  if (!touch) dragProps(p).onPointerDown(event);
                }}
                tileClass={(p) =>
                  drag === p.id && drag !== drop ? "landing" : ""
                }
              />
            ) : panesLoaded ? (
              <div className="empty-state">
                <Icon name="terminal" size="lg" />
                <h3>{live.length ? "Choose a terminal" : "No open terminals"}</h3>
                {parkedPanes.length > 0 && (
                  <p>
                    {parkedPanes.length} parked{" "}
                    {parkedPanes.length === 1 ? "terminal" : "terminals"} in the
                    sidebar.
                  </p>
                )}
                <button onClick={newTab}>
                  <Icon name="plus" size="sm" />
                  New terminal
                </button>
              </div>
            ) : null}
            {/* Every open file's editor stays mounted (hidden unless shown)
                so its unsaved buffer survives switching to a terminal, the
                grid or another file; the side panel's file is the panel's. */}
            {filesClient &&
              tabDocs.map((doc) => (
                  <FileEditor
                    key={doc.id}
                    doc={doc}
                    documents={documents}
                    home={home}
                    maxRead={filesClient.limits.max_read_bytes}
                    hidden={
                      listShown ||
                      view !== "files" ||
                      doc.id !== shownDocument?.id
                    }
                    onClose={() => closeDoc(doc.path)}
                    onReveal={() => openFiles(parentPath(doc.path))}
                    onBack={phone ? () => setPhoneList(true) : undefined}
                    onMove={
                      !phone && insertPane
                        ? () => openBeside(doc.path)
                        : undefined
                    }
                    jump={doc.id === shownDocument?.id ? mainJump : undefined}
                    tabs={doc.id === shownDocument?.id ? fileTabs : undefined}
                  />
                ))}
          </div>
          {sideDocument && filesClient && !popupPane && (
            <SidePanel
              doc={sideDocument}
              documents={documents}
              home={home}
              maxRead={filesClient.limits.max_read_bytes}
              hidden={!sideShown}
              width={preferences.sidePanelWidth}
              onWidth={(width) =>
                setPreferences((current) =>
                  current.sidePanelWidth === width
                    ? current
                    : { ...current, sidePanelWidth: width },
                )
              }
              onClose={() => closeDoc(sideDocument.path)}
              onReveal={() => openFiles(parentPath(sideDocument.path))}
              onMove={() => {
                const path = sideDocument.path;
                setSide(null);
                documents.open(path);
                setShownDoc(path);
                setMainJump(undefined);
                chooseMode("files");
              }}
              jump={side?.jump}
            />
          )}
        </div>
        {touch && focused && !listShown && (
          <KeyBar
            ctrl={ctrlArmed}
            prefix={prefixArmed}
            input={inputMode === "input"}
            onKey={barKey}
            onToggleKeyboard={toggleKeyboard}
          />
        )}
        {keyManagement}
        <input
          ref={filePicker}
          type="file"
          multiple
          hidden
          aria-hidden="true"
          tabIndex={-1}
          data-upload-picker
          onChange={(event) => {
            const files = Array.from(event.currentTarget.files || []);
            const pane = pickerPane.current;
            pickerPane.current = null;
            if (pane && files.length) openUpload(pane, files);
          }}
        />
        {uploadDrop && (
          <UploadDialog
            drop={uploadDrop}
            unavailable={uploadUnavailable}
            root={filesRoot}
            limits={uploadLimits}
            transport={uploadTransport}
            defaultDestination={uploadDestination}
            onRememberDestination={(path) => {
              saveUploadDestination(path);
              setUploadDestination(path);
            }}
            onUploaded={(paths) => pasteUploaded(uploadDrop.pane, paths)}
            onCopy={(text) =>
              navigator.clipboard
                ?.writeText(text)
                .then(() => toast("Copied", { key: "copy" }))
                .catch(fail)
            }
            onClose={() => setUploadDrop(null)}
          />
        )}
        {menu && (
          <PaneMenu
            pane={menu.pane}
            x={menu.x}
            y={menu.y}
            parked={parkedSet.has(menu.pane.id)}
            items={menuItems(menu.pane)}
            onClose={() => setMenu(null)}
          />
        )}
        {dialog && (
          <div
            className={`modal-shade ${dialog === "config" ? "settings-shade" : ""}`}
            onMouseDown={(e) => {
              if (e.target === e.currentTarget) closeDialog();
            }}
          >
            <section
              className={`modal ${dialog === "stats" ? "stats-modal" : ""} ${dialog === "config" ? "config-modal" : ""} ${dialog !== "stats" && dialog !== "config" ? "panel-modal" : ""} ${dialog === "terminals" ? "wide-modal" : ""}`}
              role="dialog"
              aria-modal="true"
              aria-labelledby="modal-title"
              data-dialog={dialog}
            >
              <header>
                <h2 id="modal-title">{dialogTitles[dialog] || dialog}</h2>
                <button
                  className="icon-btn"
                  aria-label="Close dialog"
                  onClick={closeDialog}
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
                  preferences={preferences}
                  setPreferences={setPreferences}
                  uploadDestination={uploadDestination}
                  setUploadDestination={(path) => {
                    saveUploadDestination(path);
                    setUploadDestination(path);
                  }}
                  uploadUnavailable={uploadUnavailable}
                  uploadRoot={filesRoot}
                  userConfig={userConfig}
                  saveUserConfig={saveUserConfig}
                  touch={touch}
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
              {dialog === "terminals" && (
                <TerminalsPanel
                  panes={all}
                  parked={parkedSet}
                  notifications={notifications}
                  filter={terminalFilter}
                  setFilter={setTerminalFilter}
                  operations={operations}
                  query={quietRpc}
                  cards={phone}
                  onOpen={(p) => {
                    closeDialog();
                    openPane(p);
                  }}
                />
              )}
              {dialog === "notifications" && notified && (
                <TerminalNotificationsPanel
                  pane={notified}
                  notifications={notifications}
                  onAcknowledge={acknowledge}
                />
              )}
              {dialog === "notifications" && !notified && (
                <p className="panel-empty">This terminal is closed.</p>
              )}
              {dialog === "recovery" && (
                <RecoveryPanel
                  rpc={rpc}
                  onCopy={(text) =>
                    navigator.clipboard
                      ?.writeText(text)
                      .then(() => toast("Command copied", { key: "copy" }))
                      .catch(fail)
                  }
                  onRestored={(id, existing) => {
                    closeDialog();
                    toast(
                      existing
                        ? "This conversation is already open"
                        : "Restored in a new terminal",
                      { key: "restore" },
                    );
                    const pane = all.find((p) => p.id === id);
                    if (pane) openPane(pane);
                    else activate(id);
                  }}
                />
              )}
              {dialog === "search" && (
                <SearchPanel
                  panes={all}
                  focused={focusedPane()}
                  rpc={rpc}
                  onOpen={(p, text) => {
                    closeDialog();
                    openPane(p);
                    setFindPane(0);
                    requestAnimationFrame(() => openFind(p.id, text));
                  }}
                />
              )}
              {dialog === "parked" && (
                <ParkedPanel
                  panes={parkedPanes}
                  notifications={notifications}
                  onUnpark={(p) => {
                    closeDialog();
                    unpark(p);
                  }}
                  onClose={close}
                />
              )}
              {dialog === "move" && (
                <MovePanel
                  panes={live}
                  source={focusedPane()}
                  onMove={(before) => {
                    const source = focusedPane();
                    closeDialog();
                    if (source) move(source.id, before);
                  }}
                />
              )}
              {dialog === "help" && <HelpPanel />}
            </section>
          </div>
        )}
        {confirmAction && (
          <ConfirmDialog
            action={confirmAction}
            pending={confirmPending}
            onCancel={() => setConfirmAction(null)}
            onApprove={approve}
          />
        )}
        {idle && <IdleWarning notice={idle} onStay={reportActivity} />}
      </div>
      {online && !popupPane && (
        <AttentionToasts
          toasts={attentionToasts.toasts}
          panes={all}
          onOpen={openPane}
          onDismiss={attentionToasts.dismiss}
        />
      )}
      <TitleBar location={barLocation} />
    </div>
  );
}
createRoot(document.getElementById("root")!).render(
  <ToastProvider aside={<Downloads />}>
    <App />
  </ToastProvider>,
);
