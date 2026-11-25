// The editor's CodeMirror theme. Derived from KelpHub's workspace editor
// (web/src/files/theme.ts), without the Git diff styles.
//
// It uses only CSS custom properties, which the application defines, so
// the app's appearance (light or dark, any theme) restyles the editor
// without code changes:
// - interface: --bg, --text, --text-2, --text-3, --surface-1, --surface-2,
//   --surface-3, --line, --line-strong, --accent, --accent-soft;
// - status colours: --st-failed, --st-input; --git-added, --git-deleted and
//   --git-modified for inserted, deleted and changed text;
// - spacing and type: --s1, --s2, --s3, --s5, --fs-sm, --r-sm, --font-mono;
// - the terminal's font and palette: --terminal-font, --terminal-size
//   (a number of pixels), --terminal-selection and --terminal-ansi-0 to
//   --terminal-ansi-15, which syntax colours use.
import { EditorView } from "@codemirror/view";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";

const ansi = (n: number) => `var(--terminal-ansi-${n})`;
const mix = (color: string, percent: number, base = "transparent") =>
  `color-mix(in srgb, ${color} ${percent}%, ${base})`;

export const editorTheme = EditorView.theme({
  "&": {
    height: "100%",
    color: "var(--text)",
    backgroundColor: "var(--bg)",
    fontSize: "calc(var(--terminal-size, 13) * 1px)",
  },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": {
    fontFamily: "var(--terminal-font, var(--font-mono))",
    lineHeight: "1.5",
  },
  ".cm-content": { caretColor: "var(--accent)" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--accent)" },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection":
    {
      backgroundColor:
        "var(--terminal-selection, var(--accent-soft)) !important",
    },
  ".cm-activeLine": { backgroundColor: mix("var(--surface-3)", 35) },
  ".cm-gutters": {
    backgroundColor: "var(--surface-1)",
    color: "var(--text-3)",
    borderRight: "1px solid var(--line)",
  },
  ".cm-activeLineGutter": {
    backgroundColor: "var(--surface-2)",
    color: "var(--text-2)",
  },
  ".cm-foldPlaceholder": {
    backgroundColor: "var(--surface-3)",
    border: "1px solid var(--line-strong)",
    color: "var(--text-2)",
  },
  "&.cm-focused .cm-matchingBracket": {
    backgroundColor: mix("var(--accent)", 25),
    outline: `1px solid ${mix("var(--accent)", 55)}`,
  },
  "&.cm-focused .cm-nonmatchingBracket": {
    backgroundColor: mix("var(--st-failed)", 25),
  },
  ".cm-selectionMatch": { backgroundColor: mix("var(--accent)", 14) },
  ".cm-searchMatch": {
    backgroundColor: mix("var(--st-input)", 25),
    outline: `1px solid ${mix("var(--st-input)", 50)}`,
  },
  ".cm-searchMatch.cm-searchMatch-selected": {
    backgroundColor: mix("var(--st-input)", 50),
  },
  ".cm-panels": {
    backgroundColor: "var(--surface-1)",
    color: "var(--text)",
  },
  ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--line)" },
  ".cm-panels.cm-panels-bottom": { borderTop: "1px solid var(--line)" },
  ".cm-panel.cm-search": {
    display: "flex",
    flexWrap: "wrap",
    alignItems: "center",
    gap: "var(--s1) var(--s2)",
    padding: "var(--s2) var(--s5) var(--s2) var(--s3)",
    fontSize: "var(--fs-sm)",
  },
  ".cm-panel.cm-search label": {
    display: "inline-flex",
    alignItems: "center",
    gap: "4px",
    color: "var(--text-2)",
    fontSize: "var(--fs-sm)",
  },
  ".cm-panel.cm-search br": { display: "none" },
  ".cm-panel.cm-search [name=close]": {
    top: "6px",
    right: "6px",
    color: "var(--text-2)",
    fontSize: "18px",
  },
  ".cm-textfield": {
    minHeight: "26px",
    padding: "2px var(--s2)",
    border: "1px solid var(--line-strong)",
    borderRadius: "var(--r-sm)",
    backgroundColor: "var(--bg)",
    color: "var(--text)",
    fontSize: "var(--fs-sm)",
  },
  ".cm-button": {
    minHeight: "26px",
    margin: "0",
    padding: "0 var(--s2)",
    border: "1px solid var(--line-strong)",
    borderRadius: "var(--r-sm)",
    backgroundImage: "none",
    backgroundColor: "var(--surface-2)",
    color: "var(--text)",
    fontSize: "var(--fs-sm)",
    fontWeight: "500",
  },
  ".cm-tooltip": {
    border: "1px solid var(--line-strong)",
    backgroundColor: "var(--surface-2)",
    color: "var(--text)",
  },
});

const highlight = HighlightStyle.define([
  {
    tag: [t.keyword, t.controlKeyword, t.moduleKeyword, t.operatorKeyword],
    color: ansi(5),
  },
  { tag: [t.definitionKeyword, t.modifier], color: ansi(5) },
  {
    tag: [t.string, t.special(t.string), t.character, t.attributeValue],
    color: ansi(2),
  },
  { tag: [t.regexp, t.escape], color: ansi(6) },
  { tag: [t.number, t.bool, t.null, t.atom, t.unit], color: ansi(3) },
  { tag: t.comment, color: "var(--text-3)", fontStyle: "italic" },
  {
    tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName],
    color: ansi(4),
  },
  {
    tag: [t.typeName, t.className, t.namespace, t.standard(t.typeName)],
    color: ansi(6),
  },
  { tag: [t.propertyName, t.labelName], color: ansi(12) },
  { tag: [t.self, t.special(t.variableName)], color: ansi(1) },
  { tag: t.tagName, color: ansi(1) },
  { tag: t.attributeName, color: ansi(3) },
  {
    tag: [t.meta, t.processingInstruction, t.annotation],
    color: "var(--text-2)",
  },
  { tag: [t.operator, t.punctuation, t.separator], color: "var(--text-2)" },
  { tag: t.heading, color: ansi(4), fontWeight: "bold" },
  { tag: t.strong, fontWeight: "bold" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.strikethrough, textDecoration: "line-through" },
  { tag: [t.link, t.url], color: ansi(6), textDecoration: "underline" },
  { tag: t.monospace, color: ansi(2) },
  { tag: t.quote, color: "var(--text-2)", fontStyle: "italic" },
  { tag: [t.inserted], color: "var(--git-added)" },
  { tag: [t.deleted], color: "var(--git-deleted)" },
  { tag: [t.changed], color: "var(--git-modified)" },
  { tag: t.invalid, color: ansi(1) },
]);

export const editorHighlight = syntaxHighlighting(highlight);
