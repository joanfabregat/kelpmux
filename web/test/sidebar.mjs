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
//    tool's icon differs from the Grid entry's.
// 4. The Grid entry, at the bottom above the vitals, opens the live grid,
//    selected while the grid shows (no tab is then). The grid is one
//    read-only page: 3, 8, 10 and 13 terminals all fit with no pager, no
//    tile is focused and keys typed reach no terminal. A click on a tile, a
//    tab and every opening shortcut (Alt+digit, Ctrl-B digit, Tab, l, N)
//    show the terminal alone; the Grid entry, Alt+Tab, Ctrl+Shift+G and
//    Ctrl-B g / e the grid again. Folded, the entry is its icon; phones
//    have none.
// 5. Attention toasts.
// 6. The order (files=1, two parked): the terminals first with no section
//    header, a rule between them, New terminal centred between two rules
//    with room around it, the Parked subsection, then at the bottom the
//    Grid and Files entries above the vitals: no tree, no counts, no
//    chevrons. Files opens the file browser and is selected while it
//    shows. Folded: the tabs' icons, the plus between two short rules, and
//    the Grid and Files icons above the vitals. On a phone the same order
//    without the Grid entry.
// 7. One toast stack, top right: the attention toasts, then download
//    progress and confirmations, newest first, none overlapping, and on a
//    phone full width below the terminal bar.
// 8. The installed app's title bar (TitleBar.tsx), with the window controls
//    overlay's rules forced on: full width, its content clear of the
//    controls on either side, the shell below it, a drag region in the
//    theme colour, the brand moved out of the sidebar, the location
//    following the view and giving way in a narrow window. Off, hidden.
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
/** `wco`: the window controls overlay, which Playwright cannot enter: the
 * display mode's rules apply unconditionally and the title bar area is
 * `wco` (custom properties standing in for env(titlebar-area-*)). */
