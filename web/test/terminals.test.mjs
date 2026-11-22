import { test } from "node:test";
import assert from "node:assert/strict";
import {
  agentStateText,
  filterCounts,
  githubUrl,
  matchesFilter,
  repoName,
  repoQueryPaths,
  shortDuration,
  terminalRows,
} from "../src/state/terminals.ts";

const pane = (id, status = "idle", extra = {}) => ({
  id,
  pid: 100 + id,
  cwd: `/home/u/p${id}`,
  title: `t${id}`,
  status,
  dead: false,
  ...extra,
});

test("rows follow the tab order, parked terminals last", () => {
  const panes = [pane(3), pane(1), pane(2), pane(4)];
  const rows = terminalRows(panes, new Set([1]), [], "all");
  assert.deepEqual(
    rows.map((r) => r.pane.id),
    [3, 2, 4, 1],
  );
  assert.deepEqual(
    rows.map((r) => r.position),
    [1, 2, 3, 4],
  );
  assert.deepEqual(
    rows.map((r) => r.parked),
    [false, false, false, true],
  );
});

test("filters keep positions and the TUI's agent semantics", () => {
  const panes = [
    pane(1, "input_needed", { kind: "claude" }),
    pane(2, "turn_finished", { kind: "codex" }),
    pane(3, "working"),
    pane(4, "failed", { kind: "codex" }),
    pane(5, "working", { kind: "claude", dead: true }),
    pane(6, "exited", { kind: "claude" }),
  ];
  const ids = (filter) =>
    terminalRows(panes, new Set(), [], filter).map((r) => r.pane.id);
  assert.deepEqual(ids("all"), [1, 2, 3, 4, 5, 6]);
  assert.deepEqual(ids("agents"), [1, 2, 4, 6]);
  assert.deepEqual(ids("attention"), [1, 4]);
  assert.deepEqual(ids("finished"), [2, 6]);
  assert.deepEqual(
    terminalRows(panes, new Set(), [], "finished").map((r) => r.position),
    [2, 6],
  );
  assert.deepEqual(filterCounts(panes), {
    all: 6,
    agents: 4,
    attention: 2,
    finished: 2,
  });
  // A shell never matches the agent filters, even when it needs input.
  assert.equal(matchesFilter(pane(7, "input_needed"), "attention"), false);
});

test("a row carries agent state, repository, command and activity", () => {
  const panes = [
    pane(1, "input_needed", { kind: "claude", last_activity: 50 }),
    pane(2, "working", {
      command: { started_at: 900, ended_at: null },
      cwd: "/home/u/repo/src",
    }),
    pane(3, "idle", { command: { started_at: 10, ended_at: 20 } }),
    pane(4, "working", { kind: "codex" }),
  ];
  const repos = new Map([
    [
      "/home/u/repo/src",
      { root: "/home/u/repo", branch: "main", github: "o/repo" },
    ],
  ]);
  const rows = terminalRows(panes, new Set(), [], "all", repos, {
    1: "claude",
    2: "vim",
    3: null,
  });
  assert.equal(rows[0].agent, "Waiting on you");
  assert.equal(rows[0].command, "claude");
  assert.equal(rows[0].lastActivity, 50);
  assert.equal(rows[1].agent, null);
  assert.equal(rows[1].state, "running");
  assert.equal(rows[1].command, "vim");
  assert.equal(rows[1].commandSince, 900);
  assert.deepEqual(rows[1].repo, repos.get("/home/u/repo/src"));
  assert.equal(rows[2].command, null);
  assert.equal(rows[2].commandSince, null);
  assert.equal(rows[2].repo, null);
  assert.equal(rows[2].lastActivity, null);
  // Without a reported program, an agent shows its provider.
  assert.equal(rows[3].command, "Codex");
  assert.equal(rows[3].agent, "Working");
});

test("agent states use the table's words", () => {
  assert.equal(agentStateText("input_needed"), "Waiting on you");
  assert.equal(agentStateText("finished"), "Finished");
  assert.equal(agentStateText("working"), "Working");
  assert.equal(agentStateText("failed"), "Failed");
  assert.equal(agentStateText("idle"), "Idle");
});

test("GitHub links accept only owner/repo", () => {
  assert.equal(githubUrl("o-x/r.y_z"), "https://github.com/o-x/r.y_z");
  for (const bad of [null, "", "o", "o/r/x", "../r", "o/r?x", "o/<r>"])
    assert.equal(githubUrl(bad), null, String(bad));
  assert.equal(repoName({ root: "/home/u/repo/" }), "repo");
});

test("repository queries are distinct absolute folders, bounded", () => {
  const panes = [
    { cwd: "/a" },
    { cwd: "/a" },
    { cwd: "relative" },
    { cwd: "" },
    { cwd: "/b" },
  ];
  assert.deepEqual(repoQueryPaths(panes), ["/a", "/b"]);
  const many = Array.from({ length: 80 }, (_, i) => ({ cwd: `/p${i}` }));
  assert.equal(repoQueryPaths(many).length, 64);
});

test("short durations", () => {
  assert.equal(shortDuration(5), "5s");
  assert.equal(shortDuration(125), "2m");
  assert.equal(shortDuration(7300), "2h");
  assert.equal(shortDuration(200000), "2d");
  assert.equal(shortDuration(-3), "0s");
});
