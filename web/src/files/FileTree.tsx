// The sidebar's file tree: the home folder's entries listed like the
// terminals, folders first. A folder's row expands it in place (listed
// when expanded, and again each time it is expanded); a file's row opens
// it in the main area, as a terminal's row opens the terminal, and the
// file shown is selected. Open files are marked, unsaved ones with a dot.
// Every row has the file browser's menu (right-click, or its ⋯ button);
// a row drags onto a terminal (typing its quoted path) or onto a folder
// (moving it), and desktop files dropped on a folder upload into it
// (actions.tsx). Hidden entries follow the browser's setting; which
// folders are expanded is kept in this browser.
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
  isInside,
  joinPath,
  nameProblem,
  parentPath,
  sortEntries,
} from "@kelpcore/web/files/paths.ts";
import { Icon } from "../status";
import { startPathDrag } from "./insert";
import {
  FileMenu,
  NameInput,
  kindIcon,
  kindLabel,
  renameError,
  touches,
  useFileActions,
  useFilesChanged,
  useShowHidden,
  type FileActionsProps,
} from "./actions";

const expandedKey = "kelpmux-files-tree";
/** The folders expanded in the tree (this browser only). */
function loadExpanded(): string[] {
  try {
    const saved = JSON.parse(localStorage.getItem(expandedKey) || "[]");
    return Array.isArray(saved)
      ? saved.filter((path) => typeof path === "string")
      : [];
  } catch {
    return [];
  }
}
function saveExpanded(paths: string[]) {
  try {
    localStorage.setItem(expandedKey, JSON.stringify(paths));
  } catch {}
}

type Folder = { listing?: FileListing; error?: string; loading?: boolean };
type Menu = { entry: FileEntry; path: string; x: number; y: number };
type Renaming = { path: string; name: string; error: string };

export type FileTreeProps = {
  actions: FileActionsProps;
  /** The open files: unsaved or not. */
  open: Map<string, { dirty: boolean }>;
  /** The file shown in the main area, if any. */
  current?: string;
  /** Touch: the menu button shows on every row. */
  touch?: boolean;
  /** Opens a folder in the file browser. */
  onBrowse: (dir: string) => void;
};

