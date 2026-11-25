// A checkout's history and diffs in the Repos view
// (test/repo-history.fixture.tsx), in Chromium and WebKit, against a fake
// bridge kept here:
// 1. A checkout with changes opens on Changes: conflicted, staged, not
//    staged and untracked files; a file shows its staged and unstaged
//    diffs with line numbers (`tab=changes&path=…`), an untracked one its
//    content; the editor opens it.
// 2. History lists the log with refs; a commit opens beside it
//    (`tab=history&commit=…`) with its message, people, parents and files,
//    a small commit's diffs shown: a long hunk starts collapsed, a binary
//    file and a cut diff say so; a parent opens that commit; Load more,
//    First parent only and a path filter ask the bridge (an invalid path
//    is not sent).
// 3. Branches: current first with upstream ahead/behind; a branch opens
//    its history (`rev=…`).
// 4. Back and Forward walk through checkouts, tabs and commits; a deep
//    link opens the commit; invalid URL fields are dropped before any
//    request.
// 5. A phone: the details pane takes the list's place, Close returns,
//    nothing scrolls sideways. A bridge before API minor 10: the old list.
// Set REPO_HISTORY_SCREENSHOTS to a directory to save screenshots.
import { build } from "vite";
import { chromium, webkit, devices } from "playwright";
import { readFileSync, mkdirSync } from "node:fs";
import assert from "node:assert/strict";

const out = process.env.REPO_HISTORY_FIXTURE_OUTPUT || "/tmp/kelpmux-repo-history-fixture";
const shots = process.env.REPO_HISTORY_SCREENSHOTS;
if (shots) mkdirSync(shots, { recursive: true });
await build({
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  configFile: false,
  logLevel: "warn",
  root: new URL("..", import.meta.url).pathname,
  build: {
    outDir: out,
    emptyOutDir: true,
    lib: {
      entry: new URL("./repo-history.fixture.tsx", import.meta.url).pathname,
      name: "RepoHistoryFixture",
      formats: ["iife"],
      cssFileName: "repo-history-fixture",
      fileName: () => "fixture.js",
    },
    minify: false,
  },
});
const origin = "https://terminal.test";
const now = Math.floor(Date.now() / 1000);
const sha = (c) => c.repeat(40);
const API = "/home/me/work/alpha/api";

const repo = {
  path: API,
  name: "api",
  branch: "main",
  head: sha("a"),
  detached: false,
  upstream: "origin/main",
  ahead: 1,
  behind: 2,
  files: 4,
  staged: 1,
  unstaged: 1,
  untracked: 1,
  conflicted: 1,
  added: 12,
  removed: 3,
  uncounted: 0,
  last: { sha: sha("a"), subject: "Show diffs", time: now - 3600 },
  truncated: false,
  error: null,
};
const commits = [
  { sha: sha("a"), parents: [sha("b")], author: "Ann", time: now - 3600, subject: "Show diffs", refs: ["HEAD -> main", "tag: v1"] },
  { sha: sha("b"), parents: [sha("c"), sha("d")], author: "Bob", time: now - 86400, subject: "Merge feat", refs: ["origin/main"] },
  { sha: sha("c"), parents: [sha("e")], author: "Ann", time: now - 2 * 86400, subject: "Add the log", refs: [] },
  { sha: sha("d"), parents: [sha("e")], author: "Cy", time: now - 3 * 86400, subject: "Feature work", refs: ["feat"] },
  { sha: sha("e"), parents: [], author: "Ann", time: now - 4 * 86400, subject: "First", refs: [] },
];
const longHunk = {
  header: "@@ -0,0 +1,200 @@",
  old_start: 0,
  old_lines: 0,
  new_start: 1,
  new_lines: 200,
  lines: Array.from({ length: 200 }, (_, i) => `+line ${i + 1}`),
};
const smallHunk = {
  header: "@@ -10,3 +10,4 @@ fn main() {",
  old_start: 10,
  old_lines: 3,
  new_start: 10,
  new_lines: 4,
  lines: ["     let a = 1;", "-    old();", "+    new();", "+    more();", "     done();"],
};

