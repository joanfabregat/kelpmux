// A project's home page (`?view=project&project=<id>`): what the project
// holds, on one page. Opened from a project group's name in the sidebar or
// its menu's Overview. Its header has the project's colour, name and folder
// (with copy), New terminal, Open in Files, Environments (a link to the
// Environments view, when the bridge offers it) and the project's menu; then
// three sections, each with its count and a See all link to the full view:
// 1. Agents & terminals: the project's live terminals, agents first, each
//    with its state mark and label, title, folder and last activity; a
//    click opens it.
// 2. Repos: one line per checkout (bridge `git`, kept fresh by
//    git-status.ts while the page shows); a click opens the Repos view with
//    that checkout expanded.
// 3. History: its newest entries (bridge `conversations`, or `closed`
//    before API minor 7), a closed terminal collapsed to its newest
//    conversation and their count, each with Resume (only with a known
//    conversation; an unidentified agent terminal offers its agent's
//    picker, labelled; a matched conversation is marked) (conversations.ts,
//    as the History panel).
// Nothing here adds a bridge operation.
import React, { useEffect, useRef, useState } from "react";
import type { NumberedPane } from "@kelpcore/web/types.ts";
import { Icon } from "./status";
import { Tooltip } from "./Tooltip";
import { age } from "./format";
import { StateMark } from "./state/StateMark";
import { ProviderLogo } from "./state/ProviderLogo";
import {
  isAgent,
  paneState,
  stateLabel,
  unreadFor,
  type Notification,
} from "./state/model";
import { agentStateText, agentsFirst } from "./state/terminals";
import { UnreadMark } from "./Shell";
import { aheadBehind, headLabel, type GitRepo, type ProjectGit } from "./git";
import { projectColor, type Project } from "./projects";
import { displayDir } from "./title-location";
import {
  choiceHint,
  choiceText,
  matchedHint,
  pickerHint,
  pickerLabel,
  pickerRequest,
  unidentified,
  conversationTitle,
  exactSpan,
  historyRows,
  restoreRequest,
  resumeRequest,
  span,
  toolName,
  type HistoryListing,
  type HistoryRow,
} from "./conversations";
import "./styles/repos.css";
import "./styles/project-home.css";

/** How many History entries the page shows. */
export const HOME_HISTORY = 8;

/** A section: its title and count, its See all link, then its body. */
function Section({
  id,
  title,
  count,
  icon,
  onSeeAll,
  seeAllLabel,
  children,
}: {
  id: string;
  title: string;
  count?: number;
  icon: React.ComponentProps<typeof Icon>["name"];
  onSeeAll?: () => void;
  seeAllLabel: string;
  children: React.ReactNode;
}) {
  const heading = `home-${id}-title`;
  return (
    <section className="home-card" data-section={id} aria-labelledby={heading}>
      <header className="home-card-head">
        <Icon name={icon} size="sm" className="faint" />
        <h2 id={heading}>{title}</h2>
        {count !== undefined && (
          <span className="home-count" data-count={count}>
            {count}
          </span>
        )}
        <span className="grow" />
        {onSeeAll && (
          <button
            type="button"
            className="home-see-all"
            aria-label={seeAllLabel}
            onClick={onSeeAll}
          >
            See all
            <Icon name="arrow-right" size="sm" />
          </button>
        )}
      </header>
      {children}
    </section>
  );
}

function TerminalLine({
  pane,
  parked,
  notifications,
  home,
  onOpen,
}: {
  pane: NumberedPane;
  parked: boolean;
  notifications: Notification[];
  home?: string;
  onOpen: () => void;
}) {
  const state = paneState(pane, notifications);
  const label = isAgent(pane) ? agentStateText(state) : stateLabel[state];
  const title = pane.title || "shell";
  const unread = unreadFor(notifications, pane).length;
  const activity = (pane as { last_activity?: number }).last_activity;
  return (
    <li>
      <button
        type="button"
        className="home-line home-terminal"
        data-pane={pane.id}
        data-state={state}
        aria-label={`Open ${title} (${label}${parked ? ", parked" : ""})`}
        title={`${title}\n${pane.cwd}`}
        onClick={onOpen}
      >
        <StateMark state={state} decorative />
        <ProviderLogo kind={pane.kind} decorative />
        <span className="home-line-main">
          <span className="home-line-title">{title}</span>
          <span className="home-line-sub mono">{displayDir(pane.cwd, home)}</span>
        </span>
        <span className="home-facts">
          {parked && <span className="badge">parked</span>}
          <UnreadMark count={unread} />
          <span className={`home-state state-${state}`}>{label}</span>
          <span className="home-when" title="Last activity">
            {activity ? age(activity) : "—"}
          </span>
        </span>
      </button>
    </li>
  );
}

