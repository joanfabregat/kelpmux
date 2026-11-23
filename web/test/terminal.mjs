// Real React terminal and trusted browser Copy events, without SSH credentials.
import { build } from "vite";
import { chromium, webkit } from "playwright";
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";
const out =
  process.env.TERMINAL_FIXTURE_OUTPUT || "/tmp/kelpmux-terminal-fixture";
await build({
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  configFile: false,
  root: new URL("..", import.meta.url).pathname,
  build: {
    outDir: out,
    emptyOutDir: true,
    lib: {
      entry: new URL("./terminal.fixture.tsx", import.meta.url).pathname,
      name: "TerminalFixture",
      formats: ["iife"],
      cssFileName: "terminal-fixture",
      fileName: () => "fixture.js",
    },
    minify: false,
  },
});
for (const engine of [chromium, webkit]) {
  const browser = await engine.launch({ headless: true });
  try {
    const context = await browser.newContext({
      viewport: { width: 1200, height: 800 },
      deviceScaleFactor: 2,
    });
    await context.route("https://terminal.test/**", (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path === "/")
        return route.fulfill({
          contentType: "text/html",
          body: '<html><head><link rel="stylesheet" href="/terminal-fixture.css"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>',
        });
      if (path.endsWith(".css"))
        return route.fulfill({
          contentType: "text/css",
          body: readFileSync(`${out}/terminal-fixture.css`),
        });
      if (path === "/fixture.js")
        return route.fulfill({
          contentType: "text/javascript",
          body: readFileSync(`${out}/fixture.js`),
        });
      return route.abort();
    });
    await context.addInitScript(() => {
      window.copied = [];
      window.denyWrite = true;
      window.copyEvents = [];
      Object.defineProperty(navigator, "clipboard", {
        value: {
          writeText: async (text) => {
            if (window.denyWrite) throw Error("Activation required");
            window.copied.push(text);
          },
        },
      });
      document.addEventListener("copy", (event) =>
        window.copyEvents.push({
          trusted: event.isTrusted,
          text: event.clipboardData?.getData("text/plain"),
        }),
      );
    });
    const page = await context.newPage();
    page.on("pageerror", (error) => console.error("PAGE ERROR", error.message));
    page.on("console", (message) => {
      if (message.type() === "error") console.error("BROWSER", message.text());
    });
    await page.goto("https://terminal.test/");
    await page.waitForFunction(() => window.terminalReady);
    const canvas = page.locator("canvas"),
      box = await canvas.boundingBox();
    const bounds = await canvas.evaluate((node) => {
      const r = node.getBoundingClientRect(),
        p = node.parentElement.getBoundingClientRect();
      return [
        r.left - p.left,
        r.top - p.top,
        p.right - r.right,
        p.bottom - r.bottom,
      ];
    });
    assert.deepEqual(bounds, [4, 4, 4, 4]);
    await page.mouse.move(box.x + 1, box.y + 10);
    await page.mouse.down();
    await page.mouse.move(box.x + 85, box.y + 10, { steps: 8 });
    await page.mouse.up();
    await page.getByRole("button", { name: "Copy from terminal" }).waitFor();
    await page.keyboard.press("Control+c");
    await page.waitForFunction(() => window.copyEvents.length > 0);
    const copied = await page.evaluate(() => window.copyEvents.at(-1));
    assert.equal(copied.trusted, true);
    assert.match(copied.text, /^Browser/);
    assert.equal(
      await page.getByRole("button", { name: "Copy from terminal" }).count(),
      0,
    );
    await page.evaluate(() => window.terminalClipboard("application copy"));
    await page.getByRole("button", { name: "Copy from terminal" }).waitFor();
    await page.evaluate(() => {
      window.denyWrite = false;
    });
    await page.getByRole("button", { name: "Copy from terminal" }).click();
    await page.waitForFunction(() =>
      window.copied.includes("application copy"),
    );
    await page.getByLabel("Input for terminal 1").focus();
    await page.keyboard.press("Meta+c");
    assert.equal(
      await page.evaluate(() =>
        window.terminalRequests.some((r) => r.type === "input"),
      ),
      false,
      "Cmd+C must not send an escape sequence to the terminal",
    );
    console.log(
      `PASS: ${engine.name()} actual terminal 4px inset, trusted Copy event, denied automatic clipboard fallback and Cmd+C routing`,
    );
    // The read-only grid tile: it claims the lease to size the terminal to
    // the tile, but keys, paste, wheel and pointer input reach nothing; a
    // click opens the terminal alone.
    await page.goto("https://terminal.test/?tile=1");
    await page.waitForFunction(() => window.terminalReady);
    await page.waitForFunction(() =>
      window.terminalRequests.some((r) => r.type === "control"),
    );
    const claim = await page.evaluate(() =>
      window.terminalRequests.find((r) => r.type === "control"),
    );
    assert.equal(claim.pane, 1);
    assert.ok(claim.size.cols > 0 && claim.size.rows > 0, "sized to the tile");
    const tile = page.locator('[data-tile-pane="1"]');
    assert.equal(await tile.locator("textarea, input").count(), 0);
    assert.equal(await tile.getAttribute("aria-current"), null);
    await page.keyboard.type("echo tile");
    await page.keyboard.press("Control+c");
    await page.keyboard.press("ArrowUp");
    await page.evaluate(() => {
      let event;
      try {
        const data = new DataTransfer();
        data.setData("text/plain", "pasted");
        event = new ClipboardEvent("paste", {
          clipboardData: data,
          bubbles: true,
        });
      } catch {
        event = new ClipboardEvent("paste", { bubbles: true });
      }
      document.querySelector('[data-tile-pane="1"]').dispatchEvent(event);
    });
    const area = await tile.locator("canvas").boundingBox();
    await page.mouse.move(area.x + area.width / 2, area.y + area.height / 2);
    await page.mouse.wheel(0, -200);
    await page.waitForTimeout(100);
    assert.deepEqual(await page.evaluate(() => window.opened), []);
    await page.mouse.click(area.x + 20, area.y + area.height - 20);
    assert.deepEqual(await page.evaluate(() => window.opened), [1]);
    const kinds = await page.evaluate(() =>
      window.terminalRequests.map((r) => r.type),
    );
    assert.ok(
      kinds.every((kind) => kind === "control" || kind === "resize"),
      `only sizing requests: ${kinds.join(", ")}`,
    );
    console.log(
      `PASS: ${engine.name()} read-only grid tile: lease for its size only, no input, a click opens it`,
    );
    await context.close();
  } finally {
    await browser.close();
  }
}
