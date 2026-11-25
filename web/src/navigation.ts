// Where the application is, in the page URL, so Back and Forward (the
// browser's buttons, mouse buttons, Alt+← / Alt+→, a swipe), a reload, a
// bookmark or a link return to the same place (docs/web.md, Navigation).
//
//   ?view=terminal&terminal=<pane id>              one terminal on its own
//   ?view=terminal&terminal=<id>&side=<path>       with a file beside it
//   ?view=grid                                     the live grid
//   ?view=files&dir=<folder>                       the file browser
//   ?view=files&file=<path>[&line=<n>]             an open file
//   ?view=home                                     a phone's terminal list
//   ?view=repos&project=<project id>              a project's Repos view
//   …&dialog=<panel>[&of=<pane id>]                a panel over that place
//   …&project=<project id>                          the grid, the file
//                                                  browser or History
//                                                  of one project (`scratch`
//                                                  for Scratch)
//
// Terminals are named by their server pane id, which stays the same while
// they are reordered, parked or renumbered; never by their position. `line`
// belongs to the file shown (in the main area or beside the terminal).
// Parameters this module does not own (KelpHub's workspace and
// scratch, …) are kept as they are, and "pane" is reserved for windows
// opened with Open in a window, which do not keep a history.
//
// Entry rules (historyMode): a move to another place pushes an entry, and so
// does opening a panel (Settings, Help, Terminals, Search, History,
// Parked, Move, System status, a terminal's notifications), so Back closes
// it; closing that panel goes back to the entry below instead of adding one.
// Transient interface (menus, tooltips, toasts, the find bar, confirmations,
// the upload review, security key prompts) never has an entry. The first
// location after a load or reconnection, a place selected again, a panel
// swapped for another, a new line in the same file and automatic changes (a
// closed terminal, a folder name normalised, a phone rotated into a tablet)
// replace the current entry.
import { useEffect, useRef, useState } from "react";

/** Bumped when the meaning of the URL or of `history.state` changes;
 * readHistoryState migrates older entries. */
export const HISTORY_VERSION = 1;

export type View = "terminal" | "grid" | "files" | "home" | "repos";

export type AppLocation = {
  view: View;
  /** The terminal shown on its own (view "terminal"); 0 when none is. */
  terminal: number;
  /** The file browser's folder (view "files" without a file). */
  dir: string;
  /** The open file shown in the main area (view "files"). */
  file: string;
  /** The file beside the terminal (view "terminal"). */
  side: string;
  /** The line the file shown opened at; 0 for none. */
  line: number;
  /** A modal panel over the place, by its URL name (dialogs below). */
  dialog: string;
  /** The project the grid, the file browser or History shows
   * (its id; "" for all). */
  project: string;
  /** The terminal whose notifications the panel shows. */
  of: number;
};

export const emptyLocation: AppLocation = {
  view: "grid",
  terminal: 0,
  dir: "",
  file: "",
  side: "",
  line: 0,
  dialog: "",
  of: 0,
  project: "",
};

/** Panels with an entry: URL name → the application's dialog name. */
const dialogs: Record<string, string> = {
  settings: "config",
  help: "help",
  terminals: "terminals",
  search: "search",
  history: "recovery",
  parked: "parked",
  move: "move",
  status: "stats",
  notifications: "notifications",
};

/** Older URL names of panels, read as their current name (the URL is
 * then rewritten with the current one). */
const legacyDialogs: Record<string, string> = { closed: "history" };

/** The URL name of an application dialog ("" when it has no entry). */
export function dialogParam(dialog: string): string {
  return Object.keys(dialogs).find((name) => dialogs[name] === dialog) || "";
}
/** The application dialog a URL name opens ("" for none or unknown). */
export function dialogFromParam(name: string): string {
  return Object.hasOwn(dialogs, name) ? dialogs[name] : "";
}

const owned = [
  "view",
  "terminal",
  "side",
  "dir",
  "file",
  "line",
  "dialog",
  "of",
  "project",
];
const views: View[] = ["terminal", "grid", "files", "home", "repos"];

function id(value: string | null): number {
  return value && /^[1-9]\d{0,9}$/.test(value) ? Number(value) : 0;
}
function path(value: string | null): string {
  return value && value.length <= 4096 && !/[\u0000-\u001f\u007f]/.test(value)
    ? value
    : "";
}

