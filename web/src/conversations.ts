// History's data (bridge `closed`, and `conversations` from API minor 7):
// closed terminals with every agent conversation they hosted, and agent
// conversations started outside KelpMux in a project's folder. Shared by
// the History panel (Panels.tsx) and a project's home page
// (ProjectHome.tsx): the rows, newest first, with the project each one
// belongs to, their labels, and the bridge requests that resume a
// conversation or restore a closed terminal. No React here.
import { age, basename } from "./format.ts";
import { SCRATCH, type Project } from "./projects.ts";

/** One agent conversation of History (bridge API minor 7): one a closed
 * terminal hosted, or one started outside KelpMux (`project` set). */
export type Conversation = {
  kind: string;
  id: string;
  cwd: string;
  /** Where resuming opens: its folder, else its terminal's (or project's);
   * null when neither exists. */
  folder?: string | null;
  /** From its first prompt (or its terminal's name). */
  title?: string | null;
  titled?: boolean;
  first_seen?: number | null;
  last_seen?: number | null;
  /** An outside conversation's project (`scratch` for Scratch). */
  project?: string;
  /** Linked to its closed terminal by folder and time: the server never
   * identified the terminal's conversation and this transcript is the
   * only one that can be it. */
  matched?: boolean;
};
export type ClosedEntry = {
  key: string;
  workspace: string;
  pane: number;
  cwd: string;
  name: string;
  kind?: string | null;
  session_id?: string | null;
  argv: string[];
  closed_at?: number | null;
  reason: string;
  choices: ("resume" | "restart" | "folder")[];
  /** The project the terminal belonged to (bridge API minor 5); none or
   * one that no longer exists: Scratch. */
  project?: string | null;
  /** The conversations it hosted, newest first (bridge API minor 7). */
  conversations?: Conversation[];
};
export type HistoryListing = {
  workspace: string;
  entries: ClosedEntry[];
  /** Conversations started outside KelpMux (bridge API minor 7). */
  external?: Conversation[];
  truncated?: boolean;
};

/** One row of History: a closed terminal (with its conversations) or a
 * conversation started outside KelpMux; newest first. */
export type HistoryRow =
  | { type: "terminal"; at: number; entry: ClosedEntry; project: string }
  | { type: "external"; at: number; conversation: Conversation; project: string };

export const choiceLabel = {
  resume: "Resume",
  restart: "Rerun",
  folder: "Open folder",
};
/** A closed agent terminal whose conversation is not known: it offers its
 * agent's picker, never a Resume. */
export const unidentified = (entry: ClosedEntry) =>
  (entry.kind === "codex" || entry.kind === "claude") && !entry.session_id;
/** "Open Codex's picker here": the agent's own conversation picker. */
export const pickerLabel = (kind?: string | null) =>
  `Open ${kind === "codex" ? "Codex" : "Claude"}'s picker here`;
export const pickerHint = (entry: ClosedEntry) =>
  `KelpMux did not record which conversation this terminal ran: runs “${resumeCommand(
    { kind: entry.kind },
  )}” in ${entry.cwd}, where you choose it`;
export const matchedHint =
  "KelpMux did not record this terminal's conversation; this is the only one of its tool in that folder that ended by the time the terminal closed";
/** A restore choice's label: the server's `resume` of an unidentified
 * agent opens its picker, and says so. */
export const choiceText = (
  entry: ClosedEntry,
  choice: ClosedEntry["choices"][0],
) =>
  choice === "resume" && unidentified(entry)
    ? pickerLabel(entry.kind)
    : choiceLabel[choice];
export const choiceHint = (
  entry: ClosedEntry,
  choice: ClosedEntry["choices"][0],
) =>
  choice === "resume"
    ? entry.session_id
      ? "Resume this conversation in a new terminal"
      : pickerHint(entry)
    : choice === "restart"
      ? `Rerun ${entry.argv.join(" ")} in its folder`
      : "Open a shell in this folder";

