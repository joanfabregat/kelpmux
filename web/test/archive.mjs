// Project status, archive, restore and delete in the web client
// (test/archive.fixture.tsx), in Chromium and WebKit, against a fake bridge
// kept here; nothing reaches a storage bucket:
// 1. The sidebar dims a paused project and keeps archived ones in a folded
//    Archived group that opens (and stays open in this browser).
// 2. Pause and Resume from the project menu.
// 3. Archive…: the safe-removal check (git items are warnings, with the
//    note that they will exist only in the archive), then the job with
//    the check's fingerprint, its phases and progress, done: the project
//    moves to the Archived group. Cancel stops another one.
// 4. Delete…: blockers keep Delete the folder disabled, even with the name
//    typed; Remove from KelpMux only stays possible. A clean project needs
//    its exact name typed, then the folder is deleted and the project
//    removed.
// 5. An archived project's page: when and where it was archived, Restore…
//    lists the archives (the recorded one first), restores into its folder,
//    or into a new folder when that exists again.
// 6. Settings → Archive: configured (Test writes and deletes) and not.
// 7. A phone: the dialog fits its width.
// Set ARCHIVE_SCREENSHOTS to a directory to save screenshots.
import { build } from "vite";
import { chromium, webkit, devices } from "playwright";
import { readFileSync, mkdirSync } from "node:fs";
import assert from "node:assert/strict";

const out = process.env.ARCHIVE_FIXTURE_OUTPUT || "/tmp/kelpmux-archive-fixture";
const shots = process.env.ARCHIVE_SCREENSHOTS;
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
      entry: new URL("./archive.fixture.tsx", import.meta.url).pathname,
      name: "ArchiveFixture",
      formats: ["iife"],
      cssFileName: "archive-fixture",
      fileName: () => "fixture.js",
    },
    minify: false,
  },
});
const origin = "https://terminal.test";
const now = Math.floor(Date.now() / 1000);

const project = (id, name, fields = {}) => ({
  id,
  name,
  folder: `~/work/${name}`,
  path: `/home/me/work/${name}`,
  color: null,
  missing: false,
  status: "active",
  archive: null,
  ...fields,
});
const record = (id) => ({
  backend: "gcs",
  bucket: "fogandfrog-kelpmux",
  object: `kelpmux/${id}/20261120T093000Z.tar.gz`,
  size: 48 * 1024 * 1024,
  sha256: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
  crc32c: "yZRlqg==",
  archived_at: "2026-11-20T09:30:00Z",
});

/** The fake bridge: Alpha (checkouts with work on no remote), Beta
 * (paused), Delta (clean), Gamma (archived, folder gone) and Epsilon
 * (archived, its folder back). Jobs advance one phase per poll. */
