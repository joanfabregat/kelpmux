// Phones and tablets, in Chromium and WebKit with iPhone and iPad emulation,
// without any network or SSH endpoint:
// 1. The built client (web/dist) signs in on an iPhone and an iPad in
//    portrait and landscape without horizontal scrolling, picks the phone or
//    tablet layout, and serves a valid web app manifest and icons with the
//    relay's exact headers.
// 2. The touch terminal and key bar, wired as main.tsx wires them, with a
//    recording session: reading by default, the key bar entering input
//    mode, sticky Ctrl, hiding the keyboard, one-finger history scrolling
//    and long-press selection.
// 3. The Hardware keyboard setting on an iPad: Auto, On and Off.
// Run after `npm run build`.
import { build } from "vite";
import { chromium, webkit, devices } from "playwright";
import { readFileSync, existsSync } from "node:fs";
import assert from "node:assert/strict";

const dist = new URL("../dist/", import.meta.url);
assert.ok(existsSync(new URL("index.html", dist)), "build web/dist first");
const origin = "https://terminal.test";
const connection = {
  origin,
  rpId: "terminal.test",
  hostPublic:
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOEf81Dm9FHAS/TqFm4QRJ8IbssrvKtjZCbI//PgutDQ",
  username: "alice",
  command: "/opt/kelpmux/kelpmux web-bridge --workspace main",
  workspace: "main",
  dev: false,
};
/** The relay's Content-Type choice (`content_type` in
 * src/native/web_relay.rs), so the test sees what a browser receives from
 * it. Keep the two in step. */
const relayTypes = {
  html: "text/html; charset=utf-8",
  svg: "image/svg+xml",
  js: "text/javascript; charset=utf-8",
  mjs: "text/javascript; charset=utf-8",
  css: "text/css; charset=utf-8",
  wasm: "application/wasm",
  woff2: "font/woff2",
  png: "image/png",
  webmanifest: "application/manifest+json",
  json: "application/json",
};
function relayType(path) {
  const dot = path.lastIndexOf(".");
  return (
    (dot >= 0 && relayTypes[path.slice(dot + 1)]) || "application/octet-stream"
  );
}
// The relay's response headers.
const headers = {
  "Cache-Control": "no-cache",
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
  "Cross-Origin-Resource-Policy": "same-origin",
  "X-Content-Type-Options": "nosniff",
  "Referrer-Policy": "no-referrer",
  "Content-Security-Policy":
    "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; connect-src 'self'; worker-src 'self'; img-src 'self' blob: data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
};
const targets = [
  ["iPhone 15", "phone"],
  ["iPhone 15 landscape", "phone"],
  ["iPad (gen 11)", "tablet"],
  ["iPad (gen 11) landscape", "tablet"],
];
/** A device descriptor usable in either engine. */
function emulate(name) {
  const { defaultBrowserType, ...descriptor } = devices[name];
  assert.ok(descriptor.hasTouch, `${name} has touch`);
  return descriptor;
}

