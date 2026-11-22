// Workspace dialogs, the web equivalents of the TUI's: a terminal's
// notifications, recently closed (recovery), search, parked terminals, move
// and help. The Terminals table (agents and overview) lives in Terminals.tsx.
// Each is a list of rows with mouse actions; nothing here changes the TUI's
// layout.
import { useEffect, useRef, useState } from "react";
import type { Pane, SearchMatch } from "./terminal/types";
import { Icon } from "./status";
import { age } from "./format";
import { StateMark } from "./state/StateMark";
import { ProviderLogo } from "./state/ProviderLogo";
import {
  notificationStatus,
  paneNotifications,
  paneState,
  stateLabel,
  statusState,
  type Notification,
} from "./state/model";
import { shortcuts } from "./shortcuts";
import { paletteColor, paletteNames } from "./palette";

const name = (p: Pane) => p.title || "shell";

/** One terminal's notifications, opened from the bell in its header (or
 * Ctrl-B b): its unread ones, or its whole retained history; mark one or
 * all of them read. Read marks are the workspace's, shared with the TUI. */
export function TerminalNotificationsPanel({
  pane,
  notifications,
  onAcknowledge,
}: {
  pane: Pane;
  notifications: Notification[];
  onAcknowledge: (ids: number[]) => void;
}) {
  const [history, setHistory] = useState(false);
  const list = paneNotifications(notifications, pane, history);
  const unread = paneNotifications(notifications, pane, false);
  return (
    <div className="panel-body" data-pane={pane.id}>
      <div className="panel-toolbar">
        <div className="seg panel-seg" role="group" aria-label="Notifications">
          <button aria-pressed={!history} onClick={() => setHistory(false)}>
            Unread <span className="count">{unread.length}</span>
          </button>
          <button aria-pressed={history} onClick={() => setHistory(true)}>
            History
          </button>
        </div>
        <span className="spacer" />
        <button
          type="button"
          className="subtle small"
          disabled={!unread.length}
          onClick={() => onAcknowledge(unread.map((n) => n.id))}
        >
          <Icon name="check" size="sm" />
          Mark all read
        </button>
      </div>
      <div className="panel-list" role="list">
        {list.map((n) => {
          const state = statusState(notificationStatus(notifications, n));
          return (
            <div
              className={`list-row ${n.read ? "read" : "unread"}`}
              role="listitem"
              key={n.id}
              data-notification={n.id}
            >
              <span
                className={`unread-dot ${n.read ? "" : "on"}`}
                aria-label={n.read ? "Read" : "Unread"}
                role="img"
              />
              <StateMark state={state} size="md" />
              <div className="grow">
                <span className="t">{n.title}</span>
                <span className="s">{age(n.at)}</span>
              </div>
              {!n.read && (
                <button
                  type="button"
                  className="icon-btn"
                  aria-label={`Mark “${n.title}” read`}
                  title="Mark read"
                  onClick={() => onAcknowledge([n.id])}
                >
                  <Icon name="check" size="sm" />
                </button>
              )}
            </div>
          );
        })}
        {!list.length && (
          <p className="panel-empty">
            {history
              ? "No notifications from this terminal yet."
              : "No unread notifications in this terminal."}
          </p>
        )}
      </div>
    </div>
  );
}

export type ClosedEntry = {
  key: string;
  workspace: string;
  pane: number;
  cwd: string;
  name: string;
  kind?: string | null;
  session_id?: string | null;
  argv: string[];
  closed_at?: number | null;
  reason: string;
  choices: ("resume" | "restart" | "folder")[];
};
const choiceLabel = {
  resume: "Resume",
  restart: "Rerun",
  folder: "Open folder",
};
const choiceHint = (entry: ClosedEntry, choice: ClosedEntry["choices"][0]) =>
  choice === "resume"
    ? entry.session_id
      ? "Resume this conversation in a new terminal"
      : "Open the agent's conversation picker in its folder"
    : choice === "restart"
      ? `Rerun ${entry.argv.join(" ")} in its folder`
      : "Open a shell in this folder";