function fakeBridge({ configured = true } = {}) {
  const state = {
    calls: [],
    projects: [
      project("alpha", "Alpha", { color: "blue" }),
      project("beta", "Beta", { status: "paused" }),
      project("delta", "Delta"),
      project("gamma", "Gamma", { status: "archived", missing: true, archive: record("gamma") }),
      project("epsilon", "Epsilon", { status: "archived", archive: record("epsilon") }),
    ],
    jobs: [],
  };
  let next = 0;
  const byId = (id) => state.projects.find((p) => p.id === id);
  const fingerprint = (id, mode) => `fp-${id}-${mode}`;
  const check = (id, mode) => {
    const p = byId(id);
    const dirty = id === "alpha" || id === "beta";
    const severity = mode === "archive" ? "warning" : "blocker";
    const items = dirty
      ? [
          { severity, kind: "uncommitted", path: `${p.path}/api`, message: "2 uncommitted changes in api" },
          { severity, kind: "unpushed", path: `${p.path}/api`, message: "feature: 3 commits on no remote" },
          { severity, kind: "stash", path: `${p.path}/api`, message: "1 stash in api" },
          { severity: "info", kind: "ignored", path: `${p.path}/api`, message: "14 ignored paths in api" },
        ]
      : [];
    return {
      project: id,
      folder: p.path,
      mode,
      exists: true,
      checked_at: now,
      fingerprint: fingerprint(id, mode),
      checkouts: [
        {
          path: `${p.path}/api`,
          name: "api",
          error: null,
          branch: "main",
          detached: false,
          uncommitted: dirty ? 2 : 0,
          untracked: 0,
          ignored: dirty ? 14 : 0,
          stashes: dirty ? 1 : 0,
          branches: [
            { name: "main", upstream: "origin/main", unpushed: 0 },
            ...(dirty ? [{ name: "feature", upstream: null, unpushed: 3 }] : []),
          ],
        },
      ],
      other_files: { files: 37, bytes: 5 * 1024 * 1024, truncated: false },
      items,
      blockers: items.filter((i) => i.severity === "blocker").length,
      warnings: items.filter((i) => i.severity === "warning").length,
      allowed: !items.some((i) => i.severity === "blocker"),
    };
  };
  const phases = {
    archive: ["checking", "packing", "uploading", "verifying", "removing"],
    restore: ["listing", "downloading", "extracting"],
    delete: ["checking", "removing"],
  };
  const advance = () => {
    for (const job of state.jobs) {
      if (job.state !== "running" || job.hold) continue;
      const list = phases[job.kind];
      const index = list.indexOf(job.phase);
      if (index + 1 < list.length) {
        job.phase = list[index + 1];
        job.bytes_done = Math.round(((index + 1) / list.length) * job.bytes_total);
        job.cancellable = job.phase !== "removing";
        job.updated_at++;
        continue;
      }
      job.state = "done";
      job.phase = "done";
      job.cancellable = false;
      job.bytes_done = job.bytes_total;
      const p = byId(job.project);
      if (job.kind === "archive") Object.assign(p, { status: "archived", missing: true, archive: record(p.id) });
      if (job.kind === "restore") Object.assign(p, { status: "active", missing: false, ...(job.folder ? { folder: job.folder } : {}) });
      if (job.kind === "delete") {
        if (job.remove) state.projects = state.projects.filter((x) => x !== p);
        else p.missing = true;
        job.result = { removed: job.remove };
      }
    }
  };
  const start = (kind, id, extra = {}) => {
    if (state.jobs.some((j) => j.project === id && j.state === "running"))
      throw Error("another archive job runs for this project");
    state.jobs.push({
      id: `job-${++next}`,
      project: id,
      kind,
      state: "running",
      phase: phases[kind][0],
      message: kind === "archive" ? "kelpmux/x.tar.gz" : "",
      bytes_done: 0,
      bytes_total: 64 * 1024 * 1024,
      files_done: 120,
      started_at: now,
      updated_at: now,
      error: null,
      result: null,
      cancellable: true,
      ...extra,
    });
    return jobs();
  };
  const jobs = () => ({ jobs: state.jobs.map(({ hold, folder, remove, ...job }) => job) });
  return {
    state,
    call(method, params) {
      state.calls.push([method, params]);
      switch (method) {
        case "config":
          return { projects: state.projects };
        case "closed":
          return { workspace: "", entries: [] };
        case "project_status": {
          const p = byId(params.id);
          if (p.status === "archived") throw Error("restore an archived project");
          p.status = params.status;
          return { saved: true, errors: [], config: { projects: state.projects } };
        }
        case "project_check_removal":
          return check(params.id, params.mode);
        case "project_archive":
          if (params.fingerprint !== fingerprint(params.id, "archive"))
            throw Error("the folder changed since the check");
          return start("archive", params.id, { hold: params.id === "beta" });
        case "project_delete_folder": {
          const p = byId(params.id);
          if (params.name !== p.name) throw Error("the name does not match");
          if (!check(params.id, "delete").allowed) throw Error("blocked");
          return start("delete", params.id, { remove: params.remove });
        }
        case "project_delete":
          state.projects = state.projects.filter((p) => p.id !== params.id);
          return { saved: true, errors: [], config: { projects: state.projects } };
        case "project_restore":
          return start("restore", params.id, { folder: params.folder });
        case "archive_jobs":
          advance();
          return jobs();
        case "archive_cancel": {
          const job = state.jobs.find((j) => j.project === params.project && j.state === "running");
          if (!job?.cancellable) throw Error("this job cannot be cancelled now");
          Object.assign(job, { state: "cancelled", error: "cancelled; the folder is untouched", cancellable: false });
          return jobs();
        }
        case "archive_list":
          return {
            archives: [
              { object: `kelpmux/${params.id}/20261123T180000Z.tar.gz`, size: 50 * 1024 * 1024, updated: "2026-11-23T18:00:00Z", sha256: null, crc32c: "AAAAAA==", recorded: false },
              { object: `kelpmux/${params.id}/20261120T093000Z.tar.gz`, size: 48 * 1024 * 1024, updated: "2026-11-20T09:30:00Z", sha256: "9f86", crc32c: "yZRlqg==", recorded: true },
            ],
          };
        case "archive_status":
          return configured
            ? { configured: true, backend: "gcs", bucket: "fogandfrog-kelpmux", prefix: "kelpmux", token_source: "metadata", token_label: "This machine's Google identity (metadata server)", error: null }
            : { configured: false, backend: null, bucket: null, prefix: "kelpmux", token_source: "metadata", token_label: "", error: null };
        case "archive_test":
          return { ok: true, object: "kelpmux/.kelpmux-test/1a2b3c", elapsed_ms: 412 };
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
        body: '<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover"><link rel="stylesheet" href="/archive-fixture.css"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>',
      });
    if (path === "/archive-fixture.css")
      return route.fulfill({
        contentType: "text/css; charset=utf-8",
        body: readFileSync(`${out}/archive-fixture.css`, "utf8"),
      });
    if (path === "/fixture.js")
      return route.fulfill({
        contentType: "text/javascript; charset=utf-8",
        body: readFileSync(`${out}/fixture.js`),
      });
    return route.abort();
  });
  await context.exposeFunction("bridge", (method, params) => server.call(method, params));
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(`${origin}/${search}`);
  await page.waitForFunction(() => window.fixtureReady === true);
  return { context, page, errors };
}
const calls = (server, method) =>
  server.state.calls.filter(([m]) => m === method).map(([, p]) => p);
