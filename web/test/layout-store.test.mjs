import { test } from "node:test";
import assert from "node:assert/strict";
import {
  liveColors,
  loadLayout,
  loadView,
  moveBefore,
  orderPanes,
  saveLayout,
  saveView,
  tabColor,
} from "../src/layout-store.ts";

function memoryStorage() {
  const values = new Map();
  return {
    getItem: (key) => (values.has(key) ? values.get(key) : null),
    setItem: (key, value) => values.set(key, String(value)),
    removeItem: (key) => values.delete(key),
  };
}

test("this browser's order keeps known panes and appends new ones", () => {
  const panes = [1, 2, 3, 4].map((id) => ({ id }));
  assert.deepEqual(
    orderPanes(panes, [3, 9, 1]).map((p) => p.id),
    [3, 1, 2, 4],
  );
  assert.deepEqual(moveBefore([1, 2, 3], 3, 1), [3, 1, 2]);
  assert.deepEqual(moveBefore([1, 2, 3], 1, null), [2, 3, 1]);
  assert.deepEqual(moveBefore([1, 2, 3], 2, 2), [1, 2, 3]);
  assert.deepEqual(moveBefore([1, 2, 3], 7, 1), [1, 2, 3]);
});

test("layout is per workspace and epoch and survives missing storage", () => {
  globalThis.localStorage = memoryStorage();
  assert.equal(loadView(), "grid");
  saveView("single");
  assert.equal(loadView(), "single");
  saveLayout("main", { epoch: "a", order: [2, 1], focus: 2 });
  assert.deepEqual(loadLayout("main", "a"), {
    epoch: "a",
    order: [2, 1],
    focus: 2,
    parked: [],
    colors: {},
  });
  // A new server epoch reuses pane IDs for different processes.
  assert.deepEqual(loadLayout("main", "b"), {
    epoch: "b",
    order: [],
    focus: 0,
    parked: [],
    colors: {},
  });
  assert.deepEqual(loadLayout("dev", "a").order, []);
  globalThis.localStorage.setItem("kelpmux-web-layout:main", "{not json");
  assert.deepEqual(loadLayout("main", "a").order, []);
  globalThis.localStorage = {
    getItem() {
      throw Error("blocked");
    },
    setItem() {
      throw Error("blocked");
    },
  };
  assert.equal(loadView(), "grid");
  saveView("single");
  saveLayout("main", { epoch: "a", order: [1], focus: 1 });
  assert.deepEqual(loadLayout("main", "a").order, []);
  delete globalThis.localStorage;
});

test("tab colours are this browser's, per epoch, with the TUI's defaults", () => {
  globalThis.localStorage = memoryStorage();
  saveLayout("main", {
    epoch: "a",
    order: [1, 2],
    focus: 1,
    parked: [],
    colors: { 2: 5 },
  });
  assert.deepEqual(loadLayout("main", "a").colors, { 2: 5 });
  // Pane IDs of another epoch are different processes.
  assert.deepEqual(loadLayout("main", "b").colors, {});
  // Invalid stored entries are dropped.
  globalThis.localStorage.setItem(
    "kelpmux-web-layout:main",
    JSON.stringify({
      epoch: "a",
      order: [],
      colors: { 1: 9, 2: -1, x: 3, 3: 1.5, 4: 8, 0: 2 },
    }),
  );
  assert.deepEqual(loadLayout("main", "a").colors, { 4: 8 });
  // Unpicked panes take the TUI's default, (id - 1) % 9.
  assert.equal(tabColor({}, 1), 0);
  assert.equal(tabColor({}, 10), 0);
  assert.equal(tabColor({}, 12), 2);
  assert.equal(tabColor({ 12: 7 }, 12), 7);
  // Closed panes leave the map; an unchanged map is kept as is.
  const colors = { 1: 3, 2: 4 };
  assert.equal(liveColors(colors, new Set([1, 2, 3])), colors);
  assert.deepEqual(liveColors(colors, new Set([2])), { 2: 4 });
  delete globalThis.localStorage;
});
