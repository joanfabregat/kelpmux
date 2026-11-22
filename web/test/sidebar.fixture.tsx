// The sidebar, a terminal's header and its notifications panel as main.tsx
// wires them, with fixed terminals and notifications instead of SSH
// (test/sidebar.mjs). Showing a terminal (or focusing its grid tile) marks
// its notifications read through the same hook as main.tsx; terminal 1,
// which has none, is focused first. Right-clicking a row opens that
// terminal's notifications, as its menu's Notifications… item does, and
// `window.notify(pane)` adds an unread notification. The query string picks
// the case: `mode=light`, `tabs=N`, `view=grid`, `folded=1`.
import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import type { Pane } from "../src/terminal/types";
import { Sidebar } from "../src/Sidebar";
import { NotificationsButton, PaneLabel, UnreadMark } from "../src/Shell";
import { TerminalNotificationsPanel } from "../src/Panels";
import { PhoneBar, useDevice } from "../src/Mobile";
import { Icon } from "../src/status";
import { basename } from "../src/format";
import { sidebarFolded } from "../src/device";
import { interfaceVariables, themeVariables } from "../src/appearance";
import {
  paneState,
  readingPane,
  unreadFor,
  type Notification,
} from "../src/state/model";
import { useReadShown } from "../src/state/read";
import "../src/styles/base.css";
import "../src/styles/shell.css";
import "../src/terminal/terminal.css";
import "../src/styles/dialogs.css";
import "../src/styles/login.css";
import "../src/styles/kelpmux.css";
import "../src/styles/state.css";
import "../src/styles/mobile.css";

const query = new URLSearchParams(location.search);
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
const panes: Pane[] = Array.from(
  { length: Number(query.get("tabs")) || 10 },
  (_, i) => ({
    id: i + 1,
    number: i + 1,
    pid: 100 + i + 1,
    cwd: folders[i % folders.length],
    title: kinds[i % 3] ? `${kinds[i % 3]} ${i + 1}` : "shell",
    status: statuses[i % statuses.length],
    parked: false,
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
Object.assign(window, { acks: [] as number[] });
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
  const grid = query.get("view") === "grid";
  const focused = panes.find((p) => p.id === current)!;
  const unread = (p: Pane) => unreadFor(notifications, p).length;
  const acknowledge = (ids: number[]) => {
    (window as any).acks.push(...ids);
    setNotifications((list) =>
      list.map((n) => (ids.includes(n.id) ? { ...n, read: true } : n)),
    );
  };
  const notified = panes.find((p) => p.id === dialog);
  const listShown = phone && phoneList;
  const onScreen = listShown
    ? []
    : grid
      ? panes.slice(0, 6).map((p) => p.id)
      : [current];
  useReadShown(
    panes.find((p) => p.id === readingPane(0, current, onScreen)),
    notifications,
    acknowledge,
  );
  useEffect(() => {
    let id = 100;
    Object.assign(window, {
      notify: (pane: number) => {
        const added = note(id++, pane, "input_needed");
        setNotifications((list) => [...list, added]);
      },
    });
  }, []);
  const sidebar = (
    <Sidebar
      username="alice"
      dev={false}
      folded={folded}
      touch={device.touch}
      phone={phone}
      mode={grid ? "grid" : "single"}
      page={0}
      pages={1}
      terminals={panes}
      parked={[]}
      notifications={notifications}
      current={current}
      vitals={{ cpu: 23, memory: 61, disks: [48, 92] }}
      diskMounts={["/", "/data"]}
      onDisksHover={() => {}}
      onMode={() => {}}
      onPage={() => {}}
      rowProps={() => ({})}
      rowClass={() => ""}
      colorOf={(p) => (p.id - 1) % 9}
      onOpen={(p) => {
        setCurrent(p.id);
        setPhoneList(false);
      }}
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
        {phone && !listShown && (
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
          ) : grid ? (
            <div
              className="pane-grid live-grid"
              style={{
                gridTemplateColumns: "repeat(3, minmax(0, 1fr))",
                gridTemplateRows: "repeat(2, minmax(0, 1fr))",
              }}
            >
              {panes.slice(0, 6).map((p) => (
                <button
                  type="button"
                  key={p.id}
                  className={`grid-pane live-tile ${p.id === current ? "focused" : ""}`}
                  aria-current={p.id === current ? "true" : undefined}
                  data-tile-pane={p.id}
                  onClick={() => setCurrent(p.id)}
                >
                  <header>
                    <PaneLabel pane={p} state={paneState(p, notifications)} />
                    <span className="grid-folder">{basename(p.cwd)}</span>
                    <UnreadMark count={unread(p)} />
                  </header>
                  <div className="terminal-placeholder" style={placeholder} />
                </button>
              ))}
            </div>
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
                <div className="terminal-placeholder" style={placeholder} />
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
    </div>
  );
}
createRoot(document.getElementById("root")!).render(<Fixture />);
requestAnimationFrame(() =>
  requestAnimationFrame(() => {
    (window as any).fixtureReady = true;
  }),
);
