// The upload dialog (test/upload.fixture.tsx), in Chromium and WebKit,
// against a fake server, without any network or SSH endpoint:
// 1. Files dropped onto a terminal open the dialog with the terminal, the
//    names and sizes and the destination; Upload sends them in windowed
//    chunks and pastes the shell-quoted paths, without Enter.
// 2. The advertised limits are enforced before anything is sent.
// 3. A destination override is sent as `dest` and can be remembered; the
//    server's refusal is shown.
// 4. Cancel sends `upload_cancel`.
// 5. When the terminal closed, the paths are shown with copy buttons.
// 6. A server without the `uploads` capability shows a clear message.
import { build } from "vite";
import { chromium, webkit } from "playwright";
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const out = process.env.UPLOAD_FIXTURE_OUTPUT || "/tmp/kelpmux-upload-fixture";
await build({
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  configFile: false,
  logLevel: "warn",
  root: new URL("..", import.meta.url).pathname,
  build: {
    outDir: out,
    emptyOutDir: true,
    lib: {
      entry: new URL("./upload.fixture.tsx", import.meta.url).pathname,
      name: "UploadFixture",
      formats: ["iife"],
      cssFileName: "upload-fixture",
      fileName: () => "fixture.js",
    },
    minify: false,
  },
});
const origin = "https://terminal.test";
async function open(browser, search = "") {
  const context = await browser.newContext({
    viewport: { width: 1200, height: 800 },
  });
  await context.route(`${origin}/**`, (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/")
      return route.fulfill({
        contentType: "text/html; charset=utf-8",
        body: '<html><head><meta charset="utf-8"><link rel="stylesheet" href="/upload-fixture.css"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>',
      });
    if (path.endsWith(".css"))
      return route.fulfill({
        contentType: "text/css; charset=utf-8",
        body: readFileSync(`${out}/upload-fixture.css`),
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
  await page.waitForFunction(() => window.fixtureReady === true);
  return { context, page, errors };
}
/** Drops files (name and size) onto the terminal, as a desktop drag does. */
async function dropFiles(page, files) {
  await page.evaluate((files) => {
    const data = new DataTransfer();
    for (const [name, size] of files)
      data.items.add(
        new File([new Uint8Array(size).fill(97)], name, {
          type: "application/octet-stream",
        }),
      );
    const target = document.querySelector("[data-upload-pane]");
    for (const type of ["dragenter", "dragover", "drop"])
      target.dispatchEvent(
        new DragEvent(type, {
          bubbles: true,
          cancelable: true,
          dataTransfer: data,
        }),
      );
  }, files);
}
const sent = (page, type) =>
  page.evaluate((type) => window.sent.filter((m) => m.type === type), type);

for (const engine of [chromium, webkit]) {
  const browser = await engine.launch({ headless: true });
  const name = engine.name();
  try {
    // 1. Drop, review, upload, paste.
    {
      const { context, page, errors } = await open(browser);
      await dropFiles(page, [
        ["photo 1.png", 600 * 1024],
        ["it's.txt", 13],
      ]);
      const dialog = page.getByRole("dialog", { name: "Upload files" });
      await dialog.waitFor();
      assert.match(
        await dialog.locator(".upload-target").innerText(),
        /Terminal 1 · shell/,
      );
      const rows = await dialog.locator(".upload-files li").allInnerTexts();
      assert.equal(rows.length, 2);
      assert.match(rows[0], /photo 1\.png\s+600 KiB/);
      assert.match(rows[1], /it's\.txt\s+13 B/);
      assert.equal(
        await dialog
          .getByLabel("Destination folder on the server")
          .inputValue(),
        "",
      );
      assert.match(await dialog.innerText(), /server's upload folder/);
      await dialog.getByRole("button", { name: "Upload", exact: true }).click();
      await page.waitForFunction(() => window.pasted !== null);
      assert.equal(
        await page.evaluate(() => window.pasted),
        "'/home/me/.cache/kelpserver/uploads/1795300000-3f9c/photo 1.png' '/home/me/.cache/kelpserver/uploads/1795300000-3f9c/it'\\''s.txt'",
      );
      await dialog.waitFor({ state: "detached" });
      const begin = (await sent(page, "upload_begin"))[0];
      assert.deepEqual(begin.files, [
        { name: "photo 1.png", size: 600 * 1024 },
        { name: "it's.txt", size: 13 },
      ]);
      assert.equal(begin.dest, undefined, "the server's own upload folder");
      const chunks = await sent(page, "upload_chunk");
      assert.deepEqual(
        chunks.map((c) => [c.file_index, c.offset]),
        [
          [0, 0],
          [0, 128 * 1024],
          [0, 256 * 1024],
          [0, 384 * 1024],
          [0, 512 * 1024],
          [1, 0],
        ],
      );
      assert.equal((await sent(page, "upload_finish")).length, 1);
      assert.deepEqual(errors, []);
      await context.close();
    }
    // 2. Limits, and folders, before anything is sent.
    {
      const { context, page, errors } = await open(browser, "?max_file=1000");
      await dropFiles(page, [["big.bin", 1001]]);
      const dialog = page.getByRole("dialog", { name: "Upload files" });
      await dialog.waitFor();
      assert.match(
        await dialog.getByRole("alert").innerText(),
        /big\.bin is larger than the 1000 B limit per file/,
      );
      assert.equal(
        await dialog
          .getByRole("button", { name: "Upload", exact: true })
          .isDisabled(),
        true,
      );
      await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
      await dialog.waitFor({ state: "detached" });
      assert.equal((await sent(page, "upload_begin")).length, 0);
      assert.deepEqual(errors, []);
      await context.close();
    }
    // 3. Destination: override, remember, refusal; then the picker.
    {
      const { context, page, errors } = await open(browser);
      await dropFiles(page, [["a.txt", 3]]);
      const dialog = page.getByRole("dialog", { name: "Upload files" });
      const folder = dialog.getByLabel("Destination folder on the server");
      await folder.fill("project");
      assert.match(await dialog.getByRole("alert").innerText(), /absolute/);
      await folder.fill("/home/me/project/");
      await dialog.getByLabel("Remember as the default destination").check();
      await dialog.getByRole("button", { name: "Upload", exact: true }).click();
      await page.waitForFunction(() => window.pasted !== null);
      assert.equal(
        await page.evaluate(() => window.pasted),
        "/home/me/project/1795300000-3f9c/a.txt",
      );
      assert.equal(
        (await sent(page, "upload_begin"))[0].dest,
        "/home/me/project",
      );
      assert.equal(
        await page.evaluate(() =>
          localStorage.getItem("kelpmux-web-upload-destination"),
        ),
        "/home/me/project",
      );
      // The remembered folder is the next drop's default, and Settings
      // shows it.
      await page
        .locator("input[data-upload-picker]")
        .setInputFiles([
          { name: "b.txt", mimeType: "text/plain", buffer: Buffer.from("b") },
        ]);
      await dialog.waitFor();
      assert.equal(await folder.inputValue(), "/home/me/project");
      await folder.fill("/refused");
      await dialog.getByRole("button", { name: "Upload", exact: true }).click();
      assert.match(
        await dialog.getByRole("alert").innerText(),
        /not a directory owned/,
      );
      await dialog.getByRole("button", { name: "Close", exact: true }).click();
      await page.getByRole("button", { name: "Settings" }).click();
      assert.equal(
        await page.getByLabel("Default upload destination").inputValue(),
        "/home/me/project",
      );
      await page
        .getByRole("button", { name: "Use the server's upload folder" })
        .click();
      assert.equal(
        await page.evaluate(() =>
          localStorage.getItem("kelpmux-web-upload-destination"),
        ),
        null,
      );
      assert.deepEqual(errors, []);
      await context.close();
    }
    // 4. Cancel during the upload.
    {
      const { context, page, errors } = await open(browser);
      await page.evaluate(() => {
        window.hold = true;
      });
      await dropFiles(page, [["large.bin", 2 * 1024 * 1024]]);
      const dialog = page.getByRole("dialog", { name: "Upload files" });
      await dialog.getByRole("button", { name: "Upload", exact: true }).click();
      await page.waitForFunction(
        () => window.sent.filter((m) => m.type === "upload_chunk").length === 8,
      );
      await dialog.getByRole("status").waitFor();
      assert.match(await dialog.getByRole("status").innerText(), /Uploading/);
      // Never more than the window of eight chunks while nothing is
      // acknowledged.
      await page.waitForTimeout(100);
      assert.equal((await sent(page, "upload_chunk")).length, 8);
      await dialog.getByRole("button", { name: "Cancel upload" }).click();
      await dialog.waitFor({ state: "detached" });
      const cancel = await sent(page, "upload_cancel");
      assert.equal(cancel.length, 1);
      assert.equal(cancel[0].batch, 1);
      assert.equal((await sent(page, "upload_finish")).length, 0);
      assert.equal(await page.evaluate(() => window.pasted), null);
      assert.deepEqual(errors, []);
      await context.close();
    }
    // 5. The terminal closed: the paths with copy buttons.
    {
      const { context, page, errors } = await open(browser, "?closed=1");
      await dropFiles(page, [["a b.txt", 3]]);
      const dialog = page.getByRole("dialog", { name: "Upload files" });
      await dialog.getByRole("button", { name: "Upload", exact: true }).click();
      const done = page.getByRole("dialog", { name: "Files uploaded" });
      await done.waitFor();
      assert.match(await done.innerText(), /Terminal 1 closed/);
      const path = "/home/me/.cache/kelpserver/uploads/1795300000-3f9c/a b.txt";
      await done.getByRole("button", { name: `Copy ${path}` }).click();
      assert.equal(await page.evaluate(() => window.copied), path);
      await done.getByRole("button", { name: "Copy all" }).click();
      assert.equal(await page.evaluate(() => window.copied), `'${path}'`);
      assert.deepEqual(errors, []);
      await context.close();
    }
    // 6. No uploads capability.
    {
      const { context, page, errors } = await open(browser, "?unavailable=1");
      await dropFiles(page, [["a.txt", 3]]);
      const dialog = page.getByRole("dialog", { name: "Upload files" });
      await dialog.waitFor();
      assert.match(
        await dialog.getByRole("alert").innerText(),
        /lacks the uploads capability/,
      );
      assert.equal(
        await dialog
          .getByRole("button", { name: "Upload", exact: true })
          .isDisabled(),
        true,
      );
      assert.equal((await sent(page, "upload_begin")).length, 0);
      assert.deepEqual(errors, []);
      await context.close();
    }
    console.log(
      `PASS: ${name} uploads: drop dialog, windowed chunks, quoted paste, limits, destination, cancel, closed terminal, no capability`,
    );
  } finally {
    await browser.close();
  }
}
