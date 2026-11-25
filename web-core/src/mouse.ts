import type { Screen } from "./types";
export type MousePosition = { x: number; y: number };
export function mouseButton(
  button: number,
  buttons: number,
  kind: string,
): number {
  if (kind !== "Move") return Math.max(0, Math.min(2, button));
  return buttons & 1 ? 0 : buttons & 4 ? 1 : buttons & 2 ? 2 : 3;
}
export function mouseBytes(
  screen: Screen,
  event: {
    position: MousePosition;
    kind: string;
    button: number;
    shift: boolean;
    alt: boolean;
    ctrl: boolean;
  },
): number[] {
  const { position, kind } = event;
  if (
    position.x < 0 ||
    position.y < 0 ||
    position.x >= screen.cols ||
    position.y >= screen.rows
  )
    return [];
  let button =
    kind === "WheelUp" ? 64 : kind === "WheelDown" ? 65 : event.button;
  if (kind === "Move") button += 32;
  button += (event.shift ? 4 : 0) + (event.alt ? 8 : 0) + (event.ctrl ? 16 : 0);
  const x = position.x + 1,
    y = position.y + 1;
  if (screen.modes.mouse_sgr)
    return Array.from(
      new TextEncoder().encode(
        `\x1b[<${button};${x};${y}${kind === "Release" ? "m" : "M"}`,
      ),
    );
  if (x > 223 || y > 223) return [];
  if (kind === "Release") button = 3;
  return [27, 91, 77, button + 32, x + 32, y + 32];
}
