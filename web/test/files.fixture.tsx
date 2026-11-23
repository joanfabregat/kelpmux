// The file browser and editor as main.tsx wires them (the Files entry and
// open files in the real Sidebar, the browser, the editor, path drops on
// terminal rows, uploads into folders), over the real Session and WASM core
// connected to a real `kelpmux web-bridge` and server through a pipe instead
// of SSH, as test/bridge.fixture.tsx does (test/files.mjs). `?phone=1`
// shows the phone layout (no rail: the main area only).
// `window.screenText(pane)` reads a terminal's screen, `window.toasts` the
// toasts shown and `window.openTerminal(pane, title)` adds a terminal row.
import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { Session } from "@kelpcore/web/connection.ts";
import type { NumberedPane } from "@kelpcore/web/types.ts";
import {
  FilesClient,
  isFileEvent,
  readFileLimits,
  type FilesTransport,
} from "@kelpcore/web/files/api.ts";
import {
  baseName,
  parentPath,
  typedPaths,
} from "@kelpcore/web/files/paths.ts";
import {
  readUploadLimits,
  type UploadLimits,
  type UploadTransport,
} from "@kelpcore/web/uploads.ts";
import { Sidebar } from "../src/Sidebar";
import { FilesBrowser } from "../src/files/FilesBrowser";
import { FileEditor } from "../src/files/FileEditor";
import { useDocuments } from "../src/files/documents";
import { dragHasPaths, droppedPaths, typeIntoPane } from "../src/files/insert";
import { ConfirmDialog, type ConfirmAction } from "../src/Confirm";
import { interfaceVariables, themeVariables } from "../src/appearance";
import "../src/styles/base.css";
import "../src/styles/shell.css";
import "@kelpcore/web/terminal.css";
import "../src/styles/dialogs.css";
import "../src/styles/kelpmux.css";
import "../src/styles/state.css";
import "../src/styles/mobile.css";
import "../src/styles/files.css";

const phone = new URLSearchParams(location.search).has("phone");
const root = document.documentElement;
root.dataset.mode = "dark";
for (const [key, value] of Object.entries({
  ...themeVariables("black", "dark"),
  ...interfaceVariables("black", "dark"),
}))
  root.style.setProperty(key, value);