/** The command the TUI copies for an agent conversation. */
export function resumeCommand(entry: Pick<ClosedEntry, "kind" | "session_id">) {
  const base =
    entry.kind === "codex"
      ? "codex resume"
      : entry.kind === "claude"
        ? "claude --resume"
        : "";
  if (!base) return "";
  const id = entry.session_id;
  return id && /^[A-Za-z0-9_-]{1,128}$/.test(id) ? `${base} ${id}` : base;
}

export function RecoveryPanel({
  rpc,
  onRestored,
  onCopy,
}: {
  rpc: (method: string, params?: any) => Promise<any>;
  onRestored: (pane: number, existing: boolean) => void;
  onCopy: (text: string) => void;
}) {
  const [listing, setListing] = useState<{
    workspace: string;
    entries: ClosedEntry[];
  } | null>(null);
  const [busy, setBusy] = useState("");
  useEffect(() => {
    let alive = true;
    rpc("closed").then((result) => {
      if (alive) setListing(result || { workspace: "", entries: [] });
    });
    return () => {
      alive = false;
    };
  }, []);
  async function restore(entry: ClosedEntry, choice: string) {
    if (busy) return;
    setBusy(entry.key);
    try {
      const result = await rpc("restore", {
        key: entry.key,
        choice,
        request: crypto.randomUUID(),
        confirmed: true,
      });
      if (result?.pane) onRestored(result.pane, !!result.existing);
    } finally {
      setBusy("");
    }
  }
  if (!listing)
    return (
      <div className="panel-body">
        <p className="panel-empty">Loading recently closed terminals…</p>
      </div>
    );
  return (
    <div className="panel-body">
      <div className="panel-list" role="list">
        {listing.entries.map((entry) => {
          const command = resumeCommand(entry);
          return (
            <div className="list-row" role="listitem" key={entry.key}>
              {entry.kind ? (
                <ProviderLogo kind={entry.kind} />
              ) : (
                <Icon name="terminal" size="sm" className="faint" />
              )}
              <div className="grow">
                <span className="t">{entry.name}</span>
                <span className="s">
                  {entry.workspace !== listing.workspace &&
                    `${entry.workspace} · `}
                  <span className="mono">{entry.cwd}</span> · {entry.reason} ·{" "}
                  {age(entry.closed_at || 0)}
                </span>
              </div>
              {command && (
                <button
                  type="button"
                  className="icon-btn"
                  title={`Copy “${command}”`}
                  aria-label={`Copy the resume command of ${entry.name}`}
                  onClick={() => onCopy(command)}
                >
                  <Icon name="copy" size="sm" />
                </button>
              )}
              {entry.choices.map((choice) => (
                <button
                  type="button"
                  key={choice}
                  className={`small ${choice === entry.choices[0] ? "success" : "subtle"}`}
                  title={choiceHint(entry, choice)}
                  aria-label={`${choiceLabel[choice]} ${entry.name}`}
                  disabled={!!busy}
                  onClick={() => restore(entry, choice)}
                >
                  {choice === entry.choices[0] && (
                    <Icon name="rotate-ccw" size="sm" />
                  )}
                  {choiceLabel[choice]}
                </button>
              ))}
            </div>
          );
        })}
        {!listing.entries.length && (
          <p className="panel-empty">
            Closed terminals and previous runs with an agent or a program will
            appear here.
          </p>
        )}
      </div>
    </div>
  );
}

type Hit = { pane: Pane; match: SearchMatch };
/** Workspace search, the TUI's search dialog: the focused terminal or every
 * terminal's history, through the server's bounded literal search. */