function fakeBridge() {
  const state = { calls: [] };
  return {
    state,
    call(method, params) {
      state.calls.push([method, params]);
      if (method.startsWith("git_") && params.repo !== API) throw Error("this repository is not one of the project's checkouts");
      switch (method) {
        case "config":
          return {
            projects: [{ id: "alpha", name: "Alpha", folder: "~/work/alpha", path: "/home/me/work/alpha", color: "blue", missing: false }],
          };
        case "git":
          return {
            folder: "/home/me/work/alpha",
            exists: true,
            truncated: false,
            sampled_at: now,
            repos: [
              repo,
              { ...repo, path: "/home/me/work/alpha/docs", name: "docs", files: 0, staged: 0, unstaged: 0, untracked: 0, conflicted: 0, ahead: 0, behind: 0 },
            ],
          };
        case "git_changes":
          return {
            head: sha("a"),
            staged: [{ path: "src/main.rs", status: "M", added: 2, removed: 1, binary: false }],
            unstaged: [{ path: "src/main.rs", status: "M", added: 1, removed: 0, binary: false }],
            untracked: [{ path: "notes.md", added: 2, size: 12, binary: false }],
            conflicted: ["both.rs"],
            truncated: false,
          };
        case "git_log": {
          const list = params.first_parent ? commits.filter((c) => c.sha !== sha("d")) : commits;
          const from = params.rev === "feat" ? list.filter((c) => [sha("d"), sha("e")].includes(c.sha)) : list;
          const filtered = params.path ? from.filter((c) => c.sha === sha("c")) : from;
          const limit = params.limit || 50;
          return {
            rev: params.rev,
            head: filtered[0]?.sha || sha("a"),
            skip: 0,
            commits: filtered.slice(0, limit),
            more: filtered.length > limit,
          };
        }
        case "git_commit": {
          const c = commits.find((c) => c.sha.startsWith(params.rev));
          if (!c) throw Error("unknown revision");
          const files =
            c.sha === sha("a")
              ? [
                  { path: "src/main.rs", status: "M", added: 2, removed: 1, binary: false },
                  { path: "src/log.rs", from: "src/old.rs", status: "R", added: 200, removed: 0, binary: false },
                  { path: "logo.png", status: "A", added: null, removed: null, binary: true },
                ]
              : Array.from({ length: 8 }, (_, i) => ({ path: `f${i}.txt`, status: "M", added: 1, removed: 1, binary: false }));
          return {
            sha: c.sha,
            parents: c.parents,
            merge: c.parents.length > 1,
            author: c.author,
            author_email: `${c.author.toLowerCase()}@example.invalid`,
            author_time: c.time,
            committer: "Bob",
            committer_email: "bob@example.invalid",
            commit_time: c.time + 60,
            refs: c.refs,
            message: `${c.subject}\n\nWhy: the Repos view had no history.\nHow: read-only plumbing.`,
            message_truncated: false,
            files,
            files_truncated: false,
          };
        }
        case "git_diff": {
          const file = (fields) => ({ path: params.path, status: "M", binary: false, meta: [], hunks: [], truncated: false, ...fields });
          if (params.kind === "untracked")
            return { kind: "untracked", sha: null, path: params.path, truncated: false, files: [file({ status: "?", hunks: [{ header: "@@ -0,0 +1,2 @@", old_start: 0, old_lines: 0, new_start: 1, new_lines: 2, lines: ["+# Notes", "+todo"] }] })] };
          if (params.path === "logo.png")
            return { kind: "commit", sha: sha("a"), path: params.path, truncated: false, files: [file({ status: "A", binary: true, meta: ["new file mode 100644"] })] };
          if (params.path === "src/log.rs")
            return { kind: "commit", sha: sha("a"), path: params.path, truncated: true, files: [file({ status: "R", from: "src/old.rs", meta: ["similarity index 90%", "rename from src/old.rs", "rename to src/log.rs"], hunks: [longHunk], truncated: true })] };
          return { kind: params.kind, sha: null, path: params.path, truncated: false, files: [file({ hunks: [smallHunk] })] };
        }
        case "git_branches":
          return {
            head: sha("a"),
            current: "main",
            detached: false,
            branches: [
              { name: "main", ref: "refs/heads/main", current: true, upstream: "origin/main", ahead: 1, behind: 2, gone: false, sha: sha("a"), subject: "Show diffs", author: "Ann", time: now - 3600 },
              { name: "feat", ref: "refs/heads/feat", current: false, upstream: null, ahead: null, behind: null, gone: false, sha: sha("d"), subject: "Feature work", author: "Cy", time: now - 3 * 86400 },
            ],
            remotes: [{ name: "origin/main", ref: "refs/remotes/origin/main", current: false, upstream: null, ahead: null, behind: null, gone: false, sha: sha("b"), subject: "Merge feat", author: "Bob", time: now - 86400 }],
            truncated: false,
          };
      }
      throw Error(`unexpected ${method}`);
    },
  };
}

