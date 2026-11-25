// Read-only history and diffs of a checkout (bridge API minor 10,
// `git_history`): `git_branches`, `git_log`, `git_commit`, `git_diff` and
// `git_changes`, each `{project, repo, …}` with `repo` the path `git`
// reported for one of the project's checkouts. The bridge validates every
// revision and path again and runs Git through its hardened runner (see
// src/native/web_git_history.rs); these checks only keep the URL tidy.
// Pure functions, no DOM or React, so `node --test` loads them directly.

export type Branch = {
  name: string;
  ref: string;
  current: boolean;
  upstream: string | null;
  ahead: number | null;
  behind: number | null;
  gone: boolean;
  sha: string;
  subject: string;
  author: string;
  time: number;
};

export type Branches = {
  head: string | null;
  current: string | null;
  detached: boolean;
  branches: Branch[];
  remotes: Branch[];
  truncated: boolean;
};

export type LogEntry = {
  sha: string;
  parents: string[];
  author: string;
  time: number;
  subject: string;
  refs: string[];
};

export type LogPage = {
  rev: string;
  head: string;
  skip: number;
  commits: LogEntry[];
  more: boolean;
};

export type FileChange = {
  path: string;
  from?: string;
  /** A, M, D, R, C, T or U. */
  status: string;
  added: number | null;
  removed: number | null;
  binary: boolean;
};

export type CommitDetails = {
  sha: string;
  parents: string[];
  merge: boolean;
  author: string;
  author_email: string;
  author_time: number;
  committer: string;
  committer_email: string;
  commit_time: number;
  refs: string[];
  message: string;
  message_truncated: boolean;
  files: FileChange[];
  files_truncated: boolean;
};

export type Hunk = {
  header: string;
  old_start: number;
  old_lines: number;
  new_start: number;
  new_lines: number;
  /** First character: " " context, "+" added, "-" removed, "\" a note. */
  lines: string[];
};

export type FileDiff = {
  path: string;
  from?: string;
  status: string;
  binary: boolean;
  meta: string[];
  hunks: Hunk[];
  truncated: boolean;
  /** An untracked file too large to show: its size in bytes. */
  too_large?: number;
};

export type DiffKind = "commit" | "staged" | "unstaged" | "untracked";

export type DiffResult = {
  kind: DiffKind;
  sha: string | null;
  path: string;
  files: FileDiff[];
  truncated: boolean;
};

export type UntrackedFile = {
  path: string;
  /** Lines of a small text file; null when not counted. */
  added: number | null;
  size: number | null;
  binary: boolean;
};

export type WorkingChanges = {
  head: string | null;
  staged: FileChange[];
  unstaged: FileChange[];
  untracked: UntrackedFile[];
  conflicted: string[];
  truncated: boolean;
};

export type RepoTab = "changes" | "history" | "branches";
export const repoTabs: RepoTab[] = ["changes", "history", "branches"];

const text = (value: unknown, limit = 4096) =>
  typeof value === "string" && value.length <= limit ? value : null;
const count = (value: unknown) =>
  typeof value === "number" && Number.isFinite(value) && value >= 0
    ? Math.floor(value)
    : 0;
const maybe = (value: unknown) =>
  typeof value === "number" && Number.isFinite(value) && value >= 0
    ? Math.floor(value)
    : null;
const hash = (value: unknown) =>
  typeof value === "string" && /^[0-9a-f]{40}([0-9a-f]{24})?$/.test(value)
    ? value
    : null;
const strings = (value: unknown, max: number, limit = 1024) =>
  Array.isArray(value)
    ? value
        .slice(0, max)
        .map((v) => text(v, limit))
        .filter((v): v is string => v !== null)
    : [];
const list = <T>(value: unknown, max: number, read: (v: any) => T | null) =>
  Array.isArray(value)
    ? value
        .slice(0, max)
        .map(read)
        .filter((v): v is T => v !== null)
    : [];

function readBranch(value: any): Branch | null {
  const name = text(value?.name, 255);
  const sha = hash(value?.sha);
  if (!name || !sha) return null;
  return {
    name,
    ref: text(value?.ref, 255) || name,
    current: value?.current === true,
    upstream: text(value?.upstream, 255),
    ahead: maybe(value?.ahead),
    behind: maybe(value?.behind),
    gone: value?.gone === true,
    sha,
    subject: text(value?.subject, 1024) || "",
    author: text(value?.author, 255) || "",
    time: count(value?.time),
  };
}