export function SearchPanel({
  panes,
  focused,
  rpc,
  onOpen,
}: {
  panes: Pane[];
  focused?: Pane;
  rpc: (method: string, params?: any) => Promise<any>;
  onOpen: (pane: Pane, query: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [all, setAll] = useState(!focused);
  const [hits, setHits] = useState<Hit[]>([]);
  const [busy, setBusy] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.focus(), []);
  useEffect(() => {
    let alive = true;
    setHits([]);
    const text = query.trim() ? query : "";
    if (!text || text.length > 256 || /[\u0000-\u001f\u007f]/.test(text)) {
      setBusy(false);
      return;
    }
    setBusy(true);
    const timer = setTimeout(async () => {
      const targets = all ? panes.slice(0, 32) : focused ? [focused] : [];
      const found: Hit[] = [];
      for (const pane of targets) {
        const result = await rpc("search", { pane: pane.id, query: text });
        if (!alive) return;
        for (const match of result?.PaneSearch?.hits || [])
          if (found.length < 200) found.push({ pane, match });
        setHits([...found]);
      }
      if (alive) setBusy(false);
    }, 250);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [query, all, panes.map((p) => p.id).join(",")]);
  return (
    <div className="panel-body">
      <div className="panel-toolbar">
        <input
          ref={input}
          className="grow"
          type="search"
          placeholder="Search terminal history"
          aria-label="Search terminal history"
          value={query}
          maxLength={256}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Tab" && focused) {
              event.preventDefault();
              setAll(!all);
            }
            if (event.key === "Enter" && hits[0]) onOpen(hits[0].pane, query);
          }}
        />
        <div className="seg panel-seg" role="group" aria-label="Search scope">
          <button
            aria-pressed={!all}
            disabled={!focused}
            onClick={() => setAll(false)}
          >
            Focused terminal
          </button>
          <button aria-pressed={all} onClick={() => setAll(true)}>
            All terminals
          </button>
        </div>
      </div>
      <div className="panel-list" role="list">
        {hits.map(({ pane, match }, index) => (
          <button
            type="button"
            className="list-row search-hit"
            role="listitem"
            key={`${pane.id}-${index}`}
            onClick={() => onOpen(pane, query)}
          >
            <span className="search-where">
              {pane.number || "S"} {name(pane)}
            </span>
            <span className="grow mono search-line">
              {match.lines[match.start.y - match.first_line] ||
                match.lines.join(" ")}
            </span>
          </button>
        ))}
        {!hits.length && (
          <p className="panel-empty">
            {busy
              ? "Searching…"
              : query
                ? "No matches."
                : "Type to search. Tab switches between the focused terminal and all terminals."}
          </p>
        )}
      </div>
    </div>
  );
}

export function ParkedPanel({
  panes,
  notifications,
  onUnpark,
  onClose,
}: {
  panes: Pane[];
  notifications: Notification[];
  onUnpark: (pane: Pane) => void;
  onClose: (pane: Pane) => void;
}) {
  return (
    <div className="panel-body">
      <div className="panel-list" role="list">
        {panes.map((p) => (
          <div className="list-row" role="listitem" key={p.id}>
            <StateMark state={paneState(p, notifications)} size="md" />
            <ProviderLogo kind={p.kind} />
            <div className="grow">
              <span className="t">{name(p)}</span>
              <span className="s mono">{p.cwd}</span>
            </div>
            <button
              type="button"
              className="small success"
              aria-label={`Unpark ${name(p)}`}
              onClick={() => onUnpark(p)}
            >
              <Icon name="rotate-ccw" size="sm" />
              Unpark
            </button>
            <button
              type="button"
              className="small subtle"
              aria-label={`Close ${name(p)}`}
              onClick={() => onClose(p)}
            >
              Close
            </button>
          </div>
        ))}
        {!panes.length && (
          <p className="panel-empty">
            Parking hides a terminal from this browser's tabs and grid; it keeps
            running. Park one with Ctrl-B p or its sidebar button.
          </p>
        )}
      </div>
    </div>
  );
}

