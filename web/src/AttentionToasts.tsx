// The attention toasts (toasts.ts) in the top right corner: one per
// terminal, or above TOAST_LIMIT one summary that expands into the list.
// A toast or a summary entry opens its terminal, as choosing it in the
// sidebar does; the close button only hides it (its notifications stay
// unread). Toasts have no timer: they go when read.
import { useCallback, useEffect, useRef, useState } from "react";
import type { NumberedPane } from "@kelpcore/web/types.ts";
import { Icon } from "./status";
import { StateMark } from "./state/StateMark";
import { ProviderLogo } from "./state/ProviderLogo";
import {
  notificationMatches,
  type DisplayState,
  type Notification,
  type StatePane,
} from "./state/model";
import { freshNotifications, newestId } from "./alerts";
import {
  addToasts,
  alerting,
  notificationEventLabel,
  pruneToasts,
  summarized,
  summaryText,
  type NotificationEvent,
  type NotificationEvents,
  type TerminalToast,
} from "./toasts";

const eventState: Record<NotificationEvent, DisplayState> = {
  question: "input_needed",
  bell: "input_needed",
  failed: "failed",
  finished: "finished",
  exited: "idle",
};

/** A terminal's name and the event, as its toast and summary entry show. */
function ToastLabel({
  toast,
  pane,
}: {
  toast: TerminalToast;
  pane: NumberedPane;
}) {
  return (
    <>
      {toast.event === "bell" ? (
        <Icon name="bell-ring" size="sm" className="toast-icon" />
      ) : (
        <StateMark state={eventState[toast.event]} />
      )}
      <span className="attention-text">
        <span className="attention-terminal">
          {pane.number > 0 && <strong>{pane.number}</strong>}
          <ProviderLogo kind={pane.kind} />
          <span className="attention-name">{pane.title || "shell"}</span>
        </span>
        <span className="attention-event">{toast.title}</span>
      </span>
    </>
  );
}

export function AttentionToasts({
  toasts,
  panes,
  onOpen,
  onDismiss,
}: {
  toasts: TerminalToast[];
  panes: NumberedPane[];
  onOpen: (pane: NumberedPane) => void;
  /** Hides these terminals' toasts. */
  onDismiss: (panes: number[]) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const shown = toasts
    .map((toast) => ({ toast, pane: panes.find((p) => p.id === toast.pane) }))
    .filter(
      (entry): entry is { toast: TerminalToast; pane: NumberedPane } =>
        !!entry.pane,
    );
  const summary = summarized(shown.map((entry) => entry.toast));
  useEffect(() => {
    if (!summary) setExpanded(false);
  }, [summary]);
  const tone = (toast: TerminalToast) => `attention-${toast.event}`;
  return (
    <div
      className="attention-toasts"
      role="region"
      aria-label="Terminal notifications"
      aria-live="polite"
    >
      {summary ? (
        <div
          className={`toast attention-toast attention-summary ${tone(shown[0].toast)}`}
        >
          <div className="attention-row">
            <button
              type="button"
              className="attention-open"
              aria-expanded={expanded}
              onClick={() => setExpanded(!expanded)}
            >
              <Icon name="bell-ring" size="sm" className="toast-icon" />
              <span className="attention-text">
                <span className="attention-name">
                  {summaryText(shown.length)}
                </span>
              </span>
              <Icon
                name="chevron-down"
                size="sm"
                className="attention-chevron"
              />
            </button>
            <button
              type="button"
              className="icon-btn"
              title="Dismiss all"
              aria-label="Dismiss all terminal notifications"
              onPointerDown={(event) => event.preventDefault()}
              onClick={() => onDismiss(shown.map(({ toast }) => toast.pane))}
            >
              <Icon name="x" size="sm" />
            </button>
          </div>
          {expanded && (
            <ul className="attention-list">
              {shown.map(({ toast, pane }) => (
                <li key={toast.pane} className={tone(toast)}>
                  <button
                    type="button"
                    className="attention-open"
                    data-pane={toast.pane}
                    title={`Open ${pane.title || "shell"} (${notificationEventLabel[toast.event]})`}
                    onClick={() => onOpen(pane)}
                  >
                    <ToastLabel toast={toast} pane={pane} />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : (
        shown.map(({ toast, pane }) => (
          <div
            key={toast.pane}
            className={`toast attention-toast ${tone(toast)}`}
            data-pane={toast.pane}
          >
            <div className="attention-row">
              <button
                type="button"
                className="attention-open"
                title={`Open ${pane.title || "shell"}`}
                onClick={() => onOpen(pane)}
              >
                <ToastLabel toast={toast} pane={pane} />
              </button>
              <button
                type="button"
                className="icon-btn"
                title="Dismiss"
                aria-label={`Dismiss the notification of ${pane.title || "shell"}`}
                onPointerDown={(event) => event.preventDefault()}
                onClick={() => onDismiss([toast.pane])}
              >
                <Icon name="x" size="sm" />
              </button>
            </div>
          </div>
        ))
      )}
    </div>
  );
}

/** This window's toasts, from the workspace notifications: fresh ones (never
 * those already there when `ready` became true) add or update toasts, and
 * toasts whose notifications were read anywhere, whose terminal ended or
 * whose event was turned off go. `shown` is the terminal shown alone; it
 * gets no toast while the window is visible. `onAlert` receives the fresh
 * notifications of enabled events (for the sound). Not `ready` (signed
 * out, still loading, or a popup window, which leaves alerts to the main
 * window): no toasts. */
export function useAttentionToasts({
  ready,
  notifications,
  panes,
  events,
  shown,
  onAlert,
}: {
  ready: boolean;
  notifications: Notification[];
  panes: StatePane[];
  events: NotificationEvents;
  shown: number;
  onAlert?: (fresh: Notification[]) => void;
}) {
  const [toasts, setToasts] = useState<TerminalToast[]>([]);
  /** Newest notification already considered (-1: none yet). */
  const seen = useRef(-1);
  /** This window's acknowledgements the server has not echoed yet. */
  const read = useRef(new Set<number>());
  const latest = useRef({ shown, onAlert });
  latest.current = { shown, onAlert };
  useEffect(() => {
    if (!ready) {
      seen.current = -1;
      read.current.clear();
      setToasts([]);
      return;
    }
    if (seen.current < 0) {
      seen.current = newestId(notifications);
      return;
    }
    const fresh = freshNotifications(notifications, seen.current).filter((n) =>
      panes.some((p) => notificationMatches(n, p)),
    );
    seen.current = newestId(notifications, seen.current);
    const shownNow = document.hidden ? 0 : latest.current.shown;
    setToasts((current) =>
      pruneToasts(
        addToasts(current, fresh, notifications, panes, events, shownNow),
        notifications,
        panes,
        events,
        read.current,
      ),
    );
    const alerts = alerting(fresh, notifications, events);
    if (alerts.length) latest.current.onAlert?.(alerts);
  }, [ready, notifications, panes, events]);
  /** Hides these terminals' toasts. */
  const dismiss = useCallback(
    (ids: number[]) =>
      setToasts((current) => current.filter((t) => !ids.includes(t.pane))),
    [],
  );
  /** Notifications this window marked read: their toasts go at once. */
  const markRead = useCallback(
    (ids: number[]) => {
      for (const id of ids) read.current.add(id);
      setToasts((current) =>
        pruneToasts(current, notifications, panes, events, read.current),
      );
    },
    [notifications, panes, events],
  );
  return { toasts, dismiss, markRead };
}