function RepoLine({ repo, onOpen }: { repo: GitRepo; onOpen?: () => void }) {
  const head = headLabel(repo);
  const counts = aheadBehind(repo);
  return (
    <li>
      <button
        type="button"
        className={`home-line home-repo ${repo.files ? "changed" : "clean"}`}
        data-repo={repo.name}
        disabled={!onOpen}
        aria-label={`${repo.name}${head ? ` on ${head}` : ""}: ${repo.error || (repo.files ? `${repo.files} changed` : "clean")}${counts ? `, ${counts}` : ""}. Open in Repos`}
        onClick={onOpen}
      >
        <Icon name="folder-git-2" size="sm" className="faint" />
        <span className="home-line-main home-repo-main">
          <b className="home-line-title">{repo.name}</b>
          {head && (
            <span className={`git-branch ${repo.detached ? "detached" : ""}`}>
              <Icon name="git-branch" size="sm" />
              {head}
            </span>
          )}
        </span>
        <span className="home-facts">
          {repo.error ? (
            <span className="git-error">{repo.error}</span>
          ) : repo.files ? (
            <>
              <span className="home-repo-files">
                <span className="git-dot" />
                {repo.files}
                {repo.truncated ? "+" : ""} changed
              </span>
              <span className="git-lines">
                <span className="git-added">+{repo.added}</span>
                <span className="git-removed">−{repo.removed}</span>
              </span>
            </>
          ) : (
            <span className="home-repo-clean faint">Clean</span>
          )}
          {counts && <span className="home-repo-ab">{counts}</span>}
        </span>
      </button>
    </li>
  );
}