for (const engine of [chromium, webkit]) {
  const browser = await engine.launch({ headless: true });
  try {
    let checkedManifest = false;
    for (const [name, layout] of targets) {
      const context = await browser.newContext({
        ...emulate(name),
        ignoreHTTPSErrors: true,
      });
      await context.route(`${origin}/**`, (route) => {
        let path = new URL(route.request().url()).pathname;
        if (path === "/connection.json")
          return route.fulfill({
            headers,
            contentType: "application/json",
            body: JSON.stringify(connection),
          });
        if (path === "/") path = "/index.html";
        const file = new URL(`.${path}`, dist);
        if (!existsSync(file)) return route.fulfill({ status: 404, headers });
        return route.fulfill({
          headers,
          contentType: relayType(path),
          body: readFileSync(file),
        });
      });
      const page = await context.newPage();
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      page.on("console", (message) => {
        if (message.type() === "error") errors.push(message.text());
      });
      await page.goto(origin);
      const signIn = page.getByRole("button", {
        name: /Sign in with security key/i,
      });
      await signIn.waitFor({ timeout: 15000 });
      assert.equal(await page.evaluate(() => crossOriginIsolated), true);
      const shape = await page.evaluate(() => {
        const login = document.querySelector(".login");
        const overflow = [...document.querySelectorAll(".login *")]
          .filter((node) => {
            const rect = node.getBoundingClientRect();
            return rect.width && rect.right > innerWidth + 0.5;
          })
          .map((node) => node.className || node.tagName);
        return {
          coarse: matchMedia("(pointer: coarse)").matches,
          layout: document.documentElement.dataset.layout,
          page: document.documentElement.scrollWidth - innerWidth,
          login: login.scrollWidth - login.clientWidth,
          overflow,
        };
      });
      assert.equal(shape.page, 0, `${name}: page scrolls sideways`);
      assert.equal(shape.login, 0, `${name}: sign-in scrolls sideways`);
      assert.deepEqual(shape.overflow, [], `${name}: content past the edge`);
      // Touch emulation reports a coarse pointer in both engines; the layout
      // follows it.
      assert.equal(shape.coarse, true, `${name}: coarse pointer`);
      assert.equal(shape.layout, layout, name);
      const box = await signIn.boundingBox();
      assert.ok(box.height >= 44, `${name}: sign-in tap target ${box.height}`);
      for (const label of [/Register a new key/i]) {
        const button = await page.getByRole("button", { name: label });
        assert.ok(await button.isVisible());
        assert.ok(
          (await button.boundingBox()).height >= 44,
          `${name} ${label}`,
        );
      }
      if (!checkedManifest) {
        checkedManifest = true;
        const app = await page.evaluate(async () => {
          const link = document.querySelector('link[rel="manifest"]');
          const response = await fetch(link.href);
          const manifest = JSON.parse(await response.text());
          const load = async (src) => {
            const image = new Image();
            image.src = new URL(src, location.href).href;
            await image.decode();
            return `${image.naturalWidth}x${image.naturalHeight}`;
          };
          const served = async (src) =>
            (await fetch(new URL(src, location.href).href)).headers.get(
              "content-type",
            );
          const icons = [];
          for (const icon of manifest.icons)
            icons.push([
              icon.sizes,
              icon.type,
              await load(icon.src),
              await served(icon.src),
            ]);
          const meta = (name) =>
            document.querySelector(`meta[name="${name}"]`)?.content;
          const apple = document.querySelector(
            'link[rel="apple-touch-icon"]',
          ).href;
          return {
            ok: response.ok,
            manifestType: response.headers.get("content-type"),
            manifest,
            icons,
            apple: await load(apple),
            appleType: await served(apple),
            capable: meta("apple-mobile-web-app-capable"),
            title: meta("apple-mobile-web-app-title"),
            status: meta("apple-mobile-web-app-status-bar-style"),
            viewport: meta("viewport"),
            theme: meta("theme-color"),
            serviceWorker: !!navigator.serviceWorker?.controller,
          };
        });
        assert.equal(app.ok, true);
        // The relay's exact types, which nosniff makes browsers rely on.
        assert.equal(app.manifestType, "application/manifest+json");
        assert.equal(app.appleType, "image/png");
        assert.equal(app.manifest.name, "KelpMux");
        assert.equal(app.manifest.short_name, "KelpMux");
        assert.equal(app.manifest.display, "standalone");
        assert.equal(app.manifest.start_url, "/");
        assert.equal(app.manifest.scope, "/");
        assert.match(app.manifest.theme_color, /^#[0-9a-f]{6}$/i);
        assert.match(app.manifest.background_color, /^#[0-9a-f]{6}$/i);
        for (const [sizes, type, natural, servedType] of app.icons) {
          if (sizes !== "any") assert.equal(natural, sizes);
          assert.match(type, /^image\/(png|svg\+xml)$/);
          assert.equal(servedType, type, `${sizes} icon served as declared`);
        }
        assert.ok(app.manifest.icons.some((i) => i.sizes === "512x512"));
        assert.ok(app.manifest.icons.some((i) => i.purpose === "maskable"));
        assert.equal(app.apple, "180x180");
        assert.equal(app.capable, "yes");
        assert.equal(app.title, "KelpMux");
        assert.equal(app.status, "black-translucent");
        assert.match(app.viewport, /viewport-fit=cover/);
        // The status bar and the installed app's splash match the dark
        // background token (--background in styles/base.css).
        assert.equal(app.theme, "#101113");
        assert.equal(app.manifest.theme_color, app.theme);
        assert.equal(app.manifest.background_color, app.theme);
        assert.equal(app.serviceWorker, false);
      }
      assert.deepEqual(errors, [], name);
      console.log(
        `PASS: ${engine.name()} ${name} sign-in fits, ${shape.layout} layout`,
      );
      await context.close();
    }
    console.log(
      `PASS: ${engine.name()} manifest and icons served with the relay's headers`,
    );
  } finally {
    await browser.close();
  }
}

// 2. Touch terminal and key bar.
const out = process.env.MOBILE_FIXTURE_OUTPUT || "/tmp/kelpmux-mobile-fixture";
await build({
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  configFile: false,
  logLevel: "warn",
  root: new URL("..", import.meta.url).pathname,
  build: {
    outDir: out,
    emptyOutDir: true,
    lib: {
      entry: new URL("./mobile.fixture.tsx", import.meta.url).pathname,
      name: "MobileFixture",
      formats: ["iife"],
      cssFileName: "mobile-fixture",
      fileName: () => "fixture.js",
    },
    minify: false,
  },
});
/** A context emulating `device` that serves the fixture, with a recording
 * clipboard. */
async function fixtureContext(browser, device) {
  const context = await browser.newContext(emulate(device));
  await context.route("https://terminal.test/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/")
      return route.fulfill({
        contentType: "text/html",
        body: '<html><head><meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover"><link rel="stylesheet" href="/mobile-fixture.css"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>',
      });
    if (path.endsWith(".css"))
      return route.fulfill({
        contentType: "text/css",
        body: readFileSync(`${out}/mobile-fixture.css`),
      });
    if (path === "/fixture.js")
      return route.fulfill({
        contentType: "text/javascript; charset=utf-8",
        body: readFileSync(`${out}/fixture.js`),
      });
    return route.abort();
  });
  await context.addInitScript(() => {
    window.copied = [];
    Object.defineProperty(navigator, "clipboard", {
      value: {
        writeText: async (text) => window.copied.push(text),
        readText: async () => "pasted",
      },
    });
  });
  return context;
}
/** Text of the input requests the fixture's session recorded. */
async function sentText(page) {
  return (await page.evaluate(() => window.terminalRequests))
    .filter((r) => r.type === "input")
    .map((r) => String.fromCharCode(...r.params.bytes));
}
for (const engine of [chromium, webkit]) {
  const browser = await engine.launch({ headless: true });
  try {
    const context = await fixtureContext(browser, "iPhone 15");
    const page = await context.newPage();
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto("https://terminal.test/");
    await page.waitForFunction(() => window.terminalReady);
    // The fixture's touch rules follow (pointer: coarse).
    assert.equal(
      await page.evaluate(() => document.documentElement.dataset.touch),
      "true",
    );
    const canvas = page.locator(".terminal canvas");
    const box = await canvas.boundingBox();
    const inputFocused = () =>
      page.evaluate(() =>
        document.activeElement?.classList.contains("terminal-input"),
      );
    const requests = () => page.evaluate(() => window.terminalRequests);
    const sent = () => sentText(page);
    /** Synthetic touch pointer events (both engines support them). */
    const touch = (type, x, y) =>
      canvas.evaluate(
        (node, [type, x, y]) =>
          node.dispatchEvent(
            new PointerEvent(type, {
              pointerType: "touch",
              pointerId: 7,
              isPrimary: true,
              clientX: x,
              clientY: y,
              button: 0,
              buttons: type === "pointerup" ? 0 : 1,
              bubbles: true,
              cancelable: true,
            }),
          ),
        [type, x, y],
      );

    // Opening a terminal does not bring up the keyboard; a tap claims the
    // terminal without focusing its input.
    assert.equal(await inputFocused(), false, "opened in reading mode");
    const cx = box.x + box.width / 2,
      cy = box.y + box.height / 2;
    await touch("pointerdown", cx, cy);
    await touch("pointerup", cx, cy);
    assert.equal(await inputFocused(), false, "a tap keeps reading mode");
    assert.ok((await requests()).some((r) => r.type === "control"));
    assert.equal(await page.evaluate(() => window.inputMode), "reading");

    // A key bar tap sends the key and enters input mode.
    await page.getByRole("button", { name: "Escape" }).click();
    assert.equal(await inputFocused(), true, "key bar enters input mode");
    assert.deepEqual(await sent(), ["\x1b"]);
    await page.waitForFunction(() => window.inputMode === "input");

    // Sticky Ctrl applies to the next typed key, then releases.
    const ctrl = page.getByRole("button", { name: /^Ctrl/ });
    await ctrl.click();
    assert.equal(await ctrl.getAttribute("aria-pressed"), "true");
    await page.keyboard.type("c");
    assert.equal((await sent()).at(-1), "\x03");
    assert.equal(await ctrl.getAttribute("aria-pressed"), "false");
    await page.keyboard.type("l");
    assert.equal((await sent()).at(-1), "l");
    await ctrl.click();
    await page.getByRole("button", { name: "Up arrow" }).click();
    assert.equal((await sent()).at(-1), "\x1b[1;5A");
    await page.getByRole("button", { name: "Answer yes" }).click();
    assert.equal((await sent()).at(-1), "y");

    // A tap on the terminal while typing keeps the keyboard.
    await touch("pointerdown", cx, cy);
    await touch("pointerup", cx, cy);
    assert.equal(await inputFocused(), true, "a tap keeps input mode");

    // The keyboard button hides the keyboard.
    await page.getByRole("button", { name: "Hide keyboard" }).click();
    assert.equal(await inputFocused(), false, "hidden");
    await page.waitForFunction(() => window.inputMode === "reading");
    await page.getByRole("button", { name: "Show keyboard" }).waitFor();

    // One finger dragged down scrolls into history, like the wheel.
    await touch("pointerdown", cx, box.y + 40);
    for (let y = 40; y <= 340; y += 20)
      await touch("pointermove", cx, box.y + y);
    await touch("pointerup", cx, box.y + 340);
    await page.waitForFunction(() =>
      window.terminalRequests.some((r) => r.type === "history"),
    );
    const back = page.getByRole("button", { name: /Back to live/ });
    await back.waitFor();
    await back.click();
    assert.equal(await back.count(), 0);
    assert.equal(await inputFocused(), false, "back to live keeps reading");

    // A long press selects the word under the finger and copies it.
    const wx = box.x + 12,
      wy = box.y + 12;
    await touch("pointerdown", wx, wy);
    await page.waitForTimeout(700);
    await touch("pointerup", wx, wy);
    await page.waitForFunction(() => window.copied.length > 0);
    assert.equal(await page.evaluate(() => window.copied.at(-1)), "Touch");
    assert.equal(await inputFocused(), false, "selection keeps reading");

    assert.deepEqual(errors, []);
    console.log(
      `PASS: ${engine.name()} touch terminal: reading by default, key bar input, sticky Ctrl, hide keyboard, history scroll, long-press copy`,
    );
    await context.close();
  } finally {
    await browser.close();
  }
}

