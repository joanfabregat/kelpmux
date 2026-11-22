import { test } from "node:test";
import assert from "node:assert/strict";
import {
  nextPane,
  notificationMatches,
  notificationStatus,
  paneNotifications,
  paneState,
  readingPane,
  rollup,
  rollupSummary,
  tabTitle,
  unreadFor,
} from "../src/state/model.ts";

const pane = (id, status = "idle", extra = {}) => ({
  id,
  pid: 100 + id,
  status,
  dead: false,
  ...extra,
});
const note = (id, paneId, status, extra = {}) => ({
  id,
  pane: paneId,
  title: `${status} ${id}`,
  status,
  at: 1000 + id,
  read: false,
  workspace: "main",
  origin: { run: "r", pid: 100 + paneId },
  ...extra,
});

test("notifications belong to the live process of their pane", () => {
  const p = pane(1);
  assert.equal(notificationMatches(note(1, 1, "turn_finished"), p), true);
  // A recycled pane runs a different process.
  assert.equal(
    notificationMatches(
      note(2, 1, "failed", { origin: { run: "r", pid: 9 } }),
      p,
    ),
    false,
  );
  assert.equal(notificationMatches(note(3, 2, "failed"), p), false);
  assert.equal(
    notificationMatches(note(4, 1, "failed"), { ...p, dead: true }),
    false,
  );
  assert.equal(
    notificationMatches(note(5, 1, "failed", { origin: null }), p),
    false,
  );
});

test("pane state follows the TUI indicator", () => {
  assert.equal(paneState(pane(1, "input_needed")), "input_needed");
  assert.equal(paneState(pane(1, "turn_finished")), "finished");
  assert.equal(paneState(pane(1, "working", { kind: "claude" })), "working");
  // A shell running a command is not an agent at work.
  assert.equal(paneState(pane(1, "working")), "running");
  assert.equal(paneState(pane(1, "failed", { dead: true })), "idle");
  // An idle pane shows its newest unread typed notification, before bells.
  const list = [
    note(1, 1, "turn_finished"),
    note(2, 1, "unknown", { title: "Bell · x" }),
  ];
  assert.equal(paneState(pane(1), list), "finished");
  assert.equal(paneState(pane(1), [list[1]]), "input_needed");
  assert.equal(paneState(pane(1), [{ ...list[0], read: true }]), "idle");
  assert.deepEqual(
    unreadFor(list, pane(1)).map((n) => n.id),
    [2, 1],
  );
});

test("a bell takes the status of the typed notification it accompanied", () => {
  const typed = note(1, 1, "input_needed");
  const bell = note(2, 1, "unknown", {
    title: "Bell · claude",
    at: typed.at + 1,
  });
  assert.equal(notificationStatus([typed, bell], bell), "input_needed");
  assert.equal(
    notificationStatus([typed, { ...bell, at: typed.at + 10 }], {
      ...bell,
      at: typed.at + 10,
    }),
    "unknown",
  );
  assert.equal(notificationStatus([typed], typed), "input_needed");
});

test("roll-ups count states needing attention in urgency order", () => {
  const panes = [
    pane(1, "turn_finished", { kind: "codex" }),
    pane(2, "input_needed", { kind: "claude" }),
    pane(3, "working", { kind: "claude" }),
    pane(4, "working"),
    pane(5),
  ];
  const r = rollup(panes, []);
  assert.equal(r.state, "input_needed");
  assert.deepEqual(r.counts, { finished: 1, input_needed: 1, working: 1 });
  assert.equal(r.total, 3);
  assert.equal(rollupSummary(r), "1 needs input, 1 finished, 1 working");
  assert.equal(rollupSummary(rollup([], [])), "Nothing needs attention");
  assert.equal(tabTitle(0), "KelpMux");
  assert.equal(tabTitle(3), "(3) KelpMux");
  assert.equal(tabTitle(120), "(99+) KelpMux");
});

test("next question / completion cycles from the active pane", () => {
  const panes = [
    pane(1, "input_needed"),
    pane(2),
    pane(3, "input_needed"),
    pane(4, "failed"),
  ];
  const attention = (p) => p.status === "input_needed" || p.status === "failed";
  assert.equal(nextPane(panes, 1, attention)?.id, 3);
  assert.equal(nextPane(panes, 3, attention)?.id, 4);
  assert.equal(nextPane(panes, 4, attention)?.id, 1);
  // Unknown active pane: start from the first.
  assert.equal(nextPane(panes, 99, attention)?.id, 3);
  assert.equal(
    nextPane(panes, 1, (p) => p.status === "exited"),
    null,
  );
  assert.equal(nextPane([], 1, attention), null);
});

test("a terminal's panel lists its own notifications, unread or history", () => {
  const list = [
    note(1, 1, "turn_finished"),
    note(2, 1, "failed", { read: true }),
    note(3, 7, "input_needed"),
    note(4, 1, "input_needed"),
    // An earlier process in the same pane is not this terminal's.
    note(5, 1, "failed", { origin: { run: "r", pid: 999 } }),
  ];
  assert.deepEqual(
    paneNotifications(list, pane(1), false).map((n) => n.id),
    [4, 1],
  );
  assert.deepEqual(
    paneNotifications(list, pane(1), true).map((n) => n.id),
    [4, 2, 1],
  );
  assert.deepEqual(
    paneNotifications(list, pane(7), false).map((n) => n.id),
    [3],
  );
  assert.deepEqual(paneNotifications(list, pane(2), true), []);
  // A closed terminal has none.
  assert.deepEqual(
    paneNotifications(list, pane(1, "exited", { dead: true }), true),
    [],
  );
});

test("showing a terminal or focusing its tile is what marks it read", () => {
  // A terminal shown on its own.
  assert.equal(readingPane(0, 3, [3]), 3);
  // The live grid: only the focused tile, when it is on this page.
  assert.equal(readingPane(0, 2, [1, 2, 3, 4]), 2);
  assert.equal(readingPane(0, 9, [1, 2, 3, 4]), 0);
  // Nothing on screen (the phone's terminal list) or nothing focused.
  assert.equal(readingPane(0, 3, []), 0);
  assert.equal(readingPane(0, 0, [1, 2]), 0);
  // A popup window shows its own terminal.
  assert.equal(readingPane(5, 3, [5]), 5);
});
