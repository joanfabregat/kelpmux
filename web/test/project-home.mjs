// A project's home page (test/project-home.fixture.tsx), in Chromium and
// WebKit, against a fake bridge kept here:
// 1. Clicking a project group's name in the sidebar opens its page
//    (`?view=project&project=…`, named in the title); the chevron still
//    folds the group and leaves the place alone.
// 2. The sections, in order, from the fake bridge: Agents & terminals
//    (agents first, with their state label, folder and last activity,
//    parked ones marked), Repos (one line per checkout: branch, changes,
//    +/−, ahead/behind or Clean; asked fresh) and History (the newest 8
//    of the project's entries, a closed terminal collapsed to its newest
//    conversation and their count, a matched conversation marked, an
//    unidentified Codex terminal offering only its labelled picker).
// 3. Resume sends the right `new` (the conversation's kind, id, folder
//    and project), the picker `new` with `resume: {kind, picker: true}`,
//    Rerun the right `restore`, and the reopened terminal
//    shows; a terminal line opens that terminal; the folder copies.
// 4. See all opens the project's grid, Repos view and History (a panel
//    over the page, which Back closes); a repository line opens the Repos
//    view with that checkout open on its Changes (in the URL). Back and
//    Forward walk them all.
// 5. The project menu's Overview opens another project's page (not
//    offered for the page shown); Scratch's page is reachable by URL; a
//    URL without a project or with an unknown one falls back.
// 6. An empty project says so in each section and offers New terminal.
// 7. Light theme and a phone (full width, nothing scrolls sideways).
// Set PROJECT_HOME_SCREENSHOTS to a directory to save screenshots.
import { build } from "vite";
import { chromium, webkit, devices } from "playwright";
import { readFileSync, mkdirSync } from "node:fs";
import assert from "node:assert/strict";

const out =
  process.env.PROJECT_HOME_FIXTURE_OUTPUT || "/tmp/kelpmux-project-home-fixture";
const shots = process.env.PROJECT_HOME_SCREENSHOTS;
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
      entry: new URL("./project-home.fixture.tsx", import.meta.url).pathname,
      name: "ProjectHomeFixture",
      formats: ["iife"],
      cssFileName: "project-home-fixture",
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
  last: { sha: "4f2a9c1e8b7d6a5f4e3d2c1b0a9f8e7d6c5b4a39", subject: "Docs", time: now - 86400 },
  truncated: false,
  error: null,
  ...fields,
});
const changes = [
  { path: "src/main.rs", status: "M", staged: true, unstaged: true, added: 30, removed: 7 },
  { path: "src/home.rs", status: "?", staged: false, unstaged: false, added: 12, removed: 0 },
];
const closed = (key, ago, project, fields = {}) => ({
  key,
  workspace: "main",
  pane: 40,
  cwd: "/home/me/work/alpha",
  name: key,
  kind: null,
  session_id: null,
  argv: ["npm", "run", "dev"],
  closed_at: now - ago,
  reason: "exited",
  choices: ["restart", "folder"],
  project,
  conversations: [],
  ...fields,
});

/** The fake bridge: Alpha (three checkouts, four terminals, thirteen
 * History entries), Beta, Gamma (empty) and Scratch; every call recorded. */
