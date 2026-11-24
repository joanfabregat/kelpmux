import { test } from "node:test";
import assert from "node:assert/strict";
import { FileError } from "@kelpcore/web/files/api.ts";
import {
  canGoUp,
  destinationRootProblem,
  entryAbilities,
  errorMessage,
  friendlyError,
  leadsOutside,
  linkTarget,
  outsideRootMessage,
  rootCrumbs,
  withinRoot,
} from "../src/files/root.ts";

const outside = "Outside the folders this server lets you browse";
const entry = (kind, target) => ({
  name: "x",
  kind,
  size: 0,
  modified: 0,
  ...(target ? { target } : {}),
});

test("paths inside the file root", () => {
  assert.equal(outsideRootMessage, outside);
  // No root (an older server): everything.
  for (const path of ["/", "/etc", "relative", "~"])
    assert.equal(withinRoot(path, undefined), true, path);
  const root = "/home/me";
  for (const path of ["/home/me", "/home/me/a", "/home/me/a/../b", "/home/me/"])
    assert.equal(withinRoot(path, root), true, path);
  for (const path of [
    "/",
    "/etc/passwd",
    "/home/meow",
    "/home",
    "/home/me/../x",
    "a/b",
    "",
  ])
    assert.equal(withinRoot(path, root), false, path);
  // `~` inside the home folder; unknown home: the server decides.
  assert.equal(withinRoot("~", root, "/home/me"), true);
  assert.equal(withinRoot("~/src", root, "/home/me"), true);
  assert.equal(withinRoot("~/../x", root, "/home/me"), false);
  assert.equal(withinRoot("~", "/work", "/home/me"), false);
  assert.equal(withinRoot("~", root), true);
  assert.equal(withinRoot("~other", root, "/home/me"), false);
  // A root of `/` holds every absolute path.
  assert.equal(withinRoot("/etc", "/"), true);
});

test("breadcrumbs start at the root", () => {
  const labels = (crumbs) => crumbs.map((c) => `${c.label}=${c.path}`);
  assert.deepEqual(
    labels(rootCrumbs("/home/me/src/app", "/home/me", "/home/me")),
    ["~=/home/me", "src=/home/me/src", "app=/home/me/src/app"],
  );
  assert.deepEqual(labels(rootCrumbs("/home/me", "/home/me", "/home/me")), [
    "~=/home/me",
  ]);
  // Another root (home unknown or elsewhere): its path, nothing above.
  assert.deepEqual(labels(rootCrumbs("/srv/work/a", "/srv/work", "/home/me")), [
    "/srv/work=/srv/work",
    "a=/srv/work/a",
  ]);
  assert.deepEqual(labels(rootCrumbs("/home/me/a", "/home/me")), [
    "/home/me=/home/me",
    "a=/home/me/a",
  ]);
  // No root, or `/`: from `/` or `~` as before.
  assert.deepEqual(labels(rootCrumbs("/etc/ssh", undefined)), [
    "/=/",
    "etc=/etc",
    "ssh=/etc/ssh",
  ]);
  assert.deepEqual(labels(rootCrumbs("/home/me/a", "/", "/home/me")), [
    "~=/home/me",
    "a=/home/me/a",
  ]);
});

test("the parent folder stops at the root", () => {
  assert.equal(canGoUp("/home/me", "/home/me"), false);
  assert.equal(canGoUp("/home/me/a", "/home/me"), true);
  assert.equal(canGoUp("/home/me", undefined), true);
  assert.equal(canGoUp("/", undefined), false);
  assert.equal(canGoUp("~", undefined), false);
});

test("symbolic links pointing outside the root", () => {
  const root = "/home/me";
  assert.equal(linkTarget(entry("symlink", "/etc"), "/home/me/l"), "/etc");
  assert.equal(
    linkTarget(entry("symlink", "../x"), "/home/me/a/l"),
    "/home/me/x",
  );
  assert.equal(
    linkTarget(entry("symlink", "./b/../c"), "/home/me/l"),
    "/home/me/c",
  );
  assert.equal(linkTarget(entry("symlink"), "/home/me/l"), null);
  assert.equal(linkTarget(entry("file"), "/home/me/l"), null);
  assert.equal(
    leadsOutside(entry("symlink", "/etc"), "/home/me/l", root),
    true,
  );
  assert.equal(
    leadsOutside(entry("symlink", "../../tmp"), "/home/me/a/l", root),
    true,
  );
  assert.equal(
    leadsOutside(entry("symlink", "../me2"), "/home/me/l", root),
    true,
  );
  assert.equal(
    leadsOutside(entry("symlink", "docs"), "/home/me/l", root),
    false,
  );
  assert.equal(
    leadsOutside(entry("symlink", "/home/me/docs"), "/home/me/l", root),
    false,
  );
  assert.equal(
    leadsOutside(entry("symlink", "/etc"), "/home/me/l", undefined),
    false,
  );
  assert.equal(leadsOutside(entry("dir"), "/home/me/l", root), false);
});

test("what can be done with an entry", () => {
  const root = "/home/me";
  // The root: open only (no download, rename, move, trash or delete).
  assert.deepEqual(entryAbilities(entry("dir"), root, root), {
    open: true,
    download: false,
    change: false,
  });
  // A link pointing outside: the link itself only.
  assert.deepEqual(
    entryAbilities(entry("symlink", "/etc"), "/home/me/l", root),
    {
      open: false,
      download: false,
      change: true,
    },
  );
  assert.deepEqual(entryAbilities(entry("file"), "/home/me/f", root), {
    open: true,
    download: true,
    change: true,
  });
  assert.equal(
    entryAbilities(entry("other"), "/home/me/p", root).download,
    false,
  );
  // No root: as before.
  assert.deepEqual(entryAbilities(entry("dir"), "/", undefined), {
    open: true,
    download: true,
    change: true,
  });
});

test("upload destinations inside the root", () => {
  assert.equal(destinationRootProblem("", "/home/me"), null);
  assert.equal(destinationRootProblem("/home/me/in", "/home/me"), null);
  assert.equal(destinationRootProblem("/tmp", "/home/me"), outside);
  assert.equal(destinationRootProblem("/tmp", undefined), null);
  assert.match(destinationRootProblem("rel", "/home/me"), /absolute/);
});

test("outside_root errors in words", () => {
  const server = new FileError({
    code: "outside_root",
    message: "the path is outside the file root",
    path: "/etc/passwd",
  });
  const friendly = friendlyError(server);
  assert.ok(friendly instanceof FileError);
  assert.equal(friendly.code, "outside_root");
  assert.equal(friendly.path, "/etc/passwd");
  assert.equal(friendly.message, outside);
  assert.equal(errorMessage(server), outside);
  const other = new FileError({ code: "not_found", message: "gone" });
  assert.equal(friendlyError(other), other);
  assert.equal(errorMessage(other), "gone");
  assert.equal(errorMessage(new Error("plain")), "plain");
  assert.equal(errorMessage("text"), "text");
});
