// The browser's Ctrl-B prefix keys. After Ctrl-B, a key names an action as
// in the TUI's default bindings (src/native/input.rs), adapted where the
// browser has its own equivalent. Pure, so node tests can check the map.

export type PrefixAction =
  | "new"
  | "reopen-last"
  | "scroll-mode"
  | "select-mode"
  | "grid"
  | "next"
  | "previous"
  | "focus-left"
  | "focus-right"
  | "focus-up"
  | "focus-down"
  | "next-attention"
  | "next-finished"
  | "agents"
  | "agents-attention"
  | "agents-finished"
  | "notifications"
  | "overview"
  | "recovery"
  | "search"
  | "park"
  | "parked"
  | "move"
  | "close"
  | "settings"
  | "help"
  | "disconnect"
  | "send-prefix"
  | { select: number };

/** The action of the key pressed after the prefix, or null for none. */
export function prefixAction(event: {
  key: string;
  ctrlKey?: boolean;
  shiftKey?: boolean;
}): PrefixAction | null {
  const { key } = event;
  if (event.ctrlKey && key.toLowerCase() === "b") return "send-prefix";
  if (event.ctrlKey) return null;
  if (key === "Tab") return event.shiftKey ? "previous" : "next";
  if (/^[1-9]$/.test(key)) return { select: Number(key) };
  const map: Record<string, PrefixAction> = {
    Enter: "new",
    z: "reopen-last",
    "[": "scroll-mode",
    " ": "select-mode",
    g: "grid",
    e: "grid",
    h: "focus-left",
    ArrowLeft: "focus-left",
    l: "focus-right",
    ArrowRight: "focus-right",
    k: "focus-up",
    ArrowUp: "focus-up",
    j: "focus-down",
    ArrowDown: "focus-down",
    N: "next-attention",
    P: "next-finished",
    a: "agents",
    "!": "agents-attention",
    n: "agents-finished",
    b: "notifications",
    o: "overview",
    r: "recovery",
    "/": "search",
    p: "park",
    u: "parked",
    s: "move",
    x: "close",
    ",": "settings",
    "?": "help",
    d: "disconnect",
  };
  return Object.hasOwn(map, key) ? map[key] : null;
}