async function open(browser, options, server, search = "") {
  const context = await browser.newContext(options);
  await context.route(`${origin}/**`, (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/")
      return route.fulfill({
        contentType: "text/html; charset=utf-8",
        body: '<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover"><link rel="stylesheet" href="/fixture.css"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>',
      });
    if (path === "/fixture.css")
      return route.fulfill({ contentType: "text/css; charset=utf-8", body: readFileSync(`${out}/repo-history-fixture.css`, "utf8") });
    if (path === "/fixture.js")
      return route.fulfill({ contentType: "text/javascript; charset=utf-8", body: readFileSync(`${out}/fixture.js`) });
    return route.abort();
  });
  await context.exposeFunction("bridge", (method, params) => server.call(method, params));
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(`${origin}/${search}`);
  await page.waitForFunction(() => window.fixtureReady === true);
  return { context, page, errors };
}
const search = (page) => page.evaluate(() => decodeURIComponent(location.search));
const calls = (server, method) => server.state.calls.filter(([m]) => m === method).map(([, p]) => p);
async function until(condition) {
  for (let i = 0; i < 100 && !condition(); i++) await new Promise((resolve) => setTimeout(resolve, 50));
  assert.ok(condition(), "condition not met in time");
}
const base = "?view=repos&project=alpha";
const wide = { viewport: { width: 1360, height: 900 } };

