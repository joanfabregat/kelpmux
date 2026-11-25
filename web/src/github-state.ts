// GitHub as the web bridge reports it (`github_*` operations, API minor
// 9), read defensively: where the bridge's token comes from and who it
// signs in as, the repositories it can see, the clones this bridge runs
// and read-only facts about a checkout's GitHub repository. Only public
// metadata reaches the browser, never the token; every GitHub request and
// every clone is the bridge's.

export type GitHubClone = {
  id: number;
  project: string;
  /** `owner/name` for github.com, else the checkout's folder name. */
  full_name: string;
  url: string;
  path: string;
  state: "running" | "done" | "failed" | "cancelled";
  progress: string;
  error: string | null;
};
export type GitHubRate = { limit: number; remaining: number; reset: number };
export type GitHubStatus = {
  /** A token source is configured (`github.auth`, `github.token_command`). */
  enabled: boolean;
  source: "gh" | "env" | "command" | null;
  source_label: string | null;
  signed_in: boolean;
  login: string | null;
  name: string | null;
  /** Classic tokens' scopes; `null` when GitHub does not say. */
  scopes: string[] | null;
  rate: GitHubRate | null;
  checked_at: number | null;
  ssh_agent: { configured: boolean; available: boolean };
  busy: boolean;
  error: string | null;
  repositories_ready: boolean;
  repositories_truncated: boolean;
  clones: GitHubClone[];
};
export type GitHubRepository = {
  id: number;
  name: string;
  full_name: string;
  ssh_url: string;
  https_url: string;
  private: boolean;
  archived: boolean;
  fork: boolean;
  default_branch: string;
  description: string;
  size_kib: number;
  pushed_at: string;
};
/** Facts about a checkout's GitHub repository (`github_state`). */
export type GitHubFacts = {
  visible: boolean;
  default_branch?: string;
  visibility?: string;
  archived?: boolean;
  pushed_at?: string;
  open_pulls?: number | null;
};

const text = (value: unknown, limit = 1024) =>
  typeof value === "string" && value.length <= limit ? value : null;
const count = (value: unknown) =>
  typeof value === "number" && Number.isSafeInteger(value) && value >= 0
    ? value
    : null;
const id = (value: unknown) => {
  const n = count(value);
  return n && n > 0 ? n : null;
};
/** A GitHub login or repository name: what `user_config::github_name`
 * accepts (never `.` or `..`). */
const name = (value: unknown) =>
  typeof value === "string" &&
  /^[A-Za-z0-9._-]{1,100}$/.test(value) &&
  value !== "." &&
  value !== ".."
    ? value
    : null;
const fullName = (value: unknown) => {
  if (typeof value !== "string") return null;
  const [owner, repo, extra] = value.split("/");
  return extra === undefined && name(owner) && name(repo) ? value : null;
};
const list = <T>(value: unknown, read: (item: any) => T | null): T[] =>
  Array.isArray(value)
    ? value.map(read).filter((item): item is T => item !== null)
    : [];

function readClone(value: any): GitHubClone | null {
  const full = fullName(value?.full_name) || name(value?.full_name);
  const state = value?.state;
  if (!id(value?.id) || !full) return null;
  if (!["running", "done", "failed", "cancelled"].includes(state)) return null;
  return {
    id: value.id,
    project: text(value.project, 256) || "",
    full_name: full,
    url: text(value.url, 2048) || "",
    path: text(value.path, 4096) || "",
    state,
    progress: text(value.progress, 512) || "",
    error: text(value.error),
  };
}

function readRate(value: any): GitHubRate | null {
  const limit = count(value?.limit),
    remaining = count(value?.remaining),
    reset = count(value?.reset);
  return limit !== null && remaining !== null && reset !== null
    ? { limit, remaining, reset }
    : null;
}

