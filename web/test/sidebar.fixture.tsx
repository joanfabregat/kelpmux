// The sidebar, a terminal's header and its notifications panel as main.tsx
// wires them, with fixed terminals and notifications instead of SSH
// (test/sidebar.mjs). Showing a terminal marks its notifications read
// through the same hook as main.tsx; terminal 1, which has none, is shown
// first. The read-only grid (the real LiveGrid, with placeholders for the
// screens) marks nothing read and has no focus. Right-clicking a row opens
// that terminal's notifications, as its menu's Notifications… item does,
// and `window.notify(pane)` adds an unread notification. The query string
// picks the case: `mode=light`, `tabs=N`, `view=grid`, `folded=1`. The
// Terminals header, the tabs, the tiles and the opening shortcuts
// (Alt+digit, Alt+Tab, Ctrl/Cmd+Shift+G and the Ctrl-B keys through the
// real prefix map) switch the view as in main.tsx. The shown terminal's placeholder is a text area
// recording the keys typed into it in `window.typed`. With `toasts=1` the
// attention toasts (AttentionToasts.tsx) and the bell favicon run through
// the same hook as main.tsx: `window.bell(pane)` rings a bell and
// `window.setEvents({...})` changes the Settings event toggles. With
// `files=1` the Files section lists two open files, one unsaved, and the
// home folder's tree over an in-memory stand-in files client; its header
// and rows show a placeholder for the browser or the file, and a file
// opened from the tree joins the open files. The sections' expanded state
// is kept in the real preferences store. Toasts and download progress
// show in the real top right stack (ToastProvider, Downloads).
import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import type { NumberedPane } from "@kelpcore/web/types.ts";
import { Sidebar } from "../src/Sidebar";
import { NotificationsButton, PaneLabel, type SessionMode } from "../src/Shell";
import { TerminalNotificationsPanel } from "../src/Panels";
import { LiveGrid } from "../src/LiveGrid";
import { prefixAction } from "../src/prefix";
import { PhoneBar, useDevice } from "../src/Mobile";
import { Icon } from "../src/status";
import { basename } from "../src/format";
import { sidebarFolded } from "../src/device";
import { interfaceVariables, themeVariables } from "../src/appearance";
import {
  needsAttention,
  nextPane,
  paneState,
  readingPane,
  unreadFor,
  type Notification,
} from "../src/state/model";
import { useReadShown } from "../src/state/read";
import { AttentionToasts, useAttentionToasts } from "../src/AttentionToasts";
import {
  bellRinging,
  defaultNotificationEvents,
  type NotificationEvents,
} from "../src/toasts";
import { activityIcon } from "../src/activity-icon";
import { loadPreferences, savePreferences } from "../src/layout-store";
import {
  FileError,
  type FileEntry,
  type FileListing,
  type FilesClient,
} from "@kelpcore/web/files/api.ts";
import { ToastProvider, useToast } from "../src/Toast";
import { Downloads } from "../src/files/downloads";
import type { FileActionsProps } from "../src/files/actions";
import "../src/styles/base.css";
import "../src/styles/shell.css";
import "@kelpcore/web/terminal.css";
import "../src/styles/dialogs.css";
import "../src/styles/login.css";
import "../src/styles/kelpmux.css";
import "../src/styles/state.css";
import "../src/styles/mobile.css";
import "../src/styles/files.css";

const query = new URLSearchParams(location.search);
const toastsOn = query.get("toasts") === "1";
const filesOn = query.get("files") === "1";
const mode = query.get("mode") === "light" ? "light" : "dark";
const root = document.documentElement;
root.dataset.mode = mode;
for (const [key, value] of Object.entries({
  ...themeVariables("black", mode),
  ...interfaceVariables("black", mode),
}))
  root.style.setProperty(key, value);

