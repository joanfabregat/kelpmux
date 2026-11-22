// Showing a terminal marks its notifications read, as focusing a pane does
// in the TUI (see readingPane in model.ts for which terminal counts as
// shown). Shared by main.tsx and the sidebar fixture (test/sidebar.mjs).
import { useEffect, useRef, useState } from "react";
import { unreadFor, type Notification, type StatePane } from "./model";

/** Whether someone is looking at this window: visible and focused. */
const looking = () => !document.hidden && document.hasFocus();

/** Marks the shown terminal's unread notifications read: when it is shown,
 * when the window comes back into view or focus, and when one arrives while
 * someone is looking at the window. A notification that arrives while the
 * window is hidden or in the background keeps the terminal's bell until
 * the window is looked at again. Nothing is marked read in a hidden
 * window. */
export function useReadShown(
  pane: StatePane | undefined,
  notifications: Notification[],
  acknowledge: (ids: number[]) => void,
) {
  const [attending, setAttending] = useState(looking);
  useEffect(() => {
    const update = () => setAttending(looking());
    document.addEventListener("visibilitychange", update);
    window.addEventListener("focus", update);
    window.addEventListener("blur", update);
    return () => {
      document.removeEventListener("visibilitychange", update);
      window.removeEventListener("focus", update);
      window.removeEventListener("blur", update);
    };
  }, []);
  const latest = useRef({ pane, notifications, acknowledge });
  latest.current = { pane, notifications, acknowledge };
  useEffect(() => {
    const { pane, notifications, acknowledge } = latest.current;
    if (!pane || document.hidden) return;
    const unread = unreadFor(notifications, pane);
    if (unread.length) acknowledge(unread.map((n) => n.id));
  }, [pane?.id, pane?.pid, attending, attending ? notifications : null]);
}
