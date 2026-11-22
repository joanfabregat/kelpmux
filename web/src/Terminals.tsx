// The Terminals dialog: every terminal of the workspace in one table (cards
// on a phone), with its agent state, folder and repository, foreground
// program and last activity. Clicking a row opens the terminal. Repository
// and program facts come from read-only `kelpmux web-bridge` operations
// (`repos`, `commands`) when the bridge offers them; the table works
// without them.
import { useEffect, useMemo, useState } from "react";
import type { Pane } from "./terminal/types";
import { Icon } from "./status";
import { age } from "./format";
import { StateMark } from "./state/StateMark";
import { ProviderLogo } from "./state/ProviderLogo";
import type { Notification } from "./state/model";
import {
  TERMINAL_FILTERS,
  filterCounts,
  githubUrl,
  repoName,
  repoQueryPaths,
  shortDuration,
  terminalFilterLabel,
  terminalRows,
  type RepoInfo,
  type TerminalFilter,
  type TerminalRow,
} from "./state/terminals";

/** How often the foreground programs refresh while the dialog is open. */
const COMMAND_REFRESH_MS = 3000;
/** How often repository facts refresh while the dialog is open. */
const REPO_REFRESH_MS = 10000;

/** A simple monochrome mark drawn for KelpMux, in the style of the provider
 * marks: a cat's head with ears, standing for GitHub. Not GitHub's own
 * artwork. */
export function GitHubMark() {
  return (
    <svg
      className="github-mark"
      viewBox="0 0 16 16"
      role="img"
      aria-label="GitHub"
      focusable="false"
    >
      <title>GitHub</title>
      <path
        d="M3.2 6.3 3.5 2.4l2.6 1.9a7 7 0 0 1 3.8 0l2.6-1.9.3 3.9c.7.8 1 1.7 1 2.7 0 2.7-2.4 4.2-5.8 4.2S2.2 11.7 2.2 9c0-1 .3-1.9 1-2.7Z"
        fill="currentColor"
      />
      <circle cx="5.9" cy="8.6" r="0.9" className="github-mark-eye" />
      <circle cx="10.1" cy="8.6" r="0.9" className="github-mark-eye" />
    </svg>
  );
}

type Query = (method: string, params?: any) => Promise<any>;

/** Repository facts for the panes' folders and their foreground programs,
 * fetched while the dialog is open. Failures leave the columns plain. */
