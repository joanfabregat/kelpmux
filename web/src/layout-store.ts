// Per-browser web layout: the view mode, tab order, focused terminal,
// parked terminals and sidebar and notification preferences.
// Layout is client-local by design: the TUI and every browser keep their own
// tabs, grid and focus over the same KelpServer panes and processes. Nothing
// here is sent to the server. Pane IDs are scoped to a server epoch, so
// stored order and focus are discarded when the epoch changes.
//
// Storage can be unavailable (private windows, blocked site data); every
// access is wrapped and the UI works without it.

export type ViewMode = "single" | "grid";

const viewKey = "kelpmux-web-view";
const layoutKey = (workspace: string) => `kelpmux-web-layout:${workspace}`;

/** This browser's layout of one workspace epoch. `parked` terminals are
 * hidden from this browser's tabs and grid (the TUI's own parking is
 * separate). */
export type Layout = {
  epoch: string;
  order: number[];
  focus: number;
  parked: number[];
};

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}
function write(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Storage unavailable: the layout still applies until a reload.
  }
}

export function loadView(): ViewMode {
  return read(viewKey) === "single" ? "single" : "grid";
}
export function saveView(view: ViewMode) {
  write(viewKey, view);
}

/** The stored layout for this workspace, if it belongs to this epoch. */
export function loadLayout(workspace: string, epoch: string): Layout {
  const empty = { epoch, order: [], focus: 0, parked: [] };
  try {
    const value = JSON.parse(read(layoutKey(workspace)) || "null");
    if (!value || value.epoch !== epoch || !Array.isArray(value.order))
      return empty;
    return {
      epoch,
      order: value.order
        .filter((id: unknown) => Number.isSafeInteger(id) && (id as number) > 0)
        .slice(0, 1024),
      focus: Number.isSafeInteger(value.focus) ? value.focus : 0,
      parked: ids(value.parked),
    };
  } catch {
    return empty;
  }
}
export function saveLayout(workspace: string, layout: Layout) {
  if (!workspace || !layout.epoch) return;
  write(layoutKey(workspace), JSON.stringify(layout));
}

/** Panes in this browser's order: known panes by their stored position,
 * new panes after them in the server's order. */
export function orderPanes<T extends { id: number }>(
  panes: T[],
  order: number[],
): T[] {
  const position = new Map(order.map((id, index) => [id, index]));
  return panes
    .map((pane, index) => ({ pane, index }))
    .sort((a, b) => {
      const pa = position.get(a.pane.id),
        pb = position.get(b.pane.id);
      if (pa !== undefined && pb !== undefined) return pa - pb;
      if (pa !== undefined) return -1;
      if (pb !== undefined) return 1;
      return a.index - b.index;
    })
    .map(({ pane }) => pane);
}

/** The order after moving `source` before `before` (null: to the end). */
export function moveBefore(
  ids: number[],
  source: number,
  before: number | null,
): number[] {
  if (source === before) return ids;
  const order = ids.filter((id) => id !== source);
  if (!ids.includes(source)) return ids;
  const to = before === null ? -1 : order.indexOf(before);
  order.splice(to < 0 ? order.length : to, 0, source);
  return order;
}

const ids = (value: unknown): number[] =>
  Array.isArray(value)
    ? value
        .filter((id: unknown) => Number.isSafeInteger(id) && (id as number) > 0)
        .slice(0, 1024)
    : [];

/** This browser's interface preferences (never sent to the server; the
 * TUI keeps its own configuration). */
export type Preferences = {
  /** Sidebar width in CSS pixels. */
  sidebarWidth: number;
  sidebarFolded: boolean;
  /** Play a sound when a terminal newly needs attention or finishes. */
  sound: boolean;
  /** Show a desktop notification for the same events while hidden. */
  desktop: boolean;
};
const preferencesKey = "kelpmux-web-preferences";
export const SIDEBAR_MIN = 200;
export const SIDEBAR_MAX = 480;
export const defaultPreferences: Preferences = {
  sidebarWidth: 248,
  sidebarFolded: false,
  sound: false,
  desktop: false,
};
export function loadPreferences(): Preferences {
  try {
    const value = JSON.parse(read(preferencesKey) || "{}");
    if (!value || typeof value !== "object") return { ...defaultPreferences };
    const width = Number(value.sidebarWidth);
    return {
      sidebarWidth: Number.isFinite(width)
        ? Math.round(Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, width)))
        : defaultPreferences.sidebarWidth,
      sidebarFolded: value.sidebarFolded === true,
      sound: value.sound === true,
      desktop: value.desktop === true,
    };
  } catch {
    return { ...defaultPreferences };
  }
}
export function savePreferences(preferences: Preferences) {
  write(preferencesKey, JSON.stringify(preferences));
}

/** Moves `source` one place up (-1) or down (+1) among `ids`. */
export function moveBy(ids: number[], source: number, step: -1 | 1) {
  const index = ids.indexOf(source);
  const target = index + step;
  if (index < 0 || target < 0 || target >= ids.length) return ids;
  const order = [...ids];
  [order[index], order[target]] = [order[target], order[index]];
  return order;
}
