import { test } from "node:test";
import assert from "node:assert/strict";
import {
  historyRows,
  resumeRequest,
  restoreRequest,
  choiceText,
  folderRequest,
  pickerRequest,
  resumeCommand,
  unidentified,
} from "../src/conversations.ts";
import { agentsFirst } from "../src/state/terminals.ts";

const projects = [
  { id: "alpha", name: "Alpha", folder: "~/a", path: "/a", color: null, missing: false },
];
const entry = (key, closed_at, project) => ({
  key,
  workspace: "main",
  pane: 1,
  cwd: "/a",
  name: key,
  argv: [],
  closed_at,
  reason: "exited",
  choices: ["restart", "folder"],
  project,
});

test("History rows: newest first, each with its project or Scratch", () => {
  const rows = historyRows(
    {
      workspace: "main",
      entries: [entry("old", 100, "alpha"), entry("gone", 300, "deleted")],
      external: [
        { kind: "claude", id: "x", cwd: "/a", last_seen: 200, project: "alpha" },
        { kind: "codex", id: "y", cwd: "/s", last_seen: 50, project: "scratch" },
      ],
    },
    projects,
  );
  assert.deepEqual(
    rows.map((row) => [row.type, row.at, row.project]),
    [
      ["terminal", 300, "scratch"],
      ["external", 200, "alpha"],
      ["terminal", 100, "alpha"],
      ["external", 50, "scratch"],
    ],
  );
});

test("Resume asks for a new terminal in the conversation's folder and project", () => {
  const c = { kind: "claude", id: "abc-1", cwd: "/a/api", folder: "/a/api" };
  const alpha = resumeRequest(c, "alpha");
  assert.equal(typeof alpha.request, "string");
  assert.deepEqual(
    { ...alpha, request: undefined },
    { request: undefined, project: "alpha", cwd: "/a/api", resume: { kind: "claude", id: "abc-1" } },
  );
  // Scratch is the default project: not named.
  assert.equal("project" in resumeRequest(c, "scratch"), false);
  // Without a folder there is nothing to resume in.
  assert.equal(resumeRequest({ ...c, folder: null }, "alpha"), null);
  const restore = restoreRequest(entry("k", 1, "alpha"), "restart");
  assert.equal(restore.key, "k");
  assert.equal(restore.choice, "restart");
  assert.equal(restore.confirmed, true);
});

test("a project's terminals list agents first, each group in order", () => {
  const panes = [
    { id: 1, kind: null },
    { id: 2, kind: "claude" },
    { id: 3 },
    { id: 4, kind: "codex" },
  ];
  assert.deepEqual(agentsFirst(panes).map((p) => p.id), [2, 4, 1, 3]);
});

test("an agent terminal without a known conversation opens its picker, labelled", () => {
  const codex = { ...entry("k", 1, "alpha"), kind: "codex", session_id: null, choices: ["resume"] };
  assert.equal(unidentified(codex), true);
  assert.equal(unidentified({ ...codex, session_id: "x-1" }), false);
  assert.equal(unidentified({ ...codex, kind: null }), false);
  assert.equal(choiceText(codex, "resume"), "Open Codex's picker here");
  assert.equal(choiceText({ ...codex, kind: "claude" }, "resume"), "Open Claude's picker here");
  assert.equal(choiceText({ ...codex, session_id: "x-1" }, "resume"), "Resume");
  assert.equal(resumeCommand({ kind: "codex" }), "codex resume");
  assert.deepEqual(
    { ...pickerRequest(codex, "alpha"), request: undefined },
    { request: undefined, project: "alpha", cwd: "/a", resume: { kind: "codex", picker: true } },
  );
  assert.equal("project" in pickerRequest(codex, "scratch"), false);
  assert.deepEqual(
    { ...folderRequest(codex, "alpha"), request: undefined },
    { request: undefined, project: "alpha", cwd: "/a" },
  );
});
