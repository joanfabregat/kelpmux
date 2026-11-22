// The upload dialog: files dropped (or pasted, or picked) onto a terminal go
// to the host where its shells run, then their shell-quoted paths are pasted
// into that terminal. The protocol lives in terminal/uploads.ts.
import React, { useEffect, useRef, useState } from "react";
import type { Pane } from "./terminal/types";
import {
  cancelled,
  destinationProblem,
  formatBytes,
  normalizeDestination,
  pastedPaths,
  upload,
  uploadProblem,
  type UploadLimits,
  type UploadProgress,
  type UploadTransport,
} from "./terminal/uploads";
import { Icon } from "./status";

const destinationKey = "kelpmux-web-upload-destination";
/** The default destination folder chosen in Settings → Uploads (this
 * browser only); empty for the server's upload folder. */
export function loadUploadDestination(): string {
  try {
    const value = localStorage.getItem(destinationKey) || "";
    return destinationProblem(value) ? "" : value;
  } catch {
    return "";
  }
}
export function saveUploadDestination(path: string) {
  try {
    if (path) localStorage.setItem(destinationKey, path);
    else localStorage.removeItem(destinationKey);
  } catch {}
}

/** Files dropped onto a terminal, before the dialog opens. */
export type UploadDrop = {
  pane: Pane;
  files: File[];
  /** Names of dropped folders, which are not uploaded. */
  folders: string[];
};

/** Splits a drop's items into files and folders (a folder dropped in a
 * browser arrives as an entry that is a directory). */
export function droppedFiles(data: DataTransfer): {
  files: File[];
  folders: string[];
} {
  const files: File[] = [],
    folders: string[] = [];
  const items = Array.from(data.items || []).filter(
    (item) => item.kind === "file",
  );
  if (items.length) {
    for (const item of items) {
      const entry = (item as any).webkitGetAsEntry?.();
      const file = item.getAsFile();
      if (entry?.isDirectory)
        folders.push(entry.name || file?.name || "folder");
      else if (file) files.push(file);
    }
  } else files.push(...Array.from(data.files || []));
  return { files, folders };
}

/** Whether a drag carries files (not text or a link). */
export function dragHasFiles(event: React.DragEvent | DragEvent) {
  return Array.from(event.dataTransfer?.types || []).includes("Files");
}

type Phase =
  | { kind: "review" }
  | { kind: "uploading"; progress: UploadProgress }
  | { kind: "done"; paths: string[]; reason: string }
  | { kind: "failed"; message: string };

