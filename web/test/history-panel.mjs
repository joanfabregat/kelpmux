// History (test/history-panel.fixture.tsx), in Chromium and WebKit, against
// a fake bridge kept here:
// 1. One list, newest first: closed terminals with the conversations they
//    hosted nested under them (tool, title, time span, Resume), a closed
//    terminal without one keeping Rerun and Open folder, and conversations
//    started outside KelpMux marked as such.
// 2. Resume sends `new` with the conversation's folder, its project (none
//    for Scratch) and `resume: {kind, id}`, never a command line; a
//    conversation whose folder is gone cannot be resumed.
// 3. A project's History shows its closed terminals (by their project link)
//    and its outside conversations only; the URL names it.
// 4. The former `dialog=closed` URL opens History and is rewritten.
// 5. A bridge without `conversations` gets `closed` and today's actions.
// Set HISTORY_SCREENSHOTS to a directory to save screenshots.
import { build } from "vite";
import { chromium, webkit, devices } from "playwright";
import { readFileSync, mkdirSync } from "node:fs";
import assert from "node:assert/strict";

const out =
  process.env.HISTORY_FIXTURE_OUTPUT || "/tmp/kelpmux-history-panel-fixture";
const shots = process.env.HISTORY_SCREENSHOTS;
if (shots) mkdirSync(shots, { recursive: true });
await build({
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  configFile: false,
  logLevel: "warn",
  root: new URL("..", import.meta.url).pathname,
  build: {
    outDir: out,
    emptyOutDir: true,
    lib: {
      entry: new URL("./history-panel.fixture.tsx", import.meta.url).pathname,
      name: "HistoryFixture",
      formats: ["iife"],
      cssFileName: "history-fixture",
      fileName: () => "fixture.js",
    },
    minify: false,
  },
});
const origin = "https://terminal.test";
const now = Math.floor(Date.now() / 1000);

function fakeBridge() {
  const calls = [];
  const entry = (key, name, project, more) => ({
    key,
    workspace: "main",
    pane: Number(key.split(":")[1]),
    cwd: "/home/me/work/alpha",
    name,
    kind: null,
    session_id: null,
    argv: [],
    closed_at: now - 600,
    reason: "Closed",
    choices: ["folder"],
    project,
    ...more,
  });
  const entries = [
    entry("e:1", "claude", "alpha", {
      kind: "claude",
      session_id: "c-1",
      choices: ["resume"],
      closed_at: now - 300,
      conversations: [
        {
          kind: "codex",
          id: "x-2",
          cwd: "/home/me/work/alpha/api",
          folder: "/home/me/work/alpha/api",
          title: "Write the release notes for the History view",
          titled: true,
          first_seen: now - 3 * 3600,
          last_seen: now - 400,
        },
        {
          kind: "claude",
          id: "c-1",
          cwd: "/home/me/work/alpha",
          folder: "/home/me/work/alpha",
          title: "Refactor the parser so it reads one line at a time",
          titled: true,
          first_seen: now - 2 * 86400,
          last_seen: now - 5 * 3600,
        },
      ],
    }),
    entry("e:2", "make", null, {
      cwd: "/home/me/scratch",
      argv: ["make", "test"],
      choices: ["restart", "folder"],
      closed_at: now - 7200,
      conversations: [],
    }),
    entry("e:3", "codex", "beta", {
      kind: "codex",
      session_id: "x-3",
      choices: ["resume"],
      cwd: "/home/me/gone",
      closed_at: now - 86400,
      conversations: [
        {
          kind: "codex",
          id: "x-3",
          cwd: "/home/me/gone",
          folder: null,
          title: "codex",
          titled: false,
          first_seen: null,
          last_seen: null,
        },
      ],
    }),
  ];
  const external = [
    {
      kind: "claude",
      id: "c-out",
      cwd: "/home/me/work/alpha/web",
      folder: "/home/me/work/alpha/web",
      title: "Started in a plain terminal",
      titled: true,
      first_seen: now - 4000,
      last_seen: now - 60,
      project: "alpha",
    },
    {
      kind: "codex",
      id: "x-out",
      cwd: "/home/me/scratch/idea",
      folder: "/home/me/scratch/idea",
      title: null,
      titled: false,
      first_seen: now - 90000,
      last_seen: now - 86000,
      project: "scratch",
    },
  ];
  return {
    calls,
    call(method, params) {
      calls.push([method, params]);
      switch (method) {
        case "conversations":
          return { workspace: "main", entries, external, truncated: false };
        case "closed":
          return {
            workspace: "main",
            entries: entries.map(({ conversations, ...rest }) => rest),
          };
        case "new":
          return { pane: 41 };
        case "restore":
          return { pane: 42, existing: false };
      }
      throw Error(`unexpected ${method}`);
    },
  };
}

