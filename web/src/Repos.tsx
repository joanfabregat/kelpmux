// A project's Repos view and the Git marks other views show (bridge `git`,
// API minor 6): read-only, nothing here commits, stages, fetches or writes.
// - GitMark: a dot and the number of changed files, then the commits not
//   pushed (↑N), with a tooltip listing the checkouts behind them; nothing
//   when every checkout is clean. On a project's sidebar header.
// - CheckoutLine: a checkout's branch and its changes, for the second line
//   of a folder row in the file browser.
// - ReposView: one row per checkout of the project (its folder itself and
//   its direct subfolders that are checkouts) with the branch, upstream,
//   ahead/behind, changed files and last commit; a row expands to its
//   changed files, which open in the Files editor. With the bridge's GitHub
//   operations (API minor 9) it also offers Add repository (GitHub.tsx's
//   picker), shows this project's clones while they run, and GitHub's
//   facts (visibility, default branch, open pull requests, last push) on
//   the rows of checkouts whose origin is on github.com. Cloning is the
//   only write, and the bridge makes it.
import React, { useEffect, useRef, useState } from "react";
import { Icon } from "./status";
import { Tooltip } from "./Tooltip";
import {
  aheadBehind,
  changeKinds,
  changePath,
  changesSummary,
  headLabel,
  projectChanges,
  relativeTime,
  repoChanged,
  shortSha,
  statusLabel,
  type GitChange,
  type GitRepo,
  type ProjectGit,
} from "./git";
import { projectColor, type Project } from "./projects";
import {
  GitHubPicker,
  RepoFacts,
  useGitHubFacts,
  type Rpc,
} from "./GitHub";
import { readGitHubStatus, type GitHubClone, type GitHubFacts } from "./github-state";
import "./styles/repos.css";

/** `+12 −3`, with each side coloured. */
function Lines({ added, removed }: { added: number; removed: number }) {
  return (
    <span className="git-lines">
      <span className="git-added">+{added}</span>
      <span className="git-removed">−{removed}</span>
    </span>
  );
}

