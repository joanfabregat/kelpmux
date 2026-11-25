import React, { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Renderer, type Selection } from "./renderer";
import type { NumberedPane, Snapshot, SearchMatch } from "./types";
import type { Session } from "./connection";
import { encodeKey, encodeText } from "./keyboard";
import { mouseButton, mouseBytes } from "./mouse";
import { WheelAccumulator } from "./wheel";
import { PaneFind } from "./PaneFind";
import { modeKey, modeSelection, selectMode, type KeyMode } from "./key-mode";
import { stickyText, withCtrl } from "./touch-keys";
export type Store = {
  workspace?: string;
  screens: Map<number, Snapshot>;
  events: EventTarget;
  leases: Map<number, number>;
  closing?: Set<number>;
  session: Session | null;
};
/** Delay after the last tile size change before the PTY is resized, so the
 * first resize uses the tile's final size rather than an intermediate one. */
const tileSettleMs = 150;
/** A touch held this long without moving starts a selection. */
const longPressMs = 450;
/** A touch moving this far (CSS pixels) before the long press scrolls. */
const touchSlop = 8;
/**
 * A live tile of the Terminals grid: the terminal's screen drawn with the
 * same renderer as the interactive view, with the PTY sized to the tile
 * (columns and rows from the tile's measured size and the tile font's
 * metrics). It never sends keyboard, mouse or paste input and never takes
 * focus. The size is sent once the layout has settled (fonts loaded and the
 * size unchanged for a moment), again when the tile is resized, and when
 * this window becomes active again. Sizing claims the terminal's control
 * lease, so among several windows showing the same terminal the most
 * recently active one decides its size; a background window never resizes.
 * The lease is claimed for the size only: it gives the tile no input focus,
 * and no keyboard action addresses a grid tile.
 */