async function serve(context, wco) {
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
        body: wco
          ? readFileSync(`${out}/sidebar-fixture.css`, "utf8").replaceAll(
              "(display-mode: window-controls-overlay)",
              "all",
            ) + `:root { ${wco} }`
          : readFileSync(`${out}/sidebar-fixture.css`),
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
async function open(browser, options, search = "", wco = "") {
  const context = await browser.newContext(options);
  await serve(context, wco);
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

/** The Grid entry, which opens the grid: at the bottom, below the tabs
 * and New terminal and above the vitals, aligned with the tabs, selected
 * exactly while the grid shows, with no terminal tab selected then. */
async function checkGridEntry(page, grid) {
  const entry = page.locator('.rail-views [data-view="grid"]');
  assert.equal(await entry.count(), 1, "one Grid entry");
  assert.equal(await entry.getAttribute("aria-current"), grid ? "page" : null);
  const own = await entry.boundingBox();
  const first = await box(page, ".term-row .term-item");
  const add = await box(page, ".rail-new");
  const vitals = await box(page, ".vitals");
  assert.ok(own.y >= add.y + add.height, "below New terminal");
  assert.ok(own.y + own.height <= vitals.y + 1, "above the vitals");
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
    .locator(".view-row:has([data-view='grid'])")
    .evaluate((el) => getComputedStyle(el).backgroundColor);
  if (grid) assert.notEqual(background, "rgba(0, 0, 0, 0)", "entry selected");
  else assert.equal(background, "rgba(0, 0, 0, 0)");
}
/** Tabs have no colour stripe and a little more height than the tools, a
 * 1px rule separates each terminal from the next (none above the first),
 * and New terminal is centred between two rules in the accent colour,
 * with room above and below it. */
async function checkRowLook(page) {
  const rows = page.locator(".term-list:not([aria-label='Parked']) .term-row");
  assert.ok((await rows.count()) >= 2, "several tabs");
  assert.equal(
    await rows.first().evaluate((el) => getComputedStyle(el).boxShadow),
    "none",
    "no colour stripe",
  );
  const tab = await box(page, ".term-row .term-item");
  assert.ok(tab.height >= 40, `taller tabs (${tab.height})`);
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
  assert.equal(
    (await rule(".term-row:first-child", "::before")).content,
    "none",
    "no rule above the first terminal",
  );
  // No section header, chevron or tree.
  assert.equal(
    await page
      .locator(
        "[data-section], .section-toggle, .section-summary, .file-tree, .file-row",
      )
      .count(),
    0,
  );
  // New terminal: as wide as a tab, its label centred between two rules,
  // set apart from the last tab.
  const button = await box(page, ".rail-new");
  assert.ok(Math.abs(button.x - tab.x) <= 1, "New terminal aligned");
  assert.ok(Math.abs(button.width - tab.width) <= 1, "a tab's width");
  const label = await box(page, ".rail-new .rail-new-label");
  assert.ok(
    Math.abs(label.x + label.width / 2 - (button.x + button.width / 2)) <= 2,
    "label centred",
  );
  const rules = await page.locator(".rail-new .rail-new-rule").all();
  assert.equal(rules.length, 2, "a rule on each side");
  const [left, right] = await Promise.all(rules.map((r) => r.boundingBox()));
  assert.ok(left.x + left.width <= label.x + 1, "left rule");
  assert.ok(right.x >= label.x + label.width - 1, "right rule");
  assert.ok(left.width > 16 && right.width > 16, "visible rules");
  assert.equal(Math.round(left.height), 1);
  const last = await rows.last().boundingBox();
  assert.ok(
    button.y - (last.y + last.height) >= 6,
    `room above New terminal (${button.y - (last.y + last.height)})`,
  );
  const color = (selector) =>
    page
      .locator(selector)
      .first()
      .evaluate((el) => getComputedStyle(el).color);
  assert.notEqual(
    await color(".rail-new"),
    await color(".term-row .term-title"),
    "New terminal in the accent",
  );
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
        ["Terminals", "History", "Search", "Help"],
      );
      // 4. The Grid entry is at the bottom, not selected in single view; the
      // shown terminal's tab is.
      await checkGridEntry(page, false);
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
    // tile fully visible and no pager; the Grid entry is selected and no
    // tab is; no tile is focused and nothing is marked read.
    for (const tabs of [3, 8, 13]) {
      const { context, page, errors } = await open(
        browser,
        { viewport: { width: 1440, height: 900 } },
        `?view=grid&tabs=${tabs}`,
      );
      await shot(page, name, `desktop-grid-${tabs}`);
      await checkGridEntry(page, true);
      await checkGridFits(page, tabs);
      await checkNoFocus(page);
      await checkBottom(page, 2);
      assert.deepEqual(await page.evaluate(() => window.acks), []);
      assert.deepEqual(errors, []);
      await context.close();
    }
    // A click anywhere on a tile opens it alone; a tab does too, and the
    // Grid entry the grid again. Keys typed in the grid reach no terminal;
    // every opening shortcut leaves the grid, and the grid shortcuts show
    // it again.
    {
      const { context, page, errors } = await open(
        browser,
        { viewport: { width: 1440, height: 900 } },
        "?view=grid",
      );
      // The Terminals tool's icon is not the Grid entry's.
      const icon = (selector) =>
        page.locator(`${selector} .svg-icon`).first().innerHTML();
      assert.notEqual(
        await icon('[data-tool="terminals"]'),
        await icon('[data-view="grid"]'),
        "Terminals tool and Grid entry icons differ",
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
        await checkGridEntry(page, false);
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
        await checkGridEntry(page, true);
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
      // The Grid entry is its icon, in the same place, selected.
      await checkGridEntry(page, true);
      const entry = page.locator('[data-view="grid"]');
      assert.ok(await entry.locator(".svg-icon").isVisible(), "entry icon");
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
    // Phone: the terminal list has the same bottom (no grid, so no Grid
    // entry, and without files no Files entry either); a terminal's bar
    // has its bell.
    {
      const { context, page, errors } = await open(
        browser,
        emulate("iPhone 15"),
      );
      await shot(page, name, "phone-list");
      assert.equal(await page.locator('[data-view="grid"]').count(), 0);
      assert.equal(await page.locator(".rail-views").count(), 0);
      assert.equal(await page.locator("[data-section]").count(), 0);
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
    // 6. The order (files=1, two parked): the tabs, New terminal, Parked,
    // then the Grid and Files entries above the vitals; Files opens the
    // browser and is selected while it shows.
    {
      const { context, page, errors } = await open(
        browser,
        { viewport: { width: 1440, height: 900 } },
        "?files=1&tabs=8&parked=2",
      );
      await shot(page, name, "order");
      await checkRowLook(page);
      await checkGridEntry(page, false);
      const tabRows = page.locator(
        ".term-list[aria-label='Terminals'] .term-row",
      );
      assert.equal(await tabRows.count(), 6);
      const y = async (selector) => (await box(page, selector)).y;
      const bottom = async (selector) => {
        const b = await box(page, selector);
        return b.y + b.height;
      };
      // The tabs come first: nothing above them in the list.
      assert.ok(
        Math.abs((await y(".rail-scroll")) + 8 - (await y(".term-row"))) <= 1,
        "tabs at the top of the list",
      );
      assert.ok(
        (await bottom(".term-row:last-child")) <= (await y(".rail-new")),
      );
      assert.ok((await bottom(".rail-new")) <= (await y(".parked-section")));
      assert.equal(
        await page.locator(".term-list[aria-label='Parked'] .term-row").count(),
        2,
      );
      // The two entries, in that order, at the bottom above the vitals.
      assert.deepEqual(
        await page
          .locator(".rail-views .nav-item")
          .evaluateAll((els) => els.map((e) => e.dataset.view)),
        ["grid", "files"],
      );
      assert.deepEqual(
        await page.locator(".rail-views .nav-item .grow").allTextContents(),
        ["Grid", "Files"],
      );
      assert.ok(
        (await bottom(".parked-section")) <= (await y(".rail-views")),
        "entries below the list",
      );
      assert.ok(
        Math.abs((await bottom(".rail-views")) - (await y(".rail-vitals"))) <=
          1,
        "entries right above the vitals",
      );
      // Plain entries: an icon each, no count, dot or chevron.
      assert.equal(await page.locator(".rail-views .svg-icon").count(), 2);
      assert.equal(
        await page.locator(".rail-views .count, .rail-views button").count(),
        2,
        "two buttons and no count",
      );
      const files = page.locator('.rail-views [data-view="files"]');
      assert.equal(await files.getAttribute("aria-current"), null);
      await files.click();
      await page
        .locator('.files-placeholder[data-files-shown="browser"]')
        .waitFor();
      assert.equal(await files.getAttribute("aria-current"), "page");
      assert.equal(
        await page.locator('.term-item[aria-current="page"]').count(),
        0,
      );
      await shot(page, name, "files-selected");
      await page.locator('.rail-views [data-view="grid"]').click();
      await page.locator(".live-grid").waitFor();
      assert.equal(await files.getAttribute("aria-current"), null);
      // Folded: the tabs' icons, the plus between two short rules, then
      // the Grid and Files icons above the vitals.
      await page.goto(`${origin}/?files=1&tabs=8&parked=2&folded=1`);
      await page.waitForFunction(() => window.fixtureReady === true);
      await shot(page, name, "order-folded");
      assert.equal(await tabRows.count(), 6);
      assert.ok(await page.locator(".rail-new .svg-icon").isVisible());
      assert.equal(
        await page.locator(".rail-new .rail-new-text").isVisible(),
        false,
      );
      assert.equal(
        await page.locator(".rail-new .rail-new-rule:visible").count(),
        2,
      );
      for (const view of ["grid", "files"]) {
        const entry = page.locator(`.rail-views [data-view="${view}"]`);
        assert.ok(await entry.locator(".svg-icon").isVisible(), view);
        assert.equal(await entry.locator(".grow").isVisible(), false, view);
      }
      assert.ok(
        Math.abs((await bottom(".rail-views")) - (await y(".rail-vitals"))) <=
          1,
        "folded entries right above the vitals",
      );
      assert.ok((await bottom(".rail-new")) <= (await y(".rail-views")));
      // An old stored sections preference is ignored.
      await page.evaluate(() =>
        localStorage.setItem(
          "kelpmux-web-preferences",
          JSON.stringify({ sidebarSections: { terminals: false } }),
        ),
      );
      await page.reload();
      await page.waitForFunction(() => window.fixtureReady === true);
      assert.equal(await tabRows.count(), 6);
      assert.deepEqual(errors, []);
      await context.close();
    }
    // Phone (files=1): the same order with no Grid entry; Files at the
    // bottom opens the browser.
    {
      const { context, page, errors } = await open(
        browser,
        emulate("iPhone 15"),
        "?files=1&tabs=6",
      );
      await page.waitForTimeout(300);
      await shot(page, name, "phone-files");
      assert.equal(await page.locator('[data-view="grid"]').count(), 0);
      assert.equal(await page.locator(".term-row").count(), 6);
      const files = await box(page, '.rail-views [data-view="files"]');
      const add = await box(page, ".rail-new");
      const vitals = await box(page, ".vitals");
      assert.ok(files.y >= add.y + add.height, "Files below New terminal");
      assert.ok(files.y + files.height <= vitals.y + 1, "above the vitals");
      await page.locator('.rail-views [data-view="files"]').click();
      await page
        .locator('.files-placeholder[data-files-shown="browser"]')
        .waitFor();
      assert.deepEqual(errors, []);
      await context.close();
    }
    // 7. One toast stack, top right: attention toasts, then download
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
      await page.evaluate(() => window.download("/home/alice/README.md"));
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
    // 8. The installed app's title bar (window controls overlay), forced on
    // since Playwright cannot enter the display mode: Windows/Linux
    // controls on the right, macOS ones on the left, a narrow window. Off,
    // the bar is not shown and the shell starts at the top.
    {
      const { context, page, errors } = await open(browser, {
        viewport: { width: 1280, height: 800 },
      });
      assert.equal(await page.locator(".titlebar").isVisible(), false);
      assert.equal((await box(page, ".kelpmux-shell")).y, 0);
      assert.ok(await page.locator(".rail-head .logo").isVisible());
      await context.close();
    }
    for (const [label, wco, search] of [
      [
        "wco-right",
        "--titlebar-area-x: 0px; --titlebar-area-width: calc(100% - 140px); --titlebar-area-height: 33px;",
        "",
      ],
      [
        "wco-left",
        "--titlebar-area-x: 70px; --titlebar-area-width: calc(100% - 210px); --titlebar-area-height: 28px;",
        "?mode=light&files=1",
      ],
    ]) {
      const { context, page, errors } = await open(
        browser,
        { viewport: { width: 1280, height: 800 } },
        search,
        wco,
      );
      const x = label === "wco-left" ? 70 : 0;
      const right = label === "wco-left" ? 210 - 70 : 140;
      // At least 32 px tall, whatever the OS reports.
      const height = label === "wco-left" ? 32 : 33;
      const bar = await box(page, ".titlebar");
      assert.deepEqual(
        [bar.x, bar.y, bar.width, bar.height],
        [0, 0, 1280, height],
        `${label}: full width, the title bar area's height`,
      );
      const shell = await box(page, ".kelpmux-shell");
      assert.equal(shell.y, height, `${label}: the shell starts below`);
      assert.equal(shell.y + shell.height, 800, `${label}: and fills the rest`);
      const brand = await box(page, ".titlebar-brand");
      assert.ok(brand.x >= x + 12, `${label}: clear of left controls`);
      const location = page.locator(".titlebar-location");
      assert.equal(await location.textContent(), "shell");
      const text = await location.boundingBox();
      assert.ok(
        text.x + text.width <= 1280 - right - 12,
        `${label}: clear of right controls`,
      );
      // A drag region, painted in the colour the theme-color meta gets.
      const look = await page.locator(".titlebar").evaluate((el) => {
        const style = getComputedStyle(el);
        const probe = document.createElement("div");
        probe.style.background = "var(--background)";
        document.body.append(probe);
        const background = getComputedStyle(probe).backgroundColor;
        probe.remove();
        return {
          drag:
            style.getPropertyValue("app-region") ||
            style.getPropertyValue("-webkit-app-region"),
          select: style.userSelect || style.webkitUserSelect,
          background: style.backgroundColor === background,
        };
      });
      // Chromium alone has the overlay and app-region.
      if (name === "chromium") assert.equal(look.drag, "drag");
      assert.deepEqual([look.select, look.background], ["none", true]);
      // The bar carries the brand; the sidebar keeps its header.
      assert.equal(await page.locator(".rail-head .logo").isVisible(), false);
      await shot(page, name, `${label}-terminal`);
      await page.locator('.rail-views [data-view="grid"]').click();
      assert.equal(await location.textContent(), "Grid");
      await shot(page, name, `${label}-grid`);
      if (search.includes("files=1")) {
        await page.locator('.rail-views [data-view="files"]').click();
        assert.equal(await location.textContent(), "Files — ~/src");
        await shot(page, name, `${label}-files`);
      }
      assert.deepEqual(errors, []);
      await context.close();
    }
    {
      // A narrow window: the location gives way, the name stays.
      const { context, page, errors } = await open(
        browser,
        { viewport: { width: 900, height: 600 } },
        "",
        "--titlebar-area-x: 70px; --titlebar-area-width: 180px; --titlebar-area-height: 33px;",
      );
      assert.equal(await page.locator(".titlebar-location").isVisible(), false);
      assert.ok(await page.locator(".titlebar-name").isVisible());
      await shot(page, name, "wco-narrow");
      assert.deepEqual(errors, []);
      await context.close();
    }
    console.log(
      `PASS: ${name} title bar (forced window controls overlay): full width, clear of left and right controls, shell below it, drag region in the theme colour, brand moved from the sidebar, location follows the view, narrow window drops the location; hidden otherwise`,
    );
    console.log(
      `PASS: ${name} attention toasts: top right, one per terminal, none for the shown terminal, click opens and reads, summary above three, close, Bell toggle, favicon bell, grid, phone width`,
    );
    console.log(
      `PASS: ${name} sidebar: no full width, showing a terminal marks it read, per-terminal notifications, vitals above the two-column tools, account at the bottom, tabs first with a centred New terminal between rules then Parked, Grid and Files entries above the vitals (Grid opens the grid, Files the browser, none on phones for Grid), folded rail, one read-only grid page for 3, 8, 10 and 13 terminals, opening alone from tiles and shortcuts, one top right toast stack`,
    );
  } finally {
    await browser.close();
  }
}