/** The command the TUI copies for an agent conversation. */
export function resumeCommand(
  entry: Pick<ClosedEntry, "kind"> & { session_id?: string | null },
) {
  const base =
    entry.kind === "codex"
      ? "codex resume"
      : entry.kind === "claude"
        ? "claude --resume"
        : "";
  if (!base) return "";
  const id = entry.session_id;
  return id && /^[A-Za-z0-9_-]{1,128}$/.test(id) ? `${base} ${id}` : base;
}
export const toolName = (kind: string) =>
  kind === "codex" ? "Codex" : kind === "claude" ? "Claude Code" : kind;
/** When a conversation ran: "3h ago" or "2d ago – 3h ago". */
export function span(c: Conversation) {
  const first = c.first_seen || 0;
  const last = c.last_seen || 0;
  if (!first && !last) return "";
  if (!first || !last || age(first) === age(last)) return age(last || first);
  return `${age(first)} – ${age(last)}`;
}
export function exactSpan(c: Conversation) {
  const when = (at?: number | null) =>
    at ? new Date(at * 1000).toLocaleString() : "";
  return [when(c.first_seen), when(c.last_seen)].filter(Boolean).join(" – ");
}
export const conversationTitle = (c: Conversation) =>
  c.title || `Conversation in ${basename(c.cwd || "/")}`;

/** Every row of a listing, newest first, each with its project: its link
 * when that project still exists, else Scratch. */
export function historyRows(
  listing: HistoryListing,
  projects: Project[] = [],
): HistoryRow[] {
  const known = (id?: string | null) =>
    !!id && projects.some((project) => project.id === id);
  const projectOfId = (id?: string | null) => (known(id) ? id! : SCRATCH);
  return [
    ...listing.entries.map(
      (entry): HistoryRow => ({
        type: "terminal",
        at: entry.closed_at || 0,
        entry,
        project: projectOfId(entry.project),
      }),
    ),
    ...(listing.external || []).map(
      (conversation): HistoryRow => ({
        type: "external",
        at: conversation.last_seen || 0,
        conversation,
        project: projectOfId(conversation.project),
      }),
    ),
  ]
    // Newest first; the bridge's order (this workspace first) breaks ties.
    .map((row, index) => ({ row, index }))
    .sort((a, b) => b.row.at - a.row.at || a.index - b.index)
    .map(({ row }) => row);
}

/** The bridge's `new` that resumes a conversation in a new terminal of
 * `project`, in its folder (null when it has none); the bridge builds the
 * command from the kind and id. */
export function resumeRequest(c: Conversation, project: string) {
  if (!c.folder) return null;
  return {
    request: crypto.randomUUID(),
    ...(project !== SCRATCH ? { project } : {}),
    cwd: c.folder,
    resume: { kind: c.kind, id: c.id },
  };
}
/** The bridge's `new` that opens an unidentified agent terminal's picker
 * (`claude --resume`, `codex resume`, no id) in a new terminal of
 * `project`, in the terminal's folder. */
export function pickerRequest(entry: ClosedEntry, project: string) {
  return {
    request: crypto.randomUUID(),
    ...(project !== SCRATCH ? { project } : {}),
    cwd: entry.cwd,
    resume: { kind: entry.kind, picker: true },
  };
}
/** The bridge's `new` that opens a shell in a closed terminal's folder,
 * in `project`. */
export function folderRequest(entry: ClosedEntry, project: string) {
  return {
    request: crypto.randomUUID(),
    ...(project !== SCRATCH ? { project } : {}),
    cwd: entry.cwd,
  };
}
/** The bridge's `restore` of a closed terminal with one of its choices. */
export function restoreRequest(entry: ClosedEntry, choice: string) {
  return {
    key: entry.key,
    choice,
    request: crypto.randomUUID(),
    confirmed: true,
  };
}
