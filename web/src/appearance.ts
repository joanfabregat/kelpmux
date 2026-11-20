export const defaultPalette = [
  "#171a20",
  "#d46f77",
  "#88bd92",
  "#d9bd80",
  "#79ade0",
  "#b299cf",
  "#7fbdc1",
  "#d9dce2",
  "#646d7a",
  "#ed8793",
  "#a6da95",
  "#eed49f",
  "#8aadf4",
  "#c6a0f6",
  "#8bd5ca",
  "#ffffff",
];
type Variant = {
  background: string;
  foreground?: string;
  cursor?: string;
  cursorText?: string;
  selection?: string;
  selectionText?: string;
  palette?: string[];
};
type Theme = {
  label: string;
  dark: Variant;
  light: Variant;
  /** Both variants are an upstream scheme, copied without changes. */
  upstream?: boolean;
};
/** A 16-colour ANSI palette from two space-separated rows of 8. */
function ansi(normal: string, bright: string): string[] {
  return `${normal} ${bright}`.split(" ");
}
// Solarized's canonical ANSI mapping (base02, red, green, yellow, blue,
// magenta, cyan, base2; base03, orange, base01, base00, base0, violet,
// base1, base3), shared by its dark and light variants.
const solarized = ansi(
  "#073642 #dc322f #859900 #b58900 #268bd2 #d33682 #2aa198 #eee8d5",
  "#002b36 #cb4b16 #586e75 #657b83 #839496 #6c71c4 #93a1a1 #fdf6e3",
);
// primer/github-vscode-theme: GitHub Light Default.
function githubLight(): Variant {
  return {
    background: "#ffffff",
    foreground: "#1f2328",
    palette: ansi(
      "#24292f #cf222e #116329 #4d2d00 #0969da #8250df #1b7c83 #6e7781",
      "#57606a #a40e26 #1a7f37 #633c01 #218bff #a475f9 #3192aa #8c959f",
    ),
  };
}
/** Colour mode chosen in Settings → Appearance. "auto" follows the
 * operating system (prefers-color-scheme). */
