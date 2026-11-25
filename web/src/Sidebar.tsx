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
import { projectColor, type Project } from "./projects";
import type { ProjectGit } from "./git";
import type { EnvironmentPane } from "./environments";
import "./styles/environments.css";
import { GitMark } from "./Repos";

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
  /** Shows the file browser; given when this connection offers files
   * (KelpServer `files` through the bridge), else there is no Files
   * entry. */
  onFiles?: () => void;
  /** The user configuration file has errors: Settings shows them. */
  configProblem?: boolean;
  /** Projects (bridge API minor 5), in the sidebar's order with Scratch
   * last; with none but Scratch the terminals show as one list. */
  projects?: Project[];
  /** The project a terminal belongs to. */
  projectOf?: (pane: number) => string;
  /** The project under a terminal being dragged. */
  dropProject?: string;
  /** Creates a terminal in a project. */
  onNewIn?: (project: Project) => void;
  /** A project's menu (Files, Terminals, History, Rename, Delete). */
  onProjectMenu?: (project: Project, x: number, y: number) => void;
  onNewProject?: () => void;
  /** A project's Git facts (bridge `git`), for its changes mark. */
  gitOf?: (project: Project) => ProjectGit | undefined;
  /** Opens a project's home page (its name in the group header); without
   * it, the name folds the group like the chevron. */
  onProjectHome?: (project: Project) => void;
  /** The project whose home page shows, selected in the sidebar. */
  homeProject?: string;
  /** The environment a terminal runs (bridge `environment_panes`), shown
   * as a tag so it stands apart from the project's other terminals. */
  environmentOf?: (pane: number) => EnvironmentPane | undefined;
};

/** Projects folded in this browser's sidebar. */
const foldedKey = "kelpmux-folded-projects";
function loadFolded(): string[] {
  try {
    const value = JSON.parse(localStorage.getItem(foldedKey) || "[]");
    return Array.isArray(value) ? value.filter((id) => typeof id === "string") : [];
  } catch {
    return [];
  }
}

