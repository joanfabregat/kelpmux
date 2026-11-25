// A checkout's history and diffs in the Repos view (bridge API minor 10,
// `git_history`), read-only: tabs Changes (staged, unstaged, untracked and
// conflicted files), History (a branch's or revision's log, first-parent
// and path filters, paged) and Branches (local with upstream and
// ahead/behind, remote-tracking), and a details pane beside them with a
// commit's message, people, parents and files, or a working file's
// diffs. The pane's place (checkout, tab, revision, commit, file) is the
// URL's (navigation.ts), so Back and Forward walk through it; on a phone
// or a narrow window the pane takes the list's place.
import React, { useEffect, useRef, useState } from "react";
import { Icon } from "./status";
import { Tooltip } from "./Tooltip";
import { aheadBehind, relativeTime, shortSha } from "./git";
import {
  HUNK_COLLAPSE,
  HUNK_PREVIEW,
  diffStats,
  formatSize,
  numberLines,
  readBranches,
  readChanges,
  readCommit,
  readDiff,
  readLogPage,
  refLabels,
  sidesOf,
  validRepoPath,
  type Branch,
  type Branches,
  type CommitDetails,
  type DiffKind,
  type DiffResult,
  type FileChange,
  type FileDiff,
  type Hunk,
  type LogEntry,
  type RepoTab,
  type WorkingChanges,
} from "./git-history";
import type { ReposPlace } from "./navigation";
import type { Rpc } from "./GitHub";
import "./styles/repo-history.css";

const message = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

/** Asks the bridge once per `key`, reading the answer with `read`. */
function useQuery<T>(
  ask: (() => Promise<any>) | null,
  read: (value: any) => T | null,
  key: string,
) {
  const [state, setState] = useState<{
    key: string;
    data?: T;
    error?: string;
    loading: boolean;
  }>({ key: "", loading: false });
  const latest = useRef(ask);
  latest.current = ask;
  const reader = useRef(read);
  reader.current = read;
  const enabled = !!ask;
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const run = latest.current;
    if (!run) return;
    let alive = true;
    setState((s) => ({ ...s, loading: true, error: undefined }));
    run()
      .then((value) => {
        if (!alive) return;
        const data = reader.current(value);
        setState(
          data
            ? { key, data, loading: false }
            : { key, loading: false, error: "Unexpected answer from the server" },
        );
      })
      .catch((error) => alive && setState({ key, loading: false, error: message(error) }));
    return () => {
      alive = false;
    };
  }, [key, tick, enabled]);
  return {
    // A previous key's answer stays shown while the next one loads.
    data: state.data,
    stale: state.key !== key,
    error: state.error,
    loading: state.loading || state.key !== key,
    reload: () => setTick((n) => n + 1),
  };
}

const statusNames: Record<string, string> = {
  A: "Added",
  M: "Modified",
  D: "Deleted",
  R: "Renamed",
  C: "Copied",
  T: "Type changed",
  U: "Unmerged",
  "?": "Untracked",
};
function StatusLetter({ status }: { status: string }) {
  return (
    <span
      className={`git-status git-status-${status === "?" ? "untracked" : status.toLowerCase()}`}
      title={statusNames[status] || "Changed"}
      aria-hidden="true"
    >
      {status}
    </span>
  );
}

function Counts({ added, removed, binary }: { added: number | null; removed: number | null; binary?: boolean }) {
  if (binary) return <span className="git-lines faint">binary</span>;
  if (added === null) return null;
  return (
    <span className="git-lines">
      <span className="git-added">+{added}</span>
      <span className="git-removed">−{removed || 0}</span>
    </span>
  );
}

function Refs({ refs }: { refs: string[] }) {
  if (!refs.length) return null;
  return (
    <span className="rh-refs">
      {refLabels(refs).map((ref) => (
        <span key={`${ref.kind}:${ref.label}`} className={`rh-ref rh-ref-${ref.kind}`}>
          {ref.label}
        </span>
      ))}
    </span>
  );
}

// ───── Diffs ─────