export function readBranches(value: any): Branches | null {
  if (!value || typeof value !== "object" || !Array.isArray(value.branches))
    return null;
  return {
    head: hash(value.head),
    current: text(value.current, 255),
    detached: value.detached === true,
    branches: list(value.branches, 1000, readBranch),
    remotes: list(value.remotes, 1000, readBranch),
    truncated: value.truncated === true,
  };
}

function readEntry(value: any): LogEntry | null {
  const sha = hash(value?.sha);
  if (!sha) return null;
  return {
    sha,
    parents: list(value?.parents, 64, hash),
    author: text(value?.author, 255) || "",
    time: count(value?.time),
    subject: text(value?.subject, 1024) || "",
    refs: strings(value?.refs, 32, 255),
  };
}

export function readLogPage(value: any): LogPage | null {
  if (!value || typeof value !== "object" || !Array.isArray(value.commits))
    return null;
  return {
    rev: text(value.rev, 255) || "HEAD",
    head: hash(value.head) || "",
    skip: count(value.skip),
    commits: list(value.commits, 1000, readEntry),
    more: value.more === true,
  };
}

function readFileChange(value: any): FileChange | null {
  const path = text(value?.path);
  if (!path) return null;
  const from = text(value?.from);
  return {
    path,
    ...(from ? { from } : {}),
    status: text(value?.status, 1) || "M",
    added: maybe(value?.added),
    removed: maybe(value?.removed),
    binary: value?.binary === true,
  };
}

export function readCommit(value: any): CommitDetails | null {
  const sha = hash(value?.sha);
  if (!sha) return null;
  return {
    sha,
    parents: list(value.parents, 64, hash),
    merge: value.merge === true,
    author: text(value.author, 255) || "",
    author_email: text(value.author_email, 255) || "",
    author_time: count(value.author_time),
    committer: text(value.committer, 255) || "",
    committer_email: text(value.committer_email, 255) || "",
    commit_time: count(value.commit_time),
    refs: strings(value.refs, 32, 255),
    message: text(value.message, 128 * 1024) || "",
    message_truncated: value.message_truncated === true,
    files: list(value.files, 2000, readFileChange),
    files_truncated: value.files_truncated === true,
  };
}

function readHunk(value: any): Hunk | null {
  if (!value || !Array.isArray(value.lines)) return null;
  return {
    header: text(value.header, 1024) || "@@",
    old_start: count(value.old_start),
    old_lines: count(value.old_lines),
    new_start: count(value.new_start),
    new_lines: count(value.new_lines),
    lines: strings(value.lines, 50_000, 1 << 20),
  };
}

function readFileDiff(value: any): FileDiff | null {
  const path = text(value?.path);
  if (!path) return null;
  const from = text(value?.from);
  const tooLarge = maybe(value?.too_large);
  return {
    path,
    ...(from ? { from } : {}),
    status: text(value?.status, 1) || "M",
    binary: value?.binary === true,
    meta: strings(value?.meta, 16, 1024),
    hunks: list(value?.hunks, 10_000, readHunk),
    truncated: value?.truncated === true,
    ...(tooLarge !== null ? { too_large: tooLarge } : {}),
  };
}

const diffKinds: DiffKind[] = ["commit", "staged", "unstaged", "untracked"];

export function readDiff(value: any): DiffResult | null {
  if (!value || typeof value !== "object" || !Array.isArray(value.files))
    return null;
  return {
    kind: diffKinds.includes(value.kind) ? value.kind : "commit",
    sha: hash(value.sha),
    path: text(value.path) || "",
    files: list(value.files, 16, readFileDiff),
    truncated: value.truncated === true,
  };
}

function readUntracked(value: any): UntrackedFile | null {
  const path = text(value?.path);
  if (!path) return null;
  return {
    path,
    added: maybe(value?.added),
    size: maybe(value?.size),
    binary: value?.binary === true,
  };
}

export function readChanges(value: any): WorkingChanges | null {
  if (!value || typeof value !== "object" || !Array.isArray(value.staged))
    return null;
  return {
    head: hash(value.head),
    staged: list(value.staged, 2000, readFileChange),
    unstaged: list(value.unstaged, 2000, readFileChange),
    untracked: list(value.untracked, 2000, readUntracked),
    conflicted: strings(value.conflicted, 2000),
    truncated: value.truncated === true,
  };
}

