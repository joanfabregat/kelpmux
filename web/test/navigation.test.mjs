import { test } from "node:test";
import assert from "node:assert/strict";
import {
  HISTORY_VERSION,
  dialogFromParam,
  dialogParam,
  emptyLocation,
  hasLocation,
  historyMode,
  historyState,
  locationOf,
  locationSearch,
  parseLocation,
  readHistoryState,
  resolveLocation,
} from "../src/navigation.ts";

const at = (fields) => ({ ...emptyLocation, ...fields });
const terminal = (id, more = {}) =>
  at({ view: "terminal", terminal: id, ...more });
const grid = at({ view: "grid" });

test("every place round-trips through its URL", () => {
  const places = [
    terminal(12),
    terminal(12, { side: "/home/me/a b.txt", line: 40 }),
    terminal(0),
    grid,
    at({ view: "files", dir: "/home/me/src" }),
    at({ view: "files", dir: "~" }),
    at({ view: "files", file: "/home/me/notes.md" }),
    at({ view: "files", file: "/srv/x&y=1?.rs", line: 7 }),
    at({ view: "home" }),
    at({ view: "grid", dialog: "settings" }),
    terminal(3, { dialog: "notifications", of: 9 }),
    at({ view: "home", dialog: "terminals" }),
  ];
  for (const place of places) {
    const search = locationSearch(place);
    assert.deepEqual(
      resolveLocation(parseLocation(search), grid),
      place,
      search,
    );
  }
});

test("URLs stay readable and keep parameters they do not own", () => {
  assert.equal(
    locationSearch(at({ view: "files", dir: "/home/me/my dir" })),
    "?view=files&dir=/home/me/my%20dir",
  );
  assert.equal(
    locationSearch(
      terminal(4, { side: "/a/b.ts", line: 3 }),
      "?project=p1&view=x&scratch=1",
    ),
    "?project=p1&scratch=1&view=terminal&terminal=4&side=/a/b.ts&line=3",
  );
  // A dialog's `of` only goes with notifications; a line only with a file.
  assert.equal(
    locationSearch(at({ view: "grid", line: 5, dialog: "help", of: 2 })),
    "?view=grid&dialog=help",
  );
});

test("invalid parameters are ignored", () => {
  assert.deepEqual(parseLocation("?view=nowhere&dialog=bogus"), {});
  assert.equal(hasLocation(parseLocation("")), false);
  assert.equal(hasLocation(parseLocation("?project=p&pane=4")), false);
  const parsed = parseLocation("?view=terminal&terminal=-3&side=%00x&line=abc");
  assert.equal(parsed.terminal, 0);
  assert.equal(parsed.side, "");
  assert.equal(parsed.line, undefined);
  assert.equal(parseLocation("?view=terminal&terminal=01").terminal, 0);
  assert.equal(parseLocation(`?view=files&dir=${"a".repeat(5000)}`).dir, "");
});

test("KelpHub-era URLs map to the nearest place", () => {
  const place = (search) => resolveLocation(parseLocation(search), terminal(1));
  assert.deepEqual(place("?scratch=1&view=terminals&terminal=7"), terminal(7));
  assert.deepEqual(place("?scratch=1&view=terminals"), grid);
  assert.deepEqual(place("?scratch=1&view=files"), at({ view: "files" }));
  // Overview and events leave the application's own place.
  assert.equal(hasLocation(parseLocation("?scratch=1&view=overview")), false);
  assert.equal(
    hasLocation(parseLocation("?project=a&workspace=b&view=events")),
    false,
  );
  // A legacy URL is rewritten with its other parameters intact.
  assert.equal(
    locationSearch(
      terminal(7),
      "?project=a&workspace=b&view=terminals&terminal=7",
    ),
    "?project=a&workspace=b&view=terminal&terminal=7",
  );
});

test("a dialog alone opens over the current place", () => {
  assert.deepEqual(
    resolveLocation(parseLocation("?dialog=help"), terminal(5)),
    terminal(5, { dialog: "help" }),
  );
  assert.deepEqual(
    resolveLocation(
      parseLocation("?view=grid"),
      terminal(5, { dialog: "help" }),
    ),
    grid,
  );
});

test("dialog names map both ways", () => {
  for (const [param, dialog] of [
    ["settings", "config"],
    ["closed", "recovery"],
    ["status", "stats"],
    ["help", "help"],
    ["notifications", "notifications"],
  ]) {
    assert.equal(dialogFromParam(param), dialog);
    assert.equal(dialogParam(dialog), param);
  }
  assert.equal(dialogFromParam("constructor"), "");
  assert.equal(dialogParam(""), "");
  assert.equal(dialogParam("keys"), "");
});