/** Move the focused terminal to a position, the TUI's move dialog. */
export function MovePanel({
  panes,
  source,
  onMove,
}: {
  panes: Pane[];
  source?: Pane;
  onMove: (before: number | null) => void;
}) {
  if (!source)
    return (
      <div className="panel-body">
        <p className="panel-empty">Focus a terminal to move it.</p>
      </div>
    );
  const others = panes.filter((p) => p.id !== source.id);
  return (
    <div className="panel-body">
      <p className="setting-hint">
        Move “{name(source)}” in this browser's order. You can also drag
        terminals in the sidebar or grid tiles by their header.
      </p>
      <div className="move-grid" role="list">
        {others.map((p, index) => (
          <button
            type="button"
            role="listitem"
            key={p.id}
            className="move-target"
            onClick={() => onMove(p.id)}
          >
            <span className="move-position">{index + 1}</span>
            Before {p.number} {name(p)}
          </button>
        ))}
        <button
          type="button"
          role="listitem"
          className="move-target"
          onClick={() => onMove(null)}
        >
          <span className="move-position">{others.length + 1}</span>
          At the end
        </button>
      </div>
    </div>
  );
}

export function HelpPanel() {
  return (
    <div className="panel-body">
      <table className="shortcut-table">
        <tbody>
          {shortcuts.map(([keys, action]) => (
            <tr key={keys}>
              <th scope="row">
                <kbd>{keys}</kbd>
              </th>
              <td>{action}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="setting-hint">
        The mouse works everywhere: click a terminal in the sidebar to open it,
        drag to reorder, middle-click to close and right-click for more actions.
        Every other key goes to the focused terminal. This browser's order,
        parking, grid and focus are separate from the TUI's.
      </p>
    </div>
  );
}

/** Context menu of a terminal: the actions its row and tile offer. */
export function PaneMenu({
  pane,
  x,
  y,
  parked,
  items,
  onClose,
}: {
  pane: Pane;
  x: number;
  y: number;
  parked: boolean;
  items: [string, () => void, boolean?][];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current?.querySelector<HTMLButtonElement>("button")?.focus();
    const outside = (event: PointerEvent) => {
      if (!ref.current?.contains(event.target as Node)) onClose();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("pointerdown", outside, true);
    window.addEventListener("keydown", escape, true);
    return () => {
      window.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("keydown", escape, true);
    };
  }, []);
  return (
    <div
      ref={ref}
      className="popover menu pane-menu"
      role="menu"
      aria-label={`${name(pane)}${parked ? " (parked)" : ""}`}
      style={{
        left: Math.min(x, innerWidth - 220),
        top: Math.min(y, innerHeight - 40 * items.length - 16),
      }}
    >
      {items.map(([label, action, disabled]) => (
        <button
          type="button"
          role="menuitem"
          key={label}
          className="menu-item"
          disabled={disabled}
          onClick={() => {
            onClose();
            action();
          }}
        >
          <span className="grow">{label}</span>
        </button>
      ))}
    </div>
  );
}

/** The state label of a pane, for headers and tooltips. */
export const paneStateLabel = (p: Pane, notifications: Notification[]) =>
  stateLabel[paneState(p, notifications)];

/** The tab colour picker: one of the TUI's nine palette colours, kept in
 * this browser only. */
export function ColorPanel({
  pane,
  color,
  onPick,
}: {
  pane: Pane;
  /** The pane's current colour in this browser. */
  color: number;
  onPick: (color: number) => void;
}) {
  return (
    <div className="panel-body">
      <p className="setting-hint">
        Colour for “{name(pane)}” in this browser. The TUI and other browsers
        keep their own colours.
      </p>
      <div className="color-grid" role="list">
        {paletteNames.map((label, index) => (
          <button
            type="button"
            key={label}
            className="color-swatch"
            aria-pressed={color === index}
            aria-label={label}
            onClick={() => onPick(index)}
          >
            <span
              className="swatch"
              style={{ background: paletteColor(index) }}
              aria-hidden="true"
            />
            {label}
          </button>
        ))}
      </div>
    </div>
  );
}
