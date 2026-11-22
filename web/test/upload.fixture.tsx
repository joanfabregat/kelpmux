// The upload dialog as main.tsx wires it, with a terminal drop target and a
// fake server instead of SSH (test/upload.mjs). The fake answers like
// KelpServer's `uploads`: `upload_started`, one `upload_progress` per chunk
// (held while `window.hold` is set), `upload_done` with the final paths, and
// `done` for a cancel. `window.sent` records every request, `window.pasted`
// what would be pasted into terminal 1. The query string picks the case:
// `unavailable=1` (a server without uploads), `closed=1` (the terminal closes
// during the upload), `max_file=N` (a smaller per-file limit).
import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import type { NumberedPane } from "../src/terminal/types";
import {
  UploadDialog,
  UploadSettings,
  dragHasFiles,
  droppedFiles,
  loadUploadDestination,
  saveUploadDestination,
  type UploadDrop,
} from "../src/Upload";
import {
  pastedPaths,
  type UploadLimits,
  type UploadTransport,
} from "../src/terminal/uploads";
import { interfaceVariables, themeVariables } from "../src/appearance";
import "../src/styles/base.css";
import "../src/styles/shell.css";
import "../src/terminal/terminal.css";
import "../src/styles/dialogs.css";
import "../src/styles/kelpmux.css";

const query = new URLSearchParams(location.search);
const root = document.documentElement;
root.dataset.mode = "dark";
for (const [key, value] of Object.entries({
  ...themeVariables("black", "dark"),
  ...interfaceVariables("black", "dark"),
}))
  root.style.setProperty(key, value);

const w = window as any;
w.sent = [];
w.pasted = null;
w.hold = false;
const limits: UploadLimits = {
  max_files: 64,
  max_file_bytes: Number(query.get("max_file")) || 100 * 2 ** 20,
  max_batch_bytes: 2 ** 30,
  max_chunk_bytes: 256 * 1024,
  max_chunks_in_flight: 8,
  max_batches: 4,
};
let handler: ((event: any) => void) | undefined;
const held: any[] = [];
const emit = (event: any) => setTimeout(() => handler?.(event), 5);
let names: string[] = [],
  dir = "",
  received = new Map<number, number>();
w.release = () => {
  for (const event of held.splice(0)) emit(event);
};
const transport: UploadTransport = {
  send(type, params) {
    w.sent.push({ type, ...params, data: params.data?.length });
    if (type === "upload_begin") {
      names = params.files.map((f: any) => f.name);
      dir = `${params.dest || "/home/me/.cache/kelpserver/uploads"}/1795300000-3f9c`;
      received = new Map();
      if (params.dest === "/refused")
        emit({
          type: "error",
          request: params.request,
          message: "destination is not a directory owned by the server's user",
        });
      else
        emit({
          type: "upload_started",
          request: params.request,
          batch: 1,
          dir,
        });
    } else if (type === "upload_chunk") {
      const bytes = atob(params.data).length;
      const total = (received.get(params.file_index) || 0) + bytes;
      received.set(params.file_index, total);
      const event = {
        type: "upload_progress",
        batch: params.batch,
        file_index: params.file_index,
        received: total,
      };
      if (w.hold) held.push(event);
      else emit(event);
    } else if (type === "upload_finish")
      emit({
        type: "upload_done",
        request: params.request,
        batch: params.batch,
        paths: names.map((name) => `${dir}/${name}`),
      });
    else if (type === "upload_cancel")
      emit({ type: "done", request: params.request });
  },
  listen(fn) {
    handler = fn;
    return () => {
      handler = undefined;
    };
  },
};
const pane: NumberedPane = {
  id: 1,
  number: 1,
  pid: 101,
  cwd: "/home/me/project",
  title: "shell",
  status: "idle",
  dead: false,
};

function Fixture() {
  const [drop, setDrop] = useState<UploadDrop | null>(null);
  const [over, setOver] = useState(false);
  const [destination, setDestination] = useState(loadUploadDestination);
  const [settings, setSettings] = useState(false);
  const unavailable = query.get("unavailable")
    ? "This workspace's terminal server cannot receive files (it lacks the uploads capability). Restart the workspace with a current KelpServer to upload files to terminals."
    : null;
  return (
    <div style={{ display: "grid", height: "100vh", padding: 16 }}>
      <button onClick={() => setSettings(!settings)}>Settings</button>
      {settings && (
        <UploadSettings
          destination={destination}
          onChange={(path) => {
            saveUploadDestination(path);
            setDestination(path);
          }}
          unavailable={unavailable}
        />
      )}
      <section
        className={`grid-pane selected ${over ? "drop-target" : ""}`}
        data-upload-pane={1}
        aria-label="Terminal 1"
        onDragOver={(event) => {
          if (!dragHasFiles(event)) return;
          event.preventDefault();
          setOver(true);
        }}
        onDragLeave={() => setOver(false)}
        onDrop={(event) => {
          if (!dragHasFiles(event)) return;
          event.preventDefault();
          setOver(false);
          const { files, folders } = droppedFiles(event.dataTransfer);
          if (files.length || folders.length || unavailable)
            setDrop({ pane, files, folders });
        }}
      >
        <header>Terminal 1 · shell</header>
        <input
          type="file"
          multiple
          data-upload-picker
          onChange={(event) => {
            const files = Array.from(event.currentTarget.files || []);
            if (files.length) setDrop({ pane, files, folders: [] });
          }}
        />
      </section>
      {drop && (
        <UploadDialog
          drop={drop}
          unavailable={unavailable}
          limits={limits}
          transport={transport}
          defaultDestination={destination}
          onRememberDestination={(path) => {
            saveUploadDestination(path);
            setDestination(path);
          }}
          onUploaded={(paths) => {
            if (query.get("closed"))
              return "Terminal 1 closed while the files were uploading.";
            w.pasted = pastedPaths(paths);
            return null;
          }}
          onCopy={(text) => {
            w.copied = text;
          }}
          onClose={() => setDrop(null)}
        />
      )}
    </div>
  );
}
createRoot(document.getElementById("root")!).render(<Fixture />);
w.fixtureReady = true;