async function open(browser, options, server, search = "") {
  const context = await browser.newContext(options);
  await context.route(`${origin}/**`, (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/")
      return route.fulfill({
        contentType: "text/html; charset=utf-8",
        body: '<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover"><link rel="stylesheet" href="/history-fixture.css"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>',
      });
    if (path === "/history-fixture.css")
      return route.fulfill({
        contentType: "text/css; charset=utf-8",
        body:
          readFileSync(`${out}/history-fixture.css`, "utf8") +
          "body{background:var(--bg);color:var(--text);margin:0}.fixture-nav{display:flex;gap:8px;padding:8px}",
      });
    if (path === "/fixture.js")
      return route.fulfill({
        contentType: "text/javascript; charset=utf-8",
        body: readFileSync(`${out}/fixture.js`),
      });
    return route.abort();
  });
  await context.exposeFunction("bridge", (method, params) =>
    server.call(method, params),
  );
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(`${origin}/${search}`);
  await page.waitForFunction(() => window.fixtureReady === true);
  return { context, page, errors };
}
async function shot(page, engine, name) {
  if (shots && engine === "chromium")
    await page.screenshot({ path: `${shots}/${name}.png` });
}
const dialog = '[data-dialog="recovery"]';
/** The list's top-level rows: an entry key or an outside conversation. */
const rows = (page) =>
  page.$$eval(`${dialog} .panel-list > [role="listitem"]`, (all) =>
    all.map((row) => row.dataset.entry || `outside:${row.dataset.conversation}`),
  );
const search = (page) => page.evaluate(() => location.search);

