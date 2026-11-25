// A project's environments (bridge `environments`, API minor 8; ADR 0005):
// - useProjectEnvironments: the list of the project shown, asked when it
//   shows and every five seconds while it does (never while the page is
//   hidden), with Start, Stop and Check status. Listing never runs an
//   environment's scripts: only Check status runs a deploy environment's
//   `status` script.
// - EnvironmentsSection: the list itself, reusable (the Environments view
//   and, later, the project's home page): each environment's name, kind,
//   state mark, URL, port, Start/Stop and its terminal.
// - EnvironmentsView: the section under the project's header, for
//   `?view=envs&project=<id>`.
// Nothing here starts or checks an environment on its own: Start, Stop and
// Check status are the user's clicks.
import React, { useEffect, useRef, useState } from "react";
import { Icon } from "./status";
import { Tooltip } from "./Tooltip";
import { StateMark } from "./state/StateMark";
import {
  environmentActions,
  environmentMark,
  environmentRequest,
  environmentStateLabel,
  readEnvironments,
  type Environment,
  type ProjectEnvironments,
} from "./environments";
import { projectColor, type Project } from "./projects";
import { relativeTime } from "./git";
import "./styles/environments.css";

/** An action under way: Start, Stop or Check status. */
type Pending = "start" | "stop" | "check";

/** How often the shown project's environments are asked again. */
export const ENVIRONMENTS_REFRESH_MS = 5_000;

const pageVisible = () =>
  typeof document === "undefined" || document.visibilityState !== "hidden";

export function useProjectEnvironments(options: {
  /** The bridge offers `environments`. */
  enabled: boolean;
  /** The project whose environments show ("" for none). */
  project: string;
  /** A bridge request that rejects with the bridge's error. */
  query: (method: string, params: any) => Promise<any>;
  /** Called after Start opened a pane (to show its terminal, say). */
  onStarted?: (pane: number) => void;
}) {
  const { enabled, project } = options;
  const [byProject, setByProject] = useState<Record<string, ProjectEnvironments>>({});
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  // A refused Start or Stop stays shown until the next one.
  const [actionError, setActionError] = useState("");
  const [pending, setPending] = useState<Record<string, Pending>>({});
  const latest = useRef(options);
  latest.current = options;
  const inFlight = useRef(false);

  async function load(id: string) {
    if (inFlight.current) return;
    inFlight.current = true;
    setLoading(true);
    try {
      const list = readEnvironments(
        await latest.current.query("environments", { project: id }),
      );
      if (list) setByProject((all) => ({ ...all, [id]: list }));
      setError("");
    } catch (failure: any) {
      setError(failure?.message || String(failure));
    } finally {
      inFlight.current = false;
      setLoading(false);
    }
  }

  useEffect(() => {
    setActionError("");
    if (!enabled || !project) return;
    void load(project);
    const timer = setInterval(() => {
      if (pageVisible()) void load(project);
    }, ENVIRONMENTS_REFRESH_MS);
    return () => clearInterval(timer);
  }, [enabled, project]);

  async function act(action: Pending, name: string) {
    const id = latest.current.project;
    if (!id || pending[name]) return;
    setPending((all) => ({ ...all, [name]: action }));
    try {
      const result = await latest.current.query(
        `environment_${action}`,
        action === "check"
          ? { project: id, name }
          : { project: id, name, request: environmentRequest(action, name) },
      );      setActionError("");
      if (action === "start" && typeof result?.pane === "number")
        latest.current.onStarted?.(result.pane);
    } catch (failure: any) {
      setActionError(failure?.message || String(failure));
    } finally {
      setPending(({ [name]: _, ...rest }) => rest);
      void load(id);
    }
  }

  return {
    environments: project ? byProject[project] : undefined,
    loading,
    error: actionError || error,
    pending,
    refresh: () => project && void load(project),
    start: (name: string) => void act("start", name),
    stop: (name: string) => void act("stop", name),
    check: (name: string) => void act("check", name),
  };
}

