// A project's Environments view (test/environments.fixture.tsx), in
// Chromium and WebKit, against a fake bridge kept here:
// 1. The sidebar tags the terminal that runs an environment; Scratch's menu
//    has no Environments.
// 2. The project menu's Environments opens the view
//    (`?view=envs&project=alpha`): each environment with its kind, state
//    (the terminal state marks: Running, Stopped, Failed, Invalid, Not
//    checked until Check status runs a deployment's status script), URL,
//    port, its problem or detail, and Start/Stop as its state allows.
// 3. Start asks the bridge with a request identity; the row goes Starting,
//    then Running as the bridge reports; its terminal opens from the row;
//    Stop asks the bridge and the row goes Stopped. A refused Start shows
//    the bridge's error.
// 4. Nothing is started without a click; Back leaves the view; a URL
//    without a project, or for Scratch, falls back to the grid; a project
//    without environments says how to declare one; a phone's width fits.
// Set ENVIRONMENTS_SCREENSHOTS to a directory to save screenshots.
import { build } from "vite";
import { chromium, webkit, devices } from "playwright";
import { readFileSync, mkdirSync } from "node:fs";
import assert from "node:assert/strict";

const out = process.env.ENVIRONMENTS_FIXTURE_OUTPUT || "/tmp/kelpmux-environments-fixture";
const shots = process.env.ENVIRONMENTS_SCREENSHOTS;
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
      entry: new URL("./environments.fixture.tsx", import.meta.url).pathname,
      name: "EnvironmentsFixture",
      formats: ["iife"],
      cssFileName: "environments-fixture",
      fileName: () => "fixture.js",
    },
    minify: false,
  },
});
const origin = "https://terminal.test";

const environment = (fields) => ({
  kind: "process",
  description: null,
  root: "/home/me/work/alpha/app",
  repo: "app",
  dir: "/home/me/work/alpha/app/.kelp/environments/x",
  slug: `alpha-${fields.name}`,
  port: null,
  logs: true,
  state: "stopped",
  detail: null,
  url: null,
  pane: null,
  action: null,
  error: null,
  ...fields,
});

/** The fake bridge: Alpha with three environments, Beta with none. `web`
 * starts (Starting, then Running two lists later) and stops; `api`
 * refuses to start. Every call is recorded. */
