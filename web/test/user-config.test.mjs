import { test } from "node:test";
import assert from "node:assert/strict";
import {
  configChanges,
  draftOf,
  readSaveResult,
  readUserConfig,
} from "../src/user-config.ts";

const values = {
  files: { root: "~", extra_roots: ["/mnt/data"] },
  projects: { base: "~/projects" },
  scratch: { folder: "~/scratch" },
};
const state = {
  path: "/home/u/.config/kelpmux/config.toml",
  exists: true,
  revision: "abc",
  current: false,
  values,
  resolved: values,
  errors: [
    { key: null, line: 3, column: 1, message: "unknown key `x`" },
    { message: "no position" },
    { line: 1 },
  ],
};

test("a bridge configuration is read defensively", () => {
  const config = readUserConfig(state);
  assert.equal(config.current, false);
  assert.deepEqual(config.values, values);
  assert.deepEqual(config.errors, [
    { key: null, line: 3, column: 1, message: "unknown key `x`" },
    { key: null, line: 1, column: 1, message: "no position" },
  ]);
  // A bridge before projects (API minor 4) has none.
  assert.deepEqual(config.projects, []);
  assert.equal(
    readUserConfig({ ...state, projects: [{ id: "a", name: "A", path: "/a" }] })
      .projects[0].folder,
    "/a",
  );
  assert.equal(readUserConfig(null), null);
  assert.equal(readUserConfig({ ...state, path: 1 }), null);
  assert.equal(
    readUserConfig({
      ...state,
      values: { ...values, files: { root: "~", extra_roots: [1] } },
    }),
    null,
  );
});

test("save results carry their problems and the new configuration", () => {
  const result = readSaveResult({
    saved: false,
    errors: [{ key: "projects.base", line: 2, column: 8, message: "bad" }],
    config: state,
  });
  assert.equal(result.saved, false);
  assert.equal(result.errors[0].key, "projects.base");
  assert.equal(result.config.path, state.path);
  assert.equal(readSaveResult({}), null);
});

test("only changed keys are sent", () => {
  const draft = draftOf(values);
  assert.deepEqual(configChanges(values, draft), {});
  draft.extra.push("/srv");
  draft.folder = "/srv/s";
  assert.deepEqual(configChanges(values, draft), {
    "files.extra_roots": ["/mnt/data", "/srv"],
    "scratch.folder": "/srv/s",
  });
  // The draft is a copy: editing it leaves the configuration alone.
  assert.deepEqual(values.files.extra_roots, ["/mnt/data"]);
});
