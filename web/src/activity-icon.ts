// Browser tab favicon and title from the live KelpServer pane statuses. The
// badge shows the most urgent status across every pane (parked included);
// the title counts panes that need attention, and a dot in the top right
// corner shows an unread bell.
import type { Pane } from "@kelpcore/web/types.ts";

export type PaneState =
  | "input_needed"
  | "failed"
  | "turn_finished"
  | "working"
  | "waiting_background"
  | "idle";

/** Display label per KelpServer status, as the TUI names them. */
export const stateLabel: Record<string, string> = {
  input_needed: "Needs input",
  failed: "Failed",
  turn_finished: "Finished",
  working: "Working",
  waiting_background: "Background",
  idle: "Idle",
  exited: "Exited",
  unknown: "Unknown",
};

const urgency: PaneState[] = [
  "input_needed",
  "failed",
  "turn_finished",
  "working",
  "waiting_background",
];

/** The most urgent status among these panes, or null when all are idle. */
export function urgentState(panes: Pick<Pane, "status" | "dead">[]) {
  for (const state of urgency)
    if (panes.some((p) => !p.dead && p.status === state)) return state;
  return null;
}

export function attentionCount(panes: Pick<Pane, "status" | "dead">[]) {
  return panes.filter(
    (p) =>
      !p.dead && ["input_needed", "failed", "turn_finished"].includes(p.status),
  ).length;
}

// Dark-mode status token values (base.css); the favicon has a fixed dark
// background whatever the page mode.
const badges: Partial<Record<PaneState, [string, string]>> = {
  input_needed: ["!", "#f2b84b"],
  failed: ["×", "#f06a6a"],
  turn_finished: ["✓", "#5cc98a"],
  working: ["●", "#5fafff"],
};

/** The ringing bell's dot in the favicon's top right corner, in the bell
 * colour of the sidebar (--st-input), ringed in the icon's background. */
const bellDot =
  '<circle cx="25" cy="7" r="6" fill="#f2b84b" stroke="#101113" stroke-width="2" data-bell="1"/>';

/** The favicon and title. `bell`: an unread bell rings (toasts.ts
 * bellRinging), shown as a dot until it is read. */
export function activityIcon(
  panes: Pick<Pane, "status" | "dead">[] | null,
  bell = false,
) {
  const state = panes ? urgentState(panes) : null;
  const badge = state ? badges[state] : undefined;
  const mark = badge
    ? `<circle cx="24" cy="24" r="8" fill="${badge[1]}"/><text x="24" y="28" text-anchor="middle" font-family="sans-serif" font-size="12" font-weight="bold" fill="#101113">${badge[0]}</text>`
    : "";
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 32 32"><rect width="32" height="32" rx="6" fill="#101113"/><text x="7" y="24" font-family="sans-serif" font-size="24" font-weight="bold" fill="#79ade0">K</text>${mark}${bell ? bellDot : ""}</svg>`;
  const count = panes ? attentionCount(panes) : 0;
  return {
    title: count ? `(${count}) KelpMux` : "KelpMux",
    label: badge && state ? stateLabel[state] : "",
    bell,
    url: `data:image/svg+xml,${encodeURIComponent(svg)}`,
  };
}
