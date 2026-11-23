// The file browser and editor (test/files.fixture.tsx) in Chromium, against
// a real `kelpmux server` and `kelpmux web-bridge` in a temporary HOME,
// with a pipe in place of SSH as in test/bridge.mjs. It lists the home
// folder (hidden entries only on request), navigates, creates a folder and
// a file, edits and saves the file (checked on disk), meets a conflict when
// the file changed on disk and overwrites it, renames an entry, drags it
// onto a terminal's row (its shell-quoted path appears on that terminal's
// screen), moves it into a folder by dragging, moves a file to the trash
// (~/.local/share/Trash/files) and deletes a folder permanently, and
// uploads a desktop file dropped on a folder into that folder. Then the
// phone layout. Needs a built kelpmux (KELPMUX_BINARY, default
// ../../target/release/kelpmux) and web/public/client-core.wasm. Set
// FILES_SCREENSHOTS to a directory to save screenshots.
import { build } from "vite";
import { chromium } from "playwright";
import { spawn } from "node:child_process";
import {
  existsSync,
  mkdirSync,
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
const shots = process.env.FILES_SCREENSHOTS;
if (shots) mkdirSync(shots, { recursive: true });
const out = process.env.FILES_FIXTURE_OUTPUT || "/tmp/kelpmux-files-fixture";
await build({
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  configFile: false,
  logLevel: "warn",
  root: new URL("..", import.meta.url).pathname,
  build: {
    outDir: out,
    emptyOutDir: true,
    lib: {
      entry: new URL("./files.fixture.tsx", import.meta.url).pathname,
      name: "FilesFixture",
      formats: ["iife"],
      cssFileName: "files-fixture",
      fileName: () => "fixture.js",
    },
    minify: false,
  },
});

const root = mkdtempSync(join(tmpdir(), "kelpmux-files-"));
const home = join(root, "home");
mkdirSync(join(home, "docs"), { recursive: true });
writeFileSync(join(home, "notes.txt"), "first line\n");
writeFileSync(join(home, "docs/readme.md"), "# Read me\n");
writeFileSync(join(home, ".profile-test"), "hidden\n");
const env = {
  ...process.env,
  HOME: home,
  SHELL: "/bin/sh",
  XDG_CACHE_HOME: join(root, "cache"),
  XDG_DATA_HOME: join(home, ".local/share"),
  KELPSERVER_BINARY: binary,
};
const socket = join(root, "state/control.sock");
const server = spawn(
  binary,
  ["server", "--state-dir", join(root, "state"), "--socket", socket, "--shell", "/bin/sh", "--cwd", home],
  { env, stdio: "ignore" },
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

const page_ = (path) =>
  ({
    "/": ["text/html", '<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><link rel="stylesheet" href="/files-fixture.css"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>'],
    "/files-fixture.css": ["text/css", () => readFileSync(`${out}/files-fixture.css`)],
    "/fixture.js": ["text/javascript", () => readFileSync(`${out}/fixture.js`)],
    "/client-core.wasm": ["application/wasm", () => readFileSync(wasm)],
  })[path];

/** A page connected through its own bridge (one SSH session each). */
async function open(browser, options, query = "") {
  const bridge = spawn(binary, ["web-bridge", "--socket", socket], {
    cwd: home,
    env,
    stdio: ["pipe", "pipe", "inherit"],
  });
  const context = await browser.newContext(options);
  await context.route("https://files.test/**", (route) => {
    const file = page_(new URL(route.request().url()).pathname);
    if (!file) return route.abort();
    return route.fulfill({
      contentType: file[0],
      body: typeof file[1] === "function" ? file[1]() : file[1],
    });
  });
  const page = await context.newPage();
  page.on("pageerror", (error) => console.error("PAGE ERROR", error.message));
  let chunks = [],
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
    chunks.push(data);
    if (!flushing) void flush();
  });
  await page.exposeFunction("bridgeWrite", (bytes) => {
    bridge.stdin.write(Buffer.from(bytes));
  });
  await page.goto(`https://files.test/${query}`);
  await page.waitForFunction(() => window.fixtureReady && window.welcome);
  return {
    page,
    close: async () => {
      await context.close();
      bridge.kill();
    },
  };
}

const row = (page, name) =>
  page.locator(`.files-browser .files-row[data-path$="/${name}"]`);
const shot = async (page, name) => {
  if (shots) await page.screenshot({ path: join(shots, `${name}.png`) });
};
/** Waits until `check` holds for a file on disk. */
async function onDisk(check, what) {
  for (let i = 0; i < 100; i++) {
    if (check()) return;
    await new Promise((r) => setTimeout(r, 50));
  }
  assert.fail(`on disk: ${what}`);
}
const read = (path) =>
  existsSync(path) ? readFileSync(path, "utf8") : undefined;