export type Mode = "dark" | "light" | "auto";
export const modes: readonly (readonly [Mode, string])[] = [
  ["dark", "Dark"],
  ["light", "Light"],
  ["auto", "Auto"],
];
// Dark variants: exact palettes from Rootshell Resources/themes (MIT); see
// THIRD_PARTY.md. Light variants: KelpMux's own light counterparts (Cobalt2
// light is derived from the Rootshell Cobalt2 palette); see THIRD_PARTY.md.
// In every light palette, ANSI 0/8 are dark and 7/15 light grey, and the
// text colours 1–6 and 9–14 keep at least 3:1 contrast on the background
// (enforced by test/appearance.test.mjs).
export const themes = {
  black: {
    label: "Black",
    dark: { background: "#101113" },
    light: {
      background: "#fbfbfc",
      foreground: "#1b1d22",
      cursor: "#1b1d22",
      cursorText: "#fbfbfc",
      selection: "#c9d9ee",
      selectionText: "",
      palette: [
        "#1b1d22",
        "#b8323e",
        "#2f7a3c",
        "#876500",
        "#2c5fb3",
        "#7d48ad",
        "#16757b",
        "#d4d7dc",
        "#5c6370",
        "#d03c4c",
        "#3a8a48",
        "#9a7400",
        "#3a70cc",
        "#9056c4",
        "#1d868c",
        "#eceef1",
      ],
    },
  },
  cobalt2: {
    label: "Cobalt2",
    dark: {
      background: "#132738",
      foreground: "#ffffff",
      cursor: "#f0cc09",
      cursorText: "#98998c",
      selection: "#18354f",
      selectionText: "#b5b5b5",
      palette: [
        "#000000",
        "#ff0000",
        "#38de21",
        "#ffe50a",
        "#1460d2",
        "#ff005d",
        "#00bbbb",
        "#bbbbbb",
        "#555555",
        "#f40e17",
        "#3bd01d",
        "#edc809",
        "#5555ff",
        "#ff55ff",
        "#6ae3fa",
        "#ffffff",
      ],
    },
    // Cobalt2's yellow and blues, darkened for a light background.
    light: {
      background: "#eef4fa",
      foreground: "#0d2438",
      cursor: "#a67c00",
      cursorText: "#ffffff",
      selection: "#c3daf2",
      selectionText: "",
      palette: [
        "#000000",
        "#d00000",
        "#1d8212",
        "#8f7400",
        "#1460d2",
        "#cc004a",
        "#007a7a",
        "#b8c2cc",
        "#555555",
        "#e00c14",
        "#23900f",
        "#9c7f00",
        "#4545e6",
        "#b22cb2",
        "#00809a",
        "#e2e9f0",
      ],
    },
  },
  apple: {
    label: "Apple System Colors",
    dark: {
      background: "#1e1e1e",
      foreground: "#ffffff",
      cursor: "#98989d",
      cursorText: "#ffffff",
      selection: "#3f638b",
      selectionText: "#ffffff",
      palette: [
        "#1a1a1a",
        "#cc372e",
        "#26a439",
        "#cdac08",
        "#0869cb",
        "#9647bf",
        "#479ec2",
        "#98989d",
        "#464646",
        "#ff453a",
        "#32d74b",
        "#ffd60a",
        "#0a84ff",
        "#bf5af2",
        "#76d6ff",
        "#ffffff",
      ],
    },
    // Apple light-appearance system colours, from Apple's Human Interface
    // Guidelines (Color → System colors). ANSI 1–6 use the
    // increased-contrast light values (systemRed #d70015, systemGreen
    // #248a3d, systemYellow #b25000, systemBlue #0040dd, systemPurple
    // #8944ab, systemCyan #0071a4); 9–14 use the default light values
    // (systemRed #ff3b30, systemBlue #007aff, systemPurple #af52de), with
    // systemGreen #34c759, systemYellow #ffcc00 and systemCyan #32ade6
    // darkened (same hue) to reach 3:1 on white. Greys: systemGray
    // (accessible) #6c6c70, systemGray3 #c7c7cc, systemGray5 #e5e5ea;
    // selection is macOS selectedTextBackgroundColor #b3d7ff.
    light: {
      background: "#ffffff",
      foreground: "#000000",
      cursor: "#8e8e93",
      cursorText: "#ffffff",
      selection: "#b3d7ff",
      selectionText: "#000000",
      palette: [
        "#1a1a1a",
        "#d70015",
        "#248a3d",
        "#b25000",
        "#0040dd",
        "#8944ab",
        "#0071a4",
        "#c7c7cc",
        "#6c6c70",
        "#ff3b30",
        "#2a9a46",
        "#a38500",
        "#007aff",
        "#af52de",
        "#2389b6",
        "#e5e5ea",
      ],
    },
  },
  // Upstream dark/light pairs. Every value below is the upstream palette,
  // unmodified (sources and licenses in THIRD_PARTY.md). They are marked
  // `upstream`: KelpMux does not retune them, so some light variants fall
  // below the 3:1 ANSI contrast target that KelpMux's own light palettes
  // meet (measured in test/appearance.test.mjs). Where an upstream scheme
  // defines no cursor or selection colour, the fallbacks in
  // themeVariables() apply (cursor = foreground, selection derived from
  // the scheme's blue).
  // primer/github-vscode-theme: GitHub Dark Default / Light Default. ANSI
  // colours are @primer/primitives 7.10.0 ansi.*, foreground fg.default
  // (with the theme's colors.js overrides) and background canvas.default
  // (editor.background; the theme sets no terminal.background).
  github: {
    label: "GitHub",
    upstream: true,
    dark: {
      background: "#0d1117",
      foreground: "#e6edf3",
      palette: ansi(
        "#484f58 #ff7b72 #3fb950 #d29922 #58a6ff #bc8cff #39c5cf #b1bac4",
        "#6e7681 #ffa198 #56d364 #e3b341 #79c0ff #d2a8ff #56d4dd #ffffff",
      ),
    },
    light: githubLight(),
  },
  // primer/github-vscode-theme: GitHub Dark Dimmed, paired with GitHub
  // Light Default (GitHub has no dimmed light variant).
  githubDimmed: {
    label: "GitHub Dimmed",
    upstream: true,
    dark: {
      background: "#22272e",
      foreground: "#adbac7",
      palette: ansi(
        "#545d68 #f47067 #57ab5a #c69026 #539bf5 #b083f0 #39c5cf #909dab",
        "#636e7b #ff938a #6bc46d #daaa3f #6cb6ff #dcbdfb #56d4dd #cdd9e5",
      ),
    },
    light: githubLight(),
  },
  // microsoft/vscode: Dark Modern / Light Modern. Their terminal background
  // is panel.background and the ANSI colours are VS Code's terminal
  // defaults; selection is editor.selectionBackground and the Light Modern
  // cursor is its terminalCursor.foreground.
  vscode: {
    label: "VS Code",
    upstream: true,
    dark: {
      background: "#181818",
      foreground: "#cccccc",
      selection: "#264f78",
      palette: ansi(
        "#000000 #cd3131 #0dbc79 #e5e510 #2472c8 #bc3fbc #11a8cd #e5e5e5",
        "#666666 #f14c4c #23d18b #f5f543 #3b8eea #d670d6 #29b8db #e5e5e5",
      ),
    },
    light: {
      background: "#f8f8f8",
      foreground: "#3b3b3b",
      cursor: "#005fb8",
      selection: "#add6ff",
      palette: ansi(
        "#000000 #cd3131 #107c10 #949800 #0451a5 #bc05bc #0598bc #555555",
        "#666666 #f14c4c #14ce14 #b5ba00 #3b8eea #d670d6 #29b8db #a5a5a5",
      ),
    },
  },
  // One Half Dark / One Half Light (sonph/onehalf, the terminal port of
  // Atom's One themes), as shipped in microsoft/terminal's schemes.
  // One Half Light's selectionBackground (#383a42, its foreground) is
  // left out: KelpMux paints selection opaquely, which would hide the
  // selected text, so the derived selection colour applies instead.
  one: {
    label: "One Half",
    upstream: true,
    dark: {
      background: "#282c34",
      foreground: "#dcdfe4",
      cursor: "#ffffff",
      palette: ansi(
        "#282c34 #e06c75 #98c379 #e5c07b #61afef #c678dd #56b6c2 #dcdfe4",
        "#5a6374 #e06c75 #98c379 #e5c07b #61afef #c678dd #56b6c2 #dcdfe4",
      ),
    },
    light: {
      background: "#fafafa",
      foreground: "#383a42",
      cursor: "#4f525d",
      palette: ansi(
        "#383a42 #e45649 #50a14f #c18301 #0184bc #a626a4 #0997b3 #fafafa",
        "#4f525d #df6c75 #98c379 #e4c07a #61afef #c577dd #56b5c1 #ffffff",
      ),
    },
  },
  // Solarized (Ethan Schoonover): one ANSI palette for both variants;
  // cursor text, selection and selected text as in its iTerm2 schemes.
  solarized: {
    label: "Solarized",
    upstream: true,
    dark: {
      background: "#002b36",
      foreground: "#839496",
      cursorText: "#073642",
      selection: "#073642",
      selectionText: "#93a1a1",
      palette: solarized,
    },
    light: {
      background: "#fdf6e3",
      foreground: "#657b83",
      cursorText: "#eee8d5",
      selection: "#eee8d5",
      selectionText: "#586e75",
      palette: solarized,
    },
  },
  // catppuccin/windows-terminal: Mocha / Latte.
  catppuccin: {
    label: "Catppuccin",
    upstream: true,
    dark: {
      background: "#1e1e2e",
      foreground: "#cdd6f4",
      cursor: "#f5e0dc",
      selection: "#585b70",
      palette: ansi(
        "#45475a #f38ba8 #a6e3a1 #f9e2af #89b4fa #f5c2e7 #94e2d5 #bac2de",
        "#585b70 #f38ba8 #a6e3a1 #f9e2af #89b4fa #f5c2e7 #94e2d5 #a6adc8",
      ),
    },
    light: {
      background: "#eff1f5",
      foreground: "#4c4f69",
      cursor: "#dc8a78",
      selection: "#acb0be",
      palette: ansi(
        "#5c5f77 #d20f39 #40a02b #df8e1d #1e66f5 #ea76cb #179299 #acb0be",
        "#acb0be #d20f39 #40a02b #df8e1d #1e66f5 #ea76cb #179299 #bcc0cc",
      ),
    },
  },
  // folke/tokyonight.nvim terminal extras (kitty): Night / Day.
  tokyoNight: {
    label: "Tokyo Night",
    upstream: true,
    dark: {
      background: "#1a1b26",
      foreground: "#c0caf5",
      selection: "#283457",
      selectionText: "#c0caf5",
      palette: ansi(
        "#15161e #f7768e #9ece6a #e0af68 #7aa2f7 #bb9af7 #7dcfff #a9b1d6",
        "#414868 #ff899d #9fe044 #faba4a #8db0ff #c7a9ff #a4daff #c0caf5",
      ),
    },
    light: {
      background: "#e1e2e7",
      foreground: "#3760bf",
      selection: "#b7c1e3",
      selectionText: "#3760bf",
      palette: ansi(
        "#b4b5b9 #f52a65 #587539 #8c6c3e #2e7de9 #9854f1 #007197 #6172b0",
        "#a1a6c5 #ff4774 #5c8524 #a27629 #358aff #a463ff #007ea8 #3760bf",
      ),
    },
  },
  // morhetz/gruvbox: dark and light (medium contrast), its Neovim
  // g:terminal_color_* palette and Normal foreground/background.
  gruvbox: {
    label: "Gruvbox",
    upstream: true,
    dark: {
      background: "#282828",
      foreground: "#ebdbb2",
      palette: ansi(
        "#282828 #cc241d #98971a #d79921 #458588 #b16286 #689d6a #a89984",
        "#928374 #fb4934 #b8bb26 #fabd2f #83a598 #d3869b #8ec07c #ebdbb2",
      ),
    },
    light: {
      background: "#fbf1c7",
      foreground: "#3c3836",
      palette: ansi(
        "#fbf1c7 #cc241d #98971a #d79921 #458588 #b16286 #689d6a #7c6f64",
        "#928374 #9d0006 #79740e #b57614 #076678 #8f3f71 #427b58 #3c3836",
      ),
    },
  },
} satisfies Record<string, Theme>;
export type ThemeName = keyof typeof themes;
/** The mode actually shown: "auto" resolves to the OS preference. */
export function resolveMode(
  mode: Mode,
  prefersLight: boolean,
): "dark" | "light" {
  return mode === "auto" ? (prefersLight ? "light" : "dark") : mode;
}
/** Terminal color in Settings → Appearance: follow the interface mode, or
 * always use the theme's dark or light variant for terminals. */