export function UploadDialog({
  drop,
  unavailable,
  limits,
  transport,
  defaultDestination,
  onRememberDestination,
  onUploaded,
  onCopy,
  onClose,
}: {
  drop: UploadDrop;
  /** Why this connection cannot upload, if it cannot. */
  unavailable: string | null;
  limits: UploadLimits | null;
  transport: UploadTransport | null;
  defaultDestination: string;
  onRememberDestination: (path: string) => void;
  /** Pastes the paths into the drop's terminal: null once pasted, else why
   * they could not be (the terminal closed). */
  onUploaded: (paths: string[]) => string | null;
  onCopy: (text: string) => void;
  onClose: () => void;
}) {
  const [phase, setPhase] = useState<Phase>({ kind: "review" });
  const [destination, setDestination] = useState(defaultDestination);
  const [remember, setRemember] = useState(false);
  const abort = useRef<AbortController | null>(null);
  const dialog = useRef<HTMLElement>(null);
  const { pane, files, folders } = drop;
  const dest = normalizeDestination(destination);
  const total = files.reduce((sum, file) => sum + file.size, 0);
  const problem =
    unavailable ||
    (!limits || !transport
      ? "This connection did not announce upload limits."
      : files.length
        ? uploadProblem(files, limits)
        : "Only folders were dropped; drop files instead.") ||
    destinationProblem(dest);
  const busy = phase.kind === "uploading";
  useEffect(() => {
    dialog.current
      ?.querySelector<HTMLElement>("[data-autofocus], button")
      ?.focus();
  }, [phase.kind]);
  useEffect(() => () => abort.current?.abort(), []);
  async function start() {
    if (problem || !limits || !transport) return;
    if (remember && dest !== defaultDestination) onRememberDestination(dest);
    const controller = new AbortController();
    abort.current = controller;
    setPhase({
      kind: "uploading",
      progress: { received: files.map(() => 0), done: 0, total },
    });
    try {
      const paths = await upload(transport, files, {
        limits,
        dest: dest || undefined,
        signal: controller.signal,
        onProgress: (progress) => setPhase({ kind: "uploading", progress }),
      });
      abort.current = null;
      const reason = onUploaded(paths);
      if (reason === null) onClose();
      else setPhase({ kind: "done", paths, reason });
    } catch (error) {
      abort.current = null;
      if (cancelled(error)) onClose();
      else
        setPhase({
          kind: "failed",
          message: error instanceof Error ? error.message : String(error),
        });
    }
  }
  const terminal = `Terminal ${pane.number} · ${pane.title || "shell"}`;
  return (
    <div className="modal-shade upload-shade">
      <section
        ref={dialog}
        className="upload-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="upload-title"
        data-dialog="upload"
        onKeyDown={(event) => {
          event.stopPropagation();
          if (event.key === "Escape" && !busy) onClose();
        }}
      >
        <header>
          <h2 id="upload-title">
            {phase.kind === "done" ? "Files uploaded" : "Upload files"}
          </h2>
          {!busy && (
            <button
              className="icon-btn"
              aria-label="Close upload dialog"
              onClick={onClose}
            >
              <Icon name="x" />
            </button>
          )}
        </header>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (phase.kind === "review") void start();
          }}
        >
          <p className="upload-summary">
            To <strong className="upload-target">{terminal}</strong>
            {files.length > 0 && (
              <>
                {" · "}
                {files.length} {files.length === 1 ? "file" : "files"},{" "}
                {formatBytes(total)}
              </>
            )}
          </p>
          {files.length > 0 && phase.kind !== "done" && (
            <ul className="upload-files" aria-label="Files">
              {files.map((file, index) => (
                <li key={index}>
                  <span>{file.name}</span>
                  <span>
                    {phase.kind === "uploading"
                      ? `${formatBytes(phase.progress.received[index] || 0)} / ${formatBytes(file.size)}`
                      : formatBytes(file.size)}
                  </span>
                  {phase.kind === "uploading" && (
                    <progress
                      aria-label={`${file.name} progress`}
                      max={Math.max(1, file.size)}
                      value={
                        file.size ? phase.progress.received[index] || 0 : 1
                      }
                    />
                  )}
                </li>
              ))}
            </ul>
          )}
          {folders.length > 0 && (
            <p className="muted upload-folders" role="note">
              Folders are not supported and are skipped: {folders.join(", ")}.
              Drop the files inside them instead.
            </p>
          )}
          {phase.kind === "review" && (
            <>
              <label className="upload-folder">
                Destination folder on the server
                <input
                  value={destination}
                  placeholder="The server's upload folder"
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                  disabled={!!unavailable}
                  onChange={(event) => setDestination(event.target.value)}
                />
              </label>
              <p className="muted">
                {dest
                  ? `A new private folder is created inside ${dest} for this drop.`
                  : "Each drop gets its own private folder in the server's upload folder."}{" "}
                The quoted paths are then pasted into the terminal without
                pressing Enter.
              </p>
              {dest !== defaultDestination && !unavailable && (
                <label className="upload-remember">
                  <input
                    type="checkbox"
                    checked={remember}
                    onChange={(event) => setRemember(event.target.checked)}
                  />
                  Remember as the default destination in this browser
                </label>
              )}
            </>
          )}
          {phase.kind === "uploading" && (
            <div className="upload-progress" role="status">
              <span>
                Uploading {formatBytes(phase.progress.done)} of{" "}
                {formatBytes(phase.progress.total)}
              </span>
              <progress
                aria-label="Total progress"
                max={Math.max(1, phase.progress.total)}
                value={phase.progress.total ? phase.progress.done : 0}
              />
            </div>
          )}
          {phase.kind === "done" && (
            <>
              <p role="status">{phase.reason} The files are on the server:</p>
              <ul className="upload-files upload-paths" aria-label="Paths">
                {phase.paths.map((path) => (
                  <li key={path}>
                    <span className="mono">{path}</span>
                    <button
                      type="button"
                      className="icon-btn"
                      aria-label={`Copy ${path}`}
                      onClick={() => onCopy(path)}
                    >
                      <Icon name="copy" size="sm" />
                    </button>
                  </li>
                ))}
              </ul>
            </>
          )}
          {phase.kind === "review" && problem && (
            <p className="notice error" role="alert">
              {problem}
            </p>
          )}
          {phase.kind === "failed" && (
            <p className="notice error" role="alert">
              {phase.message}
            </p>
          )}
          <div className="confirm-buttons">
            {phase.kind === "review" && (
              <>
                <button type="button" className="subtle" onClick={onClose}>
                  Cancel
                </button>
                <button type="submit" disabled={!!problem} data-autofocus>
                  <Icon name="upload" size="sm" />
                  Upload
                </button>
              </>
            )}
            {phase.kind === "uploading" && (
              <button
                type="button"
                className="subtle"
                onClick={() => {
                  // Sends upload_cancel; the server removes the batch and
                  // its partial files. Nothing is pasted.
                  abort.current?.abort();
                  onClose();
                }}
              >
                Cancel upload
              </button>
            )}
            {phase.kind === "done" && (
              <>
                <button
                  type="button"
                  className="subtle"
                  title="Copy every path, quoted for the shell"
                  onClick={() => onCopy(pastedPaths(phase.paths))}
                >
                  <Icon name="copy" size="sm" />
                  Copy all
                </button>
                <button type="button" onClick={onClose} data-autofocus>
                  Close
                </button>
              </>
            )}
            {phase.kind === "failed" && (
              <button type="button" onClick={onClose} data-autofocus>
                Close
              </button>
            )}
          </div>
        </form>
      </section>
    </div>
  );
}