function HunkView({ hunk }: { hunk: Hunk }) {
  const long = hunk.lines.length > HUNK_COLLAPSE;
  const [open, setOpen] = useState(!long);
  const rows = numberLines(hunk);
  const shown = open ? rows : rows.slice(0, HUNK_PREVIEW);
  return (
    <tbody className="rh-hunk">
      <tr className="rh-hunk-head">
        <td className="rh-ln" aria-hidden="true">…</td>
        <td className="rh-ln" aria-hidden="true">…</td>
        <td className="rh-code">{hunk.header}</td>
      </tr>
      {shown.map((row, index) => (
        <tr key={index} className={`rh-line rh-${row.kind}`}>
          <td className="rh-ln">{row.old ?? ""}</td>
          <td className="rh-ln">{row.new ?? ""}</td>
          <td className="rh-code">
            <span className="rh-mark" aria-hidden="true">
              {row.kind === "add" ? "+" : row.kind === "del" ? "−" : " "}
            </span>
            {row.text}
          </td>
        </tr>
      ))}
      {long && (
        <tr className="rh-hunk-more">
          <td colSpan={3}>
            <button type="button" className="subtle small" onClick={() => setOpen(!open)}>
              {open ? "Collapse this hunk" : `Show ${rows.length - HUNK_PREVIEW} more lines`}
            </button>
          </td>
        </tr>
      )}
    </tbody>
  );
}

/** One file's diff: binary and oversized files are summarised, cut
 * diffs say so. */
export function FileDiffView({ file, title }: { file: FileDiff; title?: string }) {
  const stats = diffStats(file);
  return (
    <div className="rh-diff" data-diff={file.path}>
      {title && <div className="rh-diff-title faint">{title}</div>}
      {file.meta.length > 0 && (
        <div className="rh-diff-meta faint">
          {file.meta.map((line) => (
            <div key={line}>{line}</div>
          ))}
        </div>
      )}
      {file.binary ? (
        <p className="rh-note faint">Binary file, not shown.</p>
      ) : file.too_large !== undefined ? (
        <p className="rh-note faint">
          Too large to show ({formatSize(file.too_large)}).
        </p>
      ) : !file.hunks.length ? (
        <p className="rh-note faint">No content changes.</p>
      ) : (
        <div className="rh-table-wrap">
          <table className="rh-table" aria-label={`Diff of ${file.path}, +${stats.added} −${stats.removed}`}>
            {file.hunks.map((hunk, index) => (
              <HunkView key={index} hunk={hunk} />
            ))}
          </table>
        </div>
      )}
      {file.truncated && (
        <p className="rh-note rh-truncated" role="note">
          This diff is too large: only its beginning is shown.
        </p>
      )}
    </div>
  );
}

/** Asks for and shows one file's diff. */
function DiffLoader({
  rpc,
  base,
  kind,
  rev,
  path,
  from,
  title,
}: {
  rpc: Rpc;
  base: { project: string; repo: string };
  kind: DiffKind;
  rev?: string;
  path: string;
  from?: string;
  title?: string;
}) {
  const query = useQuery<DiffResult>(
    () =>
      rpc("git_diff", {
        ...base,
        kind,
        path,
        ...(rev ? { rev } : {}),
        ...(from ? { from } : {}),
      }),
    readDiff,
    JSON.stringify([base.repo, kind, rev, path, from]),
  );
  if (query.error) return <p className="git-error rh-note">{query.error}</p>;
  if (!query.data || query.stale)
    return <p className="rh-note faint">Loading the diff…</p>;
  if (!query.data.files.length)
    return <p className="rh-note faint">No differences.</p>;
  return (
    <>
      {query.data.files.map((file, index) => (
        <FileDiffView key={`${file.path}:${index}`} file={file} title={title} />
      ))}
    </>
  );
}

// ───── Details pane ─────

function PaneHead({
  title,
  onClose,
  children,
}: {
  title: React.ReactNode;
  onClose: () => void;
  children?: React.ReactNode;
}) {
  return (
    <div className="rh-pane-head">
      <div className="rh-pane-title">{title}</div>
      {children}
      <Tooltip content="Close">
        <button type="button" className="icon-btn" aria-label="Close the details" onClick={onClose}>
          <Icon name="x" size="sm" />
        </button>
      </Tooltip>
    </div>
  );
}