export type TerminalColor = "follow" | "dark" | "light";
export const terminalColors: readonly (readonly [TerminalColor, string])[] = [
  ["follow", "Follow interface"],
  ["dark", "Always dark"],
  ["light", "Always light"],
];
/** The variant terminals draw with, given the interface's shown mode. */
export function resolveTerminalMode(
  terminal: TerminalColor,
  interfaceMode: "dark" | "light",
): "dark" | "light" {
  return terminal === "follow" ? interfaceMode : terminal;
}
export function variant(
  name: ThemeName,
  mode: "dark" | "light" = "dark",
): Variant {
  return themes[name][mode];
}
/** Terminal colours of one theme variant, as CSS custom properties. */
export function themeVariables(
  name: ThemeName,
  mode: "dark" | "light" = "dark",
): Record<string, string> {
  const t = variant(name, mode);
  const palette = t.palette || defaultPalette;
  return {
    "--background": t.background,
    "--terminal-foreground": t.foreground || "#d7dce3",
    "--terminal-cursor": t.cursor || t.foreground || "#d7dce3",
    "--terminal-cursor-text": t.cursorText || t.background,
    // Without an upstream selection colour, tint the background with the
    // scheme's own blue; the built-in Black dark keeps its fixed value.
    "--terminal-selection":
      t.selection ||
      (t.palette ? mix(t.palette[4], t.background, 0.3) : "#385776"),
    "--terminal-selection-text": t.selectionText || "",
    ...Object.fromEntries(
      palette.map((value, index) => [`--terminal-ansi-${index}`, value]),
    ),
  };
}

