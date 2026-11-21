import { test } from "node:test";
import assert from "node:assert/strict";
import {
  keyboardInset,
  layoutMode,
  orientation,
  sidebarFolded,
} from "../src/device.ts";

test("touch screens pick the phone or tablet layout by their short side", () => {
  const touch = (width, height) => layoutMode({ width, height, coarse: true });
  // iPhone SE, iPhone 15 and iPhone 15 Pro Max, both orientations.
  for (const [w, h] of [
    [375, 667],
    [393, 852],
    [430, 932],
  ]) {
    assert.equal(touch(w, h), "phone");
    assert.equal(touch(h, w), "phone");
  }
  // iPad mini, iPad Air 11" and iPad Pro 13", both orientations.
  for (const [w, h] of [
    [744, 1133],
    [820, 1180],
    [1032, 1376],
  ]) {
    assert.equal(touch(w, h), "tablet");
    assert.equal(touch(h, w), "tablet");
  }
});

test("a mouse gets the desktop layout unless the window is narrow", () => {
  const mouse = (width, height) => layoutMode({ width, height, coarse: false });
  assert.equal(mouse(1440, 900), "desktop");
  assert.equal(mouse(800, 1200), "desktop");
  assert.equal(mouse(420, 900), "phone");
});

test("orientation follows the longer side", () => {
  assert.equal(orientation({ width: 820, height: 1180 }), "portrait");
  assert.equal(orientation({ width: 1180, height: 820 }), "landscape");
  assert.equal(orientation({ width: 800, height: 800 }), "landscape");
});

test("a tablet in portrait starts folded; elsewhere the preference applies", () => {
  // Portrait uses its own transient state, true each time it turns.
  assert.equal(sidebarFolded("tablet", "portrait", false, true), true);
  assert.equal(sidebarFolded("tablet", "portrait", true, false), false);
  assert.equal(sidebarFolded("tablet", "landscape", false, true), false);
  assert.equal(sidebarFolded("tablet", "landscape", true, false), true);
  assert.equal(sidebarFolded("desktop", "portrait", false, true), false);
  assert.equal(sidebarFolded("desktop", "landscape", true, false), true);
  // The phone's list is the full sidebar, never folded.
  assert.equal(sidebarFolded("phone", "portrait", true, true), false);
});

test("keyboard inset is what the visual viewport loses at the bottom", () => {
  assert.equal(keyboardInset(844, { height: 844, offsetTop: 0 }), 0);
  assert.equal(keyboardInset(844, { height: 508, offsetTop: 0 }), 336);
  // Safari may scroll the visual viewport down inside the layout viewport.
  assert.equal(keyboardInset(844, { height: 508, offsetTop: 40 }), 296);
  assert.equal(keyboardInset(844, { height: 900, offsetTop: 0 }), 0);
  assert.equal(keyboardInset(844, null), 0);
});
