import type { NumberedPane } from "@kelpcore/web/types.ts";
import { StateMark } from "./state/StateMark";
import { ProviderLogo } from "./state/ProviderLogo";
import { stateLabel, type DisplayState } from "./state/model";
import { Icon } from "./status";

/** One terminal at a time, all of them live in a grid, the file browser
 * (and the files open from it), a project's Repos view or its home page. */
export type SessionMode = "single" | "grid" | "files" | "repos" | "project";

/** Number, state, provider and title of a pane, shared by the grid tiles
 * and the focused terminal's header. */
export function PaneLabel({
  pane,
  state,
  parked = false,
}: {
  pane: NumberedPane;
  state: DisplayState;
  parked?: boolean;
}) {
  return (
    <>
      <StateMark state={state} />
      {pane.number > 0 && <strong className="number">{pane.number}</strong>}
      <ProviderLogo kind={pane.kind} />
      <span className="grid-title">{pane.title || "shell"}</span>
      {parked && <span className="badge parked-badge">parked</span>}
      {state !== "idle" && state !== "running" && (
        <span className={`badge state-badge state-${state}`}>
          {stateLabel[state]}
        </span>
      )}
    </>
  );
}

const unreadText = (count: number) =>
  `${count} unread notification${count === 1 ? "" : "s"}`;

/** A terminal's unread notifications, on its sidebar tab and grid tile: a
 * ringing bell, with the count when there are several. Nothing when none. */
export function UnreadMark({ count }: { count: number }) {
  if (count <= 0) return null;
  return (
    <span
      className="unread-mark"
      role="img"
      aria-label={unreadText(count)}
      title={unreadText(count)}
    >
      <Icon name="bell-ring" size="sm" />
      {count > 1 && <b aria-hidden="true">{count > 99 ? "99+" : count}</b>}
    </span>
  );
}

/** The bell in a terminal's header: opens that terminal's notifications,
 * mainly their history (Ctrl-B b), marked while it has unread ones. */
export function NotificationsButton({
  label,
  count,
  onOpen,
}: {
  /** The terminal's name. */
  label: string;
  count: number;
  onOpen: () => void;
}) {
  const text = `Notifications of ${label}${count ? `: ${unreadText(count)}` : ""}`;
  return (
    <button
      type="button"
      className={`icon-btn pane-notifications ${count ? "unread" : ""}`}
      title={`${count ? unreadText(count) : "Notifications"} (Ctrl-B b)`}
      aria-label={text}
      data-unread={count || undefined}
      onClick={(event) => {
        event.stopPropagation();
        onOpen();
      }}
    >
      <Icon name={count ? "bell-ring" : "bell"} size="sm" />
      {count > 0 && (
        <b className="pane-notifications-count" aria-hidden="true">
          {count > 99 ? "99+" : count}
        </b>
      )}
    </button>
  );
}
