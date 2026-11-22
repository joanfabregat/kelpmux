// The touch terminal and key bar as main.tsx wires them on phones and
// tablets, with a recording session instead of SSH (test/mobile.mjs).
import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import { Terminal } from "../src/terminal/Terminal";
import { KeyBar, useDevice, useVisualViewport } from "../src/Mobile";
import {
  barKeyBytes,
  consumesCtrl,
  nextInputMode,
  type BarKey,
  type InputMode,
} from "../src/terminal/touch-keys";
import "../src/styles/base.css";
import "../src/styles/shell.css";
import "../src/terminal/terminal.css";
import "../src/styles/kelpmux.css";
import "../src/styles/mobile.css";
const cols = 80,
  rows = 24;
const line = "Touch terminal words can be selected";
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
const record = (type: string, params: unknown) =>
  (window as any).terminalRequests.push({ type, params });
const session = {
  client: 1,
  controlled: new Set<number>(),
  capabilities: new Set(),
  control(pane: number) {
    session.controlled.add(pane);
    record("control", { pane });
  },
  request: record,
  async rpc(type: string, params: unknown) {
    record(type, params);
    return {
      History: {
        lines: Array.from({ length: 200 }, (_, i) => [-200 + i, `old ${i}`]),
      },
    };
  },
  historyScreen(lines: string[], width: number, height: number) {
    (window as any).historyShown = lines[0];
    return {
      ...screen,
      cols: width,
      rows: height,
      cells: Array.from({ length: width * height }, (_, i) => ({
        ...screen.cells[0],
        text: lines[Math.floor(i / width)]?.[i % width] || " ",
      })),
    };
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
const store = {
  screens: new Map([[1, { pane: 1, revision: 1, screen }]]),
  events,
  leases: new Map(),
  closing: new Set(),
  session,
} as any;
Object.assign(window, { terminalRequests: [], terminalReady: false });
const pane = {
  id: 1,
  number: 1,
  title: "Touch fixture",
  cwd: "/tmp",
  status: "Idle",
  kind: "codex",
} as any;

function Fixture() {
  const device = useDevice();
  useVisualViewport(device.touch);
  const [mode, setMode] = useState<InputMode>("reading");
  const [ctrl, setCtrl] = useState(false);
  const keyboard = (show: boolean) =>
    events.dispatchEvent(
      new CustomEvent("terminal-keyboard", { detail: { pane: 1, show } }),
    );
  const input = (bytes: string, paste = false) =>
    events.dispatchEvent(
      new CustomEvent("terminal-input", { detail: { pane: 1, bytes, paste } }),
    );
  function onKey(key: BarKey) {
    keyboard(true);
    setMode((m) => nextInputMode(m, "key-bar"));
    if (key === "Ctrl") return setCtrl(!ctrl);
    const bytes = barKeyBytes(key, ctrl, screen.modes, pane.kind);
    if (consumesCtrl(key)) setCtrl(false);
    if (bytes) input(bytes);
  }
  (window as any).inputMode = mode;
  return (
    <div className={`kelpmux-shell ${device.touch ? "touch" : ""}`}>
      <div className="app single">
        <div className="workspace">
          <div className="pane-grid tab-pane-layout">
            <section className="grid-pane first-row selected">
              <Terminal
                pane={pane}
                store={store}
                active
                layout="tabs"
                onActivate={() => {}}
                onError={(error) => {
                  throw error;
                }}
                touchKeyboard={device.touch}
                onInputFocus={(focus) =>
                  setMode((m) => nextInputMode(m, focus ? "focus" : "blur"))
                }
                stickyCtrl={ctrl}
                onCtrlUsed={() => setCtrl(false)}
              />
            </section>
          </div>
        </div>
        <KeyBar
          ctrl={ctrl}
          prefix={false}
          input={mode === "input"}
          onKey={onKey}
          onToggleKeyboard={() => {
            const next = nextInputMode(mode, "input-control");
            keyboard(next === "input");
            setMode(next);
          }}
        />
      </div>
    </div>
  );
}
createRoot(document.getElementById("root")!).render(<Fixture />);
requestAnimationFrame(() =>
  requestAnimationFrame(() => {
    (window as any).terminalReady = true;
  }),
);
