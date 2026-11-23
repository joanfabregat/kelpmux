// The browser's real Session and WASM core against a real `kelpmux server`
// through a real `kelpmux web-bridge`, with a pipe in place of SSH (the
// bridge's standard input and output, as OpenSSH would connect them). It
// checks that the session negotiates binary screens, that the bridge
// forwards the binary records the server sends, and that a large terminal
// renders from them; it also reports how long switching from a grid tile to
// a full terminal of about 300 × 80 takes with binary and with JSON screens.
// Needs a built kelpmux (KELPMUX_BINARY, default ../../target/release/kelpmux)
// and web/public/client-core.wasm.
import { build } from "vite";
import { chromium, webkit } from "playwright";
import { spawn } from "node:child_process";
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import assert from "node:assert/strict";

const binary =
  process.env.KELPMUX_BINARY ||
  new URL("../../target/release/kelpmux", import.meta.url).pathname;
assert.ok(existsSync(binary), `build kelpmux first (${binary})`);
const wasm = new URL("../public/client-core.wasm", import.meta.url);
assert.ok(existsSync(wasm), "copy the WASM core to web/public first");
const out = process.env.BRIDGE_FIXTURE_OUTPUT || "/tmp/kelpmux-bridge-fixture";
await build({
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  configFile: false,
  logLevel: "warn",
  root: new URL("..", import.meta.url).pathname,
  build: {
    outDir: out,
    emptyOutDir: true,
    lib: {
      entry: new URL("./bridge.fixture.tsx", import.meta.url).pathname,
      name: "BridgeFixture",
      formats: ["iife"],
      cssFileName: "bridge-fixture",
      fileName: () => "fixture.js",
    },
    minify: false,
  },
});

const root = mkdtempSync(join(tmpdir(), "kelpmux-bridge-"));
const socket = join(root, "state/control.sock");
const server = spawn(
  binary,
  [
    "server",
    "--state-dir",
    join(root, "state"),
    "--socket",
    socket,
    "--shell",
    "/bin/sh",
    "--cwd",
    root,
  ],
  {
    env: { ...process.env, SHELL: "/bin/sh", XDG_CACHE_HOME: join(root, "cache") },
    stdio: "ignore",
  },
);
const reachable = () =>
  new Promise((resolve) => {
    const probe = connect(socket, () => {
      probe.destroy();
      resolve(true);
    });
    probe.on("error", () => resolve(false));
  });
for (let i = 0; !(await reachable()); i++) {
  assert.ok(i < 500, "server did not start");
  await new Promise((r) => setTimeout(r, 10));
}

// Redraws the whole screen with coloured words on every resize.
const program = `draw() {
  set -- $(stty size)
  awk -v r="$1" -v c="$2" 'BEGIN {
    printf "\\033[H"
    for (y = 0; y < r; y++) {
      for (n = 0; n + 8 <= c; n += 8)
        printf "\\033[%d;%dmword%03d ", (y + n) % 2, 31 + (y * 7 + n / 8) % 7, n
      printf "\\033[0m\\033[K"
      if (y < r - 1) printf "\\r\\n"
    }
  }'
}
trap draw WINCH
draw
while :; do sleep 60 & wait $!; done`;
writeFileSync(join(root, "draw.sh"), program);

