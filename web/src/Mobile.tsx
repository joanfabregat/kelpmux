// Phone and tablet pieces of the application shell: layout detection, the
// visual-viewport tracking that keeps the key bar above the iOS keyboard,
// the key bar itself and the phone's top bar. Layout rules are in device.ts;
// key encoding and input mode in terminal/touch-keys.ts.
import React, { useEffect, useRef, useState } from "react";
import type { NumberedPane } from "@kelpcore/web/types.ts";
import {
  barKeys,
  isHardwareKey,
  type BarKey,
} from "@kelpcore/web/touch-keys.ts";
import {
  keyboardInset,
  layoutMode,
  orientation,
  type LayoutMode,
  type Orientation,
} from "./device";
import { Icon } from "./status";
import { StateMark } from "./state/StateMark";
import type { DisplayState } from "./state/model";
import { NotificationsButton } from "./Shell";

export type Device = {
  mode: LayoutMode;
  orientation: Orientation;
  /** The primary pointer is a finger: touch input rules apply. */
  touch: boolean;
};

function measure(): Device {
  const coarse = !!window.matchMedia?.("(pointer: coarse)").matches;
  const size = { width: innerWidth, height: innerHeight };
  return {
    mode: layoutMode({ ...size, coarse }),
    orientation: orientation(size),
    touch: coarse,
  };
}

/** The current layout mode, orientation and pointer, also exposed to CSS as
 * `data-layout`, `data-orientation` and `data-touch` on the root element. */
export function useDevice(): Device {
  const [device, setDevice] = useState(measure);
  useEffect(() => {
    const update = () =>
      setDevice((previous) => {
        const next = measure();
        return previous.mode === next.mode &&
          previous.orientation === next.orientation &&
          previous.touch === next.touch
          ? previous
          : next;
      });
    const pointer = window.matchMedia?.("(pointer: coarse)");
    window.addEventListener("resize", update);
    window.addEventListener("orientationchange", update);
    pointer?.addEventListener?.("change", update);
    return () => {
      window.removeEventListener("resize", update);
      window.removeEventListener("orientationchange", update);
      pointer?.removeEventListener?.("change", update);
    };
  }, []);
  useEffect(() => {
    const root = document.documentElement.dataset;
    root.layout = device.mode;
    root.orientation = device.orientation;
    root.touch = String(device.touch);
  }, [device]);
  return device;
}

/** Tracks the visual viewport (what remains visible above the on-screen
 * keyboard) as CSS variables on the root element: `--vv-height`,
 * `--vv-top` and `--keyboard-inset`, plus `data-keyboard="open"` while
 * something covers the bottom. The touch shell is fixed to that area, so the
 * key bar sits directly above the keyboard. */
export function useVisualViewport(enabled: boolean) {
  useEffect(() => {
    const root = document.documentElement;
    const viewport = window.visualViewport;
    const clear = () => {
      root.style.removeProperty("--vv-height");
      root.style.removeProperty("--vv-top");
      root.style.removeProperty("--keyboard-inset");
      delete root.dataset.keyboard;
    };
    if (!enabled || !viewport) {
      clear();
      return;
    }
    let frame = 0;
    const update = () => {
      frame = 0;
      const inset = keyboardInset(innerHeight, viewport);
      root.style.setProperty("--vv-height", `${Math.round(viewport.height)}px`);
      root.style.setProperty(
        "--vv-top",
        `${Math.max(0, Math.round(viewport.offsetTop))}px`,
      );
      root.style.setProperty("--keyboard-inset", `${inset}px`);
      if (inset > 0) root.dataset.keyboard = "open";
      else delete root.dataset.keyboard;
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(update);
    };
    update();
    viewport.addEventListener("resize", schedule);
    viewport.addEventListener("scroll", schedule);
    window.addEventListener("resize", schedule);
    return () => {
      cancelAnimationFrame(frame);
      viewport.removeEventListener("resize", schedule);
      viewport.removeEventListener("scroll", schedule);
      window.removeEventListener("resize", schedule);
      clear();
    };
  }, [enabled]);
}

/** Keeps a tap on a bar button from moving focus away from the terminal's
 * input, which would hide the on-screen keyboard. */
const keepFocus = (event: React.PointerEvent | React.MouseEvent) =>
  event.preventDefault();

/** Calls `onKey` with a key typed on a hardware keyboard while no text
 * field has focus (`isHardwareKey`; iPadOS sends such keys to the
 * document), after the application's shortcut handlers, which listen in the
 * capture phase, had their turn. The key is taken from the page (its default
 * action is prevented) and described so it can be dispatched again on the
 * terminal's input. */