/** Settings → Uploads: this browser's default destination folder. */
export function UploadSettings({
  destination,
  onChange,
  unavailable,
}: {
  destination: string;
  onChange: (path: string) => void;
  unavailable: string | null;
}) {
  const [draft, setDraft] = useState(destination);
  useEffect(() => setDraft(destination), [destination]);
  const path = normalizeDestination(draft);
  const problem = destinationProblem(path);
  return (
    <form
      className="upload-settings"
      onSubmit={(event) => {
        event.preventDefault();
        if (!problem) onChange(path);
      }}
    >
      <h3>Default destination</h3>
      <label className="upload-folder">
        Folder on the server for files dropped onto a terminal
        <input
          aria-label="Default upload destination"
          value={draft}
          placeholder="The server's upload folder"
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          onChange={(event) => setDraft(event.target.value)}
        />
      </label>
      {problem && (
        <p className="notice error" role="alert">
          {problem}
        </p>
      )}
      <div className="confirm-buttons">
        <button
          type="button"
          className="subtle"
          disabled={!destination && !draft}
          onClick={() => {
            setDraft("");
            onChange("");
          }}
        >
          Use the server's upload folder
        </button>
        <button type="submit" disabled={!!problem || path === destination}>
          Save
        </button>
      </div>
      <p className="setting-hint">
        Drop files or images onto a terminal (or paste an image, or choose
        Upload files… in its menu) to send them to the host where its shells
        run; their quoted paths are then pasted into the terminal, without
        Enter. Each drop gets its own private folder:{" "}
        {destination
          ? `inside ${destination}, which must be an existing folder owned by your account.`
          : "in the server's upload folder (~/.cache/kelpserver/uploads unless the server sets another one), where completed uploads are kept for the server's retention (7 days by default)."}{" "}
        The upload dialog can choose another folder for one drop. This setting
        is saved in this browser only. Folders cannot be uploaded.
      </p>
      {unavailable && <p className="setting-hint">{unavailable}</p>}
    </form>
  );
}