function fakeBridge() {
  const state = { calls: [] };
  const projects = [
    { id: "alpha", name: "Alpha", folder: "~/work/alpha", path: "/home/me/work/alpha", color: "blue", missing: false },
    { id: "beta", name: "Beta", folder: "~/work/beta", path: "/home/me/work/beta", color: "green", missing: false },
    { id: "gamma", name: "Gamma", folder: "~/work/gamma", path: "/home/me/work/gamma", color: null, missing: false },
  ];
  const git = {
    alpha: (files) => ({
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
          files: 2,
          staged: 1,
          unstaged: 1,
          untracked: 1,
          added: 42,
          removed: 7,
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
    }),
    beta: () => ({
      folder: "/home/me/work/beta",
      exists: true,
      truncated: false,
      sampled_at: now,
      repos: [repo({ path: "/home/me/work/beta", name: "beta", changes: [] })],
    }),
    gamma: () => ({ folder: "/home/me/work/gamma", exists: true, truncated: false, sampled_at: now, repos: [] }),
    scratch: () => ({ folder: "/home/me/scratch", exists: true, truncated: false, sampled_at: now, repos: [] }),
  };
  const conversation = (id, title, ago, fields = {}) => ({
    kind: "claude",
    id,
    cwd: "/home/me/work/alpha/api",
    folder: "/home/me/work/alpha/api",
    title,
    titled: true,
    first_seen: now - ago - 1800,
    last_seen: now - ago,
    ...fields,
  });
  const listing = {
    workspace: "main",
    entries: [
      closed("claude-login", 600, "alpha", {
        kind: "claude",
        name: "fix the login",
        conversations: [
          conversation("c-new", "Add the project home page", 700),
          conversation("c-old", "Sketch the sections", 5000),
        ],
      }),
      closed("dev-server", 7200, "alpha"),
      // Codex whose conversation is unknown: its picker, labelled.
      closed("codex-unknown", 8000, "alpha", {
        kind: "codex",
        name: "codex",
        choices: ["resume"],
      }),
      // Its only possible conversation, matched by folder and time.
      closed("codex-matched", 9000, "alpha", {
        kind: "codex",
        name: "codex run",
        choices: ["resume"],
        conversations: [
          conversation("x-matched", "Matched by its folder", 9100, {
            kind: "codex",
            matched: true,
          }),
        ],
      }),
      closed("beta-vim", 100, "beta", { cwd: "/home/me/work/beta" }),
      ...Array.from({ length: 8 }, (_, i) =>
        closed(`old-${i + 1}`, 86400 * (i + 2), "alpha"),
      ),
    ],
    external: [
      {
        kind: "codex",
        id: "x-flaky",
        cwd: "/home/me/work/alpha",
        folder: "/home/me/work/alpha",
        title: "Investigate the flaky test",
        titled: true,
        first_seen: now - 4000,
        last_seen: now - 3600,
        project: "alpha",
      },
      {
        kind: "claude",
        id: "x-scratch",
        cwd: "/home/me/scratch",
        folder: "/home/me/scratch",
        title: "Try a regex",
        titled: true,
        first_seen: now - 300,
        last_seen: now - 200,
        project: "scratch",
      },
    ],
    truncated: false,
  };
  return {
    state,
    call(method, params) {
      state.calls.push([method, params]);
      switch (method) {
        case "config":
          return { projects, resolved: { scratch: { folder: "/home/me/scratch" } } };
        case "projects":
          return { panes: { 11: "alpha", 12: "alpha", 13: "alpha", 14: "beta", 16: "alpha" } };
        case "git":
          if (git[params.project]) return git[params.project](params.files);
          throw Error("this project no longer exists");
        case "conversations":
          return listing;
        case "new":
          return { pane: 99 };
        case "restore":
          return { pane: 98 };
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
        body: '<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover"><link rel="stylesheet" href="/project-home-fixture.css"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>',
      });
    if (path === "/project-home-fixture.css")
      return route.fulfill({
        contentType: "text/css; charset=utf-8",
        body: readFileSync(`${out}/project-home-fixture.css`, "utf8"),
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
const at = async (page, expected) => { await at0(page, expected); if (process.env.TRACE) console.log("at", expected, await page.evaluate(() => history.length)); };
const at0 = (page, expected) =>
  page.waitForFunction((want) => location.search === want, expected);
const calls = (server, method) =>
  server.state.calls.filter(([m]) => m === method).map(([, p]) => p);
/** Waits for a condition on the fake bridge's side (at most 5 s). */
async function until(condition) {
  for (let i = 0; i < 100 && !condition(); i++)
    await new Promise((resolve) => setTimeout(resolve, 50));
  assert.ok(condition(), "condition not met in time");
}
const section = (page, id) => page.locator(`.home-card[data-section="${id}"]`);
const homeOf = (page, id) => page.locator(`[data-project-home="${id}"]`);
/** Waits until every section of a page has its data. */
async function loaded(page, id) {
  await homeOf(page, id).waitFor();
  await page.waitForFunction(
    () =>
      document.querySelectorAll(".home-card .home-count").length ===
      document.querySelectorAll(".home-card").length,
  );
}
const lines = (page, id, selector = ".home-line") =>
  section(page, id).locator(selector);

for (const [name, engine] of [
  ["chromium", chromium],
  ["webkit", webkit],
]) {
  const browser = await engine.launch();
  try {
    const server = fakeBridge();
    const { context, page, errors } = await open(
      browser,
      { viewport: { width: 1280, height: 860 } },
      server,
    );
    // 1. The chevron folds; the name opens the page.
    const alphaHead = page.locator('[data-project-group="alpha"] .project-head');
    await alphaHead.locator(".project-fold").click();
    assert.equal(
      await page.locator('[data-project-group="alpha"] .term-list').count(),
      0,
      "the chevron folds the group",
    );
    assert.equal(await search(page), "?view=grid");
    await alphaHead.locator(".project-fold").click();
    await page.locator('[data-project-group="alpha"] .term-list').waitFor();
    const gitBefore = calls(server, "git").filter((p) => p.project === "alpha").length;
    await alphaHead.locator(".project-open").click();
    await at(page, "?view=project&project=alpha");
    await loaded(page, "alpha");
    assert.equal(
      await page.locator("#title").evaluate((el) => el.textContent),
      "Alpha",
    );
    assert.equal(
      await alphaHead.locator(".project-open").getAttribute("aria-current"),
      "page",
    );
    // The header: name, folder, actions.
    const head = page.locator(".home-head");
    assert.equal(await head.locator("h1").innerText(), "Alpha");
    assert.match(await head.innerText(), /~\/work\/alpha/);
    await head.getByRole("button", { name: "Copy the folder's path" }).click();
    assert.equal(await page.evaluate(() => window.copied), "/home/me/work/alpha");
    // Environments: a link only (the page asks nothing about them).
    await head.getByRole("button", { name: "Environments" }).click();
    assert.ok((await page.evaluate(() => window.toasts)).includes("envs alpha"));

    // 2. The sections, in order.
    assert.deepEqual(
      await page.$$eval(".home-card", (cards) => cards.map((c) => c.dataset.section)),
      ["terminals", "repos", "history"],
    );
    assert.equal(await section(page, "terminals").locator(".home-count").innerText(), "4");
    assert.deepEqual(
      await lines(page, "terminals").evaluateAll((rows) =>
        rows.map((row) => [
          Number(row.dataset.pane),
          row.querySelector(".home-state").textContent,
        ]),
      ),
      [
        [11, "Waiting on you"],
        [13, "Working"],
        [16, "Finished"],
        [12, "Idle"],
      ],
      "agents first, with their states",
    );
    const login = lines(page, "terminals").first();
    assert.match(await login.innerText(), /fix the login flow/);
    assert.match(await login.innerText(), /~\/work\/alpha\/api/);
    assert.match(await login.innerText(), /2m ago/);
    assert.match(
      await page.locator('.home-terminal[data-pane="16"]').innerText(),
      /parked/,
    );
    // Repos: asked fresh with files, one compact line per checkout.
    await until(() =>
      calls(server, "git")
        .filter((p) => p.project === "alpha")
        .slice(gitBefore)
        .some((p) => p.fresh && p.files),
    );
    assert.equal(await section(page, "repos").locator(".home-count").innerText(), "3");
    const api = page.locator('.home-repo[data-repo="api"]');
    assert.match(await api.innerText(), /api\s+feat\/git-view\s+2 changed\s+\+42\s*−7\s+↑2 ↓1/);
    assert.match(await page.locator('.home-repo[data-repo="docs"]').innerText(), /docs\s+main\s+Clean/);
    assert.match(await page.locator('.home-repo[data-repo="web"]').innerText(), /detached at c0ffee1\s+Clean/);
    // History: the newest 8 of Alpha's 13 entries (not Beta's or Scratch's).
    assert.equal(await section(page, "history").locator(".home-count").innerText(), "13");
    assert.equal(await lines(page, "history").count(), 8);
    const first = lines(page, "history").first();
    assert.equal(await first.getAttribute("data-entry"), "claude-login");
    assert.match(await first.innerText(), /Add the project home page/);
    assert.match(await first.innerText(), /fix the login · 2 conversations/);
    assert.match(await lines(page, "history").nth(1).innerText(), /Investigate the flaky test\s*outside KelpMux/);
    assert.match(await lines(page, "history").nth(2).innerText(), /dev-server/);
    if (shots && name === "chromium")
      await page.screenshot({ path: `${shots}/desktop-dark.png`, fullPage: true });

    // 3. Resume: the newest conversation, in its folder and project.
    await first.getByRole("button", { name: "Resume Add the project home page" }).click();
    await at(page, "?view=terminal&terminal=99");
    const resumed = calls(server, "new").pop();
    assert.equal(typeof resumed.request, "string");
    assert.deepEqual(
      { ...resumed, request: "" },
      {
        request: "",
        project: "alpha",
        cwd: "/home/me/work/alpha/api",
        resume: { kind: "claude", id: "c-new" },
      },
    );
    assert.deepEqual(await page.evaluate(() => window.restored), [[99, false]]);
    await page.goBack();
    await at(page, "?view=project&project=alpha");
    await loaded(page, "alpha");
    // An outside conversation: in the project's folder.
    await lines(page, "history")
      .nth(1)
      .getByRole("button", { name: "Resume Investigate the flaky test" })
      .click();
    await at(page, "?view=terminal&terminal=99");
    assert.deepEqual(
      { ...calls(server, "new").pop(), request: "" },
      {
        request: "",
        project: "alpha",
        cwd: "/home/me/work/alpha",
        resume: { kind: "codex", id: "x-flaky" },
      },
    );
    await page.goBack();
    await loaded(page, "alpha");
    // A closed terminal without conversations: its first choice.
    await page.getByRole("button", { name: "Rerun dev-server" }).click();
    await at(page, "?view=terminal&terminal=98");
    const restore = calls(server, "restore").pop();
    assert.equal(restore.key, "dev-server");
    assert.equal(restore.choice, "restart");
    await page.goBack();
    await loaded(page, "alpha");
    // A matched conversation: one line, marked, resumed by its id.
    const matched = page.locator('.home-history[data-entry="codex-matched"]');
    assert.equal(await matched.getAttribute("data-conversation"), "x-matched");
    assert.equal(
      await matched.locator(".history-matched").innerText(),
      "matched by folder and time",
    );
    assert.match(await matched.locator(".history-matched").getAttribute("title"), /did not record/);
    // Codex without a known conversation: no Resume, only its picker.
    const unknown = page.locator('.home-history[data-entry="codex-unknown"]');
    assert.equal(await unknown.getAttribute("data-conversation"), null);
    assert.deepEqual(
      (await unknown.locator("button").allTextContents()).map((t) => t.trim()),
      ["Open Codex's picker here"],
    );
    await unknown.getByRole("button", { name: /^Open Codex's picker here/ }).click();
    await at(page, "?view=terminal&terminal=99");
    assert.deepEqual(
      { ...calls(server, "new").pop(), request: "" },
      {
        request: "",
        project: "alpha",
        cwd: "/home/me/work/alpha",
        resume: { kind: "codex", picker: true },
      },
    );
    await page.goBack();
    await loaded(page, "alpha");
    // A terminal line opens it.
    await page.locator('.home-terminal[data-pane="13"]').click();
    await at(page, "?view=terminal&terminal=13");
    await page.goBack();
    await loaded(page, "alpha");

    // 4. See all, and a repository line.
    await section(page, "terminals").getByRole("button", { name: /See all/ }).click();
    await at(page, "?view=grid&project=alpha");
    await page.goBack();
    await loaded(page, "alpha");
    await section(page, "repos").getByRole("button", { name: /See all/ }).click();
    await at(page, "?view=repos&project=alpha");
    await page.locator('.git-repo[data-repo="api"]').waitFor();
    assert.equal(await page.locator(".git-change").count(), 0, "nothing expanded");
    await page.goBack();
    await loaded(page, "alpha");
    await api.click();
    await at(page, "?view=repos&project=alpha&repo=/home/me/work/alpha/api&tab=changes");
    await page.locator(".git-change").first().waitFor();
    assert.equal(
      await page
        .locator('.git-repo[data-repo="api"] .git-repo-toggle')
        .getAttribute("aria-expanded"),
      "true",
      "the checkout clicked is expanded",
    );
    await page.goBack();
    await loaded(page, "alpha");
    await section(page, "history").getByRole("button", { name: /See all/ }).click();
    await at(page, "?view=project&dialog=history&project=alpha");
    const dialog = page.locator('[data-dialog="recovery"]');
    await dialog.locator('[data-entry="claude-login"]').waitFor();
    assert.equal(await dialog.locator('[data-entry="beta-vim"]').count(), 0);
    assert.equal(
      await dialog.getByRole("button", { name: "All projects" }).count(),
      0,
      "History over the page stays scoped",
    );
    await page.goBack();
    await at(page, "?view=project&project=alpha");
    assert.equal(await dialog.count(), 0, "Back closes History");
    await page.goForward();
    await dialog.waitFor();
    await page.goBack();
    await at(page, "?view=project&project=alpha");

    // 5. Overview from another project's menu; not for the page shown.
    await page.locator(".home-head").getByRole("button", { name: "Menu for Alpha" }).click();
    await page.getByRole("menuitem", { name: "Repos" }).waitFor();
    assert.equal(await page.getByRole("menuitem", { name: "Overview" }).count(), 0);
    await page.keyboard.press("Escape");
    await page.getByRole("button", { name: "Menu for Beta" }).click();
    await page.getByRole("menuitem", { name: "Overview" }).click();
    await at(page, "?view=project&project=beta");
    await loaded(page, "beta");
    assert.equal(await page.locator("#title").evaluate((el) => el.textContent), "Beta");
    await page.goBack();
    await at(page, "?view=project&project=alpha");
    await loaded(page, "alpha");
    await page.goForward();
    await at(page, "?view=project&project=beta");
    await loaded(page, "beta");
    await page.goBack();
    await at(page, "?view=project&project=alpha");
    await loaded(page, "alpha");
    await page.goBack();
    await at(page, "?view=grid");
    await page.goForward();
    await at(page, "?view=project&project=alpha");
    await loaded(page, "alpha");
    await page.reload();
    await page.waitForFunction(() => window.fixtureReady === true);
    await at(page, "?view=project&project=alpha");
    await loaded(page, "alpha");
    assert.deepEqual(errors, []);
    await context.close();

    // Scratch's page by URL: its terminal and its History only.
    const scratch = await open(
      browser,
      { viewport: { width: 1280, height: 860 } },
      server,
      "?view=project&project=scratch",
    );
    await loaded(scratch.page, "scratch");
    assert.deepEqual(
      await lines(scratch.page, "terminals").evaluateAll((rows) =>
        rows.map((row) => Number(row.dataset.pane)),
      ),
      [15],
    );
    assert.deepEqual(
      await lines(scratch.page, "history").evaluateAll((rows) =>
        rows.map((row) => row.dataset.conversation),
      ),
      ["x-scratch"],
    );
    await lines(scratch.page, "history").first().getByRole("button", { name: /Resume/ }).click();
    await at(scratch.page, "?view=terminal&terminal=99");
    assert.deepEqual(
      { ...calls(server, "new").pop(), request: "" },
      { request: "", cwd: "/home/me/scratch", resume: { kind: "claude", id: "x-scratch" } },
      "Scratch is not named",
    );
    assert.deepEqual(scratch.errors, []);
    await scratch.context.close();
    // Fallbacks.
    for (const url of ["?view=project", "?view=project&project=gone"]) {
      const fallback = await open(
        browser,
        { viewport: { width: 1280, height: 800 } },
        server,
        url,
      );
      await at(fallback.page, "?view=grid");
      assert.deepEqual(fallback.errors, []);
      await fallback.context.close();
    }

    // 6. An empty project.
    const empty = await open(
      browser,
      { viewport: { width: 1280, height: 860 } },
      server,
      "?view=project&project=gamma",
    );
    await loaded(empty.page, "gamma");
    assert.match(await section(empty.page, "terminals").innerText(), /No terminals in Gamma/);
    assert.match(await section(empty.page, "repos").innerText(), /No Git checkouts in this folder/);
    assert.match(await section(empty.page, "history").innerText(), /will appear here/);
    if (shots && name === "chromium")
      await empty.page.screenshot({ path: `${shots}/empty-project.png` });
    await section(empty.page, "terminals").getByRole("button", { name: "New terminal" }).click();
    await until(() => calls(server, "new").some((p) => p.project === "gamma"));
    assert.deepEqual(empty.errors, []);
    await empty.context.close();

    // 7. Light theme; a phone.
    if (shots && name === "chromium") {
      const light = await open(
        browser,
        { viewport: { width: 1280, height: 860 } },
        server,
        "?mode=light&view=project&project=alpha",
      );
      await loaded(light.page, "alpha");
      await light.page.screenshot({ path: `${shots}/desktop-light.png`, fullPage: true });
      await light.context.close();
    }
    const { defaultBrowserType, ...iphone } = devices["iPhone 13"];
    const mobile = await open(
      browser,
      iphone,
      server,
      "?phone=1&view=project&project=alpha",
    );
    await loaded(mobile.page, "alpha");
    const overflow = await mobile.page.evaluate(() =>
      [...document.querySelectorAll(".home-card, .home-line, .home-head")].some(
        (el) => el.getBoundingClientRect().right > window.innerWidth + 0.5,
      ),
    );
    assert.equal(overflow, false, "the page fits a phone's width");
    if (shots && name === "chromium")
      await mobile.page.screenshot({ path: `${shots}/phone.png`, fullPage: true });
    // Back to the list, and See all terminals is the list too.
    await mobile.page.getByRole("button", { name: "Back to the terminals" }).click();
    await at(mobile.page, "?phone=1&view=home");
    await mobile.page.goBack();
    await at(mobile.page, "?phone=1&view=project&project=alpha");
    assert.deepEqual(mobile.errors, []);
    await mobile.context.close();
    console.log(
      `PASS: ${name} project home: sidebar name opens it (chevron folds), title names it, header copies the folder, sections in order (agents first with states, folders and activity; repos with branch, changes, lines and ahead/behind or Clean, asked fresh; the newest 8 History entries with conversations collapsed, matched marked, an unknown Codex conversation only its labelled picker), Resume and Rerun send the right requests, terminal lines open, See all to grid, Repos and History (scoped, Back closes it), a repo line opens Repos expanded, Overview from the menu, Back and Forward and reload, Scratch by URL, fallbacks, empty project, phone width`,
    );
  } finally {
    await browser.close();
  }
}
