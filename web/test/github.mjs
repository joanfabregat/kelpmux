// GitHub in the web client (test/github.fixture.tsx), in Chromium and
// WebKit, against a fake bridge kept here; nothing reaches GitHub:
// 1. Settings → GitHub, signed out: says so and shows how to configure a
//    token source. Signed in: the source, the login, the token's scopes,
//    the rate limit and the SSH agent; Test asks the bridge again.
// 2. The Repos view shows GitHub's facts on a checkout whose origin is on
//    github.com (visibility, default branch, open pull requests).
// 3. Add repository lists the repositories the token sees, searchable;
//    one already in the project cannot be cloned, a large one asks first,
//    a huge one is refused. Clone sends the SSH URL by default, HTTPS when
//    chosen, or a typed clone URL; the clone shows its progress, can be
//    cancelled, and once done the new checkout appears in the Repos view.
// 4. Signed out, the picker says so, still clones a typed URL, and opens
//    Settings → GitHub.
// 5. A phone: the picker fits its width.
// Set GITHUB_SCREENSHOTS to a directory to save screenshots.
import { build } from "vite";
import { chromium, webkit, devices } from "playwright";
import { readFileSync, mkdirSync } from "node:fs";
import assert from "node:assert/strict";

const out = process.env.GITHUB_FIXTURE_OUTPUT || "/tmp/kelpmux-github-fixture";
const shots = process.env.GITHUB_SCREENSHOTS;
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
      entry: new URL("./github.fixture.tsx", import.meta.url).pathname,
      name: "GitHubFixture",
      formats: ["iife"],
      cssFileName: "github-fixture",
      fileName: () => "fixture.js",
    },
    minify: false,
  },
});
const origin = "https://terminal.test";
const now = Math.floor(Date.now() / 1000);

const checkout = (name, github) => ({
  path: `/home/me/work/alpha/${name}`,
  name,
  branch: "main",
  head: "4f2a9c1e8b7d6a5f4e3d2c1b0a9f8e7d6c5b4a39",
  detached: false,
  upstream: "origin/main",
  ahead: 0,
  behind: 0,
  files: 0,
  staged: 0,
  unstaged: 0,
  untracked: 0,
  conflicted: 0,
  added: 0,
  removed: 0,
  uncounted: 0,
  last: {
    sha: "4f2a9c1e8b7d6a5f4e3d2c1b0a9f8e7d6c5b4a39",
    subject: "Initial commit",
    time: now - 3600,
  },
  truncated: false,
  error: null,
  github,
});

const repository = (owner, name, fields = {}) => ({
  id: Math.abs([...`${owner}/${name}`].reduce((h, c) => (h * 31 + c.charCodeAt(0)) | 0, 7)),
  name,
  full_name: `${owner}/${name}`,
  ssh_url: `git@github.com:${owner}/${name}.git`,
  https_url: `https://github.com/${owner}/${name}.git`,
  private: false,
  archived: false,
  fork: false,
  default_branch: "main",
  description: "",
  size_kib: 2048,
  pushed_at: "2026-11-20T10:00:00Z",
  ...fields,
});

/** The fake bridge: project Alpha with one checkout (`api`, from
 * octocat/api); signed in (or not) with a token command; clones run until
 * polled three times, then appear in Alpha. Every call recorded. */
