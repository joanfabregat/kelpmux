// The file browser: one folder at a time on the host where the shells run,
// through KelpServer's `files` requests (KelpCore files/api.ts). Folders
// first, then files, by name; hidden entries on request; size and
// modification time. A folder or file opens with a click; every entry has a
// menu (right-click, or its ⋯ button) to open it, type its path into the
// last terminal, copy its path, download it (a folder as a .tar.gz
// archive), rename it, move it to the trash or delete it permanently.
// Entries drag onto a terminal row (typing their quoted path) or onto a
// folder (moving them); files dragged from the desktop onto a folder, or
// onto the list, upload into it.
import React, { useEffect, useRef, useState } from "react";
import {
  FileError,
  describeCode,
  type FileEntry,
  type FileListing,
  type FilesClient,
} from "@kelpcore/web/files/api.ts";
import {
  baseName,
  breadcrumbs,
  joinPath,
  moveProblem,
  nameProblem,
  parentPath,
  sortEntries,
} from "@kelpcore/web/files/paths.ts";
import {
  cancelled,
  formatBytes,
  upload,
  uploadProblem,
  type UploadLimits,
  type UploadTransport,
} from "@kelpcore/web/uploads.ts";
import { Icon } from "../status";
import type { IconName } from "../icons";
import { dragHasFiles, droppedFiles } from "../Upload";
import { dragHasPaths, droppedPaths, startPathDrag } from "./insert";
import { startDownload } from "./downloads";
import type { ConfirmAction } from "../Confirm";

const hiddenKey = "kelpmux-files-hidden";
/** Whether hidden entries show (this browser only). */
export function loadHidden(): boolean {
  try {
    return localStorage.getItem(hiddenKey) === "true";
  } catch {
    return false;
  }
}
function saveHidden(hidden: boolean) {
  try {
    localStorage.setItem(hiddenKey, String(hidden));
  } catch {}
}

const dateFormat = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});
const timeFormat = new Intl.DateTimeFormat(undefined, { timeStyle: "short" });
/** A modification time: the time alone today, else date and time. */
export function formatModified(seconds: number): string {
  const date = new Date(seconds * 1000);
  if (!Number.isFinite(date.getTime())) return "";
  return new Date().toDateString() === date.toDateString()
    ? timeFormat.format(date)
    : dateFormat.format(date);
}
const kindIcon = (entry: FileEntry): IconName =>
  entry.kind === "dir" ? "folder" : entry.kind === "symlink" ? "link" : "file";
const kindLabel: Record<string, string> = {
  dir: "Folder",
  file: "File",
  symlink: "Symbolic link",
  other: "Special file",
};

type Editing =
  | { kind: "new-file" | "new-folder"; name: string; error: string }
  | { kind: "rename"; path: string; name: string; error: string };
type Menu = { entry: FileEntry; path: string; x: number; y: number };
type Uploading = {
  folder: string;
  names: string;
  done: number;
  total: number;
  abort: AbortController;
};

export type FilesBrowserProps = {
  client: FilesClient;
  /** The server user's home folder, once known. */
  home?: string;
  /** The folder shown. */
  dir: string;
  /** Shows a folder; `normalised` when it is the folder just listed under
   * its real name (`~`, a symbolic link), which is not a move. */
  onDir: (dir: string, normalised?: boolean) => void;
  onOpenFile: (path: string) => void;
  /** Opens a file in the side panel beside the terminal last shown alone
   * (no item without it: no terminal, or a popup window). */
  onOpenBeside?: (path: string) => void;
  /** The terminal "Insert path" types into, if any. */
  insertTarget?: { number: number; title: string };
  onInsertPath: (path: string) => void;
  onCopyPath: (path: string) => void;
  confirm: (action: ConfirmAction) => void;
  toast: (message: string, options?: { tone?: "error" | "success" }) => void;
  /** An entry moved or was renamed (open editors follow it). */
  onRenamed: (from: string, to: string) => void;
  /** Entries were trashed or deleted (their unchanged editors close). */
  onRemoved: (paths: string[]) => void;
  upload: {
    unavailable: string | null;
    limits: UploadLimits | null;
    transport: UploadTransport;
  };
  /** The phone's back button to the terminal list. */
  onBack?: () => void;
};

