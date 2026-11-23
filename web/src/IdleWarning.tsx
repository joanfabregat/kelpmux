// The idle warning every KelpMux window shows before the shared connection
// locks (idle.ts), and the hook that reports this window's activity.
import React, { useEffect, useRef, useState } from "react";
import { ACTIVITY_EVENTS, activityReporter, countdown } from "./idle";

export type IdleNotice = { idleFor: string; lockAt: number };

/** Reports keys, clicks, taps and scrolling anywhere in this window through
 * `report` (throttled; immediate while `warned`). `report` null: off. */
export function useActivityReports(
  report: (() => void) | null,
  warned: boolean,
) {
  const latest = useRef({ report, warned });
  latest.current = { report, warned };
  const enabled = !!report;
  useEffect(() => {
    if (!enabled) return;
    const reporter = activityReporter(() => latest.current.report?.());
    const listener = () => reporter(latest.current.warned);
    const options = { capture: true, passive: true };
    for (const type of ACTIVITY_EVENTS)
      window.addEventListener(type, listener, options);
    return () => {
      for (const type of ACTIVITY_EVENTS)
        window.removeEventListener(type, listener, options);
    };
  }, [enabled]);
}

export function IdleWarning({
  notice,
  onStay,
}: {
  notice: IdleNotice;
  onStay: () => void;
}) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(timer);
  }, []);
  return (
    <div className="modal-shade confirmation idle-shade">
      <section
        className="confirm-dialog idle-dialog"
        role="alertdialog"
        aria-labelledby="idle-title"
        aria-describedby="idle-text"
      >
        <h2 id="idle-title">Still there?</h2>
        <p id="idle-text">
          You've been inactive for {notice.idleFor}. KelpMux locks this
          connection in{" "}
          <span className="idle-countdown mono">
            {countdown(notice.lockAt - now)}
          </span>
          . Your shells keep running.
        </p>
        <div className="confirm-buttons">
          <button onClick={onStay}>Stay signed in</button>
        </div>
      </section>
    </div>
  );
}
