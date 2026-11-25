import { test } from "node:test";
import assert from "node:assert/strict";
import { WheelAccumulator } from "../src/wheel.ts";
test("application gestures start immediately, then accumulate pixels without magnifying every event", () => {
  const wheel = new WheelAccumulator();
  wheel.push(-1, 0, 20, 40, 1, true, 0);
  assert.equal(wheel.take(), -1);
  for (let n = 1; n <= 19; n++) {
    wheel.push(-1, 0, 20, 40, 1, true, n);
    assert.equal(wheel.take(), 0);
  }
  wheel.push(-1, 0, 20, 40, 1, true, 20);
  assert.equal(wheel.take(), -1);
  wheel.push(1, 0, 20, 40, 1, true, 21);
  assert.equal(wheel.take(), 1);
  wheel.push(1, 0, 20, 40, 1, true, 300);
  assert.equal(wheel.take(), 1);
});
test("small trackpad deltas accumulate instead of each jumping three rows", () => {
  const wheel = new WheelAccumulator();
  for (let n = 0; n < 19; n++) {
    wheel.push(1, 0, 20, 40);
    assert.equal(wheel.take(), 0);
  }
  wheel.push(1, 0, 20, 40);
  assert.equal(wheel.take(), 1);
  wheel.push(-40, 0, 20, 40);
  assert.equal(wheel.take(), -2);
});
test("wheel line/page units and application sensitivity retain bounded frame batches", () => {
  const wheel = new WheelAccumulator();
  wheel.push(3, 1, 20, 40, 3);
  assert.equal(wheel.take(), 1);
  wheel.push(1, 2, 20, 40);
  assert.equal(wheel.take(), 12);
  assert.ok(wheel.pending);
  assert.equal(wheel.take(), 12);
  assert.equal(wheel.take(), 12);
  assert.equal(wheel.take(), 4);
  wheel.push(Infinity, 0, 20, 40);
  assert.equal(wheel.take(), 0);
  wheel.push(60, 0, 20, 40, 3);
  assert.equal(wheel.take(), 1);
});