function CommitFile({
  rpc,
  base,
  sha,
  file,
  open,
  onToggle,
}: {
  rpc: Rpc;
  base: { project: string; repo: string };
  sha: string;
  file: FileChange;
  open: boolean;
  onToggle: () => void;
}) {
  return (
    <li className="rh-file" data-path={file.path} data-status={file.status}>
      <button
        type="button"
        className="git-change-open"
        aria-expanded={open}
        aria-label={`${file.path}, ${statusNames[file.status] || "changed"}: ${open ? "hide" : "show"} its diff`}
        onClick={onToggle}
      >
        <Icon name={open ? "chevron-down" : "chevron-right"} size="sm" className="git-chevron" />
        <StatusLetter status={file.status} />
        <span className="git-change-path">
          {file.from && <span className="git-change-from">{file.from} → </span>}
          {file.path}
        </span>
        <Counts added={file.added} removed={file.removed} binary={file.binary} />
      </button>
      {open && (
        <DiffLoader rpc={rpc} base={base} kind="commit" rev={sha} path={file.path} from={file.from} />
      )}
    </li>
  );
}

/** Files shown expanded when a commit opens: all of a small commit. */
const OPEN_FILES = 5;

function CommitPane({
  rpc,
  base,
  sha,
  onClose,
  onCommit,
}: {
  rpc: Rpc;
  base: { project: string; repo: string };
  sha: string;
  onClose: () => void;
  onCommit: (sha: string) => void;
}) {
  const query = useQuery<CommitDetails>(
    () => rpc("git_commit", { ...base, rev: sha }),
    readCommit,
    `${base.repo}:${sha}`,
  );
  const [open, setOpen] = useState<Set<string> | null>(null);
  const commit = query.stale ? undefined : query.data;
  useEffect(() => setOpen(null), [sha]);
  const opened =
    open ||
    new Set(commit && commit.files.length <= OPEN_FILES ? commit.files.map((f) => f.path) : []);
  const toggle = (path: string) => {
    const next = new Set(opened);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    setOpen(next);
  };
  const [subject, ...body] = (commit?.message || "").split("\n");
  const when = (time: number) => (
    <span title={new Date(time * 1000).toLocaleString()}>{relativeTime(time)}</span>
  );
  return (
    <section className="rh-pane" aria-label={`Commit ${shortSha(sha)}`} data-commit={sha}>
      <PaneHead
        title={
          <>
            <Icon name="git-branch" size="sm" /> Commit <code>{shortSha(sha)}</code>
          </>
        }
        onClose={onClose}
      />
      {query.error ? (
        <p className="git-error rh-note">{query.error}</p>
      ) : !commit ? (
        <p className="rh-note faint">Loading the commit…</p>
      ) : (
        <div className="rh-pane-body">
          <h3 className="rh-subject">{subject}</h3>
          <Refs refs={commit.refs} />
          {body.join("\n").trim() && <pre className="rh-message">{body.join("\n").trim()}</pre>}
          {commit.message_truncated && (
            <p className="rh-note faint">The message is too long: only its beginning is shown.</p>
          )}
          <dl className="rh-facts">
            <dt>Author</dt>
            <dd>
              {commit.author} <span className="faint">&lt;{commit.author_email}&gt;</span> · {when(commit.author_time)}
            </dd>
            {(commit.committer !== commit.author || commit.commit_time !== commit.author_time) && (
              <>
                <dt>Committer</dt>
                <dd>
                  {commit.committer} <span className="faint">&lt;{commit.committer_email}&gt;</span> ·{" "}
                  {when(commit.commit_time)}
                </dd>
              </>
            )}
            <dt>{commit.parents.length === 1 ? "Parent" : "Parents"}</dt>
            <dd className="rh-parents">
              {commit.parents.length
                ? commit.parents.map((parent) => (
                    <button
                      key={parent}
                      type="button"
                      className="subtle small rh-parent"
                      aria-label={`Open parent ${shortSha(parent)}`}
                      onClick={() => onCommit(parent)}
                    >
                      <code>{shortSha(parent)}</code>
                    </button>
                  ))
                : "none (first commit)"}
            </dd>
            <dt>Commit</dt>
            <dd>
              <code className="rh-sha">{commit.sha}</code>
            </dd>
          </dl>
          <h4 className="rh-files-head">
            {commit.files.length}
            {commit.files_truncated ? "+" : ""} {commit.files.length === 1 ? "file" : "files"} changed
            {commit.merge && <span className="faint"> (against the first parent)</span>}
          </h4>
          <ul className="rh-files" aria-label="Changed files">
            {commit.files.map((file) => (
              <CommitFile
                key={file.path}
                rpc={rpc}
                base={base}
                sha={commit.sha}
                file={file}
                open={opened.has(file.path)}
                onToggle={() => toggle(file.path)}
              />
            ))}
          </ul>
          {commit.files_truncated && (
            <p className="rh-note faint">Only the first {commit.files.length} files are listed.</p>
          )}
        </div>
      )}
    </section>
  );
}

