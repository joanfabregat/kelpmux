// The TUI's tab colours (KelpMux src/native/ui.rs PALETTE). Each client
// keeps its own colour per pane (ADR 0002): this browser stores the ones
// picked here in its layout (`layout-store.ts`), and a pane without one
// takes the TUI's default for its ID.

/** Foreground xterm-256 colour of each TUI tab colour. */
const PALETTE = [39, 42, 141, 214, 80, 205, 185, 209, 111];
export const paletteNames = [
  "Blue",
  "Green",
  "Purple",
  "Amber",
  "Teal",
  "Pink",
  "Lime",
  "Coral",
  "Slate",
];

/** An xterm-256 colour index (16–255) as a CSS hex colour. */
export function xterm256(index: number) {
  const hex = (n: number) => n.toString(16).padStart(2, "0");
  if (index >= 232) {
    const level = 8 + (index - 232) * 10;
    return `#${hex(level)}${hex(level)}${hex(level)}`;
  }
  const cube = [0, 95, 135, 175, 215, 255];
  const n = Math.max(16, Math.min(231, index)) - 16;
  return `#${hex(cube[Math.floor(n / 36)])}${hex(cube[Math.floor(n / 6) % 6])}${hex(cube[n % 6])}`;
}

/** The CSS colour of a pane's palette index, or a neutral colour. */
export const paletteColor = (palette?: number) =>
  palette != null && palette >= 0 && palette < PALETTE.length
    ? xterm256(PALETTE[palette])
    : "var(--line-strong)";
