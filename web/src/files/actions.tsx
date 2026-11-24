// What the file browser and the sidebar's file tree share: an entry's menu
// (open, open beside the terminal, insert or copy its path, download,
// rename, trash, delete), moving entries dragged onto a folder, uploading
// desktop files dropped on one, the inline name field, the show-hidden
// setting, and the notice that entries changed, so each view lists the
// folders a change touched again.
import React, {
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import {
  FileError,
  type FileEntry,
  type FilesClient,
} from "@kelpcore/web/files/api.ts";
import {
  baseName,
  joinPath,
  moveProblem,
  parentPath,
} from "@kelpcore/web/files/paths.ts";
import {
  cancelled,
  upload,
  uploadProblem,
  type UploadLimits,
  type UploadTransport,
} from "@kelpcore/web/uploads.ts";
import type { IconName } from "../icons";
import { dragHasFiles, droppedFiles } from "../Upload";
import { dragHasPaths, droppedPaths } from "./insert";
import { startDownload } from "./downloads";
import {
  entryAbilities,
  errorMessage,
  leadsOutside,
  outsideRootMessage,
  withinRoot,
} from "./root";
import type { ConfirmAction } from "../Confirm";

/** What both views need from the application: the connection's files
 * client and how a file opens, a path reaches a terminal or the
 * clipboard, a confirmation or a toast shows, and how open editors follow
 * renames and removals. */
export type FileActionsProps = {
  client: FilesClient;
  /** The server user's home folder, once known. */
  home?: string;
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
};

export const kindIcon = (entry: FileEntry): IconName =>
  entry.kind === "dir" ? "folder" : entry.kind === "symlink" ? "link" : "file";
export const kindLabel: Record<string, string> = {
  dir: "Folder",
  file: "File",
  symlink: "Symbolic link",
  other: "Special file",
};

// ───── Show hidden entries (this browser only, shared by both views) ─────
const hiddenKey = "kelpmux-files-hidden";
/** Whether hidden entries show (this browser only). */
export function loadHidden(): boolean {
  try {
    return localStorage.getItem(hiddenKey) === "true";
  } catch {
    return false;
  }
}
let hiddenShown = loadHidden();
const hiddenListeners = new Set<() => void>();
const subscribeHidden = (listener: () => void) => {
  hiddenListeners.add(listener);
  return () => hiddenListeners.delete(listener);
};
/** The show-hidden setting, the same in the browser and the tree. */
export function useShowHidden(): [boolean, (shown: boolean) => void] {
  const shown = useSyncExternalStore(subscribeHidden, () => hiddenShown);
  return [
    shown,
    (next: boolean) => {
      hiddenShown = next;
      try {
        localStorage.setItem(hiddenKey, String(next));
      } catch {}
      for (const listener of hiddenListeners) listener();
    },
  ];
}

// ───── Changes: which folders to list again ─────
const changeListeners = new Set<(paths: string[]) => void>();
/** Entries at `paths` were created, changed, moved or removed. */
export function filesChanged(paths: string[]) {
  for (const listener of changeListeners) listener(paths);
}
/** Whether a change of `paths` alters the listing of `folder`. */
export const touches = (paths: string[], folder: string) =>
  paths.some((path) => path === folder || parentPath(path) === folder);
/** Calls `listener` after each change, from either view. */
export function useFilesChanged(listener: (paths: string[]) => void) {
  const latest = useRef(listener);
  latest.current = listener;
  useEffect(() => {
    const call = (paths: string[]) => latest.current(paths);
    changeListeners.add(call);
    return () => {
      changeListeners.delete(call);
    };
  }, []);
}

export type Uploading = {
  folder: string;
  names: string;
  done: number;
  total: number;
  abort: AbortController;
};
export type MenuItem = [string, () => void, boolean?];

/** A rename's error, as the name field shows it. */
export function renameError(failure: unknown, name: string): string {
  return failure instanceof FileError && failure.code === "exists"
    ? `“${name}” already exists in this folder.`
    : errorMessage(failure);
}

/** The title of an entry's row: its name or path, and where a link
 * points (marked when that is outside the root). */
export function entryTitle(
  entry: FileEntry,
  label: string,
  path: string,
  root: string | undefined,
): string {
  if (entry.kind !== "symlink" || !entry.target) return label;
  const outside = leadsOutside(entry, path, root)
    ? ` (${outsideRootMessage.toLowerCase()})`
    : "";
  return `${label} → ${entry.target}${outside}`;
}

/** The actions on entries, for one view. `onFolder` opens a folder (the
 * browser shows it; the tree opens it in the browser). */
export function useFileActions(
  props: FileActionsProps,
  onFolder: (path: string) => void,
) {
  const { client } = props;
  /** The file root (none from older servers): nothing outside it is
   * opened, moved to or uploaded to. */
  const root = client.limits.root;
  const [busy, setBusy] = useState(false);
  const [uploading, setUploading] = useState<Uploading | null>(null);
  const [dropOn, setDropOn] = useState("");

  const fail = (failure: unknown) =>
    props.toast(errorMessage(failure), { tone: "error" });
  /** Runs a change, then has every view list what it touched again. */
  async function change(run: () => Promise<unknown>, touched: string[]) {
    setBusy(true);
    try {
      await run();
    } catch (failure) {
      fail(failure);
    } finally {
      setBusy(false);
      filesChanged(touched);
    }
  }

  async function open(entry: FileEntry, path: string) {
    if (entry.kind === "dir") return onFolder(path);
    if (entry.kind === "other")
      return props.toast(`${entry.name} is not a regular file or folder.`);
    if (leadsOutside(entry, path, root))
      return props.toast(`“${entry.name}”: ${outsideRootMessage}`, {
        tone: "error",
      });
    if (entry.kind === "symlink") {
      // A link opens as what it points to.
      try {
        const target = await client.list(path);
        return onFolder(target.path);
      } catch (failure) {
        if (!(
          failure instanceof FileError && failure.code === "not_a_directory"
        ))
          return fail(failure);
      }
    }
    props.onOpenFile(path);
  }

  /** Renames `from` to `to`; rejects with the server's error. */
  async function rename(from: string, to: string) {
    setBusy(true);
    try {
      await client.rename(from, to);
      props.onRenamed(from, to);
      filesChanged([from, to]);
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
        `${baseName(first.path)}: ${errorMessage(first.error)}.${advice}`,
        {
          tone: "error",
        },
      );
    } else if (outcome.removed.length)
      props.toast(
        `${verb}: ${outcome.removed.map((r) => baseName(r.path)).join(", ")}`,
      );
  }
  function trash(path: string) {
    void change(
      async () =>
        removalOutcome("Moved to the trash", await client.trash([path])),
      [path],
    );
  }
  function remove(entry: FileEntry, path: string) {
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
            change(
              async () =>
                removalOutcome("Deleted", await client.delete([path])),
              [path],
            ),
        },
      ],
    });
  }
  /** Moves entries into `folder` (a drag onto a folder). */
  function move(paths: string[], folder: string) {
    // Both ends inside the root, never the root itself.
    if (!withinRoot(folder, root)) return fail(outsideRootMessage);
    const movable = paths.filter(
      (path) =>
        !moveProblem(path, folder) && withinRoot(path, root) && path !== root,
    );
    if (!movable.length) return;
    const targets = movable.map((path) => joinPath(folder, baseName(path)));
    void change(async () => {
      for (const path of movable) {
        const to = joinPath(folder, baseName(path));
        try {
          await client.rename(path, to);
          props.onRenamed(path, to);
        } catch (failure) {
          throw failure instanceof FileError && failure.code === "exists"
            ? Error(
                `“${baseName(path)}” already exists in ${baseName(folder)}.`,
              )
            : failure;
        }
      }
      props.toast(
        movable.length === 1
          ? `Moved “${baseName(movable[0])}” to ${baseName(folder)}`
          : `Moved ${movable.length} entries to ${baseName(folder)}`,
      );
    }, [...movable, ...targets]);
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
    if (problem || !limits)
      return props.toast(problem || "", { tone: "error" });
    if (!withinRoot(folder, root)) return fail(outsideRootMessage);
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
          setUploading(
            (current) => current && { ...current, done: progress.done },
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
      filesChanged([folder]);
    }
  }

  /** Drop target props for a folder: entries dragged from either view move
   * into it, desktop files upload into it. */
  function folderDrop(folder: string) {
    // Nothing moves or uploads to a folder outside the root.
    if (!withinRoot(folder, root)) return {};
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

  /** An entry's menu; `onRename` starts editing its name in place. The
   * root itself has no download or change; a link pointing outside the
   * root nothing that goes through it (entryAbilities). */
  const menuItems = (
    entry: FileEntry,
    path: string,
    onRename: () => void,
  ): MenuItem[] => {
    const can = entryAbilities(entry, path, root);
    return [
      ...(can.open ? [["Open", () => void open(entry, path)] as MenuItem] : []),
      ...(can.open &&
      props.onOpenBeside &&
      entry.kind !== "dir" &&
      entry.kind !== "other"
        ? [
            [
              "Open beside terminal",
              () => props.onOpenBeside!(path),
            ] as MenuItem,
          ]
        : []),
      [
        props.insertTarget
          ? `Insert path into terminal ${props.insertTarget.number}`
          : "Insert path (no terminal)",
        () => props.onInsertPath(path),
        !props.insertTarget,
      ],
      ["Copy path", () => props.onCopyPath(path)],
      // A folder downloads as a `.tar.gz` archive; a link as what it points
      // to (a file only); special files not at all.
      ...(client.canDownload && can.download
        ? [
            [
              entry.kind === "dir" ? "Download as .tar.gz" : "Download",
              () =>
                startDownload(client, path, entry.kind === "dir", props.toast),
            ] as MenuItem,
          ]
        : []),
      ...(can.change
        ? ([
            ["Rename…", onRename],
            ["Move to trash", () => trash(path)],
            ["Delete permanently…", () => remove(entry, path)],
          ] as MenuItem[])
        : []),
    ];
  };

  return {
    root,
    busy,
    setBusy,
    uploading,
    dropOn,
    open,
    rename,
    uploadInto,
    folderDrop,
    menuItems,
  };
}