export function FileTree(props: FileTreeProps) {
  const { client, home } = props.actions;
  const [folders, setFolders] = useState<Record<string, Folder>>({});
  const [expanded, setExpanded] = useState(loadExpanded);
  const [hidden] = useShowHidden();
  const [menu, setMenu] = useState<Menu | null>(null);
  const [renaming, setRenaming] = useState<Renaming | null>(null);
  const actions = useFileActions(props.actions, props.onBrowse);
  const { busy, dropOn, folderDrop, uploading } = actions;
  /** Requests sent and not answered, so a folder is listed once. */
  const pending = useRef(new Map<string, FilesClient>());

  async function load(path: string) {
    if (pending.current.get(path) === client) return;
    pending.current.set(path, client);
    setFolders((all) => ({ ...all, [path]: { ...all[path], loading: true } }));
    let next: Folder;
    try {
      next = { listing: await client.list(path) };
    } catch (failure) {
      next = {
        error:
          failure instanceof FileError
            ? failure.message || describeCode(failure.code)
            : String(failure),
      };
    }
    if (pending.current.get(path) !== client) return;
    pending.current.delete(path);
    setFolders((all) => ({ ...all, [path]: next }));
  }
  // A new connection lists everything again.
  const lastClient = useRef(client);
  useEffect(() => {
    if (lastClient.current === client) return;
    lastClient.current = client;
    pending.current.clear();
    setFolders({});
  }, [client]);
  // A change here or in the browser lists the folders it touched again;
  // folders inside a moved or removed one list again when shown.
  useFilesChanged((paths) => {
    setFolders((all) => {
      const kept = { ...all };
      for (const key of Object.keys(all))
        if (paths.some((path) => path !== key && isInside(key, path)))
          delete kept[key];
      return kept;
    });
    for (const key of Object.keys(folders))
      if (touches(paths, key)) void load(key);
  });

  const isOpen = (path: string) => expanded.includes(path);
  function toggle(path: string) {
    const next = isOpen(path)
      ? expanded.filter((p) => p !== path)
      : [...expanded, path];
    setExpanded(next);
    saveExpanded(next);
    // Expanding lists the folder again, so the tree stays current.
    if (!isOpen(path) && folders[path]) void load(path);
  }

  async function commit() {
    const edit = renaming;
    if (!edit || busy) return;
    const name = edit.name.trim();
    const problem = nameProblem(name);
    if (problem) return setRenaming({ ...edit, error: problem });
    const target = joinPath(parentPath(edit.path), name);
    if (target === edit.path) return setRenaming(null);
    try {
      await actions.rename(edit.path, target);
      setRenaming(null);
      // Expanded folders inside follow the rename.
      const next = expanded.map((p) =>
        isInside(p, edit.path) ? target + p.slice(edit.path.length) : p,
      );
      setExpanded(next);
      saveExpanded(next);
    } catch (failure) {
      setRenaming({ ...edit, name, error: renameError(failure, name) });
    }
  }

  /** The folders shown without a listing yet: listed after rendering. */
  const missing: string[] = [];
  const message = (key: string, depth: number, text: string, error = false) => (
    <p
      key={key}
      className={`file-tree-message ${error ? "error" : ""}`}
      style={{ "--depth": depth } as React.CSSProperties}
      role={error ? "alert" : undefined}
    >
      {text}
    </p>
  );
  function rows(folder: string, depth: number): React.ReactNode[] {
    const state = folders[folder];
    if (!state?.listing) {
      if (!state) missing.push(folder);
      return [
        state?.error
          ? message(`${folder}/:error`, depth, state.error, true)
          : message(`${folder}/:loading`, depth, "Loading…"),
      ];
    }
    const listing = state.listing;
    const entries = sortEntries(listing.entries, { hidden });
    const out: React.ReactNode[] = [];
    for (const entry of entries) {
      const path = joinPath(listing.path, entry.name);
      out.push(row(entry, path, depth));
      if (entry.kind === "dir" && isOpen(path))
        out.push(...rows(path, depth + 1));
    }
    if (!entries.length)
      out.push(
        message(
          `${folder}/:empty`,
          depth,
          listing.entries.length ? "Only hidden entries" : "Empty folder",
        ),
      );
    if (listing.truncated)
      out.push(
        message(
          `${folder}/:truncated`,
          depth,
          `Only the first ${listing.entries.length} entries`,
        ),
      );
    return out;
  }
  function row(entry: FileEntry, path: string, depth: number) {
    const folder = entry.kind === "dir";
    const expandedNow = folder && isOpen(path);
    const doc = props.open.get(path);
    const on = props.current === path;
    const editing = renaming?.path === path ? renaming : null;
    const state = doc?.dirty ? ", unsaved changes" : doc ? ", open" : "";
    return (
      <div
        key={path}
        className={`nav-cell file-row ${dropOn === path ? "drop-target" : ""} ${menu?.path === path ? "selected" : ""}`}
        style={{ "--depth": depth } as React.CSSProperties}
        data-path={path}
        data-kind={entry.kind}
        data-open={doc ? (doc.dirty ? "dirty" : "open") : undefined}
        draggable={!editing}
        onDragStart={(event) => startPathDrag(event, [path])}
        onContextMenu={(event) => {
          event.preventDefault();
          setMenu({ entry, path, x: event.clientX, y: event.clientY });
        }}
        // A file passes drops to its folder, which lights up.
        {...folderDrop(folder ? path : parentPath(path))}
      >
        {editing ? (
          <span className="nav-item term-item file-item editing">
            <span className="file-twisty" aria-hidden="true" />
            <Icon
              name={kindIcon(entry)}
              size="sm"
              className={`files-kind ${entry.kind}`}
            />
            <NameInput
              label={`New name for ${entry.name}`}
              value={editing.name}
              error={editing.error}
              busy={busy}
              folder={folder}
              onChange={(name) => setRenaming({ ...editing, name, error: "" })}
              onCommit={() => void commit()}
              onCancel={() => setRenaming(null)}
            />
          </span>
        ) : (
          <button
            type="button"
            className={`nav-item term-item file-item ${on ? "current" : ""}`}
            aria-current={on ? "page" : undefined}
            aria-expanded={folder ? expandedNow : undefined}
            aria-label={`${entry.name}, ${kindLabel[entry.kind]}${state}`}
            title={
              entry.kind === "symlink" && entry.target
                ? `${path} → ${entry.target}`
                : path
            }
            onClick={() =>
              folder ? toggle(path) : void actions.open(entry, path)
            }
          >
            <span className="file-twisty" aria-hidden="true">
              {folder && (
                <Icon
                  name={expandedNow ? "chevron-down" : "chevron-right"}
                  size="sm"
                />
              )}
            </span>
            <Icon
              name={expandedNow ? "folder-open" : kindIcon(entry)}
              size="sm"
              className={`files-kind ${entry.kind}`}
            />
            <span className="grow term-title">{entry.name}</span>
            {doc && (
              <span
                className={doc.dirty ? "files-dirty" : "file-open-mark"}
                aria-hidden="true"
              />
            )}
          </button>
        )}
        <span className="term-row-actions">
          <button
            type="button"
            className="icon-btn"
            aria-label={`Actions for ${entry.name}`}
            title="Actions"
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
  }

  const root = home;
  const content = root ? rows(root, 0) : [message("home", 0, "Loading…")];
  // The folders shown without a listing are listed after rendering.
  const wanted = missing.join("\n");
  const latestLoad = useRef(load);
  latestLoad.current = load;
  useEffect(() => {
    for (const path of wanted.split("\n"))
      if (path) void latestLoad.current(path);
  }, [wanted, client]);

  return (
    <div
      className={`file-tree ${root && dropOn === root ? "drop-target" : ""}`}
      role="list"
      aria-label="Home folder"
      {...(root ? folderDrop(root) : {})}
    >
      {content}
      {uploading && (
        <div className="files-progress file-tree-progress" role="status">
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
      {menu && (
        <FileMenu
          label={menu.entry.name}
          x={menu.x}
          y={menu.y}
          items={actions.menuItems(menu.entry, menu.path, () =>
            setRenaming({ path: menu.path, name: menu.entry.name, error: "" }),
          )}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}
