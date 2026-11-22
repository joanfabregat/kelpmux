// Terminal and agent state as the TUI shows it, derived in the browser from
// the live stream: KelpServer pane statuses, agent kinds and the workspace's
// notifications. Semantics follow the TUI (crates/server/src/native/ui.rs and
// workspace.rs): a pane's indicator, attention and finished filters, next
// question / next completion navigation and notification matching. The marks
// and their shapes come from KelpHub's terminal UI.
//
// This file has no runtime imports so node tests can load it directly.

/** Display states, most urgent first. */
export const DISPLAY_STATES = [
  "input_needed",
  "failed",
  "finished",
  "working",
  "running",
  "idle",
] as const;
export type DisplayState = (typeof DISPLAY_STATES)[number];

export const stateLabel: Record<DisplayState, string> = {
  input_needed: "Needs input",
  failed: "Failed",
  finished: "Finished",
  working: "Working",
  running: "Running",
  idle: "Idle",
};

/** KelpServer status names, as the TUI labels them. */
export const statusText: Record<string, string> = {
  working: "Working",
  waiting_background: "Waiting in background",
  input_needed: "Input needed",
  turn_finished: "Turn finished",
  idle: "Idle",
  failed: "Failed",
  exited: "Exited",
  unknown: "Unknown",
};

/** The pane fields this module reads (a subset of KelpServer PaneInfo). */
export type StatePane = {
  id: number;
  number: number;
  pid: number;
  status: string;
  parked?: boolean;
  dead?: boolean;
  kind?: string | null;
};

/** A KelpServer notification (the stream's latest 128 records). */
export type Notification = {
  id: number;
  pane: number;
  title: string;
  status: string;
  at: number;
  read: boolean;
  workspace: string;
  origin?: { run: string; pid: number } | null;
};

/** Whether KelpServer identifies the pane's program as a coding agent. */
export const isAgent = (pane: { kind?: string | null }) => !!pane.kind;

/** Needs attention: a question or a failure (the TUI's `!` filter and
 * next-question target). */
export const needsAttention = (pane: Pick<StatePane, "status">) =>
  pane.status === "input_needed" || pane.status === "failed";

/** Finished: a completed turn or an exited program (the TUI's finished
 * filter and next-completion target). */
export const isFinished = (pane: Pick<StatePane, "status">) =>
  pane.status === "turn_finished" || pane.status === "exited";

/** Whether a notification belongs to this live pane: same pane, same
 * process. Notifications of earlier processes in a recycled pane do not
 * match, like the TUI's run-and-process check. */
export function notificationMatches(n: Notification, pane: StatePane) {
  return (
    !!n.origin && n.origin.pid === pane.pid && n.pane === pane.id && !pane.dead
  );
}

const isBell = (n: Notification) =>
  n.status === "unknown" && n.title.startsWith("Bell · ");

/** A bell's status: the typed notification it accompanied (within two
 * seconds, same pane and process), else unknown. */
export function notificationStatus(
  notifications: Notification[],
  n: Notification,
): string {
  if (!isBell(n)) return n.status;
  const typed = [...notifications]
    .sort((a, b) => b.id - a.id)
    .find(
      (o) =>
        o.status !== "unknown" &&
        o.pane === n.pane &&
        o.workspace === n.workspace &&
        o.origin?.pid === n.origin?.pid &&
        o.origin?.run === n.origin?.run &&
        Math.abs(o.at - n.at) <= 2,
    );
  return typed ? typed.status : n.status;
}

/** A KelpServer status as a display state (unknown and bells are alerts,
 * shown with the needs-input mark). */
export function statusState(status: string): DisplayState {
  switch (status) {
    case "input_needed":
    case "unknown":
      return "input_needed";
    case "failed":
      return "failed";
    case "turn_finished":
      return "finished";
    case "working":
    case "waiting_background":
      return "working";
    default:
      return "idle";
  }
}

/** Unread notifications of a live pane, newest first. */
export const unreadFor = (notifications: Notification[], pane: StatePane) =>
  notifications
    .filter((n) => !n.read && notificationMatches(n, pane))
    .sort((a, b) => b.id - a.id);

/** A pane's mark, as the TUI's indicator: its live attention status when
 * it has one; a working agent spins and a shell running a command shows a
 * neutral mark; an idle pane shows its newest unread notification (a typed
 * one before a bell); otherwise idle. */