function useBridgeFacts(panes: Pane[], query: Query, operations: Set<string>) {
  const [repos, setRepos] = useState(new Map<string, RepoInfo | null>());
  const [commands, setCommands] = useState<Record<string, string | null>>({});
  const paths = repoQueryPaths(panes);
  const key = paths.join("\n");
  useEffect(() => {
    if (!operations.has("repos") || !paths.length) return;
    let alive = true;
    const load = () =>
      query("repos", { paths }).then((result) => {
        const list: (RepoInfo | null)[] = result?.repos || [];
        if (!alive || !Array.isArray(list)) return;
        setRepos(new Map(paths.map((path, i) => [path, list[i] || null])));
      });
    load();
    // Branches change while the dialog is open; the bridge caches nothing.
    const timer = setInterval(load, REPO_REFRESH_MS);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [key, operations]);
  useEffect(() => {
    if (!operations.has("commands")) return;
    let alive = true;
    const load = () =>
      query("commands").then((result) => {
        if (alive && result?.commands) setCommands(result.commands);
      });
    load();
    const timer = setInterval(load, COMMAND_REFRESH_MS);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [operations]);
  return { repos, commands };
}

function Folder({ row }: { row: TerminalRow<Pane> }) {
  const { repo } = row;
  const url = githubUrl(repo?.github);
  return (
    <span className="terminal-folder">
      <span className="mono terminal-path" title={row.pane.cwd}>
        {row.pane.cwd || "—"}
      </span>
      {repo && (
        <span className="terminal-repo">
          {url ? (
            <a
              href={url}
              target="_blank"
              rel="noopener noreferrer"
              className="terminal-github"
              title={`Open ${repo.github} on GitHub`}
              onClick={(event) => event.stopPropagation()}
              onKeyDown={(event) => event.stopPropagation()}
            >
              <GitHubMark />
              {repo.github}
            </a>
          ) : (
            <span className="terminal-reponame" title={repo.root}>
              <Icon name="folder" size="sm" />
              {repoName(repo)}
            </span>
          )}
          {(repo.branch || repo.detached) && (
            <span
              className="terminal-branch mono"
              title={repo.branch ? "Branch" : "Detached HEAD"}
            >
              <Icon name="git-branch" size="sm" />
              {repo.branch || repo.detached}
            </span>
          )}
        </span>
      )}
    </span>
  );
}

function Agent({ row }: { row: TerminalRow<Pane> }) {
  if (!row.agent) return <span className="faint">—</span>;
  return (
    <span className="terminal-agent">
      <ProviderLogo kind={row.pane.kind} />
      <StateMark state={row.state} label={row.agent} decorative />
      <span>{row.agent}</span>
    </span>
  );
}

function Command({ row }: { row: TerminalRow<Pane> }) {
  if (!row.command) return <span className="faint">—</span>;
  const now = Date.now() / 1000;
  return (
    <span className="terminal-command">
      <span className="mono">{row.command}</span>
      {row.commandSince != null && (
        <small> · {shortDuration(now - row.commandSince)}</small>
      )}
    </span>
  );
}

const activity = (row: TerminalRow<Pane>) =>
  row.lastActivity ? age(row.lastActivity) : "—";

export function TerminalsPanel({
  panes,
  parked,
  notifications,
  filter,
  setFilter,
  operations,
  query,
  cards,
  onOpen,
}: {
  /** Every terminal, in this browser's order (parked ones included). */
  panes: Pane[];
  parked: Set<number>;
  notifications: Notification[];
  filter: TerminalFilter;
  setFilter: (filter: TerminalFilter) => void;
  /** Bridge operations available (`state`'s `operations`). */
  operations: Set<string>;
  /** A quiet bridge request: resolves to null on failure. */
  query: Query;
  /** Show cards instead of a table (phones). */
  cards: boolean;
  onOpen: (pane: Pane) => void;
}) {
  const { repos, commands } = useBridgeFacts(panes, query, operations);
  const rows = terminalRows(
    panes,
    parked,
    notifications,
    filter,
    repos,
    commands,
  );
  const counts = useMemo(() => filterCounts(panes), [panes]);
  const label = (row: TerminalRow<Pane>) =>
    `Open terminal ${row.pane.number || ""} ${row.name}${row.parked ? " (parked)" : ""}`.replace(
      /\s+/g,
      " ",
    );
  const number = (row: TerminalRow<Pane>) =>
    row.parked ? "–" : row.pane.number || "·";
  return (
    <div className="panel-body">
      <div className="seg panel-seg" role="group" aria-label="Terminal filter">
        {TERMINAL_FILTERS.map((value) => (
          <button
            key={value}
            aria-pressed={filter === value}
            onClick={() => setFilter(value)}
          >
            {terminalFilterLabel[value]}
            <span className="count">{counts[value]}</span>
          </button>
        ))}
      </div>
      {cards ? (
        <div className="terminal-cards" role="list">
          {rows.map((row) => (
            <div
              className="terminal-card"
              role="listitem"
              key={row.pane.id}
              data-pane={row.pane.id}
              onClick={() => onOpen(row.pane)}
            >
              <button
                type="button"
                className="terminal-card-head"
                aria-label={label(row)}
              >
                <span className="term-number">{number(row)}</span>
                <span className="grow terminal-name">{row.name}</span>
                <small>{activity(row)}</small>
              </button>
              {row.agent && <Agent row={row} />}
              <Folder row={row} />
              {row.command && <Command row={row} />}
            </div>
          ))}
        </div>
      ) : (
        <div className="info-table-wrap terminals-wrap">
          <table className="info-table terminals-table">
            <thead>
              <tr>
                <th>Terminal</th>
                <th>Agent</th>
                <th>Folder</th>
                <th>Running</th>
                <th>Last activity</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr
                  key={row.pane.id}
                  data-pane={row.pane.id}
                  className={row.parked ? "parked-row" : ""}
                  onClick={() => onOpen(row.pane)}
                >
                  <th scope="row">
                    <button
                      type="button"
                      className="terminal-open"
                      aria-label={label(row)}
                    >
                      <span className="term-number">{number(row)}</span>
                      <span className="terminal-name">{row.name}</span>
                    </button>
                    {row.parked && <small>Parked</small>}
                  </th>
                  <td>
                    <Agent row={row} />
                  </td>
                  <td>
                    <Folder row={row} />
                  </td>
                  <td>
                    <Command row={row} />
                  </td>
                  <td>{activity(row)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {!rows.length && (
        <p className="panel-empty">
          {filter === "all"
            ? "No terminals are open."
            : filter === "agents"
              ? "No Codex or Claude Code agents are running."
              : `No agents ${filter === "attention" ? "need attention" : "have finished"}.`}
        </p>
      )}
    </div>
  );
}
