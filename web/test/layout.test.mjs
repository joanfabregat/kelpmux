import { test } from "node:test";
import assert from "node:assert/strict";
import { gridColumns } from "../src/terminal/layout.ts";
import { themeVariables, loadAppearance, themes } from "../src/appearance.ts";
test("removed themes migrate to black while preserving font and size", () => {
  assert.deepEqual(Object.keys(themes).slice(0, 3), [
    "black",
    "cobalt2",
    "apple",
  ]);
  const original = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  try {
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: {
        getItem: () =>
          JSON.stringify({ theme: "brown", font: "courier", size: 18 }),
      },
    });
    assert.deepEqual(loadAppearance(), {
      theme: "black",
      mode: "dark",
      terminal: "follow",
      font: "courier",
      size: 18,
    });
  } finally {
    if (original) Object.defineProperty(globalThis, "localStorage", original);
    else delete globalThis.localStorage;
  }
});
test("grid favors side-by-side tiles that stay about 80 columns wide", () => {
  assert.equal(gridColumns(0, 1200), 1);
  assert.equal(gridColumns(1, 2400), 1);
  // Two terminals sit side by side unless the grid is very narrow.
  assert.equal(gridColumns(2, 1200), 2);
  assert.equal(gridColumns(2, 800), 2);
  assert.equal(gridColumns(2, 600), 1);
  // More columns while each tile keeps the minimum width.
  assert.equal(gridColumns(3, 1200), 2);
  assert.equal(gridColumns(3, 1700), 3);
  assert.equal(gridColumns(8, 2300), 4);
  assert.equal(gridColumns(8, 1700), 3);
  // Fewer, wider columns when they hold the same number of rows.
  assert.equal(gridColumns(5, 2300), 3);
  assert.equal(gridColumns(4, 1700), 2);
  // A caller-supplied minimum (from the font cell width) is honored.
  assert.equal(gridColumns(4, 1700, 0, 400), 4);
  assert.equal(gridColumns(6, 500), 1);
});
test("with its height the grid fits every terminal at a sensible shape", () => {
  const shape = (count, width, height) => {
    const cols = gridColumns(count, width, height);
    return `${cols}x${Math.ceil(count / cols)}`;
  };
  // A laptop's workspace beside the sidebar.
  assert.equal(shape(1, 1300, 850), "1x1");
  assert.equal(shape(2, 1300, 850), "2x1");
  assert.equal(shape(3, 1300, 850), "2x2");
  assert.equal(shape(4, 1300, 850), "2x2");
  assert.equal(shape(8, 1300, 850), "3x3");
  assert.equal(shape(13, 1300, 850), "4x4");
  // A large screen keeps tiles larger rather than wider.
  assert.equal(shape(8, 2300, 1350), "3x3");
  assert.equal(shape(13, 2300, 1350), "4x4");
  // A very narrow grid stacks, and never leaves a row empty.
  assert.equal(shape(2, 600, 900), "1x2");
  for (let count = 2; count <= 40; count++) {
    const cols = gridColumns(count, 1300, 850);
    const rows = Math.ceil(count / cols);
    assert.ok(cols <= count && cols * (rows - 1) < count, `${count}`);
  }
});
test("theme changes reset all terminal colors, including cursor and ANSI", () => {
  const cobalt = themeVariables("cobalt2");
  assert.equal(cobalt["--terminal-ansi-4"], "#1460d2");
  assert.equal(cobalt["--terminal-cursor"], "#f0cc09");
  assert.equal(themeVariables("apple")["--terminal-ansi-9"], "#ff453a");
  const neutral = themeVariables("black");
  assert.equal(neutral["--terminal-selection-text"], "");
  assert.notEqual(neutral["--terminal-ansi-4"], cobalt["--terminal-ansi-4"]);
});
