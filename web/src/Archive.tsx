// Project status, archive, restore and delete in the web client (bridge
// operation `archive`, API minor 11):
// - useArchiveJobs: the background jobs (`archive_jobs`), polled every
//   second while one runs, so every browser sees a job another started.
// - RemovalDialog: Archive… and Delete… from a project's menu. It runs the
//   safe-removal check (`project_check_removal`) and shows what is on no
//   remote (per checkout: uncommitted changes, untracked files, stashes,
//   branches without upstream or with commits on no remote) and the other
//   files of the folder, then asks for confirmation (Delete: the project's
//   name typed), starts the job with the check's fingerprint (the bridge
//   checks again and refuses when it changed) and shows its progress with
//   Cancel until the folder's removal starts.
// - ArchivedPanel: an archived project's home page: when and where it was
//   archived, and Restore… (one of `archive_list`, into its folder or,
//   when that exists again, a new folder under projects.base).
// - ArchiveSettings: Settings → Archive, read-only (`[archive]` is edited
//   in config.toml only), with Test (a tiny write and delete).
// The bridge reads and writes the archive store; the browser never sees a
// token.
import React, { useCallback, useEffect, useRef, useState } from "react";
import { Icon } from "./status";
import { bytes } from "./format";
import type { Project } from "./projects";
import {
  archiveDate,
  anyRunning,
  checkoutProblems,
  checkSummary,
  defaultArchive,
  jobFraction,
  jobOf,
  phaseLabel,
  readArchives,
  readArchiveStatus,
  readCheck,
  readJobs,
  restoreFolder,
  sortedItems,
  type ArchiveJob,
  type ArchiveListing,
  type ArchiveStatus,
  type RemovalCheck,
} from "./archive-state";
import "./styles/archive.css";

/** A bridge request that rejects with the bridge's error. */
export type Rpc = (method: string, params?: any) => Promise<any>;

const message = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

export type ArchiveJobs = {
  jobs: ArchiveJob[];
  /** Asks the bridge now. */
  refresh: () => Promise<void>;
  /** Takes the jobs of a job-starting result and polls while one runs. */
  update: (jobs: ArchiveJob[]) => void;
};

/** The archive jobs, polled every second while one runs; `onFinished`
 * hears of each job that ends while this browser watches. */
export function useArchiveJobs(
  rpc: Rpc | null,
  onFinished?: (job: ArchiveJob) => void,
): ArchiveJobs {
  const [jobs, setJobs] = useState<ArchiveJob[]>([]);
  const seen = useRef(new Map<string, string>());
  const finished = useRef(onFinished);
  finished.current = onFinished;
  const latest = useRef(rpc);
  latest.current = rpc;
  const update = useCallback((next: ArchiveJob[]) => {
    for (const job of next) {
      const before = seen.current.get(job.id);
      if (before === "running" && job.state !== "running") finished.current?.(job);
      seen.current.set(job.id, job.state);
    }
    setJobs(next);
  }, []);
  const refresh = useCallback(async () => {
    if (!latest.current) return;
    try {
      update(readJobs(await latest.current("archive_jobs")));
    } catch {
      // The next poll asks again.
    }
  }, [update]);
  const enabled = !!rpc;
  useEffect(() => {
    if (enabled) void refresh();
  }, [enabled, refresh]);
  const running = anyRunning(jobs);
  useEffect(() => {
    if (!enabled || !running) return;
    const timer = setInterval(() => void refresh(), 1000);
    return () => clearInterval(timer);
  }, [enabled, running, refresh]);
  return { jobs, refresh, update };
}

const kindLabel = { archive: "Archive", restore: "Restore", delete: "Delete" } as const;