test("the state's location", () => {
  const state = {
    phoneHome: false,
    view: "single",
    terminal: 4,
    filesDir: "/srv",
    file: "",
    side: "",
    dialog: "",
    notifyPane: 0,
  };
  assert.deepEqual(locationOf(state), terminal(4));
  assert.deepEqual(
    locationOf({ ...state, side: "/srv/a", sideLine: 9 }),
    terminal(4, { side: "/srv/a", line: 9 }),
  );
  assert.deepEqual(
    locationOf({ ...state, view: "grid", side: "/srv/a" }),
    grid,
  );
  assert.deepEqual(
    locationOf({ ...state, view: "files" }),
    at({ view: "files", dir: "/srv" }),
  );
  assert.deepEqual(
    locationOf({ ...state, view: "files", file: "/srv/b", fileLine: 2 }),
    at({ view: "files", file: "/srv/b", line: 2 }),
  );
  assert.deepEqual(
    locationOf({ ...state, phoneHome: true, dialog: "config" }),
    at({ view: "home", dialog: "settings" }),
  );
  assert.deepEqual(
    locationOf({ ...state, dialog: "notifications", notifyPane: 6 }),
    terminal(4, { dialog: "notifications", of: 6 }),
  );
  // Dialogs without an entry (none today) and transient UI stay out.
  assert.deepEqual(locationOf({ ...state, dialog: "unknown" }), terminal(4));
});

test("moves push, the same place does nothing, the first location replaces", () => {
  assert.equal(historyMode(null, grid).mode, "replace");
  assert.equal(historyMode(grid, grid).mode, "none");
  assert.equal(historyMode(grid, terminal(1)).mode, "push");
  assert.equal(historyMode(terminal(1), terminal(2)).mode, "push");
  assert.equal(
    historyMode(
      at({ view: "files", dir: "/a" }),
      at({ view: "files", dir: "/a/b" }),
    ).mode,
    "push",
  );
  assert.equal(
    historyMode(terminal(1), terminal(1, { side: "/f" })).mode,
    "push",
  );
  // Another line in the same file refines the entry.
  assert.equal(
    historyMode(
      at({ view: "files", file: "/f", line: 2 }),
      at({ view: "files", file: "/f", line: 9 }),
    ).mode,
    "replace",
  );
});

test("automatic changes replace", () => {
  assert.deepEqual(historyMode(terminal(1), terminal(2), { automatic: true }), {
    mode: "replace",
    overlay: false,
  });
  assert.equal(
    historyMode(terminal(1), grid, { automatic: true }).mode,
    "replace",
  );
});

test("panels push an overlay that closing goes back from", () => {
  const settings = terminal(1, { dialog: "settings" });
  assert.deepEqual(historyMode(terminal(1), settings), {
    mode: "push",
    overlay: true,
  });
  // Closing it: back to the entry below.
  assert.deepEqual(historyMode(settings, terminal(1), { overlay: true }), {
    mode: "back",
    overlay: false,
  });
  // A panel loaded from a URL has no entry below: closing replaces.
  assert.equal(
    historyMode(settings, terminal(1), { overlay: false }).mode,
    "replace",
  );
  // Another panel in its place replaces and stays an overlay.
  assert.deepEqual(
    historyMode(settings, terminal(1, { dialog: "help" }), { overlay: true }),
    { mode: "replace", overlay: true },
  );
  // Leaving from inside it (a terminal opened from the Terminals table)
  // takes the panel's entry, so Back returns to the place before it.
  const table = terminal(1, { dialog: "terminals" });
  assert.deepEqual(historyMode(table, terminal(2), { overlay: true }), {
    mode: "replace",
    overlay: false,
  });
  assert.equal(
    historyMode(table, terminal(2), { overlay: false }).mode,
    "push",
  );
  // An automatic change under a panel keeps it an overlay only in place.
  assert.deepEqual(
    historyMode(settings, at({ ...settings, line: 0, side: "" }), {
      automatic: true,
      overlay: true,
    }),
    { mode: "none", overlay: true },
  );
  assert.deepEqual(
    historyMode(settings, terminal(2, { dialog: "settings" }), {
      automatic: true,
      overlay: true,
    }),
    { mode: "replace", overlay: false },
  );
  // The first location keeps a reloaded overlay only while it has a panel.
  assert.deepEqual(historyMode(null, settings, { overlay: true }), {
    mode: "replace",
    overlay: true,
  });
  assert.deepEqual(historyMode(null, terminal(1), { overlay: true }), {
    mode: "replace",
    overlay: false,
  });
});

test("history state carries a version", () => {
  assert.deepEqual(historyState(true), {
    kelpmux: { v: HISTORY_VERSION, overlay: true },
  });
  assert.deepEqual(readHistoryState(historyState(true)), { overlay: true });
  assert.deepEqual(readHistoryState(null), { overlay: false });
  assert.deepEqual(readHistoryState({ kelpmux: { v: 999, overlay: true } }), {
    overlay: false,
  });
});
