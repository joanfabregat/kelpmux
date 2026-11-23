// The idle warning and lock (decisions 163–165), in Chromium and WebKit,
// against the real shared worker with tiny durations set through
// /connection.json, as the relay passes `idle_warning_minutes` and
// `idle_lock_minutes` (test/idle.fixture.tsx), without SSH:
// 1. Two windows share the worker; both show the warning with an mm:ss
//    countdown. Pointer movement does not lift it; "Stay signed in" in one
//    window lifts it in both, and a key press does too.
// 2. Untouched, the warning ends in the lock in both windows, with the
//    real total duration in the message.
// 3. On an emulated phone the panel fits the screen and a tap on
//    "Stay signed in" lifts it.
// Served from a local HTTP server: 127.0.0.1 is a secure context, so
// SharedWorker works without TLS.
import { build } from "vite";
import { chromium, webkit, devices } from "playwright";
import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const out = process.env.IDLE_FIXTURE_OUTPUT || "/tmp/kelpmux-idle-fixture";
const root = new URL("..", import.meta.url).pathname;
const library = (entry, name, emptyOutDir) =>
  build({
    define: { "process.env.NODE_ENV": JSON.stringify("production") },
    configFile: false,
    logLevel: "warn",
    root,
    build: {
      outDir: out,
      emptyOutDir,
      lib: {
        entry: new URL(entry, import.meta.url).pathname,
        name,
        formats: ["iife"],
        cssFileName: "idle-fixture",
        fileName: () => `${name}.js`,
      },
      minify: false,
    },
  });
await library("./idle.fixture.tsx", "fixture", true);
await library(
  "../node_modules/@kelpcore/web/web/src/session-worker.ts",
  "worker",
  false,
);

// 0.05 minutes: warn after 3 s, lock 3 s later.
const idle = { idle_warning_minutes: 0.05, idle_lock_minutes: 0.05 };
const server = createServer((request, response) => {
  const path = new URL(request.url, "http://x").pathname;
  const origin = `http://${request.headers.host}`;
  const send = (type, body) => {
    response.writeHead(200, { "Content-Type": type });
    response.end(body);
  };
  if (path === "/")
    return send(
      "text/html; charset=utf-8",
      '<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover"><link rel="stylesheet" href="/idle-fixture.css"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>',
    );
  if (path === "/connection.json")
    return send(
      "application/json",
      JSON.stringify({
        origin,
        rpId: "127.0.0.1",
        hostPublic: "ssh-ed25519 AAAA",
        username: "alice",
        command: "kelpmux web-bridge --workspace main",
        workspace: "main",
        dev: false,
        ...idle,
      }),
    );
  if (path === "/idle-fixture.css")
    return send("text/css; charset=utf-8", readFileSync(`${out}${path}`));
  if (path === "/fixture.js" || path === "/worker.js")
    return send(
      "text/javascript; charset=utf-8",
      readFileSync(`${out}${path}`),
    );
  response.writeHead(404);
  response.end();
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const base = `http://127.0.0.1:${server.address().port}`;

const warning = (page) => page.getByRole("alertdialog");
async function open(context, search = "") {
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(`${base}/${search}`);
  await page.locator('[data-state="online"]').waitFor({ timeout: 10000 });
  return { page, errors };
}
async function checkWarning(page) {
  const dialog = warning(page);
  await dialog.waitFor({ timeout: 10000 });
  const text = await dialog.textContent();
  assert.match(
    text,
    /You've been inactive for 3 seconds\. KelpMux locks this connection in 00:0[0-3]\. Your shells keep running\./,
  );
  assert.ok(
    await dialog.getByRole("button", { name: "Stay signed in" }).isVisible(),
  );
}
const lockText =
  "Session locked after 6 seconds without activity. Authenticate again to reconnect.";

try {
  for (const engine of [chromium, webkit]) {
    const browser = await engine.launch({ headless: true });
    const name = engine.name();
    try {
      // 1 and 2: two windows of one browser.
      {
        const context = await browser.newContext({
          viewport: { width: 1280, height: 800 },
        });
        const a = await open(context);
        const b = await open(context, "?resume");
        await checkWarning(a.page);
        await checkWarning(b.page);
        // Pointer movement is not activity.
        await a.page.mouse.move(10, 10);
        await a.page.mouse.move(400, 300);
        await a.page.waitForTimeout(300);
        assert.ok(await warning(a.page).isVisible(), "mouse move keeps it");
        // The countdown is live.
        const before = await a.page.locator(".idle-countdown").textContent();
        await a.page.waitForFunction(
          (t) => document.querySelector(".idle-countdown")?.textContent !== t,
          before,
        );
        // Stay signed in, in one window, lifts it in both.
        await b.page.getByRole("button", { name: "Stay signed in" }).click();
        await warning(a.page).waitFor({ state: "detached" });
        await warning(b.page).waitFor({ state: "detached" });
        // A key press lifts it too.
        await checkWarning(a.page);
        await b.page.keyboard.press("Shift");
        await warning(a.page).waitFor({ state: "detached" });
        await warning(b.page).waitFor({ state: "detached" });
        // Untouched: warning, then the lock in every window.
        await checkWarning(b.page);
        for (const { page } of [a, b]) {
          await page
            .locator('[data-state="locked"]')
            .waitFor({ timeout: 10000 });
          assert.equal(
            await page.locator('[data-state="locked"]').textContent(),
            lockText,
          );
          assert.equal(await warning(page).count(), 0);
        }
        // The lock follows the warning; no warning was lifted after it.
        const messages = await a.page.evaluate(() =>
          window.idleMessages
            .filter((m) => m.type === "idle" || m.type === "closed")
            .map((m) => m.type + (m.type === "idle" ? `:${m.warning}` : "")),
        );
        assert.deepEqual(messages, [
          "idle:true",
          "idle:false",
          "idle:true",
          "idle:false",
          "idle:true",
          "closed",
        ]);
        assert.deepEqual([...a.errors, ...b.errors], []);
        await context.close();
      }
      // 3: a phone, by touch.
      {
        const { defaultBrowserType, ...phone } = devices["iPhone 13"];
        const context = await browser.newContext(phone);
        const { page, errors } = await open(context);
        await checkWarning(page);
        const box = await page.locator(".idle-dialog").boundingBox();
        const width = await page.evaluate(() => window.innerWidth);
        assert.ok(box.x >= 0 && box.x + box.width <= width, "fits the phone");
        assert.ok(
          await page.evaluate(
            () =>
              document.documentElement.scrollWidth <=
              document.documentElement.clientWidth,
          ),
          "no horizontal scrolling",
        );
        await page.getByRole("button", { name: "Stay signed in" }).tap();
        await warning(page).waitFor({ state: "detached" });
        assert.ok(await page.locator('[data-state="online"]').isVisible());
        assert.deepEqual(errors, []);
        await context.close();
      }
      console.log(
        `PASS: ${name} idle warning: shown in every window with a countdown, lifted everywhere by Stay signed in, a key or a tap, ignored by pointer movement, then the lock`,
      );
    } finally {
      await browser.close();
    }
  }
} finally {
  server.close();
}