export function ProjectHome({
  project,
  projects,
  panes,
  parked,
  notifications,
  home,
  git,
  gitOffered,
  gitError,
  conversations,
  rpc,
  historyKey,
  onOpenPane,
  onNewTerminal,
  onOpenFiles,
  onMenu,
  onCopy,
  onSeeTerminals,
  onSeeRepos,
  onSeeHistory,
  onRestored,
  onOpenEnvironments,
  onBack,
}: {
  project: Project;
  /** The user's projects, to tell Scratch's History entries apart. */
  projects: Project[];
  /** The project's terminals, in the sidebar's order (parked included). */
  panes: NumberedPane[];
  parked: Set<number>;
  notifications: Notification[];
  /** The server user's home folder, to show folders with `~`. */
  home?: string;
  git?: ProjectGit;
  /** The bridge offers `git` (API minor 6). */
  gitOffered: boolean;
  gitError?: string;
  /** The bridge lists conversations (`conversations`, API minor 7). */
  conversations: boolean;
  rpc: (method: string, params?: any) => Promise<any>;
  /** Changes when History may have changed (a terminal closed). */
  historyKey: string;
  onOpenPane: (pane: NumberedPane) => void;
  onNewTerminal: () => void;
  onOpenFiles?: () => void;
  onMenu: (x: number, y: number) => void;
  onCopy: (text: string) => void;
  onSeeTerminals: () => void;
  /** The Repos view, with `repo` (its path) expanded when given. */
  onSeeRepos: (repo?: string) => void;
  onSeeHistory: () => void;
  onRestored: (pane: number, existing: boolean) => void;
  /** The project's Environments view (bridge `environments`, API minor
   * 8; not for Scratch); without it, no Environments button. Only a link:
   * the home page asks nothing about environments. */
  onOpenEnvironments?: () => void;
  /** A phone's way back to its terminal list. */
  onBack?: () => void;
}) {
  const color = projectColor(project);
  const [listing, setListing] = useState<HistoryListing | null>(null);
  const [busy, setBusy] = useState("");
  /** The bridge, read by the listing effect without restarting it. */
  const latestRpc = useRef(rpc);
  latestRpc.current = rpc;
  // Ages ("3m ago") stay current while the page shows.
  const [, setTick] = useState(0);
  useEffect(() => {
    const timer = setInterval(() => setTick((tick) => tick + 1), 30_000);
    return () => clearInterval(timer);
  }, []);
  useEffect(() => {
    let alive = true;
    latestRpc
      .current(conversations ? "conversations" : "closed")
      .then((result) => {
        if (alive) setListing(result || { workspace: "", entries: [] });
      })
      .catch(() => {
        if (alive) setListing((current) => current || { workspace: "", entries: [] });
      });
    return () => {
      alive = false;
    };
  }, [conversations, historyKey]);
  async function run(key: string, method: string, params: any) {
    if (busy) return;
    setBusy(key);
    try {
      const result = await rpc(method, params);
      if (result?.pane) onRestored(result.pane, !!result.existing);
    } finally {
      setBusy("");
    }
  }
  const terminals = agentsFirst(panes);
  const repos = git?.repos || [];
  const rows: HistoryRow[] = listing
    ? historyRows(listing, projects).filter((row) => row.project === project.id)
    : [];
  const recent = rows.slice(0, HOME_HISTORY);
  const resumeButton = (
    key: string,
    label: string,
    folder: string | null | undefined,
    onClick: () => void,
  ) => (
    <button
      type="button"
      className="small success"
      title={folder ? `Resume in a new terminal in ${folder}` : "Its folder no longer exists"}
      aria-label={`Resume ${label}`}
      disabled={!!busy || !folder}
      onClick={onClick}
    >
      <Icon name="rotate-ccw" size="sm" />
      Resume
    </button>
  );
  const historyLine = (row: HistoryRow) => {
    if (row.type === "external") {
      const c = row.conversation;
      const label = conversationTitle(c);
      const key = `external:${c.kind}:${c.id}`;
      return (
        <li key={key} className="home-line home-history" data-conversation={c.id}>
          <ProviderLogo kind={c.kind} />
          <span className="home-line-main">
            <span className="home-line-title" title={label}>
              {label}
            </span>
            <span className="home-line-sub">
              <span className="badge">outside KelpMux</span> {toolName(c.kind)}
            </span>
          </span>
          <span className="home-when" title={exactSpan(c)}>
            {age(c.last_seen || c.first_seen || 0)}
          </span>
          {resumeButton(key, label, c.folder, () => {
            const request = resumeRequest(c, row.project);
            if (request) void run(key, "new", request);
          })}
        </li>
      );
    }
    const entry = row.entry;
    const nested = entry.conversations || [];
    const newest = nested[0];
    if (newest) {
      const label = conversationTitle(newest);
      const key = `${entry.key}:${newest.kind}:${newest.id}`;
      return (
        <li
          key={entry.key}
          className="home-line home-history"
          data-entry={entry.key}
          data-conversation={newest.id}
        >
          <ProviderLogo kind={newest.kind} />
          <span className="home-line-main">
            <span
              className={`home-line-title ${newest.titled === false ? "untitled" : ""}`}
              title={label}
            >
              {label}
            </span>
            <span className="home-line-sub">
              {toolName(newest.kind)} · {entry.name}
              {newest.matched && (
                <>
                  {" · "}
                  <span className="history-matched" title={matchedHint}>
                    matched by folder and time
                  </span>
                </>
              )}
              {nested.length > 1 && (
                <span className="home-conversations" data-conversations={nested.length}>
                  {" "}
                  · {nested.length} conversations
                </span>
              )}
            </span>
          </span>
          <span className="home-when" title={exactSpan(newest) || undefined}>
            {span(newest) ? age(newest.last_seen || newest.first_seen || 0) : age(entry.closed_at || 0)}
          </span>
          {resumeButton(key, label, newest.folder, () => {
            const request = resumeRequest(newest, row.project);
            if (request) void run(key, "new", request);
          })}
        </li>
      );
    }
    // An agent terminal whose conversation is unknown: its agent's picker
    // (with `conversations`), never a guessed Resume.
    const picker = conversations && unidentified(entry);
    const choice = entry.choices[0];
    return (
      <li key={entry.key} className="home-line home-history" data-entry={entry.key}>
        {entry.kind ? (
          <ProviderLogo kind={entry.kind} />
        ) : (
          <Icon name="terminal" size="sm" className="faint" />
        )}
        <span className="home-line-main">
          <span className="home-line-title">{entry.name}</span>
          <span className="home-line-sub">
            <span className="mono">{displayDir(entry.cwd, home)}</span> · {entry.reason}
          </span>
        </span>
        <span className="home-when">{age(entry.closed_at || 0)}</span>
        {picker ? (
          <button
            type="button"
            className="small success"
            title={pickerHint(entry)}
            aria-label={`${pickerLabel(entry.kind)}: ${entry.name}`}
            disabled={!!busy}
            onClick={() => void run(entry.key, "new", pickerRequest(entry, row.project))}
          >
            <Icon name="rotate-ccw" size="sm" />
            {pickerLabel(entry.kind)}
          </button>
        ) : (
          choice && (
            <button
              type="button"
              className="small success"
              title={choiceHint(entry, choice)}
              aria-label={`${choiceText(entry, choice)} ${entry.name}`}
              disabled={!!busy}
              onClick={() => void run(entry.key, "restore", restoreRequest(entry, choice))}
            >
              <Icon name="rotate-ccw" size="sm" />
              {choiceText(entry, choice)}
            </button>
          )
        )}
      </li>
    );
  };
  return (
    <section
      className="project-home"
      data-project-home={project.id}
      aria-label={`${project.name} overview`}
      style={color ? ({ "--project": color } as React.CSSProperties) : undefined}
    >
      <div className="home-scroll">
        <div className="home-column">
          <header className="home-head">
            {onBack && (
              <button
                type="button"
                className="icon-btn"
                aria-label="Back to the terminals"
                onClick={onBack}
              >
                <Icon name="chevron-left" size="sm" />
              </button>
            )}
            <span className={`project-dot home-dot ${color ? "" : "plain"}`} aria-hidden="true" />
            <div className="home-title">
              <h1>{project.name}</h1>
              <span className="home-folder">
                <span className="mono" title={project.path}>
                  {project.folder || project.path}
                </span>
                <Tooltip content="Copy the folder's path">
                  <button
                    type="button"
                    className="icon-btn"
                    aria-label="Copy the folder's path"
                    onClick={() => onCopy(project.path)}
                  >
                    <Icon name="copy" size="sm" />
                  </button>
                </Tooltip>
                {project.missing && (
                  <span className="home-missing">
                    <Icon name="circle-alert" size="sm" /> Folder missing
                  </span>
                )}
              </span>
            </div>
            <div className="home-actions">
              <button
                type="button"
                className="small"
                disabled={project.missing}
                onClick={onNewTerminal}
              >
                <Icon name="plus" size="sm" />
                New terminal
              </button>
              {onOpenFiles && (
                <button
                  type="button"
                  className="small subtle"
                  disabled={project.missing}
                  onClick={onOpenFiles}
                >
                  <Icon name="folder-open" size="sm" />
                  Open in Files
                </button>
              )}
              {onOpenEnvironments && (
                <button
                  type="button"
                  className="small subtle"
                  disabled={project.missing}
                  onClick={onOpenEnvironments}
                >
                  <Icon name="layers" size="sm" />
                  Environments
                </button>
              )}
              <button
                type="button"
                className="icon-btn"
                aria-label={`Menu for ${project.name}`}
                aria-haspopup="menu"
                onClick={(event) => {
                  const rect = event.currentTarget.getBoundingClientRect();
                  onMenu(Math.max(8, rect.right - 220), rect.bottom);
                }}
              >
                <Icon name="ellipsis" size="sm" />
              </button>
            </div>
          </header>

          <Section
            id="terminals"
            title="Agents & terminals"
            count={terminals.length}
            icon="terminal"
            onSeeAll={onSeeTerminals}
            seeAllLabel={`See all terminals of ${project.name}`}
          >
            {terminals.length ? (
              <ul className="home-list">
                {terminals.map((pane) => (
                  <TerminalLine
                    key={pane.id}
                    pane={pane}
                    parked={parked.has(pane.id)}
                    notifications={notifications}
                    home={home}
                    onOpen={() => onOpenPane(pane)}
                  />
                ))}
              </ul>
            ) : (
              <div className="home-empty">
                <p>No terminals in {project.name}.</p>
                <button
                  type="button"
                  className="small"
                  disabled={project.missing}
                  onClick={onNewTerminal}
                >
                  <Icon name="plus" size="sm" />
                  New terminal
                </button>
              </div>
            )}
          </Section>

          {gitOffered && (
            <Section
              id="repos"
              title="Repos"
              count={git ? repos.length : undefined}
              icon="git-branch"
              onSeeAll={() => onSeeRepos()}
              seeAllLabel={`See all repositories of ${project.name}`}
            >
              {gitError && !git ? (
                <p className="home-empty error">{gitError}</p>
              ) : !git ? (
                <p className="home-empty faint">Reading the repositories…</p>
              ) : repos.length ? (
                <ul className="home-list">
                  {repos.map((repo) => (
                    <RepoLine
                      key={repo.path}
                      repo={repo}
                      onOpen={() => onSeeRepos(repo.path)}
                    />
                  ))}
                </ul>
              ) : (
                <p className="home-empty faint">
                  {git.exists
                    ? "No Git checkouts in this folder"
                    : `The folder ${project.path} is missing`}
                </p>
              )}
            </Section>
          )}

          <Section
            id="history"
            title="History"
            count={listing ? rows.length : undefined}
            icon="history"
            onSeeAll={onSeeHistory}
            seeAllLabel={`See all the history of ${project.name}`}
          >
            {!listing ? (
              <p className="home-empty faint">Loading history…</p>
            ) : recent.length ? (
              <ul className="home-list">{recent.map(historyLine)}</ul>
            ) : (
              <p className="home-empty faint">
                Closed terminals and agent conversations in this project will
                appear here.
              </p>
            )}
          </Section>
        </div>
      </div>
    </section>
  );
}
