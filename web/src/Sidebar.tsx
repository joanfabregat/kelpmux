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
import { FileTree } from "./files/FileTree";
import type { FileActionsProps } from "./files/actions";

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
  /** The file browser's entry and the home folder's tree, when this
   * connection offers files (KelpServer `files` through the bridge). */
  files?: SidebarFiles;
  /** Which sections are expanded (this browser's preference); without it
   * the sidebar keeps its own, both expanded. */
  sections?: SidebarSections;
  onSections?: (sections: SidebarSections) => void;
};

/** The sidebar's two collapsible sections: the terminals (with + New
 * terminal and the parked ones) and the home folder's tree. */
export type SidebarSections = { terminals: boolean; files: boolean };

/** The Files section: the browser's entry and the home folder's tree,
 * which marks the files open in the editor. */
export type SidebarFiles = {
  /** The browser (not an open file) is shown. */
  browsing: boolean;
  /** The files open in the main area (the Files view's tabs): the tree
   * marks them, and a collapsed header counts them. */
  docs: { path: string; name: string; dirty: boolean; current: boolean }[];
  /** Shows the file browser, at `dir` when given. */
  onBrowse: (dir?: string) => void;
  /** The home folder's tree and its entries' actions; without it the
   * section is its header only. */
  tree?: FileActionsProps;
};

