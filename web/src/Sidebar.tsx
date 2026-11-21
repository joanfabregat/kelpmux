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

/** Disk usage, in percent, from which the system status meter warns. */
export const diskWarningPercent = 90;

/** What a state shortcut in the sidebar summary opens. */
const openStateLabel: Partial<Record<DisplayState, string>> = {
  input_needed: "Next terminal that needs input (Ctrl-B N)",
  failed: "Next failed terminal (Ctrl-B N)",
  finished: "Next finished terminal (Ctrl-B P)",
  working: "Agents (Ctrl-B a)",
};

export type SidebarProps = {
  workspace: string;
  username: string;
  dev: boolean;
  folded: boolean;
  /** Terminals in this browser's order, parked ones excluded. */
  terminals: Pane[];
  /** Terminals parked in this browser. */
  parked: Pane[];
  notifications: Notification[];
  current?: number;
  unread: number;
  vitals: { cpu?: number; memory?: number; disks?: number[] };
  rowProps: (pane: Pane) => React.HTMLAttributes<HTMLDivElement>;
  rowClass: (pane: Pane) => string;
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

/** The left sidebar, like the TUI's: every terminal of the workspace in this
 * browser's order with its live state, parked
 * terminals, the workspace tools (agents, notifications, recently closed,
 * overview, search), system vitals and settings. Order, parking and folding
 * belong to this browser; they never change the TUI's layout. */
export function Sidebar(props: SidebarProps) {
  const {
    workspace,
    username,
    dev,
    folded,
    terminals,
    parked,
    notifications,
    current,
    unread,
    vitals,
  } = props;
  const [parkedOpen, setParkedOpen] = useState(true);
  const summary = rollup(terminals.concat(parked), notifications);
  const disks = vitals.disks || [];
  const disk = disks.length ? Math.max(...disks) : undefined;
  const vital = (label: string, value?: number, warn = 87) => (
    <div className="vital">
      <span>
        {label} <b>{value == null ? "—" : `${value}%`}</b>
      </span>
      <div className="meter">
        <i
          className={value != null && value >= warn ? "warn" : ""}
          style={{ width: `${Math.min(100, value || 0)}%` }}
        />
      </div>
    </div>
  );
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
            "--tab-color": paletteColor(p.palette),
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
          {isParked ? (
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
        </span>
      </div>
    );
  };
  const tool = (
    dialog: string,
    label: string,
    icon: IconName,
    keys: string,
    count?: number,
  ) => (
    <Tooltip key={dialog} content={`${label} (${keys})`}>
      <button
        type="button"
        className="nav-item tool-item"
        aria-label={label}
        data-tool={dialog}
        onClick={() => props.onDialog(dialog)}
      >
        <Icon name={icon} size="sm" />
        <span className="grow">{label}</span>
        {!!count && <span className="badge accent count">{count}</span>}
      </button>
    </Tooltip>
  );
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
      </div>
      <nav className="rail-scroll">
        <div className="rail-label rail-terminals-label">
          <span className="grow">{workspace || "Terminals"}</span>
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
          <Tooltip content="New terminal (Ctrl-B Enter)">
            <button
              type="button"
              className="icon-btn rail-new"
              aria-label="New terminal"
              onClick={props.onNew}
            >
              <Icon name="plus" size="sm" />
            </button>
          </Tooltip>
        </div>
        <div className="term-list" role="list" aria-label="Terminals">
          {terminals.map((p) => row(p, false))}
          {!terminals.length && <p className="rail-empty">No open terminals</p>}
        </div>
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
        <div className="rail-label">
          <span>Workspace</span>
        </div>
        <div className="tool-list">
          {tool("agents", "Agents", "bot", "Ctrl-B a")}
          {tool("notifications", "Notifications", "bell", "Ctrl-B b", unread)}
          {tool("recovery", "Recently closed", "history", "Ctrl-B r")}
          {tool("overview", "Overview", "layout-dashboard", "Ctrl-B o")}
          {tool("search", "Search", "search", "Ctrl-B /")}
          {tool("help", "Help", "book-open", "Ctrl-B ?")}
        </div>
      </nav>
      <div className="rail-foot">
        <button
          type="button"
          className="vitals metrics"
          onClick={() => props.onDialog("stats")}
          aria-label="System status"
          title="System status"
        >
          {vital("CPU", vitals.cpu)}
          {vital("Mem", vitals.memory)}
          {vital("Disk", disk, diskWarningPercent)}
        </button>
        <button
          type="button"
          className="account"
          onClick={() => props.onDialog("config")}
          aria-label="Settings"
          title="Settings (Ctrl-B ,)"
        >
          <span className="avatar" aria-hidden="true">
            {(username || "?").slice(0, 1).toUpperCase()}
          </span>
          <span className="account-name">
            <b>{username || "…"}</b>
            <small>{workspace}</small>
          </span>
          <Icon name="settings" size="sm" className="faint" />
        </button>
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
