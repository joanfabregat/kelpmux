// On-screen key bar and input mode for touch devices (phones and tablets).
// Pure, so node tests can check the encoding; nothing here imports
// application code.
import { encodeKey, encodeText, type Key } from "./keyboard.ts";

/** Keys of the bar shown above the on-screen keyboard. `Prefix` is the
 * application's Ctrl-B, `Ctrl` a sticky modifier for the next key, `Yes` and
 * `No` answer an agent's question, `Hide` hides the on-screen keyboard. */
export type BarKey =
  | "Escape"
  | "Tab"
  | "Ctrl"
  | "ArrowLeft"
  | "ArrowUp"
  | "ArrowDown"
  | "ArrowRight"
  | "Prefix"
  | "Paste"
  | "Enter"
  | "Yes"
  | "No"
  | "Hide";

/** The bar's keys in display order with their visible labels. */
export const barKeys: { key: BarKey; label: string; title: string }[] = [
  { key: "Escape", label: "Esc", title: "Escape" },
  { key: "Tab", label: "Tab", title: "Tab" },
  { key: "Ctrl", label: "Ctrl", title: "Ctrl: applies to the next key" },
  { key: "ArrowLeft", label: "←", title: "Left arrow" },
  { key: "ArrowUp", label: "↑", title: "Up arrow" },
  { key: "ArrowDown", label: "↓", title: "Down arrow" },
  { key: "ArrowRight", label: "→", title: "Right arrow" },
  { key: "Prefix", label: "^B", title: "KelpMux prefix (Ctrl-B)" },
  { key: "Paste", label: "Paste", title: "Paste" },
  { key: "Enter", label: "⏎", title: "Enter" },
  { key: "Yes", label: "Yes", title: "Answer yes" },
  { key: "No", label: "No", title: "Answer no" },
];

/** Keys sent as a terminal key, which the sticky Ctrl modifies. */
const terminalKeys = new Set<BarKey>([
  "Escape",
  "Tab",
  "ArrowLeft",
  "ArrowUp",
  "ArrowDown",
  "ArrowRight",
  "Enter",
]);

/** What an agent expects for yes or no. Claude Code's questions are numbered
 * menus whose first entry is yes and which Escape declines; Codex and plain
 * prompts (`[y/N]`) take `y` and `n`. Nothing adds Enter: a line prompt
 * still needs the Enter key. */
export function answerText(
  kind: string | null | undefined,
  answer: "yes" | "no",
): string {
  if (kind === "claude") return answer === "yes" ? "1" : "\x1b";
  return answer === "yes" ? "y" : "n";
}

/** The bytes a bar key sends to the terminal, or null when the key is an
 * application action (Ctrl, Prefix, Paste, Hide). `ctrl` is the sticky
 * modifier's state; `modes` the terminal's negotiated keyboard modes. */
export function barKeyBytes(
  key: BarKey,
  ctrl: boolean,
  modes: Record<string, boolean | number>,
  kind?: string | null,
): string | null {
  if (key === "Yes" || key === "No")
    return encodeText(answerText(kind, key === "Yes" ? "yes" : "no"), modes);
  if (!terminalKeys.has(key)) return null;
  return encodeKey({ key, ctrlKey: ctrl }, modes);
}

/** Whether pressing `key` uses up an armed sticky Ctrl. */
export function consumesCtrl(key: BarKey): boolean {
  return terminalKeys.has(key);
}

/** A key from the on-screen keyboard with the sticky Ctrl applied. A key
 * already carrying Ctrl or Cmd stays as it is. */
export function withCtrl(event: Key): Key {
  if (event.ctrlKey || event.metaKey) return event;
  return { ...event, ctrlKey: true };
}

/** Text typed on the on-screen keyboard (an `input` event without a
 * preceding key event) with the sticky Ctrl applied to its first character.
 * Returns the bytes to send. */
export function stickyText(
  text: string,
  modes: Record<string, boolean | number>,
): string {
  const [first, ...rest] = Array.from(text);
  if (first === undefined) return "";
  const control = encodeKey({ key: first, ctrlKey: true }, modes);
  return (
    (control ?? encodeText(first, modes)) + encodeText(rest.join(""), modes)
  );
}

/** Reading output is the default on touch devices; the on-screen keyboard
 * appears only in input mode, entered explicitly. */
export type InputMode = "reading" | "input";
export type InputModeEvent =
  /** A terminal was opened or switched to. */
  | "open-terminal"
  /** The Input control was tapped. */
  | "input-control"
  /** A key bar key was tapped. */
  | "key-bar"
  /** The hide-keyboard button was tapped. */
  | "hide"
  /** The terminal's input lost focus (the system dismissed the keyboard). */
  | "blur"
  /** The terminal's input gained focus. */
  | "focus";

export function nextInputMode(
  mode: InputMode,
  event: InputModeEvent,
): InputMode {
  switch (event) {
    case "open-terminal":
    case "hide":
    case "blur":
      return "reading";
    case "input-control":
      return mode === "input" ? "reading" : "input";
    case "key-bar":
    case "focus":
      return "input";
  }
}
