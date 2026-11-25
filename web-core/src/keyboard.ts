// Browser event adapter for VT keys and the negotiated kitty enhancements.
// https://sw.kovidgoyal.net/kitty/keyboard-protocol/
export type Key = {
  key: string;
  ctrlKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
  metaKey?: boolean;
  repeat?: boolean;
};
export function encodeKey(
  event: Key,
  modes: Record<string, boolean | number>,
  release = false,
): string | null {
  const flags = Number(modes.keyboard_flags || 0),
    all = !!(flags & 8),
    events = !!(flags & 2),
    enhanced = !!(flags & 1) || all;
  const mod =
    1 +
    (event.shiftKey ? 1 : 0) +
    (event.altKey ? 2 : 0) +
    (event.ctrlKey ? 4 : 0) +
    (event.metaKey ? 8 : 0);
  const eventType = release ? 3 : event.repeat ? 2 : 1;
  if (release && !events) return null;
  // Interrupt/quit controls take effect on press. The application can restore
  // the shell before its updated modes reach the browser; a stale CSI-u
  // release would then be inserted into the shell. Full-keyboard applications
  // explicitly requesting every key still receive these releases.
  if (
    release &&
    !all &&
    (event.key === "Escape" ||
      (event.ctrlKey && ["c", "d"].includes(event.key.toLowerCase())))
  )
    return null;
  const suffix = events && eventType !== 1 ? `:${eventType}` : "";
  const parameters = `${mod}${suffix}`;
  const arrows: Record<string, string> = {
    ArrowUp: "A",
    ArrowDown: "B",
    ArrowRight: "C",
    ArrowLeft: "D",
    Home: "H",
    End: "F",
  };
  if (arrows[event.key]) {
    if (release && !enhanced) return null;
    if (mod > 1 || all || events)
      return `\x1b[1;${parameters}${arrows[event.key]}`;
    return `\x1b${modes.application_cursor && !enhanced ? "O" : "["}${arrows[event.key]}`;
  }
  const tilde: Record<string, number> = {
    Insert: 2,
    Delete: 3,
    PageUp: 5,
    PageDown: 6,
    F5: 15,
    F6: 17,
    F7: 18,
    F8: 19,
    F9: 20,
    F10: 21,
    F11: 23,
    F12: 24,
  };
  if (tilde[event.key]) {
    if (release && !enhanced) return null;
    return `\x1b[${tilde[event.key]}${mod > 1 || all || events ? ";" + parameters : ""}~`;
  }
  if (/^F[1-4]$/.test(event.key)) {
    if (release && !enhanced) return null;
    const final = String.fromCharCode(79 + Number(event.key.slice(1)));
    return mod > 1 || all || events
      ? `\x1b[1;${parameters}${final}`
      : `\x1bO${final}`;
  }
  const special: Record<string, number> = {
    Enter: 13,
    Backspace: 127,
    Tab: 9,
    Escape: 27,
  };
  if (special[event.key]) {
    const enhancedKey = enhanced && (all || event.key === "Escape" || mod > 1);
    if (release && (!enhancedKey || !events)) return null;
    if (enhancedKey) return `\x1b[${special[event.key]};${parameters}u`;
    if (release) return null;
    const legacy: Record<string, string> = {
      Enter: "\r",
      Backspace: event.ctrlKey ? "\x08" : "\x7f",
      Tab: event.shiftKey ? "\x1b[Z" : "\t",
      Escape: "\x1b",
    };
    return (event.altKey ? "\x1b" : "") + legacy[event.key];
  }
  if (Array.from(event.key).length !== 1) return null;
  if (release && !all && !event.ctrlKey && !event.altKey && !event.metaKey)
    return null;
  if (enhanced && (all || event.ctrlKey || event.altKey || event.metaKey)) {
    const base = event.key.toLowerCase();
    const code =
      Array.from(base).length === 1
        ? base.codePointAt(0)!
        : event.key.codePointAt(0)!;
    const associated =
      all &&
      flags & 16 &&
      !event.ctrlKey &&
      !event.altKey &&
      !event.metaKey &&
      !release
        ? ";" +
          Array.from(event.key)
            .map((c) => c.codePointAt(0))
            .join(":")
        : "";
    return `\x1b[${code};${parameters}${associated}u`;
  }
  if (release) return null;
  if (event.ctrlKey && !event.metaKey) {
    const c = event.key.toUpperCase().charCodeAt(0);
    if (c >= 64 && c <= 95) return String.fromCharCode(c - 64);
    if (event.key === " ") return "\0";
  }
  if (event.altKey && !event.metaKey) return "\x1b" + event.key;
  return null; // Text input and IME are received through the textarea's input event.
}
export function encodeText(
  text: string,
  modes: Record<string, boolean | number>,
): string {
  const flags = Number(modes.keyboard_flags || 0);
  if (!(flags & 8)) return text;
  return Array.from(text)
    .map(
      (c) =>
        `\x1b[${c.toLowerCase().codePointAt(0)};1${flags & 16 ? ";" + c.codePointAt(0) : ""}u`,
    )
    .join("");
}
