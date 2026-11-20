// The built application (web/dist) boots to the security-key sign-in page in
// Chromium and WebKit, cross-origin isolated as the relay serves it, without
// any network or SSH endpoint. Run after `npm run build`.
import { chromium, webkit } from "playwright";
import { readFileSync, existsSync } from "node:fs";
import { extname } from "node:path";
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
const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".wasm": "application/wasm",
  ".woff2": "font/woff2",
  ".json": "application/json",
};
// The relay's isolation headers (crates/server/src/native/web_relay.rs).
const headers = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
  "Cross-Origin-Resource-Policy": "same-origin",
};
for (const engine of [chromium, webkit]) {
  const browser = await engine.launch({ headless: true });
  try {
    const context = await browser.newContext({ ignoreHTTPSErrors: true });
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
        contentType: types[extname(path)] || "application/octet-stream",
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
    await page
      .getByRole("button", { name: /Sign in with security key/i })
      .waitFor({ timeout: 15000 });
    assert.equal(await page.title(), "KelpMux");
    assert.ok(await page.getByText("KelpMux").first().isVisible());
    assert.ok(
      await page
        .getByRole("button", { name: /Register a new key/i })
        .isVisible(),
    );
    assert.equal(await page.evaluate(() => crossOriginIsolated), true);
    // The shared connection broker and WASM core ship in the bundle.
    const assets = await page.evaluate(async () => {
      const bundle = document.querySelector('script[type="module"]').src;
      const source = await (await fetch(bundle)).text();
      const worker = source.match(
        /\/assets\/session-worker-[A-Za-z0-9_-]+\.js/,
      )?.[0];
      const wasm = await fetch("/client-core.wasm");
      return {
        worker: !!worker && (await fetch(worker)).ok,
        wasm: wasm.ok && (await WebAssembly.validate(await wasm.arrayBuffer())),
      };
    });
    assert.deepEqual(assets, { worker: true, wasm: true });
    assert.deepEqual(errors, []);
    console.log(`PASS: ${engine.name()} boots the built client to sign-in`);
  } finally {
    await browser.close();
  }
}
