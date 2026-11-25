import { test } from "node:test";
import assert from "node:assert/strict";
import {
  modeKey,
  modeSelection,
  selectMode,
} from "../src/key-mode.ts";

test("scroll mode scrolls history and leaves to the live screen", () => {
  const scroll = { kind: "scroll" };
  assert.deepEqual(modeKey(scroll, { key: "ArrowUp" }, 80, 24), {
    mode: scroll,
    scroll: -1,
  });
  assert.equal(modeKey(scroll, { key: "PageDown" }, 80, 24).scroll, 23);
  assert.equal(
    modeKey(scroll, { key: "u", ctrlKey: true }, 80, 24).scroll,
    -12,
  );
  assert.equal(modeKey(scroll, { key: "g" }, 80, 24).top, true);
  assert.equal(modeKey(scroll, { key: "G" }, 80, 24).live, true);
  assert.deepEqual(modeKey(scroll, { key: "q" }, 80, 24), {
    mode: null,
    live: true,
  });
  // Other keys stay in the mode and never reach the program.
  assert.deepEqual(modeKey(scroll, { key: "x" }, 80, 24), { mode: scroll });
  assert.equal(modeKey(scroll, { key: " " }, 80, 24).mode.kind, "select");
});

test("select mode moves a cursor, marks, scrolls at edges and copies", () => {
  let mode = selectMode(24);
  assert.deepEqual(mode.cursor, { x: 0, y: 23 });
  assert.deepEqual(modeSelection(mode), {
    start: { x: 0, y: 23 },
    end: { x: 0, y: 23 },
  });
  mode = modeKey(mode, { key: "v" }, 80, 24).mode;
  mode = modeKey(mode, { key: "l" }, 80, 24).mode;
  mode = modeKey(mode, { key: "$" }, 80, 24).mode;
  assert.deepEqual(modeSelection(mode), {
    start: { x: 0, y: 23 },
    end: { x: 79, y: 23 },
  });
  mode = { ...mode, cursor: { x: 5, y: 0 } };
  const up = modeKey(mode, { key: "ArrowUp" }, 80, 24);
  assert.equal(up.scroll, -1);
  assert.deepEqual(up.mode.cursor, { x: 5, y: 0 });
  // The mark follows its line as the view scrolls.
  assert.deepEqual(up.mode.anchor, { x: 0, y: 24 });
  assert.deepEqual(modeKey(mode, { key: "y" }, 80, 24), {
    mode: null,
    copy: true,
  });
  assert.deepEqual(modeKey(mode, { key: "Escape" }, 80, 24), {
    mode: null,
    live: false,
  });
  assert.equal(modeSelection(null), null);
  assert.equal(modeSelection({ kind: "scroll" }), null);
});
