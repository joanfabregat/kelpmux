import { test } from "node:test";
import assert from "node:assert/strict";
import {
  environmentActions,
  environmentMark,
  environmentRequest,
  readEnvironmentPanes,
  readEnvironments,
  safeUrl,
} from "../src/environments.ts";

const entry = (fields) => ({
  name: "web",
  kind: "process",
  description: null,
  root: "/home/me/work/app",
  repo: "app",
  slug: "work-web",
  port: 41000,
  logs: true,
  state: "stopped",
  detail: null,
  url: null,
  pane: null,
  action: null,
  error: null,
  ...fields,
});

test("the bridge's environments reply is read defensively", () => {
  assert.equal(readEnvironments(null), null);
  assert.equal(readEnvironments({ environments: "x" }), null);
  const list = readEnvironments({
    project: "work",
    folder: "/home/me/work",
    exists: true,
    truncated: false,
    environments: [
      entry({ url: "http://localhost:41000/", state: "running", pane: 7 }),
      entry({ name: "x", url: "javascript:alert(1)", state: "weird" }),
      { kind: "process" },
    ],
  });
  assert.equal(list.environments.length, 2);
  assert.equal(list.environments[0].url, "http://localhost:41000/");
  assert.equal(list.environments[0].pane, 7);
  // A link is http(s) only; an unknown state reads as failed.
  assert.equal(list.environments[1].url, null);
  assert.equal(list.environments[1].state, "failed");
  assert.equal(safeUrl("https://a.test/x"), "https://a.test/x");
  assert.equal(safeUrl("file:///etc/passwd"), null);
});

test("states use the terminal marks and offer the right actions", () => {
  assert.equal(environmentMark("starting"), "working");
  assert.equal(environmentMark("running"), "finished");
  assert.equal(environmentMark("stopped"), "idle");
  assert.equal(environmentMark("failed"), "failed");
  assert.equal(environmentMark("invalid"), "input_needed");
  const read = (fields) => readEnvironments({ environments: [entry(fields)] }).environments[0];
  assert.deepEqual(environmentActions(read({ state: "stopped" })), { start: true, stop: false, check: false });
  assert.deepEqual(environmentActions(read({ state: "running", pane: 3 })), { start: false, stop: true, check: false });
  assert.deepEqual(environmentActions(read({ state: "failed", pane: 3 })), { start: true, stop: true, check: false });
  assert.deepEqual(environmentActions(read({ state: "invalid" })), { start: false, stop: false, check: false });
  assert.deepEqual(environmentActions(read({ kind: "deploy", state: "running" })), { start: true, stop: true, check: true });
  assert.deepEqual(environmentActions(read({ kind: "deploy", state: "starting" })), { start: false, stop: false, check: false });
  // A deploy environment never checked: unknown, Check status offered.
  const unknown = read({ kind: "deploy", state: "unknown" });
  assert.equal(unknown.checked, null);
  assert.deepEqual(environmentActions(unknown), { start: true, stop: true, check: true });
  assert.equal(environmentMark("unknown"), "idle");
  const last = read({ kind: "deploy", state: "unknown", checked: { state: "running", url: "javascript:x", detail: "ok", at: 1700000000 } });
  assert.deepEqual(last.checked, { state: "running", url: null, detail: "ok", at: 1700000000 });
  assert.equal(read({ checked: { state: "running" } }).checked, null, "a check has its time");
});

test("environment panes and request identities", () => {
  assert.deepEqual(
    readEnvironmentPanes({ panes: { 4: { project: "work", env: "web", action: "process" }, x: {}, 5: { env: "a" } } }),
    { 4: { project: "work", env: "web", action: "process" } },
  );
  const request = environmentRequest("start", "web");
  assert.match(request, /^[A-Za-z0-9_-]{1,128}$/);
  assert.notEqual(request, environmentRequest("start", "web"));
});