function fakeBridge({ signedIn = true, enabled = true } = {}) {
  const state = { calls: [], clones: [], checkouts: [checkout("api", "octocat/api")] };
  let next = 0;
  let checks = 0;
  const repos = [
    repository("octocat", "api", { private: true, description: "The API" }),
    repository("octocat", "hello", { description: "My first repository" }),
    repository("octocat", "Hello-World", { fork: true }),
    repository("acme", "monorepo", { private: true, size_kib: 2 * 1024 * 1024, description: "Everything" }),
    repository("acme", "datasets", { size_kib: 8 * 1024 * 1024, description: "Too big" }),
    repository("acme", "website", { archived: true }),
  ];
  const status = () => {
    for (const clone of state.clones)
      if (clone.state === "running" && ++clone.polls >= 3 && !clone.hold) {
        clone.state = "done";
        clone.progress = "";
        state.checkouts.push(checkout(clone.name, clone.full_name.includes("/") ? clone.full_name : null));
      }
    const busy = enabled && signedIn && checks > 0 && checks-- > 0;
    const known = enabled && signedIn && !busy;
    return {
      enabled,
      source: enabled ? "command" : null,
      source_label: enabled ? "/home/me/bin/github-token" : null,
      signed_in: known,
      login: known ? "octocat" : null,
      name: known ? "The Octocat" : null,
      scopes: known ? ["repo", "read:org", "workflow"] : null,
      rate: known ? { limit: 5000, remaining: 4987, reset: now + 1800 } : null,
      checked_at: known ? now : null,
      ssh_agent: { configured: true, available: true },
      busy,
      error: enabled && !signedIn ? "`/home/me/bin/github-token` failed (exit status 1)" : null,
      repositories_ready: known,
      repositories_truncated: false,
      clones: state.clones.map(({ polls, hold, name, ...clone }) => clone),
    };
  };
  return {
    state,
    call(method, params) {
      state.calls.push([method, params]);
      switch (method) {
        case "git":
          return {
            folder: "/home/me/work/alpha",
            exists: true,
            truncated: false,
            sampled_at: now,
            repos: state.checkouts,
          };
        case "github_status":
          return status();
        case "github_test":
          checks = 2;
          return status();
        case "github_repositories": {
          if (!enabled) throw Error("GitHub is not signed in: set `github.auth` or `github.token_command` in config.toml");
          const s = status();
          return { ...s, repositories: s.signed_in ? repos : null };
        }
        case "github_state": {
          if (!enabled || !signedIn) throw Error("GitHub is not signed in");
          const facts = {};
          for (const slug of params.repos)
            facts[slug] =
              slug === "octocat/api"
                ? { visible: true, full_name: slug, visibility: "private", default_branch: "main", archived: false, pushed_at: "2026-11-23T18:00:00Z", open_pulls: 3 }
                : { visible: true, full_name: slug, visibility: "public", default_branch: "main", archived: false, pushed_at: "2026-11-20T10:00:00Z", open_pulls: 0 };
          return { repos: facts, pending: false, limited: false };
        }
        case "github_clone": {
          const name = params.url.replace(/\/+$/, "").split(/[/:]/).pop().replace(/\.git$/, "");
          if (state.checkouts.some((c) => c.name === name)) throw Error(`/home/me/work/alpha/${name} already exists`);
          const slug = /github\.com[:/]([^/]+\/[^/]+?)(\.git)?$/.exec(params.url)?.[1];
          state.clones.push({
            id: ++next,
            project: params.project,
            full_name: slug || name,
            url: params.url,
            path: `/home/me/work/alpha/${name}`,
            state: "running",
            progress: "Receiving objects:  42% (420/1000), 1.2 MiB | 2.4 MiB/s",
            error: null,
            polls: 0,
            hold: name === "monorepo",
            name,
          });
          return status();
        }
        case "github_clone_cancel": {
          const clone = state.clones.find((c) => c.id === params.clone);
          if (!clone) throw Error("this clone is not known");
          clone.state = "cancelled";
          clone.error = "cancelled";
          return status();
        }
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
        body: '<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover"><link rel="stylesheet" href="/github-fixture.css"></head><body><div id="root"></div><script src="/fixture.js"></script></body></html>',
      });
    if (path === "/github-fixture.css")
      return route.fulfill({
        contentType: "text/css; charset=utf-8",
        body: readFileSync(`${out}/github-fixture.css`, "utf8"),
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
const calls = (server, method) =>
  server.state.calls.filter(([m]) => m === method).map(([, p]) => p);
async function until(condition) {
  for (let i = 0; i < 100 && !condition(); i++)
    await new Promise((resolve) => setTimeout(resolve, 50));
  assert.ok(condition(), "condition not met in time");
}
const repoRow = (page, name) => page.locator(`.git-repo[data-repo="${name}"]`);
const pickerRow = (page, name) =>
  page.locator(`.github-repositories li[data-repository="${name}"]`);
const wide = { viewport: { width: 1280, height: 800 } };

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
    // 1. Settings → GitHub, signed out (no token source).
    {
      const server = fakeBridge({ enabled: false });
      const { context, page, errors } = await open(browser, wide, server, "?view=settings");
      await page.getByText("Not signed in.", { exact: true }).waitFor();
      assert.match(await page.locator(".github-setup pre").innerText(), /token_command = "~\/bin\/github-token"/);
      await shoot(page, "settings-signed-out.png");
      assert.deepEqual(errors, []);
      await context.close();
    }
    // Signed in: source, login, scopes, rate limit, SSH agent; Test.
    {
      const server = fakeBridge();
      const { context, page, errors } = await open(browser, wide, server, "?view=settings");
      const facts = page.locator(".github-facts-list");
      await facts.getByText("octocat", { exact: true }).waitFor();
      const text = await facts.innerText();
      assert.match(text, /\/home\/me\/bin\/github-token/);
      assert.match(text, /The Octocat/);
      assert.match(text, /repo\s*read:org\s*workflow/);
      assert.match(text, /4,987 of 5,000 requests left/);
      assert.match(text, /github\.ssh_auth_sock/);
      await shoot(page, "settings.png");
      await page.getByRole("button", { name: "Test" }).click();
      await until(() => calls(server, "github_test").length === 1);
      await page.getByText("Checking…", { exact: true }).waitFor();
      await facts.getByText("octocat", { exact: true }).waitFor();
      assert.ok(!(await page.content()).includes("ghp_"));
      assert.deepEqual(errors, []);
      await context.close();
    }
    // A token source that fails says why.
    {
      const server = fakeBridge({ signedIn: false });
      const { context, page } = await open(browser, wide, server, "?view=settings");
      await page.getByRole("alert").getByText("exit status 1").waitFor();
      assert.match(await page.locator(".github-facts-list").innerText(), /Not signed in/);
      await context.close();
    }

    // 2. The Repos view: GitHub's facts on api.
    const server = fakeBridge();
    const { context, page, errors } = await open(browser, wide, server);
    const api = repoRow(page, "api");
    await api.locator('.github-facts[data-github="octocat/api"]').waitFor();
    const factsText = await api.locator(".github-facts").innerText();
    assert.match(factsText, /octocat\/api/);
    assert.match(factsText, /private/);
    assert.match(factsText, /main/);
    assert.match(factsText, /3/);
    assert.deepEqual(calls(server, "github_state")[0], { repos: ["octocat/api"] });
    assert.equal(
      await api.getByRole("link", { name: "Open octocat/api on GitHub" }).getAttribute("href"),
      "https://github.com/octocat/api",
    );
    await shoot(page, "repos-facts.png");

    // 3. Add repository: the list, searchable.
    await page.getByRole("button", { name: "Add repository" }).click();
    const picker = page.getByRole("dialog");
    await pickerRow(page, "octocat/hello").waitFor();
    assert.equal(await page.locator(".github-repositories li[data-repository]").count(), 6);
    assert.equal(await pickerRow(page, "octocat/api").getByRole("button").innerText(), "In project");
    assert.ok(await pickerRow(page, "acme/datasets").getByRole("button").isDisabled());
    assert.match(await pickerRow(page, "acme/website").innerText(), /Archived/);
    await shoot(page, "picker.png", picker);
    await page.getByLabel("Find a repository").fill("hello");
    assert.deepEqual(
      await page.$$eval(".github-repositories li[data-repository]", (rows) => rows.map((r) => r.dataset.repository)),
      ["octocat/hello", "octocat/Hello-World"],
    );
    // Clone over SSH (the default): the picker closes, the clone shows,
    // then the checkout appears.
    await pickerRow(page, "octocat/hello").getByRole("button", { name: "Clone octocat/hello" }).click();
    await until(() => calls(server, "github_clone").length === 1);
    assert.deepEqual(calls(server, "github_clone")[0], {
      project: "alpha",
      url: "git@github.com:octocat/hello.git",
    });
    await picker.waitFor({ state: "detached" });
    const progress = page.locator('.repos-clone[data-clone="octocat/hello"]');
    await progress.waitFor();
    assert.match(await progress.innerText(), /Receiving objects/);
    await shoot(page, "clone-progress.png");
    await repoRow(page, "hello").waitFor({ timeout: 10000 });
    await progress.waitFor({ state: "detached" });

    // HTTPS when chosen; a large repository asks first; Cancel stops it.
    await page.getByRole("button", { name: "Add repository" }).click();
    await pickerRow(page, "acme/monorepo").waitFor();
    assert.equal(await pickerRow(page, "octocat/hello").getByRole("button").innerText(), "In project");
    await page.getByLabel("HTTPS").check();
    await pickerRow(page, "acme/monorepo").getByRole("button").click();
    await page.getByRole("alertdialog", { name: "Large repository" }).waitFor();
    await shoot(page, "picker-large.png", page.getByRole("dialog"));
    await page.getByRole("button", { name: "Clone anyway" }).click();
    await until(() => calls(server, "github_clone").length === 2);
    assert.equal(calls(server, "github_clone")[1].url, "https://github.com/acme/monorepo.git");
    const monorepo = page.locator('.repos-clone[data-clone="acme/monorepo"]');
    await monorepo.waitFor();
    await monorepo.getByRole("button", { name: "Cancel" }).click();
    await until(() => calls(server, "github_clone_cancel").length === 1);
    assert.deepEqual(calls(server, "github_clone_cancel")[0], { clone: 2 });
    await monorepo.waitFor({ state: "detached" });

    // A typed clone URL.
    await page.getByRole("button", { name: "Add repository" }).click();
    const url = page.getByLabel("Clone URL");
    await url.fill("https://git.example.com/team/api.git");
    assert.ok(await page.locator(".github-url button[type=submit]").isDisabled(), "api exists");
    await url.fill("https://git.example.com/team/tool.git");
    await url.press("Enter");
    await until(() => calls(server, "github_clone").length === 3);
    assert.equal(calls(server, "github_clone")[2].url, "https://git.example.com/team/tool.git");
    await repoRow(page, "tool").waitFor({ timeout: 10000 });
    assert.deepEqual(errors, []);
    await context.close();

    // 4. Signed out: a URL still clones; Settings opens from the picker.
    {
      const server = fakeBridge({ enabled: false });
      const { context, page, errors } = await open(browser, wide, server);
      await repoRow(page, "api").waitFor();
      assert.equal(await page.locator(".github-facts").count(), 0);
      await page.getByRole("button", { name: "Add repository" }).click();
      await page.getByText("not signed in to GitHub").waitFor();
      await shoot(page, "picker-signed-out.png", page.getByRole("dialog"));
      await page.getByLabel("Clone URL").fill("git@github.com:octocat/hello.git");
      await page.getByRole("button", { name: "Clone", exact: true }).click();
      await until(() => calls(server, "github_clone").length === 1);
      await page.getByRole("button", { name: "Add repository" }).click();
      await page.getByRole("button", { name: "Open Settings → GitHub" }).click();
      assert.deepEqual(await page.evaluate(() => window.settings), ["github"]);
      assert.deepEqual(errors, []);
      await context.close();
    }

    // 5. A phone: the picker fits.
    {
      const { defaultBrowserType, ...iphone } = devices["iPhone 13"];
      const server = fakeBridge();
      const { context, page } = await open(browser, iphone, server, "?phone=1");
      await repoRow(page, "api").waitFor();
      await page.getByRole("button", { name: "Add repository" }).click();
      await pickerRow(page, "octocat/hello").waitFor();
      const overflow = await page.evaluate(
        () => document.querySelector(".github-picker").getBoundingClientRect().right > window.innerWidth + 1,
      );
      assert.equal(overflow, false, "the picker fits a phone's width");
      await shoot(page, "phone-picker.png");
      await context.close();
    }
    if (shots && name === "chromium") {
      const server = fakeBridge();
      const { context, page } = await open(browser, wide, server, "?mode=light&view=settings");
      await page.locator(".github-facts-list").getByText("octocat", { exact: true }).waitFor();
      await shoot(page, "settings-light.png");
      await context.close();
    }
    console.log(
      `PASS: ${name} github: settings signed out, signed in (source, login, scopes, rate limit, SSH agent) and failing, Test; Repos facts; picker list, search, in-project, large and huge repositories, SSH and HTTPS clones, progress, cancel, typed URL, new checkouts appear; signed-out picker clones a URL and opens Settings; phone width`,
    );
  } finally {
    await browser.close();
  }
}
