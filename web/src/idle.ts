// The idle lock as windows see it (decisions 163–165). The shared worker
// (terminal/session-worker.ts) keeps the clock: after the warning duration
// without activity in any KelpMux window it asks every window to warn, and
// after the lock duration more it locks the connection. Windows report what
// counts as activity here: keys, clicks and taps, scrolling. Terminal output
// and mere pointer movement do not count.
import type { Connection } from "@kelpcore/web/types.ts";

/** Default durations, when the relay's config.json sets none. */
export const IDLE_WARNING_MINUTES = 60;
export const IDLE_LOCK_MINUTES = 15;
/** At most one activity report per window in this interval, unless a
 * warning is shown (then the first activity lifts it at once). */
export const ACTIVITY_REPORT_MS = 30 * 1000;
/** Pointer movement is deliberately absent. */
export const ACTIVITY_EVENTS = [
  "keydown",
  "pointerdown",
  "wheel",
  "touchstart",
] as const;

export function idleMinutes(connection: Connection | null | undefined) {
  return {
    warning: connection?.idle_warning_minutes ?? IDLE_WARNING_MINUTES,
    lock: connection?.idle_lock_minutes ?? IDLE_LOCK_MINUTES,
  };
}

export const minutesText = (n: number) => `${n} minute${n === 1 ? "" : "s"}`;

/** Throttles activity reports: the first is sent, later ones only after
 * `ACTIVITY_REPORT_MS`, or at once when `urgent` (a warning is shown). */
export function activityReporter(
  send: () => void,
  now: () => number = Date.now,
) {
  let last = -Infinity;
  return (urgent = false) => {
    const t = now();
    if (!urgent && t - last < ACTIVITY_REPORT_MS) return false;
    last = t;
    send();
    return true;
  };
}

/** Time left as mm:ss, rounded up and never negative. */
export function countdown(ms: number) {
  const s = Math.max(0, Math.ceil(ms / 1000));
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(Math.floor(s / 60))}:${pad(s % 60)}`;
}
