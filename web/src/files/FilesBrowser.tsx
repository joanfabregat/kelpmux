// The file browser: one folder at a time on the host where the shells run,
// through KelpServer's `files` requests (KelpCore files/api.ts). Folders
// first, then files, by name; hidden entries on request; size and
// modification time. A folder or file opens with a click; every entry has a
// menu (right-click, or its ⋯ button) to open it, type its path into the
// last terminal, copy its path, download it (a folder as a .tar.gz
// archive), rename it, move it to the trash or delete it permanently.
// Entries drag onto a terminal row (typing their quoted path) or onto a
// folder (moving them); files dragged from the desktop onto a folder, or
// onto the list, upload into it. The menu, moves, uploads and the hidden
// setting are shared with the sidebar's file tree (actions.tsx). Browsing
// starts at the server's file root and never goes above it; a symbolic
// link pointing outside it is shown but does not open (root.ts).
import React, { useEffect, useRef, useState } from "react";
import {
  FileError,
  type FileEntry,
  type FileListing,
  type FilesClient,
} from "@kelpcore/web/files/api.ts";
import {
  baseName,
  joinPath,
  nameProblem,
  parentPath,
  sortEntries,
} from "@kelpcore/web/files/paths.ts";
import { formatBytes } from "@kelpcore/web/uploads.ts";
import { Icon } from "../status";
import { startPathDrag } from "./insert";
import {
  canGoUp,
  errorMessage,
  leadsOutside,
  outsideRootMessage,
  rootCrumbs,
  withinRoot,
} from "./root";
import {
  FileMenu,
  NameInput,
  entryTitle,
  filesChanged,
  kindIcon,
  kindLabel,
  renameError,
  touches,
  useFileActions,
  useFilesChanged,
  useShowHidden,
  type FileActionsProps,
} from "./actions";

export { loadHidden } from "./actions";

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

type Editing =
  | { kind: "new-file" | "new-folder"; name: string; error: string }
  | { kind: "rename"; path: string; name: string; error: string };
type Menu = { entry: FileEntry; path: string; x: number; y: number };

export type FilesBrowserProps = FileActionsProps & {
  /** The folder shown. */
  dir: string;
  /** Shows a folder; `normalised` when it is the folder just listed under
   * its real name (`~`, a symbolic link), which is not a move. */
  onDir: (dir: string, normalised?: boolean) => void;
  /** The phone's back button to the terminal list. */
  onBack?: () => void;
  /** The Files view's tab strip, under the header. */
  tabs?: React.ReactNode;
};

export function FilesBrowser(props: FilesBrowserProps) {
  const { client, dir, home } = props;
  const [listing, setListing] = useState<FileListing | null>(null);
  const [error, setError] = useState<FileError | null>(null);
  const [loading, setLoading] = useState(false);
  const [hidden, setHidden] = useShowHidden();
  const [editing, setEditing] = useState<Editing | null>(null);
  const [menu, setMenu] = useState<Menu | null>(null);
  const generation = useRef(0);
  const picker = useRef<HTMLInputElement>(null);
  const actions = useFileActions(props, props.onDir);
  const { root, busy, uploading, dropOn, folderDrop } = actions;
  /** The folder to show instead of `dir`: the root for `~` when the root
   * is not the home folder, and for a folder outside the root. */
  const start =
    root &&
    (dir === "~"
      ? !home || !withinRoot(home, root)
      : !withinRoot(dir, root, home))
      ? root
      : dir;

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
  // The latest load, listing and props, for the effect that follows the
  // folder.
  const latest = useRef({ load, listed: listing?.path, props });
  latest.current = { load, listed: listing?.path, props };
  const lastClient = useRef<FilesClient | null>(null);
  useEffect(() => {
    setEditing(null);
    setMenu(null);
    // Outside the root: the root, in place of this folder (not a move).
    if (start !== dir) {
      const { toast, onDir } = latest.current.props;
      if (dir !== "~") toast(outsideRootMessage, { tone: "error" });
      return onDir(start, true);
    }
    // A new connection lists again; the same one only another folder.
    if (lastClient.current !== client || latest.current.listed !== dir)
      void latest.current.load(dir);
    lastClient.current = client;
  }, [dir, start, client]);
  // A change here or in the sidebar's tree lists this folder again.
  useFilesChanged((paths) => {
    if (touches(paths, listing?.path || dir)) void load();
  });

  async function open(entry: FileEntry, path: string) {
    setMenu(null);
    await actions.open(entry, path);
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
    try {
      if (edit.kind === "rename") await actions.rename(edit.path, target);
      else {
        actions.setBusy(true);
        try {
          if (edit.kind === "new-folder") await client.mkdir(target);
          else await client.write(target, "", { create: true });
        } finally {
          actions.setBusy(false);
        }
        filesChanged([target]);
      }
      setEditing(null);
      if (edit.kind === "new-file") props.onOpenFile(target);
    } catch (failure) {
      setEditing({ ...edit, name, error: renameError(failure, name) });
    }
  }

  const entries = listing ? sortEntries(listing.entries, { hidden }) : [];
  const hiddenCount = listing
    ? listing.entries.length - sortEntries(listing.entries).length
    : 0;
  const crumbs = rootCrumbs(dir, root, home);
  /** Where the error's button goes back to: the root, else the home
   * folder. */
  const top = root || home;
  const menuItems = (m: Menu) =>
    actions.menuItems(m.entry, m.path, () =>
      setEditing({
        kind: "rename",
        path: m.path,
        name: m.entry.name,
        error: "",
      }),
    );
  const nameInput = (label: string) =>
    editing && (
      <NameInput
        label={label}
        value={editing.name}
        error={editing.error}
        busy={busy}
        folder={editing.kind === "new-folder"}
        onChange={(name) => setEditing({ ...editing, name, error: "" })}
        onCommit={() => void commit()}
        onCancel={() => setEditing(null)}
      />
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
                <span title={index === 0 ? crumb.path : undefined}>
                  {crumb.label === "~" ? "Home" : crumb.label}
                </span>
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
            disabled={!canGoUp(dir, root)}
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
            onClick={() => setHidden(!hidden)}
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
              if (files.length) void actions.uploadInto(dir, files, []);
            }}
          />
        </div>
      </header>
      {props.tabs}
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
                  : errorMessage(error)}
            </p>
            {top && dir !== top && (
              <button type="button" className="subtle small" onClick={() => props.onDir(top)}>
                {top === home ? "Go to the home folder" : `Go to ${top}`}
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
          // A link pointing outside the root: shown, never opened.
          const outside = leadsOutside(entry, path, root);
          return (
            <div
              key={entry.name}
              className={`files-row ${dropOn === path ? "drop-target" : ""} ${menu?.path === path ? "selected" : ""} ${outside ? "outside" : ""}`}
              role="row"
              data-path={path}
              data-kind={entry.kind}
              data-outside={outside || undefined}
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
                    title={entryTitle(entry, entry.name, path, root)}
                    aria-label={`${entry.name}, ${kindLabel[entry.kind]}${outside ? ", outside the browsable folders" : ""}`}
                    aria-disabled={outside || undefined}
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
