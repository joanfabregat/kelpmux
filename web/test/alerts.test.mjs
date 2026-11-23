import { test } from "node:test";
import assert from "node:assert/strict";
import { freshNotifications, newestId } from "../src/alerts.ts";
import {
  SIDEBAR_MAX,
  SIDEBAR_MIN,
  defaultPreferences,
  loadLayout,
  loadPreferences,
  moveBy,
  saveLayout,
  savePreferences,
} from "../src/layout-store.ts";

const n = (id, read = false) => ({ id, read });

test("only unread notifications newer than the last seen one alert", () => {
  const list = [n(5), n(3), n(4, true), n(7)];
  assert.deepEqual(
    freshNotifications(list, 3).map((x) => x.id),
    [5, 7],
  );
  assert.deepEqual(freshNotifications(list, 7), []);
  assert.equal(newestId(list), 7);
  assert.equal(newestId([], 9), 9);
});

function memoryStorage() {
  const values = new Map();
  return {
    getItem: (key) => (values.has(key) ? values.get(key) : null),
    setItem: (key, value) => values.set(key, String(value)),
  };
}

test("parked terminals and preferences are this browser's", () => {
  globalThis.localStorage = memoryStorage();
  saveLayout("main", {
    epoch: "a",
    order: [1, 2],
    focus: 1,
    parked: [2, -1, "x"],
  });
  assert.deepEqual(loadLayout("main", "a").parked, [2]);
  assert.deepEqual(loadLayout("main", "b").parked, []);
  assert.deepEqual(loadPreferences(), defaultPreferences);
  savePreferences({
    sidebarWidth: 9999,
    sidebarFolded: true,
    sound: true,
    desktop: "yes",
    notificationEvents: { bell: false, finished: "no", question: 0 },
    hardwareKeyboard: "sometimes",
  });
  assert.deepEqual(loadPreferences(), {
    sidebarWidth: SIDEBAR_MAX,
    sidebarFolded: true,
    sound: true,
    notificationEvents: {
      question: true,
      finished: true,
      failed: true,
      exited: true,
      bell: false,
    },
    hardwareKeyboard: "auto",
  });
  savePreferences({ ...defaultPreferences, hardwareKeyboard: "on" });
  assert.equal(loadPreferences().hardwareKeyboard, "on");
  globalThis.localStorage.setItem(
    "kelpmux-web-preferences",
    '{"sidebarWidth":10}',
  );
  assert.equal(loadPreferences().sidebarWidth, SIDEBAR_MIN);
  globalThis.localStorage.setItem("kelpmux-web-preferences", "{bad");
  assert.deepEqual(loadPreferences(), defaultPreferences);
  delete globalThis.localStorage;
});

test("move up and down swaps with the neighbour", () => {
  assert.deepEqual(moveBy([1, 2, 3], 2, -1), [2, 1, 3]);
  assert.deepEqual(moveBy([1, 2, 3], 2, 1), [1, 3, 2]);
  assert.deepEqual(moveBy([1, 2, 3], 1, -1), [1, 2, 3]);
  assert.deepEqual(moveBy([1, 2, 3], 3, 1), [1, 2, 3]);
  assert.deepEqual(moveBy([1, 2, 3], 9, 1), [1, 2, 3]);
});

test("the old desktop notification preference migrates cleanly", () => {
  globalThis.localStorage = memoryStorage();
  // Stored by the build with desktop notifications: kept settings load, the
  // desktop choice is dropped and every event alerts.
  globalThis.localStorage.setItem(
    "kelpmux-web-preferences",
    '{"sidebarWidth":300,"sidebarFolded":false,"sound":true,"desktop":true,"hardwareKeyboard":"auto"}',
  );
  const loaded = loadPreferences();
  assert.equal(loaded.sound, true);
  assert.equal(loaded.sidebarWidth, 300);
  assert.equal("desktop" in loaded, false);
  assert.deepEqual(
    loaded.notificationEvents,
    defaultPreferences.notificationEvents,
  );
  savePreferences(loaded);
  assert.equal(
    globalThis.localStorage
      .getItem("kelpmux-web-preferences")
      .includes("desktop"),
    false,
  );
  globalThis.localStorage.setItem(
    "kelpmux-web-preferences",
    '{"notificationEvents":"all"}',
  );
  assert.deepEqual(
    loadPreferences().notificationEvents,
    defaultPreferences.notificationEvents,
  );
  delete globalThis.localStorage;
});