/** The tooltip of a project's mark: one line per checkout with changes. */
function MarkTooltip({ repos }: { repos: GitRepo[] }) {
  return (
    <table className="git-tooltip">
      <tbody>
        {repos.map((repo) => (
          <tr key={repo.path}>
            <th scope="row">{repo.name}</th>
            <td>{repo.branch || (repo.head ? shortSha(repo.head) : "")}</td>
            <td>{repo.files ? `${repo.files} changed` : ""}</td>
            <td>{repo.ahead ? `↑${repo.ahead}` : ""}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** A project's changes: uncommitted files and unpushed commits over all
 * its checkouts. Renders nothing while every checkout is clean. */
export function GitMark({
  git,
  focus = "closest",
}: {
  git?: ProjectGit | null;
  focus?: "self" | "closest";
}) {
  const changes = projectChanges(git);
  if (!changes) return null;
  const label = changesSummary(changes);
  return (
    <Tooltip content={<MarkTooltip repos={changes.repos} />} focus={focus}>
      <span
        className="git-mark"
        role="img"
        aria-label={label}
        data-files={changes.files}
        data-ahead={changes.ahead}
        tabIndex={focus === "self" ? 0 : undefined}
      >
        {changes.files > 0 && (
          <span className="git-mark-files" aria-hidden="true">
            <span className="git-dot" />
            {changes.files}
            {changes.partial ? "+" : ""}
          </span>
        )}
        {changes.ahead > 0 && (
          <span className="git-mark-ahead" aria-hidden="true">
            ↑{changes.ahead}
          </span>
        )}
      </span>
    </Tooltip>
  );
}

/** A checkout folder's second line in the file browser: its branch and,
 * when it has any, its changes. */
export function CheckoutLine({ repo }: { repo: GitRepo }) {
  const head = headLabel(repo);
  return (
    <span className="git-checkout-line" data-checkout={repo.path}>
      <Icon name="git-branch" size="sm" className="git-branch-icon" />
      <span className="git-checkout-branch">{head || "no commits"}</span>
      {repoChanged(repo) && (
        <span className="git-checkout-changes">
          {repo.files > 0 && (
            <>
              <span className="git-dot" />
              {repo.files}
            </>
          )}
          {!!repo.ahead && <span>↑{repo.ahead}</span>}
        </span>
      )}
    </span>
  );
}

function ChangeRow({
  repo,
  change,
  onOpen,
}: {
  repo: GitRepo;
  change: GitChange;
  onOpen?: (path: string) => void;
}) {
  const gone = change.status === "D";
  const label = statusLabel(change);
  return (
    <li className="git-change" data-path={change.path} data-status={change.status}>
      <button
        type="button"
        className="git-change-open"
        disabled={gone || !onOpen}
        title={gone ? `${change.path} (deleted)` : `Open ${change.path}`}
        aria-label={`${change.path}, ${label}${gone ? "" : ": open in the editor"}`}
        onClick={() => onOpen?.(changePath(repo, change))}
      >
        <span
          className={`git-status git-status-${change.status === "?" ? "untracked" : change.status.toLowerCase()}`}
          title={label}
          aria-hidden="true"
        >
          {change.status}
        </span>
        <span className="git-change-path">
          {change.from && (
            <span className="git-change-from">{change.from} → </span>
          )}
          {change.path}
        </span>
        {change.added !== null ? (
          <Lines added={change.added} removed={change.removed || 0} />
        ) : (
          <span className="git-lines faint">binary</span>
        )}
      </button>
    </li>
  );
}

function RepoRow({
  repo,
  open,
  onToggle,
  onOpenFile,
  onShowInFiles,
  onNewTerminal,
  isRoot,
  facts,
}: {
  repo: GitRepo;
  /** GitHub's facts about its origin repository, when known. */
  facts?: GitHubFacts;
  open: boolean;
  onToggle: () => void;
  onOpenFile?: (path: string) => void;
  onShowInFiles?: (path: string) => void;
  onNewTerminal?: (path: string) => void;
  isRoot: boolean;
}) {
  const head = headLabel(repo);
  const counts = aheadBehind(repo);
  const changed = repoChanged(repo);
  const listed = repo.changes || [];
  const kinds = changeKinds(repo);
  return (
    <li
      className={`git-repo ${changed ? "changed" : "clean"} ${repo.error ? "error" : ""}`}
      data-repo={repo.name}
      data-path={repo.path}
    >
      <div className="git-repo-head">
        <button
          type="button"
          className="git-repo-toggle"
          aria-expanded={open}
          disabled={!repo.files}
          aria-label={`${repo.name}${head ? ` on ${head}` : ""}: ${repo.error || changesSummary({ ...repo, partial: repo.truncated })}`}
          onClick={onToggle}
        >
          <Icon
            name={open ? "chevron-down" : "chevron-right"}
            size="sm"
            className={`git-chevron ${repo.files ? "" : "hidden"}`}
          />
          <span className="git-repo-names">
            <span className="git-repo-title">
              <b className="git-repo-name">{repo.name}</b>
              {isRoot && <span className="git-tag">project folder</span>}
              {head && (
                <span className={`git-branch ${repo.detached ? "detached" : ""}`}>
                  <Icon name="git-branch" size="sm" />
                  {head}
                </span>
              )}
              {repo.upstream && (
                <span className="git-upstream" title="Upstream, as last fetched (KelpMux never fetches)">
                  {repo.upstream}
                  {counts && <span className="git-ab"> {counts}</span>}
                </span>
              )}
              {!repo.upstream && repo.branch && repo.head && (
                <span className="git-upstream faint">no upstream</span>
              )}
            </span>
            <span className="git-repo-detail">
              {repo.error ? (
                <span className="git-error">{repo.error}</span>
              ) : (
                <>
                  <span className="git-summary">
                    {repo.files
                      ? `${repo.files}${repo.truncated ? "+" : ""} changed${kinds ? ` (${kinds})` : ""}`
                      : "Clean"}
                  </span>
                  {repo.files > 0 && (
                    <Lines added={repo.added} removed={repo.removed} />
                  )}
                  {repo.last && (
                    <span className="git-last" title={new Date(repo.last.time * 1000).toLocaleString()}>
                      <code>{shortSha(repo.last.sha)}</code> {repo.last.subject}
                      <span className="faint"> · {relativeTime(repo.last.time)}</span>
                    </span>
                  )}
                  {!repo.head && <span className="git-last faint">No commits yet</span>}
                </>
              )}
            </span>
            {repo.github && facts?.visible && (
              <span className="git-repo-detail">
                <RepoFacts slug={repo.github} facts={facts} />
              </span>
            )}
          </span>
        </button>
        <span className="git-repo-actions">
          {repo.github && facts?.visible && (
            <Tooltip content="Open on GitHub">
              <a
                className="icon-link"
                href={`https://github.com/${repo.github}`}
                target="_blank"
                rel="noopener noreferrer"
                aria-label={`Open ${repo.github} on GitHub`}
              >
                <Icon name="external-link" size="sm" />
              </a>
            </Tooltip>
          )}
          {onShowInFiles && (
            <Tooltip content="Show in Files">
              <button
                type="button"
                className="icon-btn"
                aria-label={`Show ${repo.name} in Files`}
                onClick={() => onShowInFiles(repo.path)}
              >
                <Icon name="folder-open" size="sm" />
              </button>
            </Tooltip>
          )}
          {onNewTerminal && (
            <Tooltip content="New terminal here">
              <button
                type="button"
                className="icon-btn"
                aria-label={`New terminal in ${repo.name}`}
                onClick={() => onNewTerminal(repo.path)}
              >
                <Icon name="terminal" size="sm" />
              </button>
            </Tooltip>
          )}
        </span>
      </div>
      {open && repo.files > 0 && (
        <ul className="git-changes" aria-label={`Changed files in ${repo.name}`}>
          {listed.map((change) => (
            <ChangeRow
              key={change.path}
              repo={repo}
              change={change}
              onOpen={onOpenFile}
            />
          ))}
          {!repo.changes && <li className="git-more faint">Loading…</li>}
          {repo.changes && repo.files > listed.length && (
            <li className="git-more faint">
              and {repo.files - listed.length} more
            </li>
          )}
        </ul>
      )}
    </li>
  );
}

/** This bridge's clones into `project` (bridge `github_status`), followed
 * every second while one runs; `onDone` runs when one completes. */
function useProjectClones(rpc: Rpc | null | undefined, project: string, onDone: () => void) {
  const [clones, setClones] = useState<GitHubClone[]>([]);
  const [follow, setFollow] = useState(0);
  const done = useRef(onDone);
  done.current = onDone;
  const latest = useRef(rpc);
  latest.current = rpc;
  const offered = !!rpc;
  const running = useRef(new Set<number>());
  useEffect(() => {
    if (!offered) return;
    let alive = true,
      timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const status = readGitHubStatus(await latest.current!("github_status"));
        if (!alive || !status) return;
        const mine = status.clones.filter((c) => c.project === project);
        setClones(mine);
        let finished = false;
        for (const clone of mine) {
          if (clone.state === "running") running.current.add(clone.id);
          else if (running.current.delete(clone.id) && clone.state === "done")
            finished = true;
        }
        if (finished) done.current();
        if (mine.some((c) => c.state === "running"))
          timer = setTimeout(() => void poll(), 1000);
      } catch {
        // Not offered or not connected: nothing to follow.
      }
    }
    void poll();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [offered, project, follow]);
  return {
    clones,
    follow: () => setFollow((n) => n + 1),
    cancel: async (id: number) => {
      try {
        await latest.current?.("github_clone_cancel", { clone: id });
      } finally {
        setFollow((n) => n + 1);
      }
    },
    dismiss: (id: number) => setClones((all) => all.filter((c) => c.id !== id)),
  };
}

/** The clones into this project: running ones with their progress, failed
 * ones with their error. */
function CloneRows({
  clones,
  onCancel,
  onDismiss,
}: {
  clones: GitHubClone[];
  onCancel: (id: number) => void;
  onDismiss: (id: number) => void;
}) {
  const shown = clones.filter((c) => c.state === "running" || c.state === "failed");
  if (!shown.length) return null;
  return (
    <ul className="repos-clones" aria-label="Clones">
      {shown.map((clone) => (
        <li
          key={clone.id}
          className={`repos-clone ${clone.state}`}
          data-clone={clone.full_name}
          role={clone.state === "failed" ? "alert" : "status"}
        >
          <Icon
            name={clone.state === "running" ? "loader-circle" : "circle-alert"}
            size="sm"
            className={clone.state === "running" ? "spin" : ""}
          />
          <span className="grow">
            <b>{clone.full_name}</b>{" "}
            {clone.state === "running"
              ? `· ${clone.progress || "Cloning…"}`
              : `· ${clone.error || "Clone failed"}`}
          </span>
          {clone.state === "running" ? (
            <button type="button" className="subtle small" onClick={() => onCancel(clone.id)}>
              Cancel
            </button>
          ) : (
            <button
              type="button"
              className="icon-btn"
              aria-label={`Dismiss ${clone.full_name}`}
              onClick={() => onDismiss(clone.id)}
            >
              <Icon name="x" size="sm" />
            </button>
          )}
        </li>
      ))}
    </ul>
  );
}

export function ReposView({
  project,
  git,
  loading,
  error,
  onRefresh,
  onOpenFile,
  onShowInFiles,
  onNewTerminal,
  onBack,
  expanded,
  github,
}: {
  project: Project;
  git?: ProjectGit;
  loading: boolean;
  error?: string;
  onRefresh: () => void;
  /** The bridge's GitHub operations (API minor 9), when offered: Add
   * repository, clones and GitHub facts on the rows. */
  github?: Rpc | null;
  /** Opens a changed file in the Files editor (when files are offered). */
  onOpenFile?: (path: string) => void;
  /** Shows a checkout in the file browser (when files are offered). */
  onShowInFiles?: (path: string) => void;
  /** Starts a terminal in a checkout, in this project. */
  onNewTerminal?: (path: string) => void;
  /** A phone's way back to its terminal list. */
  onBack?: () => void;
  /** A checkout (its path) shown expanded when the view opens, as a
   * project's home page asks. */
  expanded?: string;
}) {
  const [open, setOpen] = useState<Set<string>>(
    () => new Set(expanded ? [expanded] : []),
  );
  // The checkout asked for scrolls into view once it is listed.
  const scrolled = useRef(false);
  useEffect(() => {
    if (!expanded || scrolled.current || !git) return;
    scrolled.current = true;
    document
      .querySelector(`.git-repo[data-path="${CSS.escape(expanded)}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [expanded, git]);
  const [picking, setPicking] = useState(false);
  const color = projectColor(project);
  const repos = git?.repos || [];
  const clones = useProjectClones(github, project.id, onRefresh);
  const facts = useGitHubFacts(
    github || null,
    repos.flatMap((repo) => (repo.github ? [repo.github] : [])),
  );
  const canAdd = !!github && !project.missing;
  const addButton = canAdd && (
    <button type="button" className="subtle small" onClick={() => setPicking(true)}>
      <Icon name="plus" size="sm" />
      Add repository
    </button>
  );
  const toggle = (path: string) =>
    setOpen((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  return (
    <section className="repos-view" aria-label={`Repositories of ${project.name}`}>
      <div
        className="project-scope"
        data-scope={project.id}
        style={color ? ({ "--project": color } as React.CSSProperties) : undefined}
      >
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
        <span className={`project-dot ${color ? "" : "plain"}`} aria-hidden="true" />
        <span className="grow">
          <b>{project.name}</b> · Repos
        </span>
        {git && (
          <span className="repos-sampled faint" title="Refreshes every 10 seconds while shown">
            {loading ? "Refreshing…" : `Updated ${new Date(git.sampled_at * 1000).toLocaleTimeString()}`}
          </span>
        )}
        {addButton}
        <Tooltip content="Refresh">
          <button
            type="button"
            className="icon-btn"
            aria-label="Refresh"
            disabled={loading}
            onClick={onRefresh}
          >
            <Icon name="refresh-cw" size="sm" className={loading ? "spin" : ""} />
          </button>
        </Tooltip>
      </div>
      <div className="repos-body">
        {error && (
          <div className="notice error compact" role="alert">
            {error}
          </div>
        )}
        <CloneRows
          clones={clones.clones}
          onCancel={(id) => void clones.cancel(id)}
          onDismiss={clones.dismiss}
        />
        {!git ? (
          !error && (
            <div className="empty-state">
              <Icon name="loader-circle" size="lg" className="spin" />
              <p>Reading the repositories…</p>
            </div>
          )
        ) : !repos.length ? (
          <div className="empty-state">
            <Icon name="git-branch" size="lg" />
            <h3>No Git repositories</h3>
            <p>
              {git.exists
                ? `Neither ${project.path} nor its subfolders are Git checkouts.`
                : `The folder ${project.path} is missing.`}
            </p>
            {addButton}
          </div>
        ) : (
          <>
            <ul className="git-repos" aria-label="Repositories">
              {repos.map((repo) => (
                <RepoRow
                  key={repo.path}
                  repo={repo}
                  facts={repo.github ? facts[repo.github] : undefined}
                  isRoot={repo.path === git.folder && repos.length > 1}
                  open={open.has(repo.path)}
                  onToggle={() => toggle(repo.path)}
                  onOpenFile={onOpenFile}
                  onShowInFiles={onShowInFiles}
                  onNewTerminal={project.missing ? undefined : onNewTerminal}
                />
              ))}
            </ul>
            {git.truncated && (
              <p className="git-more faint">
                Only the first {repos.length} repositories are shown.
              </p>
            )}
            <p className="repos-note faint">
              Read-only: the folder itself and its direct subfolders that are
              Git checkouts. Upstream counts compare with the last fetch;
              KelpMux never fetches.
              {github ? " GitHub facts come from GitHub through the server and refresh every few minutes." : ""}
            </p>
          </>
        )}
      </div>
      {picking && github && (
        <GitHubPicker
          rpc={github}
          project={project}
          existing={(git?.repos || [])
            .filter((repo) => repo.path !== git?.folder)
            .map((repo) => repo.name)}
          onStarted={clones.follow}
          onClose={() => setPicking(false)}
        />
      )}
    </section>
  );
}
