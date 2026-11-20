import React from "react";
import { createRoot } from "react-dom/client";
import { Terminal } from "../src/terminal/Terminal";
import "../src/styles/base.css";
import "../src/terminal/terminal.css";
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
  controlled: new Set([1]),
  capabilities: new Set(),
  control() {},
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
createRoot(document.getElementById("root")!).render(
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
            store={
              {
                screens: new Map([[1, { pane: 1, revision: 1, screen }]]),
                events,
                leases: new Map([[1, 1]]),
                session: session as any,
              } as any
            }
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
  </div>,
);
requestAnimationFrame(() =>
  requestAnimationFrame(() => {
    (window as any).terminalReady = true;
  }),
);
