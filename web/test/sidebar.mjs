// The sidebar and per-terminal notifications (test/sidebar.fixture.tsx), in
// Chromium and WebKit, without any network or SSH endpoint:
// 1. No full width mode: no sidebar entry and no edge handle.
// 2. A tab with unread notifications that is not shown has a bell on its
//    sidebar row and grid tile; showing the terminal alone marks them read
//    (the read-only grid marks nothing read), and one that arrives while
//    the window is in the background waits until it is looked at again.
//    The bell in a terminal's header opens that terminal's notifications
//    only, on History, where one or all are also marked read. There is no
//    global Notifications entry.
// 3. The layout: no separator under the logo, the vitals above the tools
//    in two columns, the account and settings below the tools at the very
//    bottom, all fitting 800 px with ten tabs; one column when folded; the
//    same on an iPad in portrait and a phone's terminal list. The Terminals
//    tool's icon differs from the Terminals header's.
// 4. The Terminals header heads the tabs and opens the live grid, selected
//    while the grid shows (no tab is then). The grid is one read-only page:
//    3, 8, 10 and 13 terminals all fit with no pager, no tile is focused
//    and keys typed reach no terminal. A click on a tile, a tab and every
//    opening shortcut (Alt+digit, Ctrl-B digit, Tab, l, N) show the
//    terminal alone; the Terminals header, Alt+Tab, Ctrl+Shift+G and
//    Ctrl-B g / e the grid again. Folded, the header is its icon; on phones
//    it only expands and collapses the list.
// 5. Attention toasts.
// 6. Two collapsible sections, Terminals then Files: each header's label
//    opens its view (the grid, the file browser) and is selected while it
//    shows; its chevron, a separate button, expands or collapses it with
//    the mouse or Enter/Space, and the state survives a reload. Collapsed,
//    a header keeps its count and a dot for attention (Terminals) or
//    unsaved changes (Files). The folded rail ignores collapsing.
// 7. The Files section lists the home folder as a tree, rows like the
//    terminals' (no Open group: the open files are the Files view's tabs):
//    folders expand in place (kept across a reload), a file opens in the
//    main area and is selected, open and unsaved files are marked, and
//    every row has the file browser's menu (insert path, rename, trash,
//    open a folder in the browser). Folded, only the Files header shows;
//    phones show the tree with each row's menu button.
//    with each row's menu button.
// 8. One toast stack, top right: the attention toasts, then download
//    progress and confirmations, newest first, none overlapping, and on a
//    phone full width below the terminal bar. No rule under the section
//    headers; New terminal is a secondary row (smaller, muted, indented).
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

/** The bottom of the sidebar: the vitals above the two columns, the account
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
  assert.ok(vitals.y + vitals.height <= grid.y + 1, "vitals above the tools");
  assert.ok(grid.y + grid.height <= account.y + 1, "account below the tools");
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

/** The Terminals header, which opens the grid: the first row above the
 * tabs, a terminal row's height, selected exactly while the grid shows,
 * with no terminal tab selected then. */
async function checkTerminalsHeader(page, grid) {
  const entry = page.locator('.grid-row [data-view="grid"]');
  assert.equal(await entry.count(), 1, "one Terminals header");
  assert.equal(await entry.getAttribute("aria-current"), grid ? "page" : null);
  const own = await entry.boundingBox();
  const first = await box(page, ".term-row .term-item");
  assert.ok(own.y + own.height <= first.y + 1, "header above the tabs");
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
  if (grid) assert.notEqual(background, "rgba(0, 0, 0, 0)", "header selected");
  else assert.equal(background, "rgba(0, 0, 0, 0)");
}
/** Tabs look like the Terminals header (no colour stripe), a 1px rule
 * separates each terminal from the next but none is under a section
 * header, and New terminal is secondary: as wide as them but shorter,
 * smaller and muted, its small plus past their number column, its hover
 * brightening the text without filling the row. */