/** A revision the bridge accepts: hex, or a ref name under
 * `git check-ref-format`'s rules, never an option or revision syntax. */
export function validRevision(rev: string): boolean {
  if (/^[0-9a-fA-F]{4,64}$/.test(rev)) return true;
  if (!rev || rev.length > 255 || rev === "@") return false;
  if (/^[-/]|[/.]$|\.\.|@\{|\/\/|[\u0000- \u007f~^:?*[\\]/.test(rev))
    return false;
  return rev
    .split("/")
    .every((part) => part && !part.startsWith(".") && !part.endsWith(".lock"));
}

/** A path inside a repository, as the bridge accepts one. */
export function validRepoPath(path: string): boolean {
  return (
    !!path &&
    path.length <= 4096 &&
    !/^[-:/]/.test(path) &&
    !/[\u0000-\u001f\u007f]/.test(path) &&
    path
      .split("/")
      .every(
        (part) =>
          part && part !== "." && part !== ".." && part.toLowerCase() !== ".git",
      )
  );
}

/** A full commit hash, or an abbreviated one. */
export const validCommit = (sha: string) => /^[0-9a-f]{4,64}$/.test(sha);

/** One line of a hunk, numbered. */
export type DiffRow = {
  kind: "context" | "add" | "del" | "note";
  /** Line numbers in the old and new file (null where the line is absent). */
  old: number | null;
  new: number | null;
  text: string;
};

/** A hunk's lines with their old and new line numbers. */
export function numberLines(hunk: Hunk): DiffRow[] {
  let old = hunk.old_start,
    next = hunk.new_start;
  return hunk.lines.map((line) => {
    const mark = line[0],
      body = line.slice(1);
    if (mark === "+") return { kind: "add", old: null, new: next++, text: body };
    if (mark === "-") return { kind: "del", old: old++, new: null, text: body };
    if (mark === "\\") return { kind: "note", old: null, new: null, text: line };
    return { kind: "context", old: old++, new: next++, text: body };
  });
}

/** Hunks longer than this start collapsed. */
export const HUNK_COLLAPSE = 150;
/** Lines of a collapsed hunk shown before "Show N more lines". */
export const HUNK_PREVIEW = 40;

/** Lines added and removed over a diff's hunks. */
export function diffStats(file: Pick<FileDiff, "hunks">) {
  let added = 0,
    removed = 0;
  for (const hunk of file.hunks)
    for (const line of hunk.lines) {
      if (line[0] === "+") added++;
      else if (line[0] === "-") removed++;
    }
  return { added, removed };
}

/** A ref decoration (`%D`) by kind: `HEAD -> main`, `tag: v1`,
 * `origin/main`, `main`, `HEAD`. */
export type RefLabel = { label: string; kind: "head" | "branch" | "remote" | "tag" };
export function refLabels(refs: string[], remotes: Set<string> = new Set(["origin"])): RefLabel[] {
  const labels: RefLabel[] = [];
  for (const ref of refs) {
    if (ref.startsWith("HEAD -> ")) {
      labels.push({ label: ref.slice(8), kind: "head" });
    } else if (ref === "HEAD") {
      labels.push({ label: "HEAD", kind: "head" });
    } else if (ref.startsWith("tag: ")) {
      labels.push({ label: ref.slice(5), kind: "tag" });
    } else {
      const remote = ref.includes("/") && remotes.has(ref.split("/")[0]);
      labels.push({ label: ref, kind: remote ? "remote" : "branch" });
    }
  }
  return labels;
}

/** The tab a checkout opens on: its changes when it has any, else its
 * history. */
export const defaultTab = (repo: { files: number }): RepoTab =>
  repo.files > 0 ? "changes" : "history";

/** "1.2 KB", "3.4 MB". */
export function formatSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** The paths a working-change selection covers: its sides (staged,
 * unstaged, untracked, conflicted) for one path. */
export function sidesOf(changes: WorkingChanges, path: string) {
  return {
    staged: changes.staged.find((f) => f.path === path),
    unstaged: changes.unstaged.find((f) => f.path === path),
    untracked: changes.untracked.find((f) => f.path === path),
    conflicted: changes.conflicted.includes(path),
  };
}
