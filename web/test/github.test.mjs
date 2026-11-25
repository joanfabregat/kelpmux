import { test } from "node:test";
import assert from "node:assert/strict";
import {
  cloneName,
  filterRepositories,
  formatSize,
  readFacts,
  readGitHubStatus,
  readRepositories,
  sourceText,
} from "../src/github-state.ts";
import { readProjectGit } from "../src/git.ts";

const status = (fields = {}) => ({
  enabled: true,
  source: "command",
  source_label: "/home/u/bin/github-token",
  signed_in: true,
  login: "octocat",
  name: "The Octocat",
  scopes: ["repo", "read:org"],
  rate: { limit: 5000, remaining: 4990, reset: 1900000000 },
  checked_at: 1800000000,
  ssh_agent: { configured: false, available: true },
  busy: false,
  error: null,
  repositories_ready: true,
  repositories_truncated: false,
  clones: [],
  ...fields,
});

test("the status is read defensively and never carries more than it says", () => {
  const s = readGitHubStatus(status({ token: "ghp_x", extra: 1 }));
  assert.equal(s.login, "octocat");
  assert.equal(s.source, "command");
  assert.deepEqual(s.scopes, ["repo", "read:org"]);
  assert.equal(s.rate.remaining, 4990);
  assert.equal("token" in s, false);
  assert.equal(sourceText(s), "/home/u/bin/github-token");
  assert.equal(sourceText({ ...s, source: "gh" }), "the GitHub CLI (gh auth token)");
  // Unknown or malformed values fall back.
  const odd = readGitHubStatus(
    status({ source: "sops", login: "../x", scopes: "repo", rate: { limit: -1 } }),
  );
  assert.equal(odd.source, null);
  assert.equal(odd.login, null);
  assert.equal(odd.scopes, null);
  assert.equal(odd.rate, null);
  assert.equal(readGitHubStatus({}), null);
  assert.equal(readGitHubStatus(null), null);
  const clones = readGitHubStatus(
    status({
      clones: [
        { id: 1, project: "p", full_name: "o/r", url: "git@github.com:o/r.git", path: "/p/r", state: "running", progress: "Receiving objects: 40%", error: null },
        { id: 2, project: "p", full_name: "mirror", url: "https://git.example/mirror.git", path: "/p/mirror", state: "failed", progress: "", error: "Git: denied" },
        { id: 3, full_name: "o/r", state: "exploded" },
        { id: 0, full_name: "o/r", state: "done" },
      ],
    }),
  ).clones;
  assert.deepEqual(
    clones.map((c) => [c.id, c.full_name, c.state]),
    [
      [1, "o/r", "running"],
      [2, "mirror", "failed"],
    ],
  );
});

test("repositories get their clone URLs from their name, never as given", () => {
  const repos = readRepositories({
    repositories: [
      { id: 1, name: "hello", full_name: "octocat/hello", ssh_url: "git@evil.example:x", private: true, size_kib: 2048, description: "Hi" },
      { id: 2, name: "../x", full_name: "o/../x" },
      { id: 3, name: "tool", full_name: "acme/tool", fork: true },
    ],
  });
  assert.equal(repos.length, 2);
  assert.equal(repos[0].ssh_url, "git@github.com:octocat/hello.git");
  assert.equal(repos[0].https_url, "https://github.com/octocat/hello.git");
  assert.equal(repos[1].fork, true);
  assert.equal(readRepositories({}), null);
  assert.deepEqual(
    filterRepositories(repos, "ACME").map((r) => r.full_name),
    ["acme/tool"],
  );
  assert.deepEqual(
    filterRepositories(repos, "octo hi").map((r) => r.full_name),
    ["octocat/hello"],
  );
});

test("facts and checkouts name GitHub repositories by owner/name only", () => {
  const facts = readFacts({
    repos: {
      "o/r": { visible: true, visibility: "private", default_branch: "main", open_pulls: 3, pushed_at: "2026-11-20T10:00:00Z" },
      "o/gone": { visible: false, default_branch: "x" },
      "../bad": { visible: true },
    },
  });
  assert.deepEqual(Object.keys(facts).sort(), ["o/gone", "o/r"]);
  assert.equal(facts["o/r"].open_pulls, 3);
  assert.deepEqual(facts["o/gone"], { visible: false });
  const git = readProjectGit({
    project: "p",
    folder: "/p",
    exists: true,
    sampled_at: 1,
    repos: [
      { path: "/p/a", name: "a", github: "owner/a" },
      { path: "/p/b", name: "b", github: "https://x@github.com/o/b" },
    ],
  });
  assert.equal(git.repos[0].github, "owner/a");
  assert.equal(git.repos[1].github, null);
});

test("clone URLs name the folder the bridge clones into", () => {
  for (const [url, name] of [
    ["git@github.com:octocat/Hello-World.git", "Hello-World"],
    ["https://github.com/octocat/hello", "hello"],
    ["ssh://git@host:22/srv/repo.git/", "repo"],
    ["  https://x.example/a/b.c.git ", "b.c"],
  ])
    assert.equal(cloneName(url), name, url);
  for (const url of ["", "git@github.com:o/.hidden", "https://x/a/..", "x;y"])
    assert.equal(cloneName(url), null, url);
  assert.equal(formatSize(10), "10 KB");
  assert.equal(formatSize(2048), "2 MB");
  assert.equal(formatSize(3 * 1024 * 1024), "3.0 GB");
});
