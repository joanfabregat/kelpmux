// State marks (from KelpHub's terminal UI): one mark per display state, used everywhere a
// terminal, workspace or project shows its state. Each mark has its own shape
// as well as its colour, so it stays readable without colour vision.
import type React from "react";
import {
  rollupEntries,
  rollupSummary,
  stateLabel,
  type DisplayState,
  type Rollup,
} from "./model";
import "../styles/state.css";

const glyphs: Record<DisplayState, React.ReactNode> = {
  // Amber disc with an exclamation mark.
  input_needed: (
    <>
      <circle cx="8" cy="8" r="7" fill="currentColor" />
      <path d="M8 4v4.6" className="state-mark-ink" />
      <circle cx="8" cy="11.4" r="1.1" className="state-mark-ink-fill" />
    </>
  ),
  // Red disc with a cross.
  failed: (
    <>
      <circle cx="8" cy="8" r="7" fill="currentColor" />
      <path d="M5.5 5.5l5 5M10.5 5.5l-5 5" className="state-mark-ink" />
    </>
  ),
  // Green disc with a check.
  finished: (
    <>
      <circle cx="8" cy="8" r="7" fill="currentColor" />
      <path d="M4.8 8.3l2.1 2.1 4.3-4.6" className="state-mark-ink" />
    </>
  ),
  // Blue ring with a rotating arc.
  working: (
    <>
      <circle
        cx="8"
        cy="8"
        r="5.6"
        fill="none"
        stroke="currentColor"
        strokeOpacity="0.3"
        strokeWidth="2.4"
      />
      <path
        d="M8 2.4a5.6 5.6 0 0 1 5.6 5.6"
        fill="none"
        stroke="currentColor"
        strokeWidth="2.4"
        strokeLinecap="round"
        className="state-mark-spin"
      />
    </>
  ),
  // Grey dashed ring, slowly turning: a shell command runs (never counted).
  running: (
    <circle
      cx="8"
      cy="8"
      r="5.6"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.2"
      strokeDasharray="2.6 2.3"
      className="state-mark-spin state-mark-slow"
    />
  ),
  // Grey hollow ring.
  idle: (
    <circle
      cx="8"
      cy="8"
      r="5.5"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
    />
  ),
};

export function StateMark({
  state,
  label,
  size = "sm",
  decorative = false,
  className = "",
}: {
  state: DisplayState;
  /** Accessible name; defaults to the state's label. */
  label?: string;
  size?: "sm" | "md" | "lg";
  /** Hide from assistive technology when adjacent text already says it. */
  decorative?: boolean;
  className?: string;
}) {
  const name = label || stateLabel[state];
  return (
    <svg
      className={`state-mark state-${state} state-mark-${size} ${className}`.trim()}
      viewBox="0 0 16 16"
      role={decorative ? undefined : "img"}
      aria-label={decorative ? undefined : name}
      aria-hidden={decorative ? true : undefined}
      focusable="false"
    >
      {decorative ? null : <title>{name}</title>}
      {glyphs[state]}
    </svg>
  );
}

/** A roll-up: each present state's mark with its count, most urgent first.
 * With `mostUrgentOnly`, just the most urgent mark and its count. */
export function StateCounts({
  rollup,
  hideIdle = false,
  mostUrgentOnly = false,
  className = "",
}: {
  rollup: Rollup | null | undefined;
  hideIdle?: boolean;
  mostUrgentOnly?: boolean;
  className?: string;
}) {
  if (!rollup || !rollup.total) return null;
  let entries = rollupEntries(rollup).filter(
    ([state]) => !(hideIdle && state === "idle"),
  );
  if (mostUrgentOnly) entries = entries.slice(0, 1);
  if (!entries.length) return null;
  return (
    <span
      className={`state-counts ${className}`.trim()}
      role="img"
      aria-label={rollupSummary(rollup)}
      title={rollupSummary(rollup)}
    >
      {entries.map(([state, count]) => (
        <span key={state} className={`state-count state-${state}`}>
          <StateMark state={state} decorative />
          <span className="state-count-n" aria-hidden="true">
            {count}
          </span>
        </span>
      ))}
    </span>
  );
}
