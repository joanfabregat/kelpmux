// GitHub in the web client (bridge `github_*`, API minor 9), with the
// GitHub access the server already has; the repository picker and the
// facts are adapted from KelpHub's:
// - GitHubSettings: Settings → GitHub. Where the bridge's token comes
//   from (`[github]` in config.toml), who it signs in as, its scopes and
//   rate limit, whether Git has an SSH agent, and Test.
// - GitHubPicker: a project's "Add repository" dialog: a clone URL, or the
//   repositories the token can see, searchable, cloned over SSH (the
//   default) or HTTPS into `<project folder>/<name>` with the user's own
//   Git credentials.
// - useGitHubFacts and RepoFacts: read-only facts about the GitHub
//   repository a checkout comes from (visibility, default branch, open
//   pull requests, last push), for the Repos view's rows.
// Every GitHub request and every clone is the bridge's; the browser gets
// public metadata, never the token.
import { useEffect, useRef, useState } from "react";
import { Icon } from "./status";
import { openSettings } from "./Settings";
import { relativeTime } from "./git";
import type { Project } from "./projects";
import {
  cloneName,
  filterRepositories,
  formatSize,
  LARGE_KIB,
  MAX_CLONE_KIB,
  readFacts,
  readGitHubStatus,
  readRepositories,
  sourceText,
  type GitHubFacts,
  type GitHubRepository,
  type GitHubStatus,
} from "./github-state";
import "./styles/github.css";

/** A bridge request that rejects with the bridge's error. */
export type Rpc = (method: string, params?: any) => Promise<any>;

const message = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

const CONFIG_EXAMPLE = `[github]
# One of:
auth = "gh"                            # gh auth token
# auth = "env"                         # GH_TOKEN or GITHUB_TOKEN
# token_command = "~/bin/github-token" # prints the token
# For SSH clones from the web client:
# ssh_auth_sock = "~/.ssh/agent.sock"`;

/** How to configure a token source, for a signed-out status. */
function SignInHelp() {
  return (
    <div className="notice github-setup">
      <p>
        Choose where the server's GitHub token comes from in{" "}
        <code>[github]</code> of config.toml, then check the file with{" "}
        <code>kelpmux config check</code>:
      </p>
      <pre>{CONFIG_EXAMPLE}</pre>
    </div>
  );
}

/** Polls `github_status` every `slow` ms, every second while busy. */
function useGitHubStatus(rpc: Rpc | null, slow = 10000) {
  const [status, setStatus] = useState<GitHubStatus | null>(null),
    [error, setError] = useState(""),
    [tick, setTick] = useState(0);
  const latest = useRef(rpc);
  latest.current = rpc;
  const offered = !!rpc;
  useEffect(() => {
    if (!offered) return;
    let alive = true,
      timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const s = readGitHubStatus(await latest.current!("github_status"));
        if (!alive) return;
        if (s) setStatus(s);
        setError("");
        timer = setTimeout(poll, s?.busy ? 1000 : slow);
      } catch (e) {
        if (!alive) return;
        setError(message(e));
        timer = setTimeout(poll, slow);
      }
    }
    void poll();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [offered, tick, slow]);
  return {
    status,
    setStatus,
    error,
    setError,
    /** Polls again now (after an action that starts work). */
    follow: () => setTick((n) => n + 1),
  };
}

