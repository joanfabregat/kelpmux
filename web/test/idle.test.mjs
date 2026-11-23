import { test } from "node:test";
import assert from "node:assert/strict";
import {
  ACTIVITY_EVENTS,
  activityReporter,
  countdown,
  idleMinutes,
  minutesText,
} from "../src/idle.ts";

test("activity reports are throttled per window, except while warned", () => {
  let now = 0,
    sent = 0;
  const report = activityReporter(
    () => sent++,
    () => now,
  );
  assert.equal(report(), true, "the first activity is reported at once");
  now = 29999;
  assert.equal(report(), false);
  assert.equal(sent, 1);
  now = 30000;
  assert.equal(report(), true);
  now = 30001;
  assert.equal(report(true), true, "a shown warning is lifted at once");
  assert.equal(sent, 3);
  now = 30002;
  assert.equal(report(), false, "the throttle restarts after an urgent one");
});

test("activity is keys, clicks, taps and scrolling, never pointer movement", () => {
  assert.deepEqual([...ACTIVITY_EVENTS].sort(), [
    "keydown",
    "pointerdown",
    "touchstart",
    "wheel",
  ]);
});

test("the countdown is mm:ss, rounded up and never negative", () => {
  assert.equal(countdown(15 * 60000), "15:00");
  assert.equal(countdown(15 * 60000 - 1), "15:00");
  assert.equal(countdown(14 * 60000 + 59000), "14:59");
  assert.equal(countdown(9000), "00:09");
  assert.equal(countdown(240 * 60000), "240:00");
  assert.equal(countdown(0), "00:00");
  assert.equal(countdown(-5000), "00:00");
});

test("idle durations default to 60 and 15 minutes unless the relay sets them", () => {
  assert.deepEqual(idleMinutes(null), { warning: 60, lock: 15 });
  assert.deepEqual(idleMinutes({ idle_lock_minutes: 5 }), {
    warning: 60,
    lock: 5,
  });
  assert.deepEqual(idleMinutes({ idle_warning_minutes: 1 }), {
    warning: 1,
    lock: 15,
  });
  assert.equal(minutesText(1), "1 minute");
  assert.equal(minutesText(60), "60 minutes");
});