/** Reads `github_status` (and the status part of the other answers). */
export function readGitHubStatus(value: any): GitHubStatus | null {
  if (!value || typeof value !== "object" || typeof value.enabled !== "boolean")
    return null;
  const source = ["gh", "env", "command"].includes(value.source)
    ? value.source
    : null;
  return {
    enabled: value.enabled,
    source,
    source_label: text(value.source_label, 4096),
    signed_in: value.signed_in === true,
    login: name(value.login),
    name: text(value.name, 100),
    scopes: Array.isArray(value.scopes)
      ? list(value.scopes, (s) => text(s, 64))
      : null,
    rate: readRate(value.rate),
    checked_at: count(value.checked_at),
    ssh_agent: {
      configured: value.ssh_agent?.configured === true,
      available: value.ssh_agent?.available === true,
    },
    busy: value.busy === true,
    error: text(value.error),
    repositories_ready: value.repositories_ready === true,
    repositories_truncated: value.repositories_truncated === true,
    clones: list(value.clones, readClone),
  };
}

function readRepository(value: any): GitHubRepository | null {
  const full = fullName(value?.full_name);
  const short = name(value?.name);
  if (!id(value?.id) || !full || !short) return null;
  // Clone URLs are rebuilt here from the name: never taken as given.
  return {
    id: value.id,
    name: short,
    full_name: full,
    ssh_url: `git@github.com:${full}.git`,
    https_url: `https://github.com/${full}.git`,
    private: value.private === true,
    archived: value.archived === true,
    fork: value.fork === true,
    default_branch: text(value.default_branch, 255) || "",
    description: text(value.description, 240) || "",
    size_kib: count(value.size_kib) ?? 0,
    pushed_at: text(value.pushed_at, 32) || "",
  };
}

/** The repositories of a `github_repositories` answer, once listed. */
export function readRepositories(value: any): GitHubRepository[] | null {
  return Array.isArray(value?.repositories)
    ? list(value.repositories, readRepository)
    : null;
}

/** The facts of a `github_state` answer, by `owner/name`. */
export function readFacts(value: any): Record<string, GitHubFacts> {
  const facts: Record<string, GitHubFacts> = {};
  const repos = value?.repos;
  if (!repos || typeof repos !== "object") return facts;
  for (const [slug, fact] of Object.entries(repos as Record<string, any>)) {
    if (!fullName(slug) || !fact || typeof fact !== "object") continue;
    facts[slug] = fact.visible
      ? {
          visible: true,
          default_branch: text(fact.default_branch, 255) || undefined,
          visibility: text(fact.visibility, 16) || undefined,
          archived: fact.archived === true,
          pushed_at: text(fact.pushed_at, 32) || undefined,
          open_pulls: count(fact.open_pulls),
        }
      : { visible: false };
  }
  return facts;
}

/** The checkout folder a clone URL makes, as the bridge derives it (its
 * last path segment without `.git`), or `null` when it cannot name one. */
export function cloneName(url: string): string | null {
  const trimmed = url.trim().replace(/\/+$/, "");
  const last = trimmed.split(/[/:]/).pop() || "";
  const folder = last.replace(/\.git$/, "");
  return /^[A-Za-z0-9_-][A-Za-z0-9._-]{0,99}$/.test(folder) ? folder : null;
}

/** `12 MB`, `1.4 GB`, from KiB. */
export function formatSize(kib: number): string {
  if (kib < 1024) return `${Math.max(1, Math.round(kib))} KB`;
  if (kib < 1024 * 1024) return `${Math.round(kib / 1024)} MB`;
  return `${(kib / 1024 / 1024).toFixed(1)} GB`;
}
/** Repositories above this size (KiB) ask before cloning. */
export const LARGE_KIB = 500 * 1024;
/** The bridge refuses clones above this size (KiB). */
export const MAX_CLONE_KIB = 5 * 1024 * 1024;

/** The repositories matching a search, by name or description. */
export function filterRepositories(
  repos: GitHubRepository[],
  query: string,
): GitHubRepository[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return repos.filter((r) => {
    const haystack = `${r.full_name} ${r.description}`.toLowerCase();
    return words.every((word) => haystack.includes(word));
  });
}

/** What the token source is, for people. */
export function sourceText(s: GitHubStatus): string {
  switch (s.source) {
    case "gh":
      return "the GitHub CLI (gh auth token)";
    case "env":
      return "GH_TOKEN or GITHUB_TOKEN in the bridge's environment";
    case "command":
      return s.source_label || "a token command";
    default:
      return "nothing";
  }
}
