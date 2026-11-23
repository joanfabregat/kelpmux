// Attention alerts of this browser: an optional sound when a workspace
// notification arrives (a question, a finished turn, a failure, an exited
// agent or a bell), the browser equivalent of the TUI's notification
// sound. In-app toasts (toasts.ts) replace desktop notifications.
// Preferences are this browser's own.
import type { Notification } from "./state/model";

/** Unread notifications newer than `after`, oldest first. */
export const freshNotifications = (
  notifications: Notification[],
  after: number,
) =>
  notifications
    .filter((n) => !n.read && n.id > after)
    .sort((a, b) => a.id - b.id);

/** The newest notification ID, so earlier ones never alert again. */
export const newestId = (notifications: Notification[], previous = 0) =>
  notifications.reduce((max, n) => Math.max(max, n.id), previous);

let audio: AudioContext | null = null;
/** A short two-tone chime. Browsers allow it after a user gesture. */
export function chime() {
  try {
    audio ||= new AudioContext();
    const start = audio.currentTime;
    for (const [offset, frequency] of [
      [0, 880],
      [0.12, 1320],
    ]) {
      const oscillator = audio.createOscillator();
      const gain = audio.createGain();
      oscillator.frequency.value = frequency;
      gain.gain.setValueAtTime(0.0001, start + offset);
      gain.gain.exponentialRampToValueAtTime(0.08, start + offset + 0.02);
      gain.gain.exponentialRampToValueAtTime(0.0001, start + offset + 0.18);
      oscillator.connect(gain).connect(audio.destination);
      oscillator.start(start + offset);
      oscillator.stop(start + offset + 0.2);
    }
  } catch {
    // Audio unavailable: alerts stay visual.
  }
}
