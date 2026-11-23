// The live grid: one read-only page showing every running terminal at once,
// for monitoring. Shared by main.tsx and the sidebar fixture.
import React, { useLayoutEffect, useRef, useState } from "react";
import type { NumberedPane } from "./terminal/types";
import { gridColumns } from "./terminal/layout";
import { PaneLabel, UnreadMark } from "./Shell";
import { Icon } from "./status";
import { basename } from "./format";
import {
  paneState,
  stateLabel,
  unreadFor,
  type Notification,
} from "./state/model";

export type LiveGridProps = {
  /** The live terminals in this browser's order (parked ones excluded). */
  panes: NumberedPane[];
  notifications: Notification[];
  /** A tile's body: the terminal's live screen, or a placeholder. */
  body: (pane: NumberedPane) => React.ReactNode;
  onOpen: (pane: NumberedPane) => void;
  onMenu: (pane: NumberedPane, x: number, y: number) => void;
  onNew: () => void;
  /** Starts a reorder drag from a tile (mouse only). */
  onDragStart?: (pane: NumberedPane, event: React.PointerEvent) => void;
  /** Extra classes for a tile (the one being dragged). */
  tileClass?: (pane: NumberedPane) => string;
};

/**
 * Every live terminal in one page of columns and rows that fit them all
 * (re-laid out as terminals come and go), with no paging. The grid has no
 * focused tile and sends no input: a tile shows its terminal's state,
 * badges, unread bell, title and folder above its screen, a click anywhere
 * on it opens that terminal alone, a drag reorders it and its context menu
 * offers the terminal's actions. A spare cell offers a new terminal.
 */
export function LiveGrid(props: LiveGridProps) {
  const { panes, notifications } = props;
  const grid = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ width: innerWidth, height: 0 });
  useLayoutEffect(() => {
    if (!grid.current) return;
    const observer = new ResizeObserver((entries) => {
      const { width, height } = entries[0].contentRect;
      setSize({ width, height });
    });
    observer.observe(grid.current);
    return () => observer.disconnect();
  }, []);
  const cols = gridColumns(panes.length, size.width, size.height);
  const rows = Math.max(1, Math.ceil(panes.length / cols));
  return (
    <div
      ref={grid}
      className="pane-grid live-grid"
      aria-label="Terminal grid"
      data-cols={cols}
      data-rows={rows}
      style={{
        gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`,
        gridTemplateRows: `repeat(${rows}, minmax(0, 1fr))`,
      }}
    >
      {Array.from({ length: cols * rows }, (_, i) => {
        const p = panes[i];
        if (!p)
          return i === panes.length ? (
            <button
              type="button"
              className="inactive-pane new-pane"
              key={`new-${i}`}
              onClick={props.onNew}
            >
              <Icon name="plus" size="lg" />
              New terminal
            </button>
          ) : (
            <div className="inactive-pane" key={`empty-${i}`} />
          );
        const state = paneState(p, notifications);
        return (
          <button
            type="button"
            key={p.id}
            className={`grid-pane live-tile ${state === "input_needed" ? "needs-input" : ""} ${props.tileClass?.(p) || ""}`}
            aria-label={`Open terminal ${p.title || "shell"} (${stateLabel[state]})`}
            data-tile-pane={p.id}
            data-drop-pane={p.id}
            data-drop-axis="x"
            data-state={state}
            onPointerDown={(event) => props.onDragStart?.(p, event)}
            onContextMenu={(event) => {
              event.preventDefault();
              props.onMenu(p, event.clientX, event.clientY);
            }}
            onClick={() => props.onOpen(p)}
          >
            <header>
              <PaneLabel pane={p} state={state} />
              <span className="grid-folder">{basename(p.cwd)}</span>
              <UnreadMark count={unreadFor(notifications, p).length} />
            </header>
            {props.body(p)}
          </button>
        );
      })}
    </div>
  );
}