const kinds = [undefined, "claude", "codex"];
const statuses = ["idle", "working", "input_needed", "turn_finished", "idle"];
const folders = [
  "/home/alice/repos/kelpmux",
  "/home/alice/repos/kelpserver",
  "/home/alice/scratch",
  "/home/alice",
];
const panes: NumberedPane[] = Array.from(
  { length: Number(query.get("tabs")) || 10 },
  (_, i) => ({
    id: i + 1,
    number: i + 1,
    pid: 100 + i + 1,
    cwd: folders[i % folders.length],
    title: kinds[i % 3] ? `${kinds[i % 3]} ${i + 1}` : "shell",
    status: statuses[i % statuses.length],
    dead: false,
    kind: kinds[i % 3],
  }),
);
const note = (id: number, pane: number, status: string, read = false) => ({
  id,
  pane,
  title: `${status === "turn_finished" ? "Turn finished" : "Needs input"} · ${id}`,
  status,
  at: Date.now() / 1000 - 60 * id,
  read,
  workspace: "main",
  origin: { run: "r", pid: 100 + pane },
});
const initial: Notification[] = [
  note(1, 2, "turn_finished", true),
  note(2, 2, "turn_finished"),
  note(3, 2, "input_needed"),
  note(4, 5, "turn_finished"),
];
Object.assign(window, { acks: [] as number[], typed: [] as string[] });

/** Files (files=1): a home folder in memory behind a stand-in files client,
 * as KelpCore's FilesClient answers. Downloads never finish (until
 * cancelled), so their progress toast stays; `window.toast(text)` shows a
 * confirmation. */
const tree: Record<string, string[]> = {
  "/home/alice": [
    "repos/",
    "scratch/",
    ".config/",
    "notes.md",
    "app.py",
    "README.md",
    ".profile",
  ],
  "/home/alice/repos": ["kelpmux/", "kelpserver/", "todo.txt"],
  "/home/alice/repos/kelpmux": ["src/", "Cargo.toml", "README.md"],
  "/home/alice/repos/kelpmux/src": ["main.rs", "lib.rs"],
  "/home/alice/repos/kelpserver": [],
  "/home/alice/scratch": ["draft.txt"],
  "/home/alice/.config": ["settings.toml"],
};
const fileEntry = (name: string): FileEntry => ({
  name: name.replace(/\/$/, ""),
  kind: name.endsWith("/") ? "dir" : "file",
  size: name.endsWith("/") ? 0 : 1200,
  modified: Date.now() / 1000 - 3600,
  mode: 0o644,
});
const childPath = (dir: string, name: string) =>
  `${dir}/${name.replace(/\/$/, "")}`;
function moveEntry(from: string, to: string) {
  const parent = from.slice(0, from.lastIndexOf("/"));
  const name = tree[parent].find((n) => childPath(parent, n) === from);
  if (!name) throw new FileError({ code: "not_found", message: "Not found" });
  tree[parent] = tree[parent].filter((n) => n !== name);
  const target = to.slice(0, to.lastIndexOf("/"));
  const base = to.slice(to.lastIndexOf("/") + 1);
  tree[target]?.push(name.endsWith("/") ? `${base}/` : base);
  for (const key of Object.keys(tree))
    if (key === from || key.startsWith(`${from}/`)) {
      tree[to + key.slice(from.length)] = tree[key];
      delete tree[key];
    }
}
const fakeFiles = {
  limits: { max_read_bytes: 1 << 20 },
  canDownload: true,
  async list(path: string): Promise<FileListing> {
    const dir = path === "~" ? "/home/alice" : path;
    await new Promise((resolve) => setTimeout(resolve, 10));
    if (!tree[dir])
      throw new FileError({ code: "not_found", message: "Not found" });
    return {
      path: dir,
      entries: tree[dir].map(fileEntry),
      truncated: false,
      skipped: 0,
    };
  },
  async rename(from: string, to: string) {
    moveEntry(from, to);
  },
  async trash(paths: string[]) {
    for (const path of paths) {
      const parent = path.slice(0, path.lastIndexOf("/"));
      tree[parent] = tree[parent].filter((n) => childPath(parent, n) !== path);
    }
    return { removed: paths.map((path) => ({ path })), failed: [] };
  },
  download(_paths: string[], options: any) {
    options.onProgress?.({ received: 300_000, size: 1_200_000 });
    return new Promise((_, reject) =>
      options.signal?.addEventListener("abort", () =>
        reject(new FileError({ code: "cancelled", message: "Cancelled" })),
      ),
    );
  },
} as unknown as FilesClient;
// The save picker would ask; downloads go to the browser's instead.
(window as any).showSaveFilePicker = undefined;
/** Where a terminal would be. */
const placeholder = { flex: 1, minHeight: 0, background: "var(--bg)" };

