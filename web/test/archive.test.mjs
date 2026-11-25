import { test } from "node:test";
import assert from "node:assert/strict";
import {
  anyRunning,
  checkoutProblems,
  checkSummary,
  defaultArchive,
  jobFraction,
  jobOf,
  phaseLabel,
  readArchives,
  readArchiveStatus,
  readCheck,
  readJobs,
  restoreFolder,
  sortedItems,
  splitArchived,
  statusActions,
} from "../src/archive-state.ts";
import { readProjects, scratchProject } from "../src/projects.ts";

const projects = readProjects([
  { id: "a", name: "Alpha", folder: "~/p/a", path: "/h/p/a", status: "active" },
  { id: "b", name: "Beta", folder: "~/p/b", path: "/h/p/b", status: "paused" },
  {
    id: "c",
    name: "Gamma",
    folder: "~/p/c",
    path: "/h/p/c",
    status: "archived",
    missing: true,
    archive: { backend: "gcs", bucket: "bk", object: "kelpmux/c/20261124T100000Z.tar.gz", size: 2048, sha256: "ab", crc32c: "AAAAAA==", archived_at: "2026-11-24T10:00:00Z" },
  },
  { id: "d", name: "Delta", folder: "~/p/d", path: "/h/p/d", status: "weird" },
]);

test("statuses and archive records are read defensively", () => {
  assert.deepEqual(projects.map((p) => p.status), ["active", "paused", "archived", "active"]);
  assert.equal(projects[2].archive.size, 2048);
  assert.equal(projects[2].archive.bucket, "bk");
  assert.equal(projects[0].archive, null);
  assert.equal(readProjects([{ id: "x", name: "X", path: "/x", archive: { bucket: 3 } }])[0].archive, null);
});

test("archived projects are listed apart, Scratch stays listed", () => {
  const scratch = scratchProject("~/scratch", "/h/scratch");
  const { listed, archived } = splitArchived([...projects, scratch]);
  assert.deepEqual(listed.map((p) => p.id), ["a", "b", "d", "scratch"]);
  assert.deepEqual(archived.map((p) => p.id), ["c"]);
});

test("the menu's status actions follow the status", () => {
  assert.deepEqual(statusActions(projects[0]), ["pause", "archive", "delete"]);
  assert.deepEqual(statusActions(projects[1]), ["resume", "archive", "delete"]);
  assert.deepEqual(statusActions(projects[2]), ["restore", "remove"]);
  assert.deepEqual(statusActions(scratchProject("~/s", "/h/s")), []);
});

const check = readCheck({
  project: "a",
  folder: "/h/p/a",
  mode: "delete",
  exists: true,
  checked_at: 1,
  fingerprint: "f00d",
  checkouts: [
    {
      path: "/h/p/a/api",
      name: "api",
      error: null,
      branch: "main",
      detached: false,
      uncommitted: 2,
      untracked: 1,
      ignored: 5,
      stashes: 1,
      branches: [
        { name: "main", upstream: "origin/main", unpushed: 3 },
        { name: "spike", upstream: null, unpushed: 0 },
        { name: "done", upstream: "origin/done", unpushed: 0 },
      ],
    },
    { path: "/h/p/a/web", name: "web", error: "skipped: untrusted config", branches: [] },
    { name: "no path" },
  ],
  other_files: { files: 12, bytes: 4096, truncated: false },
  items: [
    { severity: "info", kind: "ignored", path: "/h/p/a/api", message: "5 ignored paths" },
    { severity: "blocker", kind: "uncommitted", path: "/h/p/a/api", message: "2 uncommitted changes" },
    { severity: "warning", kind: "other_files", path: "/h/p/a", message: "12 other files" },
    { message: 7 },
  ],
  blockers: 1,
  warnings: 1,
  allowed: true,
});

test("a check is read defensively and never allowed with a blocker", () => {
  assert.equal(check.checkouts.length, 2);
  assert.equal(check.items.length, 3);
  assert.equal(check.allowed, false, "a blocker overrides allowed");
  assert.equal(readCheck({ project: "a" }), null);
  assert.deepEqual(
    sortedItems(check.items).map((i) => i.severity),
    ["blocker", "warning", "info"],
  );
  assert.match(checkSummary(check), /1 problem must be fixed first/);
  assert.equal(
    checkSummary({ ...check, blockers: 0, warnings: 0 }),
    "Everything in the checkouts is on a remote.",
  );
  assert.equal(checkSummary({ ...check, exists: false }), "The folder does not exist.");
});

test("a checkout's problems are named", () => {
  assert.deepEqual(checkoutProblems(check.checkouts[0]), [
    "2 uncommitted changes",
    "1 untracked file",
    "1 stash",
    "main: 3 commits on no remote",
    "spike: no upstream",
  ]);
  assert.deepEqual(checkoutProblems(check.checkouts[1]), ["not checked: skipped: untrusted config"]);
});

const job = (fields) => ({
  id: "j1",
  project: "a",
  kind: "archive",
  state: "running",
  phase: "uploading",
  message: "",
  bytes_done: 50,
  bytes_total: 200,
  files_done: 3,
  started_at: 10,
  updated_at: 20,
  error: null,
  result: null,
  cancellable: true,
  ...fields,
});

test("jobs are read, selected and measured", () => {
  const jobs = readJobs({
    jobs: [
      job({ id: "old", state: "done", updated_at: 5 }),
      job({ id: "new", state: "failed", updated_at: 30, cancellable: true }),
      job({ id: "b1", project: "b", kind: "restore" }),
      { id: 1 },
    ],
  });
  assert.equal(jobs.length, 3);
  assert.equal(jobs[1].cancellable, false, "only a running job can be cancelled");
  assert.equal(jobOf(jobs, "a").id, "new");
  assert.equal(jobOf(jobs, "b").id, "b1");
  assert.equal(jobOf(jobs, "z"), undefined);
  assert.equal(anyRunning(jobs), true);
  assert.equal(anyRunning(jobs.slice(0, 2)), false);
  assert.equal(jobFraction(jobs[2]), 0.25);
  assert.equal(jobFraction({ ...jobs[2], bytes_total: null }), null);
  assert.equal(jobFraction(jobs[0]), 1);
  assert.equal(phaseLabel("removing"), "Removing the folder");
  assert.equal(phaseLabel(""), "Starting");
});

test("archives, the default one and the backend's status", () => {
  const archives = readArchives({
    archives: [
      { object: "kelpmux/c/2.tar.gz", size: 10, updated: "2026-11-24T10:00:00Z", recorded: false },
      { object: "kelpmux/c/1.tar.gz", size: 9, recorded: true },
      { size: 3 },
    ],
  });
  assert.equal(archives.length, 2);
  assert.equal(defaultArchive(archives).object, "kelpmux/c/1.tar.gz");
  assert.equal(defaultArchive(archives.slice(0, 1)).object, "kelpmux/c/2.tar.gz");
  assert.equal(defaultArchive([]), undefined);
  assert.deepEqual(readArchiveStatus({ configured: false, prefix: 3 }), {
    configured: false,
    backend: null,
    bucket: null,
    prefix: "kelpmux",
    token_source: "metadata",
    token_label: "",
    error: null,
  });
  assert.equal(readArchiveStatus({}), null);
});

test("a restore beside the original gets a free folder", () => {
  assert.equal(restoreFolder("~/p/", "Gamma", []), "~/p/Gamma-2");
  assert.equal(restoreFolder("~/p", "Gamma", ["~/p/Gamma-2"]), "~/p/Gamma-3");
});
