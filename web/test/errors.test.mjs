import { test } from "node:test";
import assert from "node:assert/strict";
import { reportingConfig, scrubEvent, stripQuery } from "../src/errors.ts";
import {
  crateVersion,
  releaseName,
  validRelease,
} from "../scripts/release.mjs";

test("reporting is configured only by a well-formed errors member", () => {
  assert.equal(reportingConfig(undefined), null);
  assert.equal(reportingConfig(null), null);
  assert.equal(reportingConfig({}), null);
  assert.equal(reportingConfig({ dsn: 7 }), null);
  assert.equal(reportingConfig({ dsn: "http://k@g.example.com/1" }), null);
  assert.equal(
    reportingConfig({ dsn: "https://k@g.example.com/1", environment: 3 }),
    null,
  );
  assert.deepEqual(reportingConfig({ dsn: "https://k@g.example.com/1" }), {
    dsn: "https://k@g.example.com/1",
  });
  assert.deepEqual(
    reportingConfig({ dsn: "https://k@g.example.com/1", environment: "dev" }),
    { dsn: "https://k@g.example.com/1", environment: "dev" },
  );
});

test("query strings and fragments are stripped", () => {
  assert.equal(stripQuery("https://t.example/?pane=3#x"), "https://t.example/");
  assert.equal(
    stripQuery("https://t.example/a.js#x"),
    "https://t.example/a.js",
  );
  assert.equal(stripQuery("https://t.example/a.js"), "https://t.example/a.js");
});

test("beforeSend drops user, breadcrumbs, extra and request data", () => {
  const frames = () => [
    {
      filename: "https://t.example/assets/index-1.js?v=secret",
      abs_path: "https://t.example/assets/index-1.js?v=secret#f",
      lineno: 1,
    },
  ];
  const event = scrubEvent({
    message: "boom",
    tags: { component: "web" },
    user: { id: "alice", ip_address: "{{auto}}" },
    breadcrumbs: [{ message: "ls -la" }],
    extra: { title: "vim secrets.txt" },
    request: {
      url: "https://t.example/?pane=7&token=secret#frag",
      data: "typed input",
      query_string: "pane=7",
      cookies: { session: "x" },
      headers: { Referer: "https://t.example/?a" },
    },
    exception: { values: [{ stacktrace: { frames: frames() } }] },
    threads: { values: [{ stacktrace: { frames: frames() } }] },
    stacktrace: { frames: frames() },
  });
  assert.equal(event.user, undefined);
  assert.equal(event.breadcrumbs, undefined);
  assert.equal(event.extra, undefined);
  assert.deepEqual(event.request, { url: "https://t.example/" });
  assert.deepEqual(event.tags, { component: "web" });
  for (const frame of [
    ...event.exception.values[0].stacktrace.frames,
    ...event.threads.values[0].stacktrace.frames,
    ...event.stacktrace.frames,
  ]) {
    assert.equal(frame.filename, "https://t.example/assets/index-1.js");
    assert.equal(frame.abs_path, "https://t.example/assets/index-1.js");
  }
  assert.doesNotMatch(JSON.stringify(event), /secret|ls -la|vim|typed|alice/);
  // An event without a request gets none.
  assert.equal(scrubEvent({ message: "x" }).request, undefined);
});

test("the release name matches build.rs's scheme", () => {
  const manifest =
    '[package]\nname = "kelpmux"\nversion = "1.2.3-beta.4"\nedition = "2024"\n\n[workspace]\n';
  assert.equal(crateVersion(manifest), "1.2.3-beta.4");
  const sha = "0123456789abcdef0123456789abcdef01234567";
  assert.equal(
    releaseName({ env: {}, manifest, head: () => sha }),
    "kelpmux@1.2.3-beta.4+0123456789ab",
  );
  assert.equal(
    releaseName({ env: {}, manifest, head: () => undefined }),
    "kelpmux@1.2.3-beta.4",
  );
  assert.equal(
    releaseName({
      env: { KELPMUX_GIT_SHA: sha.toUpperCase() },
      manifest,
      head: () => undefined,
    }),
    "kelpmux@1.2.3-beta.4+0123456789ab",
  );
  assert.equal(
    releaseName({ env: { KELPMUX_RELEASE: "kelpmux@9+x" }, manifest }),
    "kelpmux@9+x",
  );
  assert.throws(() =>
    releaseName({ env: { KELPMUX_RELEASE: "a/b" }, manifest }),
  );
  assert.throws(() =>
    releaseName({ env: { KELPMUX_GIT_SHA: "nothex" }, manifest }),
  );
  assert.ok(validRelease("kelpmux@0.2.0-beta.7+abcdef012345"));
  assert.ok(!validRelease(".."));
  // The real manifest parses.
  assert.match(releaseName({ env: {} }), /^kelpmux@\d+\.\d+\.\d+/);
});
