// The Terminals table: one row per terminal of the workspace, in this
// browser's tab order, with its agent state, folder (and the Git repository
// the web bridge found for it), foreground program and last activity. It
// replaces the separate Agents and Overview dialogs; their shortcuts open it
// with a matching filter.
//
// Pure apart from model.ts, so node tests can load it directly.
import {
  isAgent,
  isFinished,
  needsAttention,
  paneState,
  providerLabel,
  providerOf,
  type DisplayState,
  type Notification,
  type StatePane,
} from "./model.ts";

export type TerminalFilter = "all" | "agents" | "attention" | "finished";
export const TERMINAL_FILTERS: readonly TerminalFilter[] = [
  "all",
  "agents",
  "attention",
  "finished",
];
export const terminalFilterLabel: Record<TerminalFilter, string> = {
  all: "All",
  agents: "Agents",
  attention: "Attention",
  finished: "Finished",
};

/** The repository facts `kelpmux web-bridge` reports for a folder (`repos`). */
export type RepoInfo = {
  root: string;
  branch?: string | null;
  detached?: string | null;
  /** `owner/repo` when the preferred remote is on GitHub. */
  github?: string | null;
};

/** The pane fields the table reads (a subset of KelpServer PaneInfo). */
export type TablePane = StatePane & {
  cwd: string;
  title: string;
  last_activity?: number;
  command?: { started_at: number; ended_at?: number | null } | null;
};

export type TerminalRow<T extends TablePane = TablePane> = {
  pane: T;
  /** Position in this browser's tab order, from 1. */
  position: number;
  parked: boolean;
  name: string;
  state: DisplayState;
  /** The agent's state in the table's words; null for a plain terminal. */
  agent: string | null;
  repo: RepoInfo | null;
  /** The foreground program, the agent's name, or null at a prompt. */
  command: string | null;
  /** When the shell's current command started (seconds), when running. */
  commandSince: number | null;
  lastActivity: number | null;
};

/** Whether a pane passes a filter. Attention and Finished keep the TUI's
 * agent-list semantics: they list agents only. */
export function matchesFilter(pane: StatePane, filter: TerminalFilter) {
  if (filter === "all") return true;
  if (!isAgent(pane) || pane.dead) return false;
  if (filter === "agents") return true;
  return filter === "attention" ? needsAttention(pane) : isFinished(pane);
}

/** An agent's display state as the table words it. */
export function agentStateText(state: DisplayState) {
  switch (state) {
    case "input_needed":
      return "Waiting on you";
    case "failed":
      return "Failed";
    case "finished":
      return "Finished";
    case "working":
    case "running":
      return "Working";
    default:
      return "Idle";
  }
}

/** A GitHub `owner/repo` as its web address, or null for anything else. */
export function githubUrl(github?: string | null) {
  return github && /^[A-Za-z0-9-]{1,39}\/[A-Za-z0-9._-]{1,100}$/.test(github)
    ? `https://github.com/${github}`
    : null;
}

/** The last path component of a repository root. */
export const repoName = (repo: RepoInfo) =>
  repo.root.replace(/\/+$/, "").split("/").pop() || repo.root;

/** Rows for the table: live terminals in tab order, then parked ones, then
 * filtered. Positions count the unfiltered order, so a row keeps its
 * position whichever filter is chosen. */
export function terminalRows<T extends TablePane>(
  ordered: T[],
  parked: Set<number>,
  notifications: Notification[],
  filter: TerminalFilter,
  repos: Map<string, RepoInfo | null> = new Map(),
  commands: Record<string, string | null> = {},
): TerminalRow<T>[] {
  const sorted = [
    ...ordered.filter((p) => !parked.has(p.id)),
    ...ordered.filter((p) => parked.has(p.id)),
  ];
  return sorted
    .map((pane, index) => {
      const state = paneState(pane, notifications);
      const provider = providerOf(pane.kind);
      const running =
        !isAgent(pane) && pane.command && !pane.command.ended_at
          ? pane.command.started_at || null
          : null;
      return {
        pane,
        position: index + 1,
        parked: parked.has(pane.id),
        name: pane.title || "shell",
        state,
        agent: isAgent(pane) ? agentStateText(state) : null,
        repo: repos.get(pane.cwd) || null,
        command:
          commands[String(pane.id)] ||
          (provider ? providerLabel[provider] : null),
        commandSince: running,
        lastActivity: pane.last_activity || null,
      };
    })
    .filter((row) => matchesFilter(row.pane, filter));
}

/** How many terminals each filter lists. */
export function filterCounts(panes: StatePane[]) {
  return Object.fromEntries(
    TERMINAL_FILTERS.map((f) => [
      f,
      panes.filter((p) => matchesFilter(p, f)).length,
    ]),
  ) as Record<TerminalFilter, number>;
}

/** The distinct absolute folders to ask the bridge about, at most `limit`
 * (the bridge's per-call bound). */
export function repoQueryPaths(panes: { cwd: string }[], limit = 64) {
  return [
    ...new Set(panes.map((p) => p.cwd).filter((cwd) => cwd.startsWith("/"))),
  ].slice(0, limit);
}

/** A duration in seconds as a short label: "45s", "12m", "3h", "2d". */
export function shortDuration(seconds: number) {
  const s = Math.max(0, Math.floor(seconds));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}