/** The location a URL asks for: the fields it names, valid ones only. Older
 * URLs (KelpHub's view=terminals, overview and events) map to the nearest
 * place; a URL without a view leaves the place to the application. */
export function parseLocation(search: string): Partial<AppLocation> {
  const params = new URLSearchParams(search);
  const result: Partial<AppLocation> = {};
  let view = params.get("view") || "";
  const terminal = id(params.get("terminal"));
  // KelpHub: Terminals was the grid, or one terminal with `terminal`; the
  // overview and events pages have no equivalent here.
  if (view === "terminals") view = terminal ? "terminal" : "grid";
  if ((views as string[]).includes(view)) {
    result.view = view as View;
    if (view === "terminal") {
      result.terminal = terminal;
      result.side = path(params.get("side"));
    }
    if (view === "files") {
      result.file = path(params.get("file"));
      result.dir = result.file ? "" : path(params.get("dir"));
    }
    if (result.file || result.side) result.line = id(params.get("line"));
  }
  // KelpMux before History named that panel "closed".
  const asked = params.get("dialog") || "";
  const dialog = Object.hasOwn(legacyDialogs, asked)
    ? legacyDialogs[asked]
    : asked;
  if (dialogFromParam(dialog)) {
    result.dialog = dialog;
    if (dialog === "notifications") result.of = id(params.get("of"));
  }
  const project = params.get("project") || "";
  if (
    /^[a-z0-9-]{1,64}$/.test(project) &&
    (scoped(result.view) || result.dialog === "history")
  )
    result.project = project;
  return result;
}

/** Views a project scopes: the grid, the file browser and the Repos view
 * (which always names one). */
function scoped(view: View | undefined) {
  return view === "grid" || view === "files" || view === "repos";
}
/** Whether a location keeps its project: a scoped view, or History
 * over any place. */
function keepsProject(location: Pick<AppLocation, "view" | "dialog">) {
  return scoped(location.view) || location.dialog === "history";
}

/** Whether a URL names a location (otherwise the application keeps its
 * own default place). */
export function hasLocation(wanted: Partial<AppLocation>): boolean {
  return wanted.view !== undefined || wanted.dialog !== undefined;
}

/** A complete location from a parsed URL: a URL with a view names the whole
 * place; one with only a dialog opens it over `fallback`'s place. */
export function resolveLocation(
  wanted: Partial<AppLocation>,
  fallback: AppLocation,
): AppLocation {
  const place = wanted.view
    ? { ...emptyLocation, ...wanted, dialog: "", of: 0 }
    : { ...fallback };
  return {
    ...place,
    dialog: wanted.dialog || "",
    of: wanted.dialog === "notifications" ? wanted.of || 0 : 0,
    project: keepsProject({ view: place.view, dialog: wanted.dialog || "" })
      ? wanted.project || (wanted.view ? "" : place.project)
      : "",
  };
}

// Paths stay readable: "/" and "~" need no escaping in a query.
function encode(value: string) {
  return encodeURIComponent(value).replace(/%2F/g, "/").replace(/%40/g, "@");
}

/** The query string ("" or "?…") for a location. Parameters this module
 * does not own are kept first, in their original order. */
export function locationSearch(location: AppLocation, currentSearch = "") {
  const params = new URLSearchParams(currentSearch);
  for (const key of owned) params.delete(key);
  const parts = params.toString() ? [params.toString()] : [];
  const add = (key: string, value: string | number) =>
    parts.push(`${key}=${encode(String(value))}`);
  add("view", location.view);
  if (location.view === "terminal") {
    if (location.terminal) add("terminal", location.terminal);
    if (location.side) add("side", location.side);
  }
  if (location.view === "files") {
    if (location.file) add("file", location.file);
    else if (location.dir) add("dir", location.dir);
  }
  if (location.line && (location.file || location.side))
    add("line", location.line);
  if (location.dialog) {
    add("dialog", location.dialog);
    if (location.dialog === "notifications" && location.of)
      add("of", location.of);
  }
  if (location.project && keepsProject(location))
    add("project", location.project);
  return `?${parts.join("&")}`;
}

