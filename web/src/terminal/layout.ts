/** Narrowest tile that still shows about 80 columns of the tile font. */
export const MIN_TILE_WIDTH = 560;
/** Below this tile width two terminals stack instead of sitting side by side. */
export const NARROW_TILE_WIDTH = 360;
/**
 * Columns for the live grid, favoring side-by-side tiles: as many columns
 * as fit while each tile stays at least `minTile` pixels wide, two columns
 * for two or more terminals unless the grid is very narrow, and no more
 * columns than the resulting number of rows needs (so the last row is not
 * left sparse when fewer, wider columns hold the same rows).
 */
export function gridColumns(
  count: number,
  width: number,
  minTile = MIN_TILE_WIDTH,
): number {
  if (count <= 1) return 1;
  let cols = Math.min(
    count,
    Math.max(1, Math.floor(width / Math.max(1, minTile))),
  );
  if (cols < 2 && width >= 2 * NARROW_TILE_WIDTH) cols = 2;
  return Math.ceil(count / Math.ceil(count / cols));
}
