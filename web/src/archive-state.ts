// Project status, archive, restore and delete (bridge API minor 11,
// operation `archive`), read defensively: the safe-removal check, the
// background jobs, the archive listing and the archive backend's status.
// Pure functions, no DOM or React, so `node --test` loads them directly.
// The bridge decides everything: it re-runs the check before removing a
// folder and refuses when the result changed; these only present it.
import { SCRATCH, type Project } from "./projects.ts";

export type Severity = "blocker" | "warning" | "info";
export type RemovalItem = {
  severity: Severity;
  kind: string;
  path: string;
  message: string;
};
export type RemovalBranch = { name: string; upstream: string | null; unpushed: number };
export type RemovalCheckout = {
  path: string;
  name: string;
  error: string | null;
  branch: string | null;
  detached: boolean;
  uncommitted: number;
  untracked: number;
  ignored: number;
  stashes: number;
  branches: RemovalBranch[];
};
export type RemovalCheck = {
  project: string;
  folder: string;
  mode: "archive" | "delete";
  exists: boolean;
  checked_at: number;
  fingerprint: string;
  checkouts: RemovalCheckout[];
  other_files: { files: number; bytes: number; truncated: boolean };
  items: RemovalItem[];
  blockers: number;
  warnings: number;
  allowed: boolean;
};

export type JobKind = "archive" | "restore" | "delete";
export type JobState = "running" | "done" | "failed" | "cancelled";
export type ArchiveJob = {
  id: string;
  project: string;
  kind: JobKind;
  state: JobState;
  phase: string;
  message: string;
  bytes_done: number;
  bytes_total: number | null;
  files_done: number;
  started_at: number;
  updated_at: number;
  error: string | null;
  result: any;
  cancellable: boolean;
};

export type ArchiveListing = {
  object: string;
  size: number | null;
  updated: string | null;
  sha256: string | null;
  crc32c: string | null;
  recorded: boolean;
};

export type ArchiveStatus = {
  configured: boolean;
  backend: string | null;
  bucket: string | null;
  prefix: string;
  token_source: string;
  token_label: string;
  error: string | null;
};

const count = (value: unknown) =>
  Number.isSafeInteger(value) && (value as number) >= 0 ? (value as number) : 0;
const text = (value: unknown) => (typeof value === "string" ? value : null);
const clip = (value: unknown, limit = 500) =>
  typeof value === "string" ? value.slice(0, limit) : "";

function readItem(value: any): RemovalItem | null {
  if (typeof value?.message !== "string") return null;
  const severity: Severity =
    value.severity === "blocker" || value.severity === "warning" ? value.severity : "info";
  return {
    severity,
    kind: clip(value.kind, 64),
    path: clip(value.path, 4096),
    message: clip(value.message),
  };
}

function readCheckout(value: any): RemovalCheckout | null {
  if (typeof value?.path !== "string") return null;
  return {
    path: value.path,
    name: typeof value.name === "string" ? value.name : value.path,
    error: text(value.error),
    branch: text(value.branch),
    detached: value.detached === true,
    uncommitted: count(value.uncommitted),
    untracked: count(value.untracked),
    ignored: count(value.ignored),
    stashes: count(value.stashes),
    branches: (Array.isArray(value.branches) ? value.branches : [])
      .filter((b: any) => typeof b?.name === "string")
      .slice(0, 256)
      .map((b: any) => ({ name: b.name, upstream: text(b.upstream), unpushed: count(b.unpushed) })),
  };
}

/** A `project_check_removal` result, or null when malformed. */
export function readCheck(value: any): RemovalCheck | null {
  if (typeof value?.fingerprint !== "string" || typeof value?.project !== "string")
    return null;
  const items = (Array.isArray(value.items) ? value.items : [])
    .map(readItem)
    .filter(Boolean)
    .slice(0, 1000) as RemovalItem[];
  const checkouts = (Array.isArray(value.checkouts) ? value.checkouts : [])
    .map(readCheckout)
    .filter(Boolean)
    .slice(0, 256) as RemovalCheckout[];
  const blockers = items.filter((item) => item.severity === "blocker").length;
  return {
    project: value.project,
    folder: typeof value.folder === "string" ? value.folder : "",
    mode: value.mode === "archive" ? "archive" : "delete",
    exists: value.exists !== false,
    checked_at: count(value.checked_at),
    fingerprint: value.fingerprint,
    checkouts,
    other_files: {
      files: count(value.other_files?.files),
      bytes: count(value.other_files?.bytes),
      truncated: value.other_files?.truncated === true,
    },
    items,
    blockers: Math.max(blockers, count(value.blockers)),
    warnings: count(value.warnings),
    // Never allowed with a blocker, whatever the bridge says.
    allowed: value.allowed === true && blockers === 0,
  };
}

const plural = (n: number, one: string, many = `${one}s`) =>
  `${n.toLocaleString("en-US")} ${n === 1 ? one : many}`;

/** What a checkout holds that is on no remote, one phrase each. */
export function checkoutProblems(checkout: RemovalCheckout): string[] {
  const problems: string[] = [];
  if (checkout.error) problems.push(`not checked: ${checkout.error}`);
  if (checkout.uncommitted) problems.push(plural(checkout.uncommitted, "uncommitted change"));
  if (checkout.untracked) problems.push(plural(checkout.untracked, "untracked file"));
  if (checkout.stashes) problems.push(plural(checkout.stashes, "stash", "stashes"));
  for (const branch of checkout.branches) {
    if (branch.unpushed)
      problems.push(`${branch.name}: ${plural(branch.unpushed, "commit")} on no remote`);
    else if (!branch.upstream) problems.push(`${branch.name}: no upstream`);
  }
  if (checkout.detached) problems.push("detached HEAD");
  return problems;
}

