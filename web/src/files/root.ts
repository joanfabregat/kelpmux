// The file root: KelpServer confines every file operation to one folder,
// the server user's home folder unless it was started with another, and
// advertises it as Welcome `file_limits.root` (`FilesClient.limits.root`).
// The views start there, offer nothing outside it and show the server's
// `outside_root` refusals in words. A server without a root (older than
// KelpCore 3a7d99c) keeps the earlier behaviour: everything is inside.
// Pure functions, no DOM or React, so `node --test` loads them directly.
import {
  FileError,
  describeCode,
  type FileEntry,
} from "@kelpcore/web/files/api.ts";
import {
  breadcrumbs,
  isInside,
  joinPath,
  parentPath,
  type Crumb,
} from "@kelpcore/web/files/paths.ts";
import { destinationProblem } from "@kelpcore/web/uploads.ts";
import { normalizePath } from "./path-links.ts";

/** What a path outside the root is told, as the server's `outside_root`. */
export const outsideRootMessage = describeCode("outside_root");

/** Whether `path` lies inside `root`: always without a root; `~` and `~/…`
 * inside `home` (inside, while the home folder is not known yet: the
 * server decides); never for other relative paths. `..` is resolved
 * first, as the server does. */
export function withinRoot(
  path: string,
  root: string | undefined,
  home?: string,
): boolean {
  if (!root) return true;
  if (path === "~" || path.startsWith("~/")) {
    if (!home?.startsWith("/")) return true;
    path = `${home}/${path.slice(1)}`;
  } else if (!path.startsWith("/")) return false;
  return isInside(normalizePath(path), root);
}

/** The folders from the root down to `path`, the root first (`~`, shown
 * as Home, when it is the home folder; else its path); none above it.
 * Without a root, or for a path outside it, as before: from `/`, or from
 * `~` inside the home folder. */
export function rootCrumbs(
  path: string,
  root: string | undefined,
  home?: string,
): Crumb[] {
  if (!root || root === "/" || !isInside(path, root))
    return breadcrumbs(path, home);
  const crumbs: Crumb[] = [{ label: root === home ? "~" : root, path: root }];
  let base = root;
  for (const part of path.slice(root.length).split("/").filter(Boolean)) {
    base = joinPath(base, part);
    crumbs.push({ label: part, path: base });
  }
  return crumbs;
}

/** Whether the folder above `dir` can be shown: not above the root, and
 * not above `/`. */
export function canGoUp(dir: string, root: string | undefined): boolean {
  if (dir === "/" || !dir.startsWith("/")) return false;
  return !root || (dir !== root && isInside(parentPath(dir), root));
}

/** Where the symbolic link at `path` points, as an absolute path (a
 * relative target against the link's folder), or null when not a link
 * with a target. Lexical: links further on are not followed. */
export function linkTarget(entry: FileEntry, path: string): string | null {
  if (entry.kind !== "symlink" || !entry.target) return null;
  const target = entry.target.startsWith("/")
    ? entry.target
    : `${parentPath(path)}/${entry.target}`;
  return normalizePath(target);
}

/** Whether the entry at `path` is a symbolic link pointing outside the
 * root: shown, but nothing opens, lists or downloads through it. */
export function leadsOutside(
  entry: FileEntry,
  path: string,
  root: string | undefined,
): boolean {
  if (!root) return false;
  const target = linkTarget(entry, path);
  return target !== null && !isInside(target, root);
}

/** What can be done with the entry at `path`: open it (or what it points
 * to), download it, and change it (rename, move, trash, delete). The root
 * itself can be opened only; a link pointing outside can be changed (the
 * link itself) but not opened or downloaded. */
export function entryAbilities(
  entry: FileEntry,
  path: string,
  root: string | undefined,
): { open: boolean; download: boolean; change: boolean } {
  if (root && path === root)
    return { open: true, download: false, change: false };
  const outside = leadsOutside(entry, path, root);
  return {
    open: !outside,
    download: !outside && entry.kind !== "other",
    change: true,
  };
}

/** Why `dest` cannot be an upload destination, or null: as KelpCore's
 * `destinationProblem`, and outside the root (the server refuses a chosen
 * destination there). Empty is the server's upload folder, which need not
 * lie inside the root. */
export function destinationRootProblem(
  dest: string,
  root: string | undefined,
): string | null {
  return (
    destinationProblem(dest) ||
    (dest && !withinRoot(dest, root) ? outsideRootMessage : null)
  );
}

/** `failure` with the friendly description of `outside_root` in place of
 * the server's message; anything else unchanged. */
export function friendlyError(failure: unknown): unknown {
  if (!(failure instanceof FileError) || failure.code !== "outside_root")
    return failure;
  return new FileError({
    code: failure.code,
    message: outsideRootMessage,
    path: failure.path,
  });
}
/** A failure as a message: `outside_root` in words, a file error without
 * a message by its code. */
export function errorMessage(failure: unknown): string {
  const friendly = friendlyError(failure);
  if (friendly instanceof FileError)
    return friendly.message || describeCode(friendly.code);
  return friendly instanceof Error ? friendly.message : String(friendly);
}
