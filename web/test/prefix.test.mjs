import { test } from "node:test";
import assert from "node:assert/strict";
import { gridNeighbour, prefixAction } from "../src/prefix.ts";

test("prefix keys follow the TUI's default bindings", () => {
  const key = (key, extra = {}) => prefixAction({ key, ...extra });
  assert.equal(key("Enter"), "new");
  assert.equal(key("z"), "reopen-last");
  assert.equal(key("f"), "full-width");
  assert.equal(key("e"), "grid");
  assert.equal(key("Tab"), "next");
  assert.equal(key("Tab", { shiftKey: true }), "previous");
  assert.deepEqual(key("3"), { select: 3 });
  assert.equal(key("0"), null);
  assert.equal(key("N", { shiftKey: true }), "next-attention");
  assert.equal(key("P", { shiftKey: true }), "next-finished");
  assert.equal(key("n"), "agents-finished");
  assert.equal(key("!"), "agents-attention");
  assert.equal(key("p"), "park");
  assert.equal(key("u"), "parked");
  assert.equal(key("c"), null);
  assert.equal(key("/"), "search");
  assert.equal(key("ArrowLeft"), "focus-left");
  assert.equal(key("b", { ctrlKey: true }), "send-prefix");
  assert.equal(key("B", { ctrlKey: true }), "send-prefix");
  assert.equal(key("x", { ctrlKey: true }), null);
  assert.equal(key("q"), null);
  assert.equal(key("constructor"), null);
});

test("grid focus moves to neighbours and stops at edges", () => {
  // 5 tiles in 3 columns: 0 1 2 / 3 4
  assert.equal(gridNeighbour(0, 5, 3, 1, 0), 1);
  assert.equal(gridNeighbour(2, 5, 3, 1, 0), 2);
  assert.equal(gridNeighbour(0, 5, 3, -1, 0), 0);
  assert.equal(gridNeighbour(1, 5, 3, 0, 1), 4);
  assert.equal(gridNeighbour(2, 5, 3, 0, 1), 2);
  assert.equal(gridNeighbour(4, 5, 3, 0, -1), 1);
  assert.equal(gridNeighbour(-1, 5, 3, 1, 0), -1);
  assert.equal(gridNeighbour(0, 0, 3, 1, 0), 0);
});

test("keyboard modes and grid resizing follow the TUI keys", () => {
  assert.equal(prefixAction({ key: "[" }), "scroll-mode");
  assert.equal(prefixAction({ key: " " }), "select-mode");
  assert.equal(prefixAction({ key: "H", shiftKey: true }), "grid-resize-left");
  assert.equal(prefixAction({ key: "J", shiftKey: true }), "grid-resize-down");
  assert.equal(prefixAction({ key: "=" }), "grid-equalize");
});