/** The terminal variant's colours as --tv-* properties for :root.
 * terminal.css maps them onto the terminal colour properties inside
 * .terminal and .grid-pane only, so terminals can use a different variant
 * than the interface (whose tokens stay computed on :root). The renderer
 * reads the colours from its canvas, inside .terminal. */
export function terminalVariables(
  name: ThemeName,
  mode: "dark" | "light",
): Record<string, string> {
  return Object.fromEntries(
    Object.entries(themeVariables(name, mode)).map(([key, value]) => [
      `--tv-${key.slice(2)}`,
      value,
    ]),
  );
}

// sRGB colour arithmetic. mix() matches CSS color-mix(in srgb, ...), which
// mixes gamma-encoded channels; contrast() is the WCAG 2 ratio.
type RGB = [number, number, number];
function rgb(hex: string): RGB {
  const value = /^#([\da-f]{6})$/i.exec(hex);
  if (!value) throw Error(`Unsupported colour ${hex}`);
  return [0, 2, 4].map((i) => parseInt(value[1].slice(i, i + 2), 16)) as RGB;
}
function hex(color: RGB): string {
  return (
    "#" +
    color
      .map((c) =>
        Math.round(Math.min(255, Math.max(0, c)))
          .toString(16)
          .padStart(2, "0"),
      )
      .join("")
  );
}
/** `weight` of `a` and the rest of `b`, like color-mix(in srgb, a w%, b). */
export function mix(a: string, b: string, weight: number): string {
  const [x, y] = [rgb(a), rgb(b)];
  return hex(x.map((c, i) => c * weight + y[i] * (1 - weight)) as RGB);
}
export function luminance(color: string): number {
  const [r, g, b] = rgb(color).map((value) => {
    const c = value / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
export function contrast(a: string, b: string): number {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}
/** Moves `color` towards `target` in 5% steps until `good` accepts it (or
 * it becomes `target`). */
function reach(
  color: string,
  target: string,
  good: (candidate: string) => boolean,
): string {
  for (let step = 0; step <= 20; step++) {
    const candidate = mix(target, color, step / 20);
    if (good(candidate)) return candidate;
  }
  return target;
}
/** The interface surfaces base.css derives from a theme background: they
 * lift towards white in dark mode and towards black in light mode. */
export function surfaces(
  background: string,
  mode: "dark" | "light",
): Record<"bg" | "surface-1" | "surface-2" | "surface-3", string> {
  const tint = mode === "dark" ? "#ffffff" : "#000000";
  return {
    bg: background,
    "surface-1": mix(tint, background, 0.03),
    "surface-2": mix(tint, background, 0.055),
    "surface-3": mix(tint, background, 0.1),
  };
}
/**
 * Interface colours derived from the theme variant for the interface mode.
 * base.css derives every surface, line and text token from --background
 * and --ui-foreground; this adds a foreground and accent with enough
 * contrast on those surfaces:
 * - --ui-foreground is the terminal foreground, moved towards white (dark)
 *   or black (light) only as far as needed for 7:1 on the background and
 *   for base.css's dimmer --text-2 and --text-3 mixes (64%/44% dark,
 *   72%/56% light) to keep 4.5:1 and 3:1 on the raised surfaces (with a
 *   small margin for hex rounding), so low-contrast schemes stay readable;
 * - --accent-base is the theme's blue (ANSI 4), moved the same way until
 *   it reaches 3:1 on the background and the raised surfaces;
 * - --on-accent is text on filled accents: white or the theme's dark
 *   colour, whichever contrasts more (at least 4.5:1, else black/white).
 */
export function interfaceVariables(
  name: ThemeName,
  mode: "dark" | "light",
): Record<string, string> {
  const t = variant(name, mode);
  const palette = t.palette || defaultPalette;
  const toward = mode === "dark" ? "#ffffff" : "#000000";
  const s = surfaces(t.background, mode);
  const raised = [s.bg, s["surface-2"], s["surface-3"]];
  const [dim, faint] = mode === "dark" ? [0.64, 0.44] : [0.72, 0.56];
  const foreground = reach(
    t.foreground || "#d7dce3",
    toward,
    (candidate) =>
      contrast(candidate, t.background) >= 7 &&
      raised
        .slice(0, 2)
        .every(
          (surface) =>
            contrast(mix(candidate, t.background, dim), surface) >= 4.52 &&
            contrast(mix(candidate, t.background, faint), surface) >= 3.02,
        ),
  );
  const accent = reach(palette[4], toward, (candidate) =>
    raised.every((surface) => contrast(candidate, surface) >= 3),
  );
  const dark = mode === "dark" ? t.background : foreground;
  let onAccent =
    contrast("#ffffff", accent) >= contrast(dark, accent) ? "#ffffff" : dark;
  if (contrast(onAccent, accent) < 4.5)
    onAccent = contrast("#ffffff", accent) >= 4.5 ? "#ffffff" : "#000000";
  return {
    "--ui-foreground": foreground,
    "--accent-base": accent,
    "--on-accent": onAccent,
  };
}
export const fonts = {
  jetbrains: { label: "JetBrains Mono", family: '"JetBrains Mono", monospace' },
  system: {
    label: "System monospace",
    family: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
  },
  menlo: { label: "Menlo / Monaco", family: "Menlo, Monaco, monospace" },
  courier: { label: "Courier", family: '"Courier New", Courier, monospace' },
};
export type Appearance = {
  theme: ThemeName;
  mode: Mode;
  terminal: TerminalColor;
  font: keyof typeof fonts;
  size: number;
};
export function loadAppearance(): Appearance {
  let saved: any = {};
  try {
    saved = JSON.parse(localStorage.getItem("kelpmux-appearance") || "{}");
  } catch {}
  return {
    theme: Object.hasOwn(themes, saved?.theme) ? saved.theme : "black",
    // Appearances saved before light mode existed have no mode: keep dark.
    mode: modes.some(([value]) => value === saved?.mode) ? saved.mode : "dark",
    terminal: terminalColors.some(([value]) => value === saved?.terminal)
      ? saved.terminal
      : "follow",
    font: Object.hasOwn(fonts, saved?.font) ? saved.font : "system",
    size:
      Number.isInteger(saved?.size) && saved.size >= 10 && saved.size <= 28
        ? saved.size
        : 14,
  };
}
