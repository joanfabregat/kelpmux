// Workspace dialogs, the web equivalents of the TUI's: a terminal's
// notifications, History (closed terminals and their agent conversations),
// search, parked terminals, move and help. The Terminals table (agents and
// overview) lives in Terminals.tsx.
// Each is a list of rows with mouse actions; nothing here changes the TUI's
// layout.
import React, { useEffect, useRef, useState } from "react";
import type { NumberedPane, SearchMatch } from "@kelpcore/web/types.ts";
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
import { projectColor, type Project } from "./projects";
import {
  choiceHint,
  choiceText,
  folderRequest,
  matchedHint,
  pickerHint,
  pickerLabel,
  pickerRequest,
  unidentified,
  conversationTitle,
  exactSpan,
  historyRows,
  restoreRequest,
  resumeCommand,
  resumeRequest,
  span,
  toolName,
  type ClosedEntry,
  type Conversation,
  type HistoryListing,
} from "./conversations";

const name = (p: NumberedPane) => p.title || "shell";

/** One terminal's notifications, opened from the bell in its header (or
 * Ctrl-B b): its whole retained history (the default), or its unread ones;
 * mark one or all of them read. Read marks are the workspace's, shared with
 * the TUI. */
export function TerminalNotificationsPanel({
  pane,
  notifications,
  onAcknowledge,
}: {
  pane: NumberedPane;
  notifications: Notification[];
  onAcknowledge: (ids: number[]) => void;
}) {
  // Showing a terminal marks its notifications read, so the panel is
  // mainly its history; Unread and Mark all read remain.
  const [history, setHistory] = useState(true);
  const list = paneNotifications(notifications, pane, history);
  const unread = paneNotifications(notifications, pane, false);
  return (
    <div className="panel-body" data-pane={pane.id}>
      <div className="panel-toolbar">
        <div className="seg panel-seg" role="group" aria-label="Notifications">
          <button aria-pressed={history} onClick={() => setHistory(true)}>
            History
          </button>
          <button aria-pressed={!history} onClick={() => setHistory(false)}>
            Unread <span className="count">{unread.length}</span>
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


/** History: closed terminals with every agent conversation they hosted,
 * and conversations started outside KelpMux in a project's folder, each
 * resumable in a new terminal of its project. With a bridge before API
 * minor 7, the closed terminals only, with their restore choices. */
export function HistoryPanel({
  scope,
  projects,
  onAllProjects,
  conversations,
  rpc,
  onRestored,
  onCopy,
}: {
  rpc: (method: string, params?: any) => Promise<any>;
  onRestored: (pane: number, existing: boolean) => void;
  onCopy: (text: string) => void;
  /** Only this project's entries (Scratch's: those of no existing
   * project). */
  scope?: Project;
  /** The user's projects, to tell Scratch's entries apart. */
  projects?: Project[];
  onAllProjects?: () => void;
  /** The bridge lists conversations (`conversations`, API minor 7). */
  conversations?: boolean;
}) {
  const [listing, setListing] = useState<HistoryListing | null>(null);
  const [busy, setBusy] = useState("");
  useEffect(() => {
    let alive = true;
    rpc(conversations ? "conversations" : "closed").then((result) => {
      if (alive) setListing(result || { workspace: "", entries: [] });
    });
    return () => {
      alive = false;
    };
  }, [conversations]);
  async function restore(entry: ClosedEntry, choice: string) {
    if (busy) return;
    setBusy(entry.key);
    try {
      const result = await rpc("restore", restoreRequest(entry, choice));
      if (result?.pane) onRestored(result.pane, !!result.existing);
    } finally {
      setBusy("");
    }
  }
  /** Resumes a conversation in a new terminal of `project`, in its
   * folder; the bridge builds the command from the kind and id. */
  async function resume(c: Conversation, project: string, key: string) {
    await open(key, resumeRequest(c, project));
  }
  /** Sends `new` (a resume, the agent's picker or a shell) and shows the
   * terminal it opened. */
  async function open(key: string, request: object | null) {
    if (busy || !request) return;
    setBusy(key);
    try {
      const result = await rpc("new", request);
      if (result?.pane) onRestored(result.pane, !!result.existing);
    } finally {
      setBusy("");
    }
  }
  if (!listing)
    return (
      <div className="panel-body">
        <p className="panel-empty">Loading history…</p>
      </div>
    );
  const rows = historyRows(listing, projects);
  const shown = scope ? rows.filter((row) => row.project === scope.id) : rows;
  const color = projectColor(scope);
  /** A row's project, in the global list. */
  const projectTag = (id: string) => {
    if (scope) return null;
    const project = projects?.find((p) => p.id === id);
    const tint = projectColor(project);
    return (
      <span
        className="history-project"
        style={tint ? ({ "--project": tint } as React.CSSProperties) : undefined}
      >
        <span className={`project-dot ${tint ? "" : "plain"}`} aria-hidden="true" />
        {project ? project.name : "Scratch"}
      </span>
    );
  };
  const conversationActions = (
    c: Conversation,
    project: string,
    key: string,
    label: string,
  ) => {
    const command = resumeCommand({ kind: c.kind, session_id: c.id });
    return (
      <>
        {command && (
          <button
            type="button"
            className="icon-btn"
            title={`Copy “${command}”`}
            aria-label={`Copy the resume command of ${label}`}
            onClick={() => onCopy(command)}
          >
            <Icon name="copy" size="sm" />
          </button>
        )}
        <button
          type="button"
          className="small success"
          title={
            c.folder
              ? `Resume in a new terminal in ${c.folder}`
              : "Its folder no longer exists"
          }
          aria-label={`Resume ${label}`}
          disabled={!!busy || !c.folder}
          onClick={() => resume(c, project, key)}
        >
          <Icon name="rotate-ccw" size="sm" />
          Resume
        </button>
      </>
    );
  };
  return (
    <div className="panel-body history-panel">
      {scope && (
        <div
          className="project-scope"
          data-scope={scope.id}
          style={color ? ({ "--project": color } as React.CSSProperties) : undefined}
        >
          <span className={`project-dot ${color ? "" : "plain"}`} aria-hidden="true" />
          <span className="grow">
            <b>{scope.name}</b> · {shown.length} of {rows.length}
          </span>
          {onAllProjects && (
            <button type="button" className="ghost small" onClick={onAllProjects}>
              All projects
            </button>
          )}
        </div>
      )}
      <div className="panel-list" role="list">
        {shown.map((row) => {
          if (row.type === "external") {
            const c = row.conversation;
            const label = conversationTitle(c);
            const key = `external:${c.kind}:${c.id}`;
            return (
              <div
                className="list-row history-external"
                role="listitem"
                key={key}
                data-conversation={c.id}
                data-project={row.project}
              >
                <ProviderLogo kind={c.kind} />
                <div className="grow">
                  <span className="t" title={label}>
                    {label}
                  </span>
                  <span className="s">
                    <span className="badge">outside KelpMux</span>{" "}
                    {toolName(c.kind)} · {projectTag(row.project)}
                    <span className="mono">{c.cwd}</span> ·{" "}
                    <span title={exactSpan(c)}>{span(c)}</span>
                  </span>
                </div>
                {conversationActions(c, row.project, key, label)}
              </div>
            );
          }
          const entry = row.entry;
          const nested = entry.conversations || [];
          /** Where and when the terminal ran: its secondary line. */
          const where = (cwd: string) => (
            <>
              {entry.workspace !== listing.workspace && `${entry.workspace} · `}
              {projectTag(row.project)}
              <span className="mono">{cwd}</span> · {entry.reason} ·{" "}
              {age(entry.closed_at || 0)}
            </>
          );
          const matched = (c: Conversation) =>
            c.matched && (
              <>
                <span className="history-matched" title={matchedHint}>
                  matched by folder and time
                </span>
                {" · "}
              </>
            );
          // One conversation: one row, titled by it, with its terminal as
          // the secondary text.
          if (nested.length === 1) {
            const c = nested[0];
            const label = conversationTitle(c);
            const key = `${entry.key}:${c.kind}:${c.id}`;
            return (
              <div
                className="history-group"
                role="listitem"
                key={entry.key}
                data-entry={entry.key}
                data-project={row.project}
              >
                <div
                  className="list-row history-terminal history-single"
                  data-conversation={c.id}
                >
                  <ProviderLogo kind={c.kind} />
                  <div className="grow">
                    <span
                      className={`t ${c.titled === false ? "untitled" : ""}`}
                      title={label}
                    >
                      {label}
                    </span>
                    <span className="s">
                      {toolName(c.kind)} · {matched(c)}
                      {c.titled !== false && (
                        <>
                          <span className="history-name">{entry.name}</span>
                          {" · "}
                        </>
                      )}
                      {where(c.cwd || entry.cwd)}
                      {span(c) && (
                        <>
                          {" · ran "}
                          <span title={exactSpan(c)}>{span(c)}</span>
                        </>
                      )}
                    </span>
                  </div>
                  {conversationActions(c, row.project, key, label)}
                </div>
              </div>
            );
          }
          // A closed agent terminal whose conversation is unknown: the
          // agent's own picker in its folder, never a guessed conversation.
          const picker = !!conversations && !nested.length && unidentified(entry);
          const command = nested.length || picker ? "" : resumeCommand(entry);
          return (
            <div
              className="history-group"
              role="listitem"
              key={entry.key}
              data-entry={entry.key}
              data-project={row.project}
            >
              <div className="list-row history-terminal">
                {entry.kind ? (
                  <ProviderLogo kind={entry.kind} />
                ) : (
                  <Icon name="terminal" size="sm" className="faint" />
                )}
                <div className="grow">
                  <span className="t">{entry.name}</span>
                  <span className="s">
                    {nested.length > 1 && (
                      <>
                        <span className="history-count">
                          {nested.length} conversations
                        </span>
                        {" · "}
                      </>
                    )}
                    {where(entry.cwd)}
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
                {picker && (
                  <>
                    <button
                      type="button"
                      className="small success"
                      title={pickerHint(entry)}
                      aria-label={`${pickerLabel(entry.kind)}: ${entry.name}`}
                      disabled={!!busy}
                      onClick={() =>
                        open(entry.key, pickerRequest(entry, row.project))
                      }
                    >
                      <Icon name="rotate-ccw" size="sm" />
                      {pickerLabel(entry.kind)}
                    </button>
                    <button
                      type="button"
                      className="small subtle"
                      title={`Open a shell in ${entry.cwd}`}
                      aria-label={`Open folder ${entry.name}`}
                      disabled={!!busy}
                      onClick={() => open(entry.key, folderRequest(entry, row.project))}
                    >
                      Open folder
                    </button>
                  </>
                )}
                {!nested.length &&
                  !picker &&
                  entry.choices.map((choice) => (
                    <button
                      type="button"
                      key={choice}
                      className={`small ${choice === entry.choices[0] ? "success" : "subtle"}`}
                      title={choiceHint(entry, choice)}
                      aria-label={`${choiceText(entry, choice)} ${entry.name}`}
                      disabled={!!busy}
                      onClick={() => restore(entry, choice)}
                    >
                      {choice === entry.choices[0] && (
                        <Icon name="rotate-ccw" size="sm" />
                      )}
                      {choiceText(entry, choice)}
                    </button>
                  ))}
              </div>
              {nested.length > 1 && (
                <div className="history-conversations" role="list">
                  {nested.map((c) => {
                    const label = conversationTitle(c);
                    const key = `${entry.key}:${c.kind}:${c.id}`;
                    return (
                      <div
                        className="list-row history-conversation"
                        role="listitem"
                        key={key}
                        data-conversation={c.id}
                      >
                        <ProviderLogo kind={c.kind} />
                        <div className="grow">
                          <span className={`t ${c.titled === false ? "untitled" : ""}`} title={label}>
                            {label}
                          </span>
                          <span className="s">
                            {toolName(c.kind)}
                            {c.cwd && c.cwd !== entry.cwd && (
                              <>
                                {" · "}
                                <span className="mono">{c.cwd}</span>
                              </>
                            )}
                            {span(c) && (
                              <>
                                {" · "}
                                <span title={exactSpan(c)}>{span(c)}</span>
                              </>
                            )}
                          </span>
                        </div>
                        {conversationActions(c, row.project, key, label)}
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          );
        })}
        {!shown.length && (
          <p className="panel-empty">
            Closed terminals with their agent conversations, and conversations
            started outside KelpMux in a project's folder, will appear here.
          </p>
        )}
        {listing.truncated && (
          <p className="setting-hint history-note">
            Conversations outside KelpMux: the last 30 days, at most 200 per
            project.
          </p>
        )}
      </div>
    </div>
  );
}

type Hit = { pane: NumberedPane; match: SearchMatch };
/** Workspace search, the TUI's search dialog: the focused terminal or every
 * terminal's history, through the server's bounded literal search. */
export function SearchPanel({
  panes,
  focused,
  rpc,
  onOpen,
}: {
  panes: NumberedPane[];
  focused?: NumberedPane;
  rpc: (method: string, params?: any) => Promise<any>;
  onOpen: (pane: NumberedPane, query: string) => void;
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
  panes: NumberedPane[];
  notifications: Notification[];
  onUnpark: (pane: NumberedPane) => void;
  onClose: (pane: NumberedPane) => void;
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
  panes: NumberedPane[];
  source?: NumberedPane;
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

/** The Help dialog: the browser shortcuts, then a short guide to the
 * mouse, files, the editor and alerts. Keep it in step with docs/web.md. */
export function HelpPanel() {
  return (
    <div className="panel-body help-panel">
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
      <h3>Terminals</h3>
      <p className="setting-hint">
        The mouse works everywhere: click a terminal in the sidebar to open it,
        drag to reorder, middle-click to close and right-click for more actions.
        Every other key goes to the terminal shown on its own; the grid is
        read-only and a click on a tile opens it. This browser's order, parking,
        grid and focus are separate from the TUI's.
      </p>
      <p className="setting-hint">
        The terminals come first in the sidebar. <b>New terminal</b>, right
        under them, opens a new one (Ctrl-B Enter). At the bottom, above the
        system vitals, <b>Grid</b> shows the live grid and <b>Files</b> the file
        browser.
      </p>
      <h3>Files</h3>
      <p className="setting-hint">
        Click <b>Files</b> at the bottom of the sidebar for the file browser; a
        terminal's folder button opens the folder it is in. It shows columns
        like the Finder: selecting a folder opens it in the next column,
        selecting a file shows its details (and the first lines of a small text
        file) at the right, and a double-click or Enter opens it. Each entry
        shows its date and size under its name. Arrow keys move: ↑ and ↓ within
        a column, → into a folder, ← back; type a name to jump to it. Drag a
        column's right edge to resize it. Phones show one folder at a time. New
        file and New folder (above the columns) create in the column you are in;
        each entry's menu (right-click or ⋯) opens it, opens it beside the
        terminal, inserts or copies its path, downloads it, renames it, moves it
        to the trash or deletes it permanently. Drag an entry onto a folder to
        move it, or onto a terminal's row or view to type its quoted path; drop
        files from your computer onto a folder or a column to upload them there.
        Browsing stays inside your home folder (or the folder the server was
        started with): links that point outside it are shown dimmed and do not
        open.
      </p>
      <p className="setting-hint">
        Hold Ctrl/Cmd over a file path printed in the terminal (path, path:line
        or path:line:col): a path that exists inside the browsable folders is
        underlined, and a click opens the file beside the terminal at that line,
        or a folder in Files.
      </p>
      <h3>Editor</h3>
      <p className="setting-hint">
        Files open in the main area as tabs along the Files view's top: the
        first tab is the file browser, then one per open file, with a dot while
        it has unsaved changes, which are kept while you look elsewhere. Click a
        tab to show it; its × or a middle-click closes it (asking first when it
        has unsaved changes), and the next tab shows. Ctrl/Cmd+S saves; if the
        file changed on disk meanwhile, choose Reload, Overwrite or Cancel.{" "}
        <b>Open beside terminal</b> in a file's menu, or the editor's{" "}
        <b>Move beside terminal</b>, shows the file in a side panel to the right
        of the terminal; drag its edge to resize it. Phones have no side panel.
      </p>
      <h3>Alerts</h3>
      <p className="setting-hint">
        A question, a finished turn, a failure, an exit or a bell shows a toast
        in the top right corner, one per terminal (a summary above three),
        except for the terminal you are reading; downloads and confirmations
        stack below them. Click a toast to open its terminal; it goes away once
        its notifications are read, here or anywhere else. An unread bell also
        puts a dot on the browser tab's icon. Choose the events in Settings →
        Notifications.
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
  pane: NumberedPane;
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
export const paneStateLabel = (
  p: NumberedPane,
  notifications: Notification[],
) => stateLabel[paneState(p, notifications)];
