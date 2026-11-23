import { test } from "node:test";
import assert from "node:assert/strict";
import {
  fileUrlPath,
  normalizePath,
  parseLocation,
  pathLinkAt,
  resolvePath,
  tokenAt,
} from "../src/files/path-links.ts";

/** The token around the first occurrence of `at` (its first character). */
const token = (line, at, offset = 0) =>
  tokenAt(line, line.indexOf(at) + offset)?.text ?? null;

test("tokenAt extracts the token under the pointer", () => {
  const cases = [
    // [line, clicked text, expected token]
    ["src/main.rs", "main", "src/main.rs"],
    ["error at src/x.rs:12:3: bad", "x.rs", "src/x.rs:12:3"],
    ["see file.rs:12:3.", "file", "file.rs:12:3"],
    ["(see src/x.py)", "x.py", "src/x.py"],
    ["[src/x.py]", "src", "src/x.py"],
    ["{src/x.py}", "src", "src/x.py"],
    ["<src/x.py>", "src", "src/x.py"],
    ["list: a.txt, b.txt; c.txt", "a.txt", "a.txt"],
    ["list: a.txt, b.txt; c.txt", "b.txt", "b.txt"],
    ["list: a.txt, b.txt; c.txt", "c.txt", "c.txt"],
    ['File "x.py", line 3', "x.py", "x.py"],
    ["open 'my notes.txt' now", "notes", "my notes.txt"],
    ['open "my notes.txt" now', "my", "my notes.txt"],
    ["run `src/x.py:3` again", "x.py", "src/x.py:3"],
    ["don't open src/x.py", "src", "src/x.py"],
    ["it's `a b.txt`", "b.txt", "a b.txt"],
    ["a/b", "b", "a/b"],
    ["a / b", "a", "a"],
    ["a / b", "b", "b"],
    ["./rel/x", "rel", "./rel/x"],
    ["../up/x", "up", "../up/x"],
    ["~/x.md", "x.md", "~/x.md"],
    ["/etc/hosts:", "hosts", "/etc/hosts"],
    ["https://example.com/a.", "example", "https://example.com/a"],
  ];
  for (const [line, at, expected] of cases)
    assert.equal(token(line, at), expected, `${line} @ ${at}`);
});

test("tokenAt is null on spaces and stripped punctuation", () => {
  assert.equal(tokenAt("a  b", 1), null);
  assert.equal(tokenAt("a  b", 9), null);
  assert.equal(tokenAt("a  b", -1), null);
  assert.equal(token("(src/x.py)", "("), null);
  assert.equal(tokenAt("src/x.py.", 8), null);
  assert.equal(token("open 'x' now", "'"), null);
  assert.equal(tokenAt("", 0), null);
});

test("tokenAt reports the span to underline", () => {
  assert.deepEqual(tokenAt("see (src/x.py:3).", 8), {
    start: 5,
    end: 15,
    text: "src/x.py:3",
  });
});

test("parseLocation splits :line and :line:col", () => {
  const cases = [
    ["file.rs", { path: "file.rs" }],
    ["file.rs:12", { path: "file.rs", line: 12 }],
    ["file.rs:12:3", { path: "file.rs", line: 12, col: 3 }],
    ["src/a b.py:7", { path: "src/a b.py", line: 7 }],
    ["file.rs:0", { path: "file.rs" }],
    ["file.rs:0:4", { path: "file.rs" }],
    ["file.rs:12:0", { path: "file.rs", line: 12 }],
    ["file.rs:x", { path: "file.rs:x" }],
    ["C:12", { path: "C", line: 12 }],
    ["~/x", { path: "~/x" }],
    ["http://example.com/x", null],
    ["https://example.com/x:3", null],
    ["file:///etc/hosts", null],
    ["/", null],
    [".", null],
    ["..", null],
    ["~", null],
    ["./", null],
    [":12", { path: ":12" }],
    ["a\u0007b", null],
  ];
  for (const [input, expected] of cases)
    assert.deepEqual(parseLocation(input), expected, input);
});

test("resolvePath resolves against the terminal's folder and home", () => {
  const cwd = "/home/alice/project",
    home = "/home/alice";
  const cases = [
    ["src/x.py", "/home/alice/project/src/x.py"],
    ["./src/x.py", "/home/alice/project/src/x.py"],
    ["../other/y", "/home/alice/other/y"],
    ["../../../../../etc/hosts", "/etc/hosts"],
    ["~/notes.txt", "/home/alice/notes.txt"],
    ["~", "/home/alice"],
    ["~/../bob/x", "/home/bob/x"],
    ["/etc//hosts", "/etc/hosts"],
    ["/a/./b/../c", "/a/c"],
    ["~bob/x", null],
  ];
  for (const [input, expected] of cases)
    assert.equal(resolvePath(input, cwd, home), expected, input);
  assert.equal(resolvePath("src/x", "", home), null);
  assert.equal(resolvePath("src/x", "relative", home), null);
  assert.equal(resolvePath("~/x", cwd, undefined), null);
  assert.equal(resolvePath("/abs", undefined, undefined), "/abs");
  assert.equal(normalizePath("/"), "/");
  assert.equal(normalizePath("/.."), "/");
});

test("pathLinkAt combines the token, location and resolution", () => {
  const line = "  --> src/main.rs:12:5";
  assert.deepEqual(pathLinkAt(line, 9, "/w", "/h"), {
    start: 6,
    end: 22,
    text: "src/main.rs:12:5",
    path: "/w/src/main.rs",
    line: 12,
    col: 5,
  });
  assert.equal(pathLinkAt("see https://x.org/a", 6, "/w", "/h"), null);
  assert.equal(pathLinkAt("a / b", 2, "/w", "/h"), null);
  assert.equal(pathLinkAt("x.py", 0, undefined, "/h"), null);
});

test("fileUrlPath reads file:// hyperlinks", () => {
  assert.equal(fileUrlPath("file:///home/a/x%20y.txt"), "/home/a/x y.txt");
  assert.equal(fileUrlPath("file://host/etc/hosts"), "/etc/hosts");
  assert.equal(fileUrlPath("https://example.com/x"), null);
});
