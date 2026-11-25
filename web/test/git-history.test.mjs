import { test } from "node:test";
import assert from "node:assert/strict";
import {
  defaultTab,
  diffStats,
  formatSize,
  numberLines,
  readBranches,
  readChanges,
  readCommit,
  readDiff,
  readLogPage,
  refLabels,
  sidesOf,
  validCommit,
  validRepoPath,
  validRevision,
} from "../src/git-history.ts";

const a = "a".repeat(40),
  b = "b".repeat(40);

test("revisions and paths follow the bridge's rules", () => {
  for (const good of ["HEAD", "main", "feat/x", "origin/main", "v1.2", "abcd", a])
    assert.ok(validRevision(good), good);
  for (const bad of [
    "",
    "-c",
    "--output=x",
    "HEAD:../",
    "@{-1}",
    "@",
    "HEAD~1",
    "HEAD^",
    "a..b",
    "a b",
    "x\u0001",
    "main.lock",
    "refs/.x",
    "a/",
    "/a",
    "a*",
  ])
    assert.ok(!validRevision(bad), JSON.stringify(bad));
  for (const good of ["a.txt", "src/main.rs", "a b/c", ".gitignore"])
    assert.ok(validRepoPath(good), good);
  for (const bad of ["", "../x", "a/../b", "/abs", "-x", ":(glob)*", ".git/config", "a//b", "a\nb"])
    assert.ok(!validRepoPath(bad), JSON.stringify(bad));
  assert.ok(validCommit("abcd"));
  assert.ok(!validCommit("ABCD!"));
});

test("answers are read defensively", () => {
  const branches = readBranches({
    head: a,
    current: "main",
    detached: false,
    branches: [
      { name: "main", ref: "refs/heads/main", current: true, upstream: "origin/main", ahead: 1, behind: 0, sha: a, subject: "S", author: "Ann", time: 5 },
      { name: "bad", sha: "nothex" },
    ],
    remotes: "nope",
  });
  assert.equal(branches.branches.length, 1);
  assert.equal(branches.branches[0].ahead, 1);
  assert.deepEqual(branches.remotes, []);
  assert.equal(readBranches(null), null);
  const page = readLogPage({
    rev: "HEAD",
    head: a,
    commits: [{ sha: a, parents: [b, "x"], author: "Ann", time: 1, subject: "S", refs: ["HEAD -> main", 3] }],
    more: true,
  });
  assert.deepEqual(page.commits[0].parents, [b]);
  assert.deepEqual(page.commits[0].refs, ["HEAD -> main"]);
  assert.equal(page.more, true);
  const commit = readCommit({ sha: a, parents: [b], files: [{ path: "x", status: "R", from: "y", added: 1, removed: 2 }] });
  assert.equal(commit.files[0].from, "y");
  assert.equal(commit.message, "");
  assert.equal(readCommit({ sha: "short" }), null);
  const diff = readDiff({
    kind: "staged",
    files: [{ path: "x", hunks: [{ header: "@@ -1 +1 @@", old_start: 1, old_lines: 1, new_start: 1, new_lines: 1, lines: ["-a", "+b"] }], truncated: true }],
  });
  assert.equal(diff.kind, "staged");
  assert.equal(diff.files[0].truncated, true);
  assert.equal(readDiff({ kind: "evil", files: [] }).kind, "commit");
  const changes = readChanges({
    staged: [{ path: "s", status: "M", added: 1, removed: 0 }],
    unstaged: [{ path: "s", status: "M", added: 2, removed: 1 }],
    untracked: [{ path: "u", added: 3, size: 10, binary: false }],
    conflicted: ["c"],
  });
  const sides = sidesOf(changes, "s");
  assert.equal(sides.staged.added, 1);
  assert.equal(sides.unstaged.added, 2);
  assert.equal(sides.untracked, undefined);
  assert.equal(sidesOf(changes, "c").conflicted, true);
});

test("hunks are numbered on both sides", () => {
  const rows = numberLines({
    header: "@@ -10,3 +10,4 @@",
    old_start: 10,
    old_lines: 3,
    new_start: 10,
    new_lines: 4,
    lines: [" a", "-b", "+c", "+d", " e", "\\ No newline at end of file"],
  });
  assert.deepEqual(
    rows.map((r) => [r.kind, r.old, r.new, r.text]),
    [
      ["context", 10, 10, "a"],
      ["del", 11, null, "b"],
      ["add", null, 11, "c"],
      ["add", null, 12, "d"],
      ["context", 12, 13, "e"],
      ["note", null, null, "\\ No newline at end of file"],
    ],
  );
  assert.deepEqual(diffStats({ hunks: [{ lines: ["+a", "-b", "+c", " d"] }] }), { added: 2, removed: 1 });
});

test("refs, tabs and sizes", () => {
  assert.deepEqual(refLabels(["HEAD -> main", "origin/main", "tag: v1", "feat/x"]), [
    { label: "main", kind: "head" },
    { label: "origin/main", kind: "remote" },
    { label: "v1", kind: "tag" },
    { label: "feat/x", kind: "branch" },
  ]);
  assert.equal(defaultTab({ files: 2 }), "changes");
  assert.equal(defaultTab({ files: 0 }), "history");
  assert.equal(formatSize(512), "512 B");
  assert.equal(formatSize(2048), "2.0 KB");
  assert.equal(formatSize(3 * 1024 * 1024), "3.0 MB");
});
