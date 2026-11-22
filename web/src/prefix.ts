// The browser's Ctrl-B prefix keys. After Ctrl-B, a key names an action as
// in the TUI's default bindings (src/native/input.rs), adapted where the
// browser has its own equivalent. Pure, so node tests can check the map.

export type PrefixAction =
  | "new"
  | "reopen-last"
  | "scroll-mode"
  | "select-mode"
  | "grid-resize-left"
  | "grid-resize-right"
  | "grid-resize-up"
  | "grid-resize-down"
  | "grid-equalize"
  | "grid"
  | "next"
  | "previous"
  | "page-previous"
  | "page-next"
  | "focus-left"
  | "focus-right"
  | "focus-up"
  | "focus-down"
  | "next-attention"
  | "next-finished"
  | "agents"
  | "agents-attention"
  | "agents-finished"
  | "notifications"
  | "overview"
  | "recovery"
  | "search"
  | "park"
  | "parked"
  | "move"
  | "close"
  | "settings"
  | "help"
  | "disconnect"
  | "send-prefix"
  | { select: number };

/** The action of the key pressed after the prefix, or null for none. */
export function prefixAction(event: {
  key: string;
  ctrlKey?: boolean;
  shiftKey?: boolean;
}): PrefixAction | null {
  const { key } = event;
  if (event.ctrlKey && key.toLowerCase() === "b") return "send-prefix";
  if (event.ctrlKey) return null;
  if (key === "Tab") return event.shiftKey ? "previous" : "next";
  if (/^[1-9]$/.test(key)) return { select: Number(key) };
  const map: Record<string, PrefixAction> = {
    Enter: "new",
    z: "reopen-last",
    "[": "scroll-mode",
    " ": "select-mode",
    H: "grid-resize-left",
    L: "grid-resize-right",
    K: "grid-resize-up",
    J: "grid-resize-down",
    "=": "grid-equalize",
    g: "grid",
    e: "grid",
    PageUp: "page-previous",
    PageDown: "page-next",
    h: "focus-left",
    ArrowLeft: "focus-left",
    l: "focus-right",
    ArrowRight: "focus-right",
    k: "focus-up",
    ArrowUp: "focus-up",
    j: "focus-down",
    ArrowDown: "focus-down",
    N: "next-attention",
    P: "next-finished",
    a: "agents",
    "!": "agents-attention",
    n: "agents-finished",
    b: "notifications",
    o: "overview",
    r: "recovery",
    "/": "search",
    p: "park",
    u: "parked",
    s: "move",
    x: "close",
    ",": "settings",
    "?": "help",
    d: "disconnect",
  };
  return Object.hasOwn(map, key) ? map[key] : null;
}

/** The grid neighbour of tile `index` in direction (dx, dy) for `count`
 * tiles in `cols` columns, or the same index at an edge. */
export function gridNeighbour(
  index: number,
  count: number,
  cols: number,
  dx: number,
  dy: number,
) {
  if (index < 0 || !count) return index;
  const columns = Math.max(1, cols);
  const row = Math.floor(index / columns),
    col = index % columns;
  const nextRow = row + dy,
    nextCol = col + dx;
  if (nextCol < 0 || nextCol >= columns || nextRow < 0) return index;
  const next = nextRow * columns + nextCol;
  return next < count ? next : index;
}
