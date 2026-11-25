// A project's Repos view and the Git marks (test/repos.fixture.tsx), in
// Chromium and WebKit, against a fake bridge kept here:
// 1. The sidebar's project header shows a changes mark (changed files and
//    unpushed commits over the project's checkouts, with a tooltip naming
//    them) only for a project with changes.
// 2. The project menu's Repos opens the view (`?view=repos&project=…`,
//    asked fresh with the changed files): each checkout with its branch,
//    upstream and ahead/behind, its changes and its last commit; a clean
//    checkout says so, a detached one names its commit.
// 3. Expanding a checkout lists its changed files; clicking one opens it
//    (a deleted file cannot be opened); Refresh asks again, fresh; New
//    terminal here asks the bridge for a terminal in that checkout and
//    project; Show in Files opens the browser at the checkout, where each
//    checkout folder's second line shows its branch and changes.
// 4. Back and Forward walk between the Repos view and the file browser; a
//    Repos URL without a project, or an unknown one, falls back.
// 5. A project without checkouts says so.
// Set REPOS_SCREENSHOTS to a directory to save screenshots.
import { build } from "vite";
import { chromium, webkit, devices } from "playwright";
import { readFileSync, mkdirSync } from "node:fs";
import assert from "node:assert/strict";

const out = process.env.REPOS_FIXTURE_OUTPUT || "/tmp/kelpmux-repos-fixture";
const shots = process.env.REPOS_SCREENSHOTS;
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
      entry: new URL("./repos.fixture.tsx", import.meta.url).pathname,
      name: "ReposFixture",
      formats: ["iife"],
      cssFileName: "repos-fixture",
      fileName: () => "fixture.js",
    },
    minify: false,
  },
});
const origin = "https://terminal.test";
const now = Math.floor(Date.now() / 1000);

const repo = (fields) => ({
  branch: "main",
  head: "4f2a9c1e8b7d6a5f4e3d2c1b0a9f8e7d6c5b4a39",
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
  last: {
    sha: "4f2a9c1e8b7d6a5f4e3d2c1b0a9f8e7d6c5b4a39",
    subject: "Update the docs index",
    time: now - 3 * 86400,
  },
  truncated: false,
  error: null,
  ...fields,
});

/** The fake bridge: two projects (Alpha with three checkouts, Beta whose
 * folder is one) and Scratch without any; every call recorded. */
function fakeBridge() {
  const state = { calls: [] };
  const projects = [
    { id: "alpha", name: "Alpha", folder: "~/work/alpha", path: "/home/me/work/alpha", color: "blue", missing: false },
    { id: "beta", name: "Beta", folder: "~/work/beta", path: "/home/me/work/beta", color: "green", missing: false },
  ];
  const changes = [
    { path: "src/main.rs", status: "M", staged: true, unstaged: true, added: 30, removed: 7 },
    { path: "Cargo.lock", status: "A", staged: true, unstaged: false, added: 2, removed: 0 },
    { path: "src/old.rs", status: "D", staged: false, unstaged: true, added: 0, removed: 12 },
    { path: "notes/todo.md", status: "?", staged: false, unstaged: false, added: 10, removed: 0 },
  ];
  const alpha = (files) => ({
    folder: "/home/me/work/alpha",
    exists: true,
    truncated: false,
    sampled_at: now,
    repos: [
      repo({
        path: "/home/me/work/alpha/api",
        name: "api",
        branch: "feat/git-view",
        upstream: "origin/feat/git-view",
        ahead: 2,
        behind: 1,
        files: 4,
        staged: 2,
        unstaged: 2,
        untracked: 1,
        added: 42,
        removed: 19,
        last: { sha: "9b1d0e2c3a4f5e6d7c8b9a0f1e2d3c4b5a6f7e8d", subject: "Count lines of untracked files", time: now - 2 * 3600 },
        ...(files ? { changes } : {}),
      }),
      repo({ path: "/home/me/work/alpha/docs", name: "docs", ...(files ? { changes: [] } : {}) }),
      repo({
        path: "/home/me/work/alpha/web",
        name: "web",
        branch: null,
        detached: true,
        upstream: null,
        ahead: null,
        behind: null,
        head: "c0ffee1234567890c0ffee1234567890c0ffee12",
        ...(files ? { changes: [] } : {}),
      }),
    ],
  });
  const beta = (files) => ({
    folder: "/home/me/work/beta",
    exists: true,
    truncated: false,
    sampled_at: now,
    repos: [repo({ path: "/home/me/work/beta", name: "beta", ...(files ? { changes: [] } : {}) })],
  });
  return {
    state,
    call(method, params) {
      state.calls.push([method, params]);
      switch (method) {
        case "config":
          return { projects, resolved: { scratch: { folder: "/home/me/scratch" } } };
        case "projects":
          return { panes: { 11: "alpha", 12: "beta" } };
        case "git":
          if (params.project === "alpha") return alpha(params.files);
          if (params.project === "beta") return beta(params.files);
          if (params.project === "scratch")
            return { folder: "/home/me/scratch", exists: true, truncated: false, sampled_at: now, repos: [] };
          throw Error("this project no longer exists");
        case "new":
          return { pane: 99 };
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
        body: '<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover"><link rel="stylesheet" href="/repos-fixture.css"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>',
      });
    if (path === "/repos-fixture.css")
      return route.fulfill({
        contentType: "text/css; charset=utf-8",
        body: readFileSync(`${out}/repos-fixture.css`, "utf8"),
      });
    if (path === "/fixture.js")
      return route.fulfill({
        contentType: "text/javascript; charset=utf-8",
        body: readFileSync(`${out}/fixture.js`),
      });
    return route.abort();
  });
  await context.exposeFunction("bridge", (method, params) =>
    server.call(method, params),
  );
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(`${origin}/${search}`);
  await page.waitForFunction(() => window.fixtureReady === true);
  return { context, page, errors };
}
const search = (page) => page.evaluate(() => location.search);
const gitCalls = (server, project) =>
  server.state.calls.filter(([m, p]) => m === "git" && p.project === project);