function ChangePane({
  rpc,
  base,
  path,
  changes,
  onClose,
  onOpenFile,
}: {
  rpc: Rpc;
  base: { project: string; repo: string };
  path: string;
  changes?: WorkingChanges;
  onClose: () => void;
  onOpenFile?: () => void;
}) {
  const sides = changes ? sidesOf(changes, path) : null;
  const none =
    sides && !sides.staged && !sides.unstaged && !sides.untracked && !sides.conflicted;
  return (
    <section className="rh-pane" aria-label={`Changes of ${path}`} data-change={path}>
      <PaneHead title={<code className="rh-pane-path">{path}</code>} onClose={onClose}>
        {onOpenFile && sides && !(sides.unstaged?.status === "D") && (
          <Tooltip content="Open in the editor">
            <button type="button" className="icon-btn" aria-label={`Open ${path} in the editor`} onClick={onOpenFile}>
              <Icon name="pencil" size="sm" />
            </button>
          </Tooltip>
        )}
      </PaneHead>
      <div className="rh-pane-body">
        {!sides ? (
          <p className="rh-note faint">Loading…</p>
        ) : none ? (
          <p className="rh-note faint">This file has no uncommitted changes any more.</p>
        ) : (
          <>
            {sides.conflicted && (
              <p className="rh-note git-error">Unmerged: resolve the conflict in a terminal or the editor.</p>
            )}
            {sides.staged && (
              <DiffLoader
                rpc={rpc}
                base={base}
                kind="staged"
                path={path}
                from={sides.staged.from}
                title="Staged"
              />
            )}
            {sides.unstaged && (
              <DiffLoader rpc={rpc} base={base} kind="unstaged" path={path} title="Not staged" />
            )}
            {sides.untracked && (
              <DiffLoader rpc={rpc} base={base} kind="untracked" path={path} title="Untracked" />
            )}
          </>
        )}
      </div>
    </section>
  );
}

// ───── Tabs ─────

function ChangeRow({
  file,
  selected,
  onSelect,
}: {
  file: FileChange;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <li className={`git-change ${selected ? "selected" : ""}`} data-path={file.path} data-status={file.status}>
      <button
        type="button"
        className="git-change-open"
        aria-current={selected || undefined}
        aria-label={`${file.path}, ${statusNames[file.status] || "changed"}: show its diff`}
        onClick={onSelect}
      >
        <StatusLetter status={file.status} />
        <span className="git-change-path">
          {file.from && <span className="git-change-from">{file.from} → </span>}
          {file.path}
        </span>
        <Counts added={file.added} removed={file.removed} binary={file.binary} />
      </button>
    </li>
  );
}

