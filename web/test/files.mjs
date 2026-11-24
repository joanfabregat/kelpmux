// The file browser and editor (test/files.fixture.tsx) in Chromium, against
// a real `kelpmux server` and `kelpmux web-bridge` in a temporary HOME,
// with a pipe in place of SSH as in test/bridge.mjs. It lists the home
// folder (hidden entries only on request), navigates, starts at the file
// root (the home folder) with no crumb above it, shows symbolic links
// pointing outside the root without opening them (in the browser and the
// tree, menus without Open or Download, the link deletable), creates a folder and
// a file, edits and saves the file (checked on disk), meets a conflict when
// the file changed on disk and overwrites it, renames an entry, drags it
// onto a terminal's row (its shell-quoted path appears on that terminal's
// screen), moves it into a folder by dragging, moves a file to the trash
// (~/.local/share/Trash/files) and deletes a folder permanently, and
// uploads a desktop file dropped on a folder into that folder. The
// sidebar's tree of the home folder follows the hidden setting, expands a
// folder, opens a file in the editor, types the path of a row dragged onto
// a terminal row, moves a row dropped on a folder row and uploads a desktop
// file dropped on one; the browser and the tree list each other's changes.
// It downloads
// a file from its menu (the browser's downloads), a folder as a .tar.gz
// (through a stand-in save picker, decoded) and a file from the editor,
// and cancels a stalled download; a bridge without downloads shows no
// Download item or button. With a
// terminal shown alone, it opens a file beside it (the terminal loses
// columns), saves an edit from the side panel with Ctrl+S, types into the
// terminal after a click on it, closes the panel (the columns come back),
// finds a printed path that does not exist, or lies outside the root (no
// stat sent), not clickable and Ctrl-clicks
// one that does, which opens beside the terminal at its line. Then the
// phone layout, where Open beside terminal opens the editor full screen. Needs a built kelpmux (KELPMUX_BINARY, default
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
  renameSync,
  rmSync,
  symlinkSync,
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
  await row(page, name)
    .getByRole("button", { name: `Actions for ${name}` })
    .click();
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
  // The sidebar's tree follows the same setting.
  await page.locator('.file-tree .file-row[data-path$="/.profile-test"]').waitFor();
  await page.getByRole("button", { name: "Show hidden files" }).click();
  await row(page, ".profile-test").waitFor({ state: "detached" });
  await page
    .locator('.file-tree .file-row[data-path$="/.profile-test"]')
    .waitFor({ state: "detached" });
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

  // The file root (the server user's home folder): the browser starts
  // there, with no crumb above it and no parent folder.
  assert.equal(
    await page.evaluate(() => window.welcome.file_limits.root),
    home,
  );
  assert.deepEqual(
    await page.locator(".files-crumb").allInnerTexts(),
    ["Home"],
  );
  assert.ok(
    await page.getByRole("button", { name: "Parent folder" }).isDisabled(),
  );
  // Symbolic links pointing outside it are shown, marked, and open
  // nothing; one pointing inside opens what it names. The link itself can
  // still be deleted. (The folder is made in the browser, so the sidebar's
  // tree lists it too.)
  await page.getByRole("button", { name: "New folder" }).click();
  await page.getByLabel("New folder name").fill("gate");
  await page.getByLabel("New folder name").press("Enter");
  await row(page, "gate").waitFor();
  writeFileSync(join(root, "outside.txt"), "outside\n");
  symlinkSync("/etc", join(home, "gate/etc-link"));
  symlinkSync("../../outside.txt", join(home, "gate/up-link"));
  symlinkSync("../docs", join(home, "gate/docs-link"));
  await row(page, "gate").getByRole("button", { name: /^gate,/ }).click();
  await row(page, "etc-link").waitFor();
  assert.deepEqual(
    await page.locator(".files-crumb").allInnerTexts(),
    ["Home", "gate"],
  );
  for (const [name, target] of [
    ["etc-link", "/etc"],
    ["up-link", "../../outside.txt"],
  ]) {
    assert.equal(await row(page, name).getAttribute("data-outside"), "true");
    const open = row(page, name).locator(".files-open");
    assert.match(await open.getAttribute("title"), new RegExp(`→ ${target.replace(/\./g, "\\.")} \\(outside`));
    // aria-disabled: a click only says why it does not open.
    await open.click({ force: true });
    await page.waitForFunction(
      (name) =>
        window.toasts.some(
          (t) =>
            t.message === `“${name}”: Outside the folders this server lets you browse`,
        ),
      name,
    );
    assert.equal(
      await page.locator('.files-crumb[aria-current="location"]').innerText(),
      "gate",
    );
    assert.equal(await page.locator(".files-editor").count(), 0);
    await row(page, name).hover();
    await row(page, name)
      .getByRole("button", { name: `Actions for ${name}` })
      .click();
    const items = await page.getByRole("menuitem").allInnerTexts();
    assert.ok(!items.includes("Open"), items.join());
    assert.ok(!items.some((item) => item.startsWith("Download")), items.join());
    assert.ok(!items.includes("Open beside terminal"), items.join());
    for (const item of ["Copy path", "Rename…", "Move to trash", "Delete permanently…"])
      assert.ok(items.includes(item), `${item} in ${items.join()}`);
    await page.keyboard.press("Escape");
  }
  assert.equal(await row(page, "docs-link").getAttribute("data-outside"), null);
  await row(page, "etc-link").waitFor();
  // The sidebar's tree marks them too.
  const gateTree = (name) =>
    page.locator(`.file-tree .file-row[data-path="${join(home, "gate", name)}"]`);
  await page
    .locator(`.file-tree .file-row[data-path="${join(home, "gate")}"] .file-item`)
    .click();
  await gateTree("etc-link").waitFor();
  assert.equal(await gateTree("etc-link").getAttribute("data-outside"), "true");
  assert.equal(await gateTree("docs-link").getAttribute("data-outside"), null);
  await page
    .locator(`.file-tree .file-row[data-path="${join(home, "gate")}"] .file-item`)
    .click();
  // Deleting the link removes the link, not what it points to.
  await action(page, "up-link", "Delete permanently…");
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Delete permanently" })
    .click();
  await row(page, "up-link").waitFor({ state: "detached" });
  assert.equal(read(join(root, "outside.txt")), "outside\n");
  // The inside link opens the folder it names (under the link's name: the
  // server resolves it beneath the root without renaming it).
  await row(page, "docs-link").locator(".files-open").click();
  await row(page, "readme.md").waitFor();
  assert.deepEqual(
    await page.locator(".files-crumb").allInnerTexts(),
    ["Home", "gate", "docs-link"],
  );
  // The gate folder goes, through the browser so both views list again.
  await page.locator(".files-crumb", { hasText: "Home" }).click();
  await action(page, "gate", "Delete permanently…");
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Delete permanently" })
    .click();
  await row(page, "gate").waitFor({ state: "detached" });
  assert.ok(!existsSync(join(home, "gate")));
  await shot(page, "desktop-root");
  console.log(
    "PASS: starts at the file root with no crumb above it; links outside it show but do not open",
  );

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
  await page.evaluate(({ pane, home }) => {
    window.session.request("subscribe", { panes: [pane] });
    window.openTerminal(pane, "shell", home);
  }, { pane, home });
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

  // The sidebar's tree of the home folder: a folder expands in place (the
  // upload above is listed), a file opens in the editor and is selected,
  // a row dragged onto a terminal types its path, onto a folder row moves
  // it, and a desktop file dropped on a folder row uploads into it.
  const treeRow = (path) =>
    page.locator(`.file-tree .file-row[data-path="${join(home, path)}"]`);
  await treeRow("docs").waitFor();
  await treeRow("docs").locator(".file-item").click();
  await treeRow("docs/upload.txt").waitFor();
  await treeRow("docs/upload.txt").locator(".file-item").click();
  await page
    .locator('.files-editor:not([hidden])[data-path$="/docs/upload.txt"] .cm-content')
    .waitFor();
  assert.equal(
    await treeRow("docs/upload.txt").locator(".file-item").getAttribute("aria-current"),
    "page",
  );
  assert.equal(await treeRow("docs/upload.txt").getAttribute("data-open"), "open");
  await shot(page, "desktop-tree");
  console.log("PASS: the sidebar tree expands a folder and opens a file in the editor");
  await treeRow("docs/upload.txt").dragTo(
    page.locator(`.term-row[data-pane="${pane}"]`),
  );
  await page.waitForFunction(
    ({ pane, path }) => window.screenText(pane).replace(/\n/g, "").includes(path),
    { pane, path: join(home, "docs/upload.txt") },
    { timeout: 10000 },
  );
  console.log("PASS: a tree row dragged onto a terminal row types its path");
  await page.locator('[data-view="files"]').click();
  await page.getByRole("button", { name: "New folder" }).click();
  await page.getByLabel("New folder name").fill("archive");
  await page.getByLabel("New folder name").press("Enter");
  // The browser's change lists the home folder again in the tree too.
  await treeRow("archive").waitFor();
  await treeRow("docs/upload.txt").dragTo(treeRow("archive"));
  await treeRow("docs/upload.txt").waitFor({ state: "detached" });
  await onDisk(() => existsSync(join(home, "archive/upload.txt")), "moved");
  // The open editor follows the move.
  await page
    .locator('.doc-row[data-doc$="/archive/upload.txt"]')
    .waitFor();
  console.log("PASS: a tree row dragged onto a tree folder moves it there");
  await page.evaluate((path) => {
    const target = document.querySelector(
      `.file-tree .file-row[data-path="${path}"]`,
    );
    const data = new DataTransfer();
    data.items.add(new File(["dropped on the tree\n"], "tree.txt", { type: "text/plain" }));
    for (const type of ["dragenter", "dragover", "drop"])
      target.dispatchEvent(
        new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: data }),
      );
  }, join(home, "archive"));
  await onDisk(
    () => read(join(home, "archive/tree.txt")) === "dropped on the tree\n",
    "uploaded from the tree",
  );
  await page.waitForFunction(() =>
    window.toasts.some((t) => /Uploaded “tree.txt” to archive/.test(t.message)),
  );
  // The browser lists the folder it shows again.
  await row(page, "archive").locator(".files-open").click();
  await row(page, "tree.txt").waitFor();
  await page.locator(".files-crumb", { hasText: "Home" }).click();
  await row(page, "docs").waitFor();
  const uploaded = page.locator('.doc-row[data-doc$="/archive/upload.txt"]');
  await uploaded.hover();
  await uploaded.getByRole("button", { name: /^Close file/ }).click();
  await uploaded.waitFor({ state: "detached" });
  // Back where the downloads below expect it.
  renameSync(join(home, "archive/upload.txt"), join(home, "docs/upload.txt"));
  console.log("PASS: a desktop file dropped on a tree folder uploads into it");

  // Downloads. The save picker is a stand-in that keeps the bytes in the
  // page (`window.saved`), with `window.hold` to stall its writes; without
  // it the browser's downloads get a Blob.
  await page.evaluate(() => {
    window.saved = {};
    window.showSaveFilePicker = async ({ suggestedName }) => ({
      createWritable: async () => {
        const file = (window.saved[suggestedName] = { parts: [], state: "open" });
        return {
          write: async (bytes) => {
            if (window.hold) await window.hold;
            file.parts.push(new Uint8Array(bytes));
          },
          close: async () => void (file.state = "closed"),
          abort: async () => void (file.state = "aborted"),
        };
      },
    });
    window.savedBase64 = (name) => {
      const file = window.saved[name];
      let text = "";
      for (const part of file.parts)
        for (const byte of part) text += String.fromCharCode(byte);
      return { state: file.state, data: btoa(text) };
    };
  });
  const saved = async (name) => {
    await page.waitForFunction(
      (name) => window.saved[name]?.state === "closed",
      name,
      { timeout: 20000 },
    );
    const { data } = await page.evaluate((name) => window.savedBase64(name), name);
    return Buffer.from(data, "base64");
  };
  /** A ustar archive's entries: name, type flag and content. */
  const untar = (archive) => {
    const entries = [];
    for (let at = 0; at + 512 <= archive.length; ) {
      const header = archive.subarray(at, at + 512);
      if (header.every((b) => b === 0)) break;
      const field = (from, length) =>
        header.subarray(from, from + length).toString("latin1").replace(/\0.*$/s, "");
      const size = parseInt(field(124, 12) || "0", 8);
      const prefix = field(345, 155);
      const name = (prefix ? `${prefix}/` : "") + field(0, 100);
      entries.push({
        name,
        type: field(156, 1) || "0",
        content: archive.subarray(at + 512, at + 512 + size).toString("utf8"),
      });
      at += 512 + Math.ceil(size / 512) * 512;
    }
    return entries;
  };

  // A file from its menu, through the browser's downloads (no picker).
  writeFileSync(join(home, "report.txt"), "quarterly numbers\n");
  await page.getByRole("button", { name: "Refresh" }).click();
  await row(page, "report.txt").waitFor();
  await page.evaluate(() => {
    window.picker = window.showSaveFilePicker;
    delete window.showSaveFilePicker;
  });
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    action(page, "report.txt", "Download"),
  ]);
  assert.equal(download.suggestedFilename(), "report.txt");
  assert.equal(readFileSync(await download.path(), "utf8"), "quarterly numbers\n");
  await page.waitForFunction(() =>
    window.toasts.some((t) => /Downloaded “report.txt”/.test(t.message)),
  );
  await page.evaluate(() => (window.showSaveFilePicker = window.picker));
  console.log("PASS: a file downloads as itself from its menu");

  // A folder as a .tar.gz archive, streamed to the picked file.
  mkdirSync(join(home, "docs/deep"), { recursive: true });
  writeFileSync(join(home, "docs/deep/inner.txt"), "inside\n");
  await action(page, "docs", "Download as .tar.gz");
  const { gunzipSync } = await import("node:zlib");
  const entries = untar(gunzipSync(await saved("docs.tar.gz")));
  assert.deepEqual(
    entries.map((e) => e.name),
    [
      "docs/",
      "docs/deep/",
      "docs/deep/inner.txt",
      "docs/readme.md",
      "docs/upload.txt",
    ],
  );
  assert.equal(entries.find((e) => e.name === "docs/deep/inner.txt").content, "inside\n");
  assert.equal(entries.find((e) => e.name === "docs/readme.md").content, "# Read me\n");
  await page.locator(".download-toast").waitFor({ state: "detached" });
  console.log("PASS: a folder downloads as a .tar.gz archive (decoded)");

  // From the editor's header.
  await row(page, "report.txt").getByRole("button", { name: /^report.txt,/ }).click();
  await page.locator('.files-editor:not([hidden])[data-path$="/report.txt"] .cm-content').waitFor();
  await page.getByRole("button", { name: "Download report.txt" }).click();
  assert.equal((await saved("report.txt")).toString("utf8"), "quarterly numbers\n");
  await page.locator('.files-editor:not([hidden])[data-path$="/report.txt"]').getByRole("button", { name: "Close report.txt" }).click();
  await row(page, "docs").waitFor();
  console.log("PASS: the editor downloads its file");

  // Cancel: with the disk stalled, the progress toast's Cancel stops the
  // download and discards the partial file; no error is shown.
  writeFileSync(join(home, "big.bin"), Buffer.alloc(6 * 1024 * 1024, 7));
  await page.getByRole("button", { name: "Refresh" }).click();
  await row(page, "big.bin").waitFor();
  await page.evaluate(() => {
    window.hold = new Promise((resolve) => (window.release = resolve));
  });
  const errorsBefore = await page.evaluate(() =>
    window.toasts.filter((t) => t.tone === "error").length,
  );
  await action(page, "big.bin", "Download");
  const progress = page.locator('.download-toast[data-download="big.bin"]');
  await progress.waitFor();
  // In the top right toast stack.
  const where = await progress.boundingBox();
  assert.ok(1440 - (where.x + where.width) <= 24, "download toast at the right");
  assert.ok(where.y < 120, "download toast at the top");
  await shot(page, "desktop-download-progress");
  await progress.getByRole("button", { name: "Cancel the download of big.bin" }).click();
  await progress.waitFor({ state: "detached" });
  await page.waitForFunction(() => window.saved["big.bin"]?.state === "aborted");
  await page.evaluate(() => {
    window.release();
    window.hold = null;
  });
  await page.waitForTimeout(300);
  assert.equal(
    await page.evaluate(() => window.toasts.filter((t) => t.tone === "error").length),
    errorsBefore,
  );
  // The connection stays usable: another download completes.
  await page.evaluate(() => (window.saved = {}));
  await action(page, "report.txt", "Download");
  await page.waitForFunction(
    () => window.saved["report.txt"]?.parts.length && window.saved["report.txt"].state === "closed",
  );
  console.log("PASS: Cancel stops a download and discards the partial file");

  // The side panel: a file opened beside the terminal last shown alone,
  // which narrows that terminal; edits save from it with Ctrl+S; the
  // terminal keeps its input; closing it gives the terminal its columns
  // back.
  mkdirSync(join(home, "src"));
  writeFileSync(
    join(home, "src/x.py"),
    "one = 1\ntwo = 2\nthree = 3\nfour = 4\n",
  );
  // Unsaved edits in the main area survive showing a terminal and coming
  // back to the file (its editor stays mounted); then they save.
  await row(page, "docs").getByRole("button", { name: /^docs,/ }).click();
  await row(page, "readme.md").getByRole("button", { name: /^readme.md,/ }).click();
  const readme = page.locator(
    '.files-editor:not([hidden])[data-path$="/readme.md"] .cm-content',
  );
  await readme.waitFor();
  await readme.click();
  await page.keyboard.press("Control+End");
  await page.keyboard.type("kept while away\n");
  await page.evaluate((pane) => window.showTerminal(pane), pane);
  await page.locator(".terminal canvas").waitFor();
  await readme.waitFor({ state: "hidden" });
  await page
    .locator('.doc-row[data-doc$="/readme.md"]')
    .getByRole("button", { name: /^Open file readme.md \(unsaved changes\)/ })
    .click();
  await readme.waitFor();
  assert.match(await readme.innerText(), /kept while away/);
  await page
    .locator('.files-editor:not([hidden])[data-path$="/readme.md"] .files-head .files-dirty')
    .waitFor();
  await readme.click();
  await page.keyboard.press("Control+s");
  await onDisk(
    () => read(join(home, "docs/readme.md")) === "# Read me\nkept while away\n",
    "saved after coming back",
  );
  console.log("PASS: unsaved edits survive switching to a terminal and back, then save");
  await page.locator('[data-view="files"]').click();
  await page.locator(".files-crumb", { hasText: "Home" }).click();
  await row(page, "src").waitFor();

  await page.evaluate((pane) => window.showTerminal(pane), pane);
  const canvas = page.locator(".terminal canvas");
  await canvas.waitFor();
  const cols = () => page.evaluate((pane) => window.screenCols(pane), pane);
  await page.waitForFunction((pane) => window.screenCols(pane) > 120, pane, {
    timeout: 10000,
  });
  await page.waitForTimeout(400);
  const wide = await cols();
  await page.evaluate(() => window.showFiles());
  await row(page, "src").getByRole("button", { name: /^src,/ }).click();
  await action(page, "x.py", "Open beside terminal");
  const panel = page.locator(".side-panel:not([hidden])");
  await panel.locator(".cm-content").waitFor();
  await canvas.waitFor();
  await page.waitForFunction(
    ({ pane, wide }) => window.screenCols(pane) < wide - 20,
    { pane, wide },
    { timeout: 10000 },
  );
  const narrow = await cols();
  console.log(`PASS: Open beside terminal shows the file beside it (${wide} -> ${narrow} columns)`);

  await panel.locator(".cm-content").click();
  await page.keyboard.press("Control+End");
  await page.keyboard.type("five = 5\n");
  await panel.locator(".files-dirty").waitFor();
  await page.keyboard.press("Control+s");
  await onDisk(
    () => read(join(home, "src/x.py"))?.endsWith("four = 4\nfive = 5\n"),
    "saved from the side panel",
  );
  await panel.locator(".files-dirty").waitFor({ state: "detached" });
  console.log("PASS: edits in the side panel save with Ctrl+S (checked on disk)");

  // Clicking the terminal gives it the keyboard; the panel's file is
  // untouched. Clear the paths typed earlier, then print three paths.
  await canvas.click();
  await page.keyboard.press("Control+u");
  await page.keyboard.type(
    "echo src/x.py:3; echo /etc/passwd; echo nothere/y.py:2\n",
  );
  const rowOf = (text) =>
    page.evaluate(
      ({ pane, text }) =>
        window
          .screenText(pane)
          .split("\n")
          .findLastIndex((line) => line.trimEnd() === text),
      { pane, text },
    );
  await page.waitForFunction(
    ({ pane }) =>
      window
        .screenText(pane)
        .split("\n")
        .some((line) => line.trimEnd() === "nothere/y.py:2"),
    { pane },
    { timeout: 10000 },
  );
  assert.equal(await panel.locator(".files-dirty").count(), 0);
  assert.ok(read(join(home, "src/x.py")).endsWith("five = 5\n"));
  console.log("PASS: clicking the terminal sends the keyboard to it, not the panel");

  await panel.getByRole("button", { name: "Close x.py" }).click();
  await page.locator(".side-panel").waitFor({ state: "detached" });
  await page.waitForFunction(
    ({ pane, wide }) => window.screenCols(pane) === wide,
    { pane, wide },
    { timeout: 10000 },
  );
  console.log("PASS: closing the side panel gives the terminal its columns back");

  // Cmd/Ctrl-click: a path that does not exist is never underlined nor
  // clickable; one that does opens beside the terminal at its line.
  const point = async (text) => {
    const y = await rowOf(text);
    assert.ok(y >= 0, `${text} on screen`);
    return page.evaluate((y) => window.cellPoint(4, y), y);
  };
  const missing = await point("nothere/y.py:2");
  await page.keyboard.down("Control");
  await page.mouse.move(missing.x, missing.y);
  await page.waitForTimeout(600);
  assert.equal(await page.locator(".path-link-underline").count(), 0);
  await page.mouse.click(missing.x, missing.y);
  await page.keyboard.up("Control");
  await page.waitForTimeout(400);
  assert.equal(await page.locator(".side-panel").count(), 0);
  console.log("PASS: a path that does not exist is not a link");

  // A path outside the file root (the home folder) exists but is never a
  // candidate: no stat is sent and nothing is underlined or opened.
  const outsidePath = await point("/etc/passwd");
  const statsBefore = await page.evaluate(() => window.stats.length);
  await page.keyboard.down("Control");
  await page.mouse.move(outsidePath.x, outsidePath.y);
  await page.waitForTimeout(600);
  assert.equal(await page.locator(".path-link-underline").count(), 0);
  await page.mouse.click(outsidePath.x, outsidePath.y);
  await page.keyboard.up("Control");
  await page.waitForTimeout(400);
  assert.equal(await page.locator(".side-panel").count(), 0);
  assert.deepEqual(
    (await page.evaluate(() => window.stats)).slice(statsBefore),
    [],
    "no stat for a path outside the root",
  );
  console.log("PASS: a path outside the file root is not a link (no stat sent)");

  const found = await point("src/x.py:3");
  await page.mouse.move(found.x + 30, found.y + 60);
  await page.keyboard.down("Control");
  await page.mouse.move(found.x, found.y);
  const underline = page.locator(".path-link-underline");
  await underline.waitFor();
  assert.equal(await underline.getAttribute("data-path"), join(home, "src/x.py"));
  assert.equal(
    await canvas.evaluate((c) => getComputedStyle(c).cursor),
    "pointer",
  );
  await shot(page, "desktop-path-hover");
  await page.mouse.down();
  await page.mouse.up();
  await page.keyboard.up("Control");
  await panel.locator(".cm-content").waitFor();
  await page.waitForFunction(
    () =>
      document.querySelector(".side-panel .cm-activeLine")?.textContent ===
      "three = 3",
    null,
    { timeout: 10000 },
  );
  await page.waitForFunction(
    ({ pane, wide }) => window.screenCols(pane) < wide - 20,
    { pane, wide },
    { timeout: 10000 },
  );
  await underline.waitFor({ state: "detached" });
  await page.waitForTimeout(300);
  await shot(page, "desktop-side-panel");
  console.log("PASS: Ctrl-click on a printed path:line opens it beside the terminal at that line");

  const errors = await page.evaluate(() => window.bridgeErrors);
  assert.deepEqual(errors, []);
  await close();

  // A bridge that does not relay downloads (API minor 2): no Download item
  // in the menus, no Download button in the editor.
  const old = await open(
    browser,
    { viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 },
    "?nodownloads=1",
  );
  await row(old.page, "docs").waitFor();
  await row(old.page, "report.txt").hover();
  await row(old.page, "report.txt")
    .getByRole("button", { name: "Actions for report.txt" })
    .click();
  await old.page.getByRole("menuitem", { name: "Copy path" }).waitFor();
  assert.equal(await old.page.getByRole("menuitem", { name: /Download/ }).count(), 0);
  await old.page.keyboard.press("Escape");
  await row(old.page, "report.txt").getByRole("button", { name: /^report.txt,/ }).click();
  await old.page.locator(".files-editor:not([hidden]) .cm-content").waitFor();
  assert.equal(await old.page.getByRole("button", { name: /^Download/ }).count(), 0);
  await old.close();
  console.log("PASS: without downloads on the bridge, no Download item or button");

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
  // A phone has no side panel: Open beside terminal opens the editor
  // full screen.
  const { pane: phonePane } = await phone.page.evaluate(
    (cwd) => window.session.rpc("new", { request: "files-phone", cwd }),
    home,
  );
  await phone.page.evaluate(
    ({ pane, home }) => window.openTerminal(pane, "shell", home),
    { pane: phonePane, home },
  );
  await action(phone.page, "todo.md", "Open beside terminal");
  await phone.page.locator(".files-editor .cm-content").waitFor();
  assert.equal(await phone.page.locator(".side-panel:not([hidden])").count(), 0);
  await phone.page.waitForTimeout(200);
  await shot(phone.page, "phone-open-beside");
  await phone.page.getByRole("button", { name: "Close todo.md" }).click();
  await row(phone.page, "docs").waitFor();
  console.log("PASS: on a phone, Open beside terminal opens the editor full screen");
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
