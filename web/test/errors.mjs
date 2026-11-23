// Optional error reporting, end to end in Chromium and WebKit with the built
// client (web/dist) served under the relay's headers, without any network:
// 1. With `errors.dsn` in /connection.json (and the DSN origin in the CSP's
//    connect-src, as the relay sets it), an uncaught error produces exactly
//    one envelope at a local fake ingest server: the build's release, the
//    `web` component, no breadcrumbs, no user and no query string.
// 2. Without it, the reporting chunk is never loaded and nothing is sent.
// Run after `npm run build`.
import { chromium, webkit } from "playwright";
import { createServer } from "node:http";
import { readFileSync, existsSync } from "node:fs";
import { extname } from "node:path";
import assert from "node:assert/strict";

const dist = new URL("../dist/", import.meta.url);
assert.ok(existsSync(new URL("index.html", dist)), "build web/dist first");
const release = readFileSync(
  new URL("../sourcemaps/RELEASE", import.meta.url),
  "utf8",
).trim();
const origin = "https://terminal.test";
const ingest = "https://ingest.test";
const dsn = `https://0123456789abcdef@ingest.test/42`;
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
/** The relay's headers (src/native/web_relay.rs), with its CSP. */
function headers(reporting) {
  const connect = reporting ? `'self' ${ingest}` : "'self'";
  return {
    "Cross-Origin-Opener-Policy": "same-origin",
    "Cross-Origin-Embedder-Policy": "require-corp",
    "Cross-Origin-Resource-Policy": "same-origin",
    "X-Content-Type-Options": "nosniff",
    "Referrer-Policy": "no-referrer",
    "Content-Security-Policy": `default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; connect-src ${connect}; worker-src 'self'; img-src 'self' blob: data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'`,
  };
}

// The fake GlitchTip: records envelopes, answers like Sentry's ingest.
const received = [];
const server = createServer((request, response) => {
  const chunks = [];
  request.on("data", (chunk) => chunks.push(chunk));
  request.on("end", () => {
    received.push({
      method: request.method,
      url: request.url,
      body: Buffer.concat(chunks).toString("utf8"),
    });
    response.writeHead(200, {
      "Content-Type": "application/json",
      "Access-Control-Allow-Origin": "*",
    });
    response.end("{}");
  });
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const fake = `http://127.0.0.1:${server.address().port}`;

function envelope(body) {
  const [header, item, payload] = body.split("\n").filter(Boolean);
  return {
    header: JSON.parse(header),
    item: JSON.parse(item),
    event: JSON.parse(payload),
  };
}

async function run(engine, reporting) {
  const browser = await engine.launch({ headless: true });
  const label = `${engine.name()} ${reporting ? "with" : "without"} DSN`;
  try {
    const context = await browser.newContext({ ignoreHTTPSErrors: true });
    const elsewhere = [];
    const config = reporting
      ? { ...connection, errors: { dsn, environment: "e2e" } }
      : connection;
    await context.route(`${ingest}/**`, async (route) => {
      // Forward to the local fake server; CORS applies as for a real host.
      const url = new URL(route.request().url());
      const response = await route.fetch({
        url: `${fake}${url.pathname}${url.search}`,
      });
      await route.fulfill({ response });
    });
    await context.route(`${origin}/**`, (route) => {
      let path = new URL(route.request().url()).pathname;
      if (path === "/connection.json")
        return route.fulfill({
          headers: headers(reporting),
          contentType: "application/json",
          body: JSON.stringify(config),
        });
      if (path === "/") path = "/index.html";
      const file = new URL(`.${path}`, dist);
      if (!existsSync(file))
        return route.fulfill({ status: 404, headers: headers(reporting) });
      return route.fulfill({
        headers: headers(reporting),
        contentType: types[extname(path)] || "application/octet-stream",
        body: readFileSync(file),
      });
    });
    const page = await context.newPage();
    const chunks = [];
    page.on("request", (request) => {
      const url = new URL(request.url());
      if (url.origin !== origin) elsewhere.push(request.url());
      if (/\/assets\/error-reporting-[^/]*\.js$/.test(url.pathname))
        chunks.push(url.pathname);
    });
    const loaded = reporting
      ? page.waitForResponse((r) => /\/error-reporting-/.test(r.url()))
      : null;
    // A query string the event must not carry.
    await page.goto(`${origin}/?pane=secret-query-value#secret-fragment`);
    await page
      .getByRole("button", { name: /Sign in with security key/i })
      .waitFor({ timeout: 15000 });
    assert.equal(await page.evaluate(() => crossOriginIsolated), true);
    if (loaded) {
      await loaded;
      await page.waitForTimeout(300);
    }
    // Activity that would make breadcrumbs, then an uncaught error.
    await page.evaluate(() => {
      console.log("breadcrumb candidate: ls -la");
      history.pushState(null, "", "/?pane=secret-history-value");
      document.body.click();
      setTimeout(() => {
        throw new Error("kelpmux e2e uncaught error");
      });
    });
    const before = received.length;
    if (reporting) {
      const deadline = Date.now() + 10000;
      while (received.length === before && Date.now() < deadline)
        await page.waitForTimeout(100);
      // Room for a duplicate or follow-up (session, client report) to show.
      await page.waitForTimeout(2000);
      const sent = received.slice(before);
      assert.equal(sent.length, 1, `${label}: ${JSON.stringify(sent)}`);
      assert.equal(sent[0].method, "POST");
      assert.match(sent[0].url, /^\/api\/42\/envelope\/\?/);
      const { header, item, event } = envelope(sent[0].body);
      assert.equal(item.type, "event");
      assert.match(sent[0].url, /[?&]sentry_key=0123456789abcdef(&|$)/);
      assert.ok(header.event_id);
      if (process.env.ERRORS_ENVELOPE) console.log(sent[0].url, sent[0].body);
      assert.equal(event.release, release);
      assert.equal(event.environment, "e2e");
      assert.equal(event.tags?.component, "web");
      assert.equal(
        event.exception.values[0].value,
        "kelpmux e2e uncaught error",
      );
      assert.equal(event.breadcrumbs, undefined);
      assert.equal(event.user, undefined);
      assert.equal(event.extra, undefined);
      assert.doesNotMatch(
        sent[0].body,
        /secret-query|secret-fragment|secret-history|ls -la|alice/,
      );
      assert.equal(chunks.length, 1);
    } else {
      await page.waitForTimeout(2000);
      assert.equal(received.length, before, `${label}: sent an envelope`);
      assert.deepEqual(chunks, [], `${label}: loaded the reporting chunk`);
    }
    assert.deepEqual(
      elsewhere.filter((url) => !url.startsWith(ingest)),
      [],
      `${label}: requests to other origins`,
    );
    if (!reporting) assert.deepEqual(elsewhere, []);
    console.log(
      `PASS: ${label}: ${reporting ? "one scrubbed envelope" : "nothing sent"}`,
    );
  } finally {
    await browser.close();
  }
}

try {
  for (const engine of [chromium, webkit]) {
    await run(engine, true);
    await run(engine, false);
  }
} finally {
  server.close();
}
