import type { Pane, Snapshot } from "./types";
/** Keep recent terminal frames across subscriptions, with a bounded memory cost. */
export function trimScreens(
  screens: Map<number, Snapshot>,
  panes: Pane[],
  subscribed: number[],
) {
  const live = new Set(panes.map((pane) => pane.id));
  for (const id of screens.keys()) if (!live.has(id)) screens.delete(id);
  const pinned = new Set(subscribed);
  let cells = [...screens.values()].reduce(
    (total, snapshot) => total + snapshot.screen.cells.length,
    0,
  );
  for (const [id, snapshot] of screens) {
    if (screens.size <= 8 && cells <= 200000) break;
    if (pinned.has(id)) continue;
    screens.delete(id);
    cells -= snapshot.screen.cells.length;
  }
}
