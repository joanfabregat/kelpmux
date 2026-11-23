import { test } from "node:test";
import assert from "node:assert/strict";
import {
  loadLayout,
  loadView,
  moveBefore,
  numberPanes,
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
    parked: [],
  });
  // A new server epoch reuses pane IDs for different processes.
  assert.deepEqual(loadLayout("main", "b"), {
    epoch: "b",
    order: [],
    focus: 0,
    parked: [],
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

test("layouts stored with tab colours still load, without them", () => {
  globalThis.localStorage = memoryStorage();
  // Tab colours were dropped from the web client; older layouts keep a
  // `colors` key, which loading ignores.
  globalThis.localStorage.setItem(
    "kelpmux-web-layout:main",
    JSON.stringify({
      epoch: "a",
      order: [2, 1],
      focus: 2,
      parked: [1],
      colors: { 1: 3, 2: 5 },
    }),
  );
  assert.deepEqual(loadLayout("main", "a"), {
    epoch: "a",
    order: [2, 1],
    focus: 2,
    parked: [1],
  });
  delete globalThis.localStorage;
});

test("numbers follow this browser's order, as the server's creation order did", () => {
  // KelpServer 0.3.0-beta.2 sends no number: without a stored order the
  // numbers are the creation order, after a move they follow the move.
  const panes = [4, 7, 9].map((id) => ({ id, title: `t${id}` }));
  assert.deepEqual(
    numberPanes(orderPanes(panes, [])).map((p) => [p.id, p.number]),
    [
      [4, 1],
      [7, 2],
      [9, 3],
    ],
  );
  const moved = numberPanes(orderPanes(panes, [9, 4, 7]));
  assert.deepEqual(
    moved.map((p) => [p.id, p.number, p.title]),
    [
      [9, 1, "t9"],
      [4, 2, "t4"],
      [7, 3, "t7"],
    ],
  );
  assert.equal("number" in panes[0], false);
});