async function until(condition) {
  for (let i = 0; i < 100 && !condition(); i++)
    await new Promise((resolve) => setTimeout(resolve, 50));
  assert.ok(condition(), "condition not met in time");
}
const group = (page, id) => page.locator(`[data-project-group="${id}"]`);
const wide = { viewport: { width: 1280, height: 800 } };
async function menu(page, name, item) {
  await page.locator(".rail").getByRole("button", { name: `Menu for ${name}` }).click();
  await page.getByRole("menuitem", { name: item }).click();
}

for (const [name, engine] of [
  ["chromium", chromium],
  ["webkit", webkit],
]) {
  const browser = await engine.launch();
  const shoot = (page, file, locator) =>
    shots && name === "chromium"
      ? (locator || page).screenshot({ path: `${shots}/${file}` })
      : null;
  try {
    const server = fakeBridge();
    const { context, page, errors } = await open(browser, wide, server);

    // 1. Paused dimmed, archived folded away.
    await group(page, "alpha").waitFor();
    assert.equal(await group(page, "beta").getAttribute("data-status"), "paused");
    assert.match(await group(page, "beta").getAttribute("class"), /\bpaused\b/);
    const opacity = await group(page, "beta")
      .locator(".project-head")
      .evaluate((head) => Number(getComputedStyle(head).opacity));
    assert.ok(opacity < 0.9, `a paused project is dimmed (${opacity})`);
    const archivedGroup = page.locator("[data-archived-group]");
    const toggle = archivedGroup.getByRole("button", { name: /Archived/ });
    assert.equal(await toggle.getAttribute("aria-expanded"), "false");
    assert.match(await toggle.innerText(), /2/);
    assert.equal(await group(page, "gamma").count(), 0, "archived projects are folded away");
    await toggle.click();
    await group(page, "gamma").waitFor();
    assert.equal(await page.evaluate(() => localStorage.getItem("kelpmux-archived-open")), "1");
    await shoot(page, "sidebar.png", page.locator(".rail"));

    // 2. Pause and Resume.
    await menu(page, "Delta", "Pause");
    await until(() => calls(server, "project_status").length === 1);
    assert.deepEqual(calls(server, "project_status")[0], { id: "delta", status: "paused" });
    await page.locator('[data-project-group="delta"][data-status="paused"]').waitFor();
    await menu(page, "Delta", "Resume");
    await page.locator('[data-project-group="delta"][data-status="active"]').waitFor();

    // 3. Archive Alpha: the check (warnings, with the note), then the job.
    await menu(page, "Alpha", "Archive…");
    const dialog = page.getByRole("dialog", { name: "Archive “Alpha”" });
    await dialog.locator(".removal-check").waitFor();
    assert.deepEqual(calls(server, "project_check_removal")[0], { id: "alpha", mode: "archive" });
    const report = await dialog.innerText();
    assert.match(report, /3 warnings; nothing blocks it/);
    assert.match(report, /exist only in the archive/);
    assert.match(report, /feature: 3 commits on no remote/);
    assert.match(report, /37 other files \(5\.0 MiB\)/);
    assert.match(report, /gs:\/\/fogandfrog-kelpmux\/kelpmux\/alpha\//);
    await shoot(page, "archive-check.png", dialog);
    await dialog.getByRole("button", { name: "Archive and remove the folder" }).click();
    await until(() => calls(server, "project_archive").length === 1);
    assert.deepEqual(calls(server, "project_archive")[0], { id: "alpha", fingerprint: "fp-alpha-archive" });
    const job = dialog.locator(".archive-job");
    await job.waitFor();
    await dialog.locator('.archive-job[data-phase="uploading"]').waitFor();
    assert.match(await job.innerText(), /Archive: Uploading/);
    await shoot(page, "archive-progress.png", dialog);
    await dialog.locator('.archive-job[data-state="done"]').waitFor({ timeout: 15000 });
    assert.ok(await page.evaluate(() => window.toasts.includes("archive done")));
    await dialog.getByRole("button", { name: "Close" }).click();
    await page.locator('[data-archived-group] [data-project-group="alpha"]').waitFor();

    // Cancel: Beta's archive holds until cancelled.
    await menu(page, "Beta", "Archive…");
    const beta = page.getByRole("dialog", { name: "Archive “Beta”" });
    await beta.getByRole("button", { name: "Archive and remove the folder" }).click();
    await beta.locator(".archive-job").waitFor();
    await beta.getByRole("button", { name: "Cancel" }).click();
    await until(() => calls(server, "archive_cancel").length === 1);
    assert.deepEqual(calls(server, "archive_cancel")[0], { project: "beta" });
    await beta.locator('.archive-job[data-state="cancelled"]').waitFor();
    assert.match(await beta.innerText(), /the folder is untouched/);
    await beta.getByRole("button", { name: "Close" }).click();

    // 4. Delete: Beta's blockers keep Delete the folder disabled.
    await menu(page, "Beta", "Delete…");
    const del = page.getByRole("dialog", { name: "Delete “Beta”" });
    await del.locator(".removal-check").waitFor();
    assert.match(await del.innerText(), /3 problems must be fixed first/);
    assert.equal(await del.locator('[data-severity="blocker"]').count(), 3);
    await del.getByLabel("Project name to confirm").fill("Beta");
    assert.ok(await del.getByRole("button", { name: "Delete the folder" }).isDisabled());
    await shoot(page, "delete-blocked.png", del);
    await del.getByLabel("Remove from KelpMux only (keep the folder)").check();
    assert.ok(await del.getByRole("button", { name: "Remove from KelpMux" }).isEnabled());
    await del.getByRole("button", { name: "Cancel" }).click();
    await del.waitFor({ state: "detached" });
    assert.equal(calls(server, "project_delete_folder").length, 0);

    // Delta is clean: its exact name is needed.
    await menu(page, "Delta", "Delete…");
    const delta = page.getByRole("dialog", { name: "Delete “Delta”" });
    await delta.locator(".removal-check").waitFor();
    const button = delta.getByRole("button", { name: "Delete the folder" });
    assert.ok(await button.isDisabled(), "the name must be typed");
    await delta.getByLabel("Project name to confirm").fill("delta");
    assert.ok(await button.isDisabled(), "the exact name");
    await delta.getByLabel("Project name to confirm").fill("Delta");
    assert.ok(await button.isEnabled());
    await shoot(page, "delete-confirm.png", delta);
    await button.click();
    await until(() => calls(server, "project_delete_folder").length === 1);
    assert.deepEqual(calls(server, "project_delete_folder")[0], {
      id: "delta",
      fingerprint: "fp-delta-delete",
      name: "Delta",
      remove: true,
    });
    await delta.locator('.archive-job[data-state="done"]').waitFor({ timeout: 15000 });
    await delta.getByRole("button", { name: "Close" }).click();
    await group(page, "delta").waitFor({ state: "detached" });

    // 5. Gamma's page: archived, Restore into its folder.
    await group(page, "gamma").getByRole("button", { name: /^Gamma/ }).click();
    const panel = page.locator('[data-archived="gamma"]');
    await panel.waitFor();
    const facts = await panel.innerText();
    assert.match(facts, /gs:\/\/fogandfrog-kelpmux\/kelpmux\/gamma\/20261120T093000Z\.tar\.gz/);
    assert.match(facts, /48\.0 MiB/);
    assert.equal(await page.locator(".project-home").getByRole("button", { name: "New terminal" }).count(), 0);
    assert.equal(await page.locator('[data-section="terminals"]').count(), 0);
    await shoot(page, "archived-home.png");
    await panel.getByRole("button", { name: "Restore…" }).click();
    const restore = page.getByRole("dialog", { name: "Restore “Gamma”" });
    await restore.locator("[data-archive]").first().waitFor();
    assert.ok(await restore.locator('[data-archive="kelpmux/gamma/20261120T093000Z.tar.gz"] input').isChecked(), "the recorded archive first");
    assert.equal(await restore.getByLabel("Restore into folder").count(), 0);
    await shoot(page, "restore.png", restore);
    await restore.getByRole("button", { name: "Restore", exact: true }).click();
    await until(() => calls(server, "project_restore").length === 1);
    assert.deepEqual(calls(server, "project_restore")[0], {
      id: "gamma",
      object: "kelpmux/gamma/20261120T093000Z.tar.gz",
    });
    // Restored, the page is the usual one again: the panel and its dialog go.
    await restore.locator(".archive-job").waitFor();
    await panel.waitFor({ state: "detached", timeout: 15000 });
    assert.ok(await page.evaluate(() => window.toasts.includes("restore done")));
    await page.locator(".project-home").getByRole("button", { name: "New terminal" }).first().waitFor();
    await page.locator('[data-project-group="gamma"][data-status="active"]').waitFor();

    // Epsilon's folder is back: a new folder, another archive.
    await group(page, "epsilon").getByRole("button", { name: /^Epsilon/ }).click();
    await page.locator('[data-archived="epsilon"]').getByRole("button", { name: "Restore…" }).click();
    const eps = page.getByRole("dialog", { name: "Restore “Epsilon”" });
    const folder = eps.getByLabel("Restore into folder");
    await folder.waitFor();
    assert.equal(await folder.inputValue(), "~/work/Epsilon-2");
    await eps.locator('[data-archive="kelpmux/epsilon/20261123T180000Z.tar.gz"] input').check();
    await eps.getByRole("button", { name: "Restore", exact: true }).click();
    await until(() => calls(server, "project_restore").length === 2);
    assert.deepEqual(calls(server, "project_restore")[1], {
      id: "epsilon",
      object: "kelpmux/epsilon/20261123T180000Z.tar.gz",
      folder: "~/work/Epsilon-2",
    });
    assert.deepEqual(errors, []);
    await context.close();

    // 6. Settings → Archive, configured: Test.
    {
      const server = fakeBridge();
      const { context, page, errors } = await open(browser, wide, server, "?view=settings");
      const list = page.locator(".archive-facts");
      await list.waitFor();
      const text = await list.innerText();
      assert.match(text, /Google Cloud Storage/);
      assert.match(text, /fogandfrog-kelpmux/);
      assert.match(text, /metadata server/);
      await page.getByRole("button", { name: "Test" }).click();
      await page.locator('[data-archive-test="ok"]').waitFor();
      assert.match(await page.locator('[data-archive-test="ok"]').innerText(), /in 412 ms/);
      await shoot(page, "settings.png");
      assert.deepEqual(errors, []);
      await context.close();
    }
    // Not configured: how to configure it.
    {
      const server = fakeBridge({ configured: false });
      const { context, page } = await open(browser, wide, server, "?view=settings");
      await page.getByText("Not configured.").waitFor();
      assert.match(await page.locator(".archive-setup pre").innerText(), /backend = "gcs"/);
      await shoot(page, "settings-unconfigured.png");
      await context.close();
    }

    // 7. A phone: the dialog fits.
    {
      const { defaultBrowserType, ...iphone } = devices["iPhone 13"];
      const server = fakeBridge();
      const { context, page } = await open(browser, iphone, server, "?phone=1");
      await page.locator(".phone-list").getByRole("button", { name: "Menu for Alpha" }).click();
      await page.getByRole("menuitem", { name: "Archive…" }).click();
      const dialog = page.getByRole("dialog", { name: "Archive “Alpha”" });
      await dialog.locator(".removal-check").waitFor();
      const overflow = await dialog.evaluate(
        (node) => node.getBoundingClientRect().right > window.innerWidth + 1,
      );
      assert.equal(overflow, false, "the dialog fits a phone's width");
      await shoot(page, "phone-archive.png");
      await context.close();
    }
    if (shots && name === "chromium") {
      const server = fakeBridge();
      const { context, page } = await open(browser, wide, server, "?mode=light&project=gamma");
      await page.locator('[data-archived="gamma"]').waitFor();
      await shoot(page, "archived-home-light.png");
      await context.close();
    }
    console.log(
      `PASS: ${name} archive: paused dimmed, Archived group folded and remembered; Pause and Resume; Archive check with warnings and the archive-only note, fingerprint, phases, progress, done moves it to Archived; Cancel; Delete blocked by blockers, Remove from KelpMux only offered, exact name typed, folder deleted and project removed; archived page, Restore of the recorded archive into its folder and of another into a new folder; Settings → Archive configured with Test and not configured; phone width`,
    );
  } finally {
    await browser.close();
  }
}
