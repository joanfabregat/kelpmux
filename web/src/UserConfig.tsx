// The user configuration file (~/.config/kelpmux/config.toml) in Settings:
// the bridge's `config` state, its problems, and the Files & folders
// editor. The bridge validates and saves; this view never checks paths
// itself, so the browser, `kelpmux config check` and the live reload share
// one validator.
import { useEffect, useRef, useState } from "react";
import { Icon } from "./status";
import {
  configChanges,
  draftOf,
  position,
  type ConfigChanges,
  type ConfigProblem,
  type Draft,
  type SaveResult,
  type UserConfigState,
} from "./user-config";

/** The problems of an invalid file, shown above every Settings section
 * until the file is fixed. */
export function ConfigProblems({ config }: { config: UserConfigState }) {
  if (!config.errors.length) return null;
  return (
    <div className="notice error config-problems" role="alert">
      <p>
        <Icon name="circle-alert" size="sm" />
        <span>
          <b>
            {config.path.split("/").pop() || "config.toml"} has{" "}
            {config.errors.length === 1 ? "an error" : "errors"}
          </b>
          {": "}
          {config.exists
            ? "the last valid configuration stays in effect until it is fixed."
            : "the defaults apply."}
        </span>
      </p>
      <ul>
        {config.errors.map((problem, index) => (
          <li key={index}>
            <code>{position(problem)}</code> {problem.message}
          </li>
        ))}
      </ul>
      <p className="config-path">
        <code>{config.path}</code> · check it with{" "}
        <code>kelpmux config check</code>
      </p>
    </div>
  );
}

function FieldProblems({
  problems,
  field,
}: {
  problems: ConfigProblem[];
  field: string;
}) {
  const mine = problems.filter((problem) => problem.key === field);
  if (!mine.length) return null;
  return (
    <ul className="config-field-problems" data-config-problems={field}>
      {mine.map((problem, index) => (
        <li key={index} role="alert">
          {problem.message}
        </li>
      ))}
    </ul>
  );
}

/** Settings → Files & folders: the file browser's roots and the default
 * project and scratch folders, saved to the user configuration. */
