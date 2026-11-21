import { test } from "node:test";
import assert from "node:assert/strict";
import {
  answerText,
  barKeyBytes,
  barKeys,
  consumesCtrl,
  nextInputMode,
  stickyText,
  withCtrl,
} from "../src/terminal/touch-keys.ts";

const plain = {};
const kitty = { keyboard_flags: 1 };

test("the key bar has the agreed keys in order", () => {
  assert.deepEqual(
    barKeys.map((k) => k.key),
    [
      "Escape",
      "Tab",
      "Ctrl",
      "ArrowLeft",
      "ArrowUp",
      "ArrowDown",
      "ArrowRight",
      "Prefix",
      "Paste",
      "Enter",
      "Yes",
      "No",
    ],
  );
});

test("bar keys encode as terminal keys", () => {
  assert.equal(barKeyBytes("Escape", false, plain), "\x1b");
  assert.equal(barKeyBytes("Tab", false, plain), "\t");
  assert.equal(barKeyBytes("Enter", false, plain), "\r");
  assert.equal(barKeyBytes("ArrowUp", false, plain), "\x1b[A");
  assert.equal(barKeyBytes("ArrowLeft", false, plain), "\x1b[D");
  // Application cursor mode, as vim and less request.
  assert.equal(
    barKeyBytes("ArrowDown", false, { application_cursor: true }),
    "\x1bOB",
  );
  // Kitty keyboard protocol: an unmodified Escape is disambiguated.
  assert.equal(barKeyBytes("Escape", false, kitty), "\x1b[27;1u");
});

test("sticky Ctrl modifies the next bar key", () => {
  assert.equal(barKeyBytes("ArrowRight", true, plain), "\x1b[1;5C");
  assert.equal(barKeyBytes("Tab", true, kitty), "\x1b[9;5u");
  for (const key of ["Escape", "Tab", "ArrowUp", "Enter"])
    assert.equal(consumesCtrl(key), true, key);
  for (const key of ["Ctrl", "Prefix", "Paste", "Hide"])
    assert.equal(consumesCtrl(key), false, key);
});

test("application keys send nothing to the terminal", () => {
  for (const key of ["Ctrl", "Prefix", "Paste", "Hide"])
    assert.equal(barKeyBytes(key, false, plain), null, key);
});

test("yes and no follow the agent", () => {
  assert.equal(answerText("claude", "yes"), "1");
  assert.equal(answerText("claude", "no"), "\x1b");
  assert.equal(answerText("codex", "yes"), "y");
  assert.equal(answerText("codex", "no"), "n");
  assert.equal(answerText(undefined, "yes"), "y");
  assert.equal(barKeyBytes("Yes", false, plain, "claude"), "1");
  assert.equal(barKeyBytes("No", false, plain, "codex"), "n");
  // Sticky Ctrl does not turn an answer into a control character.
  assert.equal(barKeyBytes("Yes", true, plain, "codex"), "y");
  // A full-keyboard kitty application receives the answer as key codes.
  assert.equal(
    barKeyBytes("Yes", false, { keyboard_flags: 8 }, "codex"),
    "\x1b[121;1u",
  );
});

test("sticky Ctrl applies to typed keys and text", () => {
  assert.deepEqual(withCtrl({ key: "c" }), { key: "c", ctrlKey: true });
  assert.deepEqual(withCtrl({ key: "c", metaKey: true }), {
    key: "c",
    metaKey: true,
  });
  assert.equal(stickyText("c", plain), "\x03");
  assert.equal(stickyText("d", plain), "\x04");
  assert.equal(stickyText("[", plain), "\x1b");
  // Only the first character gains Ctrl.
  assert.equal(stickyText("ab", plain), "\x01b");
  assert.equal(stickyText("", plain), "");
  // Characters without a control code are sent as typed.
  assert.equal(stickyText("é", plain), "é");
  assert.equal(stickyText("c", kitty), "\x1b[99;5u");
});

test("reading is the default; input mode is explicit", () => {
  let mode = "reading";
  mode = nextInputMode(mode, "open-terminal");
  assert.equal(mode, "reading");
  mode = nextInputMode(mode, "input-control");
  assert.equal(mode, "input");
  mode = nextInputMode(mode, "input-control");
  assert.equal(mode, "reading");
  mode = nextInputMode(mode, "key-bar");
  assert.equal(mode, "input");
  assert.equal(nextInputMode(mode, "key-bar"), "input");
  assert.equal(nextInputMode(mode, "hide"), "reading");
  assert.equal(nextInputMode(mode, "blur"), "reading");
  assert.equal(nextInputMode(mode, "open-terminal"), "reading");
  assert.equal(nextInputMode("reading", "focus"), "input");
});
