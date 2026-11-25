// Projects (bridge API minor 5): a name and a folder, declared in the user
// configuration (`[[project]]`, as the bridge's `config` reports them), and
// the built-in Scratch project on `scratch.folder`. Every terminal belongs to
// exactly one project, decided when it is created and changed only by a
// move; the bridge keeps the links (`projects`: pane → project id) for
// every browser. A terminal without a link, or linked to a project that no
// longer exists, belongs to Scratch. Pure functions, no DOM or React, so
// `node --test` loads them directly.

/** The built-in project's id; no user project can take it. */
export const SCRATCH = "scratch";

export type Project = {
  id: string;
  name: string;
  /** The folder as written in config.toml (`~/projects/kelp`). */
  folder: string;
  /** The folder resolved (`/home/joan/projects/kelp`). */
  path: string;
  color: string | null;
  /** The folder does not exist (any more). */
  missing: boolean;
  /** Scratch, the built-in project. */
  scratch?: boolean;
};

/** Named colours a project may have (config.toml `color`), in the order
 * the dialogs offer them; `#rrggbb` is accepted too. */
export const projectColors = [
  "red",
  "orange",
  "yellow",
  "green",
  "teal",
  "blue",
  "purple",
  "pink",
  "gray",
] as const;
const swatches: Record<string, string> = {
  red: "#e5626a",
  orange: "#e8915a",
  yellow: "#d9b64a",
  green: "#5cbf7d",
  teal: "#3fb8b0",
  blue: "#5b9ee6",
  purple: "#a083e6",
  pink: "#e07ab8",
  gray: "#8c939c",
};

/** The CSS colour of a project: its colour, or none (the theme's text). */
export function projectColor(project: Pick<Project, "color"> | undefined) {
  const color = project?.color;
  if (!color) return "";
  if (/^#[0-9a-fA-F]{6}$/.test(color)) return color;
  return swatches[color] || "";
}

/** Reads the bridge's `config` result's `projects`, defensively. */
export function readProjects(value: unknown): Project[] {
  if (!Array.isArray(value)) return [];
  return value
    .filter(
      (project) =>
        typeof project?.id === "string" &&
        typeof project?.name === "string" &&
        typeof project?.path === "string",
    )
    .slice(0, 256)
    .map((project) => ({
      id: project.id,
      name: project.name,
      folder: typeof project.folder === "string" ? project.folder : project.path,
      path: project.path,
      color: typeof project.color === "string" ? project.color : null,
      missing: project.missing === true,
    }));
}

/** Reads the bridge's `projects` result: pane id → project id. */
export function readLinks(value: any): Record<number, string> {
  const links: Record<number, string> = {};
  const panes = value?.panes;
  if (!panes || typeof panes !== "object") return links;
  for (const [pane, project] of Object.entries(panes))
    if (/^[1-9]\d{0,9}$/.test(pane) && typeof project === "string")
      links[Number(pane)] = project;
  return links;
}

/** The Scratch project on its folder. */
export function scratchProject(folder: string, path: string): Project {
  return {
    id: SCRATCH,
    name: "Scratch",
    folder,
    path,
    color: null,
    missing: false,
    scratch: true,
  };
}

/** The project a terminal belongs to: its link when that project exists,
 * else Scratch. */
export function projectOf(
  pane: number,
  links: Record<number, string>,
  projects: Project[],
): string {
  const id = links[pane];
  return id && projects.some((project) => project.id === id) ? id : SCRATCH;
}

/** The projects as the sidebar lists them: the user's projects by name,
 * then Scratch last. */
export function sidebarProjects(projects: Project[], scratch: Project) {
  return [
    ...[...projects].sort((a, b) =>
      a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
    ),
    scratch,
  ];
}

/** Terminals in their projects' order (the sidebar's), keeping each
 * project's own order: the numbering, the grid and Alt+digit follow it. */
export function groupOrder<T extends { id: number }>(
  panes: T[],
  order: string[],
  projectOfPane: (pane: number) => string,
): T[] {
  const rank = (pane: T) => {
    const index = order.indexOf(projectOfPane(pane.id));
    return index < 0 ? order.length : index;
  };
  return panes
    .map((pane, index) => ({ pane, index, rank: rank(pane) }))
    .sort((a, b) => a.rank - b.rank || a.index - b.index)
    .map((entry) => entry.pane);
}

const inside = (path: string, folder: string) =>
  path === folder || path.startsWith(folder.endsWith("/") ? folder : `${folder}/`);

/** The project whose folder holds `path` (the deepest; folders never
 * overlap, so at most one), if any. */
export function projectAt(path: string, projects: Project[]) {
  let found: Project | undefined;
  for (const project of projects)
    if (inside(path, project.path) && (!found || project.path.length > found.path.length))
      found = project;
  return found;
}

/** The project whose folder is exactly `path`. */
export function projectFolder(path: string, projects: Project[]) {
  return projects.find((project) => project.path === path);
}

/** Why a new project at `path` would overlap an existing one (or Scratch),
 * or null; the bridge checks again. */
export function overlapProblem(path: string, projects: Project[]) {
  for (const project of projects) {
    if (path === project.path)
      return `This folder is already the folder of “${project.name}”.`;
    if (inside(path, project.path))
      return `This folder is inside the folder of “${project.name}”.`;
    if (inside(project.path, path))
      return `This folder contains the folder of “${project.name}”.`;
  }
  return null;
}

/** Why a project name is refused, or null: as the bridge's validator. */
export function nameProblem(name: string, projects: Project[], except = "") {
  if (!name.trim()) return "Enter a name.";
  if (name.trim() !== name) return "The name must not start or end with spaces.";
  if ([...name].length > 64) return "The name must be at most 64 characters.";
  if (/[\u0000-\u001f\u007f/]/.test(name))
    return "The name must not contain “/” or control characters.";
  if (name === "." || name === "..") return "Choose another name.";
  if (name.toLowerCase() === "scratch")
    return "Scratch is the built-in project's name.";
  if (
    projects.some(
      (project) =>
        project.id !== except &&
        !project.scratch &&
        project.name.toLowerCase() === name.toLowerCase(),
    )
  )
    return `Another project is already named “${name}”.`;
  return null;
}

/** The folder a new project gets without one: `<base>/<name>`, as written. */
export function defaultFolder(base: string, name: string) {
  return `${base.replace(/\/+$/, "")}/${name}`;
}