export function useHardwareKeys(
  enabled: boolean,
  onKey: (key: KeyboardEventInit) => void,
) {
  const callback = useRef(onKey);
  callback.current = onKey;
  useEffect(() => {
    if (!enabled) return;
    const handler = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const editable = !!target?.closest?.(
        'input, textarea, select, [contenteditable]:not([contenteditable="false"])',
      );
      if (
        !isHardwareKey({
          key: event.key,
          metaKey: event.metaKey,
          isComposing: event.isComposing,
          defaultPrevented: event.defaultPrevented,
          editable,
        })
      )
        return;
      event.preventDefault();
      event.stopPropagation();
      callback.current({
        key: event.key,
        code: event.code,
        location: event.location,
        repeat: event.repeat,
        shiftKey: event.shiftKey,
        ctrlKey: event.ctrlKey,
        altKey: event.altKey,
        metaKey: event.metaKey,
      });
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [enabled]);
}

/** The fixed key bar of touch devices, above the on-screen keyboard: keys a
 * phone keyboard lacks, a sticky Ctrl, the KelpMux prefix, paste, answers
 * for agent questions, and the keyboard toggle. Tapping a key also enters
 * input mode. */
export function KeyBar({
  ctrl,
  prefix,
  input,
  onKey,
  onToggleKeyboard,
}: {
  ctrl: boolean;
  prefix: boolean;
  input: boolean;
  onKey: (key: BarKey) => void;
  onToggleKeyboard: () => void;
}) {
  return (
    <div className="key-bar" role="toolbar" aria-label="Terminal keys">
      <div className="key-bar-keys">
        {barKeys.map(({ key, label, title }) => (
          <button
            type="button"
            key={key}
            className={`key-bar-key key-${key.toLowerCase()}`}
            aria-label={title}
            aria-pressed={
              key === "Ctrl" ? ctrl : key === "Prefix" ? prefix : undefined
            }
            data-key={key}
            onPointerDown={keepFocus}
            onMouseDown={keepFocus}
            onClick={(event) => {
              event.stopPropagation();
              onKey(key);
            }}
          >
            {key === "Paste" ? (
              <Icon name="clipboard-paste" size="sm" />
            ) : (
              label
            )}
          </button>
        ))}
      </div>
      <button
        type="button"
        className="key-bar-key key-bar-keyboard"
        aria-label={input ? "Hide keyboard" : "Show keyboard"}
        aria-pressed={input}
        onPointerDown={keepFocus}
        onMouseDown={keepFocus}
        onClick={(event) => {
          event.stopPropagation();
          onToggleKeyboard();
        }}
      >
        <Icon name={input ? "keyboard-off" : "keyboard"} size="sm" />
        {!input && <span>Input</span>}
      </button>
    </div>
  );
}

/** The phone's bar above a terminal: back to the terminal list, the
 * terminal's state and title, its notifications (the bell, as in a
 * desktop terminal's header) and its menu. The Input control is on the key
 * bar below the terminal. */
export function PhoneBar({
  pane,
  state,
  attention,
  onList,
  unread,
  onNotifications,
  onMenu,
}: {
  pane: NumberedPane;
  state: DisplayState;
  /** Terminals needing attention, shown on the back button. */
  attention: number;
  onList: () => void;
  /** The terminal's unread notifications. */
  unread: number;
  onNotifications: () => void;
  onMenu: (x: number, y: number) => void;
}) {
  return (
    <header className="topbar phone-bar">
      <button
        type="button"
        className="ghost phone-back"
        aria-label="Terminals"
        onClick={(event) => {
          event.stopPropagation();
          onList();
        }}
      >
        <Icon name="chevron-left" />
        <span>Terminals</span>
        {attention > 0 && (
          <span className="badge accent count">{attention}</span>
        )}
      </button>
      <span className="phone-title" title={pane.cwd}>
        <StateMark state={state} />
        {pane.number > 0 && <span className="tab-number">{pane.number}</span>}
        <span className="phone-title-text">{pane.title || "shell"}</span>
      </span>
      <NotificationsButton
        label={pane.title || "shell"}
        count={unread}
        onOpen={onNotifications}
      />
      <button
        type="button"
        className="icon-btn phone-menu"
        aria-label={`Menu for ${pane.title || "shell"}`}
        onClick={(event) => {
          event.stopPropagation();
          const rect = event.currentTarget.getBoundingClientRect();
          onMenu(rect.right - 220, rect.bottom + 4);
        }}
      >
        <Icon name="ellipsis" />
      </button>
    </header>
  );
}