const projectMenu = async (page, name, item) => {
  await page.getByRole("button", { name: `Menu for ${name}` }).click();
  await page.getByRole("menuitem", { name: item }).click();
};
/** Waits for a condition on the fake bridge's side (at most 5 s). */
async function until(condition) {
  for (let i = 0; i < 100 && !condition(); i++)
    await new Promise((resolve) => setTimeout(resolve, 50));
  assert.ok(condition(), "condition not met in time");
}
const repoRow = (page, name) => page.locator(`.git-repo[data-repo="${name}"]`);

for (const [name, engine] of [
  ["chromium", chromium],
  ["webkit", webkit],
]) {
  const browser = await engine.launch();
  try {
    const server = fakeBridge();
    const { context, page, errors } = await open(
      browser,
      { viewport: { width: 1280, height: 800 } },
      server,
    );
    // 1. The sweep marks Alpha (4 files, 2 commits to push), not Beta.
    const mark = page.locator('[data-project-group="alpha"] .git-mark');
    await mark.waitFor();
    assert.equal(await mark.getAttribute("data-files"), "4");
    assert.equal(await mark.getAttribute("data-ahead"), "2");
    assert.equal(
      await mark.getAttribute("aria-label"),
      "4 files changed, 2 commits not pushed",
    );
    await until(() => gitCalls(server, "beta").length > 0);
    await page.waitForTimeout(50);
    assert.equal(await page.locator('[data-project-group="beta"] .git-mark').count(), 0);
    assert.ok(
      gitCalls(server, "alpha").every(([, p]) => p.files === false),
      "the sweep asks without files",
    );
    await mark.hover();
    const tooltip = page.getByRole("tooltip");
    await tooltip.waitFor();
    assert.match(await tooltip.innerText(), /api\s+feat\/git-view\s+4 changed\s+↑2/);
    if (shots && name === "chromium")
      await page
        .locator('[data-project-group="alpha"] .project-head')
        .screenshot({ path: `${shots}/sidebar-mark.png` });
    await page.mouse.move(900, 700);

    // 2. Repos from the project menu: a fresh request with files.
    const before = gitCalls(server, "alpha").length;
    await projectMenu(page, "Alpha", "Repos");
    await repoRow(page, "api").waitFor();
    assert.equal(await search(page), "?view=repos&project=alpha");
    const asked = gitCalls(server, "alpha").slice(before);
    assert.deepEqual(asked[0][1], { project: "alpha", files: true, fresh: true });
    const api = repoRow(page, "api");
    assert.match(await api.innerText(), /feat\/git-view/);
    assert.match(await api.innerText(), /origin\/feat\/git-view ↑2 ↓1/);
    assert.match(await api.innerText(), /4 changed \(2 staged, 2 unstaged, 1 untracked\)/);
    assert.match(await api.innerText(), /\+42\s*−19/);
    assert.match(await api.innerText(), /9b1d0e2 Count lines of untracked files · 2 h ago/);
    assert.match(await repoRow(page, "docs").innerText(), /Clean/);
    assert.match(await repoRow(page, "web").innerText(), /detached at c0ffee1/);
    assert.equal(await page.locator(".git-change").count(), 0, "collapsed");

    // 3. Expand api; open a file; a deleted one cannot be opened.
    await api.locator(".git-repo-toggle").click();
    await page.locator(".git-change").first().waitFor();
    assert.deepEqual(
      await page.$$eval(".git-change", (rows) =>
        rows.map((row) => [row.dataset.status, row.dataset.path]),
      ),
      [
        ["M", "src/main.rs"],
        ["A", "Cargo.lock"],
        ["D", "src/old.rs"],
        ["?", "notes/todo.md"],
      ],
    );
    assert.ok(
      await page.locator('.git-change[data-path="src/old.rs"] button').isDisabled(),
    );
    if (shots && name === "chromium")
      await page.screenshot({ path: `${shots}/repos-dark.png` });
    await page.locator('.git-change[data-path="src/main.rs"] button').click();
    assert.deepEqual(await page.evaluate(() => window.opened), [
      "/home/me/work/alpha/api/src/main.rs",
    ]);
    // Refresh asks fresh.
    const count = gitCalls(server, "alpha").length;
    await page.getByRole("button", { name: "Refresh" }).click();
    await until(() => gitCalls(server, "alpha").length > count);
    const refreshed = gitCalls(server, "alpha").slice(count);
    assert.ok(
      refreshed.some(([, p]) => p.fresh === true && p.files === true),
      "Refresh asks fresh",
    );
    // New terminal here: in the checkout, in the project.
    await page.getByRole("button", { name: "New terminal in docs" }).click();
    await until(() => server.state.calls.some(([m]) => m === "new"));
    assert.deepEqual(server.state.calls.filter(([m]) => m === "new").pop()[1], {
      project: "alpha",
      cwd: "/home/me/work/alpha/docs",
    });
    // Show in Files: the browser at the checkout, whose folder rows show
    // their branch and changes.
    await page.getByRole("button", { name: "Show api in Files" }).click();
    await page.locator('.files-row[data-path="/home/me/work/alpha/web"]').waitFor();
    assert.equal(
      await search(page),
      "?view=files&dir=/home/me/work/alpha/api&project=alpha",
    );
    const line = (path) =>
      page.locator(`.files-row[data-path="${path}"] .git-checkout-line`).innerText();
    assert.match(await line("/home/me/work/alpha/api"), /feat\/git-view\s*4\s*↑2/);
    assert.match(await line("/home/me/work/alpha/docs"), /^main$/);
    assert.match(await line("/home/me/work/alpha/web"), /detached at c0ffee1/);
    assert.equal(
      await page.locator('.files-row[data-path="/home/me/work/alpha/notes.md"] .git-checkout-line').count(),
      0,
    );
    if (shots && name === "chromium")
      await page.screenshot({ path: `${shots}/files-checkout-line.png` });

    // 4. Back to the Repos view, Forward to the files.
    await page.goBack();
    await repoRow(page, "api").waitFor();
    assert.equal(await search(page), "?view=repos&project=alpha");
    await page.goForward();
    await page.locator(".files-row").first().waitFor();
    assert.match(await search(page), /^\?view=files/);
    await page.goBack();
    await repoRow(page, "api").waitFor();
    // Beta's Repos view: another place.
    await projectMenu(page, "Beta", "Repos");
    await repoRow(page, "beta").waitFor();
    assert.equal(await search(page), "?view=repos&project=beta");
    await page.goBack();
    await repoRow(page, "api").waitFor();
    assert.deepEqual(errors, []);
    await context.close();

    // A Repos URL without a project, or with an unknown one, falls back.
    for (const url of ["?view=repos", "?view=repos&project=gone"]) {
      const fallback = await open(
        browser,
        { viewport: { width: 1280, height: 800 } },
        server,
        url,
      );
      await fallback.page.waitForFunction(() => location.search === "?view=grid");
      assert.deepEqual(fallback.errors, []);
      await fallback.context.close();
    }

    // 5. Scratch has no checkouts; deep link.
    const empty = await open(
      browser,
      { viewport: { width: 1280, height: 800 } },
      server,
      "?view=repos&project=scratch",
    );
    await empty.page.getByText("No Git repositories").waitFor();
    await empty.context.close();

    if (shots && name === "chromium") {
      const light = await open(
        browser,
        { viewport: { width: 1280, height: 800 } },
        server,
        "?mode=light&view=repos&project=alpha",
      );
      await repoRow(light.page, "api").waitFor();
      await repoRow(light.page, "api").locator(".git-repo-toggle").click();
      await light.page.locator(".git-change").first().waitFor();
      await light.page.screenshot({ path: `${shots}/repos-light.png` });
      await light.context.close();
    }
    // A phone: the view fits its width, nothing scrolls sideways.
    const { defaultBrowserType, ...iphone } = devices["iPhone 13"];
    const mobile = await open(
      browser,
      iphone,
      server,
      "?phone=1&view=repos&project=alpha",
    );
    await repoRow(mobile.page, "api").waitFor();
    await repoRow(mobile.page, "api").locator(".git-repo-toggle").click();
    await mobile.page.locator(".git-change").first().waitFor();
    const overflow = await mobile.page.evaluate(() =>
      [...document.querySelectorAll(".git-repo")].some(
        (row) => row.getBoundingClientRect().right > window.innerWidth,
      ),
    );
    assert.equal(overflow, false, "the checkouts fit a phone's width");
    if (shots && name === "chromium")
      await mobile.page.screenshot({ path: `${shots}/phone.png` });
    await mobile.context.close();
    console.log(
      `PASS: ${name} repos: sidebar changes mark with tooltip (only for a project with changes), Repos view from the project menu (fresh request with files; branch, upstream ahead/behind, change kinds, lines, last commit, clean and detached checkouts), changed files expand and open (not deleted ones), Refresh, New terminal here, Show in Files with checkout lines, Back and Forward, fallbacks, empty project, phone width`,
    );
  } finally {
    await browser.close();
  }
}