async function action(page, name, item) {
  await row(page, name).hover();
  await page.getByRole("button", { name: `Actions for ${name}` }).click();
  await page.getByRole("menuitem", { name: item }).click();
}

const browser = await chromium.launch({ headless: true });
try {
  const { page, close } = await open(browser, {
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 1,
  });
  // The Files entry in the sidebar; the home folder, folders first, the
  // hidden entry only once asked for.
  await page.locator('[data-view="files"]').waitFor();
  await row(page, "notes.txt").waitFor();
  const names = await page
    .locator(".files-browser .files-row[data-path] .files-name")
    .allTextContents();
  assert.deepEqual(names, ["docs", "notes.txt"]);
  await page.getByRole("button", { name: "Show hidden files" }).click();
  await row(page, ".profile-test").waitFor();
  await page.getByRole("button", { name: "Show hidden files" }).click();
  await row(page, ".profile-test").waitFor({ state: "detached" });
  console.log("PASS: lists the home folder, folders first, hidden on request");

  // Navigate into a folder and back through the breadcrumbs.
  await row(page, "docs").getByRole("button", { name: /^docs,/ }).click();
  await row(page, "readme.md").waitFor();
  assert.equal(
    await page.locator('.files-crumb[aria-current="location"]').innerText(),
    "docs",
  );
  await page.locator(".files-crumb", { hasText: "Home" }).click();
  await row(page, "notes.txt").waitFor();
  console.log("PASS: navigates into a folder and back by its breadcrumbs");

  // A new folder and a new file (which opens in the editor).
  await page.getByRole("button", { name: "New folder" }).click();
  await page.getByLabel("New folder name").fill("projects");
  await page.getByLabel("New folder name").press("Enter");
  await row(page, "projects").waitFor();
  assert.ok(existsSync(join(home, "projects")));
  await page.getByRole("button", { name: "New file", exact: true }).click();
  await page.getByLabel("New file name").fill("hello.py");
  await page.getByLabel("New file name").press("Enter");
  const editor = page.locator(".files-editor .cm-content");
  await editor.waitFor();
  assert.equal(read(join(home, "hello.py")), "");
  console.log("PASS: creates a folder and a file, which opens in the editor");

  // Edit and save with Ctrl+S; the sidebar marks the unsaved file.
  await editor.click();
  await page.keyboard.type('print("hello")\n');
  await page
    .locator('.doc-row[data-doc$="/hello.py"] .files-dirty')
    .waitFor();
  await page.keyboard.press("Control+s");
  await onDisk(() => read(join(home, "hello.py")) === 'print("hello")\n', "saved");
  await page
    .locator('.doc-row[data-doc$="/hello.py"] .files-dirty')
    .waitFor({ state: "detached" });
  await page.waitForTimeout(200);
  await shot(page, "desktop-editor");
  console.log("PASS: edits and saves with Ctrl+S (checked on disk)");

  // Changed on disk meanwhile: the save conflicts; Overwrite wins.
  writeFileSync(join(home, "hello.py"), "changed elsewhere\n");
  await editor.click();
  await page.keyboard.press("Control+End");
  await page.keyboard.type("print(2)\n");
  await page.keyboard.press("Control+s");
  const conflict = page.getByRole("alertdialog");
  await conflict.waitFor();
  assert.match(await conflict.innerText(), /changed on disk/);
  await shot(page, "desktop-conflict");
  await conflict.getByRole("button", { name: "Overwrite" }).click();
  await onDisk(
    () => read(join(home, "hello.py")) === 'print("hello")\nprint(2)\n',
    "overwritten",
  );
  await conflict.waitFor({ state: "detached" });
  console.log("PASS: a save over a file changed on disk conflicts; Overwrite replaces it");

  // Back to the browser; rename notes.txt to a name with a space.
  await page.locator('[data-view="files"]').click();
  await action(page, "notes.txt", "Rename…");
  const rename = page.getByLabel("New name for notes.txt");
  await rename.fill("my notes.txt");
  await rename.press("Enter");
  await row(page, "my notes.txt").waitFor();
  assert.ok(existsSync(join(home, "my notes.txt")));
  assert.ok(!existsSync(join(home, "notes.txt")));
  await shot(page, "desktop-browser");
  console.log("PASS: renames an entry");

  // A terminal: dragging an entry onto its sidebar row types the quoted
  // path (no Enter) into it.
  const { pane } = await page.evaluate(
    (cwd) => window.session.rpc("new", { request: "files-term", cwd }),
    home,
  );
  await page.evaluate((pane) => {
    window.session.request("subscribe", { panes: [pane] });
    window.openTerminal(pane, "shell");
  }, pane);
  await page.locator(`.term-row[data-pane="${pane}"]`).waitFor();
  await row(page, "my notes.txt").dragTo(
    page.locator(`.term-row[data-pane="${pane}"]`),
  );
  const quoted = `'${join(home, "my notes.txt")}'`;
  await page.waitForFunction(
    ({ pane, quoted }) =>
      window.screenText(pane).replace(/\n/g, "").includes(quoted),
    { pane, quoted },
    { timeout: 10000 },
  );
  console.log("PASS: dragging an entry onto a terminal row types its quoted path");

  // Insert path from the menu types into the same terminal.
  await action(page, "docs", "Insert path into terminal 1");
  await page.waitForFunction(
    ({ pane, path }) =>
      window.screenText(pane).replace(/\n/g, "").includes(path),
    // Pastes add no separator between them.
    { pane, path: `${quoted}${join(home, "docs")}` },
    { timeout: 10000 },
  );
  console.log("PASS: Insert path types the path into the terminal");

  // Move by dragging onto a folder.
  await row(page, "my notes.txt").dragTo(row(page, "projects"));
  await row(page, "my notes.txt").waitFor({ state: "detached" });
  assert.ok(existsSync(join(home, "projects/my notes.txt")));
  console.log("PASS: dragging an entry onto a folder moves it there");

  // Move to trash (the open, saved editor closes with it).
  await action(page, "hello.py", "Move to trash");
  await row(page, "hello.py").waitFor({ state: "detached" });
  await onDisk(
    () => existsSync(join(home, ".local/share/Trash/files/hello.py")),
    "trashed",
  );
  assert.ok(!existsSync(join(home, "hello.py")));
  assert.ok(
    existsSync(join(home, ".local/share/Trash/info/hello.py.trashinfo")),
  );
  await page
    .locator('.doc-row[data-doc$="/hello.py"]')
    .waitFor({ state: "detached" });
  console.log("PASS: moves a file to the trash (its editor closes)");

  // Delete permanently, after confirming.
  await action(page, "projects", "Delete permanently…");
  const confirm = page.getByRole("alertdialog");
  assert.match(await confirm.innerText(), /cannot be undone/);
  await confirm.getByRole("button", { name: "Delete permanently" }).click();
  await row(page, "projects").waitFor({ state: "detached" });
  assert.ok(!existsSync(join(home, "projects")));
  console.log("PASS: deletes a folder permanently after confirming");

  // A desktop file dropped on a folder uploads into that folder.
  await page.evaluate(() => {
    const target = document.querySelector(
      '.files-browser .files-row[data-path$="/docs"]',
    );
    const data = new DataTransfer();
    data.items.add(
      new File(["uploaded through the browser\n"], "upload.txt", {
        type: "text/plain",
      }),
    );
    for (const type of ["dragenter", "dragover", "drop"])
      target.dispatchEvent(
        new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: data }),
      );
  });
  await onDisk(
    () => read(join(home, "docs/upload.txt")) === "uploaded through the browser\n",
    "uploaded",
  );
  await page.waitForFunction(() =>
    window.toasts.some((t) => /Uploaded “upload.txt” to docs/.test(t.message)),
  );
  const leftovers = readFileSync(join(home, "docs/readme.md"), "utf8");
  assert.equal(leftovers, "# Read me\n");
  const { readdirSync } = await import("node:fs");
  assert.deepEqual(readdirSync(join(home, "docs")).sort(), ["readme.md", "upload.txt"]);
  console.log("PASS: a desktop file dropped on a folder uploads into it");

  const errors = await page.evaluate(() => window.bridgeErrors);
  assert.deepEqual(errors, []);
  await close();

  // The phone: the browser and an open file, full width.
  mkdirSync(join(home, "Downloads"));
  writeFileSync(join(home, "todo.md"), "- [ ] files on phones\n");
  writeFileSync(join(home, "build.log"), "x".repeat(48 * 1024));
  const phone = await open(
    browser,
    { viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, hasTouch: true, isMobile: true },
    "?phone=1",
  );
  await row(phone.page, "docs").waitFor();
  await shot(phone.page, "phone-browser");
  await row(phone.page, "docs").getByRole("button", { name: /^docs,/ }).click();
  await row(phone.page, "readme.md").getByRole("button", { name: /^readme.md,/ }).click();
  await phone.page.locator(".files-editor .cm-content").waitFor();
  await phone.page.waitForTimeout(200);
  await shot(phone.page, "phone-editor");
  const width = await phone.page.evaluate(() => document.documentElement.scrollWidth);
  assert.ok(width <= 390, `no horizontal scroll on a phone (${width})`);
  await phone.close();
  console.log("PASS: the phone layout shows the browser and the editor");
} finally {
  await browser.close();
  await new Promise((resolve) => {
    const client = connect(socket, () => {
      for (const record of [
        { Stream: { type: "hello", version: 1 } },
        { Stream: { type: "shutdown", request: "files-test" } },
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