/** The name field of a new or renamed entry: Enter commits, Escape
 * cancels; focused, it selects the name without its extension. */
export function NameInput({
  label,
  value,
  error,
  busy,
  folder,
  onChange,
  onCommit,
  onCancel,
}: {
  label: string;
  value: string;
  error: string;
  busy: boolean;
  /** A new folder: the whole name is selected. */
  folder?: boolean;
  onChange: (name: string) => void;
  onCommit: () => void;
  onCancel: () => void;
}) {
  return (
    <span className="files-name-edit">
      <input
        autoFocus
        aria-label={label}
        value={value}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        disabled={busy}
        onChange={(event) => onChange(event.target.value)}
        onFocus={(event) => {
          // Select the name without its extension, like file managers.
          const dot = event.target.value.lastIndexOf(".");
          event.target.setSelectionRange(
            0,
            dot > 0 && !folder ? dot : event.target.value.length,
          );
        }}
        onKeyDown={(event) => {
          event.stopPropagation();
          if (event.key === "Enter") {
            event.preventDefault();
            onCommit();
          } else if (event.key === "Escape") {
            event.preventDefault();
            onCancel();
          }
        }}
      />
      {error && (
        <span className="files-name-error" role="alert">
          {error}
        </span>
      )}
    </span>
  );
}

/** An entry's menu, like a terminal's (Panels.tsx PaneMenu). */
export function FileMenu({
  label,
  x,
  y,
  items,
  onClose,
}: {
  label: string;
  x: number;
  y: number;
  items: MenuItem[];
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
