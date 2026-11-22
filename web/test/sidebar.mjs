// The sidebar and per-terminal notifications (test/sidebar.fixture.tsx), in
// Chromium and WebKit, without any network or SSH endpoint:
// 1. No full width mode: no sidebar entry and no edge handle.
// 2. A tab with unread notifications shows a bell on its sidebar row and
//    grid tile; the bell in the terminal's header opens that terminal's
//    notifications only, where one or all of them are marked read. There is
//    no global Notifications entry.
// 3. The layout: no separator under the logo, the tools in two columns
//    above the vitals, the account and settings below the vitals at the very
//    bottom, all fitting 800 px with ten tabs; one column when folded; the
//    same on an iPad in portrait and a phone's terminal list.
// Set SIDEBAR_SCREENSHOTS to a directory to save screenshots of each case.
import { build } from "vite";
import { chromium, webkit, devices } from "playwright";
import { readFileSync, mkdirSync } from "node:fs";
import assert from "node:assert/strict";

const out =
  process.env.SIDEBAR_FIXTURE_OUTPUT || "/tmp/kelpmux-sidebar-fixture";
const shots = process.env.SIDEBAR_SCREENSHOTS;
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
      entry: new URL("./sidebar.fixture.tsx", import.meta.url).pathname,
      name: "SidebarFixture",
      formats: ["iife"],
      cssFileName: "sidebar-fixture",
      fileName: () => "fixture.js",
    },
    minify: false,
  },
});
const origin = "https://terminal.test";
async function serve(context) {
  await context.route(`${origin}/**`, (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/")
      return route.fulfill({
        contentType: "text/html; charset=utf-8",
        body: '<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover"><link rel="stylesheet" href="/sidebar-fixture.css"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>',
      });
    if (path.endsWith(".css"))
      return route.fulfill({
        contentType: "text/css; charset=utf-8",
        body: readFileSync(`${out}/sidebar-fixture.css`),
      });
    if (path === "/fixture.js")
      return route.fulfill({
        contentType: "text/javascript; charset=utf-8",
        body: readFileSync(`${out}/fixture.js`),
      });
    return route.abort();
  });
}
function emulate(name) {
  const { defaultBrowserType, ...descriptor } = devices[name];
  return descriptor;
}
async function open(browser, options, search = "") {
  const context = await browser.newContext(options);
  await serve(context);
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(`${origin}/${search}`);
  await page.waitForFunction(() => window.fixtureReady === true);
  return { context, page, errors };
}
const box = (page, selector) => page.locator(selector).first().boundingBox();
async function shot(page, engine, name) {
  if (shots && engine === "chromium")
    await page.screenshot({ path: `${shots}/${name}.png` });
}

/** The bottom of the sidebar: two columns above the vitals, the account
 * below them, nothing overflowing. */
async function checkBottom(page, columns) {
  const rail = await box(page, ".rail");
  const tools = await page.locator(".tool-grid .tool-item").all();
  const boxes = await Promise.all(tools.map((t) => t.boundingBox()));
  const lefts = [...new Set(boxes.map((b) => Math.round(b.x)))];
  assert.equal(lefts.length, columns, `tools in ${columns} column(s)`);
  const grid = await box(page, ".tool-grid");
  const vitals = await box(page, ".vitals");
  const account = await box(page, ".account");
  assert.ok(grid.y + grid.height <= vitals.y + 1, "tools above the vitals");
  assert.ok(vitals.y + vitals.height <= account.y + 1, "account below vitals");
  assert.ok(
    account.y + account.height <= rail.y + rail.height + 1 &&
      rail.y + rail.height - (account.y + account.height) < 16,
    "account at the very bottom",
  );
  // The tools fit without their own scrolling.
  assert.ok(
    await page
      .locator(".rail-bottom")
      .evaluate((el) => el.scrollHeight <= el.clientHeight + 1),
    "tools fit",
  );
}