function fakeBridge() {
  const state = { calls: [], web: "stopped", lists: 0, envPanes: { 14: { project: "alpha", env: "web", action: "process" } } };
  const projects = [
    { id: "alpha", name: "Alpha", folder: "~/work/alpha", path: "/home/me/work/alpha", color: "blue", missing: false },
    { id: "beta", name: "Beta", folder: "~/work/beta", path: "/home/me/work/beta", color: "green", missing: false },
  ];
  const alpha = () => {
    state.lists += 1;
    if (state.web === "starting" && state.lists > state.startedAt + 1) state.web = "running";
    const live = state.web === "starting" || state.web === "running";
    return {
      project: "alpha",
      folder: "/home/me/work/alpha",
      exists: true,
      truncated: false,
      environments: [
        environment({
          name: "api",
          description: "API dev server",
          state: "failed",
          detail: "exited with status 1",
          port: 41001,
          url: "http://localhost:41001/",
        }),
        environment({
          name: "broken",
          kind: null,
          state: "invalid",
          error: "a process environment needs `command`",
          logs: false,
        }),
        environment({
          name: "staging",
          kind: "deploy",
          root: "/home/me/work/alpha/infra",
          repo: "infra",
          // Listing never runs its status script: unknown, with the last
          // explicit check once there is one.
          state: "unknown",
          url: state.checked ? state.checked.url : null,
          checked: state.checked,
          port: 41002,
          logs: false,
        }),
        environment({
          name: "web",
          description: "Vite dev server",
          state: state.web,
          port: state.web === "stopped" && !state.startedAt ? null : 41000,
          url: state.web === "stopped" && !state.startedAt ? null : "http://localhost:41000/",
          pane: live ? 14 : null,
          action: live ? "process" : null,
        }),
      ],
    };
  };
  return {
    state,
    call(method, params) {
      state.calls.push([method, params]);
      switch (method) {
        case "config":
          return { projects, resolved: { scratch: { folder: "/home/me/scratch" } } };
        case "projects":
          return { panes: { 11: "alpha", 14: "alpha" } };
        case "environment_panes":
          return { panes: state.envPanes };
        case "environments":
          if (params.project === "alpha") return alpha();
          if (params.project === "beta")
            return { project: "beta", folder: "/home/me/work/beta", exists: true, truncated: false, environments: [] };
          throw Error("this project no longer exists");
        case "environment_start":
          if (params.name === "api") throw Error("environment `api` cannot run: port busy");
          state.web = "starting";
          state.startedAt = state.lists;
          return { pane: 14, action: "process", state: "starting", port: 41000, slug: "alpha-web" };
        case "environment_stop":
          state.web = "stopped";
          return { state: "stopped", pane: 14 };
        case "environment_check":
          state.checked = {
            state: "running",
            url: "https://alpha-staging.example.test/",
            detail: "3 pods ready",
            at: Math.floor(Date.now() / 1000),
          };
          return { name: params.name, state: "running", checked: state.checked };
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
        body: '<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover"><link rel="stylesheet" href="/environments-fixture.css"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>',
      });
    if (path === "/environments-fixture.css")
      return route.fulfill({
        contentType: "text/css; charset=utf-8",
        body: readFileSync(`${out}/environments-fixture.css`, "utf8"),
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
const calls = (server, method) =>
  server.state.calls.filter(([m]) => m === method);
const projectMenu = async (page, name, item) => {
  await page.getByRole("button", { name: `Menu for ${name}` }).click();
  await page.getByRole("menuitem", { name: item }).click();
};
const row = (page, name) => page.locator(`.env-row[data-env="${name}"]`);
async function stateOf(page, name, state) {
  await page.locator(`.env-row[data-env="${name}"][data-state="${state}"]`).waitFor({ timeout: 15000 });
}

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
    // 1. The environment's terminal is tagged in the sidebar.
    const tagged = page.locator('.term-row[data-pane="14"]');
    assert.equal(await tagged.getAttribute("data-environment"), "web");
    assert.match(await tagged.locator(".env-pane-tag").innerText(), /env web/);
    assert.equal(await page.locator('.term-row[data-pane="11"] .env-pane-tag').count(), 0);
    await page.getByRole("button", { name: "Menu for Scratch" }).click();
    assert.equal(await page.getByRole("menuitem", { name: "Environments" }).count(), 0);
    await page.keyboard.press("Escape");
    assert.equal(calls(server, "environments").length, 0, "nothing asked before the view");

    // 2. The view from the project menu.
    await projectMenu(page, "Alpha", "Environments");
    await row(page, "web").waitFor();
    assert.equal(await search(page), "?view=envs&project=alpha");
    assert.deepEqual(
      await page.$$eval(".env-row", (rows) => rows.map((r) => [r.dataset.env, r.dataset.state])),
      [
        ["api", "failed"],
        ["broken", "invalid"],
        ["staging", "unknown"],
        ["web", "stopped"],
      ],
    );
    const api = await row(page, "api").innerText();
    assert.match(api, /process/);
    assert.match(api, /Failed/);
    assert.match(api, /port 41001/);
    assert.match(api, /exited with status 1/);
    // A deployment is not checked until asked: no status script ran.
    assert.match(await row(page, "staging").innerText(), /deploy[\s\S]*Not checked[\s\S]*infra/);
    assert.equal(await row(page, "staging").locator("svg.state-mark.state-idle").count(), 1);
    assert.equal(calls(server, "environment_check").length, 0, "nothing is checked on its own");
    await row(page, "staging").getByRole("button", { name: "Check the status of staging" }).click();
    await page.waitForFunction(() =>
      /checked just now/.test(document.querySelector('.env-row[data-env="staging"]')?.textContent || ""),
    );
    assert.deepEqual(calls(server, "environment_check").pop()[1], { project: "alpha", name: "staging" });
    assert.equal(calls(server, "environment_check").length, 1);
    assert.equal(
      await row(page, "staging").locator(".env-url").getAttribute("href"),
      "https://alpha-staging.example.test/",
    );
    assert.match(
      await row(page, "staging").innerText(),
      /deploy[\s\S]*Running · checked just now[\s\S]*infra[\s\S]*3 pods ready/,
    );
    assert.match(await row(page, "broken").innerText(), /needs `command`/);
    assert.equal(await row(page, "broken").locator("button").count(), 0, "an invalid one has no actions");
    // The state marks are the terminals'.
    assert.equal(await row(page, "staging").locator("svg.state-mark.state-finished").count(), 1);
    assert.equal(await row(page, "api").locator("svg.state-mark.state-failed").count(), 1);
    assert.equal(await row(page, "web").locator("svg.state-mark.state-idle").count(), 1);
    // A deploy environment deploys again and takes down; a stopped process starts.
    assert.ok(await row(page, "staging").getByRole("button", { name: "Deploy staging" }).isVisible());
    assert.ok(await row(page, "staging").getByRole("button", { name: "Take down staging" }).isVisible());
    assert.equal(await row(page, "web").getByRole("button", { name: "Stop web" }).count(), 0);
    assert.equal(calls(server, "environment_start").length, 0, "nothing starts on its own");
    if (shots && name === "chromium") await page.screenshot({ path: `${shots}/environments-dark.png` });

    // 3. Start, Starting, Running, open its terminal, Stop.
    await row(page, "web").getByRole("button", { name: "Start web" }).click();
    await stateOf(page, "web", "starting");
    const start = calls(server, "environment_start").pop()[1];
    assert.equal(start.project, "alpha");
    assert.equal(start.name, "web");
    assert.match(start.request, /^web-env-start-web-[A-Za-z0-9_-]+$/);
    assert.match(await row(page, "web").innerText(), /Starting/);
    assert.equal(await row(page, "web").locator("svg.state-mark.state-working").count(), 1);
    if (shots && name === "chromium") await page.screenshot({ path: `${shots}/environments-starting.png` });
    await stateOf(page, "web", "running");
    assert.match(await row(page, "web").innerText(), /http:\/\/localhost:41000\/[\s\S]*port 41000/);
    await row(page, "web").getByRole("button", { name: "Open the terminal of web" }).click();
    assert.deepEqual(await page.evaluate(() => window.opened), [14]);
    if (shots && name === "chromium") await page.screenshot({ path: `${shots}/environments-running.png` });
    await row(page, "web").getByRole("button", { name: "Stop web" }).click();
    await stateOf(page, "web", "stopped");
    assert.deepEqual(
      Object.keys(calls(server, "environment_stop").pop()[1]).sort(),
      ["name", "project", "request"],
    );
    // A refused Start shows the bridge's error.
    await row(page, "api").getByRole("button", { name: "Start api" }).click();
    const alert = page.locator(".envs-section .notice.error");
    await alert.waitFor();
    assert.match(await alert.innerText(), /port busy/);

    // 4. Back leaves the view; another project's view is empty.
    await page.goBack();
    await page.waitForFunction(() => !location.search.includes("view=envs"));
    await projectMenu(page, "Beta", "Environments");
    await page.getByText("No environments").waitFor();
    assert.match(await page.locator(".envs-section").innerText(), /\.kelp\/environments\/<name>\/environment\.toml/);
    assert.deepEqual(errors, []);
    await context.close();

    for (const url of ["?view=envs", "?view=envs&project=scratch", "?view=envs&project=gone"]) {
      const fallback = await open(browser, { viewport: { width: 1280, height: 800 } }, server, url);
      await fallback.page.waitForFunction(() => location.search === "?view=grid");
      assert.deepEqual(fallback.errors, []);
      await fallback.context.close();
    }

    if (shots && name === "chromium") {
      const light = await open(browser, { viewport: { width: 1280, height: 800 } }, server, "?mode=light&view=envs&project=alpha");
      await row(light.page, "staging").waitFor();
      await light.page.screenshot({ path: `${shots}/environments-light.png` });
      await light.page.locator('.term-row[data-pane="14"]').screenshot({ path: `${shots}/sidebar-tag.png` });
      await light.context.close();
    }
    const { defaultBrowserType, ...iphone } = devices["iPhone 13"];
    const mobile = await open(browser, iphone, server, "?phone=1&view=envs&project=alpha");
    await row(mobile.page, "staging").waitFor();
    const overflow = await mobile.page.evaluate(() =>
      [...document.querySelectorAll(".env-row")].some(
        (r) => r.getBoundingClientRect().right > window.innerWidth,
      ),
    );
    assert.equal(overflow, false, "the environments fit a phone's width");
    if (shots && name === "chromium") await mobile.page.screenshot({ path: `${shots}/environments-phone.png` });
    await mobile.context.close();
    console.log(
      `PASS: ${name} environments: sidebar tag, menu entry (not for Scratch), list with kinds, states and marks, URLs and ports, problems, deployments not checked until Check status, Start (request identity) through Starting to Running, open its terminal, Stop, refused Start error, Back, fallbacks, empty project, phone width`,
    );
  } finally {
    await browser.close();
  }
}
