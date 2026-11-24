// Downloads from the file browser and the editor (KelpCore `saveDownload`,
// KelpServer `file-downloads`): a file as itself, a folder as a `.tar.gz`
// archive. Each running download shows a progress toast with a Cancel
// button; downloads keep running when the view that started them closes,
// so their state lives here rather than in a component.
import { useSyncExternalStore } from "react";
import { FileError, type FilesClient } from "@kelpcore/web/files/api.ts";
import { saveDownload, downloadName } from "@kelpcore/web/files/save.ts";
import { formatBytes } from "@kelpcore/web/uploads.ts";
import { Icon } from "../status";
import { errorMessage } from "./root";

export type RunningDownload = {
  id: number;
  name: string;
  /** Bytes saved so far. */
  received: number;
  /** A single file's size, or an archive's input bytes (with `input`). */
  total?: number;
  /** Archives: input bytes read so far, of `total`. */
  input?: number;
  abort: AbortController;
};

let running: RunningDownload[] = [];
let serial = 0;
const listeners = new Set<() => void>();
function publish(next: RunningDownload[]) {
  running = next;
  for (const listener of listeners) listener();
}
function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
export const useRunningDownloads = () =>
  useSyncExternalStore(subscribe, () => running);

type Notify = (message: string, options?: { tone?: "error" | "success" }) => void;

/** Downloads `path`: a folder (`folder`) as `<name>.tar.gz`, a file as
 * itself. Call it straight from the click: the save picker needs the
 * user's activation. */
export function startDownload(
  client: FilesClient,
  path: string,
  folder: boolean,
  notify: Notify,
) {
  const archive = folder ? ("tar.gz" as const) : undefined;
  const abort = new AbortController();
  const id = ++serial;
  const name = downloadName([path], archive);
  publish([...running, { id, name, received: 0, abort }]);
  const update = (change: Partial<RunningDownload>) =>
    publish(running.map((d) => (d.id === id ? { ...d, ...change } : d)));
  saveDownload(client, [path], {
    archive,
    signal: abort.signal,
    onProgress: (progress) =>
      update(
        archive
          ? {
              received: progress.received,
              input: progress.input,
              total: progress.inputBytes,
            }
          : { received: progress.received, total: progress.size },
      ),
  })
    .then((result) => {
      const notes = [
        result.skipped
          ? `${result.skipped} special or unreadable ${result.skipped === 1 ? "entry was" : "entries were"} left out`
          : "",
        result.changed
          ? `${result.changed} ${result.changed === 1 ? "file" : "files"} changed while being read`
          : "",
      ].filter(Boolean);
      notify(
        `Downloaded “${result.name}” (${formatBytes(result.size)})${notes.length ? `; ${notes.join(", ")}.` : ""}`,
        notes.length ? { tone: "error" } : undefined,
      );
    })
    .catch((failure) => {
      if (failure instanceof FileError && failure.code === "cancelled") return;
      notify(
        `Could not download “${name}”: ${errorMessage(failure)}`,
        { tone: "error" },
      );
    })
    .finally(() => publish(running.filter((d) => d.id !== id)));
}

/** The progress toasts of running downloads, mounted once, in the top
 * right toast stack (ToastProvider's `aside`). */
export function Downloads() {
  const downloads = useRunningDownloads();
  if (!downloads.length) return null;
  return (
    <div className="download-region" aria-label="Downloads">
      {downloads.map((d) => {
        const done = d.input ?? d.received;
        const percent =
          d.total && d.total > 0
            ? Math.min(100, Math.floor((done / d.total) * 100))
            : undefined;
        return (
          <div
            key={d.id}
            className="download-toast"
            role="status"
            data-download={d.name}
          >
            <Icon name="loader-circle" size="sm" className="spin" />
            <span className="grow">
              Downloading “{d.name}”…{" "}
              {percent !== undefined
                ? `${percent}%`
                : d.received
                  ? formatBytes(d.received)
                  : ""}
            </span>
            <button
              type="button"
              className="subtle small"
              aria-label={`Cancel the download of ${d.name}`}
              onClick={() => d.abort.abort()}
            >
              Cancel
            </button>
          </div>
        );
      })}
    </div>
  );
}