/** A job's phase, bar and Cancel (while it can still be cancelled). */
export function JobProgress({
  job,
  onCancel,
}: {
  job: ArchiveJob;
  onCancel?: () => void;
}) {
  const fraction = jobFraction(job);
  const amount =
    job.bytes_total !== null && job.bytes_total > 0
      ? `${bytes(job.bytes_done)} of ${bytes(job.bytes_total)}`
      : job.bytes_done
        ? bytes(job.bytes_done)
        : "";
  return (
    <div
      className="archive-job"
      data-job={job.kind}
      data-state={job.state}
      data-phase={job.phase}
      role="status"
    >
      <div className="archive-job-head">
        {job.state === "running" ? (
          <Icon name="loader-circle" size="sm" className="spin" />
        ) : job.state === "done" ? (
          <Icon name="circle-check" size="sm" />
        ) : (
          <Icon name="circle-x" size="sm" />
        )}
        <strong>
          {kindLabel[job.kind]}:{" "}
          {job.state === "running"
            ? phaseLabel(job.phase)
            : job.state === "done"
              ? "done"
              : job.state === "cancelled"
                ? "cancelled"
                : "failed"}
        </strong>
        <span className="grow" />
        {job.state === "running" && onCancel && (
          <button
            type="button"
            className="small subtle"
            disabled={!job.cancellable}
            title={
              job.cancellable ? undefined : "The folder's removal has started; it cannot be cancelled"
            }
            onClick={onCancel}
          >
            Cancel
          </button>
        )}
      </div>
      {job.state === "running" && (
        <progress
          aria-label={`${kindLabel[job.kind]} progress`}
          max={1}
          value={fraction ?? undefined}
        />
      )}
      {(job.message || amount || job.files_done > 0) && job.state === "running" && (
        <p className="archive-job-detail">
          {[job.message, amount, job.files_done ? `${job.files_done.toLocaleString("en-US")} files` : ""]
            .filter(Boolean)
            .join(" · ")}
        </p>
      )}
      {job.error && job.state !== "running" && (
        <p className="notice error" role="alert">
          {job.error}
        </p>
      )}
    </div>
  );
}

/** The safe-removal check, as a list: the checkouts and what they hold
 * that is on no remote, the other files, then every item. */
