import { test } from "node:test";
import assert from "node:assert/strict";
import {
  NOTIFICATION_EVENTS,
  TOAST_LIMIT,
  addToasts,
  alerting,
  bellRinging,
  defaultNotificationEvents,
  eventOf,
  notificationEvents,
  pruneToasts,
  summarized,
  summaryText,
} from "../src/toasts.ts";

const pane = (id, extra = {}) => ({
  id,
  pid: 100 + id,
  status: "idle",
  dead: false,
  kind: "claude",
  ...extra,
});
const panes = [1, 2, 3, 4, 5].map((id) => pane(id));
let at = 1000;
const note = (id, paneId, status, extra = {}) => ({
  id,
  pane: paneId,
  title: status === "unknown" ? `Bell · agent ${paneId}` : `${status} · ${id}`,
  status,
  at: (at += 10),
  read: false,
  workspace: "main",
  origin: { run: "r", pid: 100 + paneId },
  ...extra,
});
const on = defaultNotificationEvents;

test("event kinds and their stored preferences follow the TUI", () => {
  assert.deepEqual(NOTIFICATION_EVENTS, [
    "question",
    "finished",
    "failed",
    "exited",
    "bell",
  ]);
  assert.equal(eventOf("input_needed"), "question");
  assert.equal(eventOf("turn_finished"), "finished");
  assert.equal(eventOf("failed"), "failed");
  assert.equal(eventOf("exited"), "exited");
  assert.equal(eventOf("unknown"), "bell");
  assert.ok(
    Object.values(on).every((v) => v === true),
    "all on by default",
  );
  // Missing, invalid and older values: on; only an explicit false is off.
  assert.deepEqual(notificationEvents(undefined), on);
  assert.deepEqual(notificationEvents("bell"), on);
  assert.deepEqual(notificationEvents({ bell: false, failed: "no" }), {
    ...on,
    bell: false,
  });
});

test("one toast per terminal, newest first, updated by a new event", () => {
  const first = [note(1, 2, "turn_finished")];
  let toasts = addToasts([], first, first, panes, on, 0);
  assert.equal(toasts.length, 1);
  assert.equal(toasts[0].pane, 2);
  assert.equal(toasts[0].event, "finished");
  const second = [...first, note(2, 3, "input_needed")];
  toasts = addToasts(toasts, [second[1]], second, panes, on, 0);
  assert.deepEqual(
    toasts.map((t) => t.pane),
    [3, 2],
  );
  // Terminal 2 again: its toast updates and moves up; no second toast.
  const third = [...second, note(3, 2, "failed")];
  toasts = addToasts(toasts, [third[2]], third, panes, on, 0);
  assert.deepEqual(
    toasts.map((t) => t.pane),
    [2, 3],
  );
  assert.equal(toasts[0].event, "failed");
  assert.deepEqual(toasts[0].ids, [1, 3]);
  assert.equal(toasts[0].title, "failed · 3");
});

test("a bell with the event it accompanies makes one toast", () => {
  const typed = note(1, 2, "input_needed");
  const bell = { ...note(2, 2, "unknown"), at: typed.at + 1 };
  const list = [typed, bell];
  const toasts = addToasts([], list, list, panes, on, 0);
  assert.equal(toasts.length, 1);
  // The bell shows as the question it accompanied.
  assert.equal(toasts[0].event, "question");
  assert.equal(toasts[0].title, typed.title);
  assert.deepEqual(toasts[0].ids, [1, 2]);
  // A lone bell is a bell.
  const lone = [note(3, 4, "unknown")];
  assert.equal(addToasts([], lone, lone, panes, on, 0)[0].event, "bell");
});

test("no toast for the shown terminal, a disabled event or a gone pane", () => {
  const list = [note(1, 2, "turn_finished"), note(2, 3, "unknown")];
  assert.deepEqual(
    addToasts([], list, list, panes, on, 2).map((t) => t.pane),
    [3],
  );
  assert.deepEqual(
    addToasts([], list, list, panes, { ...on, bell: false }, 0).map(
      (t) => t.pane,
    ),
    [2],
  );
  // Another process in the same pane, a dead pane, a read notification.
  const other = [note(3, 4, "failed", { origin: { run: "r", pid: 1 } })];
  assert.deepEqual(addToasts([], other, other, panes, on, 0), []);
  const dead = [note(4, 5, "failed")];
  const deadPanes = panes.map((p) => (p.id === 5 ? { ...p, dead: true } : p));
  assert.deepEqual(addToasts([], dead, dead, deadPanes, on, 0), []);
  const read = [note(5, 1, "failed", { read: true })];
  assert.deepEqual(addToasts([], read, read, panes, on, 0), []);
  // The sound follows the same events.
  assert.equal(alerting(list, list, { ...on, finished: false }).length, 1);
});

test("toasts go when read anywhere, the pane ends or the event is off", () => {
  const list = [
    note(1, 1, "turn_finished"),
    note(2, 2, "input_needed"),
    note(3, 3, "unknown"),
  ];
  const toasts = addToasts([], list, list, panes, on, 0);
  assert.equal(pruneToasts(toasts, list, panes, on), toasts, "unchanged");
  // Read by another window, device or the TUI: the server says so.
  const readElsewhere = list.map((n) =>
    n.id === 2 ? { ...n, read: true } : n,
  );
  assert.deepEqual(
    pruneToasts(toasts, readElsewhere, panes, on).map((t) => t.pane),
    [3, 1],
  );
  // Read here, before the server echoes it.
  assert.deepEqual(
    pruneToasts(toasts, list, panes, on, new Set([1])).map((t) => t.pane),
    [3, 2],
  );
  // Gone from the retained list, the pane closed or another process.
  assert.equal(pruneToasts(toasts, [], panes, on).length, 0);
  assert.deepEqual(
    pruneToasts(
      toasts,
      list,
      panes.filter((p) => p.id !== 3),
      on,
    ).map((t) => t.pane),
    [2, 1],
  );
  assert.deepEqual(
    pruneToasts(
      toasts,
      list,
      panes.map((p) => (p.id === 1 ? { ...p, pid: 999 } : p)),
      on,
    ).map((t) => t.pane),
    [3, 2],
  );
  assert.deepEqual(
    pruneToasts(toasts, list, panes, { ...on, bell: false }).map((t) => t.pane),
    [2, 1],
  );
});

test("more than three toasts collapse into a summary", () => {
  assert.equal(TOAST_LIMIT, 3);
  const list = [1, 2, 3, 4].map((p) => note(p, p, "input_needed"));
  const three = addToasts([], list.slice(0, 3), list, panes, on, 0);
  assert.equal(summarized(three), false);
  const four = addToasts(three, [list[3]], list, panes, on, 0);
  assert.equal(summarized(four), true);
  assert.equal(summaryText(4), "4 terminals need attention");
  assert.equal(summaryText(1), "1 terminal needs attention");
});

test("the favicon bell rings until the bell is read, if bells alert", () => {
  const bell = note(1, 2, "unknown");
  assert.equal(bellRinging([bell], panes, on), true);
  assert.equal(bellRinging([{ ...bell, read: true }], panes, on), false);
  assert.equal(bellRinging([bell], panes, { ...on, bell: false }), false);
  assert.equal(bellRinging([note(2, 2, "input_needed")], panes, on), false);
  assert.equal(
    bellRinging([{ ...note(3, 2, "input_needed"), bell: true }], panes, on),
    true,
  );
  assert.equal(
    bellRinging(
      [bell],
      panes.filter((p) => p.id !== 2),
      on,
    ),
    false,
  );
});
