// The sidebar and per-terminal notifications (test/sidebar.fixture.tsx), in
// Chromium and WebKit, without any network or SSH endpoint:
// 1. No full width mode: no sidebar entry and no edge handle.
// 2. A tab with unread notifications that is not shown has a bell on its
//    sidebar row and grid tile; showing the terminal, or focusing its tile,
//    marks them read (other tiles keep their bells), and one that arrives
//    while the window is in the background waits until it is looked at
//    again. The bell in a terminal's header opens that terminal's
//    notifications only, on History, where one or all are also marked read.
//    There is no global Notifications entry.
// 3. The layout: no separator under the logo, the tools in two columns
//    above the vitals, the account and settings below the vitals at the very
//    bottom, all fitting 800 px with ten tabs; one column when folded; the
//    same on an iPad in portrait and a phone's terminal list.
// 4. A Grid entry heads the tabs, selected while the grid shows (no tab is
//    then); a tab opens its terminal alone and the entry the grid. The
//    focused tile has no border. Folded, the entry is its icon; phones have
//    none.
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

/** The Grid entry: the first row above the tabs, a terminal row's height,
 * selected exactly while the grid shows, with no terminal tab selected
 * then. */
async function checkGridEntry(page, grid) {
  const entry = page.locator('.grid-row [data-view="grid"]');
  assert.equal(await entry.count(), 1, "one Grid entry");
  assert.equal(await entry.getAttribute("aria-current"), grid ? "page" : null);
  const own = await entry.boundingBox();
  const first = await box(page, ".term-row .term-item");
  assert.ok(own.y + own.height <= first.y + 1, "Grid above the tabs");
  assert.ok(Math.abs(own.height - first.height) <= 1, "a tab's height");
  assert.ok(Math.abs(own.x - first.x) <= 1, "aligned with the tabs");
  assert.equal(
    await page.locator('.term-row .term-item[aria-current="page"]').count(),
    grid ? 0 : 1,
    "selected tabs",
  );
  assert.equal(
    await page.locator(".term-row:has(> .current)").count(),
    grid ? 0 : 1,
  );
  // The selected entry's highlight is its cell's, as a tab's.
  const background = await page
    .locator(".grid-row")
    .evaluate((el) => getComputedStyle(el).backgroundColor);
  if (grid) assert.notEqual(background, "rgba(0, 0, 0, 0)", "Grid selected");
  else assert.equal(background, "rgba(0, 0, 0, 0)");
}
/** The focused grid tile has no border of its own: the same border as the
 * other tiles and no ring. */
