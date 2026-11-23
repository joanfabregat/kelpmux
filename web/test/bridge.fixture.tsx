// The real Session (WASM core included), grid tile and terminal for
// test/bridge.mjs, connected to a real `kelpmux web-bridge` and server
// through a pipe instead of SSH: the page hands each outgoing record to
// `window.bridgeWrite` and receives the bridge's standard output through
// `window.bridgeRead`. Screens are kept as the application keeps them
// (src/main.tsx). With `?json=1` the session never opts in to binary
// screens, for comparison.
import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import { Session } from "@kelpcore/web/connection.ts";
import { Terminal, TerminalTile } from "@kelpcore/web/Terminal.tsx";
import { Renderer } from "@kelpcore/web/renderer.ts";
import "../src/styles/base.css";
import "../src/styles/shell.css";
import "@kelpcore/web/terminal.css";

const json = new URLSearchParams(location.search).has("json");
const w = window as any;
w.bridgeEvents = [];
w.bridgeErrors = [];
w.draws = [];
// Every frame either view draws, with its geometry and the time it ended.
const draw = Renderer.prototype.draw;
Renderer.prototype.draw = function (screen: any, ...rest: any[]) {
  const result = (draw as any).call(this, screen, ...rest);
  const last = screen.cells
    .slice((screen.rows - 1) * screen.cols)
    .map((cell: any) => cell.text)
    .join("");
  w.draws.push({
    cols: screen.cols,
    rows: screen.rows,
    full: last.includes("word"),
    at: performance.now(),
  });
  return result;
};
class PipeSession extends Session {
  send(message: any) {
    if (this.stopped) return;
    if (json && message?.Stream?.type === "binary_screens") return;
    const payload = new TextEncoder().encode(JSON.stringify(message));
    const frame = new Uint8Array(payload.length + 4);
    new DataView(frame.buffer).setUint32(0, payload.length);
    frame.set(payload, 4);
    w.bridgeWrite(Array.from(frame));
  }
}
const store = {
  screens: new Map(),
  events: new EventTarget(),
  leases: new Map(),
  session: null as any,
};
const session = new PipeSession(
  {} as any,
  {} as any,
  (event: any) => {
    const e = event.Stream;
    if (!e) return;
    w.bridgeEvents.push(e.type);
    if (e.type === "welcome") w.welcome = e;
    if (e.type === "error") w.bridgeErrors.push(e.message);
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
        }
      }
      store.leases = new Map(
        e.leases.map((lease: any) => [lease.pane, lease.owner]),
      );
      store.events.dispatchEvent(new Event("frame"));
    }
    if (e.type === "lease") {
      if (e.owner) store.leases.set(e.pane, e.owner);
      else store.leases.delete(e.pane);
      store.events.dispatchEvent(new Event("frame"));
    }
  },
  () => {},
  (message: string) => w.bridgeErrors.push(message),
);
store.session = session;
w.session = session;
// Base64 keeps the hand-off from Node cheap, so timings measure the page.
// Each read records when it started and ended.
w.reads = [];
w.bridgeRead = (base64: string) => {
  const start = performance.now();
  const bytes = (Uint8Array as any).fromBase64
    ? (Uint8Array as any).fromBase64(base64)
    : Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
  session.receive(bytes);
  w.reads.push({ start, end: performance.now(), bytes: bytes.length });
};

let show: (view: { pane: number; single: boolean } | null) => void = () => {};
function App() {
  const [view, setView] = useState<{ pane: number; single: boolean } | null>(
    null,
  );
  show = setView;
  if (!view) return null;
  const pane = {
    id: view.pane,
    number: 1,
    title: "Bridge fixture",
    cwd: "/tmp",
    status: "idle",
  } as any;
  return view.single ? (
    <div
      className="app tabs"
      style={{ "--sidebar-width": "0px", "--background": "#101113" } as any}
    >
      <div className="workspace">
        <div className="pane-grid tab-pane-layout">
          <section className="grid-pane first-row selected">
            <Terminal
              pane={pane}
              store={store as any}
              active
              layout="tabs"
              onActivate={() => {}}
              onError={(error) => w.bridgeErrors.push(String(error))}
            />
          </section>
        </div>
      </div>
    </div>
  ) : (
    <div style={{ width: 480, height: 320, display: "flex" }}>
      <TerminalTile pane={pane} store={store as any} />
    </div>
  );
}
createRoot(document.getElementById("root")!).render(<App />);
w.showTile = (pane: number) => show({ pane, single: false });
w.showSingle = (pane: number) => show({ pane, single: true });
(async () => {
  const module = await WebAssembly.instantiateStreaming(
    fetch("/client-core.wasm"),
  );
  session.wasm = module.instance.exports;
  session.wasm.reset();
  session.send({ Stream: { type: "hello", version: 1 } });
  w.fixtureReady = true;
})();
