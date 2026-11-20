// A small custom tooltip, used instead of the browser's `title` attribute.
//
// It appears 150 ms after the pointer rests on its trigger or at once when
// the trigger (or, with `focus="closest"`, its nearest focusable ancestor)
// receives keyboard focus, and immediately when the pointer moves on from
// another tooltip. Touch shows it on a long press. Escape, blur, scrolling
// and leaving dismiss it. Only one tooltip is open at a time.
//
// The bubble is portaled to <body>, is `role="tooltip"` and is referenced by
// `aria-describedby` while open. Its position is set through CSSOM
// (`element.style`), which the `style-src 'self'` policy allows; there are no
// inline style attributes or elements.
import React, {
  cloneElement,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import "./styles/tooltip.css";
import { placeTooltip } from "./tooltip-position";

const SHOW_DELAY = 150;
/** A tooltip closed less than this long ago lets the next one open at once. */
const HANDOFF = 400;
const LONG_PRESS = 450;

let closeOpen: (() => void) | null = null;
let lastClosed = 0;
/** Focus caused by a pointer press does not show tooltips; keyboard and
 * programmatic focus do. A mouse or pen focuses its target while the button
 * is down; touch focuses it after lifting (compatibility mouse events), so
 * a short window follows a touch release. Focus that merely happens soon
 * after an unrelated click (a script or the keyboard moving focus) still
 * shows the tooltip. */
let pointerHeld = false;
let touchReleased = -Infinity;
if (typeof window !== "undefined") {
  window.addEventListener("pointerdown", () => (pointerHeld = true), true);
  // A release outside the page is never reported.
  window.addEventListener("blur", () => (pointerHeld = false));
  for (const type of ["pointerup", "pointercancel"])
    window.addEventListener(
      type,
      (event) => {
        pointerHeld = false;
        if ((event as PointerEvent).pointerType === "touch")
          touchReleased = performance.now();
      },
      true,
    );
}
const pointerFocus = () =>
  pointerHeld || performance.now() - touchReleased < 600;

type TriggerProps = {
  ref?: React.Ref<HTMLElement>;
  onPointerEnter?: (event: React.PointerEvent<HTMLElement>) => void;
  onPointerLeave?: (event: React.PointerEvent<HTMLElement>) => void;
  onPointerDown?: (event: React.PointerEvent<HTMLElement>) => void;
  onPointerUp?: (event: React.PointerEvent<HTMLElement>) => void;
  onPointerCancel?: (event: React.PointerEvent<HTMLElement>) => void;
  onContextMenu?: (event: React.MouseEvent<HTMLElement>) => void;
  onClick?: (event: React.MouseEvent<HTMLElement>) => void;
  onFocus?: (event: React.FocusEvent<HTMLElement>) => void;
  onBlur?: (event: React.FocusEvent<HTMLElement>) => void;
  "aria-describedby"?: string;
};

export function Tooltip({
  content,
  children,
  focus = "self",
}: {
  /** Tooltip content; nothing is shown when empty. */
  content: React.ReactNode;
  /** One element that accepts a ref and pointer/focus handlers. */
  children: React.ReactElement<TriggerProps>;
  /** "closest": the trigger sits inside a focusable control (a button), so
   * that control's focus shows the tooltip and carries `aria-describedby`. */
  focus?: "self" | "closest";
}) {
  const id = useId();
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLElement | null>(null);
  const bubble = useRef<HTMLDivElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const pressed = useRef(false);
  const enabled = content != null && content !== false && content !== "";

  const hide = useCallback(() => {
    clearTimeout(timer.current);
    setOpen((was) => {
      if (was) lastClosed = performance.now();
      return false;
    });
  }, []);
  const show = useCallback(() => {
    clearTimeout(timer.current);
    if (closeOpen && closeOpen !== hide) closeOpen();
    closeOpen = hide;
    setOpen(true);
  }, [hide]);
  const showSoon = useCallback(() => {
    clearTimeout(timer.current);
    if (closeOpen || performance.now() - lastClosed < HANDOFF) show();
    else timer.current = setTimeout(show, SHOW_DELAY);
  }, [show]);
  useEffect(
    () => () => {
      clearTimeout(timer.current);
      if (closeOpen === hide) closeOpen = null;
    },
    [hide],
  );
  useEffect(() => {
    if (!enabled) hide();
  }, [enabled, hide]);

  // The focusable control that announces the tooltip.
  const target = () =>
    focus === "closest"
      ? (trigger.current?.parentElement?.closest<HTMLElement>(
          "button, a[href], [tabindex]",
        ) ?? trigger.current)
      : trigger.current;
  // Re-resolved after every render, so the listeners follow the control if
  // the trigger moves to another one.
  const [control, setControl] = useState<HTMLElement | null>(null);
  useLayoutEffect(() => {
    const found = focus === "closest" && enabled ? target() : null;
    if (found !== control) setControl(found);
  });
  useEffect(() => {
    if (!control || control === trigger.current) return;
    const onFocus = () => {
      if (!pointerFocus()) show();
    };
    control.addEventListener("focus", onFocus);
    control.addEventListener("blur", hide);
    return () => {
      control.removeEventListener("focus", onFocus);
      control.removeEventListener("blur", hide);
    };
  }, [control, show, hide]);

  useEffect(() => {
    if (!open) {
      if (closeOpen === hide) closeOpen = null;
      return;
    }
    const control = target();
    const previous = control?.getAttribute("aria-describedby");
    if (control && control !== trigger.current)
      control.setAttribute(
        "aria-describedby",
        previous ? `${previous} ${id}` : id,
      );
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      hide();
    };
    const outside = (event: PointerEvent) => {
      if (!trigger.current?.contains(event.target as Node)) hide();
    };
    window.addEventListener("keydown", escape, true);
    window.addEventListener("scroll", hide, true);
    window.addEventListener("resize", hide);
    window.addEventListener("pointerdown", outside, true);
    return () => {
      window.removeEventListener("keydown", escape, true);
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("resize", hide);
      window.removeEventListener("pointerdown", outside, true);
      if (control && control !== trigger.current) {
        if (previous) control.setAttribute("aria-describedby", previous);
        else control.removeAttribute("aria-describedby");
      }
    };
  }, [open, hide, id]);

  useLayoutEffect(() => {
    const el = bubble.current;
    if (!open || !el || !trigger.current) return;
    const { top, left, placement } = placeTooltip(
      trigger.current.getBoundingClientRect(),
      el.getBoundingClientRect(),
      { width: window.innerWidth, height: window.innerHeight },
    );
    el.style.top = `${Math.round(top)}px`;
    el.style.left = `${Math.round(left)}px`;
    el.dataset.placement = placement;
  });

  if (!enabled) return children;
  const props = children.props;
  const chain =
    <E,>(own: ((event: E) => void) | undefined, mine: (event: E) => void) =>
    (event: E) => {
      own?.(event);
      mine(event);
    };
  const childRef = props.ref;
  return (
    <>
      {cloneElement(children, {
        ref: (node: HTMLElement | null) => {
          trigger.current = node;
          if (typeof childRef === "function") childRef(node);
          else if (childRef)
            (childRef as React.RefObject<HTMLElement | null>).current = node;
        },
        onPointerEnter: chain(props.onPointerEnter, (event) => {
          if (event.pointerType !== "touch") showSoon();
        }),
        onPointerLeave: chain(props.onPointerLeave, (event) => {
          if (event.pointerType !== "touch") hide();
        }),
        onPointerDown: chain(props.onPointerDown, (event) => {
          if (event.pointerType !== "touch") return hide();
          pressed.current = false;
          clearTimeout(timer.current);
          timer.current = setTimeout(() => {
            pressed.current = true;
            show();
          }, LONG_PRESS);
        }),
        onPointerUp: chain(props.onPointerUp, (event) => {
          if (event.pointerType !== "touch") return;
          if (!pressed.current) clearTimeout(timer.current);
        }),
        onPointerCancel: chain(props.onPointerCancel, () =>
          clearTimeout(timer.current),
        ),
        // A long press shows the tooltip instead of the context menu, and
        // does not also activate the control around the trigger.
        onContextMenu: chain(props.onContextMenu, (event) => {
          if (pressed.current) event.preventDefault();
        }),
        onClick: (event: React.MouseEvent<HTMLElement>) => {
          if (pressed.current) {
            pressed.current = false;
            event.preventDefault();
            event.stopPropagation();
            return;
          }
          props.onClick?.(event);
        },
        ...(focus === "self"
          ? {
              onFocus: chain(props.onFocus, (event) => {
                if (!pointerFocus()) show();
              }),
              onBlur: chain(props.onBlur, hide),
              "aria-describedby": open
                ? [props["aria-describedby"], id].filter(Boolean).join(" ")
                : props["aria-describedby"],
            }
          : {}),
      })}
      {open &&
        createPortal(
          <div ref={bubble} id={id} role="tooltip" className="tooltip">
            {content}
          </div>,
          document.body,
        )}
    </>
  );
}