/** The application's state that makes up its location. */
export type LocationState = {
  /** The phone shows its terminal list. */
  phoneHome: boolean;
  /** The main area's view, as shown ("single" is one terminal). */
  view: "single" | "grid" | "files" | "repos";
  /** The terminal shown on its own (0: none). */
  terminal: number;
  filesDir: string;
  /** The open file shown in the main area ("" for the browser). */
  file: string;
  fileLine?: number;
  /** The file in the side panel while it shows beside the terminal. */
  side: string;
  sideLine?: number;
  /** The application's dialog ("" for none). */
  dialog: string;
  notifyPane: number;
  /** The project the grid, the file browser or History shows. */
  project?: string;
};

/** The location the application state shows. */
export function locationOf(state: LocationState): AppLocation {
  const location = placeOf(state);
  return keepsProject(location)
    ? { ...location, project: state.project || "" }
    : location;
}
function placeOf(state: LocationState): AppLocation {
  const dialog = dialogParam(state.dialog);
  const overlay = {
    dialog,
    of: dialog === "notifications" ? state.notifyPane : 0,
    project: "",
  };
  if (state.phoneHome) return { ...emptyLocation, view: "home", ...overlay };
  if (state.view === "files")
    return {
      ...emptyLocation,
      view: "files",
      file: state.file,
      dir: state.file ? "" : state.filesDir,
      line: state.file ? state.fileLine || 0 : 0,
      ...overlay,
    };
  if (state.view === "repos")
    return { ...emptyLocation, view: "repos", ...overlay };
  if (state.view === "grid")
    return { ...emptyLocation, view: "grid", ...overlay };
  return {
    ...emptyLocation,
    view: "terminal",
    terminal: state.terminal,
    side: state.side,
    line: state.side ? state.sideLine || 0 : 0,
    ...overlay,
  };
}

export function sameLocation(a: AppLocation | null, b: AppLocation | null) {
  return !!a && !!b && sameBase(a, b) && a.line === b.line && sameOverlay(a, b);
}
/** The same place, whatever panel is over it and whatever line. */
export function sameBase(a: AppLocation, b: AppLocation) {
  return (
    a.view === b.view &&
    a.terminal === b.terminal &&
    a.dir === b.dir &&
    a.file === b.file &&
    a.side === b.side &&
    (!scoped(a.view) || a.project === b.project)
  );
}
function sameOverlay(a: AppLocation, b: AppLocation) {
  return (
    a.dialog === b.dialog &&
    a.of === b.of &&
    (a.dialog !== "history" || a.project === b.project)
  );
}

export type HistoryDecision = {
  /** none: the URL already says it; back: closes the panel by returning
   * to the entry below it. */
  mode: "none" | "push" | "replace" | "back";
  /** The written entry is a panel over the entry below's place, so
   * closing it goes back. */
  overlay: boolean;
};

/**
 * How a change of location from `previous` (the current entry, null before
 * the first) to `next` reaches the history. `automatic` is a change the
 * user did not ask for (a fallback, a normalisation, a restored entry being
 * corrected); `overlay` says whether the current entry is a panel pushed
 * over the entry below.
 */
export function historyMode(
  previous: AppLocation | null,
  next: AppLocation,
  { automatic = false, overlay = false } = {},
): HistoryDecision {
  if (!previous) return { mode: "replace", overlay: overlay && !!next.dialog };
  if (sameLocation(previous, next)) return { mode: "none", overlay };
  const base = sameBase(previous, next);
  const kept = overlay && base && !!next.dialog;
  if (automatic) return { mode: "replace", overlay: kept };
  if (previous.dialog && !next.dialog) {
    // Closing the panel returns to the entry below; leaving from inside it
    // (a terminal opened from the Terminals table) takes its entry's place.
    if (overlay) return { mode: base ? "back" : "replace", overlay: false };
    return { mode: base ? "replace" : "push", overlay: false };
  }
  if (previous.dialog && next.dialog)
    return base
      ? { mode: "replace", overlay: kept }
      : { mode: "push", overlay: false };
  if (next.dialog) return { mode: "push", overlay: base };
  // Another line in the same file refines the entry.
  if (base) return { mode: "replace", overlay: false };
  return { mode: "push", overlay: false };
}

