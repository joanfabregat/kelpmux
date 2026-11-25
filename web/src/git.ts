// Read-only Git facts of a project's checkouts (bridge API minor 6, `git`):
// the project folder itself when it is a checkout, plus its direct
// subfolders that are. The bridge runs Git through a hardened runner (no
// hooks, filters, textconv, external diff, fsmonitor or network; see
// src/native/web_git.rs) and never writes. Pure functions, no DOM or React,
// so `node --test` loads them directly.

/** One changed file of a checkout. */
export type GitChange = {
  /** Relative to the checkout. */
  path: string;
  /** M, A, D, R, C, T, U (unmerged) or ? (untracked). */
  status: string;
  staged: boolean;
  unstaged: boolean;
  /** Lines against HEAD; null for binary, oversized or unscanned files. */
  added: number | null;
  removed: number | null;
  /** A rename's or copy's original path. */
  from?: string;
};

export type GitCommit = { sha: string; subject: string; time: number };

/** One checkout, as the bridge reports it. */
export type GitRepo = {
  path: string;
  name: string;
  branch: string | null;
  head: string | null;
  detached: boolean;
  upstream: string | null;
  ahead: number | null;
  behind: number | null;
  files: number;
  staged: number;
  unstaged: number;
  untracked: number;
  conflicted: number;
  added: number;
  removed: number;
  uncounted: number;
  last: GitCommit | null;
  /** The changed files, when asked for (at most 200). */
  changes?: GitChange[];
  /** `owner/name` when the `origin` remote is on github.com (API minor 9). */
  github?: string | null;
  /** Some output reached its limit: the counts are lower bounds. */
  truncated: boolean;
  error: string | null;
};

/** A project's checkouts. */
export type ProjectGit = {
  folder: string;
  exists: boolean;
  repos: GitRepo[];
  /** More checkouts than the bridge inspects (32). */
  truncated: boolean;
  /** Unix seconds (the server's clock). */
  sampled_at: number;
};

const count = (value: unknown) =>
  typeof value === "number" && Number.isFinite(value) && value >= 0
    ? Math.floor(value)
    : 0;
const maybe = (value: unknown) =>
  typeof value === "number" && Number.isFinite(value) && value >= 0
    ? Math.floor(value)
    : null;
const text = (value: unknown, limit = 4096) =>
  typeof value === "string" && value.length <= limit ? value : null;

function readChange(value: any): GitChange | null {
  const path = text(value?.path);
  if (!path) return null;
  const status = text(value?.status, 1) || "M";
  return {
    path,
    status,
    staged: value?.staged === true,
    unstaged: value?.unstaged === true,
    added: maybe(value?.added),
    removed: maybe(value?.removed),
    ...(text(value?.from) ? { from: value.from } : {}),
  };
}

function readRepo(value: any): GitRepo | null {
  const path = text(value?.path);
  if (!path || !path.startsWith("/")) return null;
  const last = value?.last;
  return {
    path,
    name: text(value?.name, 255) || baseName(path),
    branch: text(value?.branch, 255),
    head: text(value?.head, 64),
    detached: value?.detached === true,
    upstream: text(value?.upstream, 255),
    ahead: maybe(value?.ahead),
    behind: maybe(value?.behind),
    files: count(value?.files),
    staged: count(value?.staged),
    unstaged: count(value?.unstaged),
    untracked: count(value?.untracked),
    conflicted: count(value?.conflicted),
    added: count(value?.added),
    removed: count(value?.removed),
    uncounted: count(value?.uncounted),
    last:
      last && text(last.sha, 64)
        ? {
            sha: last.sha,
            subject: text(last.subject, 1024) || "",
            time: count(last.time),
          }
        : null,
    ...(Array.isArray(value?.changes)
      ? {
          changes: value.changes
            .slice(0, 1000)
            .map(readChange)
            .filter((change: GitChange | null): change is GitChange => !!change),
        }
      : {}),
    truncated: value?.truncated === true,
    error: text(value?.error, 1024),
    github:
      typeof value?.github === "string" &&
      value.github.length <= 201 &&
      /^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/.test(value.github) &&
      !value.github.split("/").some((part: string) => part === "." || part === "..")
      ? value.github
      : null,
  };
}

/** Reads the bridge's `git` result, defensively. */
export function readProjectGit(value: any): ProjectGit | null {
  if (!value || typeof value !== "object" || !Array.isArray(value.repos))
    return null;
  return {
    folder: text(value.folder) || "",
    exists: value.exists !== false,
    repos: value.repos
      .slice(0, 64)
      .map(readRepo)
      .filter((repo: GitRepo | null): repo is GitRepo => !!repo),
    truncated: value.truncated === true,
    sampled_at: count(value.sampled_at),
  };
}

const baseName = (path: string) =>
  path.replace(/\/+$/, "").split("/").pop() || path;

