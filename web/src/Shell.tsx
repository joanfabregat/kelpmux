import React, { useEffect, useRef } from "react";
import type { Pane } from "./terminal/types";
import type { IconName } from "./icons";
import { Icon } from "./status";
import { Tooltip } from "./Tooltip";
import { stateLabel } from "./activity-icon";

/** Disk usage, in percent, from which the system status meter warns. */
export const diskWarningPercent = 90;

/** One terminal at a time, or all of them live in a grid. */
export type SessionMode = "single" | "grid";

/** A pane's live KelpServer status as a small shape-and-colour mark. */
export function StatusDot({ status }: { status: string }) {
  return (
    <span
      className={`status-dot status-${status}`}
      role="img"
      aria-label={stateLabel[status] || status}
    />
  );
}

/** Number, title and status of a pane, shared by tabs and pane headers. */
export function PaneLabel({ pane }: { pane: Pane }) {
  const status = pane.status || "unknown";
  return (
    <>
      <StatusDot status={status} />
      {pane.number > 0 && <strong className="number">{pane.number}</strong>}
      <span className="grid-title">{pane.title || "shell"}</span>
      {pane.kind && <span className="badge kind-badge">{pane.kind}</span>}
      {pane.parked && (
        <span className="badge parked-badge" title="Parked in the TUI">
          parked
        </span>
      )}
      {status !== "idle" && status !== "unknown" && (
        <span className={`badge state-badge state-${status}`}>
          {stateLabel[status] || status}
        </span>
      )}
    </>
  );
}

/** The bar above the terminals: brand, one tab per terminal in this
 * browser's order, New terminal, the view toggle, system status and
 * settings. Tab order, the view and focus are this browser's own; they
 * never change the TUI's layout. */
export function TopBar({
  workspace,
  dev,
  terminals,
  current,
  mode,
  tabProps,
  tabClass,
  onMode,
  onTerminal,
  onClose,
  onNewTerminal,
  onStats,
  onSettings,
}: {
  workspace: string;
  dev: boolean;
  terminals: Pane[];
  current?: number;
  mode: SessionMode;
  tabProps: (pane: Pane) => React.HTMLAttributes<HTMLDivElement>;
  tabClass: (pane: Pane) => string;
  onMode: (mode: SessionMode) => void;
  onTerminal: (pane: Pane) => void;
  onClose: (pane: Pane) => void;
  onNewTerminal: () => void;
  onStats: () => void;
  onSettings: () => void;
}) {
  const tabsRef = useRef<HTMLElement>(null);
  useEffect(() => {
    tabsRef.current
      ?.querySelector<HTMLElement>(".term-tab.on")
      ?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [current, mode, terminals.length]);
  const modes: [SessionMode, string, IconName][] = [
    ["single", "One terminal", "square"],
    ["grid", "All terminals, live", "layout-grid"],
  ];
  return (
    <header className="topbar">
      <div className="crumbs">
        <span className="logo">
          <span className="logo-mark" aria-hidden="true">
            <Icon name="layers" size="sm" />
          </span>
          <span className="logo-text">KelpMux</span>
        </span>
        <span className="crumb current-crumb" title="Workspace">
          {workspace}
        </span>
        {dev && <span className="env-badge">DEV</span>}
      </div>
      <nav className="term-tabs" aria-label="Terminals" ref={tabsRef}>
        {terminals.map((p) => {
          const title = p.title || "shell";
          const on = current === p.id;
          return (
            <div
              key={p.id}
              className={`term-tab ${on ? "on" : ""} ${tabClass(p)}`}
              data-drop-pane={p.id}
              data-pane={p.id}
              data-number={p.number}
              data-state={p.status}
              title={`${title}\n${p.cwd}`}
              {...tabProps(p)}
              // Middle click closes, like a browser tab; pressing the
              // middle button must not start autoscroll or a paste.
              onMouseDown={(event) => {
                if (event.button === 1) event.preventDefault();
              }}
              onAuxClick={(event) => {
                if (event.button !== 1) return;
                event.preventDefault();
                event.stopPropagation();
                onClose(p);
              }}
            >
              <button
                type="button"
                className="term-tab-open"
                aria-current={
                  on ? (mode === "single" ? "page" : "true") : undefined
                }
                aria-label={
                  mode === "grid"
                    ? `Focus terminal ${title} in the grid`
                    : `Open terminal ${title}`
                }
                onClick={() => onTerminal(p)}
              >
                <StatusDot status={p.status} />
                {p.number > 0 && <span className="tab-number">{p.number}</span>}
                <span className="term-tab-title">{title}</span>
              </button>
              <Tooltip content="Close terminal (middle-click)">
                <button
                  type="button"
                  className="icon-btn term-tab-close"
                  aria-label={`Close terminal ${title}`}
                  onClick={(event) => {
                    event.stopPropagation();
                    onClose(p);
                  }}
                >
                  <Icon name="x" size="sm" />
                </button>
              </Tooltip>
            </div>
          );
        })}
      </nav>
      <div className="term-bar-actions">
        <Tooltip content="New terminal (Ctrl-B Enter)">
          <button
            type="button"
            className="icon-btn term-bar-btn term-tab-new"
            aria-label="New terminal"
            onClick={onNewTerminal}
          >
            <Icon name="plus" size="sm" />
          </button>
        </Tooltip>
        <div
          className="seg view-toggle"
          role="group"
          aria-label="Terminal view"
        >
          {modes.map(([value, label, icon]) => (
            <Tooltip key={value} content={label}>
              <button
                type="button"
                data-mode={value}
                aria-pressed={mode === value}
                aria-label={label}
                onClick={() => onMode(value)}
              >
                <Icon name={icon} size="sm" />
              </button>
            </Tooltip>
          ))}
        </div>
        <Tooltip content="System status">
          <button
            type="button"
            className="icon-btn term-bar-btn"
            aria-label="System status"
            onClick={onStats}
          >
            <Icon name="activity" size="sm" />
          </button>
        </Tooltip>
        <Tooltip content="Settings (Ctrl-B ,)">
          <button
            type="button"
            className="icon-btn term-bar-btn"
            aria-label="Settings"
            onClick={onSettings}
          >
            <Icon name="settings" size="sm" />
          </button>
        </Tooltip>
      </div>
    </header>
  );
}

/** The footer, like the TUI's: account and workspace on the left, live
 * system vitals on the right. */
export function StatusBar({
  username,
  workspace,
  count,
  vitals,
  hint,
  onStats,
}: {
  username: string;
  workspace: string;
  count: number;
  vitals: any;
  hint: string;
  onStats: () => void;
}) {
  const disks: number[] = vitals?.disks || [];
  const disk = disks.length ? Math.max(...disks) : undefined;
  const vital = (label: string, value?: number, warn = 87) => (
    <span className={`vital ${value != null && value >= warn ? "warn" : ""}`}>
      {label} <b>{value == null ? "—" : `${value}%`}</b>
    </span>
  );
  return (
    <footer className="status-bar">
      <span className="mono">
        {username}@{workspace}
      </span>
      <span>
        {count} {count === 1 ? "terminal" : "terminals"}
      </span>
      <span className="faint status-hint">{hint}</span>
      <span className="spacer" />
      <button
        type="button"
        className="vitals-line"
        onClick={onStats}
        aria-label="System status"
      >
        {vital("CPU", vitals?.cpu)}
        {vital("Mem", vitals?.memory)}
        {vital("Disk", disk, diskWarningPercent)}
        {vitals?.time && <span className="vital">{vitals.time}</span>}
      </button>
    </footer>
  );
}