function EnvironmentRow({
  env,
  pending,
  onStart,
  onStop,
  onCheck,
  onOpenPane,
}: {
  env: Environment;
  pending?: Pending;
  onStart: () => void;
  onStop: () => void;
  onCheck: () => void;
  onOpenPane?: (pane: number) => void;
}) {
  const actions = environmentActions(env);
  // Not checked since its last action: the last check, with its time.
  const last = env.state === "unknown" ? env.checked : null;
  const shown = last ? last.state : env.state;
  const label = last
    ? `${environmentStateLabel[last.state]} · checked ${relativeTime(last.at)}`
    : environmentStateLabel[env.state];
  const startLabel = env.kind === "deploy" ? "Deploy" : "Start";
  const stopLabel = env.kind === "deploy" ? "Take down" : "Stop";
  return (
    <li
      className={`env-row env-${env.state}`}
      data-env={env.name}
      data-state={env.state}
    >
      <span className="env-mark">
        <StateMark
          state={environmentMark(shown)}
          label={`${env.name}: ${label}`}
          size="md"
        />
      </span>
      <span className="env-names">
        <span className="env-title">
          <b className="env-name">{env.name}</b>
          {env.kind && <span className="env-tag">{env.kind}</span>}
          <span className={`env-state env-state-${shown} ${last ? "env-state-last" : ""}`}>
            {pending === "start"
              ? "Starting…"
              : pending === "stop"
                ? "Stopping…"
                : pending === "check"
                  ? "Checking…"
                : label}
          </span>
          {env.repo && (
            <span className="env-repo faint" title={env.root}>
              {env.repo}
            </span>
          )}
        </span>
        <span className="env-detail">
          {env.description && <span>{env.description}</span>}
          {env.url && (
            <a
              className="env-url"
              href={env.url}
              target="_blank"
              rel="noopener noreferrer"
            >
              {env.url}
              <Icon name="external-link" size="sm" />
            </a>
          )}
          {env.port !== null && (
            <span className="env-port" title="KELP_PORT, stable for this environment">
              port {env.port}
            </span>
          )}
        </span>
        {(env.error || env.detail || last?.detail) && (
          <span className={`env-message ${env.error || shown === "failed" ? "error" : "faint"}`}>
            {env.error || env.detail || last?.detail}
          </span>
        )}
      </span>
      <span className="env-actions">
        {env.pane !== null && onOpenPane && (
          <Tooltip content={`Open its terminal${env.action ? ` (${env.action})` : ""}`}>
            <button
              type="button"
              className="icon-btn"
              aria-label={`Open the terminal of ${env.name}`}
              onClick={() => onOpenPane(env.pane!)}
            >
              <Icon name="terminal" size="sm" />
            </button>
          </Tooltip>
        )}
        {actions.check && (
          <Tooltip content="Run its status script now (never automatically)">
            <button
              type="button"
              className="small subtle env-check"
              disabled={!!pending}
              aria-label={`Check the status of ${env.name}`}
              onClick={onCheck}
            >
              <Icon name="refresh-cw" size="sm" />
              Check status
            </button>
          </Tooltip>
        )}
        {actions.start && (
          <button
            type="button"
            className="small env-start"
            disabled={!!pending}
            aria-label={`${startLabel} ${env.name}`}
            onClick={onStart}
          >
            <Icon name="play" size="sm" />
            {startLabel}
          </button>
        )}
        {actions.stop && (
          <button
            type="button"
            className="small subtle env-stop"
            disabled={!!pending}
            aria-label={`${stopLabel} ${env.name}`}
            onClick={onStop}
          >
            <Icon name="square" size="sm" />
            {stopLabel}
          </button>
        )}
      </span>
    </li>
  );
}

/** A project's environments, reusable wherever a project shows them. */
export function EnvironmentsSection({
  project,
  environments,
  error,
  pending = {},
  onStart,
  onStop,
  onCheck,
  onOpenPane,
}: {
  project: Project;
  environments?: ProjectEnvironments;
  error?: string;
  pending?: Record<string, Pending>;
  onStart: (name: string) => void;
  onStop: (name: string) => void;
  /** Runs a deploy environment's `status` script (explicitly). */
  onCheck: (name: string) => void;
  /** Shows an environment's terminal. */
  onOpenPane?: (pane: number) => void;
}) {
  const list = environments?.environments || [];
  return (
    <div className="envs-section" aria-label={`Environments of ${project.name}`}>
      {error && (
        <div className="notice error compact" role="alert">
          {error}
        </div>
      )}
      {!environments ? (
        !error && (
          <div className="empty-state">
            <Icon name="loader-circle" size="lg" className="spin" />
            <p>Reading the environments…</p>
          </div>
        )
      ) : !list.length ? (
        <div className="empty-state">
          <Icon name="layers" size="lg" />
          <h3>No environments</h3>
          <p>
            {environments.exists ? (
              <>
                Declare one in the project folder or one of its repositories:{" "}
                <code>.kelp/environments/&lt;name&gt;/environment.toml</code>.
              </>
            ) : (
              `The folder ${project.path} is missing.`
            )}
          </p>
        </div>
      ) : (
        <>
          <ul className="env-list" aria-label="Environments">
            {list.map((env) => (
              <EnvironmentRow
                key={`${env.root}/${env.name}`}
                env={env}
                pending={pending[env.name]}
                onStart={() => onStart(env.name)}
                onStop={() => onStop(env.name)}
                onCheck={() => onCheck(env.name)}
                onOpenPane={onOpenPane}
              />
            ))}
          </ul>
          {environments.truncated && (
            <p className="faint">Only the first {list.length} environments are shown.</p>
          )}
        </>
      )}
    </div>
  );
}

/** `?view=envs&project=<id>`: the project's header and its environments. */
export function EnvironmentsView({
  project,
  loading,
  onRefresh,
  onBack,
  ...section
}: React.ComponentProps<typeof EnvironmentsSection> & {
  loading: boolean;
  onRefresh: () => void;
  /** A phone's way back to its terminal list. */
  onBack?: () => void;
}) {
  const color = projectColor(project);
  return (
    <section className="envs-view" aria-label={`Environments of ${project.name}`}>
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
          <b>{project.name}</b> · Environments
        </span>
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
      <div className="envs-body">
        <EnvironmentsSection project={project} {...section} />
        <p className="envs-note faint">
          Started and stopped only from here or with <code>kelpmux env</code>.
          A dev server runs in a terminal of this project; Stop sends it
          SIGTERM and closes that terminal. A deployment's status script runs
          only on Check status.
        </p>
      </div>
    </section>
  );
}