function ChangesTab({
  changes,
  error,
  selected,
  onSelect,
}: {
  changes?: WorkingChanges;
  error?: string;
  selected: string;
  onSelect: (path: string) => void;
}) {
  if (error) return <p className="git-error rh-note">{error}</p>;
  if (!changes) return <p className="rh-note faint">Reading the working changes…</p>;
  const sections: [string, FileChange[]][] = [
    ["Conflicted", changes.conflicted.map((path) => ({ path, status: "U", added: null, removed: null, binary: false }))],
    ["Staged", changes.staged],
    ["Not staged", changes.unstaged],
    [
      "Untracked",
      changes.untracked.map((file) => ({
        path: file.path,
        status: "?",
        added: file.added,
        removed: file.added === null ? null : 0,
        binary: file.binary,
      })),
    ],
  ];
  const shown = sections.filter(([, files]) => files.length);
  if (!shown.length) return <p className="rh-note faint">No uncommitted changes.</p>;
  return (
    <div className="rh-changes">
      {shown.map(([title, files]) => (
        <section key={title} className="rh-section" aria-label={title}>
          <h4 className="rh-section-head">
            {title} <span className="faint">{files.length}</span>
          </h4>
          <ul className="rh-list">
            {files.map((file) => (
              <ChangeRow
                key={`${title}:${file.path}`}
                file={file}
                selected={selected === file.path}
                onSelect={() => onSelect(file.path)}
              />
            ))}
          </ul>
        </section>
      ))}
      {changes.truncated && <p className="rh-note faint">Some changes are not listed (too many).</p>}
    </div>
  );
}

function CommitRow({
  entry,
  selected,
  onSelect,
}: {
  entry: LogEntry;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <li className={`rh-commit ${selected ? "selected" : ""}`} data-sha={entry.sha}>
      <button
        type="button"
        className="rh-commit-open"
        aria-current={selected || undefined}
        aria-label={`Commit ${shortSha(entry.sha)}: ${entry.subject}`}
        onClick={onSelect}
      >
        <span className="rh-commit-main">
          <span className="rh-commit-subject">{entry.subject || "(no message)"}</span>
          <Refs refs={entry.refs} />
        </span>
        <span className="rh-commit-meta faint">
          <code>{shortSha(entry.sha)}</code>
          {entry.parents.length > 1 && <span className="rh-merge">merge</span>}
          <span>{entry.author}</span>
          <span title={new Date(entry.time * 1000).toLocaleString()}>{relativeTime(entry.time)}</span>
        </span>
      </button>
    </li>
  );
}

/** Commits per page of the History tab. */
const PAGE = 50;

