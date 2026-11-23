import { test } from "node:test";
import assert from "node:assert/strict";
import {
  activityIcon,
  attentionCount,
  urgentState,
} from "../src/activity-icon.ts";

test("the favicon shows the most urgent live pane status", () => {
  const panes = [
    { status: "working", dead: false },
    { status: "turn_finished", dead: false },
    { status: "input_needed", dead: true },
  ];
  assert.equal(urgentState(panes), "turn_finished");
  assert.equal(attentionCount(panes), 1);
  const icon = activityIcon(panes);
  assert.equal(icon.title, "(1) KelpMux");
  assert.equal(icon.label, "Finished");
  assert.ok(icon.url.startsWith("data:image/svg+xml,"));
  assert.ok(decodeURIComponent(icon.url).includes("✓"));
});

test("idle and offline workspaces have a plain icon", () => {
  assert.equal(urgentState([{ status: "idle", dead: false }]), null);
  const offline = activityIcon(null);
  assert.equal(offline.title, "KelpMux");
  assert.equal(offline.label, "");
  assert.ok(!decodeURIComponent(offline.url).includes("<circle"));
});

test("an unread bell adds a dot to the favicon", () => {
  const panes = [{ status: "idle", dead: false }];
  const quiet = decodeURIComponent(activityIcon(panes).url);
  const ringing = activityIcon(panes, true);
  assert.equal(ringing.bell, true);
  assert.ok(!quiet.includes("data-bell"));
  assert.ok(decodeURIComponent(ringing.url).includes("data-bell"));
  assert.equal(ringing.title, "KelpMux");
});