async function run(engine, mode) {
  const bridge = spawn(binary, ["web-bridge", "--socket", socket], {
    cwd: root,
    env: { ...process.env, SHELL: "/bin/sh", KELPSERVER_BINARY: binary },
    stdio: ["pipe", "pipe", "inherit"],
  });
  const browser = await engine.launch({ headless: true });
  try {
    const context = await browser.newContext({
      viewport: { width: 2100, height: 1500 },
      deviceScaleFactor: 1,
    });
    await context.route("https://bridge.test/**", (route) => {
      const path = new URL(route.request().url()).pathname;
      const files = {
        "/": ["text/html", '<html><head><link rel="stylesheet" href="/bridge-fixture.css"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>'],
        "/bridge-fixture.css": ["text/css", () => readFileSync(`${out}/bridge-fixture.css`)],
        "/fixture.js": ["text/javascript", () => readFileSync(`${out}/fixture.js`)],
        "/client-core.wasm": ["application/wasm", () => readFileSync(wasm)],
      };
      const file = files[path];
      if (!file) return route.abort();
      return route.fulfill({
        contentType: file[0],
        body: typeof file[1] === "function" ? file[1]() : file[1],
      });
    });
    const page = await context.newPage();
    page.on("pageerror", (error) => console.error("PAGE ERROR", error.message));
    // The bridge's records as they pass, and its bytes, by phase.
    const wire = { json: 0, binary: 0, bytes: 0 };
    let pending = Buffer.alloc(0),
      chunks = [],
      flushing = false;
    const flush = async () => {
      flushing = true;
      while (chunks.length) {
        const data = Buffer.concat(chunks);
        chunks = [];
        await page
          .evaluate((data) => window.bridgeRead(data), data.toString("base64"))
          .catch(() => {});
      }
      flushing = false;
    };
    bridge.stdout.on("data", (data) => {
      wire.bytes += data.length;
      pending = Buffer.concat([pending, data]);
      while (pending.length >= 4 && pending.length >= 4 + pending.readUInt32BE(0)) {
        const length = pending.readUInt32BE(0);
        if (pending[4] === 0) wire.binary += 1;
        else wire.json += 1;
        pending = pending.subarray(4 + length);
      }
      chunks.push(data);
      if (!flushing) void flush();
    });
    await page.exposeFunction("bridgeWrite", (bytes) => {
      bridge.stdin.write(Buffer.from(bytes));
    });
    await page.goto(`https://bridge.test/${mode === "json" ? "?json=1" : ""}`);
    await page.waitForFunction(() => window.fixtureReady && window.welcome);
    const capabilities = await page.evaluate(() => window.welcome.capabilities);
    assert.ok(capabilities.includes("binary-screens"));
    // A terminal as the browser creates one (the bridge's `new`), running
    // the drawing program once the grid tile holds it.
    const { pane } = await page.evaluate(
      ({ request, cwd }) => window.session.rpc("new", { request, cwd }),
      { request: `bridge-${engine.name()}-${mode}`, cwd: root },
    );
    await page.evaluate((pane) => {
      window.session.request("subscribe", { panes: [pane] });
      window.showTile(pane);
    }, pane);
    await page.waitForFunction(
      (pane) => window.session.controlled.has(pane),
      pane,
    );
    await page.evaluate(
      ({ pane, command }) =>
        window.session.request("input", {
          pane,
          bytes: [...new TextEncoder().encode(command)],
        }),
      { pane, command: `exec /bin/bash ${join(root, "draw.sh")}\n` },
    );
    const samples = [];
    for (let round = 0; round < 4; round++) {
      // The grid tile: wait for its (small) screen, fully drawn.
      await page.evaluate((pane) => window.showTile(pane), pane);
      await page.waitForFunction(
        () => {
          const last = window.draws.at(-1);
          return last && last.cols < 100 && last.full;
        },
        null,
        { timeout: 15000 },
      );
      await page.waitForTimeout(300);
      const before = wire.bytes;
      const start = await page.evaluate((pane) => {
        window.draws.length = 0;
        window.reads.length = 0;
        const start = performance.now();
        window.showSingle(pane);
        return start;
      }, pane);
      await page.waitForFunction(
        () => window.draws.some((d) => d.cols >= 200 && d.full),
        null,
        { timeout: 15000 },
      );
      const { draws, reads } = await page.evaluate(() => ({
        draws: window.draws,
        reads: window.reads,
      }));
      const first = draws.find((d) => d.cols >= 200);
      const full = draws.find((d) => d.cols >= 200 && d.full);
      const received = reads.filter((r) => r.end <= full.at);
      samples.push({
        cols: first.cols,
        rows: first.rows,
        first: first.at - start,
        full: full.at - start,
        // Time inside Session.receive (WASM core, JSON, the application's
        // screen update), and from the last read to the frame drawn.
        receive: received.reduce((sum, r) => sum + r.end - r.start, 0),
        draw: full.at - (received.at(-1)?.end ?? start),
        bytes: wire.bytes - before,
      });
    }
    const errors = await page.evaluate(() => window.bridgeErrors);
    assert.deepEqual(errors, []);
    if (mode === "binary") {
      assert.ok(wire.binary > 0, "the bridge forwarded binary screen records");
    } else {
      assert.equal(wire.binary, 0, "no binary records without the opt-in");
    }
    const median = (key) =>
      samples.map((s) => s[key]).sort((a, b) => a - b)[samples.length >> 1];
    console.log(
      `PASS: ${engine.name()} ${mode} screens through the bridge (${wire.binary} binary, ${wire.json} JSON records): ` +
        `grid tile -> ${samples[0].cols}x${samples[0].rows} terminal, first frame ${median("first").toFixed(0)} ms, ` +
        `complete ${median("full").toFixed(0)} ms (in Session.receive ${median("receive").toFixed(0)} ms, last read to frame ${median("draw").toFixed(0)} ms), ` +
        `${Math.round(median("bytes") / 1024)} KiB from the bridge (median of ${samples.length})`,
    );
  } finally {
    await browser.close();
    bridge.kill();
  }
}

try {
  for (const engine of [chromium, webkit])
    for (const mode of ["binary", "json"]) await run(engine, mode);
} finally {
  // Stop the server through the stream session API (`shutdown`).
  await new Promise((resolve) => {
    const client = connect(socket, () => {
      for (const record of [
        { Stream: { type: "hello", version: 1 } },
        { Stream: { type: "shutdown", request: "bridge-test" } },
      ]) {
        const body = Buffer.from(JSON.stringify(record));
        const header = Buffer.alloc(4);
        header.writeUInt32BE(body.length);
        client.write(Buffer.concat([header, body]));
      }
    });
    client.on("error", resolve);
    client.on("close", resolve);
    setTimeout(resolve, 3000);
  });
  if (server.exitCode === null) server.kill();
  rmSync(root, { recursive: true, force: true });
}