/** One line about a check: what blocks it, or what it warns about. */
export function checkSummary(check: RemovalCheck): string {
  if (!check.exists) return "The folder does not exist.";
  if (check.blockers)
    return `${plural(check.blockers, "problem")} must be fixed first.`;
  if (check.warnings) return `${plural(check.warnings, "warning")}; nothing blocks it.`;
  return "Everything in the checkouts is on a remote.";
}

/** The items to list, blockers first, then warnings, then the rest. */
export function sortedItems(items: RemovalItem[]): RemovalItem[] {
  const rank = { blocker: 0, warning: 1, info: 2 } as const;
  return [...items].sort((a, b) => rank[a.severity] - rank[b.severity]);
}

function readJob(value: any): ArchiveJob | null {
  if (typeof value?.id !== "string" || typeof value?.project !== "string") return null;
  const kind: JobKind =
    value.kind === "restore" || value.kind === "delete" ? value.kind : "archive";
  const state: JobState = ["done", "failed", "cancelled"].includes(value.state)
    ? value.state
    : "running";
  return {
    id: value.id,
    project: value.project,
    kind,
    state,
    phase: clip(value.phase, 64),
    message: clip(value.message),
    bytes_done: count(value.bytes_done),
    bytes_total: Number.isSafeInteger(value.bytes_total) ? value.bytes_total : null,
    files_done: count(value.files_done),
    started_at: count(value.started_at),
    updated_at: count(value.updated_at),
    error: text(value.error),
    result: value.result ?? null,
    cancellable: state === "running" && value.cancellable === true,
  };
}

/** The `jobs` of an `archive_jobs` (or job-starting) result. */
export function readJobs(value: any): ArchiveJob[] {
  return (Array.isArray(value?.jobs) ? value.jobs : [])
    .map(readJob)
    .filter(Boolean)
    .slice(0, 256) as ArchiveJob[];
}

/** A project's job to show: its running one, else its newest. */
export function jobOf(jobs: ArchiveJob[], project: string): ArchiveJob | undefined {
  const mine = jobs.filter((job) => job.project === project);
  return (
    mine.find((job) => job.state === "running") ||
    [...mine].sort((a, b) => b.updated_at - a.updated_at)[0]
  );
}

/** Whether a job runs (polling continues while one does). */
export const anyRunning = (jobs: ArchiveJob[]) => jobs.some((job) => job.state === "running");

/** The share of a job done, 0–1, or null when unknown. */
export function jobFraction(job: ArchiveJob): number | null {
  if (job.state === "done") return 1;
  if (!job.bytes_total) return null;
  return Math.min(1, job.bytes_done / job.bytes_total);
}

export const phaseLabels: Record<string, string> = {
  checking: "Checking",
  packing: "Packing",
  uploading: "Uploading",
  verifying: "Verifying",
  removing: "Removing the folder",
  listing: "Listing archives",
  downloading: "Downloading",
  extracting: "Extracting",
  done: "Done",
};
export const phaseLabel = (phase: string) => phaseLabels[phase] || phase || "Starting";

/** The archives of `archive_list`, newest first as the bridge sends them. */
export function readArchives(value: any): ArchiveListing[] {
  return (Array.isArray(value?.archives) ? value.archives : [])
    .filter((a: any) => typeof a?.object === "string")
    .slice(0, 1000)
    .map((a: any) => ({
      object: a.object,
      size: Number.isSafeInteger(a.size) ? a.size : null,
      updated: text(a.updated),
      sha256: text(a.sha256),
      crc32c: text(a.crc32c),
      recorded: a.recorded === true,
    }));
}

/** The archive a restore offers first: the recorded one, else the newest. */
export const defaultArchive = (archives: ArchiveListing[]) =>
  archives.find((a) => a.recorded) || archives[0];

export function readArchiveStatus(value: any): ArchiveStatus | null {
  if (typeof value?.configured !== "boolean") return null;
  return {
    configured: value.configured,
    backend: text(value.backend),
    bucket: text(value.bucket),
    prefix: typeof value.prefix === "string" ? value.prefix : "kelpmux",
    token_source: value.token_source === "command" ? "command" : "metadata",
    token_label: clip(value.token_label) || "",
    error: text(value.error),
  };
}

/** The sidebar's projects in two lists: those listed as usual (paused ones
 * too, shown dimmed; Scratch last) and the archived ones, by name. */
export function splitArchived(projects: Project[]) {
  const listed: Project[] = [];
  const archived: Project[] = [];
  for (const project of projects)
    (project.status === "archived" && !project.scratch ? archived : listed).push(project);
  return { listed, archived };
}

/** A free folder for a restore beside the original: `<base>/<name>-2`,
 * `-3`… (as written; the bridge checks again). */
export function restoreFolder(base: string, name: string, taken: string[]) {
  const stem = `${base.replace(/\/+$/, "")}/${name}`;
  for (let n = 2; n < 100; n++) {
    const folder = `${stem}-${n}`;
    if (!taken.includes(folder)) return folder;
  }
  return `${stem}-restored`;
}

/** The status actions a project's menu offers (not Scratch's). */
export function statusActions(project: Project): ("pause" | "resume" | "archive" | "delete" | "restore" | "remove")[] {
  if (project.scratch || project.id === SCRATCH) return [];
  switch (project.status) {
    case "archived":
      return ["restore", "remove"];
    case "paused":
      return ["resume", "archive", "delete"];
    default:
      return ["pause", "archive", "delete"];
  }
}

/** `2026-11-24T10:00:00Z` as a short local date, or the text unchanged. */
export function archiveDate(value: string | null) {
  if (!value) return "";
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? value
    : date.toLocaleString("en-US", {
        year: "numeric",
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
}
