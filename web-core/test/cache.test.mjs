import { test } from "node:test";
import assert from "node:assert/strict";
import { trimScreens } from "../src/screen-cache.ts";
const screen = (id, n = 1) => ({
  pane: id,
  revision: 1,
  screen: { cells: Array(n).fill({ text: " " }) },
});
test("tab frames survive subscription changes but closed panes are removed", () => {
  const cache = new Map([
    [1, screen(1)],
    [2, screen(2)],
    [3, screen(3)],
  ]);
  trimScreens(cache, [{ id: 1 }, { id: 2 }], [2]);
  assert.ok(cache.has(1));
  assert.ok(cache.has(2));
  assert.equal(cache.has(3), false);
});
test("cached frames have count and cell budgets without evicting subscribed panes", () => {
  const cache = new Map(
    Array.from({ length: 12 }, (_, n) => [n + 1, screen(n + 1, 30000)]),
  );
  trimScreens(
    cache,
    Array.from({ length: 12 }, (_, n) => ({ id: n + 1 })),
    [1, 12],
  );
  assert.ok(cache.has(1));
  assert.ok(cache.has(12));
  assert.ok(cache.size <= 8);
  assert.ok(
    [...cache.values()].reduce(
      (sum, value) => sum + value.screen.cells.length,
      0,
    ) <= 200000,
  );
});