export function TerminalTile({
  pane,
  store,
}: {
  pane: NumberedPane;
  store: Store;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  useLayoutEffect(() => {
    if (!canvas.current) return;
    const renderer = new Renderer(canvas.current);
    let frame = 0,
      alive = true,
      sent = "",
      settle: ReturnType<typeof setTimeout> | null = null;
    const draw = () => {
      const screen = store.screens.get(pane.id)?.screen;
      if (screen) renderer.draw(screen);
      else renderer.clear();
    };
    const changed = () => {
      if (!frame)
        frame = requestAnimationFrame(() => {
          frame = 0;
          draw();
        });
    };
    const sync = () => {
      settle = null;
      const session = store.session;
      if (
        !alive ||
        !session ||
        session.stopped ||
        document.hidden ||
        !document.hasFocus() ||
        store.closing?.has(pane.id)
      )
        return;
      const rect = canvas.current?.getBoundingClientRect();
      if (!rect || rect.width < 1 || rect.height < 1) return;
      const size = renderer.geometry();
      const key = `${size.cols}x${size.rows}`;
      if (!session.controlled.has(pane.id)) {
        // Claim, then resize: both travel in order on the same channel.
        session.control(pane.id, size);
        sent = key;
      } else if (key !== sent) {
        session.request("resize", { pane: pane.id, ...size });
        sent = key;
      }
    };
    const schedule = (delay = tileSettleMs) => {
      if (settle) clearTimeout(settle);
      settle = setTimeout(sync, delay);
    };
    const resized = () => {
      changed();
      schedule();
    };
    // Becoming the active window again makes this view authoritative.
    const activated = () => {
      sent = "";
      schedule();
    };
    store.events.addEventListener("frame", changed);
    const observer = new ResizeObserver(resized);
    observer.observe(canvas.current);
    const fontsChanged = () => {
      renderer.glyphs.clear();
      renderer.metrics();
      resized();
    };
    document.fonts.addEventListener("loadingdone", fontsChanged);
    window.addEventListener("focus", activated);
    document.addEventListener("visibilitychange", activated);
    void document.fonts.ready.then(() => alive && schedule());
    draw();
    return () => {
      alive = false;
      cancelAnimationFrame(frame);
      if (settle) clearTimeout(settle);
      observer.disconnect();
      store.events.removeEventListener("frame", changed);
      document.fonts.removeEventListener("loadingdone", fontsChanged);
      window.removeEventListener("focus", activated);
      document.removeEventListener("visibilitychange", activated);
      renderer.dispose();
    };
  }, [pane.id, store]);
  return (
    <div className="terminal terminal-tile" aria-hidden="true">
      <canvas ref={canvas} tabIndex={-1} />
    </div>
  );
}
export function Terminal({
  pane,
  store,
  active,
  layout,
  onActivate,
  onError,
  find = false,
  findQuery = "",
  onCloseFind = () => {},
  touchKeyboard = false,
  onInputFocus,
  stickyCtrl = false,
  onCtrlUsed,
  interceptText,
  onFiles,
}: {
  pane: NumberedPane;
  store: Store;
  active: boolean;
  layout: "grid" | "tabs";
  onActivate: () => void;
  onError: (e: any) => void;
  find?: boolean;
  /** Initial query when find opens. */
  findQuery?: string;
  onCloseFind?: () => void;
  /** Touch devices: showing, tapping or activating the terminal never
   * focuses its input, so the on-screen keyboard appears only when the
   * application asks (a `terminal-keyboard` store event). */
  touchKeyboard?: boolean;
  /** The terminal's input gained (true) or lost (false) focus. */
  onInputFocus?: (focused: boolean) => void;
  /** A sticky Ctrl modifier applies to the next typed key. */
  stickyCtrl?: boolean;
  /** The sticky Ctrl was applied to a key. */
  onCtrlUsed?: () => void;
  /** Sees text typed without a key event first; true consumes it. */
  interceptText?: (text: string) => boolean;
  /** Files pasted from the clipboard (an image, a screenshot) without
   * text: the application may upload them instead of pasting text. */
  onFiles?: (files: File[]) => void;
}) {
  const touchKeyboardRef = useRef(touchKeyboard);
  touchKeyboardRef.current = touchKeyboard;
  /** A touch on the canvas: a tap, a one-finger scroll or a long-press
   * selection. */
  const touch = useRef<{
    id: number;
    x: number;
    y: number;
    lastY: number;
    mode: "tap" | "scroll" | "select";
    timer: ReturnType<typeof setTimeout> | null;
    keyboard: boolean;
  } | null>(null);
  const canvas = useRef<HTMLCanvasElement>(null),
    input = useRef<HTMLTextAreaElement>(null),
    renderer = useRef<Renderer | null>(null),
    selection = useRef<Selection>(null);
  const [history, setHistory] = useState<string[] | null>(null),
    historyRef = useRef<string[] | null>(null),
    offset = useRef(0);
  const historyCache = useRef(new Map<number, string>());
  const historyBusy = useRef(false);
  const oldestHistory = useRef<number | null>(null);
  const wheelRows = useRef(new WheelAccumulator());
  const wheelFrame = useRef(0);
  const wheelIntent = useRef<{ app: boolean; event: WheelEvent } | null>(null);
  const mounted = useRef(true);
  const preview = useRef<SearchMatch | null>(null);
  const previewOffset = useRef(0);
  const [searchPreview, setSearchPreview] = useState(false);
  const [clipboardOffer, setClipboardOffer] = useState<string | null>(null);
  const applicationCopy = useRef<string | null>(null);
  /** Keyboard scroll or select mode (Ctrl-B [ or Ctrl-B Space). */
  const keyMode = useRef<KeyMode | null>(null);
  const [modeKind, setModeKind] = useState<KeyMode["kind"] | null>(null);
  function setMode(mode: KeyMode | null) {
    keyMode.current = mode;
    setModeKind(mode?.kind ?? null);
    selection.current = modeSelection(mode);
    draw();
  }
  useEffect(() => {
    // The application asks this terminal to enter a keyboard mode.
    const enter = (event: Event) => {
      const { pane: id, kind } = (event as CustomEvent).detail || {};
      if (id !== pane.id) return;
      const rows = get()?.rows || 24;
      if (kind === "scroll") setMode({ kind: "scroll" });
      else if (kind === "select") setMode(selectMode(rows));
      input.current?.focus({ preventScroll: true });
    };
    store.events.addEventListener("terminal-key-mode", enter);
    return () => store.events.removeEventListener("terminal-key-mode", enter);
  }, [pane.id, store]);
  useEffect(() => {
    // The application shows or hides the on-screen keyboard, or sends key
    // bar input. Dispatched synchronously from a tap, so focus() still runs
    // inside the user gesture iOS requires to show the keyboard.
    // `virtual: false` focuses the input for a hardware keyboard without
    // the on-screen keyboard (`inputmode="none"`); `key` is a key typed on
    // that keyboard before the input had focus, delivered to the terminal
    // as if the input had received it.
    const keyboard = (event: Event) => {
      const {
        pane: id,
        show,
        virtual = true,
        key: first,
      } = (event as CustomEvent).detail || {};
      if (id !== pane.id) return;
      const field = input.current;
      if (show) {
        control();
        if (!field) return;
        if (virtual) field.removeAttribute("inputmode");
        else field.setAttribute("inputmode", "none");
        field.focus({ preventScroll: true });
        if (!first) return;
        const taken = !field.dispatchEvent(
          new KeyboardEvent("keydown", {
            ...first,
            bubbles: true,
            cancelable: true,
          }),
        );
        // A key the keydown handler left alone is text, which a browser
        // would insert into the focused input: deliver it the same way.
        if (
          !taken &&
          !first.ctrlKey &&
          !first.metaKey &&
          !first.altKey &&
          typeof first.key === "string" &&
          Array.from(first.key).length === 1
        ) {
          field.value = first.key;
          field.dispatchEvent(
            new InputEvent("input", {
              bubbles: true,
              data: first.key,
              inputType: "insertText",
            }),
          );
        }
      } else field?.blur();
    };
    const typed = (event: Event) => {
      const detail = (event as CustomEvent).detail || {};
      const { pane: id, bytes, paste } = detail;
      if (id !== pane.id || typeof bytes !== "string" || !bytes) return;
      // Tells the sender that a mounted terminal took the input.
      detail.handled = true;
      send(bytes, !!paste);
    };
    store.events.addEventListener("terminal-keyboard", keyboard);
    store.events.addEventListener("terminal-input", typed);
    return () => {
      store.events.removeEventListener("terminal-keyboard", keyboard);
      store.events.removeEventListener("terminal-input", typed);
    };
  }, [pane.id, store]);
  /** Applies a key in keyboard mode; never sends it to the program. */
  function modeInput(event: React.KeyboardEvent) {
    const mode = keyMode.current;
    const screen = get();
    if (!mode || !screen) return;
    if (["Shift", "Control", "Alt", "Meta"].includes(event.key)) return;
    event.preventDefault();
    const result = modeKey(mode, event, screen.cols, screen.rows);
    if (result.copy) {
      void copy();
      setMode(null);
      return;
    }
    setMode(result.mode);
    if (result.live) returnLive();
    else if (result.top) void scroll(-100000, true);
    else if (result.scroll) void scroll(result.scroll, true);
  }
  useEffect(() => {
    const received = (event: Event) => {
      const { pane: id, text } = (event as CustomEvent).detail;
      if (id !== pane.id || typeof text !== "string") return;
      applicationCopy.current = text;
      setClipboardOffer(text);
      // Async OSC 52 delivery can lack the user activation required by Safari.
      // Keep an explicit copy action available if this write is denied.
      if (document.hasFocus() && !document.hidden) void copy(text);
    };
    store.events.addEventListener("clipboard", received);
    return () => store.events.removeEventListener("clipboard", received);
  }, [pane.id, store]);
  useEffect(() => {
    if (!find) {
      if (renderer.current) renderer.current.historyMatch = null;
      draw();
    }
  }, [find]);
  const lastSize = useRef("");
  const lastTheme = useRef("");
  const resizeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pendingSize = useRef<{ cols: number; rows: number } | null>(null);
  const lastResize = useRef(0);
  const sentKeys = useRef(new Set<string>());
  const pendingLink = useRef<{
    uri: string;
    start: { x: number; y: number };
    clientX: number;
    clientY: number;
  } | null>(null);
  const pressedMouse = useRef(false);
  const wheelHandler = useRef<(event: WheelEvent) => void>(() => {});
  const get = () => store.screens.get(pane.id)?.screen;
  const reportsMouse = () => {
    const modes = get()?.modes;
    return (
      modes && (modes.mouse_click || modes.mouse_drag || modes.mouse_motion)
    );
  };
  const owned = () =>
    !store.closing?.has(pane.id) &&
    !!store.session &&
    !document.hidden &&
    document.hasFocus() &&
    store.session.controlled.has(pane.id);
  function control() {
    if (store.closing?.has(pane.id)) return;
    if (!owned()) lastTheme.current = "";
    store.session?.control(pane.id, renderer.current?.geometry());
    reportTheme();
  }
  function draw() {
    const screen = get();
    if (screen)
      renderer.current?.draw(
        screen,
        selection.current,
        historyRef.current || undefined,
      );
    else renderer.current?.clear();
  }
  function reportTheme() {
    if (!owned() || !canvas.current) return;
    if (store.session?.capabilities.has("controller-theme")) {
      const style = getComputedStyle(canvas.current!);
      const foreground = style.getPropertyValue("--terminal-foreground").trim();
      const background = style.getPropertyValue("--background").trim();
      const theme = foreground + background;
      if (
        theme !== lastTheme.current &&
        /^#[\da-f]{6}$/i.test(foreground) &&
        /^#[\da-f]{6}$/i.test(background)
      ) {
        const rgb = (hex: string) =>
          [1, 3, 5].map((offset) =>
            parseInt(hex.slice(offset, offset + 2), 16),
          );
        store.session.request("theme", {
          pane: pane.id,
          foreground: rgb(foreground),
          background: rgb(background),
        });
        lastTheme.current = theme;
      }
    }
  }
  function resize() {
    if (!owned() || !renderer.current) return;
    reportTheme();
    const size = renderer.current.geometry();
    const key = `${size.cols}x${size.rows}`;
    if (key === lastSize.current) {
      pendingSize.current = null;
      if (resizeTimer.current) clearTimeout(resizeTimer.current);
      resizeTimer.current = null;
      return;
    }
    if (key !== lastSize.current) {
      pendingSize.current = size;
      const flush = () => {
        resizeTimer.current = null;
        const next = pendingSize.current;
        pendingSize.current = null;
        if (!next || !owned()) return;
        lastSize.current = `${next.cols}x${next.rows}`;
        lastResize.current = performance.now();
        store.session?.request("resize", { pane: pane.id, ...next });
      };
      if (!resizeTimer.current) {
        const delay = Math.max(
          0,
          32 - (performance.now() - lastResize.current),
        );
        if (delay) resizeTimer.current = setTimeout(flush, delay);
        else flush();
      }
    }
  }
  useLayoutEffect(() => {
    if (!canvas.current) return;
    renderer.current = new Renderer(canvas.current);
    renderer.current.historyLayout = (lines, cols, rows) =>
      store.session!.historyScreen(lines, cols, rows);
    mounted.current = true;
    let frame = 0;
    const changed = () => {
      if (!frame)
        frame = requestAnimationFrame(() => {
          frame = 0;
          resize();
          draw();
        });
    };
    store.events.addEventListener("frame", changed);
    const fontsChanged = () => {
      renderer.current?.glyphs.clear();
      renderer.current?.metrics();
      changed();
    };
    document.fonts.addEventListener("loadingdone", fontsChanged);
    const observer = new ResizeObserver(changed);
    observer.observe(canvas.current);
    let density: MediaQueryList;
    const densityChanged = () => {
      density?.removeEventListener("change", densityChanged);
      density = matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
      density.addEventListener("change", densityChanged);
      changed();
    };
    densityChanged();
    const wheel = (event: WheelEvent) => {
      event.preventDefault();
      wheelHandler.current(event);
    };
    canvas.current.addEventListener("wheel", wheel, { passive: false });
    draw();
    changed();
    return () => {
      cancelAnimationFrame(frame);
      mounted.current = false;
      if (touch.current?.timer) clearTimeout(touch.current.timer);
      touch.current = null;
      cancelAnimationFrame(wheelFrame.current);
      if (resizeTimer.current) clearTimeout(resizeTimer.current);
      observer.disconnect();
      density.removeEventListener("change", densityChanged);
      canvas.current?.removeEventListener("wheel", wheel);
      store.events.removeEventListener("frame", changed);
      document.fonts.removeEventListener("loadingdone", fontsChanged);
      renderer.current?.dispose();
      renderer.current = null;
    };
  }, [pane.id]);
  useLayoutEffect(() => {
    // A foreground layout change is an explicit request to size its panes.
    // Passive redraw and background views still never claim input ownership.
    if (
      document.hidden ||
      !document.hasFocus() ||
      !store.session ||
      !(active || layout === "grid")
    )
      return;
    control();
    resize();
    draw();
  }, [layout, active, pane.id]);
  function displayHistory(lines: string[] | null) {
    historyRef.current = lines;
    setHistory(lines);
    draw();
  }
  function returnLive() {
    preview.current = null;
    previewOffset.current = 0;
    setSearchPreview(false);
    if (renderer.current) renderer.current.historyMatch = null;
    offset.current = 0;
    oldestHistory.current = null;
    historyCache.current.clear();
    wheelRows.current.reset();
    displayHistory(null);
  }
  useEffect(() => {
    const focus = () => {
      if (
        !active ||
        document.hidden ||
        !document.hasFocus() ||
        document.querySelector(
          '[role="dialog"],[role="alertdialog"],[role="search"]',
        )
      )
        return;
      control();
      if (!touchKeyboardRef.current)
        input.current?.focus({ preventScroll: true });
    };
    focus();
    window.addEventListener("focus", focus);
    store.events.addEventListener("terminal-focus", focus);
    return () => {
      window.removeEventListener("focus", focus);
      store.events.removeEventListener("terminal-focus", focus);
    };
  }, [active]);
  function send(bytes: string, paste = false) {
    if (store.closing?.has(pane.id)) return;
    control();
    const data = new TextEncoder().encode(bytes);
    if (data.length > 65536) {
      onError(Error("Input is limited to 64 KiB per paste. Nothing was sent."));
      return;
    }
    store.session?.request(paste ? "paste" : "input", {
      pane: pane.id,
      bytes: Array.from(data),
    });
    if (offset.current > 0) {
      returnLive();
    }
    selection.current = null;
  }
  function key(event: React.KeyboardEvent, release = false) {
    if (event.nativeEvent.isComposing) return;
    if (keyMode.current) {
      if (!release) modeInput(event);
      return;
    }
    const id = event.code || event.key;
    if (release) {
      if (!sentKeys.current.delete(id)) return;
      const encoded = encodeKey(event, get()?.modes || {}, true);
      if (encoded && owned()) send(encoded);
      return;
    }
    if (
      stickyCtrl &&
      !["Shift", "Control", "Alt", "Meta"].includes(event.key)
    ) {
      // The key bar's sticky Ctrl applies to this key from the on-screen
      // keyboard.
      // A key without a usable value (an IME's "Unidentified") leaves the
      // modifier armed for the text that follows it.
      const bytes = encodeKey(
        withCtrl({
          key: event.key,
          shiftKey: event.shiftKey,
          altKey: event.altKey,
          ctrlKey: event.ctrlKey,
          metaKey: event.metaKey,
        }),
        get()?.modes || {},
      );
      if (bytes !== null) {
        onCtrlUsed?.();
        event.preventDefault();
        send(bytes);
        return;
      }
    }
    if (
      (event.ctrlKey || event.metaKey) &&
      event.key.toLowerCase() === "c" &&
      (selection.current || (event.metaKey && applicationCopy.current !== null))
    ) {
      // Let the browser produce a trusted Copy event. Its synchronous data path
      // works without an asynchronous clipboard permission or a round trip.
      sentKeys.current.delete(id);
      return;
    }
    if (event.metaKey && event.key.toLowerCase() === "c") return;
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "v")
      return;
    const bytes = encodeKey(event, get()?.modes || {});
    if (bytes !== null) {
      event.preventDefault();
      control();
      sentKeys.current.add(id);
      send(bytes);
    }
  }
  function mouse(event: React.PointerEvent, kind: string) {
    if (store.closing?.has(pane.id)) return;
    const screen = get();
    if (!screen || !renderer.current) return;
    const position = renderer.current.point(event, screen);
    control();
    // Encode locally so retained TUI servers also support legacy ncurses mouse
    // mode. Those servers reject legacy events on the structured Mouse route.
    const bytes = mouseBytes(screen, {
      position,
      kind,
      button: mouseButton(event.button, event.buttons, kind),
      shift: event.shiftKey,
      alt: event.altKey,
      ctrl: event.ctrlKey,
    });
    if (bytes.length) store.session?.request("input", { pane: pane.id, bytes });
  }
  function point(event: React.PointerEvent) {
    const screen = get();
    return screen && renderer.current
      ? renderer.current.point(event, screen)
      : { x: 0, y: 0 };
  }
  /** Text from the on-screen keyboard or an IME: the application may take
   * it (a prefix key), and an armed sticky Ctrl applies to its first
   * character. */
  function typed(text: string) {
    if (interceptText?.(text)) return;
    const modes = get()?.modes || {};
    if (stickyCtrl) {
      onCtrlUsed?.();
      send(stickyText(text, modes));
      return;
    }
    send(encodeText(text, modes));
  }
  function capture(pointer: number) {
    // A synthetic or already-ended pointer cannot be captured.
    try {
      canvas.current?.setPointerCapture(pointer);
    } catch {}
  }
  /** The word under a point (cells up to whitespace on that row). */
  function wordAt(p: { x: number; y: number }): Selection {
    const screen = get();
    if (!screen) return null;
    const cell = (x: number) => screen.cells[p.y * screen.cols + x]?.text || "";
    let left = p.x,
      right = p.x;
    while (left > 0 && /\S/.test(cell(left - 1))) left--;
    while (right < screen.cols - 1 && /\S/.test(cell(right + 1))) right++;
    return { start: { x: left, y: p.y }, end: { x: right, y: p.y } };
  }
  /** Touch: a tap activates (and claims) the terminal without showing the
   * keyboard, a one-finger drag scrolls history like the wheel, and a long
   * press selects the word under the finger; dragging then extends the
   * selection, which is copied on release. */
  function touchDown(event: React.PointerEvent) {
    event.preventDefault();
    if (touch.current) return; // A second finger is ignored.
    capture(event.pointerId);
    onActivate();
    control();
    // Typing stays available when the keyboard was already up.
    const keyboard = document.activeElement === input.current;
    if (!touchKeyboard) input.current?.focus({ preventScroll: true });
    const state = {
      id: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      lastY: event.clientY,
      mode: "tap" as "tap" | "scroll" | "select",
      timer: null as ReturnType<typeof setTimeout> | null,
      keyboard,
    };
    state.timer = setTimeout(() => {
      state.timer = null;
      if (touch.current !== state || state.mode !== "tap" || history) return;
      const screen = get();
      if (!screen || !renderer.current) return;
      state.mode = "select";
      selection.current = wordAt(
        renderer.current.point({ clientX: state.x, clientY: state.y }, screen),
      );
      draw();
    }, longPressMs);
    touch.current = state;
  }
  function touchMove(event: React.PointerEvent) {
    const state = touch.current;
    if (!state || state.id !== event.pointerId) return;
    event.preventDefault();
    if (
      state.mode === "tap" &&
      Math.hypot(event.clientX - state.x, event.clientY - state.y) >= touchSlop
    ) {
      state.mode = "scroll";
      if (state.timer) clearTimeout(state.timer);
      state.timer = null;
    }
    if (state.mode === "scroll") {
      // Finger down shows older lines, as the wheel turned up does.
      const deltaY = state.lastY - event.clientY;
      state.lastY = event.clientY;
      if (deltaY)
        wheelHandler.current({
          deltaY,
          deltaMode: 0,
          shiftKey: false,
          altKey: false,
          ctrlKey: false,
          clientX: event.clientX,
          clientY: event.clientY,
        } as WheelEvent);
    } else if (state.mode === "select" && selection.current) {
      selection.current.end = point(event);
      draw();
    }
  }
  function touchEnd(event: React.PointerEvent, canceled = false) {
    const state = touch.current;
    if (!state || state.id !== event.pointerId) return;
    touch.current = null;
    if (state.timer) clearTimeout(state.timer);
    if (canceled) return;
    if (state.mode === "select") {
      void copy();
      return;
    }
    if (state.mode !== "tap") return;
    if (selection.current) {
      selection.current = null;
      draw();
    }
    const uri = linkAt(event);
    if (uri) {
      window.open(uri, "_blank", "noopener,noreferrer");
      return;
    }
    if (reportsMouse() && owned() && !history) {
      mouse(event, "Press");
      mouse(event, "Release");
    }
    // A tap must not take the keyboard away while the user is typing.
    if (state.keyboard) input.current?.focus({ preventScroll: true });
  }
  function down(event: React.PointerEvent) {
    if (event.pointerType === "touch") return touchDown(event);
    pressedMouse.current = false;
    pendingLink.current = null;
    const uri = linkAt(event);
    if (uri && event.button === 0 && !event.shiftKey) {
      event.preventDefault();
      canvas.current?.setPointerCapture(event.pointerId);
      pendingLink.current = {
        uri,
        start: point(event),
        clientX: event.clientX,
        clientY: event.clientY,
      };
      return;
    }
    // The canvas default pointer action otherwise blurs the hidden input after
    // focus(), so Copy/Cmd+C is dispatched to the page instead of the terminal.
    if (event.button === 0) event.preventDefault();
    onActivate();
    control();
    input.current?.focus({ preventScroll: true });
    const screen = get();
    if (!screen) return;
    if (reportsMouse() && !event.shiftKey && owned() && !history) {
      event.preventDefault();
      canvas.current?.setPointerCapture(event.pointerId);
      pressedMouse.current = true;
      mouse(event, "Press");
      return;
    }
    if (event.button !== 0) return;
    canvas.current?.setPointerCapture(event.pointerId);
    const p = point(event);
    selection.current = { start: p, end: p };
    draw();
  }
  function move(event: React.PointerEvent) {
    if (event.pointerType === "touch") return touchMove(event);
    const screen = get();
    if (!screen) return;
    const uri = !event.shiftKey ? linkAt(event) : undefined;
    if (canvas.current) {
      canvas.current.style.cursor = uri
        ? "pointer"
        : reportsMouse() && !event.shiftKey
          ? "default"
          : "text";
      canvas.current.title = uri || "";
    }
    if (renderer.current && renderer.current.hoverLink !== uri) {
      renderer.current.hoverLink = uri;
      draw();
    }
    if (pendingLink.current) {
      const link = pendingLink.current;
      if (
        Math.hypot(event.clientX - link.clientX, event.clientY - link.clientY) <
        4
      )
        return;
      selection.current = { start: link.start, end: point(event) };
      pendingLink.current = null;
      draw();
      return;
    }
    if (
      !selection.current &&
      owned() &&
      !history &&
      (screen.modes.mouse_motion ||
        (screen.modes.mouse_drag && event.buttons)) &&
      !event.shiftKey
    ) {
      mouse(event, "Move");
      return;
    }
    if (selection.current && event.buttons) {
      selection.current.end = point(event);
      draw();
    }
  }
  function linkAt(event: {
    clientX: number;
    clientY: number;
  }): string | undefined {
    const screen = get();
    if (!screen || !renderer.current || history) return;
    const p = renderer.current.point(event, screen);
    const uri = screen.cells[p.y * screen.cols + p.x]?.hyperlink;
    return uri && /^https?:\/\//i.test(uri) ? uri : undefined;
  }
  function selectedText(): string | null {
    if (!selection.current || !store.session) return applicationCopy.current;
    let text: string;
    if (history) {
      const { start, end } = selection.current;
      const [a, b] =
        start.y < end.y || (start.y === end.y && start.x <= end.x)
          ? [start, end]
          : [end, start];
      text = history
        .slice(a.y, b.y + 1)
        .map((line, i, lines) =>
          line.slice(
            i === 0 ? a.x : 0,
            i === lines.length - 1 ? b.x + 1 : undefined,
          ),
        )
        .join("\n");
    } else {
      text = store.session.select(
        pane.id,
        selection.current.start,
        selection.current.end,
      );
    }
    return text;
  }
  async function copy(value?: string) {
    let text: string | null = value ?? null;
    try {
      text = value ?? selectedText();
      if (text === null) return;
      if (!navigator.clipboard?.writeText) throw Error("Clipboard unavailable");
      await navigator.clipboard.writeText(text);
      setClipboardOffer((pending) => (pending === text ? null : pending));
    } catch {
      if (text !== null) setClipboardOffer(text);
    }
  }
  async function scroll(delta: number, local = false, event?: WheelEvent) {
    if (store.closing?.has(pane.id)) return;
    const screen = get();
    if (!screen) return;
    if (preview.current) {
      const match = preview.current;
      const next = Math.max(
        0,
        Math.min(
          Math.max(0, match.lines.length - screen.rows),
          previewOffset.current - delta,
        ),
      );
      previewOffset.current = next;
      if (renderer.current)
        renderer.current.historyMatch = find
          ? {
              start: { ...match.start, y: match.start.y - next },
              end: { ...match.end, y: match.end.y - next },
            }
          : null;
      displayHistory(match.lines.slice(next, next + screen.rows));
      return;
    }
    if (reportsMouse() && offset.current === 0 && !local) {
      control();
      const bytes = mouseBytes(screen, {
        position:
          event && renderer.current
            ? renderer.current.point(event, screen)
            : { x: 0, y: 0 },
        kind: delta < 0 ? "WheelUp" : "WheelDown",
        button: 0,
        shift: false,
        alt: !!event?.altKey,
        ctrl: !!event?.ctrlKey,
      });
      if (bytes.length) {
        const batch = Array.from(
          { length: Math.abs(delta) },
          () => bytes,
        ).flat();
        store.session?.request("input", { pane: pane.id, bytes: batch });
      }
      return;
    }
    const next = Math.max(
      0,
      Math.min(oldestHistory.current ?? 100000, offset.current - delta),
    );
    if (next === offset.current && historyRef.current) return;
    offset.current = next;
    if (next === 0) {
      returnLive();
      return;
    }
    await loadHistory();
  }
  async function loadHistory() {
    if (historyBusy.current || !mounted.current) return;
    historyBusy.current = true;
    try {
      while (mounted.current && offset.current > 0) {
        const wanted = offset.current;
        const rows = get()?.rows || 40;
        const cache = historyCache.current;
        const cached = Array.from({ length: rows }, (_, index) =>
          cache.get(-wanted + index),
        );
        if (cached.every((line) => line !== undefined)) {
          displayHistory(cached as string[]);
          const below = -wanted + rows + 32;
          const topBuffered =
            cache.has(-wanted - 32) ||
            (oldestHistory.current !== null &&
              wanted + 32 >= oldestHistory.current);
          if (topBuffered && (below >= rows || cache.has(below))) break;
        }
        const start = Math.max(-100000, -wanted - (200 - rows));
        const result = await store.session?.rpc("history", {
          workspace: store.workspace,
          pane: pane.id,
          start,
          limit: 200,
        });
        if (!mounted.current || offset.current === 0) break;
        const lines: [number, string][] = result?.History?.lines || [];
        if (lines.length && lines[0][0] > start) {
          oldestHistory.current = Math.max(0, -lines[0][0]);
          offset.current = Math.min(offset.current, oldestHistory.current);
          if (offset.current === 0) {
            returnLive();
            break;
          }
        }
        for (const [line, text] of lines) cache.set(line, text);
        if (cache.size > 2048) {
          const nearest = [...cache.keys()].sort(
            (a, b) => Math.abs(a + wanted) - Math.abs(b + wanted),
          );
          for (const line of nearest.slice(2048)) cache.delete(line);
        }
        if (wanted !== offset.current) continue;
        const visible = lines
          .filter(([line]) => line >= -wanted)
          .slice(0, rows)
          .map(([, text]) => text);
        displayHistory(
          visible.length
            ? visible
            : lines.slice(0, rows).map(([, text]) => text),
        );
        break;
      }
    } finally {
      historyBusy.current = false;
    }
  }
  wheelHandler.current = (event) => {
    onActivate();
    const app = !!reportsMouse() && offset.current === 0 && !event.shiftKey;
    if (wheelIntent.current?.app !== app) wheelRows.current.reset();
    wheelIntent.current = { app, event };
    wheelRows.current.push(
      event.deltaY,
      event.deltaMode,
      renderer.current?.ch || 20,
      get()?.rows || 40,
      1,
      app,
    );
    const flush = () => {
      wheelFrame.current = 0;
      const intent = wheelIntent.current;
      if (!intent || !mounted.current) return;
      if (
        intent.app !==
        (!!reportsMouse() && offset.current === 0 && !intent.event.shiftKey)
      ) {
        wheelRows.current.reset();
        return;
      }
      const rows = wheelRows.current.take();
      if (rows) void scroll(rows, !intent.app, intent.event).catch(onError);
      if (wheelRows.current.pending)
        wheelFrame.current = requestAnimationFrame(flush);
    };
    if (!wheelFrame.current) wheelFrame.current = requestAnimationFrame(flush);
  };
  return (
    <div className="terminal">
      {find && (
        <PaneFind
          store={store}
          pane={pane.id}
          initialQuery={findQuery}
          onError={onError}
          onClose={onCloseFind}
          onReveal={(match) => {
            if (!mounted.current || store.closing?.has(pane.id)) return;
            if (!match) {
              returnLive();
              return;
            }
            preview.current = match;
            previewOffset.current = 0;
            setSearchPreview(true);
            historyCache.current.clear();
            offset.current = Math.max(1, -match.first_line);
            if (renderer.current)
              renderer.current.historyMatch = {
                start: match.start,
                end: match.end,
              };
            displayHistory(match.lines.slice(0, get()?.rows || 40));
          }}
        />
      )}
      <canvas
        ref={canvas}
        aria-label={`Terminal ${pane.number}: ${pane.title}`}
        onPointerDown={down}
        onPointerMove={move}
        onPointerCancel={(event) => {
          if (event.pointerType === "touch") return touchEnd(event, true);
          pendingLink.current = null;
          pressedMouse.current = false;
        }}
        onPointerLeave={() => {
          if (renderer.current) {
            renderer.current.hoverLink = undefined;
            draw();
          }
        }}
        onPointerUp={(event) => {
          if (event.pointerType === "touch") return touchEnd(event);
          if (pendingLink.current) {
            const { uri } = pendingLink.current;
            pendingLink.current = null;
            window.open(uri, "_blank", "noopener,noreferrer");
            return;
          }
          if (
            pressedMouse.current &&
            !selection.current &&
            reportsMouse() &&
            !event.shiftKey &&
            owned()
          ) {
            mouse(event, "Release");
            pressedMouse.current = false;
          } else if (selection.current) {
            if (
              selection.current.start.x === selection.current.end.x &&
              selection.current.start.y === selection.current.end.y
            ) {
              selection.current = null;
              draw();
            } else copy();
          }
        }}
        onDoubleClick={(event) => {
          const screen = get();
          if (!screen) return;
          if (reportsMouse() && !event.shiftKey && !history) return;
          selection.current = wordAt(renderer.current!.point(event, screen));
          draw();
          copy();
        }}
      />
      <textarea
        ref={input}
        className="terminal-input"
        aria-label={`Input for terminal ${pane.number}`}
        autoCapitalize="off"
        autoCorrect="off"
        spellCheck={false}
        onKeyDown={(event) => key(event)}
        onKeyUp={(event) => key(event, true)}
        onFocus={() => {
          if (
            active &&
            !document.hidden &&
            document.hasFocus() &&
            !document.querySelector('[role="dialog"],[role="alertdialog"]')
          )
            control();
          if (owned() && get()?.modes.focus) send("\x1b[I");
          onInputFocus?.(true);
        }}
        onBlur={() => {
          onInputFocus?.(false);
          sentKeys.current.clear();
          if (owned() && get()?.modes.focus) send("\x1b[O");
        }}
        onInput={(e) => {
          if (
            !(e.nativeEvent as InputEvent).isComposing &&
            e.currentTarget.value
          ) {
            typed(e.currentTarget.value);
            e.currentTarget.value = "";
          }
        }}
        onCompositionEnd={(e) => {
          if (e.data) typed(e.data);
          e.currentTarget.value = "";
        }}
        onPaste={(e) => {
          e.preventDefault();
          const text = e.clipboardData.getData("text/plain");
          const files = Array.from(e.clipboardData.files || []);
          if (!text && files.length && onFiles) onFiles(files);
          else send(text, true);
        }}
        onCopy={(e) => {
          try {
            const text = selectedText();
            if (text === null) return;
            e.clipboardData.setData("text/plain", text);
            e.preventDefault();
            setClipboardOffer(null);
          } catch (error) {
            onError(error);
          }
        }}
      />
      {clipboardOffer !== null && (
        <div className="clipboard-offer" role="status">
          <button onClick={() => void copy(clipboardOffer)}>
            Copy from terminal
          </button>
          <button
            aria-label="Dismiss clipboard copy"
            onClick={() => setClipboardOffer(null)}
          >
            ×
          </button>
          <textarea
            aria-label="Text to copy"
            readOnly
            value={clipboardOffer}
            onFocus={(event) => event.currentTarget.select()}
          />
        </div>
      )}
      {modeKind && (
        <div className="key-mode-indicator" role="status">
          {modeKind === "scroll"
            ? "Scroll · ↑↓ PgUp PgDn g G · Space select · q exit"
            : "Select · arrows/hjkl move · v mark · y copy · q exit"}
        </div>
      )}
      {history && (
        <button
          className={`history-indicator ${searchPreview ? "search-preview" : ""}`}
          onClick={() => {
            returnLive();
            if (!touchKeyboard) input.current?.focus({ preventScroll: true });
          }}
        >
          {searchPreview
            ? "Search result · Back to live"
            : "↑ History · Back to live"}
        </button>
      )}
      <div className="sr-only" aria-live="off">
        {history
          ? history.join("\n")
          : get()
              ?.cells.filter((c) => c.width > 0)
              .map((c) => c.text)
              .join("")}
      </div>
    </div>
  );
}
