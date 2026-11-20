export const basename = (path: string) =>
  path.replace(/\/$/, "").split("/").pop() || path;
export const age = (at?: number) => {
  if (!at) return "Previous run";
  const seconds = Math.max(0, Date.now() / 1000 - at);
  for (const [unit, n] of [
    ["w", 604800],
    ["d", 86400],
    ["h", 3600],
    ["m", 60],
    ["s", 1],
  ] as const)
    if (seconds >= n || n === 1) return `${Math.floor(seconds / n)}${unit} ago`;
};
export const bytes = (n: number) => {
  for (const [unit, factor] of [
    ["TiB", 2 ** 40],
    ["GiB", 2 ** 30],
    ["MiB", 2 ** 20],
    ["KiB", 1024],
  ] as const)
    if (n >= factor) return `${(n / factor).toFixed(1)} ${unit}`;
  return `${n || 0} B`;
};
