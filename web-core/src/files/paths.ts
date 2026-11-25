// Path helpers for file browsers over KelpServer `files`: absolute POSIX
// paths as the server normalises them (no trailing slash, no `.` or `..`).
// Pure functions, no DOM or React, so `node --test` loads them directly.
import type { FileEntry } from "./api.ts";
import { pastedPaths } from "../uploads.ts";

/** `name` inside the folder `dir`. */
export function joinPath(dir: string, name: string): string {
  return dir === "/" ? `/${name}` : `${dir}/${name}`;
}
/** The folder holding `path` (`/` for `/` itself). */
export function parentPath(path: string): string {
  if (path === "/") return "/";
  const slash = path.lastIndexOf("/");
  return slash <= 0 ? "/" : path.slice(0, slash);
}
/** The last component of `path` (`/` for the root). */
export function baseName(path: string): string {
  if (path === "/") return "/";
  return path.slice(path.lastIndexOf("/") + 1);
}
/** Whether `path` is `dir` or inside it. */
export function isInside(path: string, dir: string): boolean {
  if (dir === "/") return path.startsWith("/");
  return path === dir || path.startsWith(`${dir}/`);
}
/** `path` relative to the home folder as `~` or `~/…`, else unchanged. */
export function displayPath(path: string, home?: string): string {
  if (!home || home === "/") return path;
  if (path === home) return "~";
  return path.startsWith(`${home}/`) ? `~${path.slice(home.length)}` : path;
}

export type Crumb = { label: string; path: string };
/** The folders from the root (or from `~` inside the home folder) down to
 * `path`, each with the path it opens. */
export function breadcrumbs(path: string, home?: string): Crumb[] {
  const crumbs: Crumb[] = [];
  let rest = path;
  let base = "/";
  if (home && home !== "/" && isInside(path, home)) {
    crumbs.push({ label: "~", path: home });
    rest = path.slice(home.length);
    base = home;
  } else crumbs.push({ label: "/", path: "/" });
  for (const part of rest.split("/").filter(Boolean)) {
    base = joinPath(base, part);
    crumbs.push({ label: part, path: base });
  }
  return crumbs;
}

/** Why `name` cannot name a new or renamed entry, or null. */
export function nameProblem(name: string): string | null {
  if (!name) return "Enter a name.";
  if (name === "." || name === "..") return "This name is reserved.";
  if (name.includes("/")) return "A name cannot contain /.";
  if (name.includes("\0")) return "A name cannot contain NUL.";
  if (new TextEncoder().encode(name).length > 255)
    return "The name is longer than 255 bytes.";
  return null;
}
/** Why `from` cannot move into the folder `toDir`, or null when it can. */
export function moveProblem(from: string, toDir: string): string | null {
  if (parentPath(from) === toDir) return "It is already in this folder.";
  if (isInside(toDir, from)) return "A folder cannot move into itself.";
  return null;
}
/** Whether a path can be typed into a shell: control characters (a newline
 * would run a command) are refused, everything else is quoted. */
export function pastablePath(path: string): boolean {
  return !/[\u0000-\u001f\u007f-\u009f]/.test(path);
}
/** The shell-quoted text typed into a terminal for these paths, with no
 * newline, or null when one of them cannot be typed safely. */
export function typedPaths(paths: readonly string[]): string | null {
  return paths.every(pastablePath) ? pastedPaths(paths) : null;
}

const collator = new Intl.Collator(undefined, {
  numeric: true,
  sensitivity: "base",
});
/** Whether an entry is hidden (its name starts with a dot). */
export const hiddenEntry = (entry: Pick<FileEntry, "name">) =>
  entry.name.startsWith(".");
/** Entries as a browser shows them: folders first, then by name (natural,
 * case-insensitive order), hidden ones left out unless `hidden`. */
export function sortEntries<T extends Pick<FileEntry, "name" | "kind">>(
  entries: readonly T[],
  options: { hidden?: boolean } = {},
): T[] {
  return entries
    .filter((entry) => options.hidden || !hiddenEntry(entry))
    .sort(
      (a, b) =>
        Number(b.kind === "dir") - Number(a.kind === "dir") ||
        collator.compare(a.name, b.name) ||
        (a.name < b.name ? -1 : a.name > b.name ? 1 : 0),
    );
}