export function FilesBrowser(props: FilesBrowserProps) {
  const { client, dir, home } = props;
  const [listing, setListing] = useState<FileListing | null>(null);
  const [error, setError] = useState<FileError | null>(null);
  const [loading, setLoading] = useState(false);
  const [hidden, setHidden] = useState(loadHidden);
  const [editing, setEditing] = useState<Editing | null>(null);
  const [menu, setMenu] = useState<Menu | null>(null);
  const [dropOn, setDropOn] = useState("");
  const [uploading, setUploading] = useState<Uploading | null>(null);
  const [busy, setBusy] = useState(false);
  const generation = useRef(0);
  const picker = useRef<HTMLInputElement>(null);

  async function load(path = dir) {
    const mine = ++generation.current;
    setLoading(true);
    try {
      const result = await client.list(path);
      if (mine !== generation.current) return;
      setListing(result);
      setError(null);
      // `~` and symlinked folders come back normalised.
      if (result.path !== dir) props.onDir(result.path, true);
    } catch (failure) {
      if (mine !== generation.current) return;
      setError(
        failure instanceof FileError
          ? failure
          : new FileError({ code: "io", message: String(failure) }),
      );
      setListing(null);
    } finally {
      if (mine === generation.current) setLoading(false);
    }
  }
  // The latest load and listing, for the effect that follows the folder.
  const latest = useRef({ load, listed: listing?.path });
  latest.current = { load, listed: listing?.path };
  const lastClient = useRef<FilesClient | null>(null);
  useEffect(() => {
    setEditing(null);
    setMenu(null);
    // A new connection lists again; the same one only another folder.
    if (lastClient.current !== client || latest.current.listed !== dir)
      void latest.current.load(dir);
    lastClient.current = client;
  }, [dir, client]);

  const fail = (failure: unknown) =>
    props.toast(
      failure instanceof Error ? failure.message : String(failure),
      { tone: "error" },
    );
  /** Runs a change, then lists the folder again. */
  async function change(run: () => Promise<unknown>) {
    setBusy(true);
    try {
      await run();
    } catch (failure) {
      fail(failure);
    } finally {
      setBusy(false);
      await load();
    }
  }

  async function open(entry: FileEntry, path: string) {
    setMenu(null);
    if (entry.kind === "dir") return props.onDir(path);
    if (entry.kind === "other")
      return props.toast(`${entry.name} is not a regular file or folder.`);
    if (entry.kind === "symlink") {
      // A link opens as what it points to.
      try {
        const target = await client.list(path);
        return props.onDir(target.path);
      } catch (failure) {
        if (!(failure instanceof FileError && failure.code === "not_a_directory"))
          return fail(failure);
      }
    }
    props.onOpenFile(path);
  }

  async function commit() {
    const edit = editing;
    if (!edit || busy) return;
    const name = edit.name.trim();
    const problem = nameProblem(name);
    if (problem) return setEditing({ ...edit, error: problem });
    const target = joinPath(dir, name);
    if (edit.kind === "rename" && target === edit.path)
      return setEditing(null);
    setBusy(true);
    try {
      if (edit.kind === "rename") {
        await client.rename(edit.path, target);
        props.onRenamed(edit.path, target);
      } else if (edit.kind === "new-folder") await client.mkdir(target);
      else await client.write(target, "", { create: true });
      setEditing(null);
      await load();
      if (edit.kind === "new-file") props.onOpenFile(target);
    } catch (failure) {
      const message =
        failure instanceof FileError && failure.code === "exists"
          ? `“${name}” already exists in this folder.`
          : failure instanceof Error
            ? failure.message
            : String(failure);
      setEditing({ ...edit, name, error: message });
    } finally {
      setBusy(false);
    }
  }

  function removalOutcome(
    verb: string,
    outcome: Awaited<ReturnType<FilesClient["trash"]>>,
  ) {
    props.onRemoved(outcome.removed.map((r) => r.path));
    if (outcome.failed.length) {
      const [first] = outcome.failed;
      const advice =
        verb === "Moved to the trash" && first.error.code === "cross_device"
          ? " It is on another file system than the trash; delete it permanently instead."
          : "";
      props.toast(
        `${baseName(first.path)}: ${first.error.message}.${advice}`,
        { tone: "error" },
      );
    } else if (outcome.removed.length)
      props.toast(
        `${verb}: ${outcome.removed.map((r) => baseName(r.path)).join(", ")}`,
      );
  }
  function trash(path: string) {
    setMenu(null);
    void change(async () =>
      removalOutcome("Moved to the trash", await client.trash([path])),
    );
  }
  function remove(entry: FileEntry, path: string) {
    setMenu(null);
    props.confirm({
      title: "Delete permanently",
      text:
        entry.kind === "dir"
          ? `Delete the folder “${entry.name}” and everything in it? This cannot be undone.`
          : `Delete “${entry.name}”? This cannot be undone.`,
      buttons: [
        {
          label: "Delete permanently",
          color: "danger",
          action: () =>
            change(async () =>
              removalOutcome("Deleted", await client.delete([path])),
            ),
        },
      ],
    });
  }
  /** Moves entries into `folder` (a drag inside the browser). */
  function move(paths: string[], folder: string) {
    const movable = paths.filter((path) => !moveProblem(path, folder));
    if (!movable.length) return;
    void change(async () => {
      for (const path of movable) {
        const to = joinPath(folder, baseName(path));
        try {
          await client.rename(path, to);
          props.onRenamed(path, to);
        } catch (failure) {
          throw failure instanceof FileError && failure.code === "exists"
            ? Error(`“${baseName(path)}” already exists in ${baseName(folder)}.`)
            : failure;
        }
      }
      props.toast(
        movable.length === 1
          ? `Moved “${baseName(movable[0])}” to ${baseName(folder)}`
          : `Moved ${movable.length} entries to ${baseName(folder)}`,
      );
    });
  }
  /** Uploads desktop files into `folder`: KelpServer puts each upload in a
   * private batch folder inside the destination; the files then move out
   * of it into `folder` itself and the empty batch folder is removed. */
  async function uploadInto(folder: string, files: File[], folders: string[]) {
    const { unavailable, limits, transport } = props.upload;
    if (folders.length && !files.length)
      return props.toast("Folders cannot be uploaded; drop files instead.", {
        tone: "error",
      });
    const problem =
      unavailable ||
      (limits ? uploadProblem(files, limits) : "No upload limits announced.");
    if (problem || !limits) return props.toast(problem || "", { tone: "error" });
    if (uploading) return props.toast("An upload is already running.");
    const abort = new AbortController();
    const total = files.reduce((sum, file) => sum + file.size, 0);
    const names =
      files.length === 1 ? `“${files[0].name}”` : `${files.length} files`;
    setUploading({ folder, names, done: 0, total, abort });
    let batch = "";
    try {
      const paths = await upload(transport, files, {
        limits,
        dest: folder,
        signal: abort.signal,
        onStarted: (dir) => (batch = dir),
        onProgress: (progress) =>
          setUploading((current) =>
            current && { ...current, done: progress.done },
          ),
      });
      const kept: string[] = [];
      for (const path of paths) {
        try {
          await client.rename(path, joinPath(folder, baseName(path)));
        } catch {
          kept.push(path);
        }
      }
      if (!kept.length && batch) await client.delete([batch]).catch(() => {});
      if (kept.length)
        props.toast(
          `Uploaded ${names}; ${kept.length === 1 ? "one name was" : `${kept.length} names were`} already taken, so ${kept.length === 1 ? "it stays" : "they stay"} in ${batch}.`,
          { tone: "error" },
        );
      else props.toast(`Uploaded ${names} to ${baseName(folder)}`);
    } catch (failure) {
      if (!cancelled(failure)) fail(failure);
    } finally {
      setUploading(null);
      if (folder === dir) await load();
    }
  }

  /** Drop target props for a folder (a row, a breadcrumb or the list). */
  function folderDrop(folder: string) {
    return {
      "data-upload-folder": folder,
      onDragOver: (event: React.DragEvent) => {
        const paths = dragHasPaths(event),
          files = dragHasFiles(event);
        if (!paths && !files) return;
        event.preventDefault();
        event.stopPropagation();
        event.dataTransfer.dropEffect = paths ? "move" : "copy";
        if (dropOn !== folder) setDropOn(folder);
      },
      onDragLeave: (event: React.DragEvent) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node))
          setDropOn((current) => (current === folder ? "" : current));
      },
      onDrop: (event: React.DragEvent) => {
        setDropOn("");
        if (dragHasPaths(event)) {
          event.preventDefault();
          event.stopPropagation();
          move(droppedPaths(event.dataTransfer), folder);
        } else if (dragHasFiles(event)) {
          event.preventDefault();
          event.stopPropagation();
          const { files, folders } = droppedFiles(event.dataTransfer);
          void uploadInto(folder, files, folders);
        }
      },
    };
  }

  const entries = listing ? sortEntries(listing.entries, { hidden }) : [];
  const hiddenCount = listing
    ? listing.entries.length - sortEntries(listing.entries).length
    : 0;
  const crumbs = breadcrumbs(dir, home);
  const menuItems = (m: Menu): [string, () => void, boolean?][] => [
    ["Open", () => void open(m.entry, m.path)],
    ...(props.onOpenBeside && m.entry.kind !== "dir" && m.entry.kind !== "other"
      ? [
          ["Open beside terminal", () => props.onOpenBeside!(m.path)] as [
            string,
            () => void,
          ],
        ]
      : []),
    [
      props.insertTarget
        ? `Insert path into terminal ${props.insertTarget.number}`
        : "Insert path (no terminal)",
      () => props.onInsertPath(m.path),
      !props.insertTarget,
    ],
    ["Copy path", () => props.onCopyPath(m.path)],
    // A folder downloads as a `.tar.gz` archive; a link as what it points
    // to (a file only); special files not at all.
    ...(client.canDownload && m.entry.kind !== "other"
      ? [
          [
            m.entry.kind === "dir" ? "Download as .tar.gz" : "Download",
            () =>
              startDownload(client, m.path, m.entry.kind === "dir", props.toast),
          ] as [string, () => void],
        ]
      : []),
    [
      "Rename…",
      () =>
        setEditing({ kind: "rename", path: m.path, name: m.entry.name, error: "" }),
    ],
    ["Move to trash", () => trash(m.path)],
    ["Delete permanently…", () => remove(m.entry, m.path)],
  ];
  const nameInput = (label: string) =>
    editing && (
      <span className="files-name-edit">
        <input
          autoFocus
          aria-label={label}
          value={editing.name}
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          disabled={busy}
          onChange={(event) =>
            setEditing({ ...editing, name: event.target.value, error: "" })
          }
          onFocus={(event) => {
            // Select the name without its extension, like file managers.
            const dot = event.target.value.lastIndexOf(".");
            event.target.setSelectionRange(
              0,
              dot > 0 && editing.kind !== "new-folder" ? dot : event.target.value.length,
            );
          }}
          onKeyDown={(event) => {
            event.stopPropagation();
            if (event.key === "Enter") {
              event.preventDefault();
              void commit();
            } else if (event.key === "Escape") {
              event.preventDefault();
              setEditing(null);
            }
          }}
        />
        {editing.error && (
          <span className="files-name-error" role="alert">
            {editing.error}
          </span>
        )}
      </span>
    );

  return (
    <section
      className="files-view files-browser"
      aria-label="Files"
      onClick={(event) => event.stopPropagation()}
    >
      <header className="files-head">
        {props.onBack && (
          <button
            type="button"
            className="ghost phone-back"
            aria-label="Terminals"
            onClick={props.onBack}
          >
            <Icon name="chevron-left" />
          </button>
        )}
        <nav className="files-crumbs" aria-label="Folder path">
          {crumbs.map((crumb, index) => (
            <React.Fragment key={crumb.path}>
              {index > 0 && crumbs[index - 1].label !== "/" && (
                <span className="files-crumb-sep" aria-hidden="true">
                  /
                </span>
              )}
              <button
                type="button"
                className={`ghost files-crumb ${dropOn === crumb.path ? "drop-target" : ""}`}
                aria-current={index === crumbs.length - 1 ? "location" : undefined}
                onClick={() => props.onDir(crumb.path)}
                {...folderDrop(crumb.path)}
              >
                {crumb.label === "~" ? <Icon name="house" size="sm" /> : null}
                <span>{crumb.label === "~" ? "Home" : crumb.label}</span>
              </button>
            </React.Fragment>
          ))}
        </nav>
        <div className="files-tools">
          <button
            type="button"
            className="icon-btn"
            title="Parent folder"
            aria-label="Parent folder"
            disabled={dir === "/"}
            onClick={() => props.onDir(parentPath(dir))}
          >
            <Icon name="arrow-up" size="sm" />
          </button>
          <button
            type="button"
            className="icon-btn"
            title={hidden ? "Hide hidden files" : "Show hidden files"}
            aria-label="Show hidden files"
            aria-pressed={hidden}
            onClick={() => {
              saveHidden(!hidden);
              setHidden(!hidden);
            }}
          >
            <Icon name={hidden ? "eye" : "eye-off"} size="sm" />
          </button>
          <button
            type="button"
            className="icon-btn"
            title="Refresh"
            aria-label="Refresh"
            disabled={loading}
            onClick={() => void load()}
          >
            <Icon name="refresh-cw" size="sm" />
          </button>
          <button
            type="button"
            className="icon-btn"
            title="New file"
            aria-label="New file"
            disabled={!listing}
            onClick={() => setEditing({ kind: "new-file", name: "", error: "" })}
          >
            <Icon name="file-plus" size="sm" />
          </button>
          <button
            type="button"
            className="icon-btn"
            title="New folder"
            aria-label="New folder"
            disabled={!listing}
            onClick={() =>
              setEditing({ kind: "new-folder", name: "", error: "" })
            }
          >
            <Icon name="folder-plus" size="sm" />
          </button>
          <button
            type="button"
            className="icon-btn"
            title="Upload files here"
            aria-label="Upload files here"
            disabled={!listing || !!uploading}
            onClick={() => {
              const input = picker.current;
              if (!input) return;
              input.value = "";
              input.click();
            }}
          >
            <Icon name="upload" size="sm" />
          </button>
          <input
            ref={picker}
            type="file"
            multiple
            hidden
            aria-hidden="true"
            tabIndex={-1}
            data-files-picker
            onChange={(event) => {
              const files = Array.from(event.currentTarget.files || []);
              if (files.length) void uploadInto(dir, files, []);
            }}
          />
        </div>
      </header>
      {uploading && (
        <div className="files-progress" role="status">
          <Icon name="loader-circle" size="sm" className="spin" />
          <span className="grow">
            Uploading {uploading.names} to {baseName(uploading.folder)}…{" "}
            {uploading.total
              ? `${Math.floor((uploading.done / uploading.total) * 100)}%`
              : ""}
          </span>
          <button
            type="button"
            className="subtle small"
            onClick={() => uploading.abort.abort()}
          >
            Cancel
          </button>
        </div>
      )}
      <div
        className={`files-list ${dropOn === dir ? "drop-target" : ""}`}
        role="grid"
        aria-label={`Contents of ${dir}`}
        aria-busy={loading}
        {...folderDrop(dir)}
      >
        <div className="files-row files-columns" role="row">
          <span role="columnheader" className="files-name-col">
            Name
          </span>
          <span role="columnheader" className="files-size">
            Size
          </span>
          <span role="columnheader" className="files-modified">
            Modified
          </span>
          <span className="files-actions" aria-hidden="true" />
        </div>
        {editing && editing.kind !== "rename" && (
          <div className="files-row editing" role="row">
            <span className="files-name-col" role="gridcell">
              <Icon
                name={editing.kind === "new-folder" ? "folder" : "file"}
                size="sm"
              />
              {nameInput(
                editing.kind === "new-folder" ? "New folder name" : "New file name",
              )}
            </span>
          </div>
        )}
        {error && (
          <div className="files-message error" role="alert">
            <p>
              {error.code === "not_found"
                ? `${dir} does not exist.`
                : error.code === "not_a_directory"
                  ? `${dir} is not a folder.`
                  : error.message || describeCode(error.code)}
            </p>
            {home && dir !== home && (
              <button type="button" className="subtle small" onClick={() => props.onDir(home)}>
                Go to the home folder
              </button>
            )}
          </div>
        )}
        {!error && listing && !entries.length && !editing && (
          <p className="files-message">
            {hiddenCount
              ? `This folder holds only hidden entries (${hiddenCount}).`
              : "This folder is empty."}
          </p>
        )}
        {entries.map((entry) => {
          const path = joinPath(listing!.path, entry.name);
          const renaming = editing?.kind === "rename" && editing.path === path;
          const folder = entry.kind === "dir";
          return (
            <div
              key={entry.name}
              className={`files-row ${dropOn === path ? "drop-target" : ""} ${menu?.path === path ? "selected" : ""}`}
              role="row"
              data-path={path}
              data-kind={entry.kind}
              draggable={!renaming}
              onDragStart={(event) => startPathDrag(event, [path])}
              onContextMenu={(event) => {
                event.preventDefault();
                setMenu({ entry, path, x: event.clientX, y: event.clientY });
              }}
              {...(folder ? folderDrop(path) : {})}
            >
              <span className="files-name-col" role="gridcell">
                <Icon
                  name={kindIcon(entry)}
                  size="sm"
                  className={`files-kind ${entry.kind}`}
                />
                {renaming ? (
                  nameInput(`New name for ${entry.name}`)
                ) : (
                  <button
                    type="button"
                    className="files-open"
                    title={
                      entry.kind === "symlink" && entry.target
                        ? `${entry.name} → ${entry.target}`
                        : entry.name
                    }
                    aria-label={`${entry.name}, ${kindLabel[entry.kind]}`}
                    onClick={() => void open(entry, path)}
                  >
                    <span className="files-name">{entry.name}</span>
                    {entry.kind === "symlink" && entry.target && (
                      <small className="files-target">→ {entry.target}</small>
                    )}
                    <small className="files-meta">
                      {[
                        entry.kind === "file" ? formatBytes(entry.size) : "",
                        formatModified(entry.modified),
                      ]
                        .filter(Boolean)
                        .join(" · ")}
                    </small>
                  </button>
                )}
              </span>
              <span className="files-size" role="gridcell">
                {entry.kind === "file" ? formatBytes(entry.size) : ""}
              </span>
              <span className="files-modified" role="gridcell">
                {formatModified(entry.modified)}
              </span>
              <span className="files-actions" role="gridcell">
                <button
                  type="button"
                  className="icon-btn"
                  aria-label={`Actions for ${entry.name}`}
                  onClick={(event) => {
                    const rect = event.currentTarget.getBoundingClientRect();
                    setMenu({
                      entry,
                      path,
                      x: Math.max(8, rect.right - 240),
                      y: rect.bottom,
                    });
                  }}
                >
                  <Icon name="ellipsis" size="sm" />
                </button>
              </span>
            </div>
          );
        })}
        {listing?.truncated && (
          <p className="files-message">
            Showing the first {listing.entries.length} entries of this folder.
          </p>
        )}
      </div>
      {menu && (
        <FileMenu
          label={menu.entry.name}
          x={menu.x}
          y={menu.y}
          items={menuItems(menu)}
          onClose={() => setMenu(null)}
        />
      )}
    </section>
  );
}

/** An entry's menu, like a terminal's (Panels.tsx PaneMenu). */
function FileMenu({
  label,
  x,
  y,
  items,
  onClose,
}: {
  label: string;
  x: number;
  y: number;
  items: [string, () => void, boolean?][];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    ref.current?.querySelector<HTMLButtonElement>("button")?.focus();
    const outside = (event: PointerEvent) => {
      if (!ref.current?.contains(event.target as Node)) close.current();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        close.current();
      }
    };
    window.addEventListener("pointerdown", outside, true);
    window.addEventListener("keydown", escape, true);
    return () => {
      window.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("keydown", escape, true);
    };
  }, []);
  return (
    <div
      ref={ref}
      className="popover menu pane-menu files-menu"
      role="menu"
      aria-label={label}
      style={{
        left: Math.max(8, Math.min(x, innerWidth - 248)),
        top: Math.max(8, Math.min(y, innerHeight - 36 * items.length - 16)),
      }}
    >
      {items.map(([text, action, disabled]) => (
        <button
          type="button"
          role="menuitem"
          key={text}
          className="menu-item"
          disabled={disabled}
          onClick={() => {
            onClose();
            action();
          }}
        >
          <span className="grow">{text}</span>
        </button>
      ))}
    </div>
  );
}