for (const [engine, launcher] of [
  ["chromium", chromium],
  ["webkit", webkit],
]) {
  const browser = await launcher.launch();
  try {
    // 1. The whole History, newest first.
    const server = fakeBridge();
    const { context, page, errors } = await open(
      browser,
      { viewport: { width: 1100, height: 800 } },
      server,
      "?view=grid&dialog=history",
    );
    await page.waitForSelector(`${dialog} .history-group`);
    assert.deepEqual(await rows(page), [
      "outside:c-out",
      "e:1",
      "e:2",
      "outside:x-out",
      "e:3",
    ]);
    assert.equal(server.calls[0][0], "conversations");
    const alpha = page.locator(`${dialog} [data-entry="e:1"]`);
    assert.deepEqual(
      await alpha.locator(".history-conversation .t").allTextContents(),
      [
        "Write the release notes for the History view",
        "Refactor the parser so it reads one line at a time",
      ],
    );
    assert.match(
      await alpha.locator(".history-conversation").first().innerText(),
      /Codex · \/home\/me\/work\/alpha\/api · 3h ago – 6m ago/,
    );
    assert.equal(await alpha.locator(".history-conversation .provider-logo").count(), 2);
    // The terminal row itself offers no restore action: its conversations do.
    assert.equal(
      await alpha.locator(".history-terminal button").count(),
      0,
    );
    const make = page.locator(`${dialog} [data-entry="e:2"]`);
    assert.deepEqual(
      (await make.locator("button").allTextContents()).map((t) => t.trim()),
      ["Rerun", "Open folder"],
    );
    const outside = page.locator(`${dialog} .history-external`);
    assert.equal(await outside.count(), 2);
    assert.match(await outside.first().innerText(), /outside KelpMux/);
    assert.match(
      await outside.nth(1).locator(".t").innerText(),
      /Conversation in idea/,
    );
    // A conversation whose folder is gone cannot be resumed.
    assert.equal(
      await page
        .locator(`${dialog} [data-entry="e:3"] .history-conversation button.success`)
        .isDisabled(),
      true,
    );
    await shot(page, engine, "history-dark");

    // 2. Resume sends `new` with the conversation's folder and project.
    await alpha
      .getByRole("button", {
        name: "Resume Write the release notes for the History view",
      })
      .click();
    await page.waitForFunction(() => window.restored.length === 1);
    const resume = server.calls.find(([method]) => method === "new")[1];
    assert.equal(typeof resume.request, "string");
    assert.deepEqual(
      { ...resume, request: undefined },
      {
        request: undefined,
        project: "alpha",
        cwd: "/home/me/work/alpha/api",
        resume: { kind: "codex", id: "x-2" },
      },
    );
    assert.deepEqual(await page.evaluate(() => window.restored), [[41, false]]);
    // An outside conversation in Scratch: no project.
    await page
      .locator(`${dialog} .history-external[data-conversation="x-out"]`)
      .getByRole("button", { name: /^Resume / })
      .click();
    await page.waitForFunction(() => window.restored.length === 2);
    const scratch = server.calls.filter(([method]) => method === "new")[1][1];
    assert.equal("project" in scratch, false);
    assert.deepEqual(scratch.resume, { kind: "codex", id: "x-out" });
    assert.equal(scratch.cwd, "/home/me/scratch/idea");
    // The copy button copies the resume command.
    await alpha
      .getByRole("button", {
        name: "Copy the resume command of Refactor the parser so it reads one line at a time",
      })
      .click();
    assert.equal(await page.evaluate(() => window.copied), "claude --resume c-1");
    // A terminal without conversations keeps its restore choices.
    await make.getByRole("button", { name: "Rerun make" }).click();
    await page.waitForFunction(() => window.restored.length === 3);
    assert.deepEqual(
      server.calls.find(([method]) => method === "restore")[1].choice,
      "restart",
    );

    // 3. A project's History: its terminals and its outside conversations.
    await page.locator("#scope-alpha").dispatchEvent("click");
    await page.waitForFunction(
      () => document.querySelector("#modal-title")?.textContent === "History · Alpha",
    );
    await page.waitForSelector(`${dialog} .history-group`);
    assert.deepEqual(await rows(page), ["outside:c-out", "e:1"]);
    assert.match(await search(page), /dialog=history&project=alpha/);
    assert.match(await page.locator(".project-scope").innerText(), /Alpha · 2 of 5/);
    await shot(page, engine, "history-project-dark");
    await page.locator("#scope-scratch").dispatchEvent("click");
    await page.waitForFunction(
      () => document.querySelector("#modal-title")?.textContent === "History · Scratch",
    );
    await page.waitForSelector(`${dialog} .history-group`);
    assert.deepEqual(await rows(page), ["e:2", "outside:x-out"]);
    await page.getByRole("button", { name: "All projects" }).click();
    await page.waitForFunction(
      () => document.querySelector("#modal-title")?.textContent === "History",
    );
    await page.waitForSelector(`${dialog} .history-group`);
    assert.equal((await rows(page)).length, 5);
    assert.deepEqual(errors, []);
    await context.close();

    // 4. The former URL of Recently closed opens History, renamed.
    {
      const legacy = await open(
        browser,
        { viewport: { width: 1100, height: 800 } },
        fakeBridge(),
        "?view=grid&dialog=closed&project=beta",
      );
      await legacy.page.waitForSelector(`${dialog} .history-group`);
      assert.deepEqual(await rows(legacy.page), ["e:3"]);
      assert.equal(await search(legacy.page), "?view=grid&dialog=history&project=beta");
      assert.deepEqual(legacy.errors, []);
      await legacy.context.close();
    }

    // 5. A bridge before `conversations`: `closed`, with today's actions.
    {
      const old = fakeBridge();
      const before = await open(
        browser,
        { viewport: { width: 1100, height: 800 } },
        old,
        "?view=grid&dialog=history&old=1",
      );
      await before.page.waitForSelector(`${dialog} .history-group`);
      assert.equal(old.calls[0][0], "closed");
      assert.deepEqual(await rows(before.page), ["e:1", "e:2", "e:3"]);
      assert.equal(await before.page.locator(".history-conversation").count(), 0);
      assert.deepEqual(
        await before.page
          .locator(`${dialog} [data-entry="e:1"] button`)
          .allTextContents()
          .then((all) => all.map((t) => t.trim())),
        ["", "Resume"],
      );
      await before.context.close();
    }

    // Light theme and a phone.
    if (engine === "chromium") {
      for (const [name, scope] of [
        ["history-light", ""],
        ["history-project-light", "&project=alpha"],
      ]) {
        const light = await open(
          browser,
          { viewport: { width: 1100, height: 800 } },
          fakeBridge(),
          `?view=grid&dialog=history${scope}&mode=light`,
        );
        await light.page.waitForSelector(`${dialog} .history-group`);
        await shot(light.page, engine, name);
        await light.context.close();
      }
      for (const mode of ["dark", "light"]) {
        const phone = await open(
          browser,
          { ...devices["iPhone 15"], defaultBrowserType: undefined },
          fakeBridge(),
          `?view=grid&dialog=history&phone=1&mode=${mode}`,
        );
        await phone.page.waitForSelector(`${dialog} .history-group`);
        const overflow = await phone.page.evaluate(
          () => document.documentElement.scrollWidth - window.innerWidth,
        );
        assert.ok(overflow <= 0, `no horizontal scroll on a phone (${overflow})`);
        await shot(phone.page, engine, `history-phone-${mode}`);
        await phone.context.close();
      }
    }
    console.log(
      `PASS: ${engine} history: closed terminals with nested conversations (tool, title, time span), outside conversations marked, Resume sends new with folder, project and resume, gone folders disabled, copy command, rerun kept, per-project filtering (projects and Scratch) named in the URL, the former dialog=closed URL, an older bridge's closed list`,
    );
  } finally {
    await browser.close();
  }
}
