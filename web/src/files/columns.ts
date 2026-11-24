// The file browser's column view (FilesBrowser.tsx), like the Finder's:
// which folders show as columns, and the second line under each entry's
// name (its modification date and size). Pure functions, no DOM or React,
// so `node --test` loads them directly.
import type { FileEntry } from "@kelpcore/web/files/api.ts";
import { rootCrumbs } from "./root.ts";

/** The folders shown as columns for the deepest folder `dir`: the root
 * first, then each folder down to `dir`. A path not yet normalised (`~`)
 * is one column until the server names it. */
export function columnFolders(
  dir: string,
  root: string | undefined,
  home?: string,
): string[] {
  if (!dir.startsWith("/")) return [dir];
  return rootCrumbs(dir, root, home).map((crumb) => crumb.path);
}

/** A size as the Finder writes it, in decimal units: "512 bytes",
 * "4.2 KB", "13 MB"; one decimal below 10. */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "";
  if (bytes < 1000) return bytes === 1 ? "1 byte" : `${bytes} bytes`;
  const units = ["KB", "MB", "GB", "TB", "PB"];
  let value = bytes / 1000,
    unit = 0;
  while (value >= 999.5 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  const text =
    value < 9.95
      ? String(Math.round(value * 10) / 10)
      : String(Math.round(value));
  return `${text} ${units[unit]}`;
}

const dayStart = (date: Date) =>
  new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();

/** A modification time (Unix seconds), short: "Today 14:32",
 * "Yesterday 09:10", else the date alone ("Nov 3, 2026" in English).
 * `locale` defaults to the browser's. */
export function formatModified(
  seconds: number,
  now: Date = new Date(),
  locale?: string,
): string {
  const date = new Date(seconds * 1000);
  if (!seconds || !Number.isFinite(date.getTime())) return "";
  const time = new Intl.DateTimeFormat(locale, { timeStyle: "short" });
  const days = Math.round((dayStart(now) - dayStart(date)) / 86_400_000);
  if (days === 0) return `Today ${time.format(date)}`;
  if (days === 1) return `Yesterday ${time.format(date)}`;
  return new Intl.DateTimeFormat(locale, { dateStyle: "medium" }).format(date);
}

/** A modification time in full, for the info column: date and time. */
export function formatModifiedFull(seconds: number, locale?: string): string {
  const date = new Date(seconds * 1000);
  if (!seconds || !Number.isFinite(date.getTime())) return "";
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "long",
    timeStyle: "short",
  }).format(date);
}

/** What an entry's second line says after its date: a file's size,
 * "Folder", where a link points, or "Special file". */
export function entryDetail(entry: FileEntry): string {
  switch (entry.kind) {
    case "file":
      return formatSize(entry.size);
    case "dir":
      return "Folder";
    case "symlink":
      return entry.target ? `→ ${entry.target}` : "Link";
    default:
      return "Special file";
  }
}

/** An entry's second line: its date and its size (or kind), separated by
 * a middle dot. */
export function entryMeta(
  entry: FileEntry,
  now: Date = new Date(),
  locale?: string,
): string {
  return [formatModified(entry.modified, now, locale), entryDetail(entry)]
    .filter(Boolean)
    .join(" · ");
}

/** Type-to-select: the index of the first name starting with `prefix`
 * (case-insensitive), from `from` on and wrapping around; -1 if none. */
export function matchPrefix(
  names: readonly string[],
  prefix: string,
  from = 0,
): number {
  const wanted = prefix.toLocaleLowerCase();
  if (!wanted || !names.length) return -1;
  for (let step = 0; step < names.length; step++) {
    const index = (Math.max(0, from) + step) % names.length;
    if (names[index].toLocaleLowerCase().startsWith(wanted)) return index;
  }
  return -1;
}
