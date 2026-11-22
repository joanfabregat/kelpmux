import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  contrast as libContrast,
  interfaceVariables,
  loadAppearance,
  mix,
  resolveMode,
  resolveTerminalMode,
  surfaces,
  terminalVariables,
  themeVariables,
  themes,
} from "../src/appearance.ts";

// WCAG 2 relative luminance and contrast ratio.
function rgb(css) {
  const named = { white: "#ffffff", black: "#000000" }[css] || css;
  const match = /^#([\da-f]{6})$/i.exec(named);
  if (!match) throw Error(`Unsupported colour ${css}`);
  return [0, 2, 4].map((i) => parseInt(match[1].slice(i, i + 2), 16));
}
function luminance([r, g, b]) {
  const channel = (value) => {
    const c = value / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}
function contrast(a, b) {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}

// Token blocks from base.css: the dark defaults and the light overrides.
const css = readFileSync(
  new URL("../src/styles/base.css", import.meta.url),
  "utf8",
).replace(/\/\*[\s\S]*?\*\//g, "");
function declarations(selector) {
  const start = css.indexOf(`${selector} {`);
  assert.notEqual(start, -1, `${selector} block`);
  const body = css.slice(start, css.indexOf("\n}", start));
  return Object.fromEntries(
    [...body.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)].map(([, name, value]) => [
      name,
      value
        .replace(/\s+/g, " ")
        .replace(/\( /g, "(")
        .replace(/ \)/g, ")")
        .trim(),
    ]),
  );
}
const darkTokens = declarations(":root");
const lightTokens = {
  ...darkTokens,
  ...declarations(':root[data-mode="light"]'),
};
function splitTopLevel(text) {
  const parts = [];
  let depth = 0,
    last = 0;
  for (let i = 0; i < text.length; i++) {
    if (text[i] === "(") depth++;
    else if (text[i] === ")") depth--;
    else if (text[i] === "," && depth === 0) {
      parts.push(text.slice(last, i).trim());
      last = i + 1;
    }
  }
  parts.push(text.slice(last).trim());
  return parts;
}
// Evaluates the colour forms the token block uses: hex, white/black and
// color-mix(in srgb, A [p%], B [q%]), mixing gamma-encoded sRGB channels.
function evaluate(value, tokens) {
  value = value.trim();
  const variable = /^var\((--[\w-]+)\)$/.exec(value);
  if (variable) return evaluate(tokens[variable[1]], tokens);
  const mix = /^color-mix\(in srgb,(.*)\)$/.exec(value);
  if (!mix) return rgb(value);
  const stops = splitTopLevel(mix[1]).map((part) => {
    const percent = /\s([\d.]+)%$/.exec(part);
    return {
      color: evaluate(percent ? part.slice(0, percent.index) : part, tokens),
      weight: percent ? Number(percent[1]) / 100 : undefined,
    };
  });
  assert.equal(stops.length, 2);
  const [a, b] = stops;
  if (a.weight === undefined && b.weight === undefined)
    a.weight = b.weight = 0.5;
  else if (a.weight === undefined) a.weight = 1 - b.weight;
  else if (b.weight === undefined) b.weight = 1 - a.weight;
  const total = a.weight + b.weight;
  return a.color.map(
    (channel, i) => (channel * a.weight + b.color[i] * b.weight) / total,
  );
}
// A token as the interface computes it on :root for a theme and mode.
function token(name, theme, mode) {
  const tokens = mode === "light" ? lightTokens : darkTokens;
  return evaluate(`var(${name})`, {
    ...tokens,
    ...themeVariables(theme, mode),
    ...interfaceVariables(theme, mode),
  });
}
const names = Object.keys(themes);
const own = names.filter((name) => !themes[name].upstream);
const upstream = names.filter((name) => themes[name].upstream);
const modes = ["dark", "light"];
const hex6 = /^#[\da-f]{6}$/i;

test("mode resolution follows the saved mode, and the OS only for auto", () => {
  for (const prefersLight of [false, true]) {
    assert.equal(resolveMode("dark", prefersLight), "dark");
    assert.equal(resolveMode("light", prefersLight), "light");
  }
  assert.equal(resolveMode("auto", false), "dark");
  assert.equal(resolveMode("auto", true), "light");
});

test("terminal color follows the interface unless forced dark or light", () => {
  for (const shown of modes) {
    assert.equal(resolveTerminalMode("follow", shown), shown);
    assert.equal(resolveTerminalMode("dark", shown), "dark");
    assert.equal(resolveTerminalMode("light", shown), "light");
  }
  // Terminal variables are the chosen variant's colours under --tv-*.
  for (const name of names)
    for (const mode of modes) {
      const plain = themeVariables(name, mode);
      const scoped = terminalVariables(name, mode);
      assert.equal(Object.keys(scoped).length, Object.keys(plain).length);
      for (const [key, value] of Object.entries(plain))
        assert.equal(scoped[`--tv-${key.slice(2)}`], value);
    }
  // terminal.css maps every --tv-* property inside terminals.
  const terminalCss = readFileSync(
    new URL("../src/terminal/terminal.css", import.meta.url),
    "utf8",
  );
  for (const key of Object.keys(themeVariables("black")))
    assert.match(
      terminalCss,
      new RegExp(`\\s${key}: var\\(--tv-${key.slice(2)}[,)]`),
      key,
    );
});

test("saved appearances load with defaults; stale and unknown values are ignored", () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  const load = (saved) => {
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: { getItem: () => (saved === null ? null : JSON.stringify(saved)) },
    });
    return loadAppearance();
  };
  try {
    assert.deepEqual(load({ theme: "apple", font: "courier", size: 18 }), {
      theme: "apple",
      mode: "dark",
      terminal: "follow",
      font: "courier",
      size: 18,
    });
    assert.equal(load(null).mode, "dark");
    assert.equal(load(null).terminal, "follow");
    assert.equal(load({ theme: "cobalt2", mode: "sepia" }).mode, "dark");
    for (const mode of ["dark", "light", "auto"])
      assert.equal(load({ theme: "cobalt2", mode }).mode, mode);
    for (const terminal of ["follow", "dark", "light"])
      assert.equal(load({ terminal }).terminal, terminal);
    assert.equal(load({ terminal: "sepia" }).terminal, "follow");
    // A stale accent from the retired accent setting is dropped.
    const stale = load({ theme: "gruvbox", accent: "sage" });
    assert.equal(stale.theme, "gruvbox");
    assert.equal("accent" in stale, false);
  } finally {
    if (original) Object.defineProperty(globalThis, "localStorage", original);
    else delete globalThis.localStorage;
  }
});

