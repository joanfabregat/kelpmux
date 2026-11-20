import { test } from "node:test";
import assert from "node:assert/strict";
import {
  loadLayout,
  loadView,
  moveBefore,
  orderPanes,
  saveLayout,
  saveView,
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
  });
  // A new server epoch reuses pane IDs for different processes.
  assert.deepEqual(loadLayout("main", "b"), {
    epoch: "b",
    order: [],
    focus: 0,
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
