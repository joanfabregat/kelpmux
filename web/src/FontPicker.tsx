import React, { useEffect, useId, useRef, useState } from "react";
import { fonts } from "./appearance";
import { Icon } from "./status";
export function FontPicker({
  value,
  onChange,
}: {
  value: keyof typeof fonts;
  onChange: (font: keyof typeof fonts) => void;
}) {
  const [open, setOpen] = useState(false),
    root = useRef<HTMLDivElement>(null),
    trigger = useRef<HTMLButtonElement>(null),
    id = useId();
  useEffect(() => {
    if (!open) return;
    root.current
      ?.querySelector<HTMLButtonElement>(
        '[role="option"][aria-selected="true"]',
      )
      ?.focus();
    const outside = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);
  const close = () => {
    setOpen(false);
    trigger.current?.focus();
  };
  return (
    <div
      className="font-picker"
      ref={root}
      data-font-picker
      data-open={open}
      onKeyDown={(event) => {
        if (event.key === "Escape" && open) {
          event.preventDefault();
          event.stopPropagation();
          close();
        }
        if (event.key === "Tab") setOpen(false);
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault();
          if (!open) {
            setOpen(true);
            return;
          }
          const options = [
            ...(root.current?.querySelectorAll<HTMLButtonElement>(
              '[role="option"]',
            ) || []),
          ];
          const current = options.indexOf(
            document.activeElement as HTMLButtonElement,
          );
          options[
            (current + (event.key === "ArrowDown" ? 1 : -1) + options.length) %
              options.length
          ]?.focus();
        }
      }}
    >
      <span id={id + "-label"}>Terminal font</span>
      <button
        type="button"
        className="font-trigger"
        ref={trigger}
        role="combobox"
        aria-labelledby={id + "-label"}
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-controls={id}
        onClick={() => setOpen(!open)}
      >
        <span style={{ fontFamily: fonts[value].family }}>
          {fonts[value].label}
        </span>
        <Icon name="chevron-down" size="sm" className="select-chevron" />
      </button>
      {open && (
        <div
          className="font-options"
          role="listbox"
          aria-label="Terminal font choices"
          id={id}
        >
          {Object.entries(fonts).map(([key, font]) => (
            <button
              type="button"
              role="option"
              aria-selected={value === key}
              key={key}
              style={{ fontFamily: font.family }}
              onClick={() => {
                onChange(key as keyof typeof fonts);
                close();
              }}
            >
              <span>{font.label}</span>
              <span className="font-sample" aria-hidden="true">
                Aa 0123
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