const w = window as any;
w.bridgeErrors = [];
w.toasts = [];
class PipeSession extends Session {
  send(message: any) {
    if (this.stopped) return;
    // JSON screens keep `screenText` simple.
    if (message?.Stream?.type === "binary_screens") return;
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
function isUploadEvent(e: any) {
  return (
    typeof e.type === "string" &&
    (e.type.startsWith("upload_") ||
      (["done", "error"].includes(e.type) &&
        typeof e.request === "string" &&
        e.request.startsWith("upload-")))
  );
}
let onWelcome: (e: any) => void = () => {};
const session = new PipeSession(
  {} as any,
  {} as any,
  (event: any) => {
    const e = event.Stream;
    if (!e) return;
    if (e.type === "welcome") onWelcome(e);
    if (isUploadEvent(e)) {
      store.events.dispatchEvent(new CustomEvent("upload-event", { detail: e }));
      return;
    }
    if (isFileEvent(e)) {
      store.events.dispatchEvent(new CustomEvent("file-event", { detail: e }));
      return;
    }
    if (e.type === "error") w.bridgeErrors.push(e.message);
    if (e.type === "update") {
      for (const s of e.snapshots) store.screens.set(s.pane, s);
      for (const patch of e.patches) {
        const previous = store.screens.get(patch.pane);
        if (previous)
          for (const [index, cell] of patch.changes)
            previous.screen.cells[index] = cell;
      }
    }
  },
  () => {},
  (message: string) => w.bridgeErrors.push(message),
);
store.session = session;
w.session = session;
w.bridgeRead = (base64: string) =>
  session.receive(Uint8Array.from(atob(base64), (c) => c.charCodeAt(0)));
w.screenText = (pane: number) => {
  const s = store.screens.get(pane);
  if (!s) return "";
  const { cols, cells } = s.screen;
  let text = "";
  cells.forEach((cell: any, i: number) => {
    text += cell.text || " ";
    if ((i + 1) % cols === 0) text += "\n";
  });
  return text;
};

const transport = (name: string) => ({
  send(type: string, params: any) {
    session.request(type, params);
  },
  listen(handler: (e: any) => void) {
    const listener = (event: Event) => handler((event as CustomEvent).detail);
    store.events.addEventListener(name, listener);
    return () => store.events.removeEventListener(name, listener);
  },
});
const filesTransport: FilesTransport = transport("file-event");
const uploadTransport: UploadTransport = transport("upload-event");

function App() {
  const [client, setClient] = useState<FilesClient | null>(null);
  const [uploadLimits, setUploadLimits] = useState<UploadLimits | null>(null);
  const [home, setHome] = useState<string>();
  const [dir, setDir] = useState("~");
  const [shown, setShown] = useState("");
  const [browsing, setBrowsing] = useState(true);
  const [confirm, setConfirm] = useState<ConfirmAction | null>(null);
  const [panes, setPanes] = useState<NumberedPane[]>([]);
  const [dropPane, setDropPane] = useState(0);
  const documents = useDocuments(client);
  useEffect(() => {
    onWelcome = (e) => {
      setUploadLimits(readUploadLimits(e.upload_limits));
      const limits = readFileLimits(e.file_limits);
      if (!e.capabilities.includes("files") || !limits) return;
      const c = new FilesClient(filesTransport, { limits });
      setClient(c);
      c.stat("~").then((r) => setHome(r.path));
      w.welcome = e;
    };
    w.openTerminal = (id: number, title: string) =>
      setPanes((all) => [
        ...all,
        { id, number: all.length + 1, title, cwd: "/", status: "idle" } as any,
      ]);
  }, []);
  const toast = (message: string, options?: any) =>
    w.toasts.push({ message, ...options });
  const doc = documents.docs.find((d) => d.path === shown);
  const openDoc = (path: string) => {
    documents.open(path);
    setShown(path);
    setBrowsing(false);
  };
  const closeDoc = (path: string) => {
    const d = documents.docs.find((x) => x.path === path);
    const close = () => {
      documents.close(path);
      if (shown === path) setShown("");
      setBrowsing(true);
    };
    if (!d?.dirty) return close();
    setConfirm({
      title: "Discard changes",
      text: `“${baseName(path)}” has unsaved changes.`,
      buttons: [{ label: "Discard changes", color: "danger", action: close }],
    });
  };
  // As main.tsx's pathDropProps and insertPaths.
  const insertPaths = (paths: string[], pane: NumberedPane) => {
    const text = typedPaths(paths);
    if (text === null) return toast("control characters", { tone: "error" });
    typeIntoPane(store as any, pane.id, text);
    toast(`Inserted into terminal ${pane.number}`);
  };
  const pathDropProps = (p: NumberedPane) => ({
    onDragOver: (event: React.DragEvent) => {
      if (!dragHasPaths(event)) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = "copy";
      setDropPane(p.id);
    },
    onDragLeave: (event: React.DragEvent) => {
      if (!event.currentTarget.contains(event.relatedTarget as Node))
        setDropPane(0);
    },
    onDrop: (event: React.DragEvent) => {
      if (!dragHasPaths(event)) return;
      event.preventDefault();
      setDropPane(0);
      insertPaths(droppedPaths(event.dataTransfer), p);
    },
  });
  if (!client) return null;
  const target = panes[0];
  const main =
    !browsing && doc ? (
      <FileEditor
        key={doc.id}
        doc={doc}
        documents={documents}
        home={home}
        maxRead={client.limits.max_read_bytes}
        onClose={() => closeDoc(doc.path)}
        onReveal={() => {
          setDir(parentPath(doc.path));
          setBrowsing(true);
        }}
        onBack={phone ? () => {} : undefined}
      />
    ) : (
      <FilesBrowser
        client={client}
        home={home}
        dir={dir}
        onDir={setDir}
        onOpenFile={openDoc}
        insertTarget={
          target ? { number: target.number, title: target.title } : undefined
        }
        onInsertPath={(path) => target && insertPaths([path], target)}
        onCopyPath={(path) => toast(`copied ${path}`)}
        confirm={setConfirm}
        toast={toast}
        onRenamed={documents.renamed}
        onRemoved={documents.removed}
        upload={{
          unavailable: null,
          limits: uploadLimits,
          transport: uploadTransport,
        }}
        onBack={phone ? () => {} : undefined}
      />
    );
  return (
    <div
      className={`kelpmux-shell ${phone ? "phone touch" : ""}`}
      style={{ "--rail": "264px" } as React.CSSProperties}
    >
      {!phone && (
        <div className="rail-wrap">
          <Sidebar
            username="alice"
            dev={false}
            folded={false}
            mode="files"
            terminals={panes}
            parked={[]}
            notifications={[]}
            vitals={{}}
            diskMounts={[]}
            onDisksHover={() => {}}
            onMode={() => {}}
            rowProps={pathDropProps}
            rowClass={(p) => (dropPane === p.id ? "drop-target" : "")}
            onOpen={() => {}}
            onClose={() => {}}
            onPark={() => {}}
            onUnpark={() => {}}
            onMenu={() => {}}
            onNew={() => {}}
            onState={() => {}}
            onDialog={() => {}}
            onResizeStart={() => {}}
            onToggleFold={() => {}}
            files={{
              browsing,
              docs: documents.docs.map((d) => ({
                path: d.path,
                name: baseName(d.path),
                dirty: d.dirty,
                current: !browsing && d.path === shown,
              })),
              onBrowse: () => setBrowsing(true),
              onOpenDoc: (path) => {
                setShown(path);
                setBrowsing(false);
              },
              onCloseDoc: closeDoc,
            }}
          />
        </div>
      )}
      <div className="app single">
        <div className="workspace">{main}</div>
      </div>
      {confirm && (
        <ConfirmDialog
          action={confirm}
          pending={false}
          onCancel={() => setConfirm(null)}
          onApprove={async (index) => {
            await confirm.buttons[index].action();
            setConfirm(null);
          }}
        />
      )}
    </div>
  );
}
createRoot(document.getElementById("root")!).render(<App />);
(async () => {
  const module = await WebAssembly.instantiateStreaming(
    fetch("/client-core.wasm"),
  );
  session.wasm = module.instance.exports;
  session.wasm.reset();
  session.send({ Stream: { type: "hello", version: 1 } });
  w.fixtureReady = true;
})();
