// Cmd/Ctrl-click on a file path in the terminal shown alone. Nothing is
// scanned ahead: under the pointer, with Cmd or Ctrl held, the row's text
// at that cell gives a token (path-links.ts), resolved against the
// terminal's folder; a debounced, briefly cached `file_stat` decides
// whether it names something, and only then is it underlined, with a
// pointer cursor, and clickable. A file opens beside the terminal (at its
// :line:col), a folder in the file browser. The handlers sit on the
// terminal's section in the capture phase, ahead of the KelpCore terminal's
// own: a click they take never reaches the terminal (no selection, no
// mouse report to the application), and everything else passes through
// untouched. An http(s) hyperlink (OSC 8) under the pointer is left to the
// terminal, which opens it on any click; a `file://` one is used as the
// path. Rows only: the screen model has no soft-wrap marks, so a path
// wrapped across rows is not joined. Terminal history (scrolled back) has
// no links. Paths outside the server's file root are never links.
import React, { useEffect, useRef, useState } from "react";
import type { Store } from "@kelpcore/web/Terminal.tsx";
import type { FilesClient, FileKind } from "@kelpcore/web/files/api.ts";
import type { NumberedPane } from "@kelpcore/web/types.ts";
import { fileUrlPath, pathLinkAt, type Location } from "./path-links";
import { withinRoot } from "./root";

/** What a link opens: the server's path and its kind. */
export type PathTarget = Location & { kind: FileKind };
type Found = Location & {
  /** Row and columns (end exclusive) to underline. */
  row: number;
  from: number;
  to: number;
};
type Hover = { found: Found; target: PathTarget };
type Stat = { at: number; target: { path: string; kind: FileKind } | null };

/** How long a `file_stat` answer is trusted. */
const cacheMs = 5000;
/** Pointer rest before a path is checked. */
const debounceMs = 80;
const defaultFont = "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace";
let measure: CanvasRenderingContext2D | null = null;
/** The terminal's cell size, measured as KelpCore's renderer does. */
export function cellSize(canvas: HTMLCanvasElement) {
  const style = getComputedStyle(canvas);
  const size = parseInt(style.getPropertyValue("--terminal-size")) || 14;
  const family =
    style.getPropertyValue("--terminal-font").trim() || defaultFont;
  const dpr = window.devicePixelRatio || 1;
  measure ||= document.createElement("canvas").getContext("2d");
  if (!measure) return null;
  measure.font = `${size}px ${family}`;
  return {
    width: Math.max(1, Math.round(measure.measureText("M").width * dpr) / dpr),
    height: Math.ceil(size * 1.4),
  };
}
const modifier = (event: { metaKey: boolean; ctrlKey: boolean }) =>
  event.metaKey || event.ctrlKey;

