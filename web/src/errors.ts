// Optional error reporting to GlitchTip (Sentry-compatible), off unless the
// relay's /connection.json carries `errors.dsn`. Only then is the Sentry
// browser SDK loaded (a separate chunk, ./error-reporting.ts). This module
// holds the SDK-independent parts, unit-tested in test/errors.test.mjs.
// Privacy rules (docs/web.md, "Error reporting"): errors only, no
// breadcrumbs, user, cookies, request bodies or query strings, and never
// terminal content.

export type ErrorReporting = { dsn: string; environment?: string };

/** The `errors` member of /connection.json, or null when reporting is off
 * or the value is malformed. */
export function reportingConfig(value: unknown): ErrorReporting | null {
  if (!value || typeof value !== "object") return null;
  const { dsn, environment } = value as Record<string, unknown>;
  if (typeof dsn !== "string" || !dsn.startsWith("https://")) return null;
  if (environment !== undefined && typeof environment !== "string") return null;
  return environment === undefined ? { dsn } : { dsn, environment };
}

/** A URL without its query string and fragment. */
export function stripQuery(url: string): string {
  const end = url.search(/[?#]/);
  return end < 0 ? url : url.slice(0, end);
}

type Frame = { filename?: string; abs_path?: string };
type Exception = { stacktrace?: { frames?: Frame[] } };
/** The parts of a Sentry event the scrubber touches. */
export type ScrubbableEvent = {
  request?: {
    url?: string;
    data?: unknown;
    query_string?: unknown;
    cookies?: unknown;
    headers?: unknown;
    env?: unknown;
  };
  user?: unknown;
  breadcrumbs?: unknown;
  extra?: unknown;
  exception?: { values?: Exception[] };
  stacktrace?: { frames?: Frame[] };
  threads?: { values?: Exception[] };
};

function scrubFrames(frames: Frame[] | undefined) {
  for (const frame of frames || []) {
    if (frame.filename) frame.filename = stripQuery(frame.filename);
    if (frame.abs_path) frame.abs_path = stripQuery(frame.abs_path);
  }
}

/** `beforeSend`: drops whatever could identify the user or carry page
 * content, whatever the SDK or an integration added. */
export function scrubEvent<T extends ScrubbableEvent>(event: T): T {
  const e: ScrubbableEvent = event;
  delete e.user;
  delete e.breadcrumbs;
  delete e.extra;
  if (e.request) {
    const url = e.request.url;
    e.request = url ? { url: stripQuery(url) } : {};
  }
  for (const value of e.exception?.values || [])
    scrubFrames(value.stacktrace?.frames);
  for (const value of e.threads?.values || [])
    scrubFrames(value.stacktrace?.frames);
  scrubFrames(e.stacktrace?.frames);
  return event;
}

let started = false;
/** Starts reporting once per page when the relay configured a DSN. Loading
 * or initialising the SDK never affects the application. */
export function startErrorReporting(connection: { errors?: unknown }) {
  const settings = reportingConfig(connection.errors);
  if (!settings || started) return;
  started = true;
  import("./error-reporting")
    .then(({ start }) => start(settings))
    .catch(() => {});
}
