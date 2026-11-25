// Projects (test/projects.fixture.tsx), in Chromium and WebKit, against a
// fake bridge kept here, so its state outlives a reload:
// 1. The sidebar groups the terminals by project, the user's by name and
//    Scratch last, each group with its colour, its count and a menu.
// 2. A row dragged onto another project's group moves the terminal there:
//    the bridge records the move and a reload shows it in its new group.
//    The row menu's Move to project dialog does the same from the keyboard.
// 3. A project's menu: Terminals shows its grid only, with a way back to
//    every terminal; Files starts the columns at its folder and colours the
//    project folders; Recently closed lists its closed terminals only
//    (Scratch's: those of no existing project).
// 4. The file browser's "Create project from this folder…": absent on a
//    project's folder, refused with a clear message on a folder containing
//    one, and creating a project from a free folder adds its group.
// 5. The URL names the project; Back and Forward move between a project's
//    views and every terminal's.
// Set PROJECTS_SCREENSHOTS to a directory to save screenshots.
import { build } from "vite";
import { chromium, webkit, devices } from "playwright";
import { readFileSync, mkdirSync } from "node:fs";
import assert from "node:assert/strict";

const out =
  process.env.PROJECTS_FIXTURE_OUTPUT || "/tmp/kelpmux-projects-fixture";
const shots = process.env.PROJECTS_SCREENSHOTS;
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
      entry: new URL("./projects.fixture.tsx", import.meta.url).pathname,
      name: "ProjectsFixture",
      formats: ["iife"],
      cssFileName: "projects-fixture",
      fileName: () => "fixture.js",
    },
    minify: false,
  },
});
const origin = "https://terminal.test";

/** The fake bridge: the configuration's projects, the links and the
 * closed terminals, with every call recorded. */
