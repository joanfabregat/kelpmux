// The file browser, a column view like the Finder's, on the host where the
// shells run, through KelpServer's `files` requests (KelpCore files/api.ts).
// The first column is the file root; selecting a folder shows its listing
// in a new column to its right (replacing the deeper ones), selecting a
// file shows an info column at the far right (kind, size, date, path, the
// first lines of a small text file, Open and Download). The deepest folder
// shown is the browser's folder (`dir`, in the URL). Each entry shows its
// modification date and size under its name (columns.ts). Folders first,
// then files, by name; hidden entries on request. The keyboard moves as
// in the Finder: ↑/↓ within a column, → into the selected folder, ← back
// to the parent column, Enter opens, typing selects by name. Phones show
// one column at a time, where a tap opens.
//
// Every entry has a menu (right-click, its ⋯ button, or the context menu
// key) to open it, type its path into the last terminal, copy its path,
// download it (a folder as a .tar.gz archive), rename it, move it to the
// trash or delete it permanently. Entries drag onto a terminal row (typing
// their quoted path) or onto a folder (moving them); files dragged from
// the desktop onto a folder row, or onto a column, upload into that
// folder. The menu, moves, uploads and the hidden setting live in
// actions.tsx. Browsing starts at the server's file root and never goes
// above it; a symbolic link pointing outside it is shown but does not open
// (root.ts).
import React, { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  FileError,
  type FileEntry,
  type FileListing,
  type FilesClient,
} from "@kelpcore/web/files/api.ts";
import {
  baseName,
  displayPath,
  isInside,
  joinPath,
  nameProblem,
  parentPath,
  sortEntries,
} from "@kelpcore/web/files/paths.ts";
import { Icon } from "../status";
import { startPathDrag } from "./insert";
import {
  canGoUp,
  entryAbilities,
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
import {
  columnFolders,
  entryMeta,
  formatModifiedFull,
  formatSize,
  matchPrefix,
} from "./columns";
import { startDownload } from "./downloads";
import {
  projectAt,
  projectColor,
  projectFolder,
  type Project,
} from "../projects";

export { loadHidden } from "./actions";

/** A column's listing: none yet, or the server's error. */
type Column = {
  listing: FileListing | null;
  error: FileError | null;
  loading: boolean;
};
type Editing =
  | {
      kind: "new-file" | "new-folder";
      /** The folder it is created in (the focused column's). */
      folder: string;
      name: string;
      error: string;
    }
  | { kind: "rename"; path: string; name: string; error: string };
type Menu = { entry: FileEntry; path: string; x: number; y: number };

/** A column's width in pixels: the default, and the bounds of a drag. */
const COLUMN_WIDTH = 260;
const MIN_COLUMN = 160;
const MAX_COLUMN = 640;
/** Files up to this size show their first lines in the info column. */
const PREVIEW_BYTES = 32 * 1024;
const PREVIEW_LINES = 40;

const asFileError = (failure: unknown) =>
  failure instanceof FileError
    ? failure
    : new FileError({ code: "io", message: String(failure) });

export type FilesBrowserProps = FileActionsProps & {
  /** The deepest folder shown (the browser's folder). */
  dir: string;
  /** Shows a folder as the deepest column. `replace` replaces the history
   * entry instead of adding one: the folder just listed under its real
   * name (`~`, a symbolic link), or reached by the keyboard, by selecting
   * a file in another column, or by a rename or removal. */
  onDir: (dir: string, replace?: boolean) => void;
  /** A phone: one column at a time, a tap opens. */
  phone?: boolean;
  /** The phone's back button to the terminal list. */
  onBack?: () => void;
  /** The Files view's tab strip, above the header. */
  tabs?: React.ReactNode;
  /** Every project, Scratch included: their folders show in their colour. */
  projects?: Project[];
  /** A project's files: the columns start at its folder. */
  scope?: Project;
  /** Shows a project's files. */
  onOpenProject?: (project: Project) => void;
  /** Leaves a project's files for every file. */
  onAllFiles?: () => void;
};

export function FilesBrowser(props: FilesBrowserProps) {
  const { client, dir, home, phone, projects, scope } = props;
  const [columns, setColumns] = useState<Record<string, Column>>({});
  const [hidden, setHidden] = useShowHidden();
  const [editing, setEditing] = useState<Editing | null>(null);
  const [menu, setMenu] = useState<Menu | null>(null);
  /** The entry selected in the deepest column that is not a folder (a
   * file, a link to one, a special file): the info column shows it. */
  const [selected, setSelected] = useState("");
  /** The folder of the column the keyboard is in. */
  const [active, setActive] = useState("");
  const [widths, setWidths] = useState<Record<number, number>>({});
  /** Re-renders for a focus move alone (phones). */
  const [, setTick] = useState(0);
  const generations = useRef(new Map<string, number>());
  const requested = useRef(new Set<string>());
  const picker = useRef<HTMLInputElement>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const crumbBar = useRef<HTMLElement>(null);
  /** A row to focus once it is shown (keyboard moves). */
  const focusWanted = useRef("");
  /** A column the keyboard entered before its listing arrived. */
  const enterWanted = useRef("");
  /** Rows already scrolled into view in their column. */
  const revealed = useRef(new Set<string>());
  const typed = useRef({ text: "", at: 0 });
  /** Selections in flight (a symbolic link is listed first): only the
   * latest applies. */
  const choosing = useRef(0);
  const latest = useRef({ dir, props });
  latest.current = { dir, props };

  /** Open views follow a renamed or moved folder shown as a column. */
  function follow(from: string, to: string) {
    const current = latest.current.dir;
    if (isInside(current, from))
      latest.current.props.onDir(to + current.slice(from.length), true);
    setSelected((path) =>
      path && isInside(path, from) ? to + path.slice(from.length) : path,
    );
  }
  /** A removed folder shown as a column: its parent becomes the deepest. */
  function forget(paths: string[]) {
    const current = latest.current.dir;
    const gone = paths.find((path) => isInside(current, path));
    if (gone) latest.current.props.onDir(parentPath(gone), true);
    setSelected((path) =>
      paths.some((removed) => path && isInside(path, removed)) ? "" : path,
    );
  }
  const actions = useFileActions(
    {
      ...props,
      onRenamed: (from, to) => {
        props.onRenamed(from, to);
        follow(from, to);
      },
      onRemoved: (paths) => {
        props.onRemoved(paths);
        forget(paths);
      },
    },
    (path) => props.onDir(path),
  );
  const { root, busy, uploading, dropOn, folderDrop } = actions;
  /** The folder to show instead of `dir`: the root for `~` when the root
   * is not the home folder, and for a folder outside the root. */
  const start = scope
    ? // A project's view stays inside the project's folder.
      dir.startsWith("/") && isInside(dir, scope.path)
      ? dir
      : scope.path
    : root &&
        (dir === "~"
          ? !home || !withinRoot(home, root)
          : !withinRoot(dir, root, home))
      ? root
      : dir;
  /** Where the columns start: the project's folder in a project's view,
   * else the root. */
  const base = scope?.path || root;
  const folders = columnFolders(dir, base, home);
  const shown = phone ? folders.slice(-1) : folders;
  const shownKey = shown.join("\n");

  function store(folder: string, column: Column, also?: string) {
    setColumns((all) => ({
      ...all,
      [folder]: column,
      ...(also && also !== folder ? { [also]: column } : {}),
    }));
  }
  async function load(folder: string) {
    const mine = (generations.current.get(folder) || 0) + 1;
    generations.current.set(folder, mine);
    requested.current.add(folder);
    setColumns((all) => ({
      ...all,
      [folder]: {
        listing: all[folder]?.listing || null,
        error: all[folder]?.error || null,
        loading: true,
      },
    }));
    try {
      const result = await client.list(folder);
      if (generations.current.get(folder) !== mine) return;
      store(
        folder,
        { listing: result, error: null, loading: false },
        result.path,
      );
      // `~` and symlinked folders come back normalised.
      if (result.path !== folder) {
        requested.current.add(result.path);
        if (folder === latest.current.dir)
          latest.current.props.onDir(result.path, true);
      }
    } catch (failure) {
      if (generations.current.get(folder) !== mine) return;
      store(folder, {
        listing: null,
        error: asFileError(failure),
        loading: false,
      });
    }
  }
  const loader = useRef(load);
  loader.current = load;
  const lastClient = useRef<FilesClient | null>(null);
  useEffect(() => {
    setEditing(null);
    setMenu(null);
    // Outside the root: the root, in place of this folder (not a move).
    if (start !== dir) {
      const { toast, onDir, scope: scoped } = latest.current.props;
      if (dir !== "~" && !scoped) toast(outsideRootMessage, { tone: "error" });
      return onDir(start, true);
    }
    // A new connection lists every column again; the same one only the
    // columns it has not listed.
    if (lastClient.current !== client) requested.current.clear();
    lastClient.current = client;
    const wanted = shownKey.split("\n");
    for (const folder of [...requested.current])
      if (!wanted.includes(folder)) {
        requested.current.delete(folder);
        generations.current.set(
          folder,
          (generations.current.get(folder) || 0) + 1,
        );
      }
    setColumns((all) =>
      Object.fromEntries(
        Object.entries(all).filter(([folder]) => wanted.includes(folder)),
      ),
    );
    for (const folder of wanted)
      if (!requested.current.has(folder)) void loader.current(folder);
  }, [shownKey, dir, start, client]);
  // A change made through the entry actions lists the folders it touched
  // again.
  useFilesChanged((paths) => {
    for (const folder of shown) if (touches(paths, folder)) void load(folder);
  });

  /** A column's entries as shown, and the path its rows start from. */
  function entriesOf(folder: string) {
    const listing = columns[folder]?.listing;
    return {
      base: listing?.path || folder,
      entries: listing ? sortEntries(listing.entries, { hidden }) : [],
    };
  }
  /** The entry highlighted in the column at `index`: the folder shown
   * next to it, or the selection in the deepest column. */
  function highlightOf(index: number): string {
    const next = folders[index + 1];
    if (next) return next;
    return selected && parentPath(selected) === folders[index] ? selected : "";
  }
  /** The column the keyboard is in: the focused one, else the deepest. */
  const activeIndex = (() => {
    const index = shown.indexOf(active);
    return index >= 0 ? index : shown.length - 1;
  })();

  /** Selects an entry of the column at `index` (`how`: a click or a
   * key). A folder shows as the next column; anything else is selected
   * in its column, which becomes the deepest, and shown in the info
   * column. A symbolic link is listed first to know which it is. A click
   * on a folder adds a history entry; the rest replaces it. */
  async function choose(
    entry: FileEntry,
    path: string,
    index: number,
    how: "click" | "key",
  ) {
    const mine = ++choosing.current;
    setMenu(null);
    const folder = folders[index];
    setActive(folder);
    if (how === "key") focusWanted.current = path;
    const outside = leadsOutside(entry, path, root);
    if (entry.kind === "dir") {
      setSelected("");
      if (path !== dir) props.onDir(path, how === "key");
      return;
    }
    if (entry.kind === "symlink" && !outside) {
      try {
        const target = await client.list(path);
        if (mine !== choosing.current) return;
        requested.current.add(target.path);
        store(target.path, { listing: target, error: null, loading: false });
        setSelected("");
        if (target.path !== dir) props.onDir(target.path, how === "key");
        return;
      } catch (failure) {
        if (mine !== choosing.current) return;
        if (!(
          failure instanceof FileError && failure.code === "not_a_directory"
        ))
          props.toast(errorMessage(failure), { tone: "error" });
      }
    }
    setSelected(path);
    if (folder !== dir) props.onDir(folder, true);
    // A click on a link pointing outside the root says why it does not
    // open.
    if (how === "click" && outside) void actions.open(entry, path);
  }
  async function open(entry: FileEntry, path: string) {
    setMenu(null);
    await actions.open(entry, path);
  }
  /** The keyboard enters the column at `index`: its first entry. */
  function enter(index: number) {
    const folder = shown[index];
    if (!folder) return;
    setActive(folder);
    const { base, entries } = entriesOf(folder);
    if (!columns[folder]?.listing) {
      enterWanted.current = folder;
      return;
    }
    const first = entries[0];
    if (first) void choose(first, joinPath(base, first.name), index, "key");
    else
      scroller.current
        ?.querySelector<HTMLElement>(`.files-column:nth-child(${index + 1})`)
        ?.focus();
  }
  // The column entered arrived: its first entry.
  useEffect(() => {
    const folder = enterWanted.current;
    if (!folder || !columns[folder]?.listing) return;
    enterWanted.current = "";
    const index = shown.indexOf(folder);
    if (index >= 0) enter(index);
  });

  function onColumnKey(event: React.KeyboardEvent, index: number) {
    if ((event.target as HTMLElement).closest("input")) return;
    const folder = shown[index];
    // Phones show the deepest column only.
    const at = phone ? folders.length - 1 : index;
    const { base, entries } = entriesOf(folder);
    const paths = entries.map((entry) => joinPath(base, entry.name));
    const row = (event.target as HTMLElement).closest<HTMLElement>(
      "[data-path]",
    );
    let current = row ? paths.indexOf(row.dataset.path || "") : -1;
    if (current < 0) current = paths.indexOf(highlightOf(at));
    const entry = entries[current];
    const plain = !event.ctrlKey && !event.metaKey && !event.altKey;
    const key = event.key;
    if ((key === "ArrowDown" || key === "ArrowUp") && plain) {
      event.preventDefault();
      if (!entries.length) return;
      const next =
        current < 0
          ? 0
          : Math.max(
              0,
              Math.min(
                entries.length - 1,
                current + (key === "ArrowDown" ? 1 : -1),
              ),
            );
      if (next === current) return;
      if (phone) {
        focusWanted.current = paths[next];
        setActive(folder);
        return setTick((tick) => tick + 1);
      }
      void choose(entries[next], paths[next], at, "key");
    } else if (key === "ArrowRight" && plain && !phone) {
      event.preventDefault();
      if (entry && paths[current] === folders[at + 1]) enter(at + 1);
    } else if (key === "ArrowLeft" && plain && !phone) {
      event.preventDefault();
      if (at === 0) return;
      // The folder of this column, selected in its parent's column.
      setActive(folders[at - 1]);
      setSelected("");
      focusWanted.current = folder;
      if (folder !== dir) props.onDir(folder, true);
    } else if (key === "Enter" && plain) {
      event.preventDefault();
      if (!entry) return;
      const path = paths[current];
      if (!phone && path === folders[at + 1]) enter(at + 1);
      else void open(entry, path);
    } else if (key === "ContextMenu" || (key === "F10" && event.shiftKey)) {
      event.preventDefault();
      if (!entry || !row) return;
      const rect = row.getBoundingClientRect();
      setMenu({
        entry,
        path: paths[current],
        x: rect.left + 24,
        y: rect.bottom,
      });
    } else if (
      key.length === 1 &&
      plain &&
      (key !== " " || typed.current.text)
    ) {
      // Type-to-select: the first name starting with what was typed.
      const now = performance.now();
      const buffer = typed.current;
      buffer.text = now - buffer.at > 800 ? key : buffer.text + key;
      buffer.at = now;
      const found = matchPrefix(
        entries.map((entry) => entry.name),
        buffer.text,
        buffer.text.length === 1 ? current + 1 : Math.max(0, current),
      );
      if (found < 0 || found === current) return;
      event.preventDefault();
      if (phone) {
        focusWanted.current = paths[found];
        return setTick((tick) => tick + 1);
      }
      void choose(entries[found], paths[found], at, "key");
    }
  }

  // Focuses the row a keyboard move asked for, once it is shown.
  useEffect(() => {
    const path = focusWanted.current;
    if (!path) return;
    const button = Array.from(
      scroller.current?.querySelectorAll<HTMLElement>(
        ".files-row[data-path]",
      ) || [],
    ).find((row) => row.dataset.path === path);
    if (!button) return;
    focusWanted.current = "";
    button.querySelector<HTMLElement>(".files-open")?.focus();
  });
  // The newest column (or the info column) in view, and each column's
  // highlighted row once.
  const selectedEntry = (() => {
    if (!selected || parentPath(selected) !== dir || phone) return undefined;
    return columns[dir]?.listing?.entries.find(
      (entry) => entry.name === baseName(selected),
    );
  })();
  const informing = !!selectedEntry;
  useLayoutEffect(() => {
    const element = scroller.current;
    if (!element || phone) return;
    const last = element.lastElementChild as HTMLElement | null;
    if (!last) return;
    const right = last.offsetLeft + last.offsetWidth;
    if (right > element.scrollLeft + element.clientWidth)
      element.scrollLeft = right - element.clientWidth;
  }, [shownKey, selected, informing, phone]);
  useLayoutEffect(() => {
    for (const column of scroller.current?.querySelectorAll<HTMLElement>(
      ".files-column",
    ) || []) {
      const body = column.querySelector<HTMLElement>(".files-column-body");
      const row = body?.querySelector<HTMLElement>(".files-row.highlighted");
      const key = `${column.dataset.folder}\n${row?.dataset.path}`;
      if (!body || !row || revealed.current.has(key)) continue;
      revealed.current.add(key);
      const top = row.offsetTop,
        bottom = top + row.offsetHeight;
      if (top < body.scrollTop) body.scrollTop = top;
      else if (bottom > body.scrollTop + body.clientHeight)
        body.scrollTop = bottom - body.clientHeight;
    }
  });

  async function commit() {
    const edit = editing;
    if (!edit || busy) return;
    const name = edit.name.trim();
    const problem = nameProblem(name);
    if (problem) return setEditing({ ...edit, error: problem });
    const folder = edit.kind === "rename" ? parentPath(edit.path) : edit.folder;
    const target = joinPath(folder, name);
    if (edit.kind === "rename" && target === edit.path) return setEditing(null);
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

  function resizeStart(event: React.PointerEvent, index: number) {
    if (event.button !== 0) return;
    event.preventDefault();
    const handle = event.currentTarget as HTMLElement;
    const from = event.clientX;
    const width = widths[index] || COLUMN_WIDTH;
    handle.setPointerCapture(event.pointerId);
    document.body.classList.add("files-resizing");
    const move = (moved: PointerEvent) =>
      setWidths((all) => ({
        ...all,
        [index]: Math.round(
          Math.max(
            MIN_COLUMN,
            Math.min(MAX_COLUMN, width + moved.clientX - from),
          ),
        ),
      }));
    const end = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", end);
      handle.removeEventListener("pointercancel", end);
      document.body.classList.remove("files-resizing");
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", end);
    handle.addEventListener("pointercancel", end);
  }

  const crumbs = rootCrumbs(dir, base, home).map((crumb, index) =>
    index === 0 && scope ? { ...crumb, label: scope.name } : crumb,
  );
  /** The project whose folder holds the deepest folder shown. */
  const here = projects ? projectAt(dir, projects) : undefined;
  const hereColor = projectColor(here);
  /** A crumb that is a project's folder, in its colour. */
  const crumbProject = (path: string) =>
    projects ? projectFolder(path, projects) : undefined;
  const crumbStyle = (path: string) => {
    const color = projectColor(crumbProject(path));
    return color ? ({ "--project": color } as React.CSSProperties) : undefined;
  };
  // The crumbs scroll to the deepest folder (narrow screens).
  useLayoutEffect(() => {
    const bar = crumbBar.current;
    if (bar) bar.scrollLeft = bar.scrollWidth;
  }, [dir]);
  /** Where an error's button goes back to: the root, else the home
   * folder. */
  const top = root || home;
  /** New entries and uploads go into the focused column's folder. */
  const target = shown[activeIndex] || dir;
  const targetListed = !!columns[target]?.listing;
  const targetName =
    crumbs.find((crumb) => crumb.path === target)?.label === "~"
      ? "Home"
      : baseName(target);
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
  const openMenu = (
    event: React.MouseEvent,
    entry: FileEntry,
    path: string,
  ) => {
    const rect = event.currentTarget.getBoundingClientRect();
    setMenu({ entry, path, x: Math.max(8, rect.right - 240), y: rect.bottom });
  };

  function renderColumn(folder: string, index: number) {
    const at = phone ? folders.length - 1 : index;
    const state = columns[folder];
    const listing = state?.listing;
    const error = state?.error;
    const { base, entries } = entriesOf(folder);
    const hiddenCount = listing
      ? listing.entries.length - sortEntries(listing.entries).length
      : 0;
    const highlight = highlightOf(at);
    const isActive = index === activeIndex;
    // One tab stop: the highlighted row of the focused column, else its
    // first row.
    const paths = entries.map((entry) => joinPath(base, entry.name));
    const stop = isActive
      ? paths.includes(highlight)
        ? highlight
        : paths[0]
      : "";
    const deepest = at === folders.length - 1;
    // A column inside a project's folder has a thin rule in its colour.
    const columnColor = projects
      ? projectColor(projectAt(folder, projects))
      : "";
    return (
      <div
        key={folder}
        className={`files-column ${isActive ? "active" : ""} ${dropOn === folder ? "drop-target" : ""} ${columnColor ? "in-project" : ""}`}
        style={
          {
            ...(phone ? {} : { width: widths[index] || COLUMN_WIDTH }),
            ...(columnColor ? { "--project": columnColor } : {}),
          } as React.CSSProperties
        }
        role="group"
        aria-label={`Contents of ${folder === home ? "Home" : baseName(folder)}`}
        aria-busy={state?.loading || !state}
        data-folder={folder}
        tabIndex={-1}
        onKeyDown={(event) => onColumnKey(event, index)}
        onPointerDown={(event) => {
          const target = event.target as HTMLElement;
          if (!target.closest(".files-row")) setActive(folder);
        }}
        {...folderDrop(folder)}
      >
        <div className="files-column-body">
          {editing &&
            editing.kind !== "rename" &&
            editing.folder === folder && (
              <div className="files-row editing">
                <Icon
                  name={editing.kind === "new-folder" ? "folder" : "file"}
                  className="files-kind"
                />
                {nameInput(
                  editing.kind === "new-folder"
                    ? "New folder name"
                    : "New file name",
                )}
              </div>
            )}
          {error && (
            <div className="files-message error" role="alert">
              <p>
                {error.code === "not_found"
                  ? `${folder} does not exist.`
                  : error.code === "not_a_directory"
                    ? `${folder} is not a folder.`
                    : errorMessage(error)}
              </p>
              {deepest && top && folder !== top && (
                <button
                  type="button"
                  className="subtle small"
                  onClick={() => props.onDir(top)}
                >
                  {top === home ? "Go to the home folder" : `Go to ${top}`}
                </button>
              )}
            </div>
          )}
          {!error &&
            listing &&
            !entries.length &&
            !(
              editing &&
              editing.kind !== "rename" &&
              editing.folder === folder
            ) && (
              <p className="files-message">
                {hiddenCount
                  ? `Only hidden entries (${hiddenCount}).`
                  : "Empty folder"}
              </p>
            )}
          {entries.map((entry, row) => {
            const path = paths[row];
            const renaming =
              editing?.kind === "rename" && editing.path === path;
            const folderRow = entry.kind === "dir";
            const outside = leadsOutside(entry, path, root);
            const lit = path === highlight;
            // A folder (or a link) shown as the next column has the
            // Finder's chevron, as does every folder.
            const chevron = folderRow || (lit && !!folders[at + 1]);
            // A project's folder shows in the project's colour.
            const rowProject =
              folderRow && projects ? projectFolder(path, projects) : undefined;
            const rowColor = projectColor(rowProject);
            return (
              <div
                key={entry.name}
                className={`files-row ${lit ? "highlighted" : ""} ${dropOn === path ? "drop-target" : ""} ${menu?.path === path ? "menu-open" : ""} ${outside ? "outside" : ""} ${rowProject ? "project-folder" : ""}`}
                style={
                  rowColor
                    ? ({ "--project": rowColor } as React.CSSProperties)
                    : undefined
                }
                data-project-folder={rowProject?.id}
                data-path={path}
                data-kind={entry.kind}
                data-outside={outside || undefined}
                draggable={!renaming}
                onDragStart={(event) => startPathDrag(event, [path])}
                onContextMenu={(event) => {
                  event.preventDefault();
                  setMenu({ entry, path, x: event.clientX, y: event.clientY });
                }}
                onFocus={() => setActive(folder)}
                {...(folderRow ? folderDrop(path) : {})}
              >
                <Icon
                  name={kindIcon(entry)}
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
                    aria-current={lit || undefined}
                    aria-disabled={outside || undefined}
                    tabIndex={path === stop ? 0 : -1}
                    onClick={() =>
                      phone
                        ? void open(entry, path)
                        : void choose(entry, path, at, "click")
                    }
                    onDoubleClick={() => {
                      if (phone || entry.kind === "dir") return;
                      void open(entry, path);
                    }}
                  >
                    <span className="files-name">{entry.name}</span>
                    <small className="files-meta">
                      {rowProject
                        ? `Project ${rowProject.name} · ${entryMeta(entry)}`
                        : entryMeta(entry)}
                    </small>
                  </button>
                )}
                <button
                  type="button"
                  className="icon-btn files-more"
                  aria-label={`Actions for ${entry.name}`}
                  tabIndex={-1}
                  onClick={(event) => openMenu(event, entry, path)}
                >
                  <Icon name="ellipsis" size="sm" />
                </button>
                {chevron && (
                  <Icon
                    name="chevron-right"
                    size="sm"
                    className="files-chevron"
                  />
                )}
              </div>
            );
          })}
          {listing?.truncated && (
            <p className="files-message small">
              Showing the first {listing.entries.length} entries of this folder.
            </p>
          )}
        </div>
        {!phone && (
          <div
            className="files-column-edge"
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize column"
            onPointerDown={(event) => resizeStart(event, index)}
            onDoubleClick={() =>
              setWidths((all) => {
                const { [index]: _, ...rest } = all;
                return rest;
              })
            }
          />
        )}
      </div>
    );
  }

  return (
    <section
      className={`files-view files-browser ${phone ? "single-column" : ""}`}
      aria-label="Files"
      onClick={(event) => event.stopPropagation()}
    >
      {props.tabs}
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
        <nav ref={crumbBar} className="files-crumbs" aria-label="Folder path">
          {crumbs.map((crumb, index) => (
            <React.Fragment key={crumb.path}>
              {index > 0 && crumbs[index - 1].label !== "/" && (
                <span className="files-crumb-sep" aria-hidden="true">
                  /
                </span>
              )}
              <button
                type="button"
                className={`ghost files-crumb ${dropOn === crumb.path ? "drop-target" : ""} ${crumbProject(crumb.path) ? "project-crumb" : ""}`}
                style={crumbStyle(crumb.path)}
                data-project-folder={crumbProject(crumb.path)?.id}
                aria-current={
                  index === crumbs.length - 1 ? "location" : undefined
                }
                onClick={() => {
                  setActive("");
                  setSelected("");
                  props.onDir(crumb.path);
                }}
                {...folderDrop(crumb.path)}
              >
                {crumb.label === "~" ? <Icon name="house" size="sm" /> : null}
                {crumbProject(crumb.path) && (
                  <span className="project-dot" aria-hidden="true" />
                )}
                <span title={index === 0 ? crumb.path : undefined}>
                  {crumb.label === "~" ? "Home" : crumb.label}
                </span>
              </button>
            </React.Fragment>
          ))}
        </nav>
        {scope && props.onAllFiles ? (
          <button
            type="button"
            className="ghost small files-project-link"
            onClick={props.onAllFiles}
          >
            All files
          </button>
        ) : here && props.onOpenProject ? (
          <button
            type="button"
            className="ghost small files-project-link"
            style={
              hereColor
                ? ({ "--project": hereColor } as React.CSSProperties)
                : undefined
            }
            title={`Show the files of ${here.name}`}
            onClick={() => props.onOpenProject!(here)}
          >
            <span
              className={`project-dot ${hereColor ? "" : "plain"}`}
              aria-hidden="true"
            />
            {here.name}
          </button>
        ) : null}
        <div className="files-tools">
          <button
            type="button"
            className="icon-btn"
            title="Parent folder"
            aria-label="Parent folder"
            disabled={!canGoUp(dir, base)}
            onClick={() => {
              setActive("");
              props.onDir(parentPath(dir));
            }}
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
            disabled={shown.some((folder) => columns[folder]?.loading)}
            onClick={() => {
              for (const folder of shown) void load(folder);
            }}
          >
            <Icon name="refresh-cw" size="sm" />
          </button>
          <button
            type="button"
            className="icon-btn"
            title={`New file in ${targetName}`}
            aria-label="New file"
            disabled={!targetListed}
            onClick={() =>
              setEditing({
                kind: "new-file",
                folder: target,
                name: "",
                error: "",
              })
            }
          >
            <Icon name="file-plus" size="sm" />
          </button>
          <button
            type="button"
            className="icon-btn"
            title={`New folder in ${targetName}`}
            aria-label="New folder"
            disabled={!targetListed}
            onClick={() =>
              setEditing({
                kind: "new-folder",
                folder: target,
                name: "",
                error: "",
              })
            }
          >
            <Icon name="folder-plus" size="sm" />
          </button>
          <button
            type="button"
            className="icon-btn"
            title={`Upload files to ${targetName}`}
            aria-label="Upload files here"
            disabled={!targetListed || !!uploading}
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
              if (files.length) void actions.uploadInto(target, files, []);
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
      <div className="files-columns" ref={scroller}>
        {shown.map(renderColumn)}
        {selectedEntry && (
          <InfoColumn
            entry={selectedEntry}
            path={selected}
            client={client}
            home={home}
            root={root}
            canOpenBeside={!!props.onOpenBeside}
            onOpen={() => void open(selectedEntry, selected)}
            onOpenBeside={() => props.onOpenBeside?.(selected)}
            onDownload={() =>
              startDownload(client, selected, false, props.toast)
            }
          />
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

/** The info column at the far right: what the selected entry is, the
 * first lines of a small text file, and its actions. */
function InfoColumn({
  entry,
  path,
  client,
  home,
  root,
  canOpenBeside,
  onOpen,
  onOpenBeside,
  onDownload,
}: {
  entry: FileEntry;
  path: string;
  client: FilesClient;
  home?: string;
  root?: string;
  canOpenBeside: boolean;
  onOpen: () => void;
  onOpenBeside: () => void;
  onDownload: () => void;
}) {
  const can = entryAbilities(entry, path, root);
  const opens = can.open && entry.kind !== "other";
  const outside = leadsOutside(entry, path, root);
  const facts: [string, string][] = [
    ["Kind", kindLabel[entry.kind] || entry.kind],
    ...(entry.kind === "file"
      ? ([
          [
            "Size",
            `${formatSize(entry.size)}${entry.size >= 1000 ? ` (${entry.size.toLocaleString()} bytes)` : ""}`,
          ],
        ] as [string, string][])
      : []),
    ["Modified", formatModifiedFull(entry.modified)],
    ["Where", displayPath(parentPath(path), home)],
    ...(entry.kind === "symlink" && entry.target
      ? ([
          [
            "Points to",
            `${entry.target}${outside ? ` (${outsideRootMessage.toLowerCase()})` : ""}`,
          ],
        ] as [string, string][])
      : []),
  ];
  return (
    <aside
      className="files-info-column"
      aria-label={`About ${entry.name}`}
      data-path={path}
    >
      <Icon
        name={kindIcon(entry)}
        size="lg"
        className={`files-kind ${entry.kind}`}
      />
      <h3 className="files-info-name">{entry.name}</h3>
      <Preview client={client} path={path} entry={entry} />
      <dl className="files-facts">
        {facts.map(([term, value]) => (
          <React.Fragment key={term}>
            <dt>{term}</dt>
            <dd data-fact={term.toLowerCase()}>{value}</dd>
          </React.Fragment>
        ))}
      </dl>
      <div className="files-info-actions">
        {opens && (
          <button type="button" className="subtle small" onClick={onOpen}>
            Open
          </button>
        )}
        {opens && canOpenBeside && (
          <button type="button" className="subtle small" onClick={onOpenBeside}>
            Open beside terminal
          </button>
        )}
        {client.canDownload && can.download && (
          <button type="button" className="subtle small" onClick={onDownload}>
            <Icon name="download" size="sm" />
            Download
          </button>
        )}
      </div>
    </aside>
  );
}

/** The first lines of a small text file (none for anything else). */
function Preview({
  client,
  path,
  entry,
}: {
  client: FilesClient;
  path: string;
  entry: FileEntry;
}) {
  const [text, setText] = useState<string | null>(null);
  const limit = Math.min(PREVIEW_BYTES, client.limits.max_read_bytes);
  const small = entry.kind === "file" && entry.size > 0 && entry.size <= limit;
  useEffect(() => {
    setText(null);
    if (!small) return;
    let live = true;
    client.read(path, { maxBytes: limit }).then(
      (content) => {
        if (live && !content.binary && content.text !== undefined)
          setText(content.text.split("\n").slice(0, PREVIEW_LINES).join("\n"));
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, [client, path, small, limit, entry.modified, entry.size]);
  if (!text) return null;
  return (
    <pre className="files-preview" aria-label={`First lines of ${entry.name}`}>
      {text}
    </pre>
  );
}
