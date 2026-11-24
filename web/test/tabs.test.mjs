import { test } from "node:test";
import assert from "node:assert/strict";
import { neighbourTab, tabLabels } from "../src/files/tabs.ts";

test("tab labels name the file, with folders only for same names", () => {
  const labels = tabLabels([
    "/home/me/notes.md",
    "/home/me/a/src/main.rs",
    "/home/me/b/src/main.rs",
    "/home/me/web/index.ts",
    "/home/me/api/index.ts",
  ]);
  assert.deepEqual(labels.get("/home/me/notes.md"), {
    name: "notes.md",
    hint: "",
  });
  assert.deepEqual(labels.get("/home/me/a/src/main.rs"), {
    name: "main.rs",
    hint: "a/src",
  });
  assert.deepEqual(labels.get("/home/me/b/src/main.rs"), {
    name: "main.rs",
    hint: "b/src",
  });
  assert.equal(labels.get("/home/me/web/index.ts").hint, "web");
  assert.equal(labels.get("/home/me/api/index.ts").hint, "api");
});

test("a file at the root has / as its folder", () => {
  const labels = tabLabels(["/x.txt", "/home/x.txt"]);
  assert.equal(labels.get("/x.txt").hint, "/");
  assert.equal(labels.get("/home/x.txt").hint, "home");
});

test("closing the shown tab shows the next, else the previous, else none", () => {
  const tabs = ["/a", "/b", "/c"];
  assert.equal(neighbourTab(tabs, "/b"), "/c");
  assert.equal(neighbourTab(tabs, "/c"), "/b");
  assert.equal(neighbourTab(["/a"], "/a"), "");
  assert.equal(neighbourTab(tabs, "/z"), "");
});
