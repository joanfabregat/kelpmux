// The editor docked to the right of the terminal shown alone: one per
// window, showing one open file through the same FileEditor (and document
// model) as the main area, with a splitter on its left edge whose drag
// resizes it (saved in this browser like the sidebar's width). It stays
// mounted, hidden, while the grid, the file browser or the phone layout
// shows, so unsaved edits survive and it reappears with the terminal.
import React from "react";
import { FileEditor } from "./FileEditor";
import type { Documents, OpenDoc } from "./documents";
import { SIDE_PANEL_MAX, SIDE_PANEL_MIN } from "../layout-store";

/** Room the terminal keeps beside the panel, in CSS pixels. */
const TERMINAL_MIN = 240;

export function SidePanel({
  doc,
  documents,
  home,
  maxRead,
  hidden,
  width,
  onWidth,
  onClose,
  onReveal,
  onMove,
  jump,
}: {
  doc: OpenDoc;
  documents: Documents;
  home?: string;
  maxRead: number;
  hidden: boolean;
  width: number;
  onWidth: (width: number) => void;
  onClose: () => void;
  onReveal: () => void;
  onMove: () => void;
  jump?: { line: number; col?: number; seq: number };
}) {
  function resizeStart(event: React.PointerEvent<HTMLDivElement>) {
    if (event.button !== 0) return;
    event.preventDefault();
    const panel = event.currentTarget.parentElement!;
    const room =
      (panel.parentElement?.getBoundingClientRect().width || innerWidth) -
      TERMINAL_MIN;
    const x = event.clientX,
      initial = panel.getBoundingClientRect().width;
    const limit = (value: number) =>
      Math.round(
        Math.max(SIDE_PANEL_MIN, Math.min(SIDE_PANEL_MAX, room, value)),
      );
    const move = (e: PointerEvent) => onWidth(limit(initial + x - e.clientX));
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      document.body.classList.remove("side-resizing");
    };
    document.body.classList.add("side-resizing");
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }
  return (
    <aside
      className="side-panel"
      hidden={hidden}
      aria-label="Side panel"
      style={{ "--side-width": `${width}px` } as React.CSSProperties}
      onClick={(event) => event.stopPropagation()}
    >
      <div
        className="side-divider"
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize the side panel"
        title="Drag to resize"
        onPointerDown={resizeStart}
      >
        <span aria-hidden="true" />
      </div>
      <FileEditor
        key={doc.id}
        doc={doc}
        documents={documents}
        home={home}
        maxRead={maxRead}
        onClose={onClose}
        onReveal={onReveal}
        side
        onMove={onMove}
        jump={jump}
      />
    </aside>
  );
}
