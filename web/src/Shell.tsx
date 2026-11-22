import type { Pane } from "./terminal/types";
import { StateMark } from "./state/StateMark";
import { ProviderLogo } from "./state/ProviderLogo";
import { stateLabel, type DisplayState } from "./state/model";

/** One terminal at a time, or all of them live in a grid. */
export type SessionMode = "single" | "grid";

/** Number, state, provider and title of a pane, shared by the grid tiles
 * and the focused terminal's header. */
export function PaneLabel({
  pane,
  state,
  parked = false,
}: {
  pane: Pane;
  state: DisplayState;
  parked?: boolean;
}) {
  return (
    <>
      <StateMark state={state} />
      {pane.number > 0 && <strong className="number">{pane.number}</strong>}
      <ProviderLogo kind={pane.kind} />
      <span className="grid-title">{pane.title || "shell"}</span>
      {parked && <span className="badge parked-badge">parked</span>}
      {pane.parked && (
        <span className="badge parked-badge" title="Parked in the TUI">
          parked in TUI
        </span>
      )}
      {state !== "idle" && state !== "running" && (
        <span className={`badge state-badge state-${state}`}>
          {stateLabel[state]}
        </span>
      )}
    </>
  );
}
