import { test } from "node:test";
import assert from "node:assert/strict";
import {
  aheadBehind,
  changeKinds,
  changePath,
  changesSummary,
  checkoutsByPath,
  headLabel,
  projectChanges,
  readProjectGit,
  relativeTime,
  statusLabel,
} from "../src/git.ts";

const repo = (fields) => ({
  path: "/h/p/kelp/api",
  name: "api",
  branch: "main",
  head: "0123456789abcdef0123456789abcdef01234567",
  detached: false,
  upstream: "origin/main",
  ahead: 0,
  behind: 0,
  files: 0,
  staged: 0,
  unstaged: 0,
  untracked: 0,
  conflicted: 0,
  added: 0,
  removed: 0,
  uncounted: 0,
  last: { sha: "0123456789abcdef", subject: "Fix", time: 1_700_000_000 },
  truncated: false,
  error: null,
  ...fields,
});

test("the bridge's git result is read defensively", () => {
  const git = readProjectGit({
    folder: "/h/p/kelp",
    repos: [
      repo({ files: 2, changes: [{ path: "a.rs", status: "M", staged: true, added: 3, removed: "x" }, { status: "?" }] }),
      { path: "relative/path" },
      { path: "/h/p/kelp/web", files: -1, ahead: "2", last: { subject: "no sha" } },
    ],
    truncated: true,
    sampled_at: 10,
  });
  assert.equal(git.repos.length, 2);
  assert.equal(git.truncated, true);
  assert.deepEqual(git.repos[0].changes, [
    { path: "a.rs", status: "M", staged: true, unstaged: false, added: 3, removed: null },
  ]);
  assert.equal(git.repos[1].name, "web");
  assert.equal(git.repos[1].files, 0);
  assert.equal(git.repos[1].ahead, null);
  assert.equal(git.repos[1].last, null);
  assert.equal(readProjectGit(null), null);
  assert.equal(readProjectGit({ repos: "no" }), null);
});

test("a project's changes add up its changed checkouts", () => {
  const git = {
    folder: "/h/p/kelp",
    exists: true,
    truncated: false,
    sampled_at: 0,
    repos: [
      repo({ name: "api", files: 3, added: 10, removed: 2, ahead: 1 }),
      repo({ name: "web", path: "/h/p/kelp/web" }),
      repo({ name: "docs", path: "/h/p/kelp/docs", ahead: 2, truncated: true }),
      repo({ name: "bad", path: "/h/p/kelp/bad", files: 9, error: "Git: broken" }),
    ],
  };
  const changes = projectChanges(git);
  assert.deepEqual(
    changes.repos.map((r) => r.name),
    ["api", "docs"],
  );
  assert.equal(changes.files, 3);
  assert.equal(changes.ahead, 3);
  assert.equal(changes.added, 10);
  assert.equal(changes.partial, true);
  assert.equal(changesSummary(changes), "3 files or more changed, 3 commits not pushed");
  assert.equal(changesSummary({ files: 1, ahead: 0 }), "1 file changed");
  assert.equal(projectChanges({ ...git, repos: [git.repos[1]] }), null);
  assert.equal(projectChanges(undefined), null);
  assert.equal(checkoutsByPath({ kelp: git }).get("/h/p/kelp/web").name, "web");
});

test("labels", () => {
  assert.equal(headLabel(repo({})), "main");
  assert.equal(headLabel(repo({ branch: null, detached: true })), "detached at 0123456");
  assert.equal(headLabel(repo({ branch: null, head: null })), "");
  assert.equal(aheadBehind(repo({ ahead: 2, behind: 1 })), "↑2 ↓1");
  assert.equal(aheadBehind(repo({ ahead: null, behind: null })), "");
  assert.equal(
    changeKinds(repo({ staged: 1, unstaged: 2, untracked: 0, conflicted: 1 })),
    "1 staged, 2 unstaged, 1 conflicted",
  );
  assert.equal(statusLabel({ status: "M", staged: true, unstaged: true }), "Modified, staged and unstaged");
  assert.equal(statusLabel({ status: "?", staged: false, unstaged: false }), "Untracked");
  assert.equal(changePath(repo({ path: "/h/p/" }), { path: "src/a.rs" }), "/h/p/src/a.rs");
  const now = 1_000_000;
  assert.equal(relativeTime(now - 10, now), "just now");
  assert.equal(relativeTime(now - 300, now), "5 min ago");
  assert.equal(relativeTime(now - 7200, now), "2 h ago");
  assert.equal(relativeTime(now - 86400 * 1.5, now), "yesterday");
  assert.equal(relativeTime(now - 86400 * 20, now), "2 weeks ago");
  assert.equal(relativeTime(now - 86400 * 400, now), "1 year ago");
});
