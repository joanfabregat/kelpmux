// A project's environments (bridge `environments`, API minor 8; ADR 0005):
// dev servers (`process`) and deployments (`deploy`) declared in the
// project's repositories under `.kelp/environments/<name>/`. Parsing and
// presentation only, free of React so the unit tests can import it.
import type { DisplayState } from "./state/model";

export type EnvironmentState =
  | "starting"
  | "running"
  | "stopping"
  | "stopped"
  | "failed"
  | "invalid"
  // A deploy environment not checked since its last action: listing
  // never runs its `status` script.
  | "unknown";

export type Environment = {
  name: string;
  kind: "process" | "deploy" | null;
  description: string | null;
  /** The repository (or project folder) declaring it. */
  root: string;
  repo: string | null;
  slug: string;
  /** Its stable port (`KELP_PORT`), once it has started once. */
  port: number | null;
  /** `kelpmux env logs` has something to show. */
  logs: boolean;
  state: EnvironmentState;
  detail: string | null;
  /** Where it is reached: http(s) only. */
  url: string | null;
  /** The pane running it, or its latest `up`/`down`, while open. */
  pane: number | null;
  action: string | null;
  /** A deploy environment's last explicit status check, with its time
   * (epoch seconds). */
  checked: EnvironmentCheck | null;
  /** Why it cannot run (an invalid manifest). */
  error: string | null;
};

/** What a deploy environment's `status` script said when last asked. */
export type EnvironmentCheck = {
  state: "running" | "stopped" | "failed";
  url: string | null;
  detail: string | null;
  at: number;
};

export type ProjectEnvironments = {
  project: string;
  folder: string;
  exists: boolean;
  environments: Environment[];
  truncated: boolean;
};

/** An environment pane: the sidebar shows it apart from terminals. */
export type EnvironmentPane = { project: string; env: string; action: string };

const states: EnvironmentState[] = [
  "starting",
  "running",
  "stopping",
  "stopped",
  "failed",
  "invalid",
  "unknown",
];

const text = (value: unknown, limit = 4096): string | null =>
  typeof value === "string" && value.length <= limit ? value : null;
const count = (value: unknown): number | null =>
  typeof value === "number" && Number.isInteger(value) && value > 0 ? value : null;

/** A URL the page may link to: http(s) only. */
export function safeUrl(value: unknown): string | null {
  const url = text(value, 2048);
  if (!url) return null;
  try {
    const parsed = new URL(url);
    return parsed.protocol === "http:" || parsed.protocol === "https:"
      ? url
      : null;
  } catch {
    return null;
  }
}

function readCheck(value: any): EnvironmentCheck | null {
  const at = count(value?.at);
  if (!at) return null;
  const state =
    value?.state === "running" || value?.state === "stopped" ? value.state : "failed";
  return {
    state,
    url: safeUrl(value?.url),
    detail: text(value?.detail, 1000),
    at,
  };
}

function readEnvironment(value: any): Environment | null {
  const name = text(value?.name, 64);
  if (!name) return null;
  const state = states.includes(value?.state) ? value.state : "failed";
  const kind =
    value?.kind === "process" || value?.kind === "deploy" ? value.kind : null;
  return {
    name,
    kind,
    description: text(value?.description, 400),
    root: text(value?.root) || "",
    repo: text(value?.repo, 256),
    slug: text(value?.slug, 64) || "",
    port: count(value?.port),
    logs: value?.logs === true,
    state,
    detail: text(value?.detail, 1000),
    url: safeUrl(value?.url),
    pane: count(value?.pane),
    action: text(value?.action, 16),
    checked: readCheck(value?.checked),
    error: text(value?.error, 2000),
  };
}

/** The bridge's `environments` reply, or null when it is not one. */
export function readEnvironments(value: any): ProjectEnvironments | null {
  if (!value || typeof value !== "object" || !Array.isArray(value.environments))
    return null;
  return {
    project: text(value.project, 128) || "",
    folder: text(value.folder) || "",
    exists: value.exists !== false,
    environments: value.environments
      .map(readEnvironment)
      .filter((env: Environment | null): env is Environment => !!env),
    truncated: value.truncated === true,
  };
}

/** The bridge's `environment_panes` reply: pane id → environment. */
export function readEnvironmentPanes(value: any): Record<number, EnvironmentPane> {
  const panes: Record<number, EnvironmentPane> = {};
  const source = value?.panes;
  if (!source || typeof source !== "object") return panes;
  for (const [key, entry] of Object.entries<any>(source)) {
    const pane = Number(key);
    const project = text(entry?.project, 128);
    const env = text(entry?.env, 64);
    if (Number.isInteger(pane) && pane > 0 && project && env)
      panes[pane] = { project, env, action: text(entry?.action, 16) || "" };
  }
  return panes;
}

/** The terminal state mark an environment state is drawn with: Starting
 * and Stopping spin, Running is the green check, Stopped the grey ring,
 * Failed the red cross and an invalid manifest the amber mark. */
export function environmentMark(state: EnvironmentState): DisplayState {
  switch (state) {
    case "starting":
    case "stopping":
      return "working";
    case "running":
      return "finished";
    case "failed":
      return "failed";
    case "invalid":
      return "input_needed";
    default:
      return "idle";
  }
}

export const environmentStateLabel: Record<EnvironmentState, string> = {
  starting: "Starting",
  running: "Running",
  stopping: "Stopping",
  stopped: "Stopped",
  failed: "Failed",
  invalid: "Invalid",
  unknown: "Not checked",
};

/** Which of Start, Stop and Check status an environment offers now.
 * Check status (a deploy environment's `status` script, the only way it
 * runs) is a click, never polling. */
export function environmentActions(env: Environment): {
  start: boolean;
  stop: boolean;
  check: boolean;
} {
  if (env.state === "invalid") return { start: false, stop: false, check: false };
  const busy = env.state === "starting" || env.state === "stopping";
  if (env.kind === "deploy")
    // `up` again redeploys; `down` is offered unless one is running.
    return { start: !busy, stop: !busy, check: !busy };
  return {
    start: env.state === "stopped" || env.state === "failed",
    stop: env.state === "starting" || env.state === "running" || env.pane !== null,
    check: false,
  };
}

/** A request identity for `environment_start`/`environment_stop`
 * (letters, digits, `-` and `_`). */
export function environmentRequest(action: string, name: string): string {
  const random = Math.random().toString(36).slice(2, 10);
  return `web-env-${action}-${name}-${Date.now().toString(36)}-${random}`.slice(0, 128);
}