async function checkTileBorder(page, focused, other) {
  await page.mouse.move(0, 0);
  const style = (id) =>
    page.locator(`[data-tile-pane="${id}"]`).evaluate((el) => {
      const s = getComputedStyle(el);
      return {
        color: s.borderTopColor,
        width: s.borderTopWidth,
        shadow: s.boxShadow,
        outline: s.outlineStyle,
      };
    });
  const own = await style(focused);
  assert.equal(
    await page
      .locator(`[data-tile-pane="${focused}"]`)
      .getAttribute("aria-current"),
    "true",
  );
  assert.deepEqual(own, await style(other), "focused tile like the others");
  assert.equal(own.shadow, "none");
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
        ["Terminals", "Closed", "Search", "Help"],
      );
      // 4. The Grid entry heads the tabs, not selected in single view; the
      // shown terminal's tab is.
      await checkGridEntry(page, false);
      assert.equal(
        await page
          .locator('.term-row[data-pane="1"] .term-item')
          .getAttribute("aria-current"),
        "page",
      );
      // 2. Terminal 1 is shown. Unread bells on tabs 2 (two unread) and 5
      // (one), none elsewhere, and nothing marked read yet.
      const row = (id) => page.locator(`.term-row[data-pane="${id}"]`);
      const acks = () => page.evaluate(() => window.acks);
      assert.equal(await row(2).locator(".unread-mark").count(), 1);
      assert.equal(await row(2).locator(".unread-mark b").textContent(), "2");
      assert.equal(await row(5).locator(".unread-mark").count(), 1);
      assert.equal(await row(5).locator(".unread-mark b").count(), 0);
      assert.equal(await row(1).locator(".unread-mark").count(), 0);
      assert.deepEqual(await acks(), []);
      const bell = page.locator(".grid-pane > header .pane-notifications");
      assert.equal(await bell.getAttribute("data-unread"), null);
      // A terminal's notifications (its menu's Notifications…) open on
      // History; Unread and marking one or all read remain.
      await row(2).click({ button: "right" });
      const dialog = page.locator('[data-dialog="notifications"]');
      await dialog.waitFor();
      assert.match(await dialog.locator("h2").textContent(), /claude 2/);
      assert.equal(
        await dialog
          .getByRole("button", { name: "History" })
          .getAttribute("aria-pressed"),
        "true",
      );
      await shot(page, name, "desktop-notifications");
      const rows = () => dialog.locator(".list-row");
      const ids = () =>
        rows().evaluateAll((els) => els.map((e) => e.dataset.notification));
      // Its read one too; notification 4 (tab 5) never appears.
      assert.deepEqual(await ids(), ["3", "2", "1"]);
      await dialog.getByRole("button", { name: /^Unread/ }).click();
      assert.deepEqual(await ids(), ["3", "2"]);
      const markThree = dialog.locator('[data-notification="3"] .icon-btn');
      assert.equal(
        await markThree.getAttribute("aria-label"),
        "Mark “Needs input · 3” read",
      );
      await markThree.click();
      assert.deepEqual(await acks(), [3]);
      assert.equal(await row(2).locator(".unread-mark b").count(), 0);
      await dialog.getByRole("button", { name: "Mark all read" }).click();
      assert.deepEqual(await acks(), [3, 2]);
      assert.equal(await rows().count(), 0);
      assert.equal(await row(2).locator(".unread-mark").count(), 0);
      await dialog.getByRole("button", { name: "Close dialog" }).click();
      // Showing terminal 5 marks its notification read; the header's bell
      // has none left.
      assert.equal(await row(5).locator(".unread-mark").count(), 1);
      await row(5).locator(".term-item").click();
      await page.waitForFunction(() => window.acks.length === 3);
      assert.deepEqual(await acks(), [3, 2, 4]);
      await row(5).locator(".unread-mark").waitFor({ state: "detached" });
      assert.equal(await bell.getAttribute("data-unread"), null);
      // One arriving for the shown terminal is read at once; another
      // terminal's keeps its bell.
      await page.evaluate(() => window.notify(5));
      await page.waitForFunction(() => window.acks.length === 4);
      assert.deepEqual(await acks(), [3, 2, 4, 100]);
      await page.evaluate(() => window.notify(1));
      await page.waitForTimeout(100);
      assert.deepEqual(await acks(), [3, 2, 4, 100]);
      assert.equal(await row(1).locator(".unread-mark").count(), 1);
      // With the window in the background, an arrival keeps the shown
      // terminal's bell until the window is focused again.
      await page.evaluate(() => {
        document.hasFocus = () => false;
        window.dispatchEvent(new Event("blur"));
      });
      await page.evaluate(() => window.notify(5));
      await page.waitForTimeout(100);
      assert.equal(await row(5).locator(".unread-mark").count(), 1);
      assert.equal(await bell.getAttribute("data-unread"), "1");
      assert.equal((await acks()).length, 4);
      await page.evaluate(() => {
        document.hasFocus = () => true;
        window.dispatchEvent(new Event("focus"));
      });
      await page.waitForFunction(() => window.acks.length === 5);
      assert.deepEqual(await acks(), [3, 2, 4, 100, 102]);
      await row(5).locator(".unread-mark").waitFor({ state: "detached" });
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
    // 4. Grid view, three pages: the Grid entry is selected and no tab is;
    // the focused tile has no border; the pages take a row of the tools.
    // A tab opens its terminal alone, and the Grid entry the grid again.
    {
      const { context, page, errors } = await open(
        browser,
        { viewport: { width: 1440, height: 900 } },
        "?view=grid&pages=3",
      );
      await shot(page, name, "desktop-grid");
      await checkGridEntry(page, true);
      await checkTileBorder(page, 1, 2);
      await checkBottom(page, 2);
      assert.equal(await page.locator(".view-cell .grid-pages").count(), 1);
      await page.locator('.term-row[data-pane="3"] .term-item').click();
      await page.locator(".tab-pane-layout").waitFor();
      assert.equal(await page.locator(".live-grid").count(), 0);
      await checkGridEntry(page, false);
      assert.equal(
        await page
          .locator('.term-row[data-pane="3"] .term-item')
          .getAttribute("aria-current"),
        "page",
      );
      assert.equal(
        await page.locator(".tab-pane-layout .grid-title").textContent(),
        "codex 3",
      );
      assert.equal(await page.locator(".view-cell").count(), 0, "no pages");
      await page.locator('[data-view="grid"]').click();
      await page.locator(".live-grid").waitFor();
      await checkGridEntry(page, true);
      assert.equal(await page.locator(".view-cell .grid-pages").count(), 1);
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
      // The Grid entry is its icon, in the same place, selected.
      await checkGridEntry(page, true);
      const entry = page.locator('[data-view="grid"]');
      assert.ok(await entry.locator(".svg-icon").isVisible(), "Grid icon");
      assert.equal(await entry.locator(".grow").isVisible(), false);
      await checkTileBorder(page, 1, 3);
      // The grid tiles carry the bell too; focusing a tile marks its
      // notifications read, and the other tiles keep theirs.
      const tileBell = (id) =>
        page.locator(`[data-tile-pane="${id}"] header .unread-mark`);
      assert.equal(await tileBell(2).count(), 1);
      assert.equal(await tileBell(5).count(), 1);
      assert.equal(await tileBell(1).count(), 0);
      assert.deepEqual(await page.evaluate(() => window.acks), []);
      await page.locator('[data-tile-pane="2"]').click();
      await page.waitForFunction(() => window.acks.length === 2);
      assert.deepEqual(await page.evaluate(() => window.acks), [3, 2]);
      await tileBell(2).waitFor({ state: "detached" });
      assert.equal(await tileBell(5).count(), 1);
      await page.evaluate(() => window.notify(5));
      await page.waitForTimeout(100);
      assert.equal(await tileBell(5).locator("b").textContent(), "2");
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
      assert.equal(await page.locator('[data-view="grid"]').count(), 0);
      assert.equal(await page.locator(".grid-row").count(), 0);
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
      // The list shows no terminal: nothing is marked read until one opens.
      assert.deepEqual(await page.evaluate(() => window.acks), []);
      await page.locator('.term-row[data-pane="2"] .term-item').click();
      await page.waitForFunction(() => window.acks.length === 2);
      const bell = page.locator(".phone-bar .pane-notifications");
      await page
        .locator(".phone-bar .pane-notifications:not([data-unread])")
        .waitFor();
      await shot(page, name, "phone-terminal");
      await bell.click();
      const dialog = page.locator('[data-dialog="notifications"]');
      await dialog.waitFor();
      assert.equal(await dialog.locator(".list-row").count(), 3);
      await shot(page, name, "phone-notifications");
      assert.deepEqual(errors, []);
      await context.close();
    }
    console.log(
      `PASS: ${name} sidebar: no full width, showing a terminal marks it read, per-terminal notifications, two-column tools above the vitals, account at the bottom, Grid entry and view switching, no focused-tile border`,
    );
  } finally {
    await browser.close();
  }
}