test("dark variants are unchanged and the default mode", () => {
  for (const name of names)
    assert.deepEqual(themeVariables(name), themeVariables(name, "dark"));
  assert.equal(themeVariables("black")["--background"], "#101113");
  assert.equal(themeVariables("cobalt2")["--background"], "#132738");
  assert.equal(themeVariables("apple")["--background"], "#1e1e1e");
  assert.equal(themeVariables("black")["--terminal-selection"], "#385776");
  assert.equal(themeVariables("black")["--terminal-cursor"], "#d7dce3");
  for (const name of names)
    assert.notEqual(
      themeVariables(name, "light")["--background"],
      themeVariables(name)["--background"],
    );
});

test("upstream themes keep their published palettes", () => {
  assert.deepEqual(upstream, [
    "github",
    "githubDimmed",
    "vscode",
    "one",
    "solarized",
    "catppuccin",
    "tokyoNight",
    "gruvbox",
  ]);
  // Spot checks of each scheme's defining colours.
  const spots = {
    github: [
      ["#0d1117", "#58a6ff"],
      ["#ffffff", "#0969da"],
    ],
    githubDimmed: [
      ["#22272e", "#539bf5"],
      ["#ffffff", "#0969da"],
    ],
    vscode: [
      ["#181818", "#2472c8"],
      ["#f8f8f8", "#0451a5"],
    ],
    one: [
      ["#282c34", "#61afef"],
      ["#fafafa", "#0184bc"],
    ],
    solarized: [
      ["#002b36", "#268bd2"],
      ["#fdf6e3", "#268bd2"],
    ],
    catppuccin: [
      ["#1e1e2e", "#89b4fa"],
      ["#eff1f5", "#1e66f5"],
    ],
    tokyoNight: [
      ["#1a1b26", "#7aa2f7"],
      ["#e1e2e7", "#2e7de9"],
    ],
    gruvbox: [
      ["#282828", "#458588"],
      ["#fbf1c7", "#458588"],
    ],
  };
  for (const [name, pair] of Object.entries(spots))
    modes.forEach((mode, i) => {
      const vars = themeVariables(name, mode);
      assert.equal(vars["--background"], pair[i][0], `${name}/${mode}`);
      assert.equal(vars["--terminal-ansi-4"], pair[i][1], `${name}/${mode}`);
    });
  // Every variant has a full 16-colour palette of #rrggbb values.
  for (const name of upstream)
    for (const mode of modes) {
      const palette = themes[name][mode].palette;
      assert.equal(palette.length, 16, `${name}/${mode}`);
      for (const color of palette) assert.match(color, hex6);
    }
});