function Fixture() {
  const device = useDevice();
  const phone = device.mode === "phone";
  const [notifications, setNotifications] = useState(initial);
  const [current, setCurrent] = useState(1);
  const [dialog, setDialog] = useState(0);
  const [saved, setSaved] = useState(query.get("folded") === "1");
  const [portraitFolded, setPortraitFolded] = useState(true);
  const [phoneList, setPhoneList] = useState(!query.has("pane"));
  const folded = sidebarFolded(
    device.mode,
    device.orientation,
    saved,
    portraitFolded,
  );
  // The view, as main.tsx keeps it: the grid entry opens the grid and a
  // tab opens its terminal alone. Phones have no grid.
  const [view, setView] = useState<SessionMode>(
    query.get("view") === "grid" ? "grid" : "single",
  );
  const grid = view === "grid" && !phone;
  // The sidebar's sections, kept in the real preferences store.
  const [sections, setSections] = useState(
    () => loadPreferences().sidebarSections,
  );
  // Files (files=1): two open files, the second with unsaved changes;
  // the Files header shows the browser, a file's row the file.
  const [docs, setDocs] = useState(
    filesOn
      ? [
          { path: "/home/alice/notes.md", name: "notes.md", dirty: false },
          { path: "/home/alice/app.py", name: "app.py", dirty: true },
        ]
      : [],
  );
  const [shownDoc, setShownDoc] = useState("");
  const files = view === "files";
  const focused = panes.find((p) => p.id === current)!;
  const unread = (p: NumberedPane) => unreadFor(notifications, p).length;
  const acknowledge = (ids: number[]) => {
    attention.markRead(ids);
    (window as any).acks.push(...ids);
    setNotifications((list) =>
      list.map((n) => (ids.includes(n.id) ? { ...n, read: true } : n)),
    );
  };
  const notified = panes.find((p) => p.id === dialog);
  const listShown = phone && phoneList;
  // Only a terminal shown alone is read; grid tiles keep their bells.
  const onScreen = listShown || grid || files ? [] : [current];
  useReadShown(
    panes.find((p) => p.id === readingPane(0, current, onScreen)),
    notifications,
    acknowledge,
  );
  // Attention toasts and the bell favicon, as main.tsx wires them.
  const [events, setEvents] = useState<NotificationEvents>(
    defaultNotificationEvents,
  );
  const attention = useAttentionToasts({
    ready: toastsOn,
    notifications,
    panes,
    events,
    shown: onScreen[0] || 0,
  });
  const ringing = toastsOn && bellRinging(notifications, panes, events);
  useEffect(() => {
    if (!toastsOn) return;
    let icon = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
    if (!icon) {
      icon = document.createElement("link");
      icon.rel = "icon";
      document.head.append(icon);
    }
    icon.href = activityIcon(panes, ringing).url;
  }, [ringing]);
  useEffect(() => {
    let id = 100;
    Object.assign(window, {
      notify: (pane: number) => {
        const added = note(id++, pane, "input_needed");
        setNotifications((list) => [...list, added]);
      },
      bell: (pane: number) => {
        const added = {
          ...note(id++, pane, "unknown"),
          title: `Bell · agent ${pane}`,
          at: Date.now() / 1000,
        };
        setNotifications((list) => [...list, added]);
      },
      setEvents,
    });
  }, []);
  /** Opens a terminal alone, leaving the grid, as main.tsx's openPane. */
  const openPane = (p: NumberedPane) => {
    setCurrent(p.id);
    setPhoneList(false);
    setView("single");
  };
  const cycle = (step: number) => {
    const index = panes.findIndex((p) => p.id === current);
    openPane(panes[(index + step + panes.length) % panes.length]);
  };
  // The opening and grid shortcuts, as main.tsx's keyboard handler.
  const [prefix, setPrefix] = useState(false);
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const toggle = () => setView(view === "grid" ? "single" : "grid");
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.code === "KeyG") {
        e.preventDefault();
        return toggle();
      }
      if (e.altKey && !e.ctrlKey && !e.metaKey) {
        if (e.code === "Tab") {
          e.preventDefault();
          return toggle();
        }
        if (/^Digit[0-9]$/.test(e.code)) {
          const pane = panes[(Number(e.code.slice(5)) || 10) - 1];
          if (pane) {
            e.preventDefault();
            openPane(pane);
          }
          return;
        }
      }
      if (e.ctrlKey && !e.altKey && e.key.toLowerCase() === "b" && !prefix) {
        e.preventDefault();
        return setPrefix(true);
      }
      if (!prefix || ["Shift", "Control", "Alt", "Meta"].includes(e.key))
        return;
      setPrefix(false);
      e.preventDefault();
      const action = prefixAction(e);
      if (typeof action === "object" && action) {
        const pane = panes[action.select - 1];
        if (pane) openPane(pane);
      } else if (action === "grid") setView("grid");
      else if (action === "next" || action === "focus-right") cycle(1);
      else if (action === "previous" || action === "focus-left") cycle(-1);
      else if (action === "next-attention") {
        const pane = nextPane(panes, current, needsAttention);
        if (pane) openPane(pane);
      }
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  });
  // The tree's actions, as main.tsx's fileActions.
  const toast = useToast();
  const openDoc = (path: string) => {
    setDocs((list) =>
      list.some((d) => d.path === path)
        ? list
        : [...list, { path, name: basename(path), dirty: false }],
    );
    setShownDoc(path);
    setPhoneList(false);
    setView("files");
  };
  useEffect(() => {
    Object.assign(window, { toast: (text: string) => toast(text) });
  }, [toast]);
  const fileActions: FileActionsProps = {
    client: fakeFiles,
    home: "/home/alice",
    onOpenFile: openDoc,
    onOpenBeside: openDoc,
    insertTarget: { number: focused.number, title: focused.title || "" },
    onInsertPath: (path) => (window as any).typed.push(path),
    onCopyPath: (path) => toast(`Copied ${path}`, { key: "copy" }),
    confirm: (action) => void action.buttons[0].action(),
    toast: (message, options) => toast(message, { key: "files", ...options }),
    onRenamed: (from, to) => {
      setDocs((list) =>
        list.map((d) =>
          d.path === from ? { ...d, path: to, name: basename(to) } : d,
        ),
      );
      setShownDoc((shown) => (shown === from ? to : shown));
    },
    onRemoved: (paths) =>
      setDocs((list) => list.filter((d) => !paths.includes(d.path))),
    upload: {
      unavailable: "No uploads here",
      limits: null,
      transport: {} as any,
    },
  };
  const sidebar = (
    <Sidebar
      username="alice"
      dev={false}
      folded={folded}
      touch={device.touch}
      phone={phone}
      mode={grid ? "grid" : files ? "files" : "single"}
      sections={sections}
      onSections={(next) => {
        setSections(next);
        savePreferences({ ...loadPreferences(), sidebarSections: next });
      }}
      files={
        filesOn
          ? {
              browsing: !shownDoc,
              docs: docs.map((d) => ({ ...d, current: d.path === shownDoc })),
              onBrowse: () => {
                setShownDoc("");
                setPhoneList(false);
                setView("files");
              },
              onOpenDoc: (path) => {
                setShownDoc(path);
                setPhoneList(false);
                setView("files");
              },
              onCloseDoc: (path) =>
                setDocs((list) => list.filter((d) => d.path !== path)),
              tree: fileActions,
            }
          : undefined
      }
      terminals={panes}
      parked={[]}
      notifications={notifications}
      current={grid || files ? undefined : current}
      vitals={{ cpu: 23, memory: 61, disks: [48, 92] }}
      diskMounts={["/", "/data"]}
      onDisksHover={() => {}}
      onMode={(next) => setView(next)}
      rowProps={() => ({})}
      rowClass={() => ""}
      colorOf={(p) => (p.id - 1) % 9}
      onOpen={openPane}
      onClose={() => {}}
      onPark={() => {}}
      onUnpark={() => {}}
      onMenu={(p) => setDialog(p.id)}
      onNew={() => {}}
      onState={() => {}}
      onDialog={() => {}}
      onResizeStart={() => {}}
      onToggleFold={() =>
        device.mode === "tablet" && device.orientation === "portrait"
          ? setPortraitFolded(!folded)
          : setSaved(!folded)
      }
    />
  );
  return (
    <div
      className={`kelpmux-shell ${device.touch ? "touch" : ""} ${phone ? "phone" : ""} ${listShown ? "phone-home" : ""}`}
      style={{ "--rail": `${folded ? 56 : 264}px` } as React.CSSProperties}
    >
      {!phone && <div className="rail-wrap">{sidebar}</div>}
      <div className={`app ${grid ? "grid" : "single"}`}>
        {phone && !listShown && !files && (
          <PhoneBar
            pane={focused}
            state={paneState(focused, notifications)}
            attention={1}
            onList={() => setPhoneList(true)}
            unread={unread(focused)}
            onNotifications={() => setDialog(focused.id)}
            onMenu={() => {}}
          />
        )}
        <div className="workspace">
          {listShown ? (
            <div className="phone-list">{sidebar}</div>
          ) : files ? (
            <div
              className="files-placeholder"
              data-files-shown={shownDoc || "browser"}
              style={placeholder}
            />
          ) : grid ? (
            <LiveGrid
              panes={panes}
              notifications={notifications}
              body={() => (
                <div className="terminal-placeholder" style={placeholder} />
              )}
              onOpen={openPane}
              onMenu={(p) => setDialog(p.id)}
              onNew={() => {}}
            />
          ) : (
            <div className="pane-grid tab-pane-layout">
              <section className="grid-pane first-row selected">
                {!phone && (
                  <header>
                    <PaneLabel
                      pane={focused}
                      state={paneState(focused, notifications)}
                    />
                    <span className="grid-folder">{basename(focused.cwd)}</span>
                    <div className="pane-actions">
                      <NotificationsButton
                        label={focused.title}
                        count={unread(focused)}
                        onOpen={() => setDialog(focused.id)}
                      />
                      <button
                        type="button"
                        className="icon-btn"
                        aria-label="Find"
                      >
                        <Icon name="text-search" size="sm" />
                      </button>
                      <button
                        type="button"
                        className="icon-btn"
                        aria-label="Close"
                      >
                        <Icon name="x" size="sm" />
                      </button>
                    </div>
                  </header>
                )}
                <textarea
                  className="terminal-placeholder"
                  aria-label={`Input for terminal ${focused.number}`}
                  style={{ ...placeholder, border: 0, resize: "none" }}
                  value=""
                  onChange={() => {}}
                  onKeyDown={(e) => (window as any).typed.push(e.key)}
                />
              </section>
            </div>
          )}
        </div>
        {notified && (
          <div
            className="modal-shade"
            onMouseDown={(e) => {
              if (e.target === e.currentTarget) setDialog(0);
            }}
          >
            <section
              className="modal panel-modal"
              role="dialog"
              aria-modal="true"
              aria-labelledby="modal-title"
              data-dialog="notifications"
            >
              <header>
                <h2 id="modal-title">Notifications · {notified.title}</h2>
                <button
                  className="icon-btn"
                  aria-label="Close dialog"
                  onClick={() => setDialog(0)}
                >
                  <Icon name="x" />
                </button>
              </header>
              <TerminalNotificationsPanel
                pane={notified}
                notifications={notifications}
                onAcknowledge={acknowledge}
              />
            </section>
          </div>
        )}
      </div>
      {toastsOn && (
        <AttentionToasts
          toasts={attention.toasts}
          panes={panes}
          onOpen={openPane}
          onDismiss={attention.dismiss}
        />
      )}
    </div>
  );
}
createRoot(document.getElementById("root")!).render(
  <ToastProvider aside={<Downloads />}>
    <Fixture />
  </ToastProvider>,
);
requestAnimationFrame(() =>
  requestAnimationFrame(() => {
    (window as any).fixtureReady = true;
  }),
);
