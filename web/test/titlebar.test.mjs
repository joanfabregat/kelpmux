// The installed app's title bar (TitleBar.tsx, styles/titlebar.css): the
// manifest asks for the window controls overlay, the bar's location text,
// and the CSS that keeps it a drag region clear of the window controls and
// hidden outside that display mode. Playwright cannot enter the display
// mode; test/sidebar.mjs lays the bar out with the media query forced on.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { displayDir, titleLocation } from "../src/title-location.ts";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");

test("the manifest asks for the window controls overlay", () => {
  const manifest = JSON.parse(read("../public/manifest.webmanifest"));
  assert.deepEqual(manifest.display_override, ["window-controls-overlay"]);
  // Browsers without it, or with the overlay turned off, stay standalone.
  assert.equal(manifest.display, "standalone");
});

test("the location names the place shown", () => {
  assert.equal(titleLocation({ view: "sign-in" }), "Sign in");
  assert.equal(
    titleLocation({ view: "stopped", workspace: "main" }),
    "Workspace main stopped",
  );
  assert.equal(titleLocation({ view: "terminals" }), "Terminals");
  assert.equal(titleLocation({ view: "grid" }), "Grid");
  assert.equal(titleLocation({ view: "project", project: "Kelp" }), "Kelp");
  assert.equal(titleLocation({ view: "terminal", title: "claude" }), "claude");
  assert.equal(titleLocation({ view: "terminal", title: "" }), "shell");
  assert.equal(titleLocation({ view: "terminal" }), "");
  assert.equal(
    titleLocation({ view: "files", dir: "/home/me/src", home: "/home/me" }),
    "Files — ~/src",
  );
  assert.equal(
    titleLocation({ view: "files", dir: "/srv", home: "/home/me" }),
    "Files — /srv",
  );
  assert.equal(
    titleLocation({
      view: "files",
      dir: "/home/me",
      file: "/home/me/src/notes.md",
      home: "/home/me",
    }),
    "notes.md",
  );
});

test("folders under home start with ~", () => {
  assert.equal(displayDir("/home/me", "/home/me"), "~");
  assert.equal(displayDir("/home/me/a", "/home/me"), "~/a");
  assert.equal(displayDir("/home/meow", "/home/me"), "/home/meow");
  assert.equal(displayDir("~", undefined), "~");
  assert.equal(displayDir("/etc", "/"), "/etc");
});

test("the bar shows only in the overlay, as a drag region clear of the controls", () => {
  const css = read("../src/styles/titlebar.css");
  // Hidden by default, outside any media query.
  assert.match(css, /^\.titlebar \{\s*display: none;\s*\}/m);
  const start = css.indexOf("@media (display-mode: window-controls-overlay)");
  assert.ok(start > 0, "the overlay media query");
  const overlay = css.slice(start, css.indexOf("\n}\n", start));
  for (const env of ["x", "y", "width", "height"])
    assert.match(overlay, new RegExp(`env\\(titlebar-area-${env}\\b`));
  assert.match(overlay, /-webkit-app-region: drag;/);
  assert.match(overlay, /\bapp-region: drag;/);
  assert.match(overlay, /app-region: no-drag;/);
  assert.match(overlay, /user-select: none;/);
  assert.match(overlay, /background: var\(--background\);/);
  assert.match(overlay, /container: titlebar \/ inline-size;/);
  assert.match(css, /@container titlebar/);
});

test("the controls are painted in the bar's colour", () => {
  // main.tsx gives the theme-color meta --background, the bar's background.
  const main = read("../src/main.tsx");
  assert.match(
    main,
    /meta\[name="theme-color"\]'\)\s*\?\.setAttribute\("content", variables\["--background"\]\)/,
  );
});
