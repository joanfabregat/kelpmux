import { test } from "node:test";
import assert from "node:assert/strict";
import { prefixAction } from "../src/prefix.ts";

test("prefix keys follow the TUI's default bindings", () => {
  const key = (key, extra = {}) => prefixAction({ key, ...extra });
  assert.equal(key("Enter"), "new");
  assert.equal(key("z"), "reopen-last");
  // Full width was removed from the browser; f is free.
  assert.equal(key("f"), null);
  assert.equal(key("b"), "notifications");
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

test("keyboard modes follow the TUI keys; the grid has no pages or splits", () => {
  assert.equal(prefixAction({ key: "[" }), "scroll-mode");
  assert.equal(prefixAction({ key: " " }), "select-mode");
  assert.equal(prefixAction({ key: "l" }), "focus-right");
  // The grid is one read-only page: no paging, no resizing of tiles.
  assert.equal(prefixAction({ key: "PageUp" }), null);
  assert.equal(prefixAction({ key: "PageDown" }), null);
  assert.equal(prefixAction({ key: "H", shiftKey: true }), null);
  assert.equal(prefixAction({ key: "J", shiftKey: true }), null);
  assert.equal(prefixAction({ key: "=" }), null);
});