/** The left sidebar, like the TUI's, and the only navigation: the state
 * counts, then every terminal in this browser's order with its live state
 * and a rule between terminals, the New terminal row right under them
 * (centred on a rule, like the TUI's "+ New tab") and the parked
 * terminals. A terminal's row opens that terminal alone; the view on
 * screen is selected. A tab with unread notifications shows a bell; the
 * notifications themselves open from the terminal's header. At the
 * bottom, two entries for the other views: **Grid** (the live grid; not
 * on phones) and **Files** (the file browser, when the connection offers
 * files), then the system vitals, the tools in two columns like the TUI's
 * buttons (the Terminals table, recently closed, search, help) and the
 * account with its settings. Order, parking and folding belong to this
 * browser; they never change the TUI's layout. The grid is read-only: a
 * tile's click opens its terminal alone. */
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
  const [foldedProjects, setFoldedProjects] = useState(loadFolded);
  function toggleProject(id: string) {
    const next = foldedProjects.includes(id)
      ? foldedProjects.filter((other) => other !== id)
      : [...foldedProjects, id];
    setFoldedProjects(next);
    try {
      localStorage.setItem(foldedKey, JSON.stringify(next));
    } catch {}
  }
  const projects = props.projects || [];
  /** Grouped by project once there is a project besides Scratch. */
  const grouped = projects.length > 1 && !!props.projectOf;
  const summary = rollup(terminals.concat(parked), notifications);
  /** An entry for another view (the grid, the files): a row like a tab's
   * with an icon in the number column, selected while its view shows. */
  const viewEntry = (options: {
    view: SessionMode;
    label: string;
    icon: IconName;
    tooltip: string;
    onOpen: () => void;
  }) => {
    const on = mode === options.view;
    return (
      <div className="nav-cell view-row">
        <Tooltip content={options.tooltip}>
          <button
            type="button"
            className={`nav-item view-item ${on ? "current" : ""}`}
            aria-current={on ? "page" : undefined}
            aria-label={options.label}
            data-view={options.view}
            onClick={options.onOpen}
          >
            <span className="term-number" aria-hidden="true">
              <Icon name={options.icon} size="sm" />
            </span>
            <span className="grow">{options.label}</span>
          </button>
        </Tooltip>
      </div>
    );
  };
  const row = (p: NumberedPane, isParked: boolean) => {
    const state = paneState(p, notifications);
    const unread = unreadFor(notifications, p).length;
    const title = p.title || "shell";
    const on = current === p.id && !isParked;
    const environment = props.environmentOf?.(p.id);
    return (
      <div
        key={p.id}
        className={`nav-cell term-row ${isParked ? "parked-row" : ""} ${props.rowClass(p)}`}
        data-drop-pane={isParked ? undefined : p.id}
        data-pane={p.id}
        data-project={props.projectOf?.(p.id)}
        data-state={state}
        data-environment={environment?.env}
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
          aria-label={`${isParked ? "Unpark and open" : "Open"} terminal ${p.number || ""} ${title}${environment ? `, environment ${environment.env}` : ""} (${stateLabel[state]}${unread ? `, ${unread} unread` : ""})`}
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
            <small className="term-folder">
              {environment && (
                <span className="env-pane-tag" aria-hidden="true">
                  env {environment.env}
                </span>
              )}{" "}
              {basename(p.cwd)}
            </small>
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
        {/* The tabs, + New terminal and the parked terminals. */}
        {grouped ? (
          projects.map((project) => {
            const members = terminals.filter(
              (p) => props.projectOf!(p.id) === project.id,
            );
            const open = !foldedProjects.includes(project.id);
            const color = projectColor(project);
            return (
              <section
                key={project.id}
                className={`project-group ${props.dropProject === project.id ? "drop-target" : ""}`}
                data-project-group={project.id}
                aria-label={project.scratch ? "Scratch" : `Project ${project.name}`}
                style={color ? ({ "--project": color } as React.CSSProperties) : undefined}
              >
                <div
                  className="project-head"
                  data-drop-project={project.id}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    props.onProjectMenu?.(project, event.clientX, event.clientY);
                  }}
                >
                  <button
                    type="button"
                    className="project-fold"
                    aria-expanded={open}
                    aria-label={`${open ? "Fold" : "Unfold"} ${project.name}`}
                    title={open ? "Fold" : "Unfold"}
                    onClick={() => toggleProject(project.id)}
                  >
                    <Icon
                      name={open ? "chevron-down" : "chevron-right"}
                      size="sm"
                      className="project-chevron"
                    />
                  </button>
                  <button
                    type="button"
                    className={`project-open ${props.homeProject === project.id ? "current" : ""}`}
                    aria-current={
                      props.homeProject === project.id ? "page" : undefined
                    }
                    title={`${project.name}\n${project.path}`}
                    onClick={() =>
                      props.onProjectHome
                        ? props.onProjectHome(project)
                        : toggleProject(project.id)
                    }
                  >
                    <span
                      className={`project-dot ${color ? "" : "plain"}`}
                      aria-hidden="true"
                    />
                    <span className="grow project-name">{project.name}</span>
                    {!open && members.length > 0 && (
                      <StateCounts rollup={rollup(members, notifications)} />
                    )}
                    {props.gitOf && <GitMark git={props.gitOf(project)} />}
                    <span className="count">{members.length || ""}</span>
                  </button>
                  {project.missing && (
                    <Tooltip content={`The folder ${project.path} is missing`}>
                      <span
                        className="project-missing"
                        tabIndex={0}
                        aria-label={`The folder of ${project.name} is missing`}
                      >
                        <Icon name="circle-alert" size="sm" />
                      </span>
                    </Tooltip>
                  )}
                  <span className="project-actions">
                    {props.onNewIn && (
                      <Tooltip content={`New terminal in ${project.name}`}>
                        <button
                          type="button"
                          className="icon-btn"
                          aria-label={`New terminal in ${project.name}`}
                          disabled={project.missing}
                          onClick={() => props.onNewIn!(project)}
                        >
                          <Icon name="plus" size="sm" />
                        </button>
                      </Tooltip>
                    )}
                    {props.onProjectMenu && (
                      <button
                        type="button"
                        className="icon-btn"
                        aria-label={`Menu for ${project.name}`}
                        aria-haspopup="menu"
                        onClick={(event) => {
                          const rect = event.currentTarget.getBoundingClientRect();
                          props.onProjectMenu!(
                            project,
                            Math.max(8, rect.right - 220),
                            rect.bottom,
                          );
                        }}
                      >
                        <Icon name="ellipsis" size="sm" />
                      </button>
                    )}
                  </span>
                </div>
                {open && (
                  <div
                    className="term-list"
                    role="list"
                    aria-label={project.name}
                  >
                    {members.map((p) => row(p, false))}
                  </div>
                )}
              </section>
            );
          })
        ) : (
          <div className="term-list" role="list" aria-label="Terminals">
            {terminals.map((p) => row(p, false))}
            {!terminals.length && (
              <p className="rail-empty">No open terminals</p>
            )}
          </div>
        )}
        {/* Like the TUI's "+ New tab" row, right under the tabs: a rule
            on each side of the label centred between them. Folded, the
            plus between two short rules. */}
        <Tooltip content="New terminal (Ctrl-B Enter)">
          <button
            type="button"
            className="nav-item rail-new"
            aria-label="New terminal"
            aria-keyshortcuts="Control+B Enter"
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
        {props.onNewProject && (
          <button
            type="button"
            className="rail-new-project"
            onClick={props.onNewProject}
          >
            <Icon name="folder-plus" size="sm" />
            <span className="rail-new-text">New project</span>
          </button>
        )}
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
      {/* The other views, at the bottom above the vitals: the live grid
          (phones have none) and the file browser (when the connection
          offers files). */}
      {(!phone || props.onFiles) && (
        <nav className="rail-views" aria-label="Views">
          {!phone &&
            viewEntry({
              view: "grid",
              label: "Grid",
              icon: "layout-grid",
              tooltip: "Live grid (Ctrl-B g; Alt+Tab toggles)",
              onOpen: () => props.onMode("grid"),
            })}
          {props.onFiles &&
            viewEntry({
              view: "files",
              label: "Files",
              icon: "folder",
              tooltip: "File browser",
              onOpen: props.onFiles,
            })}
        </nav>
      )}
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
          {tool("recovery", "History", "History", "history", "Ctrl-B r")}
          {tool("search", "Search", "Search", "search", "Ctrl-B /")}
          {tool("help", "Help", "Help", "book-open", "Ctrl-B ?")}
        </div>
      </div>
      <div className="rail-foot">
        {/* The account and its settings, at the very bottom. */}
        <Tooltip
          content={
            props.configProblem
              ? "Settings (Ctrl-B ,): config.toml has errors"
              : "Settings (Ctrl-B ,)"
          }
        >
          <button
            type="button"
            className="nav-item tool-item account"
            onClick={() => props.onDialog("config")}
            aria-label={
              props.configProblem
                ? "Settings: the configuration file has errors"
                : "Settings"
            }
            data-tool="config"
          >
            <span className="avatar" aria-hidden="true">
              {(username || "?").slice(0, 1).toUpperCase()}
            </span>
            <span className="grow account-name">
              <b>{username || "…"}</b>
            </span>
            {props.configProblem ? (
              <Icon name="circle-alert" size="sm" className="config-alert" />
            ) : (
              <Icon name="settings" size="sm" className="faint" />
            )}
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
