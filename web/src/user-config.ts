// The user configuration (~/.config/kelpmux/config.toml) as the web bridge
// reports it (`config`, `Config` notices, `config_set` results), read
// defensively, and the changes a Settings form makes to it.
export type ConfigProblem = {
  /** The key it concerns (`files.root`), if any. */
  key: string | null;
  line: number;
  column: number;
  message: string;
};
export type UserConfigValues = {
  files: { root: string; extra_roots: string[] };
  projects: { base: string };
  scratch: { folder: string };
};
export type UserConfigState = {
  path: string;
  exists: boolean;
  revision: string;
  /** Whether `values` is the file's current content (false while the file
   * is invalid: the last valid configuration stays in effect). */
  current: boolean;
  values: UserConfigValues;
  resolved: UserConfigValues;
  errors: ConfigProblem[];
};
export type SaveResult = {
  saved: boolean;
  errors: ConfigProblem[];
  config: UserConfigState | null;
};
/** One change per key, as the bridge's `config_set` takes them. */
export type ConfigChanges = Record<string, string | string[]>;

const text = (value: unknown) => (typeof value === "string" ? value : null);
const strings = (value: unknown) =>
  Array.isArray(value) && value.every((item) => typeof item === "string")
    ? (value as string[])
    : null;

function readValues(value: any): UserConfigValues | null {
  const root = text(value?.files?.root);
  const extra = strings(value?.files?.extra_roots);
  const base = text(value?.projects?.base);
  const folder = text(value?.scratch?.folder);
  if (root === null || extra === null || base === null || folder === null)
    return null;
  return {
    files: { root, extra_roots: extra },
    projects: { base },
    scratch: { folder },
  };
}

export function readProblems(value: unknown): ConfigProblem[] {
  if (!Array.isArray(value)) return [];
  return value
    .filter((problem) => typeof problem?.message === "string")
    .slice(0, 32)
    .map((problem) => ({
      key: typeof problem.key === "string" ? problem.key : null,
      line: Number.isSafeInteger(problem.line) ? problem.line : 1,
      column: Number.isSafeInteger(problem.column) ? problem.column : 1,
      message: problem.message,
    }));
}

/** A bridge `config` result or `Config` notice, or null when malformed. */
export function readUserConfig(value: any): UserConfigState | null {
  const values = readValues(value?.values);
  const resolved = readValues(value?.resolved);
  if (!values || !resolved || typeof value.path !== "string") return null;
  return {
    path: value.path,
    exists: value.exists === true,
    revision: String(value.revision ?? ""),
    current: value.current !== false,
    values,
    resolved,
    errors: readProblems(value.errors),
  };
}

export function readSaveResult(value: any): SaveResult | null {
  if (typeof value?.saved !== "boolean") return null;
  return {
    saved: value.saved,
    errors: readProblems(value.errors),
    config: readUserConfig(value.config),
  };
}

export const position = (problem: ConfigProblem) =>
  `${problem.line}:${problem.column}`;

export type Draft = {
  root: string;
  extra: string[];
  base: string;
  folder: string;
};
export const draftOf = (values: UserConfigValues): Draft => ({
  root: values.files.root,
  extra: [...values.files.extra_roots],
  base: values.projects.base,
  folder: values.scratch.folder,
});
const sameList = (a: string[], b: string[]) =>
  a.length === b.length && a.every((item, index) => item === b[index]);

/** The keys whose draft differs from the file. */
export function configChanges(
  values: UserConfigValues,
  draft: Draft,
): ConfigChanges {
  const changes: ConfigChanges = {};
  if (draft.root !== values.files.root) changes["files.root"] = draft.root;
  if (!sameList(draft.extra, values.files.extra_roots))
    changes["files.extra_roots"] = draft.extra;
  if (draft.base !== values.projects.base)
    changes["projects.base"] = draft.base;
  if (draft.folder !== values.scratch.folder)
    changes["scratch.folder"] = draft.folder;
  return changes;
}