/** The left sidebar, like the TUI's, and the only navigation: the state
 * counts, then two collapsible sections. **Terminals** holds every
 * terminal in this browser's order with its live state, a rule between
 * terminals, the New terminal row right under them (secondary: smaller,
 * muted, indented under the names) and the parked terminals; its header
 * opens the live grid (not on phones). **Files**, when the connection
 * offers files, holds the home folder as a tree whose rows look and
 * behave like the terminals' (FileTree), marking the open files (they are
 * tabs in the Files view, not sidebar rows); its header opens the file
 * browser. Each header's
 * chevron expands or collapses its section, and a collapsed header keeps
 * a count and a dot when something in it needs attention (or is unsaved),
 * so collapsing never hides an alert. The folded rail ignores collapsing.
 * The headers and the terminals' rows look alike. A terminal's row opens
 * that terminal alone; the view on screen is selected. A tab with unread
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
    files,
  } = props;
  const [parkedOpen, setParkedOpen] = useState(true);
  const [ownSections, setOwnSections] = useState<SidebarSections>({
    terminals: true,
    files: true,
  });
  const sections = props.sections || ownSections;
  const toggleSection = (key: keyof SidebarSections) => {
    const next = { ...sections, [key]: !sections[key] };
    if (props.onSections) props.onSections(next);
    else setOwnSections(next);
  };
  // The folded rail shows every row as its icon: collapsing is ignored.
  const terminalsOpen = folded || sections.terminals;
  const filesOpen = folded || sections.files;
  const summary = rollup(terminals.concat(parked), notifications);
  const grid = mode === "grid";
  /** What a collapsed Terminals section still shows: terminals needing
   * input or failed, and unread notifications, parked ones included. */
  const attention = terminals.concat(parked).reduce(
    (total, p) => {
      const state = paneState(p, notifications);
      return {
        states:
          total.states +
          (state === "input_needed" || state === "failed" ? 1 : 0),
        unread: total.unread + unreadFor(notifications, p).length,
      };
    },
    { states: 0, unread: 0 },
  );
  const attentionText = [
    attention.states &&
      `${attention.states} need${attention.states === 1 ? "s" : ""} attention`,
    attention.unread && `${attention.unread} unread`,
  ]
    .filter(Boolean)
    .join(", ");
  const dirtyDocs = files ? files.docs.filter((d) => d.dirty).length : 0;
  /** A section's header: its label opens the section's view, its chevron
   * (a separate button) expands or collapses it. Collapsed, the label
   * keeps a count and, while something inside needs it, a dot. */
  const sectionHeader = (options: {
    key: keyof SidebarSections;
    label: string;
    icon: IconName;
    /** The label's accessible name when it differs from `label`. */
    name?: string;
    tooltip: string;
    view?: string;
    current: boolean;
    open: boolean;
    count: number;
    /** Text of the collapsed section's dot; empty: no dot. */
    alert: string;
    /** Whether the section has anything to expand. */
    toggle: boolean;
    onOpen?: () => void;
  }) => {
    const id = `rail-section-${options.key}`;
    /** The label's accessible name, with what a collapsed section hides. */
    const name = `${options.name || options.label}${
      options.open
        ? ""
        : ` (${options.count}${options.alert ? `, ${options.alert}` : ""})`
    }`;
    const collapsed = !options.open && (
      <span className="section-summary">
        <span className="count">{options.count}</span>
        {options.alert && (
          <span
            className="section-alert"
            role="img"
            aria-label={options.alert}
            title={options.alert}
          />
        )}
      </span>
    );
    const chevron = (
      <button
        type="button"
        className="icon-btn section-toggle"
        aria-expanded={options.open}
        aria-controls={id}
        aria-label={`${options.label} section`}
        title={options.open ? "Collapse" : "Expand"}
        onClick={() => toggleSection(options.key)}
      >
        <Icon
          name={options.open ? "chevron-down" : "chevron-right"}
          size="sm"
        />
      </button>
    );
    return (
      <div className="nav-cell grid-row section-row" data-section={options.key}>
        {options.onOpen ? (
          <Tooltip content={options.tooltip}>
            <button
              type="button"
              className={`nav-item term-item grid-item ${options.current ? "current" : ""}`}
              aria-current={options.current ? "page" : undefined}
              aria-label={name}
              data-view={options.view}
              onClick={options.onOpen}
            >
              <span className="term-number" aria-hidden="true">
                <Icon name={options.icon} size="sm" />
              </span>
              <span className="grow">{options.label}</span>
              {collapsed}
            </button>
          </Tooltip>
        ) : (
          // Nothing to open (the phone's terminals): the whole header
          // toggles the section.
          <button
            type="button"
            className="nav-item term-item grid-item"
            aria-label={name}
            aria-expanded={options.open}
            aria-controls={id}
            onClick={() => toggleSection(options.key)}
          >
            <span className="term-number" aria-hidden="true">
              <Icon name={options.icon} size="sm" />
            </span>
            <span className="grow">{options.label}</span>
            {collapsed}
            <span className="section-chevron" aria-hidden="true">
              <Icon
                name={options.open ? "chevron-down" : "chevron-right"}
                size="sm"
              />
            </span>
          </button>
        )}
        {options.toggle && !folded && options.onOpen && chevron}
      </div>
    );
  };
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
        {/* Terminals: the header opens the live grid (phones have none)
            and is selected like a tab while it shows; then the tabs,
            + New terminal and the parked terminals. */}
        <section className="rail-section" aria-label="Terminals section">
          {sectionHeader({
            key: "terminals",
            label: "Terminals",
            icon: "layout-grid",
            name: phone ? undefined : "Terminals: live grid",
            tooltip: "Live grid (Ctrl-B g; Alt+Tab toggles)",
            view: "grid",
            current: grid && !phone,
            open: terminalsOpen,
            count: terminals.length,
            alert: attentionText,
            toggle: true,
            onOpen: phone ? undefined : () => props.onMode("grid"),
          })}
          {terminalsOpen && (
            <div id="rail-section-terminals" className="rail-section-body">
              <div className="term-list" role="list" aria-label="Terminals">
                {terminals.map((p) => row(p, false))}
                {!terminals.length && (
                  <p className="rail-empty">No open terminals</p>
                )}
              </div>
              {/* Like the TUI's "+ New tab" row, right under the tabs, but
                  secondary: shorter and smaller than a terminal's row,
                  muted, its small plus indented under the names rather
                  than in the number column; a hover brightens the text. */}
              <Tooltip content="New terminal (Ctrl-B Enter)">
                <button
                  type="button"
                  className="nav-item rail-new"
                  aria-label="New terminal"
                  aria-keyshortcuts="Control+B Enter"
                  onClick={props.onNew}
                >
                  <span className="rail-new-plus" aria-hidden="true">
                    <Icon name="plus" />
                  </span>
                  <span className="grow rail-new-text">New terminal</span>
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
            </div>
          )}
        </section>
        {/* Files: the header opens the file browser; the home folder's
            tree follows, marking the open files (a dot while one has
            unsaved changes). The open files themselves are tabs in the
            Files view (FileTabs). The folded rail shows the header only. */}
        {files && (
          <section className="rail-section" aria-label="Files section">
            {sectionHeader({
              key: "files",
              label: "Files",
              icon: "folder",
              tooltip: "File browser",
              view: "files",
              current: mode === "files" && files.browsing,
              open: filesOpen || !files.tree,
              count: files.docs.length,
              alert: dirtyDocs ? `${dirtyDocs} with unsaved changes` : "",
              toggle: !!files.tree,
              onOpen: () => files.onBrowse(),
            })}
            {filesOpen && files.tree && !folded && (
              <div id="rail-section-files" className="rail-section-body">
                {files.tree && (
                  <FileTree
                    actions={files.tree}
                    open={
                      new Map(
                        files.docs.map((d) => [d.path, { dirty: d.dirty }]),
                      )
                    }
                    current={
                      mode === "files"
                        ? files.docs.find((d) => d.current)?.path
                        : undefined
                    }
                    touch={props.touch}
                    onBrowse={(dir) => files.onBrowse(dir)}
                  />
                )}
              </div>
            )}
          </section>
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