/** Settings → GitHub. */
export function GitHubSettings({ rpc }: { rpc: Rpc | null }) {
  const { status, setStatus, error, setError, follow } = useGitHubStatus(rpc);
  async function test() {
    setError("");
    try {
      const s = readGitHubStatus(await rpc!("github_test"));
      if (s) setStatus(s);
      follow();
    } catch (e) {
      setError(message(e));
    }
  }
  const head = (
    <>
      <h3>GitHub</h3>
      <p className="setting-hint">
        KelpMux uses the GitHub access this server already has: it lists the
        repositories your account can see, shows facts about the GitHub
        repositories of a project's checkouts, and clones with your own Git
        credentials. The token stays on the server, in memory for a few
        minutes; this browser never sees it.
      </p>
    </>
  );
  const problem = (error || status?.error) && (
    <p className="notice error" role="alert">
      {error || status?.error}
    </p>
  );
  if (!rpc)
    return (
      <>
        {head}
        <p className="setting-hint">
          This kelpmux web bridge does not offer GitHub. Install a matching
          kelpmux on the server.
        </p>
      </>
    );
  if (!status)
    return (
      <>
        {head}
        {problem || <p className="setting-hint">Loading…</p>}
      </>
    );
  const rate = status.rate;
  return (
    <>
      {head}
      {!status.enabled ? (
        <>
          <p>
            <strong>Not signed in.</strong> No token source is configured.
          </p>
          <SignInHelp />
        </>
      ) : (
        <>
          <dl className="github-facts-list">
            <dt>Token from</dt>
            <dd data-github-source={status.source}>{sourceText(status)}</dd>
            <dt>Signed in as</dt>
            <dd>
              {status.signed_in && status.login ? (
                <>
                  <strong>{status.login}</strong>
                  {status.name ? ` (${status.name})` : ""}
                </>
              ) : status.busy ? (
                "Checking…"
              ) : (
                "Not signed in"
              )}
            </dd>
            {status.signed_in && (
              <>
                <dt>Scopes</dt>
                <dd>
                  {status.scopes === null
                    ? "Not reported (fine-grained or app token)"
                    : status.scopes.length
                      ? status.scopes.map((s) => (
                          <span key={s} className="git-tag">
                            {s}
                          </span>
                        ))
                      : "None"}
                </dd>
                <dt>Rate limit</dt>
                <dd>
                  {rate
                    ? `${rate.remaining.toLocaleString()} of ${rate.limit.toLocaleString()} requests left, resets ${new Date(rate.reset * 1000).toLocaleTimeString()}`
                    : "Not reported"}
                </dd>
              </>
            )}
            <dt>SSH agent for clones</dt>
            <dd>
              {status.ssh_agent.available
                ? status.ssh_agent.configured
                  ? "github.ssh_auth_sock"
                  : "The bridge's SSH_AUTH_SOCK"
                : status.ssh_agent.configured
                  ? "github.ssh_auth_sock is not a socket: SSH clones will fail"
                  : "None: clone over HTTPS, or set github.ssh_auth_sock"}
            </dd>
          </dl>
          <div className="actions">
            <button
              type="button"
              className="subtle"
              disabled={status.busy}
              onClick={() => void test()}
            >
              <Icon
                name={status.busy ? "loader-circle" : "refresh-cw"}
                size="sm"
                className={status.busy ? "spin" : ""}
              />
              Test
            </button>
          </div>
          {!status.signed_in && !status.busy && status.error && <SignInHelp />}
        </>
      )}
      {problem}
    </>
  );
}

type Protocol = "ssh" | "https";

/** A project's "Add repository" dialog: picks a repository (or takes a
 * clone URL) and clones it into the project's folder. */