test("terminal foregrounds and light ANSI colours are readable", () => {
  for (const name of names)
    for (const mode of modes) {
      const vars = themeVariables(name, mode);
      const background = rgb(vars["--background"]);
      const ratio = contrast(rgb(vars["--terminal-foreground"]), background);
      // KelpMux's own palettes target 7:1; upstream schemes are kept as
      // published, and the lowest (Solarized Light) is 4.1:1.
      const minimum = themes[name].upstream ? 4 : 7;
      assert.ok(
        ratio >= minimum,
        `${name}/${mode} foreground ${ratio.toFixed(2)}`,
      );
      // The controller-theme report needs #rrggbb values.
      for (const key of Object.keys(vars))
        if (key !== "--terminal-selection-text")
          assert.match(vars[key], hex6, `${name}/${mode} ${key}`);
    }
  // Light ANSI contrast is enforced for KelpMux's own light palettes only.
  // Upstream light schemes are copied unmodified; their measured values
  // are printed with KELP_CONTRAST=1.
  for (const name of own) {
    const vars = themeVariables(name, "light");
    const background = rgb(vars["--background"]);
    const ansi = (index) => rgb(vars[`--terminal-ansi-${index}`]);
    for (const index of [1, 2, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14]) {
      const ratio = contrast(ansi(index), background);
      assert.ok(ratio >= 3, `${name} light ANSI ${index} ${ratio.toFixed(2)}`);
    }
    for (const index of [0, 8])
      assert.ok(contrast(ansi(index), background) >= 4.5, `${name} ${index}`);
    for (const index of [7, 15])
      assert.ok(luminance(ansi(index)) > 0.4, `${name} ANSI ${index} light`);
    assert.ok(
      contrast(rgb(vars["--terminal-cursor"]), background) >= 3,
      `${name} light cursor`,
    );
  }
});

test("interface surfaces derive from the theme background for the mode", () => {
  for (const name of names)
    for (const mode of modes) {
      const background = themeVariables(name, mode)["--background"];
      const derived = surfaces(background, mode);
      assert.equal(derived.bg, background);
      // The TypeScript derivation matches base.css's color-mix tokens.
      for (const key of ["surface-1", "surface-2", "surface-3"])
        token(`--${key}`, name, mode).forEach((channel, i) =>
          assert.ok(
            Math.abs(channel - rgb(derived[key])[i]) <= 0.5,
            `${name}/${mode} ${key}`,
          ),
        );
      // Surfaces lift towards white in dark mode and black in light mode.
      const lift = mode === "dark" ? 1 : -1;
      assert.ok(
        lift *
          (luminance(rgb(derived["surface-3"])) - luminance(rgb(background))) >
          0,
        `${name}/${mode} surface direction`,
      );
    }
  // Different themes give different interface surfaces.
  const backgrounds = new Set(
    names.map(
      (name) =>
        surfaces(themeVariables(name, "dark")["--background"], "dark")[
          "surface-2"
        ],
    ),
  );
  assert.equal(backgrounds.size, names.length);
  assert.equal(mix("#ffffff", "#000000", 0.5), "#808080");
  assert.equal(libContrast("#ffffff", "#000000"), 21);
});

