/** Browser shortcuts handled before keys reach the focused terminal. Ctrl-B
 * mirrors the TUI's default prefix for the actions both clients share. */
export const shortcuts: readonly (readonly [string, string])[] = [
  ["Ctrl-B Enter", "New terminal"],
  ["Ctrl-B z", "Toggle one terminal / live grid"],
  ["Ctrl-B g", "Live grid"],
  ["Ctrl-B n / Ctrl-B p", "Next / previous terminal"],
  ["Ctrl-B x", "Close the focused terminal (confirmed)"],
  ["Ctrl-B /", "Find in the focused terminal"],
  ["Ctrl-B ,", "Settings"],
  ["Ctrl-B d", "Disconnect this browser"],
  ["Ctrl-B Ctrl-B", "Send Ctrl-B to the terminal"],
  ["Alt+1 … Alt+0", "Open terminal 1–10"],
  ["Alt+Tab", "Toggle one terminal / live grid"],
  ["Ctrl/Cmd+F", "Find in the focused terminal"],
  [
    "Shift+drag, Shift+wheel",
    "Local selection and history, even in mouse apps",
  ],
];