export function FilesAndFolders({
  config,
  save,
  serverRoot,
}: {
  config: UserConfigState | null;
  save: (changes: ConfigChanges) => Promise<SaveResult | null>;
  /** The file root the running server uses (Welcome `file_limits.root`). */
  serverRoot?: string;
}) {
  const [draft, setDraft] = useState<Draft | null>(
    config ? draftOf(config.values) : null,
  );
  const [problems, setProblems] = useState<ConfigProblem[]>([]);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [newRoot, setNewRoot] = useState("");
  // The values the form was filled from: it is edited when it differs.
  const shown = useRef(config?.values);
  const changes = config && draft ? configChanges(config.values, draft) : {};
  const dirty = Object.keys(changes).length > 0;
  // A change made elsewhere (an agent, another window) replaces an
  // unedited form; an edited one keeps the user's typing.
  useEffect(() => {
    if (!config) return;
    const edited =
      draft &&
      shown.current &&
      Object.keys(configChanges(shown.current, draft)).length > 0;
    if (!edited) setDraft(draftOf(config.values));
    shown.current = config.values;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [config?.revision, config?.current]);
  if (!config || !draft)
    return (
      <>
        <h3>Files &amp; folders</h3>
        <p className="setting-hint">
          This kelpmux web bridge cannot read the user configuration. Install a
          matching kelpmux on the server.
        </p>
      </>
    );
  const set = (next: Partial<Draft>) => {
    setDraft({ ...draft, ...next });
    setSaved(false);
  };
  const general = problems.filter(
    (problem) =>
      !problem.key ||
      ![
        "files.root",
        "files.extra_roots",
        "projects.base",
        "scratch.folder",
      ].includes(problem.key),
  );
  const restartNeeded =
    !!serverRoot && config.current && config.resolved.files.root !== serverRoot;
  const input = (
    label: string,
    field: string,
    value: string,
    onChange: (value: string) => void,
  ) => (
    <input
      aria-label={label}
      data-config-key={field}
      aria-invalid={problems.some((problem) => problem.key === field)}
      value={value}
      spellCheck={false}
      autoCapitalize="off"
      autoCorrect="off"
      onChange={(event) => onChange(event.target.value)}
    />
  );
  const addRoot = () => {
    const path = newRoot.trim();
    if (!path) return;
    set({ extra: [...draft.extra, path] });
    setNewRoot("");
  };
  return (
    <form
      className="config-form"
      onSubmit={async (event) => {
        event.preventDefault();
        if (!dirty || saving) return;
        setSaving(true);
        const result = await save(changes);
        setSaving(false);
        if (!result) return;
        setProblems(result.errors);
        if (result.saved) {
          setSaved(true);
          if (result.config) setDraft(draftOf(result.config.values));
        }
      }}
    >
      <h3>File browser</h3>
      <label className="upload-folder">
        Root: the file browser shows nothing outside it
        {input("File browser root", "files.root", draft.root, (root) =>
          set({ root }),
        )}
      </label>
      <FieldProblems problems={problems} field="files.root" />
      <p className="setting-hint">
        Applies on the next workspace start.
        {restartNeeded && (
          <>
            {" "}
            <span className="badge accent" data-config-restart>
              Restart the workspace to apply
            </span>{" "}
            The running workspace still uses <code>{serverRoot}</code>.
          </>
        )}
      </p>
      <div className="upload-folder config-list">
        <span id="config-extra-roots">
          Extra roots: folders outside it the file browser may also open
        </span>
        <ul aria-labelledby="config-extra-roots">
          {draft.extra.map((path, index) => (
            <li key={`${index}:${path}`}>
              {input(
                `Extra root ${index + 1}`,
                "files.extra_roots",
                path,
                (value) =>
                  set({
                    extra: draft.extra.map((item, i) =>
                      i === index ? value : item,
                    ),
                  }),
              )}
              <button
                type="button"
                className="icon-btn"
                aria-label={`Remove ${path}`}
                onClick={() =>
                  set({ extra: draft.extra.filter((_, i) => i !== index) })
                }
              >
                <Icon name="x" size="sm" />
              </button>
            </li>
          ))}
          <li>
            <input
              aria-label="New extra root"
              placeholder="/mnt/data"
              value={newRoot}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              onChange={(event) => setNewRoot(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  addRoot();
                }
              }}
            />
            <button
              type="button"
              className="subtle"
              disabled={!newRoot.trim()}
              onClick={addRoot}
            >
              <Icon name="plus" size="sm" />
              Add
            </button>
          </li>
        </ul>
      </div>
      <FieldProblems problems={problems} field="files.extra_roots" />
      <p className="setting-hint">
        Such as a data drive. Applies on the next workspace start, with a
        terminal server that supports extra roots.
      </p>
      <h3>Folders</h3>
      <label className="upload-folder">
        Projects: new projects go in this folder
        {input("Projects folder", "projects.base", draft.base, (base) =>
          set({ base }),
        )}
      </label>
      <FieldProblems problems={problems} field="projects.base" />
      <label className="upload-folder">
        Scratch: where terminals outside any project start
        {input("Scratch folder", "scratch.folder", draft.folder, (folder) =>
          set({ folder }),
        )}
      </label>
      <FieldProblems problems={problems} field="scratch.folder" />
      <p className="setting-hint">
        Both must lie inside the root or an extra root. Paths are absolute or
        start with <code>~/</code>.
      </p>
      {general.length > 0 && (
        <div className="notice error" role="alert">
          {general.map((problem, index) => (
            <p key={index}>
              <code>{position(problem)}</code> {problem.message}
            </p>
          ))}
        </div>
      )}
      <div className="confirm-buttons">
        {saved && !dirty && (
          <span className="config-saved" role="status">
            <Icon name="circle-check" size="sm" /> Saved
          </span>
        )}
        <button
          type="button"
          className="subtle"
          disabled={!dirty || saving}
          onClick={() => {
            setDraft(draftOf(config.values));
            setProblems([]);
          }}
        >
          Revert
        </button>
        <button type="submit" disabled={!dirty || saving}>
          {saving ? "Saving…" : "Save"}
        </button>
      </div>
      <p className="setting-hint">
        Saved for every KelpMux on this account in <code>{config.path}</code>,
        keeping its comments. You or an agent can also edit that file (check it
        with <code>kelpmux config check</code>); changes made there appear here
        at once.
      </p>
    </form>
  );
}
