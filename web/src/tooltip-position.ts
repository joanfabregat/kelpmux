// Tooltip placement, kept free of DOM and CSS imports so it can be unit tested.

const MARGIN = 8;
const GAP = 6;

/** Places `bubble` below `anchor` (above when there is no room), inside the
 * viewport. */
export function placeTooltip(
  anchor: { top: number; bottom: number; left: number; width: number },
  bubble: { width: number; height: number },
  viewport: { width: number; height: number },
) {
  const below = anchor.bottom + GAP;
  const above = anchor.top - GAP - bubble.height;
  const top =
    below + bubble.height <= viewport.height - MARGIN || above < MARGIN
      ? Math.max(
          MARGIN,
          Math.min(below, viewport.height - MARGIN - bubble.height),
        )
      : above;
  const centered = anchor.left + anchor.width / 2 - bubble.width / 2;
  const left = Math.max(
    MARGIN,
    Math.min(centered, viewport.width - MARGIN - bubble.width),
  );
  return { top, left, placement: top < anchor.top ? "above" : "below" };
}