export function GitHubPicker({
  rpc,
  project,
  existing,
  onStarted,
  onClose,
}: {
  rpc: Rpc;
  project: Project;
  /** Folder names already in the project. */
  existing: string[];
  /** A clone started: the Repos view follows it. */
  onStarted: () => void;
  onClose: () => void;
}) {
  const [status, setStatus] = useState<GitHubStatus | null>(null),
    [repos, setRepos] = useState<GitHubRepository[] | null>(null),
    [error, setError] = useState(""),
    [query, setQuery] = useState(""),
    [url, setUrl] = useState(""),
    [protocol, setProtocol] = useState<Protocol>("ssh"),
    [confirm, setConfirm] = useState<GitHubRepository | null>(null),
    [starting, setStarting] = useState("");
  const search = useRef<HTMLInputElement>(null);
  const alive = useRef(true);
  const latest = useRef(rpc);
  latest.current = rpc;
  async function load(refresh = false) {
    try {
      const listed = await latest.current("github_repositories", { refresh });
      if (!alive.current) return;
      const next = readGitHubStatus(listed);
      if (next) setStatus(next);
      const list = readRepositories(listed);
      if (next?.busy || !list) {
        setTimeout(() => alive.current && void load(), 1000);
      } else {
        setRepos(list);
      }
    } catch (e) {
      if (alive.current) setError(message(e));
    }
  }
  const loading = useRef(load);
  loading.current = load;
  useEffect(() => {
    alive.current = true;
    void (async () => {
      try {
        const s = readGitHubStatus(await latest.current("github_status"));
        if (!alive.current || !s) return;
        setStatus(s);
        if (s.enabled) void loading.current();
      } catch (e) {
        if (alive.current) setError(message(e));
      }
    })();
    return () => {
      alive.current = false;
    };
  }, []);
  const listed = repos !== null;
  useEffect(() => search.current?.focus(), [listed]);
  const taken = new Set(existing.map((name) => name.toLowerCase()));
  const cloning = new Set(
    (status?.clones || [])
      .filter((c) => c.state === "running")
      .map((c) => c.full_name.toLowerCase()),
  );
  async function clone(target: string, key: string) {
    setConfirm(null);
    setError("");
    setStarting(key);
    try {
      const s = readGitHubStatus(
        await rpc("github_clone", { project: project.id, url: target }),
      );
      if (s) setStatus(s);
      onStarted();
      onClose();
    } catch (e) {
      setError(message(e));
    } finally {
      if (alive.current) setStarting("");
    }
  }
  const urlOf = (repo: GitHubRepository) =>
    protocol === "ssh" ? repo.ssh_url : repo.https_url;
  const typedName = cloneName(url);
  const shown = repos ? filterRepositories(repos, query) : [];
  const signedOut = status && (!status.enabled || (!status.signed_in && !status.busy && !!status.error));
  return (
    <div
      className="modal-shade confirmation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <section
        className="confirm-dialog github-picker"
        role="dialog"
        aria-modal="true"
        aria-labelledby="github-picker-title"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.stopPropagation();
            if (confirm) setConfirm(null);
            else onClose();
          }
        }}
      >
        <h2 id="github-picker-title">Add a repository to {project.name}</h2>
        <p className="project-hint">
          Clones into <code>{project.path}/</code> with your own Git
          credentials.
        </p>
        <form
          className="github-url"
          onSubmit={(event) => {
            event.preventDefault();
            if (typedName && !taken.has(typedName.toLowerCase()))
              void clone(url.trim(), "url");
          }}
        >
          <label className="project-field">
            <span>Clone URL</span>
            <input
              aria-label="Clone URL"
              placeholder="git@github.com:owner/name.git"
              value={url}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              onChange={(e) => setUrl(e.target.value)}
            />
          </label>
          <button
            type="submit"
            className="small"
            disabled={!typedName || taken.has(typedName.toLowerCase()) || !!starting}
            title={
              typedName && taken.has(typedName.toLowerCase())
                ? `${project.path}/${typedName} already exists`
                : undefined
            }
          >
            {starting === "url" ? "Starting…" : "Clone"}
          </button>
        </form>
        {confirm ? (
          <div className="notice" role="alertdialog" aria-label="Large repository">
            <p>
              {confirm.full_name} is {formatSize(confirm.size_kib)} on GitHub.
              Cloning it can take a while and uses that much disk space or more.
            </p>
            <div className="actions">
              <button
                type="button"
                onClick={() => void clone(urlOf(confirm), String(confirm.id))}
              >
                Clone anyway
              </button>
              <button type="button" className="subtle" onClick={() => setConfirm(null)}>
                Cancel
              </button>
            </div>
          </div>
        ) : !status ? (
          !error && <p role="status">Checking GitHub…</p>
        ) : signedOut ? (
          <div className="notice">
            <p>
              {status.enabled
                ? "The server's GitHub token is not working, so your repositories cannot be listed."
                : "The server is not signed in to GitHub, so your repositories cannot be listed."}{" "}
              A clone URL above still works.
            </p>
            <div className="actions">
              <button
                type="button"
                className="subtle"
                onClick={() => {
                  onClose();
                  openSettings("github");
                }}
              >
                Open Settings → GitHub
              </button>
            </div>
          </div>
        ) : (
          <>
            <div className="github-search">
              <Icon name="search" size="sm" />
              <input
                ref={search}
                aria-label="Find a repository"
                placeholder={
                  status.login ? `Find a repository of ${status.login}…` : "Find a repository…"
                }
                value={query}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                onChange={(e) => setQuery(e.target.value)}
              />
              <span className="github-protocol" role="radiogroup" aria-label="Clone over">
                {(["ssh", "https"] as const).map((p) => (
                  <label key={p}>
                    <input
                      type="radio"
                      name="github-protocol"
                      checked={protocol === p}
                      onChange={() => setProtocol(p)}
                    />
                    {p === "ssh" ? "SSH" : "HTTPS"}
                  </label>
                ))}
              </span>
              <button
                type="button"
                className="icon-btn"
                aria-label="Refresh the list"
                disabled={status.busy}
                onClick={() => {
                  setRepos(null);
                  void load(true);
                }}
              >
                <Icon name="refresh-cw" size="sm" className={status.busy ? "spin" : ""} />
              </button>
            </div>
            {!repos ? (
              <p role="status">Loading repositories…</p>
            ) : (
              <ul className="github-list github-repositories" aria-label="GitHub repositories">
                {shown.slice(0, 200).map((r) => {
                  const there = taken.has(r.name.toLowerCase());
                  const running = cloning.has(r.full_name.toLowerCase());
                  const huge = r.size_kib > MAX_CLONE_KIB;
                  return (
                    <li key={r.id} data-repository={r.full_name}>
                      <span className="github-item">
                        <strong>{r.full_name}</strong>
                        <small>
                          {[
                            r.private ? "Private" : "Public",
                            r.archived ? "Archived" : "",
                            r.fork ? "Fork" : "",
                            r.size_kib ? formatSize(r.size_kib) : "",
                            r.description,
                          ]
                            .filter(Boolean)
                            .join(" · ")}
                        </small>
                      </span>
                      <button
                        type="button"
                        className="small"
                        disabled={there || running || huge || !!starting}
                        title={
                          there
                            ? `${project.path}/${r.name} already exists`
                            : huge
                              ? "Too large to clone here; clone it in a terminal"
                              : urlOf(r)
                        }
                        aria-label={`Clone ${r.full_name}`}
                        onClick={() =>
                          r.size_kib > LARGE_KIB
                            ? setConfirm(r)
                            : void clone(urlOf(r), String(r.id))
                        }
                      >
                        {there
                          ? "In project"
                          : running
                            ? "Cloning…"
                            : starting === String(r.id)
                              ? "Starting…"
                              : "Clone"}
                      </button>
                    </li>
                  );
                })}
                {repos.length === 0 && (
                  <li className="faint">This account sees no repositories.</li>
                )}
                {repos.length > 0 && shown.length === 0 && (
                  <li className="faint">No repository matches “{query}”.</li>
                )}
                {shown.length > 200 && (
                  <li className="faint">{shown.length - 200} more; refine the search.</li>
                )}
                {status.repositories_truncated && (
                  <li className="faint">
                    Only the first {repos.length} repositories are listed; enter
                    a clone URL for others.
                  </li>
                )}
              </ul>
            )}
          </>
        )}
        {(error || (!signedOut && status?.error)) && (
          <p className="notice error" role="alert">
            {error || status?.error}
          </p>
        )}
        <div className="confirm-buttons">
          <button type="button" className="subtle" onClick={onClose}>
            Close
          </button>
        </div>
      </section>
    </div>
  );
}

