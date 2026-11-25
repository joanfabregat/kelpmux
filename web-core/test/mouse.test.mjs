import test from "node:test";
import assert from "node:assert/strict";
import { mouseBytes, mouseButton } from "../src/mouse.ts";
const screen = { cols: 240, rows: 80, modes: {} };
const event = {
  position: { x: 9, y: 4 },
  kind: "Press",
  button: 0,
  shift: false,
  alt: false,
  ctrl: false,
};
test("ncurses legacy clicks, releases and wheel remain binary bytes", () => {
  assert.deepEqual(mouseBytes(screen, event), [27, 91, 77, 32, 42, 37]);
  assert.deepEqual(
    mouseBytes(screen, { ...event, kind: "Release" }),
    [27, 91, 77, 35, 42, 37],
  );
  assert.deepEqual(
    mouseBytes(screen, {
      ...event,
      kind: "WheelDown",
      position: { x: 190, y: 4 },
    }),
    [27, 91, 77, 97, 223, 37],
  );
  assert.deepEqual(
    mouseBytes(screen, { ...event, position: { x: 223, y: 4 } }),
    [],
  );
});
test("SGR release uses the original button and motion uses the held button", () => {
  const sgr = { ...screen, modes: { mouse_sgr: true } };
  assert.equal(
    new TextDecoder().decode(
      Uint8Array.from(mouseBytes(sgr, { ...event, kind: "Release" })),
    ),
    "\x1b[<0;10;5m",
  );
  assert.equal(mouseButton(-1, 1, "Move"), 0);
  assert.equal(mouseButton(-1, 4, "Move"), 1);
  assert.equal(mouseButton(-1, 2, "Move"), 2);
  assert.equal(mouseButton(-1, 0, "Move"), 3);
});