export function paneState(
  pane: StatePane,
  notifications: Notification[] = [],
): DisplayState {
  if (pane.dead) return "idle";
  switch (pane.status) {
    case "input_needed":
      return "input_needed";
    case "failed":
      return "failed";
    case "turn_finished":
      return "finished";
    case "waiting_background":
      return "working";
    case "working":
      return isAgent(pane) ? "working" : "running";
  }
  const unread = unreadFor(notifications, pane);
  const typed = unread.find((n) => n.status !== "unknown");
  if (typed) return statusState(typed.status);
  if (unread.length) return "input_needed";
  return "idle";
}

/** Whether a pane's state counts in roll-ups and the browser tab: every
 * agent state except idle, and any unread notification of a shell. */
export const counted = (pane: StatePane, state: DisplayState) =>
  state !== "idle" &&
  state !== "running" &&
  (isAgent(pane) || state !== "working");

export type Rollup = {
  state: DisplayState | null;
  counts: Partial<Record<DisplayState, number>>;
  total: number;
};
export const emptyRollup = (): Rollup => ({
  state: null,
  counts: {},
  total: 0,
});
const rank = (state: DisplayState) => DISPLAY_STATES.indexOf(state);
export function addToRollup(rollup: Rollup, state: DisplayState) {
  rollup.counts[state] = (rollup.counts[state] || 0) + 1;
  rollup.total += 1;
  if (rollup.state === null || rank(state) < rank(rollup.state))
    rollup.state = state;
  return rollup;
}
/** Counted states of these panes. */
export function rollup(panes: StatePane[], notifications: Notification[]) {
  const result = emptyRollup();
  for (const pane of panes) {
    const state = paneState(pane, notifications);
    if (counted(pane, state)) addToRollup(result, state);
  }
  return result;
}
/** Counts in urgency order, omitting states without terminals. */
export const rollupEntries = (r: Rollup) =>
  DISPLAY_STATES.filter((s) => (r.counts[s] || 0) > 0).map(
    (s) => [s, r.counts[s] as number] as const,
  );
export function rollupSummary(r: Rollup) {
  if (!r.total) return "Nothing needs attention";
  return rollupEntries(r)
    .map(([state, n]) =>
      state === "input_needed"
        ? `${n} ${n === 1 ? "needs" : "need"} input`
        : `${n} ${stateLabel[state].toLowerCase()}`,
    )
    .join(", ");
}

/** The next pane after `active` (cyclically) matching
 * `predicate`, or null: the TUI's next question / next completion. */
export function nextPane<T extends StatePane>(
  panes: T[],
  active: number,
  predicate: (pane: T) => boolean,
): T | null {
  const list = panes;
  if (!list.length) return null;
  const start = Math.max(
    0,
    list.findIndex((p) => p.id === active),
  );
  for (let offset = 1; offset <= list.length; offset++) {
    const pane = list[(start + offset) % list.length];
    if (predicate(pane)) return pane;
  }
  return null;
}

/** The terminal whose notifications showing it marks read, as focusing a
 * pane does in the TUI: a popup's terminal, or the focused terminal when it
 * is on screen (shown on its own, or the focused tile of the grid page;
 * the other tiles keep their bells). 0 when none is, such as on the
 * phone's terminal list. `onScreen` lists the terminals on screen. */
export function readingPane(
  popup: number,
  focused: number,
  onScreen: readonly number[],
) {
  if (popup) return popup;
  return focused && onScreen.includes(focused) ? focused : 0;
}

/** A terminal's notifications, newest first: its unread ones, or with
 * `history` every retained one of its current process (the panel its
 * header's bell opens). The panel opens on its history. */
export function paneNotifications(
  notifications: Notification[],
  pane: StatePane,
  history: boolean,
) {
  return [...notifications]
    .sort((a, b) => b.id - a.id)
    .filter((n) => (history || !n.read) && notificationMatches(n, pane));
}

/** Agent provider of a pane kind, or null for a plain terminal. */
export type Provider = "claude" | "codex";
export const providerOf = (kind?: string | null): Provider | null =>
  kind === "claude" || kind === "codex" ? kind : null;
export const providerLabel: Record<Provider, string> = {
  claude: "Claude Code",
  codex: "Codex",
};

/** Browser tab title: "(3) KelpMux" while 3 terminals need attention. */
export const tabTitle = (count: number, base = "KelpMux") =>
  count > 0 ? `(${count > 99 ? "99+" : count}) ${base}` : base;
