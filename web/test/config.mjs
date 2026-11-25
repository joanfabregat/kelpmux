// Settings → Files & folders (test/config.fixture.tsx), in Chromium and
// WebKit, against a fake bridge, without any network or SSH endpoint:
// 1. The configuration's values are shown; nothing is sent until edited.
// 2. Only the changed keys are saved; a refused value shows its problem
//    under its field and nothing is reported saved.
// 3. Extra roots are added and removed as a list.
// 4. A `Config` notice with problems shows the banner with their positions
//    and marks the section, until a valid one replaces it; an unedited form
//    follows changes made elsewhere.
// 5. A saved root that differs from the running server's says a workspace
//    restart applies it.
// CONFIG_SCREENSHOTS=DIR saves screenshots of each state there.
import { build } from "vite";
import { chromium, webkit } from "playwright";
import { mkdirSync, readFileSync } from "node:fs";
import assert from "node:assert/strict";

const out = process.env.CONFIG_FIXTURE_OUTPUT || "/tmp/kelpmux-config-fixture";
const shots = process.env.CONFIG_SCREENSHOTS;
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
      entry: new URL("./config.fixture.tsx", import.meta.url).pathname,
      name: "ConfigFixture",
      formats: ["iife"],
      cssFileName: "config-fixture",
      fileName: () => "fixture.js",
    },
    minify: false,
  },
});
const origin = "https://terminal.test";
async function open(browser, search = "") {
  const context = await browser.newContext({
    viewport: { width: 1000, height: 900 },
  });
  await context.route(`${origin}/**`, (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/")
      return route.fulfill({
        contentType: "text/html; charset=utf-8",
        body: '<html><head><meta charset="utf-8"><link rel="stylesheet" href="/config-fixture.css"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>',
      });
    if (path.endsWith(".css"))
      return route.fulfill({
        contentType: "text/css; charset=utf-8",
        body: readFileSync(`${out}/config-fixture.css`),
      });
    if (path === "/fixture.js")
      return route.fulfill({
        contentType: "text/javascript; charset=utf-8",
        body: readFileSync(`${out}/fixture.js`),
      });
    return route.abort();
  });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(`${origin}/${search}`);
  await page.waitForFunction(() => window.fixtureReady === true);
  return { context, page, errors };
}
const shot = async (page, name, engine) => {
  if (shots && engine === "chromium")
    await page.screenshot({ path: `${shots}/${name}.png`, fullPage: true });
};

for (const [engine, launcher] of [
  ["chromium", chromium],
  ["webkit", webkit],
]) {
  const browser = await launcher.launch();
  try {
    const { context, page, errors } = await open(browser);
    const field = (key) => page.locator(`[data-config-key="${key}"]`);
    // 1. Values as written in the file.
    assert.equal(await field("files.root").inputValue(), "~");
    assert.equal(await field("projects.base").inputValue(), "~/projects");
    assert.equal(await field("scratch.folder").inputValue(), "~/scratch");
    assert.equal(
      await page.getByLabel("Extra root 1", { exact: true }).inputValue(),
      "/mnt/data",
    );
    const saveButton = page.getByRole("button", { name: "Save", exact: true });
    assert.equal(await saveButton.isDisabled(), true);
    await shot(page, "1-files-and-folders", engine);

    // 2. A refused value: its problem under the field, nothing saved.
    await field("scratch.folder").fill("scratch");
    await saveButton.click();
    await page.waitForSelector('[data-config-problems="scratch.folder"]');
    assert.deepEqual(await page.evaluate(() => window.saved), [
      { "scratch.folder": "scratch" },
    ]);
    assert.equal(
      await field("scratch.folder").getAttribute("aria-invalid"),
      "true",
    );
    assert.equal(await page.locator(".config-saved").count(), 0);
    await shot(page, "2-inline-error", engine);

    // 3. The extra roots list; then a valid save.
    await field("scratch.folder").fill("/mnt/data/scratch");
    await page.getByLabel("New extra root").fill("/srv/shared");
    await page.getByLabel("New extra root").press("Enter");
    await page.getByRole("button", { name: "Remove /mnt/data" }).click();
    await saveButton.click();
    await page.waitForSelector(".config-saved");
    const saved = await page.evaluate(() => window.saved);
    assert.deepEqual(saved.at(-1), {
      "files.extra_roots": ["/srv/shared"],
      "scratch.folder": "/mnt/data/scratch",
    });
    assert.equal(await page.locator("[data-config-problems]").count(), 0);
    assert.equal(await saveButton.isDisabled(), true);

    // 4. A hand edit with a typo, pushed by the bridge.
    await page.evaluate(() =>
      window.push({
        ...window.current(),
        revision: "x",
        current: false,
        errors: [
          {
            key: null,
            line: 12,
            column: 1,
            message:
              "unknown key `scratch.floder`; did you mean `scratch.folder`?",
          },
        ],
      }),
    );
    const banner = page.locator(".config-problems");
    await banner.waitFor();
    assert.match(await banner.innerText(), /12:1/);
    assert.match(await banner.innerText(), /last valid configuration/);
    assert.equal(await page.locator(".settings-nav .config-alert").count(), 1);
    await shot(page, "3-file-error-banner", engine);
    // Fixed elsewhere, with a new value: the banner goes, the form follows.
    await page.evaluate(() => {
      const config = window.current();
      window.push({
        ...config,
        revision: "y",
        current: true,
        errors: [],
        values: {
          ...config.values,
          projects: { base: "/srv/shared/projects" },
        },
      });
    });
    await banner.waitFor({ state: "detached" });
    assert.equal(
      await field("projects.base").inputValue(),
      "/srv/shared/projects",
    );
    assert.equal(await page.locator(".settings-nav .config-alert").count(), 0);

    // 5. A new root applies on the next workspace start.
    assert.equal(await page.locator("[data-config-restart]").count(), 0);
    await field("files.root").fill("/srv");
    await field("projects.base").fill("/srv/projects");
    await field("scratch.folder").fill("/srv/scratch");
    await saveButton.click();
    await page.locator("[data-config-restart]").waitFor();
    await shot(page, "4-restart-needed", engine);
    assert.deepEqual(errors, []);
    await context.close();

    // Light mode, for the record.
    if (shots && engine === "chromium") {
      const light = await open(browser, "?mode=light");
      await shot(light.page, "5-light", engine);
      await light.context.close();
    }
  } finally {
    await browser.close();
  }
  console.log(`config: ${engine} ok`);
}