export function usePathLinks({
  pane,
  store,
  client,
  home,
  onOpen,
}: {
  pane: NumberedPane | undefined;
  store: Store;
  /** Files over this connection; none disables links. */
  client: FilesClient | null;
  home?: string;
  onOpen: (target: PathTarget) => void;
}) {
  const section = useRef<HTMLElement | null>(null);
  const [hover, setHover] = useState<Hover | null>(null);
  const hoverRef = useRef(hover);
  hoverRef.current = hover;
  const cache = useRef(new Map<string, Stat>());
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pointer = useRef<{ clientX: number; clientY: number } | null>(null);
  const taken = useRef(false);
  /** Whether Cmd or Ctrl was held at the last pointer or key event. */
  const holding = useRef(false);
  const latest = useRef({ pane, client, home, onOpen });
  latest.current = { pane, client, home, onOpen };
  // A new connection or terminal starts over.
  useEffect(() => {
    cache.current.clear();
    setHover(null);
  }, [client, pane?.id]);

  /** The path under a point, before any existence check. Only paths
   * inside the file root are candidates: nothing is asked about the
   * others, which the server refuses. */
  function find(point: { clientX: number; clientY: number }): Found | null {
    const found = locate(point);
    return found && withinRoot(found.path, latest.current.client?.limits.root)
      ? found
      : null;
  }
  function locate(point: { clientX: number; clientY: number }): Found | null {
    const { pane, client, home } = latest.current;
    const root = section.current;
    if (!pane || !client || !root) return null;
    if (root.querySelector(".history-indicator")) return null;
    const canvas = root.querySelector<HTMLCanvasElement>(".terminal canvas");
    const screen = store.screens.get(pane.id)?.screen;
    if (!canvas || !screen) return null;
    const size = cellSize(canvas);
    if (!size) return null;
    const r = canvas.getBoundingClientRect();
    const x = Math.floor((point.clientX - r.left) / size.width);
    const y = Math.floor((point.clientY - r.top) / size.height);
    if (x < 0 || y < 0 || x >= screen.cols || y >= screen.rows) return null;
    const cells = screen.cells.slice(y * screen.cols, (y + 1) * screen.cols);
    // An OSC 8 hyperlink: http(s) is the terminal's; file:// is a path.
    const uri = cells[x]?.hyperlink;
    if (uri) {
      const path = fileUrlPath(uri);
      if (!path) return null;
      let from = x,
        to = x + 1;
      while (from > 0 && cells[from - 1]?.hyperlink === uri) from--;
      while (to < cells.length && cells[to]?.hyperlink === uri) to++;
      return { path, row: y, from, to };
    }
    // The row as text, and the string index each column starts at (a
    // wide character's second column maps to its first).
    let line = "";
    const index: number[] = [];
    cells.forEach((cell, column) => {
      if (cell.width === 0 && column > 0) index.push(index[column - 1]);
      else {
        index.push(line.length);
        line += cell.text || " ";
      }
    });
    const link = pathLinkAt(line, index[x], pane.cwd, home);
    if (!link) return null;
    const from = index.findIndex((i) => i >= link.start);
    let to = from;
    while (to < index.length && index[to] < link.end) to++;
    return {
      path: link.path,
      line: link.line,
      col: link.col,
      row: y,
      from,
      to,
    };
  }
  const same = (a: Found | null, b: Found | null) =>
    !!a && !!b && a.path === b.path && a.row === b.row && a.from === b.from;
  const fresh = (path: string) => {
    const stat = cache.current.get(path);
    return stat && performance.now() - stat.at < cacheMs ? stat : undefined;
  };
  /** Hover state for a point with the modifier held or not. */
  function update(point: { clientX: number; clientY: number }, held: boolean) {
    holding.current = held;
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    const found = held ? find(point) : null;
    if (!found) {
      if (hoverRef.current) setHover(null);
      return;
    }
    const known = fresh(found.path);
    if (known) {
      // Moving within the link shown re-renders nothing.
      if (known.target && same(hoverRef.current?.found ?? null, found)) return;
      setHover(
        known.target ? { found, target: { ...found, ...known.target } } : null,
      );
      return;
    }
    if (hoverRef.current && !same(hoverRef.current.found, found))
      setHover(null);
    timer.current = setTimeout(() => {
      timer.current = null;
      const client = latest.current.client;
      if (!client) return;
      const done = (target: Stat["target"]) => {
        cache.current.set(found.path, { at: performance.now(), target });
        if (cache.current.size > 256)
          cache.current.delete(cache.current.keys().next().value!);
        // Still over the same path, modifier still held?
        if (
          pointer.current &&
          holding.current &&
          same(find(pointer.current), found)
        )
          update(pointer.current, true);
      };
      client.stat(found.path).then(
        ({ path, entry }) =>
          done(entry.kind === "other" ? null : { path, kind: entry.kind }),
        () => done(null),
      );
    }, debounceMs);
  }
  const updater = useRef(update);
  updater.current = update;
  useEffect(() => {
    // Pressing or releasing the modifier over a still pointer.
    const key = (event: KeyboardEvent) => {
      if (!["Control", "Meta"].includes(event.key) || !pointer.current) return;
      updater.current(pointer.current, modifier(event));
    };
    const blur = () => updater.current({ clientX: -1, clientY: -1 }, false);
    window.addEventListener("keydown", key, true);
    window.addEventListener("keyup", key, true);
    window.addEventListener("blur", blur);
    return () => {
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("keyup", key, true);
      window.removeEventListener("blur", blur);
      if (timer.current) clearTimeout(timer.current);
    };
  }, []);

  const props = {
    ref: (element: HTMLElement | null) => {
      section.current = element;
    },
    onPointerMoveCapture: (event: React.PointerEvent) => {
      if (event.pointerType === "touch") return;
      const onCanvas = (event.target as Element).tagName === "CANVAS";
      pointer.current = onCanvas
        ? { clientX: event.clientX, clientY: event.clientY }
        : null;
      if (!onCanvas) {
        if (hoverRef.current) setHover(null);
        return;
      }
      update(event, modifier(event) && !event.buttons);
    },
    onPointerLeave: () => {
      pointer.current = null;
      update({ clientX: -1, clientY: -1 }, false);
    },
    onPointerDownCapture: (event: React.PointerEvent) => {
      taken.current = false;
      if (
        event.pointerType === "touch" ||
        event.button !== 0 ||
        !modifier(event) ||
        (event.target as Element).tagName !== "CANVAS"
      )
        return;
      const found = find(event);
      const known = found && fresh(found.path);
      if (!found || !known?.target) {
        // Not checked yet: check now, so the next click finds it.
        if (found && !known) update(event, true);
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      taken.current = true;
      setHover(null);
      latest.current.onOpen({ ...found, ...known.target });
    },
    onPointerUpCapture: (event: React.PointerEvent) => {
      if (!taken.current) return;
      event.preventDefault();
      event.stopPropagation();
    },
    onClickCapture: (event: React.MouseEvent) => {
      if (!taken.current) return;
      taken.current = false;
      event.stopPropagation();
    },
    onContextMenuCapture: (event: React.MouseEvent) => {
      // macOS turns Ctrl-click into a context menu click.
      if (taken.current) event.preventDefault();
    },
  };
  let overlay: React.ReactNode = null;
  const canvas =
    section.current?.querySelector<HTMLCanvasElement>(".terminal canvas");
  if (hover && canvas && section.current) {
    const size = cellSize(canvas);
    const r = canvas.getBoundingClientRect();
    const s = section.current.getBoundingClientRect();
    if (size)
      overlay = (
        <span
          className="path-link-underline"
          aria-hidden="true"
          data-path={hover.target.path}
          style={{
            left: r.left - s.left + hover.found.from * size.width,
            top: r.top - s.top + (hover.found.row + 1) * size.height - 2,
            width: (hover.found.to - hover.found.from) * size.width,
          }}
        />
      );
  }
  return { props, overlay, className: hover ? "path-link-hover" : "" };
}
