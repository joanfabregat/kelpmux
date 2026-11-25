/** Narrowest tile that still shows about 80 columns of the tile font. */
export const MIN_TILE_WIDTH = 560;
/** Below this tile width two terminals stack instead of sitting side by side. */
export const NARROW_TILE_WIDTH = 360;
/** Width-to-height ratio of a sensible terminal tile (about 80 × 24 cells
 * of the tile font), which the grid keeps its tiles close to. */
export const TILE_ASPECT = 1.6;
/**
 * Columns for the live grid, which shows every running terminal at once.
 * Two columns for two or more terminals unless the grid is very narrow,
 * and no more columns than the resulting number of rows needs (so the last
 * row is not left sparse when fewer, wider columns hold the same rows).
 *
 * With the grid's `height`, the columns are those whose tiles hold the
 * largest box of `TILE_ASPECT` (fewer empty cells on a tie), so tiles keep
 * a sensible shape however many terminals there are. Without it (not yet
 * measured), as many columns as fit while each tile stays at least
 * `minTile` pixels wide.
 */
export function gridColumns(
  count: number,
  width: number,
  height = 0,
  minTile = MIN_TILE_WIDTH,
): number {
  if (count <= 1) return 1;
  const least = width >= 2 * NARROW_TILE_WIDTH ? 2 : 1;
  let cols: number;
  if (height > 0 && width > 0) {
    cols = least;
    let best = -1,
      empty = Infinity;
    for (let c = least; c <= count; c++) {
      const rows = Math.ceil(count / c);
      const fit = Math.min(width / c / TILE_ASPECT, height / rows);
      const spare = c * rows - count;
      // A box within a hundredth counts as equal, preferring fuller rows.
      if (fit > best * 1.01 || (fit >= best * 0.99 && spare < empty)) {
        if (fit > best) best = fit;
        cols = c;
        empty = spare;
      }
    }
  } else {
    cols = Math.min(
      count,
      Math.max(1, Math.floor(width / Math.max(1, minTile))),
    );
    if (cols < least) cols = least;
  }
  return Math.ceil(count / Math.ceil(count / cols));
}
