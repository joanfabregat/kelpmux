// The real React terminal for test/terminal.mjs, without SSH: a fake
// session records every request in `window.terminalRequests`. With `?tile=1`
// it renders the read-only grid instead (the real LiveGrid and
// TerminalTile), recording opened tiles in `window.opened`.
import React from "react";
import { createRoot } from "react-dom/client";
import { Terminal, TerminalTile } from "@kelpcore/web/Terminal.tsx";
import { LiveGrid } from "../src/LiveGrid";
import "../src/styles/base.css";
import "../src/styles/shell.css";
import "@kelpcore/web/terminal.css";
const tile = new URLSearchParams(location.search).has("tile");
const cols = 80,
  rows = 24;
const line = "Browser terminal selection is copyable";
const screen = {
  cols,
  rows,
  modes: {},
  cells: Array.from({ length: cols * rows }, (_, i) => ({
    text: i < line.length ? line[i] : " ",
    width: 1,
    fg: "Default",
    bg: "Default",
    bold: false,
    italic: false,
    underline: false,
  })),
};
const events = new EventTarget();
const session = {
  // A tile claims the lease for its size; the terminal holds it already.
  controlled: new Set(tile ? [] : [1]),
  capabilities: new Set(),
  control(pane: number, size?: unknown) {
    session.controlled.add(pane);
    (window as any).terminalRequests.push({ type: "control", pane, size });
  },
  request(type: string, params: unknown) {
    (window as any).terminalRequests.push({ type, params });
  },
  select(_pane: number, start: any, end: any) {
    const a = start.y * cols + start.x,
      b = end.y * cols + end.x;
    return screen.cells
      .slice(Math.min(a, b), Math.max(a, b) + 1)
      .map((c) => c.text)
      .join("");
  },
};
Object.assign(window, {
  terminalRequests: [],
  opened: [],
  terminalReady: false,
  terminalClipboard: (text: string) =>
    events.dispatchEvent(
      new CustomEvent("clipboard", { detail: { pane: 1, text } }),
    ),
});
const pane = {
  id: 1,
  number: 1,
  title: "Clipboard fixture",
  cwd: "/tmp",
  status: "Idle",
};
const store = {
  screens: new Map([[1, { pane: 1, revision: 1, screen }]]),
  events,
  leases: new Map([[1, 1]]),
  session: session as any,
} as any;
createRoot(document.getElementById("root")!).render(
  tile ? (
    <div
      className="app grid"
      style={{ display: "flex", width: "100vw", height: "100vh" } as any}
    >
      <div className="workspace">
        <LiveGrid
          panes={[pane as any]}
          notifications={[]}
          body={(p) => <TerminalTile pane={p} store={store} />}
          onOpen={(p) => (window as any).opened.push(p.id)}
          onMenu={() => {}}
          onNew={() => {}}
        />
      </div>
    </div>
  ) : (
    <div
      className="app tabs"
      style={{ "--sidebar-width": "240px", "--background": "#101113" } as any}
    >
      <aside className="sidebar" />
      <div className="workspace">
        <div className="pane-grid tab-pane-layout">
          <section className="grid-pane first-row selected">
            <Terminal
              pane={pane as any}
              store={store}
              active
              layout="tabs"
              onActivate={() => {}}
              onError={(error) => {
                throw error;
              }}
            />
          </section>
        </div>
      </div>
    </div>
  ),
);
requestAnimationFrame(() =>
  requestAnimationFrame(() => {
    (window as any).terminalReady = true;
  }),
);