test("UI text, status, Git and vitals tokens are readable in both modes", () => {
  for (const name of names)
    for (const mode of modes) {
      for (const surface of ["--bg", "--surface-2"])
        for (const text of ["--text", "--text-2"]) {
          const ratio = contrast(
            token(text, name, mode),
            token(surface, name, mode),
          );
          assert.ok(
            ratio >= 4.5,
            `${name}/${mode} ${text} on ${surface} ${ratio.toFixed(2)}`,
          );
        }
      for (const status of [
        "--st-working",
        "--st-input",
        "--st-done",
        "--st-failed",
        "--st-idle",
        "--git-added",
        "--git-modified",
        "--git-deleted",
        "--vital-cpu",
        "--vital-memory",
        "--vital-disk",
        "--vital-critical",
      ]) {
        const ratio = contrast(
          token(status, name, mode),
          token("--surface-2", name, mode),
        );
        assert.ok(ratio >= 3, `${name}/${mode} ${status} ${ratio.toFixed(2)}`);
      }
    }
});

// Accent lines, rings, icons and text against surfaces, and the text of
// filled accent buttons.
function accentRatios(theme, mode) {
  const fill = token("--accent", theme, mode);
  const ratios = {};
  for (const surface of ["--bg", "--surface-2", "--surface-3"]) {
    const s = token(surface, theme, mode);
    ratios[`--accent on ${surface}`] = contrast(fill, s);
    ratios[`--accent-text on ${surface}`] = contrast(
      token("--accent-text", theme, mode),
      s,
    );
  }
  ratios["--on-accent on --accent"] = contrast(
    token("--on-accent", theme, mode),
    fill,
  );
  return ratios;
}

test("accents come from the theme's blue and stay readable in both modes", () => {
  for (const name of names)
    for (const mode of modes) {
      for (const [label, ratio] of Object.entries(accentRatios(name, mode)))
        assert.ok(
          ratio >= (label.startsWith("--on-accent") ? 4.5 : 3),
          `${name}/${mode} ${label} ${ratio.toFixed(2)}`,
        );
      const { "--accent-base": accent } = interfaceVariables(name, mode);
      const blue = themeVariables(name, mode)["--terminal-ansi-4"];
      // The accent is the blue itself, or the blue moved towards white
      // (dark) or black (light) only as far as contrast requires.
      if (accent !== blue) {
        const toward = mode === "dark" ? "#ffffff" : "#000000";
        assert.ok(
          Array.from({ length: 21 }, (_, step) =>
            mix(toward, blue, step / 20),
          ).includes(accent),
          `${name}/${mode} accent ${accent} from ${blue}`,
        );
      }
    }
  // Themes whose blue already contrasts use it unchanged.
  assert.equal(interfaceVariables("black", "dark")["--accent-base"], "#79ade0");
  assert.equal(
    interfaceVariables("github", "light")["--accent-base"],
    "#0969da",
  );
});

// Prints the measured values with `KELP_CONTRAST=1 node --test`.
if (process.env.KELP_CONTRAST)
  for (const name of names)
    for (const mode of modes) {
      const vars = themeVariables(name, mode);
      const bg = rgb(vars["--background"]);
      const ansi = [1, 2, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14].map((i) =>
        contrast(rgb(vars[`--terminal-ansi-${i}`]), bg).toFixed(2),
      );
      console.log(
        name,
        mode,
        "fg",
        contrast(rgb(vars["--terminal-foreground"]), bg).toFixed(2),
        "ansi",
        ansi.join(" "),
        "accent",
        Object.entries(accentRatios(name, mode))
          .map(([label, ratio]) => `${label} ${ratio.toFixed(2)}`)
          .join(", "),
      );
    }

test("the interface foreground is the theme's own unless contrast requires", () => {
  // KelpMux's own themes already contrast: the interface text is unchanged.
  for (const name of own)
    for (const mode of modes)
      assert.equal(
        interfaceVariables(name, mode)["--ui-foreground"],
        themeVariables(name, mode)["--terminal-foreground"],
      );
  // Low-contrast upstream foregrounds only gain contrast.
  for (const name of upstream)
    for (const mode of modes) {
      const ui = interfaceVariables(name, mode)["--ui-foreground"];
      const vars = themeVariables(name, mode);
      const bg = vars["--background"];
      assert.ok(
        libContrast(ui, bg) >= libContrast(vars["--terminal-foreground"], bg),
        `${name}/${mode}`,
      );
    }
});
