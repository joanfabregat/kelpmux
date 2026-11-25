import test from "node:test";
import assert from "node:assert/strict";
import { encodeKey, encodeText } from "../src/keyboard.ts";
test("quit controls do not leak CSI-u releases into a restored shell", () => {
  for (const event of [
    { key: "c", ctrlKey: true },
    { key: "d", ctrlKey: true },
    { key: "Escape" },
  ]) {
    assert.equal(encodeKey(event, { keyboard_flags: 3 }, true), null);
  }
  assert.equal(
    encodeKey({ key: "c", ctrlKey: true }, { keyboard_flags: 11 }, true),
    "\x1b[99;5:3u",
  );
});
test("legacy shell controls and cursor modes remain distinct", () => {
  assert.equal(encodeKey({ key: "c", ctrlKey: true }, {}), "\x03");
  assert.equal(
    encodeKey({ key: "ArrowUp" }, { application_cursor: true }),
    "\x1bOA",
  );
  assert.equal(encodeKey({ key: "Tab", shiftKey: true }, {}), "\x1b[Z");
  assert.equal(encodeKey({ key: "Backspace", altKey: true }, {}), "\x1b\x7f");
  assert.equal(encodeKey({ key: "a" }, {}), null);
  assert.equal(encodeText("日本語", {}), "日本語");
});
test("Codex enhancement preserves modified enter and disambiguated controls", () => {
  const modes = { keyboard_flags: 1 };
  assert.equal(
    encodeKey({ key: "Enter", shiftKey: true }, modes),
    "\x1b[13;2u",
  );
  assert.equal(encodeKey({ key: "Enter" }, modes), "\r");
  assert.equal(encodeKey({ key: "c", ctrlKey: true }, modes), "\x1b[99;5u");
  assert.equal(encodeKey({ key: "Escape" }, modes), "\x1b[27;1u");
});
test("event and associated-text flags do not duplicate text or invent releases", () => {
  assert.equal(encodeKey({ key: "a" }, { keyboard_flags: 1 }, true), null);
  assert.equal(encodeKey({ key: "Enter" }, { keyboard_flags: 3 }, true), null);
  assert.equal(
    encodeKey({ key: "a", repeat: true }, { keyboard_flags: 26 }),
    "\x1b[97;1:2;97u",
  );
  assert.equal(
    encodeKey({ key: "a" }, { keyboard_flags: 10 }, true),
    "\x1b[97;1:3u",
  );
  assert.equal(encodeText("a", { keyboard_flags: 24 }), "\x1b[97;1;97u");
});
