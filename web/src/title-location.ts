// The installed app's title bar (TitleBar.tsx): what it says about the place
// shown. Kept free of React so the unit tests can import it.

/** The place the title bar names. */
export type TitlePlace =
  | { view: "sign-in" }
  | { view: "stopped"; workspace: string }
  | { view: "terminals" }
  | { view: "grid" }
  | { view: "repos"; project: string }
  | { view: "project"; project: string }
  | { view: "envs"; project: string }
  | { view: "terminal"; title?: string }
  | { view: "files"; dir: string; file?: string; home?: string };

/** A folder for display: the home folder and what is inside it start with
 * `~`. */
export function displayDir(dir: string, home?: string) {
  if (!home || home === "/") return dir;
  if (dir === home) return "~";
  if (dir.startsWith(home + "/")) return "~" + dir.slice(home.length);
  return dir;
}

/** The title bar's location text: the terminal shown alone, the grid, the
 * file browser's folder or the open file. Empty when there is nothing to
 * name. */
export function titleLocation(place: TitlePlace): string {
  switch (place.view) {
    case "sign-in":
      return "Sign in";
    case "stopped":
      return place.workspace ? `Workspace ${place.workspace} stopped` : "";
    case "terminals":
      return "Terminals";
    case "grid":
      return "Grid";
    case "repos":
      return `Repos — ${place.project}`;
    case "project":
      return place.project;
    case "envs":
      return `Environments — ${place.project}`;
    case "terminal":
      return place.title === undefined ? "" : place.title || "shell";
    case "files":
      if (place.file)
        return place.file.split("/").filter(Boolean).pop() || place.file;
      return `Files — ${displayDir(place.dir, place.home)}`;
  }
}