function CheckReport({ check }: { check: RemovalCheck }) {
  const archive = check.mode === "archive";
  const items = sortedItems(check.items);
  return (
    <div className="removal-check" data-allowed={check.allowed}>
      <p className={`removal-summary ${check.blockers ? "blocked" : ""}`}>
        <Icon name={check.blockers ? "circle-x" : check.warnings ? "circle-alert" : "circle-check"} size="sm" />
        {checkSummary(check)}
      </p>
      {archive && check.checkouts.some((c) => checkoutProblems(c).length) && (
        <p className="notice removal-archive-note">
          Commits, stashes and changes listed below are on no remote: after
          archiving they exist only in the archive (it contains each
          checkout's <code>.git</code>). Restoring it brings them back.
        </p>
      )}
      {check.checkouts.length > 0 && (
        <ul className="removal-checkouts" aria-label="Checkouts">
          {check.checkouts.map((checkout) => {
            const problems = checkoutProblems(checkout);
            return (
              <li key={checkout.path} data-checkout={checkout.name}>
                <Icon name="git-branch" size="sm" className="faint" />
                <div className="removal-checkout">
                  <strong>{checkout.name}</strong>
                  {checkout.branch && <span className="faint mono"> {checkout.branch}</span>}
                  {problems.length ? (
                    <ul className="removal-problems">
                      {problems.map((problem) => (
                        <li key={problem}>{problem}</li>
                      ))}
                    </ul>
                  ) : (
                    <span className="removal-ok"> · everything on a remote</span>
                  )}
                  {checkout.ignored > 0 && (
                    <small className="faint">
                      {checkout.ignored.toLocaleString("en-US")} ignored{" "}
                      {checkout.ignored === 1 ? "path" : "paths"} (build output,
                      .env…) {archive ? "go into the archive" : "are lost"}
                    </small>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
      )}
      {check.exists && (
        <p className="removal-other" data-other-files={check.other_files.files}>
          {check.other_files.files
            ? `${check.other_files.files.toLocaleString("en-US")}${check.other_files.truncated ? "+" : ""} other ${check.other_files.files === 1 ? "file" : "files"} (${bytes(check.other_files.bytes)}) outside the checkouts: ${archive ? "they go into the archive" : "deleting loses them"}.`
            : "No other files outside the checkouts."}
        </p>
      )}
      {items.length > 0 && (
        <ul className="removal-items" aria-label="Findings">
          {items.map((item, index) => (
            <li key={index} data-severity={item.severity} data-kind={item.kind}>
              <span className={`removal-badge ${item.severity}`}>{item.severity}</span>
              <span>
                {item.message}
                {item.path && <span className="faint mono"> {item.path}</span>}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** Archive… or Delete… of a project: the check, the confirmation, then
 * the job's progress. */
export function RemovalDialog({
  mode,
  project,
  rpc,
  jobs,
  onRemoveOnly,
  onClose,
}: {
  mode: "archive" | "delete";
  project: Project;
  rpc: Rpc;
  jobs: ArchiveJobs;
  /** Removes the project from KelpMux only (`project_delete`). */
  onRemoveOnly?: () => Promise<boolean>;
  onClose: () => void;
}) {
  const [check, setCheck] = useState<RemovalCheck | null>(null);
  const [checking, setChecking] = useState(true);
  const [error, setError] = useState("");
  const [status, setStatus] = useState<ArchiveStatus | null>(null);
  const [typed, setTyped] = useState("");
  const [choice, setChoice] = useState<"folder" | "config">("folder");
  const [remove, setRemove] = useState(true);
  const [started, setStarted] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const archive = mode === "archive";
  const running = jobOf(jobs.jobs, project.id);
  const job = started ? jobs.jobs.find((j) => j.id === started) : running?.state === "running" ? running : undefined;
  const runCheck = useCallback(async () => {
    setChecking(true);
    setError("");
    try {
      const result = readCheck(await rpc("project_check_removal", { id: project.id, mode }));
      if (!result) throw Error("The bridge's check is not readable");
      setCheck(result);
    } catch (e) {
      setError(message(e));
    } finally {
      setChecking(false);
    }
  }, [rpc, project.id, mode]);
  useEffect(() => {
    void runCheck();
    if (archive)
      rpc("archive_status")
        .then((value) => setStatus(readArchiveStatus(value)))
        .catch((e) => setError(message(e)));
  }, [runCheck, archive, rpc]);
  async function start() {
    if (!check || busy) return;
    setBusy(true);
    setError("");
    try {
      if (!archive && choice === "config") {
        if (await onRemoveOnly?.()) onClose();
        return;
      }
      const result = archive
        ? await rpc("project_archive", { id: project.id, fingerprint: check.fingerprint })
        : await rpc("project_delete_folder", {
            id: project.id,
            fingerprint: check.fingerprint,
            name: typed,
            remove,
          });
      const next = readJobs(result);
      jobs.update(next);
      const mine = jobOf(next, project.id);
      if (mine) setStarted(mine.id);
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  }
  async function cancel() {
    try {
      jobs.update(readJobs(await rpc("archive_cancel", { project: project.id })));
    } catch (e) {
      setError(message(e));
    }
  }
  const unconfigured = archive && status !== null && !status.configured;
  const nameOk = typed === project.name;
  const folderChosen = archive || choice === "folder";
  const canStart =
    !!check &&
    !checking &&
    !busy &&
    !job &&
    (folderChosen ? check.allowed && check.exists : true) &&
    (archive ? !unconfigured : choice === "config" || nameOk);
  const title = archive ? `Archive “${project.name}”` : `Delete “${project.name}”`;
  const close = () => {
    if (!busy) onClose();
  };
  return (
    <div
      className="modal-shade confirmation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && (!job || job.state !== "running")) close();
      }}
    >
      <section
        className="confirm-dialog removal-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="removal-title"
        data-removal={mode}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.stopPropagation();
            close();
          }
        }}
      >
        <h2 id="removal-title">{title}</h2>
        <p>
          {archive ? (
            <>
              Packs <span className="mono">{project.folder}</span> as a
              tar.gz, uploads it
              {status?.bucket ? (
                <>
                  {" "}
                  to <span className="mono">gs://{status.bucket}/{status.prefix}/{project.id}/</span>
                </>
              ) : null}
              , verifies it, then removes the folder from this machine. The
              project stays in KelpMux, archived, and can be restored.
            </>
          ) : (
            <>
              Deletes <span className="mono">{project.folder}</span> from this
              machine, without an archive.
            </>
          )}
        </p>
        {unconfigured && (
          <p className="notice error" role="alert">
            No archive storage is configured: set <code>[archive]</code> in
            config.toml (Settings → Archive).
          </p>
        )}
        {job ? (
          <JobProgress job={job} onCancel={() => void cancel()} />
        ) : checking && !check ? (
          <p className="faint removal-checking">
            <Icon name="loader-circle" size="sm" className="spin" /> Checking what
            is not on a remote…
          </p>
        ) : check ? (
          <CheckReport check={check} />
        ) : null}
        {error && (
          <p className="notice error" role="alert">
            {error}
          </p>
        )}
        {!archive && check && !job && (
          <fieldset className="removal-choice">
            <legend>What to delete</legend>
            <label>
              <input
                type="radio"
                name="removal-choice"
                checked={choice === "folder"}
                onChange={() => setChoice("folder")}
              />
              Delete the folder
            </label>
            <label className="removal-sub">
              <input
                type="checkbox"
                checked={remove}
                disabled={choice !== "folder"}
                onChange={(event) => setRemove(event.target.checked)}
              />
              Also remove the project from KelpMux
            </label>
            {onRemoveOnly && (
              <label>
                <input
                  type="radio"
                  name="removal-choice"
                  checked={choice === "config"}
                  onChange={() => setChoice("config")}
                />
                Remove from KelpMux only (keep the folder)
              </label>
            )}
            {choice === "folder" && (
              <label className="project-field removal-confirm">
                <span>
                  Type <strong>{project.name}</strong> to confirm
                </span>
                <input
                  aria-label="Project name to confirm"
                  value={typed}
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                  autoComplete="off"
                  onChange={(event) => setTyped(event.target.value)}
                />
              </label>
            )}
          </fieldset>
        )}
        <div className="confirm-buttons">
          {job && job.state !== "running" ? (
            <button type="button" onClick={onClose}>
              Close
            </button>
          ) : (
            <>
              <button
                type="button"
                className="subtle"
                disabled={busy || checking}
                onClick={() => void runCheck()}
                hidden={!!job}
              >
                Check again
              </button>
              <button type="button" className="subtle" disabled={busy} onClick={onClose}>
                {job ? "Hide" : "Cancel"}
              </button>
              {!job && (
                <button
                  type="button"
                  className={archive ? "" : "danger"}
                  disabled={!canStart}
                  onClick={() => void start()}
                >
                  {archive
                    ? "Archive and remove the folder"
                    : choice === "config"
                      ? "Remove from KelpMux"
                      : "Delete the folder"}
                </button>
              )}
            </>
          )}
        </div>
      </section>
    </div>
  );
}

/** Restore… of an archived project: one of its archives, into its folder
 * or (when that exists) a new one under projects.base. */
function RestoreDialog({
  project,
  rpc,
  jobs,
  base,
  taken,
  onClose,
}: {
  project: Project;
  rpc: Rpc;
  jobs: ArchiveJobs;
  base: string;
  taken: string[];
  onClose: () => void;
}) {
  const [archives, setArchives] = useState<ArchiveListing[] | null>(null);
  const [object, setObject] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [started, setStarted] = useState("");
  const elsewhere = !project.missing;
  const [folder, setFolder] = useState(() => restoreFolder(base, project.name, taken));
  useEffect(() => {
    rpc("archive_list", { id: project.id })
      .then((value) => {
        const list = readArchives(value);
        setArchives(list);
        setObject(defaultArchive(list)?.object || "");
      })
      .catch((e) => setError(message(e)));
  }, [rpc, project.id]);
  const job = started ? jobs.jobs.find((j) => j.id === started) : undefined;
  async function restore() {
    setBusy(true);
    setError("");
    try {
      const next = readJobs(
        await rpc("project_restore", {
          id: project.id,
          object,
          ...(elsewhere ? { folder: folder.trim() } : {}),
        }),
      );
      jobs.update(next);
      const mine = jobOf(next, project.id);
      if (mine) setStarted(mine.id);
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="modal-shade confirmation">
      <section
        className="confirm-dialog removal-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="restore-title"
        data-restore={project.id}
        onKeyDown={(event) => {
          if (event.key === "Escape" && !busy) {
            event.stopPropagation();
            onClose();
          }
        }}
      >
        <h2 id="restore-title">Restore “{project.name}”</h2>
        {job ? (
          <JobProgress
            job={job}
            onCancel={() =>
              void rpc("archive_cancel", { project: project.id })
                .then((value) => jobs.update(readJobs(value)))
                .catch((e) => setError(message(e)))
            }
          />
        ) : !archives ? (
          !error && (
            <p className="faint">
              <Icon name="loader-circle" size="sm" className="spin" /> Listing
              the archives…
            </p>
          )
        ) : archives.length ? (
          <fieldset className="restore-archives">
            <legend>Archive</legend>
            {archives.map((archive) => (
              <label key={archive.object} data-archive={archive.object}>
                <input
                  type="radio"
                  name="restore-archive"
                  checked={object === archive.object}
                  onChange={() => setObject(archive.object)}
                />
                <span className="mono">{archive.object.split("/").pop()}</span>
                <span className="faint">
                  {[archive.size !== null ? bytes(archive.size) : "", archive.recorded ? "recorded" : ""]
                    .filter(Boolean)
                    .join(" · ")}
                </span>
              </label>
            ))}
          </fieldset>
        ) : (
          <p className="notice">No archive of this project was found.</p>
        )}
        {!job && archives && archives.length > 0 && (
          elsewhere ? (
            <label className="project-field">
              <span>
                {project.folder} exists again: restore into a new folder
              </span>
              <input
                aria-label="Restore into folder"
                value={folder}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                onChange={(event) => setFolder(event.target.value)}
              />
            </label>
          ) : (
            <p className="setting-hint">
              Into <span className="mono">{project.folder}</span>; the project
              becomes active again.
            </p>
          )
        )}
        {error && (
          <p className="notice error" role="alert">
            {error}
          </p>
        )}
        <div className="confirm-buttons">
          {job && job.state !== "running" ? (
            <button type="button" onClick={onClose}>
              Close
            </button>
          ) : (
            <>
              <button type="button" className="subtle" disabled={busy} onClick={onClose}>
                {job ? "Hide" : "Cancel"}
              </button>
              {!job && (
                <button
                  type="button"
                  disabled={busy || !object || (elsewhere && !folder.trim())}
                  onClick={() => void restore()}
                >
                  Restore
                </button>
              )}
            </>
          )}
        </div>
      </section>
    </div>
  );
}

/** An archived project's home page body: when and where it was archived,
 * a job of it that runs, and Restore…. */
export function ArchivedPanel({
  project,
  rpc,
  jobs,
  base,
  taken,
}: {
  project: Project;
  rpc: Rpc | null;
  jobs: ArchiveJobs;
  /** `projects.base`, as written. */
  base: string;
  /** The other projects' folders, as written. */
  taken: string[];
}) {
  const [restoring, setRestoring] = useState(false);
  const record = project.archive;
  const job = jobOf(jobs.jobs, project.id);
  return (
    <section className="home-card archived-panel" data-archived={project.id} aria-labelledby="archived-title">
      <header className="home-card-head">
        <Icon name="archive" size="sm" className="faint" />
        <h2 id="archived-title">Archived</h2>
      </header>
      <div className="archived-body">
        <p>
          The folder is no longer on this machine
          {record?.archived_at ? <> since {archiveDate(record.archived_at)}</> : null}.
          Restore it to work on {project.name} again.
        </p>
        {record && (
          <dl className="github-facts-list archived-facts">
            <dt>Archive</dt>
            <dd className="mono">gs://{record.bucket}/{record.object}</dd>
            {record.size !== null && (
              <>
                <dt>Size</dt>
                <dd>{bytes(record.size)}</dd>
              </>
            )}
            {record.sha256 && (
              <>
                <dt>SHA-256</dt>
                <dd className="mono archived-hash">{record.sha256}</dd>
              </>
            )}
          </dl>
        )}
        {job?.state === "running" && <JobProgress job={job} />}
        {rpc && (
          <button
            type="button"
            className="small"
            disabled={job?.state === "running"}
            onClick={() => setRestoring(true)}
          >
            <Icon name="rotate-ccw" size="sm" />
            Restore…
          </button>
        )}
      </div>
      {restoring && rpc && (
        <RestoreDialog
          project={project}
          rpc={rpc}
          jobs={jobs}
          base={base}
          taken={taken}
          onClose={() => setRestoring(false)}
        />
      )}
    </section>
  );
}

const CONFIG_EXAMPLE = `[archive]
backend = "gcs"
bucket = "my-kelpmux-archives"
# prefix = "kelpmux"
# Default: the machine's own Google identity (metadata server).
# token_command = "~/bin/gcs-token"   # prints an access token`;

/** Settings → Archive: the archive store `[archive]` names, read-only,
 * and Test. */
export function ArchiveSettings({ rpc }: { rpc: Rpc | null }) {
  const [status, setStatus] = useState<ArchiveStatus | null>(null);
  const [error, setError] = useState("");
  const [testing, setTesting] = useState(false);
  const [tested, setTested] = useState<{ ok: boolean; text: string } | null>(null);
  useEffect(() => {
    if (!rpc) return;
    rpc("archive_status")
      .then((value) => setStatus(readArchiveStatus(value)))
      .catch((e) => setError(message(e)));
  }, [rpc]);
  async function test() {
    setTesting(true);
    setTested(null);
    try {
      const result = await rpc!("archive_test");
      setTested({
        ok: true,
        text: `Wrote and deleted ${result?.object || "a test object"}${
          Number.isFinite(result?.elapsed_ms) ? ` in ${result.elapsed_ms} ms` : ""
        }.`,
      });
    } catch (e) {
      setTested({ ok: false, text: message(e) });
    } finally {
      setTesting(false);
    }
  }
  const head = (
    <>
      <h3>Archive</h3>
      <p className="setting-hint">
        Archiving a project packs its folder, uploads it to the archive
        storage, verifies it and removes the folder from this machine.
        The storage is set in <code>[archive]</code> of config.toml, edited
        there only (see docs/config.md); the server's credentials never reach
        this browser.
      </p>
    </>
  );
  if (!rpc)
    return (
      <>
        {head}
        <p className="setting-hint">
          This kelpmux web bridge does not offer archives. Install a matching
          kelpmux on the server.
        </p>
      </>
    );
  if (!status)
    return (
      <>
        {head}
        {error ? (
          <p className="notice error" role="alert">
            {error}
          </p>
        ) : (
          <p className="setting-hint">Loading…</p>
        )}
      </>
    );
  return (
    <>
      {head}
      {status.error && (
        <p className="notice error" role="alert">
          {status.error}
        </p>
      )}
      {!status.configured ? (
        <div className="notice github-setup archive-setup">
          <p>
            <strong>Not configured.</strong> Name the storage in config.toml,
            then check it with <code>kelpmux config check</code>:
          </p>
          <pre>{CONFIG_EXAMPLE}</pre>
        </div>
      ) : (
        <>
          <dl className="github-facts-list archive-facts">
            <dt>Backend</dt>
            <dd>{status.backend === "gcs" ? "Google Cloud Storage" : status.backend}</dd>
            <dt>Bucket</dt>
            <dd className="mono">{status.bucket}</dd>
            <dt>Prefix</dt>
            <dd className="mono">{status.prefix}/</dd>
            <dt>Credentials</dt>
            <dd data-token-source={status.token_source}>
              {status.token_source === "command"
                ? `Command: ${status.token_label}`
                : status.token_label || "This machine's Google identity (metadata server)"}
            </dd>
          </dl>
          <div className="archive-test">
            <button type="button" className="small subtle" disabled={testing} onClick={() => void test()}>
              {testing ? "Testing…" : "Test"}
            </button>
            <span className="setting-hint">
              Writes and deletes a tiny object under {status.prefix}/.kelpmux-test/.
            </span>
          </div>
          {tested && (
            <p
              className={`notice ${tested.ok ? "" : "error"}`}
              role={tested.ok ? "status" : "alert"}
              data-archive-test={tested.ok ? "ok" : "failed"}
            >
              {tested.text}
            </p>
          )}
        </>
      )}
    </>
  );
}
