// The Files view's tab strip (FileTabs.tsx): what each open file's tab
// says and which tab shows after one closes. Pure, for the unit tests.

/** A tab's label: the file's name and, when another open file has the same
 * name, the shortest run of its parent folders that tells them apart
 * (like VS Code's tab description). */
export type TabLabel = { name: string; hint: string };

const segments = (path: string) => path.split("/").filter(Boolean);

/** The labels of the open files, by path. */
export function tabLabels(paths: string[]): Map<string, TabLabel> {
  const byName = new Map<string, string[]>();
  for (const path of paths) {
    const name = segments(path).at(-1) || path;
    byName.set(name, [...(byName.get(name) || []), path]);
  }
  const labels = new Map<string, TabLabel>();
  for (const [name, group] of byName) {
    const parents = group.map((path) => segments(path).slice(0, -1));
    const deepest = Math.max(...parents.map((p) => p.length));
    // The fewest trailing parent folders that make every hint unique.
    let depth = 1;
    const hintAt = (folders: string[], n: number) =>
      folders.slice(Math.max(0, folders.length - n)).join("/") || "/";
    while (
      group.length > 1 &&
      depth < deepest &&
      new Set(parents.map((p) => hintAt(p, depth))).size < group.length
    )
      depth++;
    group.forEach((path, index) =>
      labels.set(path, {
        name,
        hint: group.length > 1 ? hintAt(parents[index], depth) : "",
      }),
    );
  }
  return labels;
}

/** The tab shown after `closing` closes while it is shown: the next tab,
 * else the previous one, else the browser (""). */
export function neighbourTab(paths: string[], closing: string): string {
  const index = paths.indexOf(closing);
  if (index < 0) return "";
  return paths[index + 1] ?? paths[index - 1] ?? "";
}