/** How often a shown project's GitHub facts are asked again (the bridge
 * caches them for five minutes). */
export const FACTS_REFRESH_MS = 5 * 60_000;

/** GitHub facts for the given `owner/name`s, while `rpc` is set. */
export function useGitHubFacts(rpc: Rpc | null, slugs: string[]) {
  const [facts, setFacts] = useState<Record<string, GitHubFacts>>({});
  const key = [...new Set(slugs)].sort().join("\n");
  const latest = useRef(rpc);
  latest.current = rpc;
  const offered = !!rpc;
  useEffect(() => {
    if (!offered || !key) return;
    let alive = true,
      timer: ReturnType<typeof setTimeout>;
    async function load(retry: number) {
      let next = FACTS_REFRESH_MS;
      try {
        const result = await latest.current!("github_state", {
          repos: key.split("\n").slice(0, 64),
        });
        if (!alive) return;
        setFacts((all) => ({ ...all, ...readFacts(result) }));
        // Sooner while the bridge is still fetching some.
        if (result?.pending) next = Math.min(retry, FACTS_REFRESH_MS);
      } catch {
        // Signed out, or not offered: no facts.
      }
      if (alive) timer = setTimeout(() => void load(retry * 2), next);
    }
    void load(5000);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [offered, key]);
  return facts;
}

/** A checkout's GitHub facts, on its row. */
export function RepoFacts({ slug, facts }: { slug: string; facts?: GitHubFacts }) {
  if (!facts?.visible) return null;
  const pushed = facts.pushed_at ? Date.parse(facts.pushed_at) / 1000 : NaN;
  return (
    <span className="github-facts" data-github={slug}>
      <span className="github-facts-link" title="Repository on GitHub">
        {slug}
      </span>
      {facts.visibility && <span className="git-tag">{facts.visibility}</span>}
      {facts.archived && <span className="git-tag">archived</span>}
      {facts.default_branch && (
        <span title="Default branch on GitHub">
          <Icon name="git-branch" size="sm" />
          {facts.default_branch}
        </span>
      )}
      {typeof facts.open_pulls === "number" && (
        <span title="Open pull requests">
          <Icon name="git-pull-request" size="sm" />
          {facts.open_pulls}
        </span>
      )}
      {Number.isFinite(pushed) && (
        <span className="faint" title={new Date(pushed * 1000).toLocaleString()}>
          pushed {relativeTime(pushed)}
        </span>
      )}
    </span>
  );
}
