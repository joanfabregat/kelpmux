// The project dialogs, in the confirmation dialog's style: create a project
// (a name, a folder defaulting to `<projects.base>/<name>`, a colour) or
// rename one (name and colour; the id and folder stay), and move a terminal
// to another project (the keyboard and touch alternative to dragging its
// row). The bridge validates and saves (config.toml); its problems show
// under the field they concern.
import React, { useEffect, useRef, useState } from "react";
import { Icon } from "./status";
import {
  defaultFolder,
  nameProblem,
  overlapProblem,
  projectColor,
  projectColors,
  type Project,
} from "./projects";

export type ProjectDraft = {
  name: string;
  folder: string;
  color: string | null;
};
/** A saved change, or the bridge's problems by field (`name`, `folder`,
 * `color`, or `""` for the rest). */
export type ProjectSaveResult = { saved: boolean; problems: Record<string, string[]> };

export function ProjectDialog({
  mode,
  project,
  initial,
  base,
  projects,
  resolve,
  onSave,
  onCancel,
}: {
  mode: "create" | "rename";
  /** The project renamed. */
  project?: Project;
  /** Prefilled values (Create project from this folder…). */
  initial?: Partial<ProjectDraft>;
  /** `projects.base`, as written: the default folder's parent. */
  base: string;
  /** Every project, Scratch included, for the checks made before saving. */
  projects: Project[];
  /** A folder as written (`~/x`) resolved, for the overlap check; null
   * when it cannot be resolved here. */
  resolve: (folder: string) => string | null;
  onSave: (draft: ProjectDraft) => Promise<ProjectSaveResult | null>;
  onCancel: () => void;
}) {
  const [draft, setDraft] = useState<ProjectDraft>({
    name: initial?.name ?? project?.name ?? "",
    folder: initial?.folder ?? "",
    color: initial?.color ?? project?.color ?? null,
  });
  const [problems, setProblems] = useState<Record<string, string[]>>({});
  const [busy, setBusy] = useState(false);
  const nameInput = useRef<HTMLInputElement>(null);
  useEffect(() => {
    nameInput.current?.focus();
    nameInput.current?.select();
  }, []);
  const folder = draft.folder.trim() || defaultFolder(base, draft.name || "…");
  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (busy) return;
    const local: Record<string, string[]> = {};
    const name = nameProblem(draft.name, projects, project?.id);
    if (name) local.name = [name];
    if (mode === "create") {
      const path = resolve(folder);
      const overlap = path && overlapProblem(path, projects);
      if (overlap) local.folder = [overlap];
    }
    setProblems(local);
    if (Object.keys(local).length) return;
    setBusy(true);
    try {
      const result = await onSave({ ...draft, folder: draft.folder.trim() });
      if (result && !result.saved) setProblems(result.problems);
    } finally {
      setBusy(false);
    }
  }
  const fieldProblems = (field: string) =>
    problems[field]?.length ? (
      <ul className="config-field-problems" data-project-problems={field}>
        {problems[field].map((message, index) => (
          <li key={index} role="alert">
            {message}
          </li>
        ))}
      </ul>
    ) : null;
  const title = mode === "create" ? "New project" : `Rename “${project?.name}”`;
  return (
    <div
      className="modal-shade confirmation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !busy) onCancel();
      }}
    >
      <form
        className="confirm-dialog project-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="project-dialog-title"
        onSubmit={submit}
        onKeyDown={(event) => {
          if (event.key === "Escape" && !busy) {
            event.stopPropagation();
            onCancel();
          }
        }}
      >
        <h2 id="project-dialog-title">{title}</h2>
        <label className="project-field">
          <span>Name</span>
          <input
            ref={nameInput}
            aria-label="Project name"
            aria-invalid={!!problems.name}
            value={draft.name}
            maxLength={64}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            onChange={(event) => {
              setDraft({ ...draft, name: event.target.value });
              setProblems({});
            }}
          />
        </label>
        {fieldProblems("name")}
        {mode === "create" ? (
          <>
            <label className="project-field">
              <span>Folder</span>
              <input
                aria-label="Project folder"
                aria-invalid={!!problems.folder}
                value={draft.folder}
                placeholder={folder}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                onChange={(event) => {
                  setDraft({ ...draft, folder: event.target.value });
                  setProblems({});
                }}
              />
            </label>
            {fieldProblems("folder")}
            <p className="setting-hint project-hint">
              {draft.folder.trim()
                ? "The folder is created if it does not exist."
                : `Empty: ${folder}, created for the project.`}
            </p>
          </>
        ) : (
          <p className="setting-hint project-hint mono">{project?.folder}</p>
        )}
        <fieldset className="project-colors">
          <legend>Colour</legend>
          <button
            type="button"
            className={`project-swatch none ${draft.color ? "" : "selected"}`}
            aria-label="No colour"
            aria-pressed={!draft.color}
            onClick={() => setDraft({ ...draft, color: null })}
          />
          {projectColors.map((color) => (
            <button
              type="button"
              key={color}
              className={`project-swatch ${draft.color === color ? "selected" : ""}`}
              style={{ background: projectColor({ color }) }}
              aria-label={color}
              aria-pressed={draft.color === color}
              onClick={() => setDraft({ ...draft, color })}
            />
          ))}
        </fieldset>
        {fieldProblems("color")}
        {fieldProblems("")}
        <div className="confirm-buttons">
          <button type="button" className="subtle" disabled={busy} onClick={onCancel}>
            Cancel
          </button>
          <button type="submit" disabled={busy}>
            {mode === "create" ? "Create project" : "Save"}
          </button>
        </div>
      </form>
    </div>
  );
}

/** Moves a terminal to another project: a list to choose from. */
export function MoveToProjectDialog({
  title,
  projects,
  current,
  onMove,
  onCancel,
}: {
  title: string;
  projects: Project[];
  current: string;
  onMove: (project: Project) => void;
  onCancel: () => void;
}) {
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => {
    list.current
      ?.querySelector<HTMLButtonElement>('[aria-current="true"], button')
      ?.focus();
  }, []);
  return (
    <div
      className="modal-shade confirmation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onCancel();
      }}
    >
      <section
        className="confirm-dialog project-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="move-project-title"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.stopPropagation();
            onCancel();
          }
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            const buttons = [
              ...(list.current?.querySelectorAll<HTMLButtonElement>("button") || []),
            ];
            const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
            const next = buttons[(index + (event.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length];
            next?.focus();
          }
        }}
      >
        <h2 id="move-project-title">Move “{title}” to project</h2>
        <p>Only the project changes; the shell stays in its folder.</p>
        <div className="project-choices" role="list" ref={list}>
          {projects.map((project) => {
            const color = projectColor(project);
            return (
              <button
                type="button"
                key={project.id}
                role="listitem"
                className="menu-item project-choice"
                aria-current={project.id === current}
                data-move-project={project.id}
                onClick={() => onMove(project)}
                style={color ? ({ "--project": color } as React.CSSProperties) : undefined}
              >
                <span className={`project-dot ${color ? "" : "plain"}`} aria-hidden="true" />
                <span className="grow">{project.name}</span>
                {project.id === current && <Icon name="check" size="sm" />}
              </button>
            );
          })}
        </div>
        <div className="confirm-buttons">
          <button type="button" className="subtle" onClick={onCancel}>
            Cancel
          </button>
        </div>
      </section>
    </div>
  );
}
