import { test } from "node:test";
import assert from "node:assert/strict";
import {
  columnFolders,
  entryDetail,
  entryMeta,
  formatModified,
  formatModifiedFull,
  formatSize,
  matchPrefix,
} from "../src/files/columns.ts";

/** Unix seconds of a local date and time. */
const at = (...parts) => new Date(...parts).getTime() / 1000;
const now = new Date(2026, 10, 23, 15, 0);
/** ICU writes a narrow no-break space before AM/PM. */
const plain = (text) => text.replace(/\s/g, " ");

test("sizes in decimal units, like the Finder", () => {
  for (const [bytes, text] of [
    [0, "0 bytes"],
    [1, "1 byte"],
    [999, "999 bytes"],
    [1000, "1 KB"],
    [4200, "4.2 KB"],
    [9949, "9.9 KB"],
    [9950, "10 KB"],
    [123_456, "123 KB"],
    [999_499, "999 KB"],
    [999_500, "1 MB"],
    [13_000_000, "13 MB"],
    [1_500_000_000, "1.5 GB"],
    [2e15, "2 PB"],
    [5e18, "5000 PB"],
  ])
    assert.equal(formatSize(bytes), text, String(bytes));
  assert.equal(formatSize(-1), "");
  assert.equal(formatSize(Number.NaN), "");
});

test("modification dates: today and yesterday with the time, else the date", () => {
  assert.equal(
    formatModified(at(2026, 10, 23, 14, 32), now, "en-GB"),
    "Today 14:32",
  );
  assert.equal(
    formatModified(at(2026, 10, 23, 0, 0), now, "en-GB"),
    "Today 00:00",
  );
  assert.equal(
    formatModified(at(2026, 10, 22, 9, 10), now, "en-GB"),
    "Yesterday 09:10",
  );
  assert.equal(
    plain(formatModified(at(2026, 10, 22, 9, 10), now, "en-US")),
    "Yesterday 9:10 AM",
  );
  assert.equal(
    formatModified(at(2026, 10, 3, 9, 10), now, "en-US"),
    "Nov 3, 2026",
  );
  assert.equal(
    formatModified(at(2025, 0, 31, 9, 10), now, "en-GB"),
    "31 Jan 2025",
  );
  // Across a month boundary, and a date in the future (a skewed clock).
  assert.equal(
    formatModified(
      at(2026, 11, 31, 23, 0),
      new Date(2027, 0, 1, 8, 0),
      "en-GB",
    ),
    "Yesterday 23:00",
  );
  assert.equal(
    formatModified(at(2026, 10, 25, 9, 0), now, "en-US"),
    "Nov 25, 2026",
  );
  // Unknown times are left out.
  assert.equal(formatModified(0, now), "");
  assert.equal(formatModified(Number.NaN, now), "");
  assert.equal(
    plain(formatModifiedFull(at(2026, 10, 3, 9, 10), "en-US")),
    "November 3, 2026 at 9:10 AM",
  );
  assert.equal(formatModifiedFull(0), "");
});

test("the second line: date, then size or kind", () => {
  const entry = (kind, extra = {}) => ({
    name: "x",
    kind,
    size: 4200,
    modified: at(2026, 10, 23, 14, 32),
    mode: 0o644,
    ...extra,
  });
  assert.equal(entryDetail(entry("file")), "4.2 KB");
  assert.equal(entryDetail(entry("dir")), "Folder");
  assert.equal(
    entryDetail(entry("symlink", { target: "../docs" })),
    "→ ../docs",
  );
  assert.equal(entryDetail(entry("symlink")), "Link");
  assert.equal(entryDetail(entry("other")), "Special file");
  assert.equal(entryMeta(entry("file"), now, "en-GB"), "Today 14:32 · 4.2 KB");
  assert.equal(entryMeta(entry("dir"), now, "en-GB"), "Today 14:32 · Folder");
  assert.equal(entryMeta(entry("file", { modified: 0 }), now), "4.2 KB");
});

test("the columns: the root down to the folder", () => {
  assert.deepEqual(columnFolders("/home/u/a/b", "/home/u", "/home/u"), [
    "/home/u",
    "/home/u/a",
    "/home/u/a/b",
  ]);
  assert.deepEqual(columnFolders("/home/u", "/home/u", "/home/u"), ["/home/u"]);
  // Another root.
  assert.deepEqual(columnFolders("/srv/x/y", "/srv", "/home/u"), [
    "/srv",
    "/srv/x",
    "/srv/x/y",
  ]);
  // Not yet normalised: one column until the server names it.
  assert.deepEqual(columnFolders("~", "/home/u", "/home/u"), ["~"]);
  // No root (an older server): from the home folder, or from /.
  assert.deepEqual(columnFolders("/home/u/a", undefined, "/home/u"), [
    "/home/u",
    "/home/u/a",
  ]);
  assert.deepEqual(columnFolders("/etc/ssh", undefined, "/home/u"), [
    "/",
    "/etc",
    "/etc/ssh",
  ]);
});

test("type-to-select: the first name with the prefix, wrapping", () => {
  const names = ["docs", "Downloads", "notes.txt", "dotfiles"];
  assert.equal(matchPrefix(names, "d"), 0);
  assert.equal(matchPrefix(names, "d", 1), 1);
  assert.equal(matchPrefix(names, "D", 2), 3);
  assert.equal(matchPrefix(names, "d", 4), 0);
  assert.equal(matchPrefix(names, "dow"), 1);
  assert.equal(matchPrefix(names, "NOTES"), 2);
  assert.equal(matchPrefix(names, "z"), -1);
  assert.equal(matchPrefix(names, ""), -1);
  assert.equal(matchPrefix([], "a"), -1);
});
