import React, { useState } from "react";
import type { Pane } from "./terminal/types";
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
  type DisplayState,
  type Notification,
} from "./state/model";
import { paletteColor } from "./palette";
import { Vitals } from "./Vitals";
import type { SessionMode } from "./Shell";

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
  /** Phone home screen: no grid and no full width. */
  phone?: boolean;
  /** The terminal view, its grid page and page count. */
  mode: SessionMode;
  page: number;
  pages: number;
  /** Terminals in this browser's order, parked ones excluded. */
  terminals: Pane[];
  /** Terminals parked in this browser. */
  parked: Pane[];
  notifications: Notification[];
  current?: number;
  unread: number;
  vitals: { cpu?: number; memory?: number; disks?: number[] };
  /** Mount points of the vitals' disks, in order, once known. */
  diskMounts: string[];
  onDisksHover: () => void;
  onMode: (mode: SessionMode) => void;
  onPage: (page: number) => void;
  onFullWidth: () => void;
  rowProps: (pane: Pane) => React.HTMLAttributes<HTMLDivElement>;
  rowClass: (pane: Pane) => string;
  /** The pane's tab colour in this browser (a TUI palette index). */
  colorOf: (pane: Pane) => number;
  onOpen: (pane: Pane) => void;
  onClose: (pane: Pane) => void;
  onPark: (pane: Pane) => void;
  onUnpark: (pane: Pane) => void;
  onMenu: (pane: Pane, x: number, y: number) => void;
  onNew: () => void;
  onState: (state: DisplayState) => void;
  onDialog: (dialog: string) => void;
  onResizeStart: (event: React.PointerEvent) => void;
  onToggleFold: () => void;
};

/** The left sidebar, like the TUI's, and the only navigation: the state
 * counts, every terminal in this browser's order with its live state, the
 * new-terminal row right under them and the parked terminals; at the bottom
 * the view and tools (live grid, full width, the Terminals table,
 * notifications, recently closed, search, help, settings), then the system
 * vitals. Order, parking and folding belong to this browser; they never
 * change the TUI's layout. */
export function Sidebar(props: SidebarProps) {
  const {
    username,
    dev,
    folded,
    phone,
    mode,
    page,
    pages,
    terminals,
    parked,
    notifications,
    current,
    unread,
    vitals,
  } = props;
  const [parkedOpen, setParkedOpen] = useState(true);
  const summary = rollup(terminals.concat(parked), notifications);
  const grid = mode === "grid";
  const row = (p: Pane, isParked: boolean) => {
    const state = paneState(p, notifications);
    const title = p.title || "shell";
    const on = current === p.id && !isParked;
    return (
      <div
        key={p.id}
        className={`nav-cell term-row ${isParked ? "parked-row" : ""} ${props.rowClass(p)}`}
        data-drop-pane={isParked ? undefined : p.id}
        data-pane={p.id}
        data-state={state}
        style={
          {
            "--tab-color": paletteColor(props.colorOf(p)),
          } as React.CSSProperties
        }
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
          aria-label={`${isParked ? "Unpark and open" : "Open"} terminal ${p.number || ""} ${title} (${stateLabel[state]})`}
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
  /** A bottom entry: an action with its icon, label and shortcut. */
  const entry = (
    key: string,
    label: string,
    icon: IconName,
    keys: string,
    onClick: () => void,
    extra: { count?: number; pressed?: boolean } = {},
  ) => (
    <Tooltip key={key} content={`${label} (${keys})`}>
      <button
        type="button"
        className="nav-item tool-item"
        aria-label={label}
        aria-pressed={extra.pressed}
        data-tool={key}
        onClick={onClick}
      >
        <Icon name={icon} size="sm" />
        <span className="grow">{label}</span>
        {!!extra.count && (
          <span className="badge accent count">{extra.count}</span>
        )}
      </button>
    </Tooltip>
  );
  const tool = (
    dialog: string,
    label: string,
    icon: IconName,
    keys: string,
    count?: number,
  ) =>
    entry(dialog, label, icon, keys, () => props.onDialog(dialog), { count });
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
        <div className="term-list" role="list" aria-label="Terminals">
          {terminals.map((p) => row(p, false))}
          {!terminals.length && <p className="rail-empty">No open terminals</p>}
        </div>
        {/* Like the TUI's "+" row, right under the tabs. */}
        <Tooltip content="New terminal (Ctrl-B Enter)">
          <button
            type="button"
            className="nav-item rail-new"
            aria-label="New terminal"
            onClick={props.onNew}
          >
            <Icon name="plus" size="sm" />
            <span className="grow">New terminal</span>
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
      <div className="rail-bottom">
        <div className="tool-list">
          {!phone && (
            <div className="nav-cell view-cell">
              {entry(
                "grid",
                "Live grid",
                "layout-grid",
                "Ctrl-B g; Alt+Tab toggles",
                () => props.onMode(grid ? "single" : "grid"),
                { pressed: grid },
              )}
              {grid && pages > 1 && (
                <span className="grid-pages">
                  <Tooltip content="Previous page (Ctrl-B PgUp)">
                    <button
                      type="button"
                      className="icon-btn"
                      aria-label="Previous grid page"
                      disabled={!page}
                      onClick={() => props.onPage(page - 1)}
                    >
                      <Icon name="chevron-left" size="sm" />
                    </button>
                  </Tooltip>
                  <span className="grid-page" aria-live="polite">
                    {page + 1}/{pages}
                  </span>
                  <Tooltip content="Next page (Ctrl-B PgDn)">
                    <button
                      type="button"
                      className="icon-btn"
                      aria-label="Next grid page"
                      disabled={page + 1 >= pages}
                      onClick={() => props.onPage(page + 1)}
                    >
                      <Icon name="chevron-right" size="sm" />
                    </button>
                  </Tooltip>
                </span>
              )}
            </div>
          )}
          {!phone &&
            entry(
              "full-width",
              "Full width",
              "panel-left-close",
              "Ctrl-B f",
              props.onFullWidth,
            )}
          {tool("terminals", "Terminals", "layout-dashboard", "Ctrl-B o")}
          {tool("notifications", "Notifications", "bell", "Ctrl-B b", unread)}
          {tool("recovery", "Recently closed", "history", "Ctrl-B r")}
          {tool("search", "Search", "search", "Ctrl-B /")}
          {tool("help", "Help", "book-open", "Ctrl-B ?")}
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
      </div>
      <div className="rail-foot">
        <Vitals
          vitals={vitals}
          mounts={props.diskMounts}
          folded={folded && !phone}
          onOpen={() => props.onDialog("stats")}
          onDisksHover={props.onDisksHover}
        />
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
