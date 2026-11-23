import React, { useState } from "react";
import type { NumberedPane } from "@kelpcore/web/types.ts";
import type { IconName } from "./icons";
import { Icon } from "./status";
import { Tooltip } from "./Tooltip";
import { basename } from "./format";
import { StateCounts, StateMark } from "./state/StateMark";
import { ProviderLogo } from "./state/ProviderLogo";
import {
  paneState,
  rollup,
  rollupEntries,
  stateLabel,
  unreadFor,
  type DisplayState,
  type Notification,
} from "./state/model";
import { Vitals } from "./Vitals";
import { UnreadMark, type SessionMode } from "./Shell";

/** What a state shortcut in the sidebar summary opens. */
const openStateLabel: Partial<Record<DisplayState, string>> = {
  input_needed: "Next terminal that needs input (Ctrl-B N)",
  failed: "Next failed terminal (Ctrl-B N)",
  finished: "Next finished terminal (Ctrl-B P)",
  working: "Terminals: agents (Ctrl-B a)",
};

export type SidebarProps = {
  username: string;
  dev: boolean;
  folded: boolean;
  /** Touch device: rows show a menu button instead of hover actions. */
  touch?: boolean;
  /** Phone home screen: no grid. */
  phone?: boolean;
  /** The terminal view: one terminal or the read-only grid. */
  mode: SessionMode;
  /** Terminals in this browser's order, parked ones excluded. */
  terminals: NumberedPane[];
  /** Terminals parked in this browser. */
  parked: NumberedPane[];
  notifications: Notification[];
  current?: number;
  vitals: { cpu?: number; memory?: number; disks?: number[] };
  /** Mount points of the vitals' disks, in order, once known. */
  diskMounts: string[];
  onDisksHover: () => void;
  onMode: (mode: SessionMode) => void;
  rowProps: (pane: NumberedPane) => React.HTMLAttributes<HTMLDivElement>;
  rowClass: (pane: NumberedPane) => string;
  onOpen: (pane: NumberedPane) => void;
  onClose: (pane: NumberedPane) => void;
  onPark: (pane: NumberedPane) => void;
  onUnpark: (pane: NumberedPane) => void;
  onMenu: (pane: NumberedPane, x: number, y: number) => void;
  onNew: () => void;
  onState: (state: DisplayState) => void;
  onDialog: (dialog: string) => void;
  onResizeStart: (event: React.PointerEvent) => void;
  onToggleFold: () => void;
};

/** The left sidebar, like the TUI's, and the only navigation: the state
 * counts, the live grid's entry (not on phones), every terminal in this
 * browser's order with its live state, a rule between entries, the
 * new-terminal row right under them (centred on a rule, like the TUI's)
 * and the parked terminals. The grid's entry and the terminals' look
 * alike. The grid's entry opens the grid and a terminal's opens that
 * terminal alone; the one on screen is selected. A tab with unread
 * notifications shows a bell; the notifications themselves open from the
 * terminal's header. At the bottom the system vitals, the tools in two
 * columns like the TUI's buttons (the Terminals table, recently closed,
 * search, help), then the account and its settings. Order, parking and
 * folding belong to this browser; they never change the TUI's layout. The
 * grid is read-only: a tile's click opens its terminal alone. */
