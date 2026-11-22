// Per-browser web layout: the view mode, tab order, focused terminal,
// parked terminals, tab colours and sidebar and notification preferences.
// Layout is client-local by design: the TUI and every browser keep their own
// tabs, grid, colours and focus over the same KelpServer panes and
// processes (ADR 0002: client UI state is keyed by server epoch and pane
// ID). Nothing here is sent to the server. Pane IDs are scoped to a server
// epoch, so stored order, focus, parking and colours are discarded when the
// epoch changes.
//
// Storage can be unavailable (private windows, blocked site data); every
// access is wrapped and the UI works without it.

export type ViewMode = "single" | "grid";

const viewKey = "kelpmux-web-view";
const layoutKey = (workspace: string) => `kelpmux-web-layout:${workspace}`;

/** Number of tab colours (the TUI's palette, see `palette.ts`). */
export const TAB_COLORS = 9;
/** This browser's tab colour for each pane that has one, by pane ID. */
export type TabColors = Record<number, number>;

/** This browser's layout of one workspace epoch. `parked` terminals are
 * hidden from this browser's tabs and grid (the TUI's own parking is
 * separate); `colors` are the tab colours picked in this browser. */
export type Layout = {
  epoch: string;
  order: number[];
  focus: number;
  parked: number[];
  colors: TabColors;
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
  const empty = { epoch, order: [], focus: 0, parked: [], colors: {} };
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
      colors: tabColors(value.colors),
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

/** This browser's grid track weights for one grid shape (`colsxrows`).
 * Missing entries are 1. */
export type GridWeights = { shape: string; cols: number[]; rows: number[] };
export const GRID_WEIGHT_MIN = 0.4;
export const GRID_WEIGHT_MAX = 3;
/** A CSS grid template for `count` tracks with these weights. */
export function gridTemplate(weights: number[], count: number) {
  return Array.from(
    { length: count },
    (_, i) => `minmax(0,${weights[i] ?? 1}fr)`,
  ).join(" ");
}
/** Weights after growing (+1) or shrinking (-1) track `index` by a quarter. */
export function resizeTrack(
  weights: number[],
  count: number,
  index: number,
  step: number,
) {
  if (index < 0 || index >= count || count < 2) return weights;
  const next = Array.from({ length: count }, (_, i) => weights[i] ?? 1);
  next[index] =
    Math.round(
      Math.min(
        GRID_WEIGHT_MAX,
        Math.max(GRID_WEIGHT_MIN, next[index] + step * 0.25),
      ) * 100,
    ) / 100;
  return next;
}

/** Stored tab colours, keeping only pane IDs with a valid colour index. */
function tabColors(value: unknown): TabColors {
  const colors: TabColors = {};
  if (!value || typeof value !== "object" || Array.isArray(value))
    return colors;
  for (const [key, color] of Object.entries(value).slice(0, 1024)) {
    const id = Number(key);
    if (
      Number.isSafeInteger(id) &&
      id > 0 &&
      Number.isInteger(color) &&
      (color as number) >= 0 &&
      (color as number) < TAB_COLORS
    )
      colors[id] = color as number;
  }
  return colors;
}

/** A pane's tab colour in this browser: the one picked here, otherwise
 * the TUI's default for its ID (`(id - 1) % 9`), so a fresh browser and
 * a fresh TUI start from the same colours. */
export function tabColor(colors: TabColors, pane: number): number {
  return colors[pane] ?? (Math.max(pane, 1) - 1) % TAB_COLORS;
}

/** The colours left after closed panes are dropped; the same object when
 * nothing changed. */
export function liveColors(colors: TabColors, live: Set<number>): TabColors {
  const kept = Object.entries(colors).filter(([id]) => live.has(Number(id)));
  return kept.length === Object.keys(colors).length
    ? colors
    : Object.fromEntries(kept);
}
