// In-app attention toasts: the browser's own notifications of workspace
// events (a question, a finished turn, a failure, an exited agent or a
// bell), shown in the top right corner of the app instead of the browser's
// desktop notifications. One toast per terminal: a new event of a terminal
// that already has a toast updates it. Above TOAST_LIMIT toasts they
// collapse into one summary. A toast goes away when its notifications are
// read anywhere (KelpServer shares read marks with every client), when its
// terminal ends, or when closed. The event names and kinds are the TUI's
// (`notification_events`, src/native/ui.rs).
//
// No runtime imports but model.ts, so node tests can load it directly.
import {
  bellCompanion,
  isBell,
  notificationMatches,
  notificationStatus,
  type Notification,
  type StatePane,
} from "./state/model.ts";

/** The TUI's notification event kinds, in its order. */
export const NOTIFICATION_EVENTS = [
  "question",
  "finished",
  "failed",
  "exited",
  "bell",
] as const;
export type NotificationEvent = (typeof NOTIFICATION_EVENTS)[number];

/** Settings labels, as the TUI's Events row names them. */
export const notificationEventLabel: Record<NotificationEvent, string> = {
  question: "Question",
  finished: "Finished",
  failed: "Failed",
  exited: "Exited",
  bell: "Bell",
};
export const notificationEventHint: Record<NotificationEvent, string> = {
  question: "an agent needs input or asks a question",
  finished: "an agent finishes its turn",
  failed: "an agent fails",
  exited: "an agent exits",
  bell: "an agent rings the terminal bell",
};

/** Which events alert (toast, sound and, for bells, the favicon). */
export type NotificationEvents = Record<NotificationEvent, boolean>;
/** All on, as the TUI's default: KelpServer only records agents' events,
 * and the toast rules (none for the terminal being read, one per terminal,
 * a summary above three) keep them from piling up. */
export const defaultNotificationEvents: NotificationEvents = {
  question: true,
  finished: true,
  failed: true,
  exited: true,
  bell: true,
};
/** Stored events: a missing or invalid value is on, as a missing key in
 * the TUI's `notification_events`. */
export function notificationEvents(value: unknown): NotificationEvents {
  const stored =
    value && typeof value === "object"
      ? (value as Record<string, unknown>)
      : {};
  return Object.fromEntries(
    NOTIFICATION_EVENTS.map((event) => [event, stored[event] !== false]),
  ) as NotificationEvents;
}

/** The event kind of a KelpServer notification status, as the TUI's
 * `NotificationEvent::of`: bells carry the unknown status. */
export function eventOf(status: string): NotificationEvent {
  switch (status) {
    case "input_needed":
      return "question";
    case "turn_finished":
      return "finished";
    case "failed":
      return "failed";
    case "exited":
      return "exited";
    default:
      return "bell";
  }
}

/** A terminal's toast. */
export type TerminalToast = {
  /** The terminal (pane ID) and its process: one toast per terminal. */
  pane: number;
  pid: number;
  /** The notifications it stands for, oldest first. */
  ids: number[];
  /** Its newest event, shown. A bell that accompanied a typed event shows
   * as that event. */
  event: NotificationEvent;
  status: string;
  title: string;
};

/** More toasts than this collapse into one summary. */
export const TOAST_LIMIT = 3;

/** The event a notification alerts as: a bell with its companion's. */
export const alertEvent = (all: Notification[], n: Notification) =>
  eventOf(notificationStatus(all, n));

/** Fresh notifications (oldest first) of enabled events, as the TUI's
 * sound and desktop alerts filter them. */
export const alerting = (
  fresh: Notification[],
  all: Notification[],
  events: NotificationEvents,
) => fresh.filter((n) => events[alertEvent(all, n)]);

/** The toasts after fresh notifications arrive, newest first: each enabled
 * event of a live terminal adds its toast or updates and raises the
 * terminal's existing one, except for `shown`, the terminal being read
 * (shown alone in a visible window; 0 when none is). */
export function addToasts(
  toasts: TerminalToast[],
  fresh: Notification[],
  all: Notification[],
  panes: StatePane[],
  events: NotificationEvents,
  shown: number,
): TerminalToast[] {
  let result = toasts;
  for (const n of [...fresh].sort((a, b) => a.id - b.id)) {
    const pane = panes.find((p) => notificationMatches(n, p));
    if (!pane || pane.id === shown || n.read) continue;
    const event = alertEvent(all, n);
    if (!events[event]) continue;
    const existing = result.find((t) => t.pane === pane.id);
    // A bell shows as the typed event it accompanied.
    const source = bellCompanion(all, n) ?? n;
    const ids = existing && existing.pid === pane.pid ? existing.ids : [];
    result = [
      {
        pane: pane.id,
        pid: pane.pid,
        ids: [...ids.filter((id) => id !== n.id), n.id].slice(-128),
        event,
        status: source.status,
        title: source.title,
      },
      ...result.filter((t) => t.pane !== pane.id),
    ];
  }
  return result;
}

/** The toasts still due: those with an unread notification (in `all`, and
 * not in `acknowledged`, this browser's marks the server has not echoed
 * yet) of their live terminal and process, and of an enabled event. Read
 * anywhere, the terminal ended or its event turned off: gone. */
export function pruneToasts(
  toasts: TerminalToast[],
  all: Notification[],
  panes: StatePane[],
  events: NotificationEvents,
  acknowledged: ReadonlySet<number> = new Set(),
): TerminalToast[] {
  const unread = new Set(
    all.filter((n) => !n.read && !acknowledged.has(n.id)).map((n) => n.id),
  );
  const next = toasts.filter(
    (t) =>
      events[t.event] &&
      t.ids.some((id) => unread.has(id)) &&
      panes.some((p) => p.id === t.pane && p.pid === t.pid && !p.dead),
  );
  return next.length === toasts.length ? toasts : next;
}

/** Whether the toasts collapse into a summary. */
export const summarized = (toasts: TerminalToast[]) =>
  toasts.length > TOAST_LIMIT;

/** The summary's text. */
export const summaryText = (count: number) =>
  `${count} terminal${count === 1 ? " needs" : "s need"} attention`;

/** Whether the favicon shows a ringing bell: an unread bell of a live
 * terminal, while bells are enabled. Stays until the bell is read. */
export const bellRinging = (
  all: Notification[],
  panes: StatePane[],
  events: NotificationEvents,
) =>
  events.bell &&
  all.some(
    (n) =>
      !n.read &&
      (isBell(n) || n.bell === true) &&
      panes.some((p) => notificationMatches(n, p)),
  );