export function Sidebar(props: SidebarProps) {
  const {
    username,
    dev,
    folded,
    phone,
    mode,
    terminals,
    parked,
    notifications,
    current,
    vitals,
  } = props;
  const [parkedOpen, setParkedOpen] = useState(true);
  const summary = rollup(terminals.concat(parked), notifications);
  const grid = mode === "grid";
  const row = (p: NumberedPane, isParked: boolean) => {
    const state = paneState(p, notifications);
    const unread = unreadFor(notifications, p).length;
    const title = p.title || "shell";
    const on = current === p.id && !isParked;
    return (
      <div
        key={p.id}
        className={`nav-cell term-row ${isParked ? "parked-row" : ""} ${props.rowClass(p)}`}
        data-drop-pane={isParked ? undefined : p.id}
        data-pane={p.id}
        data-state={state}
        {...(isParked ? {} : props.rowProps(p))}
        onContextMenu={(event) => {
          event.preventDefault();
          props.onMenu(p, event.clientX, event.clientY);
        }}
        onMouseDown={(event) => {
          if (event.button === 1) event.preventDefault();
        }}
        onAuxClick={(event) => {
          if (event.button !== 1) return;
          event.preventDefault();
          props.onClose(p);
        }}
      >
        <button
          type="button"
          className={`nav-item term-item ${on ? "current" : ""}`}
          aria-current={on ? "page" : undefined}
          aria-label={`${isParked ? "Unpark and open" : "Open"} terminal ${p.number || ""} ${title} (${stateLabel[state]}${unread ? `, ${unread} unread` : ""})`}
          title={`${title}\n${p.cwd}`}
          onClick={() => (isParked ? props.onUnpark(p) : props.onOpen(p))}
        >
          <span className="term-number" aria-hidden="true">
            {isParked ? "–" : p.number || "·"}
          </span>
          <StateMark state={state} decorative />
          <ProviderLogo kind={p.kind} decorative />
          <span className="grow term-names">
            <span className="term-title">{title}</span>
            <small className="term-folder">{basename(p.cwd)}</small>
          </span>
          <UnreadMark count={unread} />
        </button>
        <span className="term-row-actions">
          {props.touch ? (
            <button
              type="button"
              className="icon-btn term-row-menu"
              aria-label={`Menu for ${title}`}
              onClick={(event) => {
                const rect = event.currentTarget.getBoundingClientRect();
                props.onMenu(p, Math.max(8, rect.right - 220), rect.bottom);
              }}
            >
              <Icon name="ellipsis" size="sm" />
            </button>
          ) : isParked ? (
            <Tooltip content="Unpark">
              <button
                type="button"
                className="icon-btn"
                aria-label={`Unpark ${title}`}
                onClick={() => props.onUnpark(p)}
              >
                <Icon name="rotate-ccw" size="sm" />
              </button>
            </Tooltip>
          ) : (
            <Tooltip content="Park: hide from tabs and grid (Ctrl-B p)">
              <button
                type="button"
                className="icon-btn"
                aria-label={`Park ${title}`}
                onClick={() => props.onPark(p)}
              >
                <Icon name="archive" size="sm" />
              </button>
            </Tooltip>
          )}
          {!props.touch && (
            <Tooltip content="Close (middle-click)">
              <button
                type="button"
                className="icon-btn"
                aria-label={`Close ${title}`}
                onClick={() => props.onClose(p)}
              >
                <Icon name="x" size="sm" />
              </button>
            </Tooltip>
          )}
        </span>
      </div>
    );
  };
  /** A tool button: its icon and short label, the full name and shortcut
   * in its tooltip and accessible name. */
  const entry = (
    key: string,
    label: string,
    short: string,
    icon: IconName,
    keys: string,
    onClick: () => void,
  ) => (
    <Tooltip key={key} content={`${label} (${keys})`}>
      <button
        type="button"
        className="nav-item tool-item"
        aria-label={label}
        data-tool={key}
        onClick={onClick}
      >
        <Icon name={icon} size="sm" />
        <span className="grow">{short}</span>
      </button>
    </Tooltip>
  );
  const tool = (
    dialog: string,
    label: string,
    short: string,
    icon: IconName,
    keys: string,
  ) => entry(dialog, label, short, icon, keys, () => props.onDialog(dialog));
  return (
    <aside className={`rail ${folded ? "folded" : ""}`} aria-label="Terminals">
      <div className="rail-head">
        <span className="logo">
          <span className="logo-mark" aria-hidden="true">
            <Icon name="layers" size="sm" />
          </span>
          <span className="logo-text">KelpMux</span>
        </span>
        {dev && <span className="env-badge">DEV</span>}
        {summary.total > 0 && (
          <span className="state-counts rail-marks">
            {rollupEntries(summary).map(([state, count]) => (
              <Tooltip
                key={state}
                content={openStateLabel[state] || stateLabel[state]}
              >
                <button
                  type="button"
                  className={`state-count state-${state} rail-mark`}
                  aria-label={openStateLabel[state] || stateLabel[state]}
                  onClick={() => props.onState(state)}
                >
                  <StateMark state={state} decorative />
                  <span className="state-count-n" aria-hidden="true">
                    {count}
                  </span>
                </button>
              </Tooltip>
            ))}
          </span>
        )}
      </div>
      <nav className="rail-scroll" aria-label="Terminal tabs">
        {/* The live grid, above the tabs and selected like one while it
            shows. Phones have no grid. */}
        {!phone && (
          <div className="nav-cell grid-row">
            <Tooltip content="Live grid (Ctrl-B g; Alt+Tab toggles)">
              <button
                type="button"
                className={`nav-item term-item grid-item ${grid ? "current" : ""}`}
                aria-current={grid ? "page" : undefined}
                aria-label="Grid"
                data-view="grid"
                onClick={() => props.onMode("grid")}
              >
                <span className="term-number" aria-hidden="true">
                  <Icon name="layout-grid" size="sm" />
                </span>
                <span className="grow">Grid</span>
              </button>
            </Tooltip>
          </div>
        )}
        <div className="term-list" role="list" aria-label="Terminals">
          {terminals.map((p) => row(p, false))}
          {!terminals.length && <p className="rail-empty">No open terminals</p>}
        </div>
        {/* Like the TUI's "+ New tab" row, right under the tabs: a rule
            across the row with the label centred on it. */}
        <Tooltip content="New terminal (Ctrl-B Enter)">
          <button
            type="button"
            className="nav-item rail-new"
            aria-label="New terminal"
            onClick={props.onNew}
          >
            <span className="rail-new-rule" aria-hidden="true" />
            <span className="rail-new-label">
              <Icon name="plus" size="sm" />
              <span className="rail-new-text">New terminal</span>
            </span>
            <span className="rail-new-rule" aria-hidden="true" />
          </button>
        </Tooltip>
        {parked.length > 0 && (
          <div className="parked-section">
            <button
              type="button"
              className="rail-label rail-fold"
              aria-expanded={parkedOpen}
              onClick={() => setParkedOpen(!parkedOpen)}
            >
              <Icon
                name={parkedOpen ? "chevron-down" : "chevron-right"}
                size="sm"
              />
              <span className="grow">Parked</span>
              <StateCounts rollup={rollup(parked, notifications)} />
              <span className="count">{parked.length}</span>
            </button>
            {parkedOpen && (
              <div className="term-list" role="list" aria-label="Parked">
                {parked.map((p) => row(p, true))}
              </div>
            )}
          </div>
        )}
      </nav>
      {/* The vitals, then the tools in two columns like the TUI's sidebar
          buttons. */}
      <div className="rail-vitals">
        <Vitals
          vitals={vitals}
          mounts={props.diskMounts}
          folded={folded && !phone}
          onOpen={() => props.onDialog("stats")}
          onDisksHover={props.onDisksHover}
        />
      </div>
      <div className="rail-bottom">
        <div className="tool-grid">
          {tool("terminals", "Terminals", "Terminals", "list", "Ctrl-B o")}
          {tool("recovery", "Recently closed", "Closed", "history", "Ctrl-B r")}
          {tool("search", "Search", "Search", "search", "Ctrl-B /")}
          {tool("help", "Help", "Help", "book-open", "Ctrl-B ?")}
        </div>
      </div>
      <div className="rail-foot">
        {/* The account and its settings, at the very bottom. */}
        <Tooltip content="Settings (Ctrl-B ,)">
          <button
            type="button"
            className="nav-item tool-item account"
            onClick={() => props.onDialog("config")}
            aria-label="Settings"
            data-tool="config"
          >
            <span className="avatar" aria-hidden="true">
              {(username || "?").slice(0, 1).toUpperCase()}
            </span>
            <span className="grow account-name">
              <b>{username || "…"}</b>
            </span>
            <Icon name="settings" size="sm" className="faint" />
          </button>
        </Tooltip>
      </div>
      <div
        className="sidebar-divider"
        onPointerDown={props.onResizeStart}
        title="Click to fold; drag to resize"
      >
        <span aria-hidden="true" />
      </div>
      <button
        type="button"
        className="sidebar-fold"
        onClick={props.onToggleFold}
        aria-label={folded ? "Unfold sidebar" : "Fold sidebar"}
        aria-expanded={!folded}
        title={folded ? "Unfold sidebar" : "Fold sidebar"}
      >
        <Icon name="chevron-right" size="sm" />
      </button>
    </aside>
  );
}