/** The `history.state` this module writes. */
export function historyState(overlay: boolean) {
  return { kelpmux: { v: HISTORY_VERSION, overlay } };
}
/** Reads an entry's state; unknown or older versions read as plain
 * entries (the URL holds the location itself). */
export function readHistoryState(state: unknown): { overlay: boolean } {
  const value = (state as any)?.kelpmux;
  if (value?.v === HISTORY_VERSION) return { overlay: value.overlay === true };
  return { overlay: false };
}

/**
 * Mirrors the application's location into the browser history and applies
 * the locations Back, Forward and a loaded URL ask for. `here` is the
 * location the state shows (null: no history, as in a popup window);
 * `ready` is false until the state can be shown (signed in, terminals
 * listed, files known), and each time it becomes ready the URL's location is
 * applied again. `apply` changes the state to show a location; anything it
 * cannot show (a closed terminal) it replaces, after calling `automatic`.
 * `context` names conditions under which the same state shows another
 * location (a phone or a wider screen): a change of context replaces.
 *
 * Every change of `here` goes through one decision (historyMode), so the
 * sidebar, the keyboard shortcuts and the panels' own buttons push and
 * replace alike. Call `automatic()` just before a state change the user did
 * not ask for; from an effect, only from one declared after this hook.
 */
export function useHistorySync(options: {
  here: AppLocation | null;
  ready: boolean;
  apply: (target: AppLocation, reason: "load" | "pop") => void;
  context?: string;
}) {
  const [, setTick] = useState(0);
  /** The entry this window wrote or applied last, whether it is a panel
   * over the entry below, the pending automatic change and context. */
  const nav = useRef({
    entry: null as AppLocation | null,
    overlay: false,
    automatic: false,
    applied: false,
    context: options.context,
  });
  const latest = useRef(options);
  latest.current = options;
  const active = options.ready && !!options.here;
  // Writes the state's location (runs after every render).
  useEffect(() => {
    const state = nav.current,
      { here, context } = latest.current;
    if (!active || !state.applied || !here) return;
    let automatic = state.automatic;
    state.automatic = false;
    if (state.context !== context) {
      state.context = context;
      automatic = true;
    }
    const decision = historyMode(state.entry, here, {
      automatic,
      overlay: state.overlay,
    });
    state.entry = here;
    state.overlay = decision.overlay;
    const url = `${location.pathname}${locationSearch(here, location.search)}${location.hash}`;
    const entry = historyState(decision.overlay);
    if (decision.mode === "back") history.back();
    else if (decision.mode === "push") history.pushState(entry, "", url);
    else if (
      decision.mode === "replace" ||
      url !== `${location.pathname}${location.search}${location.hash}`
    )
      history.replaceState(entry, "", url);
  });
  // Applies the URL's location once the state is ready.
  useEffect(() => {
    const state = nav.current,
      { here, context, apply } = latest.current;
    if (!active || !here) {
      state.applied = false;
      return;
    }
    if (state.applied) return;
    state.applied = true;
    state.entry = null;
    state.overlay = readHistoryState(history.state).overlay;
    state.context = context;
    const wanted = parseLocation(location.search);
    if (hasLocation(wanted)) {
      const target = resolveLocation(wanted, here);
      if (!sameLocation(target, here)) apply(target, "load");
    }
    setTick((tick) => tick + 1);
  }, [active]);
  useEffect(() => {
    const popped = (event: PopStateEvent) => {
      const state = nav.current,
        { here, apply } = latest.current;
      if (!state.applied || !here) return;
      state.overlay = readHistoryState(event.state).overlay;
      const target = resolveLocation(
        parseLocation(location.search),
        state.entry || here,
      );
      // The entry below a closed panel: already shown.
      if (sameLocation(target, state.entry)) return;
      state.entry = target;
      // Whatever the state cannot show replaces this entry.
      state.automatic = true;
      apply(target, "pop");
      setTick((tick) => tick + 1);
    };
    window.addEventListener("popstate", popped);
    return () => window.removeEventListener("popstate", popped);
  }, []);
  return {
    /** The next change of location replaces the current entry. */
    automatic() {
      nav.current.automatic = true;
      setTick((tick) => tick + 1);
    },
  };
}