function HistoryTab({
  rpc,
  base,
  rev,
  commit,
  branches,
  onRev,
  onCommit,
  refreshKey,
}: {
  rpc: Rpc;
  base: { project: string; repo: string };
  rev: string;
  commit: string;
  branches?: Branches;
  onRev: (rev: string) => void;
  onCommit: (sha: string) => void;
  refreshKey: string;
}) {
  const [firstParent, setFirstParent] = useState(false);
  const [filter, setFilter] = useState("");
  const [path, setPath] = useState("");
  const [pages, setPages] = useState(1);
  const key = JSON.stringify([base.repo, rev, firstParent, path, pages, refreshKey]);
  const query = useQuery(
    () =>
      rpc("git_log", {
        ...base,
        rev: rev || "HEAD",
        limit: Math.min(200, PAGE * pages),
        first_parent: firstParent,
        ...(path ? { path } : {}),
      }),
    readLogPage,
    key,
  );
  useEffect(() => setPages(1), [rev, firstParent, path]);
  const page = query.data;
  const filterValid = !filter || validRepoPath(filter);
  const names = [
    ...(branches?.branches || []).map((b) => b.name),
    ...(branches?.remotes || []).map((b) => b.name),
  ];
  const shownRev = rev || "HEAD";
  if (!names.includes(shownRev)) names.unshift(shownRev);
  if (shownRev !== "HEAD") names.unshift("HEAD");
  return (
    <div className="rh-history">
      <div className="rh-toolbar">
        <label className="rh-field">
          <span className="faint">Revision</span>
          <select
            value={shownRev}
            aria-label="Revision"
            onChange={(event) => onRev(event.target.value === "HEAD" ? "" : event.target.value)}
          >
            {names.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
        </label>
        <label className="rh-check">
          <input
            type="checkbox"
            checked={firstParent}
            onChange={(event) => setFirstParent(event.target.checked)}
          />
          First parent only
        </label>
        <form
          className="rh-field rh-path"
          onSubmit={(event) => {
            event.preventDefault();
            if (filterValid) setPath(filter.trim());
          }}
        >
          <input
            type="search"
            placeholder="Filter by path (Enter)"
            aria-label="Filter by path"
            aria-invalid={!filterValid || undefined}
            value={filter}
            onChange={(event) => {
              setFilter(event.target.value);
              if (!event.target.value) setPath("");
            }}
          />
        </form>
      </div>
      {!filterValid && <p className="rh-note git-error">A relative path inside the repository, without “..”.</p>}
      {query.error ? (
        <p className="git-error rh-note">{query.error}</p>
      ) : !page ? (
        <p className="rh-note faint">Reading the history…</p>
      ) : !page.commits.length ? (
        <p className="rh-note faint">No commits{path ? ` touch ${path}` : ""}.</p>
      ) : (
        <>
          <ul className="rh-log" aria-label={`History of ${shownRev}`} aria-busy={query.loading || undefined}>
            {page.commits.map((entry) => (
              <CommitRow
                key={entry.sha}
                entry={entry}
                selected={commit === entry.sha}
                onSelect={() => onCommit(entry.sha)}
              />
            ))}
          </ul>
          {page.more &&
            (pages * PAGE < 200 ? (
              <button
                type="button"
                className="subtle small rh-more"
                disabled={query.loading}
                onClick={() => setPages(pages + 1)}
              >
                {query.loading ? "Loading…" : "Load more"}
              </button>
            ) : (
              <p className="rh-note faint">Only the newest 200 commits are listed.</p>
            ))}
        </>
      )}
    </div>
  );
}

function BranchRow({ branch, onOpen }: { branch: Branch; onOpen: () => void }) {
  return (
    <li className={`rh-branch ${branch.current ? "current" : ""}`} data-branch={branch.name}>
      <button
        type="button"
        className="rh-commit-open"
        aria-label={`${branch.name}${branch.current ? " (current)" : ""}: show its history`}
        onClick={onOpen}
      >
        <span className="rh-commit-main">
          <span className={`git-branch ${branch.current ? "" : "detached"}`}>
            <Icon name="git-branch" size="sm" />
            {branch.name}
          </span>
          {branch.current && <span className="git-tag">current</span>}
          {branch.upstream && (
            <span className="git-upstream">
              {branch.upstream}
              {branch.gone ? (
                <span className="faint"> (gone)</span>
              ) : aheadBehind(branch) ? (
                <span className="git-ab"> {aheadBehind(branch)}</span>
              ) : null}
            </span>
          )}
        </span>
        <span className="rh-commit-meta faint">
          <code>{shortSha(branch.sha)}</code>
          <span className="rh-branch-subject">{branch.subject}</span>
          <span>{branch.author}</span>
          <span title={new Date(branch.time * 1000).toLocaleString()}>{relativeTime(branch.time)}</span>
        </span>
      </button>
    </li>
  );
}

function BranchesTab({
  branches,
  error,
  onOpen,
}: {
  branches?: Branches;
  error?: string;
  onOpen: (name: string) => void;
}) {
  if (error) return <p className="git-error rh-note">{error}</p>;
  if (!branches) return <p className="rh-note faint">Reading the branches…</p>;
  return (
    <div className="rh-branches">
      {branches.detached && branches.head && (
        <p className="rh-note">
          HEAD is detached at <code>{shortSha(branches.head)}</code>.
        </p>
      )}
      <section className="rh-section" aria-label="Local branches">
        <h4 className="rh-section-head">
          Local <span className="faint">{branches.branches.length}</span>
        </h4>
        {branches.branches.length ? (
          <ul className="rh-list">
            {branches.branches.map((branch) => (
              <BranchRow key={branch.ref} branch={branch} onOpen={() => onOpen(branch.name)} />
            ))}
          </ul>
        ) : (
          <p className="rh-note faint">No branches yet.</p>
        )}
      </section>
      {branches.remotes.length > 0 && (
        <section className="rh-section" aria-label="Remote-tracking branches">
          <h4 className="rh-section-head">
            Remote-tracking <span className="faint">{branches.remotes.length}</span>
          </h4>
          <ul className="rh-list">
            {branches.remotes.map((branch) => (
              <BranchRow key={branch.ref} branch={branch} onOpen={() => onOpen(branch.name)} />
            ))}
          </ul>
        </section>
      )}
      {branches.truncated && <p className="rh-note faint">Only the newest branches are listed.</p>}
      <p className="rh-note faint">Upstream counts are as of the last fetch; KelpMux never fetches.</p>
    </div>
  );
}

const tabNames: Record<RepoTab, string> = {
  changes: "Changes",
  history: "History",
  branches: "Branches",
};

/**
 * A checkout's tabs and details pane. `place` is the URL's Repos place for
 * this checkout; `onPlace` moves to another (a new history entry).
 * `changesKey` changes with the checkout's working changes and
 * `historyKey` with its commits and refs (from the `git` facts), so the
 * open tab asks again.
 */
export function CheckoutPanel({
  rpc,
  project,
  repo,
  place,
  defaultTab,
  changesKey,
  historyKey,
  onPlace,
  onOpenFile,
  changeCount,
}: {
  rpc: Rpc;
  project: string;
  repo: string;
  place: ReposPlace;
  defaultTab: RepoTab;
  changesKey: string;
  historyKey: string;
  onPlace: (place: Partial<ReposPlace>) => void;
  onOpenFile?: (path: string) => void;
  changeCount: number;
}) {
  const base = { project, repo };
  const tab = (place.tab as RepoTab) || defaultTab;
  const changes = useQuery<WorkingChanges>(
    tab === "changes" ? () => rpc("git_changes", base) : null,
    readChanges,
    `${repo}:${changesKey}`,
  );
  const branches = useQuery<Branches>(
    tab !== "changes" ? () => rpc("git_branches", base) : null,
    readBranches,
    `${repo}:${historyKey}`,
  );
  const detail =
    tab === "history" && place.commit
      ? "commit"
      : tab === "changes" && place.change
        ? "change"
        : "";
  return (
    <div className={`rh-panel ${detail ? "has-detail" : ""}`} data-tab={tab}>
      <div className="rh-main">
        <div className="rh-tabs" role="tablist" aria-label="Checkout views">
          {(Object.keys(tabNames) as RepoTab[]).map((name) => (
            <button
              key={name}
              type="button"
              role="tab"
              aria-selected={tab === name}
              className={`rh-tab ${tab === name ? "active" : ""}`}
              onClick={() => onPlace({ tab: name, rev: "", commit: "", change: "" })}
            >
              {tabNames[name]}
              {name === "changes" && changeCount > 0 && <span className="rh-tab-count">{changeCount}</span>}
            </button>
          ))}
        </div>
        <div className="rh-tab-body" role="tabpanel" aria-label={tabNames[tab]}>
          {tab === "changes" ? (
            <ChangesTab
              changes={changes.data}
              error={changes.error}
              selected={place.change}
              onSelect={(path) => onPlace({ tab: "changes", change: path })}
            />
          ) : tab === "history" ? (
            <HistoryTab
              rpc={rpc}
              base={base}
              rev={place.rev}
              commit={place.commit}
              branches={branches.data}
              refreshKey={historyKey}
              onRev={(rev) => onPlace({ tab: "history", rev, commit: "" })}
              onCommit={(sha) => onPlace({ tab: "history", rev: place.rev, commit: sha })}
            />
          ) : (
            <BranchesTab
              branches={branches.data}
              error={branches.error}
              onOpen={(name) => onPlace({ tab: "history", rev: name, commit: "" })}
            />
          )}
        </div>
      </div>
      {detail === "commit" && (
        <CommitPane
          rpc={rpc}
          base={base}
          sha={place.commit}
          onClose={() => onPlace({ tab: "history", rev: place.rev, commit: "" })}
          onCommit={(sha) => onPlace({ tab: "history", rev: place.rev, commit: sha })}
        />
      )}
      {detail === "change" && (
        <ChangePane
          rpc={rpc}
          base={base}
          path={place.change}
          changes={changes.data}
          onClose={() => onPlace({ tab: "changes", change: "" })}
          onOpenFile={onOpenFile ? () => onOpenFile(place.change) : undefined}
        />
      )}
    </div>
  );
}
