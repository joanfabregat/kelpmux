// Keyboard scroll and selection modes, the browser's version of the TUI's
// `Ctrl-B [` (scroll) and `Ctrl-B Space` (select). Pure, so node tests can
// check the key map; the Terminal component applies the result. Nothing here
// imports application code.

export type Point = { x: number; y: number };
/** No mode: keys go to the terminal. */
export type KeyMode =
  { kind: "scroll" } | { kind: "select"; cursor: Point; anchor: Point | null };

export type ModeResult = {
  /** The mode after this key; null leaves the mode. */
  mode: KeyMode | null;
  /** History rows to move: negative is older (up), positive newer (down). */
  scroll?: number;
  /** Jump to the oldest history line. */
  top?: boolean;
  /** Return to the live screen. */
  live?: boolean;
  /** Copy the selection, then leave the mode. */
  copy?: boolean;
};

/** The selection a mode shows: the cursor cell, or anchor to cursor. */
export function modeSelection(mode: KeyMode | null) {
  if (!mode || mode.kind !== "select") return null;
  return { start: mode.anchor || mode.cursor, end: mode.cursor };
}

/** Starts select mode with the cursor at the bottom-left cell. */
export const selectMode = (rows: number): KeyMode => ({
  kind: "select",
  cursor: { x: 0, y: Math.max(0, rows - 1) },
  anchor: null,
});

/** Applies one key in a mode. Unknown keys keep the mode unchanged. */
export function modeKey(
  mode: KeyMode,
  event: { key: string; ctrlKey?: boolean; shiftKey?: boolean },
  cols: number,
  rows: number,
): ModeResult {
  const { key } = event;
  const page = Math.max(1, rows - 1),
    half = Math.max(1, Math.floor(rows / 2));
  if (key === "Escape" || key === "q" || (event.ctrlKey && key === "c"))
    return { mode: null, live: mode.kind === "scroll" };
  // Scrolling keys work in both modes.
  const scrolls: Record<string, number> = {
    PageUp: -page,
    PageDown: page,
  };
  if (event.ctrlKey && (key === "u" || key === "d"))
    return { mode, scroll: key === "u" ? -half : half };
  if (event.ctrlKey && (key === "b" || key === "f"))
    return { mode, scroll: key === "b" ? -page : page };
  if (key in scrolls) return { mode, scroll: scrolls[key] };
  if (key === "g" || key === "Home") return { mode, top: true };
  if (key === "G" || key === "End") return { mode, live: true };
  if (mode.kind === "scroll") {
    if (key === "ArrowUp" || key === "k") return { mode, scroll: -1 };
    if (key === "ArrowDown" || key === "j") return { mode, scroll: 1 };
    if (key === " " || key === "v") return { mode: selectMode(rows) };
    return { mode };
  }
  const { cursor, anchor } = mode;
  const at = (x: number, y: number): ModeResult => {
    const next = {
      ...mode,
      cursor: {
        x: Math.max(0, Math.min(cols - 1, x)),
        y: Math.max(0, Math.min(rows - 1, y)),
      },
    };
    // Moving past the top or bottom edge scrolls the view instead.
    const overflow = y < 0 ? y : y > rows - 1 ? y - (rows - 1) : 0;
    if (!overflow) return { mode: next };
    return {
      mode: {
        ...next,
        anchor: anchor && { x: anchor.x, y: anchor.y - overflow },
      },
      scroll: overflow,
    };
  };
  switch (key) {
    case "ArrowLeft":
    case "h":
      return at(cursor.x - 1, cursor.y);
    case "ArrowRight":
    case "l":
      return at(cursor.x + 1, cursor.y);
    case "ArrowUp":
    case "k":
      return at(cursor.x, cursor.y - 1);
    case "ArrowDown":
    case "j":
      return at(cursor.x, cursor.y + 1);
    case "0":
      return at(0, cursor.y);
    case "$":
      return at(cols - 1, cursor.y);
    case "w":
      return at(cursor.x + 8, cursor.y);
    case "b":
      return at(cursor.x - 8, cursor.y);
    case " ":
    case "v":
      return { mode: { ...mode, anchor: anchor ? null : { ...cursor } } };
    case "y":
    case "Enter":
      return { mode: null, copy: true };
    default:
      return { mode };
  }
}