function fakeBridge() {
  const state = {
    projects: [
      {
        id: "beta",
        name: "Beta",
        folder: "~/work/beta",
        path: "/home/me/work/beta",
        color: "green",
        missing: false,
      },
      {
        id: "alpha",
        name: "Alpha",
        folder: "~/work/alpha",
        path: "/home/me/work/alpha",
        color: "blue",
        missing: false,
      },
    ],
    links: { 11: "alpha", 12: "beta" },
    calls: [],
  };
  const closed = (key, name, project) => ({
    key,
    workspace: "main",
    pane: Number(key.split(":")[1]),
    cwd: "/home/me",
    name,
    kind: null,
    argv: [name],
    closed_at: 1_700_000_000,
    reason: "exited",
    choices: ["restart", "folder"],
    project,
  });
  const config = () => ({
    projects: state.projects,
    resolved: { scratch: { folder: "/home/me/scratch" } },
  });
  const panes = () => ({ panes: { ...state.links } });
  const resolve = (folder) =>
    folder.startsWith("~/") ? `/home/me/${folder.slice(2)}` : folder;
  return {
    state,
    call(method, params) {
      state.calls.push([method, params]);
      switch (method) {
        case "config":
          return config();
        case "projects":
          return panes();
        case "project_move":
          if (params.project) state.links[params.pane] = params.project;
          else delete state.links[params.pane];
          return panes();
        case "project_create": {
          const path = resolve(params.folder || `~/work/${params.name}`);
          const clash = state.projects.find(
            (p) =>
              p.path === path ||
              p.path.startsWith(`${path}/`) ||
              path.startsWith(`${p.path}/`),
          );
          if (clash)
            return {
              saved: false,
              errors: [
                {
                  key: `project[${state.projects.length}].folder`,
                  message: `${params.folder} overlaps the folder of project “${clash.name}”`,
                },
              ],
              config: config(),
            };
          const id = params.name.toLowerCase();
          state.projects.push({
            id,
            name: params.name,
            folder: params.folder,
            path,
            color: params.color,
            missing: false,
          });
          return { saved: true, project: id, config: config() };
        }
        case "closed":
          return {
            workspace: "main",
            entries: [
              closed("e:21", "cargo test", "alpha"),
              closed("e:22", "make", null),
              closed("e:23", "npm run dev", "beta"),
              closed("e:24", "old job", "deleted-project"),
            ],
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
    if (path === "/" || path.startsWith("/?"))
      return route.fulfill({
        contentType: "text/html; charset=utf-8",
        body: '<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover"><link rel="stylesheet" href="/projects-fixture.css"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>',
      });
    if (path === "/projects-fixture.css")
      return route.fulfill({
        contentType: "text/css; charset=utf-8",
        body:
          readFileSync(`${out}/projects-fixture.css`, "utf8") +
          ".fixture-grid{display:flex;flex-wrap:wrap;gap:8px;padding:12px}.fixture-tile{width:160px;height:90px;border:1px solid var(--border);border-radius:8px;padding:8px}",
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
async function shot(page, engine, name) {
  if (shots && engine === "chromium")
    await page.screenshot({ path: `${shots}/${name}.png` });
}
const groups = (page) =>
  page.$$eval("[data-project-group]", (all) =>
    all.map((group) => [
      group.dataset.projectGroup,
      [...group.querySelectorAll("[data-pane]")].map((row) =>
        Number(row.dataset.pane),
      ),
    ]),
  );
const search = (page) => page.evaluate(() => location.search);
const projectMenu = async (page, name, item) => {
  await page.getByRole("button", { name: `Menu for ${name}` }).click();
  await page.getByRole("menuitem", { name: item }).click();
};

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
    // 1. Grouped by project, by name, Scratch last.
    assert.deepEqual(await groups(page), [
      ["alpha", [11]],
      ["beta", [12]],
      ["scratch", [13, 14]],
    ]);
    const dot = await page
      .locator('[data-project-group="alpha"] .project-dot')
      .evaluate((el) => getComputedStyle(el).backgroundColor);
    assert.notEqual(dot, "rgba(0, 0, 0, 0)", "the project's colour");
    await shot(page, name, "desktop-dark");

    // 2. Drag terminal 13 (Scratch) onto Alpha's group.
    const row = page.locator('.rail [data-pane="13"]');
    const target = page.locator('[data-project-group="alpha"] .project-head');
    const from = await row.boundingBox();
    const to = await target.boundingBox();
    await page.mouse.move(from.x + 40, from.y + from.height / 2);
    await page.mouse.down();
    await page.mouse.move(from.x + 40, from.y + from.height / 2 - 12, {
      steps: 3,
    });
    await page.mouse.move(to.x + 40, to.y + to.height / 2, { steps: 8 });
    await page.mouse.up();
    await page.waitForFunction(
      () =>
        document.querySelector('[data-project-group="alpha"] [data-pane="13"]'),
    );
    assert.equal(server.state.links[13], "alpha", "the bridge has the move");
    await page.reload();
    await page.waitForFunction(() => window.fixtureReady === true);
    await page.waitForFunction(
      () =>
        document.querySelector('[data-project-group="alpha"] [data-pane="13"]'),
    );
    assert.deepEqual(await groups(page), [
      ["alpha", [11, 13]],
      ["beta", [12]],
      ["scratch", [14]],
    ]);
    // The keyboard: the row menu's Move to project, back to Scratch.
    await page.locator('.rail [data-pane="13"]').click({ button: "right" });
    const dialog = page.getByRole("dialog");
    assert.ok(await dialog.getByText("Move “shell” to project").isVisible());
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await page.waitForFunction(
      () =>
        document.querySelector('[data-project-group="scratch"] [data-pane="13"]'),
    );
    assert.equal(server.state.links[13], undefined);

    // 3. Alpha's terminals: its grid only, then every terminal.
    await projectMenu(page, "Alpha", "Terminals");
    assert.equal(await page.locator("[data-tile]").count(), 1);
    assert.match(await search(page), /view=grid&project=alpha/);
    await page.getByRole("button", { name: "All terminals" }).click();
    assert.equal(await page.locator("[data-tile]").count(), 4);
    // Alpha's files: the columns start at its folder.
    await projectMenu(page, "Alpha", "Files");
    await page.waitForSelector('.files-row[data-path="/home/me/work/alpha/src"]');
    assert.equal(
      await page.locator('.files-row[data-path="/home/me/work"]').count(),
      0,
      "nothing above the project's folder",
    );
    assert.match(await search(page), /view=files&dir=.*&project=alpha/);
    // Every file: the project folders in their colour, with a link.
    await page.getByRole("button", { name: "All files" }).click();
    await page.waitForSelector('.files-row[data-path="/home/me/work"]');
    await page.locator('.files-row[data-path="/home/me/work"]').click();
    await page.waitForSelector('.files-row[data-project-folder="alpha"]');
    assert.deepEqual(
      (
        await page
          .locator(".files-row[data-project-folder]")
          .evaluateAll((rows) => rows.map((row) => row.dataset.projectFolder))
      ).sort(),
      ["alpha", "beta", "scratch"],
      "Scratch's folder too",
    );
    await shot(page, name, "files-projects");
    await page.locator('.files-row[data-path="/home/me/work/beta"]').click();
    await page.locator(".files-project-link").click();
    assert.match(await search(page), /project=beta/);
    await page.getByRole("button", { name: "All files" }).click();

    // 4. Create project from this folder.
    await page
      .locator('.files-row[data-path="/home/me/work/alpha"]')
      .click({ button: "right" });
    assert.equal(
      await page
        .getByRole("menuitem", { name: "Create project from this folder…" })
        .count(),
      0,
      "not on a project's folder",
    );
    await page.keyboard.press("Escape");
    await page
      .locator('.files-row[data-path="/home/me/work"]')
      .click({ button: "right" });
    await page
      .getByRole("menuitem", { name: "Create project from this folder…" })
      .click();
    await page.getByRole("button", { name: "Create project" }).click();
    assert.match(
      await page.locator('[data-project-problems="folder"]').innerText(),
      /contains the folder of “(Alpha|Beta)”/,
    );
    assert.ok(
      !server.state.calls.some(([method]) => method === "project_create"),
      "refused before asking the bridge",
    );
    await page.getByRole("button", { name: "Cancel" }).click();
    await page
      .locator('.files-row[data-path="/home/me/work/gamma"]')
      .click({ button: "right" });
    await page
      .getByRole("menuitem", { name: "Create project from this folder…" })
      .click();
    assert.equal(
      await page.getByLabel("Project name").inputValue(),
      "gamma",
    );
    await page.getByLabel("Project name").fill("Gamma");
    await page.getByRole("button", { name: "purple" }).click();
    await shot(page, name, "create-project");
    await page.getByRole("button", { name: "Create project" }).click();
    await page.waitForSelector('[data-project-group="gamma"]');
    assert.deepEqual(
      server.state.calls.find(([method]) => method === "project_create")[1],
      { name: "Gamma", folder: "/home/me/work/gamma", color: "purple" },
    );
    await page.waitForSelector('.files-row[data-project-folder="gamma"]');

    // 3. Recently closed, by project.
    await projectMenu(page, "Alpha", "Recently closed");
    await page.waitForSelector('[data-dialog="recovery"] .list-row');
    assert.deepEqual(
      await page.locator('[data-dialog="recovery"] .list-row').allInnerTexts().then((all) => all.map((t) => t.split("\n")[0])),
      ["cargo test"].map((t) => t),
    );
    assert.match(await search(page), /dialog=closed&project=alpha/);
    await page.locator("#close-dialog").click();
    await projectMenu(page, "Scratch", "Recently closed");
    await page.waitForSelector('[data-dialog="recovery"] .list-row');
    assert.equal(
      await page.locator('[data-dialog="recovery"] .list-row').count(),
      2,
      "no project and a deleted one",
    );
    await page.getByRole("button", { name: "All projects" }).click();
    assert.equal(
      await page.locator('[data-dialog="recovery"] .list-row').count(),
      4,
    );
    await page.locator("#close-dialog").click();

    // 5. Back and Forward between a project's views.
    await projectMenu(page, "Beta", "Terminals");
    assert.match(await search(page), /view=grid&project=beta/);
    await page.locator(".rail").getByRole("button", { name: "Grid" }).click();
    assert.doesNotMatch(await search(page), /project=/);
    assert.equal(await page.locator("[data-tile]").count(), 4);
    await page.goBack();
    await page.waitForFunction(() => location.search.includes("project=beta"));
    await page.waitForFunction(
      () => document.querySelectorAll("[data-tile]").length === 1,
    );
    await page.goForward();
    await page.waitForFunction(
      () => document.querySelectorAll("[data-tile]").length === 4,
    );
    // A deep link to a project that does not exist: every terminal.
    await page.goto(`${origin}/?view=grid&project=nope`);
    await page.waitForFunction(() => window.fixtureReady === true);
    await page.waitForFunction(() => !location.search.includes("project="));
    assert.equal(await page.locator("[data-tile]").count(), 4);
    assert.deepEqual(errors, []);
    await context.close();

    // Screenshots: light, and a phone (no sidebar; the project's files).
    if (shots && name === "chromium") {
      const light = await open(
        browser,
        { viewport: { width: 1280, height: 800 } },
        server,
        "?mode=light",
      );
      await shot(light.page, name, "desktop-light");
      await light.page
        .locator('[data-project-group="alpha"] .project-head')
        .screenshot({ path: `${shots}/sidebar-head.png` });
      await light.page
        .locator(".rail")
        .screenshot({ path: `${shots}/sidebar-projects.png` });
      await light.context.close();
      const { defaultBrowserType, ...iphone } = devices["iPhone 13"];
      const mobile = await open(
        browser,
        iphone,
        server,
        "?phone=1&view=files&dir=/home/me/work&project=alpha",
      );
      await mobile.page.waitForSelector(".files-row");
      await shot(mobile.page, name, "phone");
      await mobile.context.close();
    }
    console.log(
      `PASS: ${name} projects: sidebar grouped by project (by name, Scratch last), a dragged row moves its terminal and the bridge keeps it across a reload, Move to project from the keyboard, a project's grid, files and Recently closed, coloured project folders with links, Create project from this folder (refused overlap, created group), project URLs with Back and Forward, unknown project falls back`,
    );
  } finally {
    await browser.close();
  }
}
