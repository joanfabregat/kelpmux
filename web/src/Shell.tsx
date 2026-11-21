import type { Pane } from "./terminal/types";
import type { IconName } from "./icons";
import { Icon } from "./status";
import { Tooltip } from "./Tooltip";
import { StateMark } from "./state/StateMark";
import { ProviderLogo } from "./state/ProviderLogo";
import { stateLabel, type DisplayState } from "./state/model";
import { diskWarningPercent } from "./Sidebar";

/** One terminal at a time, or all of them live in a grid. */
export type SessionMode = "single" | "grid";

/** Number, state, provider and title of a pane, shared by the grid tiles
 * and the focused terminal's header. */
export function PaneLabel({
  pane,
  state,
  parked = false,
}: {
  pane: Pane;
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
      {pane.parked && (
        <span className="badge parked-badge" title="Parked in the TUI">
          parked in TUI
        </span>
      )}
      {state !== "idle" && state !== "running" && (
        <span className={`badge state-badge state-${state}`}>
          {stateLabel[state]}
        </span>
      )}
    </>
  );
}

/** The bar above the terminals, like KelpHub's: where you are (workspace,
 * then the focused terminal) and the view actions: New terminal,
 * full width and the one terminal / live grid toggle. Everything here is
 * this browser's view; nothing changes the TUI's layout. */
export function TopBar({
  workspace,
  current,
  state,
  mode,
  fullWidth,
  page,
  pages,
  onMode,
  onNewTerminal,
  onFullWidth,
  onPage,
}: {
  workspace: string;
  current?: Pane;
  state?: DisplayState;
  mode: SessionMode;
  fullWidth: boolean;
  page: number;
  pages: number;
  onMode: (mode: SessionMode) => void;
  onNewTerminal: () => void;
  onFullWidth: () => void;
  onPage: (page: number) => void;
}) {
  const modes: [SessionMode, string, IconName][] = [
    ["single", "One terminal (Alt+Tab)", "square"],
    ["grid", "All terminals, live (Ctrl-B g)", "layout-grid"],
  ];
  return (
    <header className="topbar">
      <div className="crumbs">
        {fullWidth && (
          <span className="logo">
            <span className="logo-mark" aria-hidden="true">
              <Icon name="layers" size="sm" />
            </span>
          </span>
        )}
        <span className="crumb" title="Workspace">
          {workspace}
        </span>
        <span className="crumb-sep" aria-hidden="true">
          /
        </span>
        {mode === "grid" ? (
          <span className="crumb current-crumb">
            Live grid
            {pages > 1 && (
              <span className="faint">
                {" "}
                · page {page + 1}/{pages}
              </span>
            )}
          </span>
        ) : current ? (
          <span className="crumb current-crumb" title={current.cwd}>
            {state && <StateMark state={state} />}
            {current.number > 0 && (
              <span className="tab-number">{current.number}</span>
            )}
            {current.title || "shell"}
          </span>
        ) : (
          <span className="crumb current-crumb">No terminal</span>
        )}
      </div>
      <div className="spacer" />
      <div className="term-bar-actions">
        {mode === "grid" && pages > 1 && (
          <div className="pagination">
            <button
              className="icon-btn term-bar-btn"
              aria-label="Previous page (Ctrl-B PgUp)"
              disabled={!page}
              onClick={() => onPage(page - 1)}
            >
              <Icon name="chevron-right" size="sm" className="flip" />
            </button>
            <button
              className="icon-btn term-bar-btn"
              aria-label="Next page (Ctrl-B PgDn)"
              disabled={page + 1 >= pages}
              onClick={() => onPage(page + 1)}
            >
              <Icon name="chevron-right" size="sm" />
            </button>
          </div>
        )}
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
        <Tooltip
          content={
            fullWidth ? "Show the sidebar (Ctrl-B f)" : "Full width (Ctrl-B f)"
          }
        >
          <button
            type="button"
            className="icon-btn term-bar-btn"
            aria-label="Full width"
            aria-pressed={fullWidth}
            onClick={onFullWidth}
          >
            <Icon
              name={fullWidth ? "panel-left-open" : "panel-left-close"}
              size="sm"
            />
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
      </div>
    </header>
  );
}

/** The footer, like the TUI's: account and workspace on the left, the
 * control hint, and live vitals on the right while the sidebar (which shows
 * them otherwise) is hidden or folded. */
export function StatusBar({
  username,
  workspace,
  count,
  parked,
  vitals,
  hint,
  showVitals,
  onStats,
}: {
  username: string;
  workspace: string;
  count: number;
  parked: number;
  vitals: any;
  hint: string;
  showVitals: boolean;
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
        {parked > 0 && ` · ${parked} parked`}
      </span>
      <span className="faint status-hint">{hint}</span>
      <span className="spacer" />
      <span className="faint">Ctrl-B ? help</span>
      {showVitals && (
        <button
          type="button"
          className="vitals-line"
          onClick={onStats}
          aria-label="System status"
        >
          {vital("CPU", vitals?.cpu)}
          {vital("Mem", vitals?.memory)}
          {vital("Disk", disk, diskWarningPercent)}
        </button>
      )}
      {vitals?.time && <span className="vital">{vitals.time}</span>}
    </footer>
  );
}
