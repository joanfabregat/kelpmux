// Paths from the file browser into terminals: the drag data a browser entry
// carries, and typing shell-quoted paths into a pane as uploads paste theirs
// (no Enter; bracketed when the program asked for it). The quoting and the
// refusal of control characters are KelpCore's (files/paths.ts).
import type React from "react";
import type { Store } from "@kelpcore/web/Terminal.tsx";

/** The drag data type of entries dragged out of the file browser: the
 * absolute paths, one per line. Other applications get `text/plain`. */
export const PATH_MIME = "application/x-kelpmux-paths";

/** Whether a drag carries paths from the file browser. */
export function dragHasPaths(event: React.DragEvent | DragEvent): boolean {
  return Array.from(event.dataTransfer?.types || []).includes(PATH_MIME);
}
/** The paths a drop from the file browser carries. */
export function droppedPaths(data: DataTransfer | null): string[] {
  return (data?.getData(PATH_MIME) || "")
    .split("\n")
    .filter((path) => path.startsWith("/"));
}
/** Starts dragging `paths` out of the file browser. */
export function startPathDrag(event: React.DragEvent, paths: string[]) {
  event.dataTransfer.setData(PATH_MIME, paths.join("\n"));
  event.dataTransfer.setData("text/plain", paths.join("\n"));
  event.dataTransfer.effectAllowed = "copyMove";
}

/** Types `text` into a pane as a paste: through its mounted terminal when
 * there is one (taking the pane at its size), otherwise by taking the pane
 * and pasting directly. False when there is no connection. */
export function typeIntoPane(store: Store, pane: number, text: string) {
  const session = store.session;
  if (!session || store.closing?.has(pane)) return false;
  const detail: any = { pane, bytes: text, paste: true };
  store.events.dispatchEvent(new CustomEvent("terminal-input", { detail }));
  if (!detail.handled) {
    session.control(pane);
    session.request("paste", {
      pane,
      bytes: Array.from(new TextEncoder().encode(text)),
    });
  }
  return true;
}