// 3. The Hardware keyboard setting on an iPad. Emulation cannot attach a
// keyboard; Playwright's key presses reach the page as a hardware
// keyboard's do on iPadOS: a keydown on the document while no text field
// has focus.
for (const engine of [chromium, webkit]) {
  const browser = await engine.launch({ headless: true });
  try {
    const context = await fixtureContext(browser, "iPad (gen 11)");
    const page = await context.newPage();
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const open = async (hardware) => {
      await page.goto(`https://terminal.test/?hardware=${hardware}`);
      await page.waitForFunction(() => window.terminalReady);
    };
    const field = () =>
      page.evaluate(() => {
        const input = document.querySelector(".terminal-input");
        return {
          focused: document.activeElement === input,
          inputmode: input.getAttribute("inputmode"),
          mode: window.inputMode,
        };
      });

    // Auto: reading at first; the first hardware key enters input mode,
    // without the on-screen keyboard, and reaches the terminal.
    await open("auto");
    assert.deepEqual(await field(), {
      focused: false,
      inputmode: null,
      mode: "reading",
    });
    await page.keyboard.press("x");
    await page.waitForFunction(() => window.inputMode === "input");
    assert.deepEqual(await field(), {
      focused: true,
      inputmode: "none",
      mode: "input",
    });
    assert.deepEqual(await sentText(page), ["x"], "first key delivered");
    await page.keyboard.press("Enter");
    await page.keyboard.press("ArrowUp");
    assert.deepEqual(await sentText(page), ["x", "\r", "\x1b[A"]);
    // Leaving input mode, then a modifier on its own changes nothing.
    await page.getByRole("button", { name: "Hide keyboard" }).click();
    await page.waitForFunction(() => window.inputMode === "reading");
    await page.keyboard.press("Shift");
    assert.equal((await field()).mode, "reading", "a lone modifier reads on");
    // The Input control still shows the on-screen keyboard.
    await page.getByRole("button", { name: "Show keyboard" }).click();
    await page.waitForFunction(() => window.inputMode === "input");
    assert.deepEqual(await field(), {
      focused: true,
      inputmode: null,
      mode: "input",
    });

    // On: terminals open in input mode without the on-screen keyboard.
    await open("on");
    await page.waitForFunction(() => window.inputMode === "input");
    assert.deepEqual(await field(), {
      focused: true,
      inputmode: "none",
      mode: "input",
    });
    await page.keyboard.press("a");
    assert.deepEqual(await sentText(page), ["a"]);

    // Off: hardware keys do nothing until Input is tapped.
    await open("off");
    await page.keyboard.press("y");
    assert.deepEqual(await field(), {
      focused: false,
      inputmode: null,
      mode: "reading",
    });
    assert.deepEqual(await sentText(page), []);

    assert.deepEqual(errors, []);
    console.log(
      `PASS: ${engine.name()} iPad hardware keyboard: Auto enters input on the first key and delivers it, On opens in input, Off waits for Input`,
    );
    await context.close();
  } finally {
    await browser.close();
  }
}