async function checkRowLook(page) {
  const rows = page.locator(".term-list:not([aria-label='Parked']) .term-row");
  assert.ok((await rows.count()) >= 2, "several tabs");
  assert.equal(
    await rows.first().evaluate((el) => getComputedStyle(el).boxShadow),
    "none",
    "no colour stripe",
  );
  const rule = (selector, pseudo) =>
    page
      .locator(selector)
      .first()
      .evaluate((el, p) => {
        const s = getComputedStyle(el, p);
        return { content: s.content, height: s.height };
      }, pseudo);
  const between = await rule(".term-row + .term-row", "::before");
  assert.notEqual(between.content, "none", "rule between terminals");
  assert.equal(between.height, "1px", "rule between terminals");
  for (const [selector, pseudo] of [
    [".grid-row", "::after"],
    [".grid-row", "::before"],
    [".term-row:first-child", "::before"],
    [".rail-new", "::before"],
    [".rail-new", "::after"],
  ])
    assert.equal(
      (await rule(selector, pseudo)).content,
      "none",
      `no rule at ${selector}${pseudo}`,
    );
  assert.equal(await page.locator(".rail-new-rule").count(), 0);
  // Secondary to the terminals' rows: shorter, smaller and muted, its
  // small plus indented past the number column, a hover that brightens
  // the text without filling the row.
  const button = await box(page, ".rail-new");
  const tab = await box(page, ".term-row .term-item");
  assert.ok(Math.abs(button.x - tab.x) <= 1, "New terminal aligned");
  assert.ok(Math.abs(button.width - tab.width) <= 1, "a tab's width");
  assert.ok(button.height < tab.height - 4, "shorter than a terminal's row");
  assert.equal(await page.locator(".rail-new .term-number").count(), 0);
  const plus = await box(page, ".rail-new .rail-new-plus");
  const number = await box(page, ".term-row .term-number");
  assert.ok(plus.x > number.x + number.width + 4, "plus past the numbers");
  assert.ok(plus.width < number.width, "a smaller plus");
  const style = (selector) =>
    page
      .locator(selector)
      .first()
      .evaluate((el) => {
        const s = getComputedStyle(el);
        return {
          color: s.color,
          size: s.fontSize,
          background: s.backgroundColor,
        };
      });
  const newRow = await style(".rail-new");
  const title = await style(".term-row .term-title");
  assert.notEqual(newRow.color, title.color, "New terminal muted");
  assert.ok(
    parseFloat(newRow.size) < parseFloat(title.size),
    `smaller text (${newRow.size} < ${title.size})`,
  );
  await page.locator(".rail-new").hover();
  const hovered = await style(".rail-new");
  assert.notEqual(hovered.color, newRow.color, "hover brightens the text");
  assert.equal(hovered.background, "rgba(0, 0, 0, 0)", "no filled hover");
  await page.mouse.move(0, 0);
  assert.equal(
    await page.locator(".rail-new").getAttribute("aria-label"),
    "New terminal",
  );
}
/** One grid page holds every terminal: a tile per terminal, each wholly
 * inside the grid and the viewport, columns × rows that fit them with no
 * empty row, no scrolling and no pager. */
async function checkGridFits(page, count) {
  const grid = page.locator(".live-grid");
  const area = await grid.boundingBox();
  const tiles = await page.locator(".live-tile").all();
  assert.equal(tiles.length, count, `${count} tiles`);
  const viewport = page.viewportSize();
  for (const tile of tiles) {
    const b = await tile.boundingBox();
    assert.ok(
      b.x >= area.x - 1 &&
        b.y >= area.y - 1 &&
        b.x + b.width <= Math.min(area.x + area.width, viewport.width) + 1 &&
        b.y + b.height <= Math.min(area.y + area.height, viewport.height) + 1,
      "tile inside the grid and the viewport",
    );
    assert.ok(b.height > 60 && b.width > 150, "tile large enough to read");
  }
  const cols = Number(await grid.getAttribute("data-cols"));
  const rows = Number(await grid.getAttribute("data-rows"));
  assert.ok(cols * rows >= count && cols * (rows - 1) < count, "no empty row");
  assert.ok(
    await grid.evaluate((el) => el.scrollHeight <= el.clientHeight + 1),
    "no scrolling",
  );
  assert.equal(await page.locator(".grid-pages, .grid-page").count(), 0);
  assert.equal(
    await page.getByRole("button", { name: /grid page/i }).count(),
    0,
    "no pager",
  );
}
/** No grid tile is focused: none is current, and every tile has the same
 * border and header as the others. */
