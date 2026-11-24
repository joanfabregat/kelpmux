// Back and Forward through the application (src/navigation.ts, wired as in
// main.tsx by test/history.fixture.tsx), in Chromium and WebKit, without any
// network or SSH endpoint:
// 1. Terminal A → grid → terminal B → a folder → a file → Settings each
//    push an entry with a URL naming it (terminals by pane id); Back walks
//    them in reverse, Forward replays them, and Back closes Settings.
//    Closing a panel goes back instead of adding an entry, a panel swapped
//    for another or left by opening a terminal from it leaves no entry of
//    its own, the same place twice adds nothing, Alt+Tab pushes like a
//    click, and `~` normalised, a closed file and another line in the same
//    file replace.
// 2. Deep links: a reload or a URL loaded directly opens its place once the
//    state is ready; a closed terminal falls back to the grid and a missing
//    file to its folder, with a toast, replacing the entry; KelpHub-era
//    URLs keep their other parameters.
// 3. A phone: its list → a terminal → Back → the list → Forward.
import { build } from "vite";
import { chromium, webkit } from "playwright";
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const out =
  process.env.HISTORY_FIXTURE_OUTPUT || "/tmp/kelpmux-history-fixture";
await build({
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  configFile: false,
  logLevel: "warn",
  root: new URL("..", import.meta.url).pathname,
  build: {
    outDir: out,
    emptyOutDir: true,
    lib: {
      entry: new URL("./history.fixture.tsx", import.meta.url).pathname,
      name: "HistoryFixture",
      formats: ["iife"],
      fileName: () => "fixture.js",
    },
    minify: false,
  },
});
const origin = "https://terminal.test";
async function open(browser, search = "") {
  const context = await browser.newContext();
  await context.route(`${origin}/**`, (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/")
      return route.fulfill({
        contentType: "text/html; charset=utf-8",
        body: '<html><head><meta charset="utf-8"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>',
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
  await ready(page);
  return { context, page, errors };
}
const ready = (page) =>
  page.waitForFunction(() => window.fixtureReady === true);
/** Waits for the place shown and its URL query. */
async function at(page, place, search) {
  await page
    .waitForFunction(
      ([place, search]) =>
        document.querySelector("#place")?.textContent === place &&
        location.search === search,
      [place, search],
      { timeout: 3000 },
    )
    .catch(async () => {
      const got = await page.evaluate(() => [
        document.querySelector("#place")?.textContent,
        location.search,
      ]);
      assert.fail(`expected ${place} at ${search}, got ${got.join(" at ")}`);
    });
}
const length = (page) => page.evaluate(() => history.length);
const dialogShown = (page) => page.locator('[role="dialog"]').count();
const back = (page) => page.evaluate(() => history.back());
const forward = (page) => page.evaluate(() => history.forward());

for (const [name, engine] of [
  ["chromium", chromium],
  ["webkit", webkit],
]) {
  const browser = await engine.launch();
  try {
    // 1. A walk through the application, then back and forward.
    {
      const { context, page, errors } = await open(browser);
      // The first location replaces the loaded entry.
      await at(page, "grid", "?view=grid");
      // A new page starts after about:blank.
      const loaded = await length(page);
      const walk = [
        ['[data-pane="11"]', "terminal 11", "?view=terminal&terminal=11"],
        ["#grid", "grid", "?view=grid"],
        ['[data-pane="12"]', "terminal 12", "?view=terminal&terminal=12"],
        ["#src", "folder /home/me/src", "?view=files&dir=/home/me/src"],
        [
          "#open-file",
          "file /home/me/src/main.rs",
          "?view=files&file=/home/me/src/main.rs",
        ],
      ];
      for (const [button, place, search] of walk) {
        await page.click(button);
        await at(page, place, search);
      }
      await page.click("#settings");
      const settings = "?view=files&file=/home/me/src/main.rs&dialog=settings";
      await at(page, "file /home/me/src/main.rs", settings);
      assert.equal(await dialogShown(page), 1);
      assert.equal(
        await length(page),
        loaded + 6,
        "one entry per place and panel",
      );
      // Back: Settings closes, then each place in reverse.
      await page.goBack();
      await at(page, ...walk[4].slice(1));
      assert.equal(await dialogShown(page), 0, "Back closes the panel");
      for (const step of [3, 2, 1, 0]) {
        await page.goBack();
        await at(page, ...walk[step].slice(1));
      }
      await page.goBack();
      await at(page, "grid", "?view=grid");
      // Forward replays them, up to Settings open again.
      for (const step of [0, 1, 2, 3, 4]) {
        await page.goForward();
        await at(page, ...walk[step].slice(1));
      }
      await page.goForward();
      await at(page, "file /home/me/src/main.rs", settings);
      assert.equal(await dialogShown(page), 1, "Forward reopens the panel");
      // Escape closes it by going back to the file's entry: Back then
      // reaches the folder, with no second entry for the file.
      await page.keyboard.press("Escape");
      await at(page, ...walk[4].slice(1));
      assert.equal(await dialogShown(page), 0);
      await back(page);
      await at(page, ...walk[3].slice(1));
      await forward(page);
      await at(page, ...walk[4].slice(1));
      // The same place twice adds nothing; Alt+Tab pushes like a click.
      await page.click('[data-pane="12"]');
      await at(page, ...walk[2].slice(1));
      const before = await length(page);
      await page.click('[data-pane="12"]');
      await page.waitForTimeout(50);
      assert.equal(await length(page), before, "no duplicate entry");
      await page.keyboard.press("Alt+Tab");
      await at(page, "grid", "?view=grid");
      assert.equal(await length(page), before + 1);
      await back(page);
      await at(page, ...walk[2].slice(1));
      // A panel swapped for another replaces its entry; Back closes it.
      await page.click("#terminals");
      await at(
        page,
        "terminal 12",
        "?view=terminal&terminal=12&dialog=terminals",
      );
      await page.click("#help");
      await at(page, "terminal 12", "?view=terminal&terminal=12&dialog=help");
      await back(page);
      await at(page, ...walk[2].slice(1));
      assert.equal(await dialogShown(page), 0);
      // Opening a terminal from the Terminals table takes the panel's
      // entry: Back returns to the terminal it was opened over.
      await page.click("#terminals");
      await at(
        page,
        "terminal 12",
        "?view=terminal&terminal=12&dialog=terminals",
      );
      await page.click("#table-open");
      await at(page, "terminal 13", "?view=terminal&terminal=13");
      await back(page);
      await at(page, ...walk[2].slice(1));
      assert.equal(await dialogShown(page), 0);
      // A file beside the terminal is its own place.
      await page.click("#beside");
      await at(
        page,
        "terminal 12 + /home/me/notes.md",
        "?view=terminal&terminal=12&side=/home/me/notes.md",
      );
      await back(page);
      await at(page, ...walk[2].slice(1));
      // Files: `~` is replaced by the folder it names; another line in the
      // same file and closing the file replace.
      await page.click("#files");
      await at(page, "folder /home/me", "?view=files&dir=/home/me");
      await back(page);
      await at(page, ...walk[2].slice(1));
      await forward(page);
      await at(page, "folder /home/me", "?view=files&dir=/home/me");
      const entries = (await length(page)) - 1;
      await page.click("#open-file");
      await at(page, "file /home/me/src/main.rs", walk[4][2]);
      await page.click("#open-line");
      await at(
        page,
        "file /home/me/src/main.rs:42",
        "?view=files&file=/home/me/src/main.rs&line=42",
      );
      await page.click("#close-file");
      await at(page, "folder /home/me", "?view=files&dir=/home/me");
      assert.equal(await length(page), entries + 2);
      await back(page);
      await at(page, "folder /home/me", "?view=files&dir=/home/me");
      await back(page);
      await at(page, ...walk[2].slice(1));
      assert.deepEqual(errors, []);
      await context.close();
    }
    // 2. Deep links.
    {
      const { context, page, errors } = await open(
        browser,
        "?view=files&file=/home/me/notes.md&line=3",
      );
      const loaded = await length(page);
      await at(
        page,
        "file /home/me/notes.md:3",
        "?view=files&file=/home/me/notes.md&line=3",
      );
      await page.reload();
      await ready(page);
      await at(
        page,
        "file /home/me/notes.md:3",
        "?view=files&file=/home/me/notes.md&line=3",
      );
      assert.equal(await length(page), loaded, "the load and reload replace");
      // A closed terminal: the grid, with a toast, in place of the entry.
      await page.goto(`${origin}/?view=terminal&terminal=99`);
      await ready(page);
      await at(page, "grid", "?view=grid");
      assert.equal(
        await page.locator("#toast").textContent(),
        "Terminal 99 is closed",
      );
      // A missing file: its folder.
      await page.goto(`${origin}/?view=files&file=/home/me/gone.txt`);
      await ready(page);
      await at(page, "folder /home/me", "?view=files&dir=/home/me");
      assert.match(await page.locator("#toast").textContent(), /gone\.txt/);
      // A KelpHub-era URL keeps its project parameters.
      await page.goto(`${origin}/?scratch=1&view=terminals&terminal=12`);
      await ready(page);
      await at(page, "terminal 12", "?scratch=1&view=terminal&terminal=12");
      // A panel loaded from a URL has no entry below: closing it replaces.
      await page.goto(`${origin}/?view=terminal&terminal=13&dialog=settings`);
      await ready(page);
      await at(
        page,
        "terminal 13",
        "?view=terminal&terminal=13&dialog=settings",
      );
      assert.equal(await dialogShown(page), 1);
      const entries = await length(page);
      await page.click("#close-dialog");
      await at(page, "terminal 13", "?view=terminal&terminal=13");
      assert.equal(await length(page), entries);
      // Back from there leaves for the previous page load's last place.
      await page.goBack();
      await ready(page);
      await at(page, "terminal 12", "?scratch=1&view=terminal&terminal=12");
      assert.deepEqual(errors, []);
      await context.close();
    }
    // 3. A phone.
    {
      const { context, page, errors } = await open(browser, "?phone=1");
      await at(page, "home", "?phone=1&view=home");
      await page.click('[data-pane="11"]');
      await at(page, "terminal 11", "?phone=1&view=terminal&terminal=11");
      await page.goBack();
      await at(page, "home", "?phone=1&view=home");
      await page.goForward();
      await at(page, "terminal 11", "?phone=1&view=terminal&terminal=11");
      // The terminal bar's list button is a place too.
      await page.click("#home");
      await at(page, "home", "?phone=1&view=home");
      await page.goBack();
      await at(page, "terminal 11", "?phone=1&view=terminal&terminal=11");
      assert.deepEqual(errors, []);
      await context.close();
    }
    console.log(
      `PASS: ${name} history: terminal, grid, folder, file and Settings entries walked back and forward, Back closes panels, no duplicates, replacements for normalised folders, lines, closed files and swapped panels, deep links with fallbacks, legacy URLs, phone list and terminal`,
    );
  } finally {
    await browser.close();
  }
}
