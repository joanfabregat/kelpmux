// Layout selection for phones, tablets and desktops. Pure, so node tests can
// check it; main.tsx feeds it the window size and the primary pointer.

/** desktop: sidebar and grid with a mouse; tablet: the same layout adapted
 * for touch; phone: a terminal list and one terminal at a time. */
export type LayoutMode = "desktop" | "tablet" | "phone";
export type Orientation = "portrait" | "landscape";
export type Viewport = {
  width: number;
  height: number;
  /** The primary pointer is coarse (a finger): `(pointer: coarse)`. */
  coarse: boolean;
};

/** Touch screens whose short side is below this (CSS pixels) are phones.
 * Every iPhone is below 500 in either orientation; the smallest iPad
 * (mini, 744 × 1133) is above it. */
export const PHONE_SHORT_SIDE = 600;
/** Without touch, a window narrower than this gets the phone layout. */
export const NARROW_WIDTH = 600;

export function layoutMode({ width, height, coarse }: Viewport): LayoutMode {
  if (coarse)
    return Math.min(width, height) < PHONE_SHORT_SIDE ? "phone" : "tablet";
  return width < NARROW_WIDTH ? "phone" : "desktop";
}

export function orientation({
  width,
  height,
}: Pick<Viewport, "width" | "height">): Orientation {
  return height > width ? "portrait" : "landscape";
}

/** Whether the sidebar is folded. A tablet in portrait starts folded each
 * time it turns to portrait (`portraitFolded` is that transient state, reset
 * to true on the turn); everywhere else the saved preference applies. The
 * phone has no side-by-side sidebar, so its list is never folded. */
export function sidebarFolded(
  mode: LayoutMode,
  turn: Orientation,
  saved: boolean,
  portraitFolded: boolean,
): boolean {
  if (mode === "phone") return false;
  if (mode === "tablet" && turn === "portrait") return portraitFolded;
  return saved;
}

/** Height (CSS pixels) of what covers the bottom of the layout viewport,
 * normally the on-screen keyboard, from `window.innerHeight` and the
 * `visualViewport`. */
export function keyboardInset(
  layoutHeight: number,
  visual: { height: number; offsetTop: number } | null | undefined,
): number {
  if (!visual) return 0;
  return Math.max(
    0,
    Math.round(layoutHeight - visual.height - visual.offsetTop),
  );
}