async function checkNoFocus(page) {
  await page.mouse.move(0, 0);
  assert.equal(await page.locator(".live-tile[aria-current]").count(), 0);
  assert.equal(await page.locator(".live-tile.focused").count(), 0);
  const styles = await page.locator(".live-tile").evaluateAll((els) =>
    els.map((el) => {
      const s = getComputedStyle(el);
      const h = getComputedStyle(el.querySelector("header"));
      return `${s.borderTopWidth} ${s.boxShadow} ${h.backgroundColor}`;
    }),
  );
  // Tiles needing input have their own border colour, not width.
  assert.equal(new Set(styles).size, 1, "tiles alike");
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
      // 4. The Terminals header heads the tabs, not selected in single view; the
      // shown terminal's tab is.
      await checkTerminalsHeader(page, false);
      await checkRowLook(page);
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
    // 4. The read-only grid with 3, 8 and 13 terminals: one page with every
    // tile fully visible and no pager; the Terminals header is selected and no
    // tab is; no tile is focused and nothing is marked read.
    for (const tabs of [3, 8, 13]) {
      const { context, page, errors } = await open(
        browser,
        { viewport: { width: 1440, height: 900 } },
        `?view=grid&tabs=${tabs}`,
      );
      await shot(page, name, `desktop-grid-${tabs}`);
      await checkTerminalsHeader(page, true);
      await checkGridFits(page, tabs);
      await checkNoFocus(page);
      await checkBottom(page, 2);
      assert.deepEqual(await page.evaluate(() => window.acks), []);
      assert.deepEqual(errors, []);
      await context.close();
    }
    // A click anywhere on a tile opens it alone; a tab does too, and the
    // Terminals header the grid again. Keys typed in the grid reach no terminal;
    // every opening shortcut leaves the grid, and the grid shortcuts show
    // it again.
    {
      const { context, page, errors } = await open(
        browser,
        { viewport: { width: 1440, height: 900 } },
        "?view=grid",
      );
      // The Terminals tool's icon is not the Terminals header's.
      const icon = (selector) =>
        page.locator(`${selector} .svg-icon`).first().innerHTML();
      assert.notEqual(
        await icon('[data-tool="terminals"]'),
        await icon('[data-view="grid"]'),
        "Terminals tool and header icons differ",
      );
      await page.keyboard.type("echo grid\n");
      await page.keyboard.press("Control+c");
      assert.deepEqual(await page.evaluate(() => window.typed), []);
      assert.equal(await page.locator(".live-grid").count(), 1, "still grid");
      assert.equal(
        await page.locator(".live-grid textarea, .live-grid input").count(),
        0,
      );
      const title = () =>
        page.locator(".tab-pane-layout .grid-title").textContent();
      const showsAlone = async (number, label) => {
        await page.locator(".tab-pane-layout").waitFor();
        assert.equal(await page.locator(".live-grid").count(), 0, label);
        await checkTerminalsHeader(page, false);
        assert.equal(
          await page
            .locator(`.term-row[data-pane="${number}"] .term-item`)
            .getAttribute("aria-current"),
          "page",
          label,
        );
      };
      const backToGrid = async (keys) => {
        for (const key of keys) await page.keyboard.press(key);
        await page.locator(".live-grid").waitFor();
        await checkTerminalsHeader(page, true);
      };
      // A click near a tile's bottom corner, on its screen.
      const tile = await box(page, '[data-tile-pane="4"]');
      await page.mouse.click(
        tile.x + tile.width - 12,
        tile.y + tile.height - 12,
      );
      await showsAlone(4, "tile click");
      assert.equal(await title(), "shell");
      // The shown terminal gets the keys.
      await page.getByLabel("Input for terminal 4").focus();
      await page.keyboard.type("ok");
      assert.deepEqual(await page.evaluate(() => window.typed), ["o", "k"]);
      await page.locator('[data-view="grid"]').click();
      await page.locator(".live-grid").waitFor();
      await page.locator('.term-row[data-pane="3"] .term-item').click();
      await showsAlone(3, "tab");
      assert.equal(await title(), "codex 3");
      await backToGrid(["Alt+Tab"]);
      await page.keyboard.press("Alt+Digit2");
      await showsAlone(2, "Alt+2");
      await backToGrid(["Control+Shift+KeyG"]);
      await page.keyboard.press("Control+b");
      await page.keyboard.press("5");
      await showsAlone(5, "Ctrl-B 5");
      await backToGrid(["Control+b", "g"]);
      await page.keyboard.press("Control+b");
      await page.keyboard.press("Tab");
      await showsAlone(6, "Ctrl-B Tab");
      await backToGrid(["Control+b", "e"]);
      await page.keyboard.press("Control+b");
      await page.keyboard.press("l");
      await showsAlone(7, "Ctrl-B l");
      await backToGrid(["Alt+Tab"]);
      // The next question: terminal 8 needs input.
      await page.keyboard.press("Control+b");
      await page.keyboard.press("Shift+N");
      await showsAlone(8, "Ctrl-B N");
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
      // The Terminals header is its icon, in the same place, selected.
      await checkTerminalsHeader(page, true);
      const entry = page.locator('[data-view="grid"]');
      assert.ok(await entry.locator(".svg-icon").isVisible(), "header icon");
      assert.equal(await entry.locator(".grow").isVisible(), false);
      await checkGridFits(page, 10);
      await checkNoFocus(page);
      // The grid tiles carry the bell too. Nothing in the grid is marked
      // read; a tile's click opens its terminal alone, which marks its
      // notifications read, and the grid's other tiles keep theirs.
      const tileBell = (id) =>
        page.locator(`[data-tile-pane="${id}"] header .unread-mark`);
      assert.equal(await tileBell(2).count(), 1);
      assert.equal(await tileBell(5).count(), 1);
      assert.equal(await tileBell(1).count(), 0);
      await page.evaluate(() => window.notify(1));
      await page.waitForTimeout(100);
      assert.equal(await tileBell(1).count(), 1);
      assert.deepEqual(await page.evaluate(() => window.acks), []);
      await page.locator('[data-tile-pane="2"]').click();
      await page.locator(".tab-pane-layout").waitFor();
      await page.waitForFunction(() => window.acks.length === 2);
      assert.deepEqual(await page.evaluate(() => window.acks), [3, 2]);
      await page.locator('[data-view="grid"]').click();
      await page.locator(".live-grid").waitFor();
      assert.equal(await tileBell(2).count(), 0);
      assert.equal(await tileBell(5).count(), 1);
      await page.evaluate(() => window.notify(5));
      await page.waitForTimeout(100);
      assert.equal(await tileBell(5).locator("b").textContent(), "2");
      assert.deepEqual(await page.evaluate(() => window.acks), [3, 2]);
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
    // Phone: the terminal list has the same bottom (no grid: the Terminals
    // header only expands and collapses the list); a terminal's bar has
    // its bell.
    {
      const { context, page, errors } = await open(
        browser,
        emulate("iPhone 15"),
      );
      await shot(page, name, "phone-list");
      assert.equal(await page.locator('[data-view="grid"]').count(), 0);
      const header = page.locator('[data-section="terminals"] > .nav-item');
      assert.equal(await header.getAttribute("aria-expanded"), "true");
      assert.equal(
        await page
          .locator('[data-section="terminals"] .section-toggle')
          .count(),
        0,
        "the header is the toggle",
      );
      // Measured once layout has settled: the phone list slides in, and a
      // single early read sometimes caught a tool mid-transition (a third
      // x position).
      let lefts = [];
      for (let i = 0; i < 20; i += 1) {
        lefts = await page
          .locator(".tool-grid .tool-item")
          .evaluateAll((els) => [
            ...new Set(els.map((e) => Math.round(e.getBoundingClientRect().x))),
          ]);
        if (lefts.length === 2) break;
        await page.waitForTimeout(100);
      }
      assert.equal(
        lefts.length,
        2,
        `two columns on the phone too (${lefts.join(", ")})`,
      );
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
    // 6. The Terminals and Files sections (files=1): order, active
    // headers, chevrons by mouse and keyboard, the state kept across a
    // reload, and what a collapsed header still shows.
    {
      const { context, page, errors } = await open(
        browser,
        { viewport: { width: 1440, height: 900 } },
        "?files=1",
      );
      const section = (key) => page.locator(`[data-section="${key}"]`);
      const label = (key) => section(key).locator("> .nav-item");
      const toggle = (key) => section(key).locator(".section-toggle");
      const fileRows = page.locator(".file-tree .file-row");
      const fileRow = (path) =>
        page.locator(`.file-row[data-path="/home/alice/${path}"]`);
      const tabRows = page.locator(
        ".term-list[aria-label='Terminals'] .term-row",
      );
      const current = async () => ({
        terminals: await label("terminals").getAttribute("aria-current"),
        files: await label("files").getAttribute("aria-current"),
        tabs: await page
          .locator('.term-row .term-item[aria-current="page"]')
          .count(),
      });
      await shot(page, name, "sections-expanded");
      // Order: Terminals header, the tabs, + New terminal, then the Files
      // header and the home folder's tree (no Open group).
      assert.deepEqual(
        await page
          .locator("[data-section]")
          .evaluateAll((els) => els.map((e) => e.dataset.section)),
        ["terminals", "files"],
      );
      const y = async (locator) => (await locator.boundingBox()).y;
      assert.ok((await y(label("terminals"))) < (await y(tabRows.first())));
      assert.ok(
        (await y(tabRows.last())) < (await y(page.locator(".rail-new"))),
      );
      assert.ok(
        (await y(page.locator(".rail-new"))) < (await y(label("files"))),
      );
      assert.ok((await y(label("files"))) < (await y(fileRows.first())));
      assert.equal(await tabRows.count(), 10);
      assert.equal(await page.locator(".doc-row, .open-files").count(), 0);
      for (const key of ["terminals", "files"]) {
        assert.equal(await toggle(key).getAttribute("aria-expanded"), "true");
        assert.equal(await section(key).locator(".section-summary").count(), 0);
      }
      assert.equal(
        await page.locator('.grid-row [data-view="grid"]').count(),
        1,
        "no separate Grid entry",
      );
      // A terminal is shown: neither header is selected.
      assert.deepEqual(await current(), {
        terminals: null,
        files: null,
        tabs: 1,
      });
      // The Terminals label shows the grid; the Files label the browser;
      // a file's tree row the file.
      await label("terminals").click();
      await page.locator(".live-grid").waitFor();
      assert.deepEqual(await current(), {
        terminals: "page",
        files: null,
        tabs: 0,
      });
      await label("files").click();
      await page
        .locator('.files-placeholder[data-files-shown="browser"]')
        .waitFor();
      assert.deepEqual(await current(), {
        terminals: null,
        files: "page",
        tabs: 0,
      });
      await fileRow("app.py").locator(".file-item").click();
      await page
        .locator('.files-placeholder[data-files-shown="/home/alice/app.py"]')
        .waitFor();
      assert.equal(await label("files").getAttribute("aria-current"), null);
      assert.equal(
        await fileRow("app.py")
          .locator(".file-item")
          .getAttribute("aria-current"),
        "page",
      );
      // The chevron, a separate button, collapses Terminals without
      // changing the view, with Enter; collapsed, the header keeps the
      // count and the attention dot (terminals need input, others unread).
      await toggle("terminals").focus();
      await page.keyboard.press("Enter");
      assert.equal(
        await toggle("terminals").getAttribute("aria-expanded"),
        "false",
      );
      assert.equal(await tabRows.count(), 0, "tabs hidden");
      assert.equal(await page.locator(".rail-new").count(), 0);
      assert.ok(
        await page.locator(".files-placeholder").isVisible(),
        "view kept",
      );
      const terminalsSummary = section("terminals").locator(".section-summary");
      assert.equal(
        await terminalsSummary.locator(".count").textContent(),
        "10",
      );
      const terminalsDot = terminalsSummary.locator(".section-alert");
      assert.ok(await terminalsDot.isVisible(), "attention dot");
      assert.match(
        await terminalsDot.getAttribute("aria-label"),
        /need attention.*unread/,
      );
      assert.match(
        await label("terminals").getAttribute("aria-label"),
        /^Terminals: live grid \(10, .*need attention/,
      );
      await shot(page, name, "sections-terminals-collapsed");
      // Space expands it again; the label still navigates while collapsed.
      await page.keyboard.press("Space");
      assert.equal(await tabRows.count(), 10);
      await page.keyboard.press("Space");
      await label("terminals").click();
      await page.locator(".live-grid").waitFor();
      assert.equal(
        await toggle("terminals").getAttribute("aria-expanded"),
        "false",
        "still collapsed",
      );
      await toggle("terminals").click();
      assert.equal(await tabRows.count(), 10);
      // Files collapsed: its count and a dot for the unsaved file.
      await toggle("files").click();
      assert.equal(await fileRows.count(), 0, "tree hidden");
      const filesSummary = section("files").locator(".section-summary");
      assert.equal(await filesSummary.locator(".count").textContent(), "2");
      assert.ok(
        await filesSummary.locator(".section-alert").isVisible(),
        "unsaved dot",
      );
      await shot(page, name, "sections-files-collapsed");
      // Both collapsed survive a reload.
      await toggle("terminals").click();
      await page.reload();
      await page.waitForFunction(() => window.fixtureReady === true);
      for (const key of ["terminals", "files"])
        assert.equal(
          await toggle(key).getAttribute("aria-expanded"),
          "false",
          `${key} still collapsed`,
        );
      assert.equal(await tabRows.count(), 0);
      assert.equal(await fileRows.count(), 0);
      // With no unsaved file, a collapsed Files header has no dot.
      await toggle("files").click();
      await page.evaluate(() => window.closeDoc("/home/alice/app.py"));
      await toggle("files").click();
      assert.equal(await filesSummary.locator(".count").textContent(), "1");
      assert.equal(await filesSummary.locator(".section-alert").count(), 0);
      // The folded rail ignores collapsing: every tab shows as its icon,
      // with both sections still collapsed in this browser's preferences.
      await page.goto(`${origin}/?files=1&folded=1`);
      await page.waitForFunction(() => window.fixtureReady === true);
      assert.deepEqual(
        await page.evaluate(
          () =>
            JSON.parse(localStorage.getItem("kelpmux-web-preferences"))
              .sidebarSections,
        ),
        { terminals: false, files: false },
      );
      assert.equal(await tabRows.count(), 10);
      assert.equal(await page.locator(".section-toggle:visible").count(), 0);
      assert.deepEqual(errors, []);
      await context.close();
    }
    // Phone (files=1): the same two sections; the Terminals header toggles
    // the list, the Files label opens the browser.
    {
      const { context, page, errors } = await open(
        browser,
        emulate("iPhone 15"),
        "?files=1",
      );
      const header = page.locator('[data-section="terminals"] > .nav-item');
      await header.click();
      assert.equal(await header.getAttribute("aria-expanded"), "false");
      assert.equal(await page.locator(".term-row").count(), 0);
      assert.ok(
        await page
          .locator('[data-section="terminals"] .section-alert')
          .isVisible(),
      );
      await shot(page, name, "phone-sections-collapsed");
      await header.click();
      assert.equal(await page.locator(".term-row").count(), 10);
      await page.locator('[data-view="files"]').click();
      await page
        .locator('.files-placeholder[data-files-shown="browser"]')
        .waitFor();
      assert.deepEqual(errors, []);
      await context.close();
    }
    // 7. The Files section's tree (files=1): the home folder listed like the
    // terminals, folders expanding in place, files
    // opening like terminals, the browser's menu on every row, and the
    // expanded folders kept across a reload.
    {
      const { context, page, errors } = await open(
        browser,
        { viewport: { width: 1440, height: 900 } },
        "?files=1&tabs=4",
      );
      const fileRow = (path) =>
        page.locator(`.file-row[data-path="/home/alice/${path}"]`);
      const item = (path) => fileRow(path).locator(".file-item");
      const names = () =>
        page
          .locator(".file-tree .file-row")
          .evaluateAll((els) =>
            els.map((e) => e.dataset.path.replace("/home/alice/", "")),
          );
      await fileRow("repos").waitFor();
      // Folders first, then files; hidden entries left out (the browser's
      // setting); no Open group above the tree.
      assert.deepEqual(await names(), [
        "repos",
        "scratch",
        "app.py",
        "notes.md",
        "README.md",
      ]);
      assert.equal(
        await page.locator(".open-files, .doc-row").count(),
        0,
        "no Open group",
      );
      // Rows like the terminals': the same left edge, no rule between them.
      const tab = await box(page, ".term-row .term-item");
      const first = await item("repos").boundingBox();
      assert.ok(Math.abs(first.x - tab.x) <= 1, "aligned with the tabs");
      assert.equal(
        await fileRow("scratch").evaluate(
          (el) => getComputedStyle(el, "::before").content,
        ),
        "none",
      );
      // Open files are marked, the unsaved one with a dot.
      assert.equal(await fileRow("app.py").getAttribute("data-open"), "dirty");
      assert.equal(await fileRow("app.py").locator(".files-dirty").count(), 1);
      assert.equal(await fileRow("notes.md").getAttribute("data-open"), "open");
      assert.equal(await fileRow("README.md").getAttribute("data-open"), null);
      // A folder's row expands it in place, indented; again collapses it.
      assert.equal(await item("repos").getAttribute("aria-expanded"), "false");
      await item("repos").click();
      await fileRow("repos/todo.txt").waitFor();
      assert.equal(await item("repos").getAttribute("aria-expanded"), "true");
      assert.deepEqual((await names()).slice(0, 4), [
        "repos",
        "repos/kelpmux",
        "repos/kelpserver",
        "repos/todo.txt",
      ]);
      const icon = (path) => fileRow(path).locator(".files-kind").boundingBox();
      assert.ok(
        (await icon("repos/todo.txt")).x > (await icon("repos")).x + 8,
        "children indented",
      );
      await item("repos/kelpserver").click();
      await page.getByText("Empty folder").waitFor();
      // A file's row opens it in the main area, selected like the shown
      // terminal; it joins the open files.
      await item("repos/todo.txt").click();
      await page
        .locator(
          '.files-placeholder[data-files-shown="/home/alice/repos/todo.txt"]',
        )
        .waitFor();
      assert.equal(
        await item("repos/todo.txt").getAttribute("aria-current"),
        "page",
      );
      assert.equal(
        await page.locator('.term-item[aria-current="page"]').count(),
        1,
        "the tree row only (the open files are tabs in the Files view)",
      );
      assert.equal(
        await fileRow("repos/todo.txt").getAttribute("data-open"),
        "open",
      );
      await page.mouse.move(800, 500);
      await shot(page, name, "files-tree");
      // Right-click: the browser's menu.
      await fileRow("README.md").click({ button: "right" });
      const menu = page.locator(".files-menu");
      await menu.waitFor();
      assert.deepEqual(await menu.locator(".menu-item").allTextContents(), [
        "Open",
        "Open beside terminal",
        "Insert path into terminal 1",
        "Copy path",
        "Download",
        "Rename…",
        "Move to trash",
        "Delete permanently…",
      ]);
      await shot(page, name, "files-tree-menu");
      await menu.getByRole("menuitem", { name: /Insert path/ }).click();
      assert.ok(
        (await page.evaluate(() => window.typed)).includes(
          "/home/alice/README.md",
        ),
      );
      // The ⋯ button opens it too (on hover; always on touch); Rename
      // edits the name in place, Enter renames.
      await fileRow("notes.md").hover();
      await fileRow("notes.md")
        .getByRole("button", { name: "Actions for notes.md" })
        .click();
      await menu.getByRole("menuitem", { name: "Rename…" }).click();
      const field = page.getByRole("textbox", {
        name: "New name for notes.md",
      });
      await field.fill("ideas.md");
      await field.press("Enter");
      await fileRow("ideas.md").waitFor();
      assert.equal(await fileRow("notes.md").count(), 0);
      assert.equal(
        await fileRow("ideas.md").getAttribute("data-open"),
        "open",
        "the open file follows",
      );
      // Move to trash: the row goes, a toast says so.
      await fileRow("README.md").click({ button: "right" });
      await menu.getByRole("menuitem", { name: "Move to trash" }).click();
      await fileRow("README.md").waitFor({ state: "detached" });
      await page
        .locator(".toast", { hasText: "Moved to the trash: README.md" })
        .waitFor();
      // A folder's Open shows it in the file browser.
      await fileRow("scratch").click({ button: "right" });
      await menu.getByRole("menuitem", { name: "Open" }).click();
      await page
        .locator('.files-placeholder[data-files-shown="browser"]')
        .waitFor();
      // The expanded folders survive a reload; collapsing the section hides
      // the tree; the folded rail shows the Files header only.
      await page.reload();
      await page.waitForFunction(() => window.fixtureReady === true);
      await fileRow("repos/kelpserver").waitFor();
      assert.equal(
        await item("repos/kelpserver").getAttribute("aria-expanded"),
        "true",
      );
      await page.locator('[data-section="files"] .section-toggle').click();
      assert.equal(await page.locator(".file-row").count(), 0);
      await page.locator('[data-section="files"] .section-toggle').click();
      await fileRow("repos").waitFor();
      await page.goto(`${origin}/?files=1&tabs=4&folded=1`);
      await page.waitForFunction(() => window.fixtureReady === true);
      assert.equal(await page.locator(".file-row").count(), 0);
      assert.equal(await page.locator(".doc-row, .open-files").count(), 0);
      assert.deepEqual(errors, []);
      await context.close();
    }
    // The tree on a phone's list: the same rows, each with its menu button.
    {
      const { context, page, errors } = await open(
        browser,
        emulate("iPhone 15"),
        "?files=1&tabs=3",
      );
      const repos = page.locator('.file-row[data-path="/home/alice/repos"]');
      await repos.waitFor();
      assert.ok(
        await repos
          .getByRole("button", { name: "Actions for repos" })
          .isVisible(),
        "menu button shown on touch",
      );
      await repos.locator(".file-item").click();
      const todo = page.locator(
        '.file-row[data-path="/home/alice/repos/todo.txt"]',
      );
      await todo.waitFor();
      await shot(page, name, "phone-files-tree");
      await todo.locator(".file-item").click();
      await page
        .locator(
          '.files-placeholder[data-files-shown="/home/alice/repos/todo.txt"]',
        )
        .waitFor();
      assert.deepEqual(errors, []);
      await context.close();
    }
    // 8. One toast stack, top right: attention toasts, then download
    // progress and confirmations (newest first), none overlapping; on a
    // phone, full width under the terminal bar.
    {
      const { context, page, errors } = await open(
        browser,
        { viewport: { width: 1440, height: 900 } },
        "?files=1&toasts=1&tabs=6",
      );
      await page.evaluate(() => window.bell(3));
      const attention = page.locator('.attention-toast[data-pane="3"]');
      await attention.waitFor();
      await page
        .locator('.file-row[data-path="/home/alice/README.md"]')
        .click({ button: "right" });
      await page.getByRole("menuitem", { name: "Download" }).click();
      const download = page.locator(
        '.download-toast[data-download="README.md"]',
      );
      await download.waitFor();
      await page.evaluate(() => window.toast("First confirmation"));
      await page.evaluate(() => window.toast("Second confirmation"));
      const confirmations = page.locator(".toast-region .toast");
      await page.waitForFunction(
        () => document.querySelectorAll(".toast-region .toast").length === 2,
      );
      assert.match(
        await confirmations.first().textContent(),
        /Second confirmation/,
        "newest first",
      );
      const header = await box(page, ".grid-pane > header");
      const boxes = [
        await attention.boundingBox(),
        await download.boundingBox(),
        await confirmations.nth(0).boundingBox(),
        await confirmations.nth(1).boundingBox(),
      ];
      assert.ok(boxes[0].y >= header.y + header.height, "below the header");
      for (const [index, b] of boxes.entries()) {
        assert.ok(1440 - (b.x + b.width) <= 24, `toast ${index} at the right`);
        assert.ok(
          Math.abs(b.x + b.width - (boxes[0].x + boxes[0].width)) <= 1,
          `toast ${index} right-aligned with the others`,
        );
        if (index)
          assert.ok(
            b.y >= boxes[index - 1].y + boxes[index - 1].height + 4,
            `toast ${index} below the previous one`,
          );
      }
      assert.ok(boxes[3].y + boxes[3].height < 450, "all at the top");
      await shot(page, name, "toast-stack");
      // Dismissing the attention toast moves the others up.
      const before = (await download.boundingBox()).y;
      await attention.getByRole("button", { name: /Dismiss/ }).click();
      await attention.waitFor({ state: "detached" });
      await page.waitForFunction(
        (y) =>
          document.querySelector(".download-toast").getBoundingClientRect().y <
          y - 20,
        before,
      );
      await download
        .getByRole("button", { name: "Cancel the download of README.md" })
        .click();
      await download.waitFor({ state: "detached" });
      assert.deepEqual(errors, []);
      await context.close();
    }
    {
      const { context, page, errors } = await open(
        browser,
        emulate("iPhone 13"),
        "?toasts=1&pane=1",
      );
      const width = page.viewportSize().width;
      await page.evaluate(() => window.bell(3));
      const attention = page.locator('.attention-toast[data-pane="3"]');
      await attention.waitFor();
      await page.evaluate(() => window.toast("Path copied"));
      const toast = page.locator(".toast-region .toast");
      await toast.waitFor();
      const b = await toast.boundingBox();
      const a = await attention.boundingBox();
      const bar = await box(page, ".phone-bar");
      assert.ok(Math.abs(b.x - 16) <= 1, "left gutter");
      assert.ok(Math.abs(width - (b.x + b.width) - 16) <= 1, "right gutter");
      assert.ok(b.y >= a.y + a.height, "below the attention toast");
      assert.ok(a.y >= bar.y + bar.height, "below the terminal bar");
      await shot(page, name, "toast-stack-phone");
      assert.deepEqual(errors, []);
      await context.close();
    }
    // 5. Attention toasts replace desktop notifications (toasts=1).
    {
      const { context, page, errors } = await open(
        browser,
        { viewport: { width: 1440, height: 900 } },
        "?toasts=1",
      );
      const toast = (id) => page.locator(`.attention-toast[data-pane="${id}"]`);
      const favicon = () =>
        page.evaluate(() =>
          decodeURIComponent(document.querySelector('link[rel="icon"]').href),
        );
      const row = (id) => page.locator(`.term-row[data-pane="${id}"]`);
      // Notifications already there when the page loads make no toast.
      assert.equal(await page.locator(".attention-toast").count(), 0);
      assert.ok(!(await favicon()).includes("data-bell"));
      // A bell on a background terminal: a toast top right, a bell dot on
      // the favicon.
      await page.evaluate(() => window.bell(3));
      await toast(3).waitFor();
      const b = await toast(3).boundingBox();
      assert.ok(1440 - (b.x + b.width) <= 24, "toast at the right edge");
      assert.ok(b.y < 100, "toast at the top");
      const header = await box(page, ".grid-pane > header");
      assert.ok(b.y >= header.y + header.height, "below the terminal header");
      assert.match(await toast(3).textContent(), /codex 3.*Bell · agent 3/);
      assert.ok((await favicon()).includes("data-bell"), "favicon rings");
      // A second event of the same terminal updates its toast.
      await page.evaluate(() => window.notify(3));
      await page.waitForFunction(() =>
        document
          .querySelector('.attention-toast[data-pane="3"]')
          ?.textContent.includes("Needs input"),
      );
      assert.equal(await page.locator(".attention-toast").count(), 1);
      // None for the terminal shown alone.
      await page.evaluate(() => window.bell(1));
      await page.waitForFunction(() => window.acks.length > 0);
      assert.equal(await toast(1).count(), 0);
      await shot(page, name, "toast-desktop");
      // Clicking it shows that terminal, which reads it: the toast goes
      // and the favicon is restored.
      await toast(3).locator(".attention-open").click();
      assert.equal(
        await row(3).locator(".term-item").getAttribute("aria-current"),
        "page",
      );
      await toast(3).waitFor({ state: "detached" });
      await page.waitForFunction(
        () =>
          !decodeURIComponent(
            document.querySelector('link[rel="icon"]').href,
          ).includes("data-bell"),
      );
      // Four terminals: one summary, which lists them; an entry opens its
      // terminal, leaving three toasts.
      await page.evaluate(() => [4, 6, 7, 8].forEach((p) => window.notify(p)));
      const summary = page.locator(".attention-summary");
      await summary.waitFor();
      assert.equal(await page.locator(".attention-toast").count(), 1);
      assert.match(await summary.textContent(), /4 terminals need attention/);
      await summary.locator(".attention-row .attention-open").click();
      assert.equal(await summary.locator(".attention-list li").count(), 4);
      await shot(page, name, "toast-desktop-summary");
      await summary.locator('.attention-list [data-pane="6"]').click();
      assert.equal(
        await row(6).locator(".term-item").getAttribute("aria-current"),
        "page",
      );
      await summary.waitFor({ state: "detached" });
      assert.equal(await page.locator(".attention-toast").count(), 3);
      await shot(page, name, "toast-desktop-three");
      // Closing one hides it; its terminal stays unread.
      await toast(8)
        .getByRole("button", { name: /Dismiss/ })
        .click();
      await toast(8).waitFor({ state: "detached" });
      assert.equal(await row(8).locator(".unread-mark").count(), 1);
      // The Bell toggle off: no toast and no favicon dot for a bell.
      await page.evaluate(() =>
        window.setEvents({
          question: true,
          finished: true,
          failed: true,
          exited: true,
          bell: false,
        }),
      );
      await page.evaluate(() => window.bell(9));
      await page.waitForTimeout(100);
      assert.equal(await toast(9).count(), 0);
      assert.equal(await row(9).locator(".unread-mark").count(), 1);
      assert.ok(!(await favicon()).includes("data-bell"));
      assert.deepEqual(errors, []);
      await context.close();
    }
    // In the read-only grid, which has no focused terminal, every terminal
    // gets toasts.
    {
      const { context, page, errors } = await open(
        browser,
        { viewport: { width: 1440, height: 900 } },
        "?toasts=1&view=grid",
      );
      await page.evaluate(() => window.bell(1));
      await page.locator('.attention-toast[data-pane="1"]').waitFor();
      await shot(page, name, "toast-desktop-grid");
      await page
        .locator('.attention-toast[data-pane="1"] .attention-open')
        .click();
      await page.locator('.attention-toast[data-pane="1"]').waitFor({
        state: "detached",
      });
      assert.equal(await page.locator(".live-grid").count(), 0, "shown alone");
      assert.deepEqual(errors, []);
      await context.close();
    }
    // Phone: full width under the terminal bar, clear of the key bar's
    // place at the bottom; a tap opens the terminal.
    {
      const { context, page, errors } = await open(
        browser,
        emulate("iPhone 13"),
        "?toasts=1&pane=1",
      );
      const width = page.viewportSize().width;
      await page.evaluate(() => window.bell(3));
      const toast = page.locator('.attention-toast[data-pane="3"]');
      await toast.waitFor();
      const b = await toast.boundingBox();
      const bar = await box(page, ".phone-bar");
      assert.ok(Math.abs(b.x - 16) <= 1, "left gutter");
      assert.ok(Math.abs(width - (b.x + b.width) - 16) <= 1, "right gutter");
      assert.ok(b.y >= bar.y + bar.height, "below the terminal bar");
      assert.ok(b.y + b.height < page.viewportSize().height / 2, "at the top");
      await shot(page, name, "toast-phone");
      await page.evaluate(() => [4, 5, 6].forEach((p) => window.notify(p)));
      await page.locator(".attention-summary").waitFor();
      await page
        .locator(".attention-summary .attention-row .attention-open")
        .click();
      await shot(page, name, "toast-phone-summary");
      await page.locator('.attention-list [data-pane="3"]').click();
      assert.match(await page.locator(".phone-bar").textContent(), /codex 3/);
      assert.deepEqual(errors, []);
      await context.close();
    }
    console.log(
      `PASS: ${name} attention toasts: top right, one per terminal, none for the shown terminal, click opens and reads, summary above three, close, Bell toggle, favicon bell, grid, phone width`,
    );
    console.log(
      `PASS: ${name} sidebar: no full width, showing a terminal marks it read, per-terminal notifications, vitals above the two-column tools, account at the bottom, Terminals header opens the grid, one read-only grid page for 3, 8, 10 and 13 terminals, opening alone from tiles and shortcuts, collapsible Terminals and Files sections, a secondary New terminal row, the home folder tree (expand, open, marks, menu, rename, trash) with no Open group, one top right toast stack`,
    );
  } finally {
    await browser.close();
  }
}