for (const engine of [chromium, webkit]) {
  const browser = await engine.launch({ headless: true });
  const name = engine.name();
  try {
    // Desktop, dark: layout, then the notifications flow.
    {
      const { context, page, errors } = await open(browser, {
        viewport: { width: 1440, height: 900 },
      });
      await shot(page, name, "desktop-dark");
      // 1. No full width mode.
      assert.equal(
        await page.getByRole("button", { name: "Full width" }).count(),
        0,
      );
      assert.equal(await page.locator(".sidebar-restore").count(), 0);
      // No global Notifications entry.
      assert.equal(
        await page.locator('[data-tool="notifications"]').count(),
        0,
      );
      // 3. No separator under the logo.
      assert.equal(
        await page
          .locator(".rail-head")
          .evaluate((el) => getComputedStyle(el).borderBottomWidth),
        "0px",
      );
      await checkBottom(page, 2);
      // Short, readable labels.
      assert.deepEqual(
        await page.locator(".tool-grid .tool-item .grow").allTextContents(),
        ["Grid", "Terminals", "Closed", "Search", "Help"],
      );
      // 2. Unread bells on tabs 2 (two unread) and 5 (one), none elsewhere.
      const row = (id) => page.locator(`.term-row[data-pane="${id}"]`);
      assert.equal(await row(2).locator(".unread-mark").count(), 1);
      assert.equal(await row(2).locator(".unread-mark b").textContent(), "2");
      assert.equal(await row(5).locator(".unread-mark").count(), 1);
      assert.equal(await row(5).locator(".unread-mark b").count(), 0);
      assert.equal(await row(1).locator(".unread-mark").count(), 0);
      // The header's bell opens this terminal's notifications only.
      const bell = page.locator(".grid-pane > header .pane-notifications");
      assert.equal(await bell.getAttribute("data-unread"), "2");
      await bell.click();
      const dialog = page.locator('[data-dialog="notifications"]');
      await dialog.waitFor();
      assert.match(await dialog.locator("h2").textContent(), /claude 2/);
      await shot(page, name, "desktop-notifications");
      const rows = () => dialog.locator(".list-row");
      assert.deepEqual(
        await rows().evaluateAll((els) =>
          els.map((e) => e.dataset.notification),
        ),
        ["3", "2"],
      );
      // History adds its read one; notification 4 (tab 5) never appears.
      await dialog.getByRole("button", { name: "History" }).click();
      assert.deepEqual(
        await rows().evaluateAll((els) =>
          els.map((e) => e.dataset.notification),
        ),
        ["3", "2", "1"],
      );
      await dialog.getByRole("button", { name: /^Unread/ }).click();
      // Mark one read, then all of this terminal's.
      const markThree = dialog.locator('[data-notification="3"] .icon-btn');
      assert.equal(
        await markThree.getAttribute("aria-label"),
        "Mark “Needs input · 3” read",
      );
      await markThree.click();
      assert.deepEqual(await page.evaluate(() => window.acks), [3]);
      assert.equal(await row(2).locator(".unread-mark b").count(), 0);
      await dialog.getByRole("button", { name: "Mark all read" }).click();
      assert.deepEqual(await page.evaluate(() => window.acks), [3, 2]);
      assert.equal(await rows().count(), 0);
      assert.equal(await row(2).locator(".unread-mark").count(), 0);
      assert.equal(await bell.getAttribute("data-unread"), null);
      // Tab 5 keeps its bell: acknowledging is per terminal.
      assert.equal(await row(5).locator(".unread-mark").count(), 1);
      assert.deepEqual(errors, []);
      await context.close();
    }
    // Desktop, light; then 800 px tall with ten tabs; then folded.
    {
      const { context, page, errors } = await open(
        browser,
        { viewport: { width: 1440, height: 900 } },
        "?mode=light",
      );
      await shot(page, name, "desktop-light");
      await checkBottom(page, 2);
      assert.deepEqual(errors, []);
      await context.close();
    }
    {
      const { context, page, errors } = await open(
        browser,
        { viewport: { width: 1280, height: 800 } },
        "?tabs=10",
      );
      await shot(page, name, "desktop-800-10-tabs");
      await checkBottom(page, 2);
      // Every tab stays visible above the tools.
      const list = await box(page, ".rail-scroll");
      const last = await box(page, '.term-row[data-pane="10"]');
      assert.ok(
        last.y + last.height <= list.y + list.height + 1,
        "ten tabs fit",
      );
      assert.deepEqual(errors, []);
      await context.close();
    }
    {
      const { context, page, errors } = await open(
        browser,
        { viewport: { width: 1440, height: 900 } },
        "?folded=1&view=grid",
      );
      await shot(page, name, "desktop-folded-grid");
      await checkBottom(page, 1);
      // The grid tiles carry the bell too.
      assert.equal(
        await page.locator('[data-tile-pane="2"] header .unread-mark').count(),
        1,
      );
      assert.equal(
        await page.locator('[data-tile-pane="1"] header .unread-mark').count(),
        0,
      );
      assert.deepEqual(errors, []);
      await context.close();
    }
    // iPad in portrait: the sidebar starts folded; unfolded it has the same
    // two columns.
    {
      const { context, page, errors } = await open(
        browser,
        emulate("iPad (gen 11)"),
      );
      await shot(page, name, "ipad-portrait-folded");
      await checkBottom(page, 1);
      await page.locator(".sidebar-fold").click();
      await shot(page, name, "ipad-portrait");
      await checkBottom(page, 2);
      assert.deepEqual(errors, []);
      await context.close();
    }
    // Phone: the terminal list has the same bottom (no Grid); a terminal's
    // bar has its bell.
    {
      const { context, page, errors } = await open(
        browser,
        emulate("iPhone 15"),
      );
      await shot(page, name, "phone-list");
      assert.equal(await page.locator('[data-tool="grid"]').count(), 0);
      const lefts = await page
        .locator(".tool-grid .tool-item")
        .evaluateAll((els) => [
          ...new Set(els.map((e) => Math.round(e.getBoundingClientRect().x))),
        ]);
      assert.equal(lefts.length, 2, "two columns on the phone too");
      assert.equal(
        await page.locator('.term-row[data-pane="2"] .unread-mark').count(),
        1,
      );
      await page.locator('.term-row[data-pane="2"] .term-item').click();
      const bell = page.locator(".phone-bar .pane-notifications");
      assert.equal(await bell.getAttribute("data-unread"), "2");
      await shot(page, name, "phone-terminal");
      await bell.click();
      await page.locator('[data-dialog="notifications"]').waitFor();
      await shot(page, name, "phone-notifications");
      assert.deepEqual(errors, []);
      await context.close();
    }
    console.log(
      `PASS: ${name} sidebar: no full width, per-terminal notifications, two-column tools above the vitals, account at the bottom`,
    );
  } finally {
    await browser.close();
  }
}