for (const [name, engine] of [
  ["chromium", chromium],
  ["webkit", webkit],
]) {
  const browser = await engine.launch();
  try {
    const server = fakeBridge();
    const { context, page, errors } = await open(browser, wide, server, base);
    const api = page.locator('.git-repo[data-repo="api"]');
    await api.waitFor();

    // 1. Changes.
    await api.locator(".git-repo-toggle").click();
    await page.getByRole("tab", { name: /Changes/ }).waitFor();
    assert.equal(await search(page), `${base}&repo=${API}`);
    assert.equal(await page.getByRole("tab", { name: /Changes/ }).getAttribute("aria-selected"), "true");
    await page.locator(".rh-section").first().waitFor();
    assert.deepEqual(
      await page.$$eval(".rh-section-head", (heads) => heads.map((h) => h.firstChild.textContent.trim())),
      ["Conflicted", "Staged", "Not staged", "Untracked"],
    );
    await page.locator('.rh-section[aria-label="Staged"] .git-change[data-path="src/main.rs"] button').click();
    const pane = page.locator(".rh-pane");
    await pane.locator(".rh-diff").nth(1).waitFor();
    assert.equal(await search(page), `${base}&repo=${API}&tab=changes&path=src/main.rs`);
    assert.deepEqual(
      await pane.locator(".rh-diff-title").allInnerTexts(),
      ["STAGED", "NOT STAGED"].map((t) => t),
    );
    const firstDiff = pane.locator(".rh-diff").first();
    assert.deepEqual(
      await firstDiff.locator(".rh-line").evaluateAll((rows) =>
        rows.map((row) => [row.className.replace("rh-line rh-", ""), ...[...row.cells].map((c) => c.textContent)]),
      ),
      [
        ["context", "10", "10", "     let a = 1;"],
        ["del", "11", "", "−    old();"],
        ["add", "", "11", "+    new();"],
        ["add", "", "12", "+    more();"],
        ["context", "12", "13", "     done();"],
      ],
    );
    assert.deepEqual(
      calls(server, "git_diff").map((p) => [p.kind, p.path]),
      [
        ["staged", "src/main.rs"],
        ["unstaged", "src/main.rs"],
      ],
    );
    await pane.getByRole("button", { name: "Open src/main.rs in the editor" }).click();
    assert.deepEqual(await page.evaluate(() => window.opened), [`${API}/src/main.rs`]);
    if (shots && name === "chromium") await page.screenshot({ path: `${shots}/changes-dark.png` });
    await page.locator('.git-change[data-path="notes.md"] button').click();
    await pane.locator('[data-diff="notes.md"] .rh-add').first().waitFor();
    assert.match(await pane.innerText(), /# Notes/);
    await page.locator('.git-change[data-path="both.rs"] button').click();
    await pane.getByText(/Unmerged/).waitFor();

    // 2. History and a commit.
    await page.getByRole("tab", { name: "History" }).click();
    await page.locator(".rh-commit").first().waitFor();
    assert.equal(await search(page), `${base}&repo=${API}&tab=history`);
    assert.equal(await page.locator(".rh-commit").count(), 5);
    assert.match(await page.locator(".rh-commit").first().innerText(), /Show diffs[\s\S]*main[\s\S]*v1[\s\S]*aaaaaaa[\s\S]*Ann/);
    assert.match(await page.locator(".rh-commit").nth(1).innerText(), /merge/);
    await page.locator(`.rh-commit[data-sha="${sha("a")}"] button`).click();
    await pane.locator(".rh-subject").waitFor();
    assert.equal(await search(page), `${base}&repo=${API}&tab=history&commit=${sha("a")}`);
    assert.equal(await pane.locator(".rh-subject").innerText(), "Show diffs");
    assert.match(await pane.locator(".rh-message").innerText(), /Why: the Repos view had no history\./);
    assert.match(await pane.locator(".rh-facts").innerText(), /Ann <ann@example\.invalid>[\s\S]*Committer[\s\S]*Bob/);
    // A small commit shows every file's diff: a long hunk collapsed, a
    // binary file summarised, a cut diff said so.
    await pane.locator('[data-diff="logo.png"]').waitFor();
    assert.match(await pane.locator('[data-diff="logo.png"]').innerText(), /Binary file, not shown/);
    const log = pane.locator('[data-diff="src/log.rs"]');
    await log.waitFor();
    assert.match(await log.innerText(), /rename from src\/old\.rs/);
    assert.equal(await log.locator(".rh-line").count(), 40);
    assert.match(await log.innerText(), /only its beginning is shown/);
    await log.getByRole("button", { name: "Show 160 more lines" }).click();
    assert.equal(await log.locator(".rh-line").count(), 200);
    assert.match(await pane.locator('.rh-file[data-path="src/log.rs"]').innerText(), /src\/old\.rs → src\/log\.rs/);
    if (shots && name === "chromium") await page.screenshot({ path: `${shots}/commit-dark.png`, fullPage: false });
    // Its parent; a large commit's files start collapsed.
    await pane.getByRole("button", { name: `Open parent ${sha("b").slice(0, 7)}` }).click();
    await pane.locator(`[data-commit="${sha("b")}"] .rh-subject, .rh-subject`).first().waitFor();
    await until(() => calls(server, "git_commit").some((p) => p.rev === sha("b")));
    await page.waitForFunction(() => document.querySelector(".rh-subject")?.textContent === "Merge feat");
    assert.match(await pane.innerText(), /8 files changed \(against the first parent\)/i);
    assert.equal(await pane.locator(".rh-diff").count(), 0);
    // First parent only, a path filter, and an invalid one never sent.
    await page.getByLabel("First parent only").check();
    await until(() => calls(server, "git_log").some((p) => p.first_parent === true));
    await page.waitForFunction(() => document.querySelectorAll(".rh-commit").length === 4);
    await page.getByLabel("Filter by path").fill("../etc/passwd");
    await page.getByLabel("Filter by path").press("Enter");
    await page.getByText("A relative path inside the repository").waitFor();
    await page.getByLabel("Filter by path").fill("src/log.rs");
    await page.getByLabel("Filter by path").press("Enter");
    await until(() => calls(server, "git_log").some((p) => p.path === "src/log.rs"));
    assert.ok(!calls(server, "git_log").some((p) => p.path && p.path.includes("..")));
    await page.waitForFunction(() => document.querySelectorAll(".rh-commit").length === 1);

    // 3. Branches.
    await page.getByRole("tab", { name: "Branches" }).click();
    await page.locator(".rh-branch").first().waitFor();
    assert.equal(await search(page), `${base}&repo=${API}&tab=branches`);
    assert.deepEqual(await page.$$eval(".rh-branch", (rows) => rows.map((r) => r.dataset.branch)), ["main", "feat", "origin/main"]);
    assert.match(await page.locator('.rh-branch[data-branch="main"]').innerText(), /current[\s\S]*origin\/main ↑1 ↓2/);
    if (shots && name === "chromium") await page.screenshot({ path: `${shots}/branches-dark.png` });
    await page.locator('.rh-branch[data-branch="feat"] button').click();
    await page.waitForFunction(() => document.querySelectorAll(".rh-commit").length === 2);
    assert.equal(await search(page), `${base}&repo=${API}&tab=history&rev=feat`);
    assert.equal(calls(server, "git_log").pop().rev, "feat");
    assert.equal(await page.getByLabel("Revision").inputValue(), "feat");

    // 4. Back and Forward.
    await page.goBack();
    await page.locator(".rh-branch").first().waitFor();
    assert.equal(await search(page), `${base}&repo=${API}&tab=branches`);
    await page.goBack();
    await page.waitForFunction(() => document.querySelector(".rh-subject")?.textContent === "Merge feat");
    assert.equal(await search(page), `${base}&repo=${API}&tab=history&commit=${sha("b")}`);
    await page.goBack();
    await page.waitForFunction(() => document.querySelector(".rh-subject")?.textContent === "Show diffs");
    await page.goForward();
    await page.waitForFunction(() => document.querySelector(".rh-subject")?.textContent === "Merge feat");
    // Closing the pane is a step too.
    await pane.getByRole("button", { name: "Close the details" }).click();
    await page.locator(".rh-pane").waitFor({ state: "detached" });
    assert.equal(await search(page), `${base}&repo=${API}&tab=history`);
    // Collapsing the checkout.
    await api.locator(".git-repo-toggle").click();
    await page.locator(".rh-panel").waitFor({ state: "detached" });
    assert.equal(await search(page), base);
    assert.deepEqual(errors, []);
    await context.close();

    // A deep link to a commit; invalid fields dropped before any request.
    const deep = await open(browser, wide, server, `${base}&repo=${API}&tab=history&rev=feat&commit=${sha("d")}`);
    await deep.page.waitForFunction(() => document.querySelector(".rh-subject")?.textContent === "Feature work");
    const before = server.state.calls.length;
    const bad = await open(
      browser,
      wide,
      server,
      `${base}&repo=${API}&tab=history&rev=${encodeURIComponent("--output=/tmp/x")}&commit=${encodeURIComponent("HEAD:../")}`,
    );
    await bad.page.locator(".rh-commit").first().waitFor();
    assert.equal(await search(bad.page), `${base}&repo=${API}&tab=history`);
    const asked = server.state.calls.slice(before).filter(([m]) => m.startsWith("git_"));
    assert.ok(asked.every(([, p]) => !JSON.stringify(p).includes("--output") && !JSON.stringify(p).includes("HEAD:")), JSON.stringify(asked));
    // A checkout the view does not list opens nothing (the bridge refuses
    // it too: web_git_history::task).
    const other = await open(browser, wide, server, `${base}&repo=/etc&tab=history`);
    await other.page.locator('.git-repo[data-repo="api"]').waitFor();
    assert.equal(await other.page.locator(".rh-panel").count(), 0);
    for (const s of [deep, bad, other]) {
      assert.deepEqual(s.errors, []);
      await s.context.close();
    }

    if (shots && name === "chromium") {
      const light = await open(browser, wide, server, `?mode=light&view=repos&project=alpha&repo=${API}&tab=history&commit=${sha("a")}`);
      await light.page.locator('[data-diff="src/main.rs"] .rh-line').first().waitFor();
      await light.page.screenshot({ path: `${shots}/commit-light.png` });
      await light.context.close();
    }

    // 5. A phone: the pane takes the list's place.
    const { defaultBrowserType, ...iphone } = devices["iPhone 13"];
    const mobile = await open(browser, iphone, server, `?phone=1&view=repos&project=alpha&repo=${API}&tab=history&commit=${sha("a")}`);
    await mobile.page.locator('[data-diff="src/main.rs"] .rh-line').first().waitFor();
    assert.equal(await mobile.page.locator(".rh-main").isVisible(), false);
    const overflow = () =>
      mobile.page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth || [...document.querySelectorAll(".git-repo, .rh-pane")].some((el) => el.getBoundingClientRect().right > window.innerWidth + 1));
    assert.equal(await overflow(), false, "the commit fits a phone's width");
    if (shots && name === "chromium") await mobile.page.screenshot({ path: `${shots}/phone-commit.png` });
    await mobile.page.getByRole("button", { name: "Close the details" }).click();
    await mobile.page.locator(".rh-commit").first().waitFor();
    assert.equal(await mobile.page.locator(".rh-main").isVisible(), true);
    assert.equal(await overflow(), false, "the log fits a phone's width");
    if (shots && name === "chromium") await mobile.page.screenshot({ path: `${shots}/phone-log.png` });
    assert.deepEqual(mobile.errors, []);
    await mobile.context.close();

    // A bridge before API minor 10: the changed files, as before.
    const old = await open(browser, wide, server, `?nohistory=1&view=repos&project=alpha`);
    await old.page.locator('.git-repo[data-repo="api"] .git-repo-toggle').click();
    await old.page.locator(".git-changes").waitFor();
    assert.equal(await old.page.locator(".rh-panel").count(), 0);
    assert.ok(await old.page.locator('.git-repo[data-repo="docs"] .git-repo-toggle').isDisabled());
    await old.context.close();
    console.log(
      `PASS: ${name} repo-history: Changes (sections, staged and unstaged diffs with line numbers, untracked content, conflict, open in editor), History (log with refs, commit details, diffs, collapsed long hunk, binary and cut diffs, parents, first parent, path filter, invalid path not sent), Branches (current first, upstream counts, branch history), Back and Forward, close and collapse, deep link, invalid URL fields dropped, unknown checkout, phone stacking without overflow, older bridge`,
    );
  } finally {
    await browser.close();
  }
}