/** A checkout with uncommitted changes or commits not on its upstream. */
export const repoChanged = (repo: GitRepo) =>
  !repo.error && (repo.files > 0 || (repo.ahead || 0) > 0);

/** What a project's changes mark shows: totals over its checkouts. */
export type ProjectChanges = {
  files: number;
  added: number;
  removed: number;
  ahead: number;
  /** The checkouts with changes, for the tooltip. */
  repos: GitRepo[];
  /** Some counts are lower bounds. */
  partial: boolean;
};

/** A project's changes, or null when every checkout is clean (or none is
 * known yet). */
export function projectChanges(git: ProjectGit | undefined | null): ProjectChanges | null {
  const repos = (git?.repos || []).filter(repoChanged);
  if (!repos.length) return null;
  const sum = (pick: (repo: GitRepo) => number) =>
    repos.reduce((total, repo) => total + pick(repo), 0);
  return {
    files: sum((repo) => repo.files),
    added: sum((repo) => repo.added),
    removed: sum((repo) => repo.removed),
    ahead: sum((repo) => repo.ahead || 0),
    repos,
    partial: repos.some((repo) => repo.truncated),
  };
}

const plural = (n: number, one: string, many = `${one}s`) =>
  `${n} ${n === 1 ? one : many}`;

/** "3 files changed, 2 commits not pushed". */
export function changesSummary(changes: {
  files: number;
  ahead: number | null;
  partial?: boolean;
}) {
  const parts: string[] = [];
  if (changes.files)
    parts.push(
      `${plural(changes.files, "file")}${changes.partial ? " or more" : ""} changed`,
    );
  if (changes.ahead) parts.push(`${plural(changes.ahead, "commit")} not pushed`);
  return parts.join(", ") || "No changes";
}

/** "1 staged, 2 unstaged, 1 untracked, 1 conflicted" (only what is there). */
export function changeKinds(repo: GitRepo) {
  return [
    [repo.staged, "staged"],
    [repo.unstaged, "unstaged"],
    [repo.untracked, "untracked"],
    [repo.conflicted, "conflicted"],
  ]
    .filter(([n]) => n)
    .map(([n, label]) => `${n} ${label}`)
    .join(", ");
}

/** Where HEAD is: the branch, else "detached at abc1234", else nothing. */
export function headLabel(repo: Pick<GitRepo, "branch" | "head" | "detached">) {
  if (repo.branch) return repo.branch;
  if (repo.head) return `detached at ${shortSha(repo.head)}`;
  return "";
}

export const shortSha = (sha: string) => sha.slice(0, 7);

/** The upstream's ahead/behind: "↑2 ↓1", "" when even or unknown. */
export function aheadBehind(repo: Pick<GitRepo, "ahead" | "behind">) {
  const parts: string[] = [];
  if (repo.ahead) parts.push(`↑${repo.ahead}`);
  if (repo.behind) parts.push(`↓${repo.behind}`);
  return parts.join(" ");
}

/** "just now", "5 min ago", "3 h ago", "yesterday", "4 days ago", "3 weeks
 * ago", "5 months ago", "2 years ago". `now` in Unix seconds. */
export function relativeTime(time: number, now = Date.now() / 1000) {
  const seconds = Math.max(0, now - time);
  const minutes = Math.floor(seconds / 60),
    hours = Math.floor(seconds / 3600),
    days = Math.floor(seconds / 86400);
  if (minutes < 1) return "just now";
  if (hours < 1) return `${minutes} min ago`;
  if (days < 1) return `${hours} h ago`;
  if (days === 1) return "yesterday";
  if (days < 14) return `${days} days ago`;
  if (days < 60) return `${Math.floor(days / 7)} weeks ago`;
  if (days < 365) return `${Math.floor(days / 30)} months ago`;
  const years = Math.floor(days / 365);
  return `${plural(years, "year")} ago`;
}

const statusNames: Record<string, string> = {
  M: "Modified",
  A: "Added",
  D: "Deleted",
  R: "Renamed",
  C: "Copied",
  T: "Type changed",
  U: "Unmerged",
  "?": "Untracked",
};
/** A change's status letter in words, and where the change is. */
export function statusLabel(change: GitChange) {
  const name = statusNames[change.status] || "Changed";
  if (change.status === "?" || change.status === "U") return name;
  const where =
    change.staged && change.unstaged
      ? "staged and unstaged"
      : change.staged
        ? "staged"
        : "unstaged";
  return `${name}, ${where}`;
}

/** A changed file's absolute path. */
export const changePath = (repo: GitRepo, change: GitChange) =>
  `${repo.path.replace(/\/+$/, "")}/${change.path}`;

/** Every known checkout by its path, from every project's facts. */
export function checkoutsByPath(projects: Record<string, ProjectGit | undefined>) {
  const map = new Map<string, GitRepo>();
  for (const git of Object.values(projects))
    for (const repo of git?.repos || []) map.set(repo.path, repo);
  return map;
}
