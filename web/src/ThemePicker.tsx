import type React from "react";
import { themeVariables, themes, type ThemeName } from "./appearance";
import { Icon } from "./status";

const names = Object.keys(themes) as ThemeName[];

/** A tiny shell listing drawn in one theme variant's own colors. */
function Sample({ name, mode }: { name: ThemeName; mode: "dark" | "light" }) {
  const v = themeVariables(name, mode);
  const color = (index: number) => ({ color: v[`--terminal-ansi-${index}`] });
  return (
    <span
      className="theme-sample"
      style={{
        background: v["--background"],
        color: v["--terminal-foreground"],
      }}
    >
      <span>
        <span style={color(2)}>user@host</span>:<span style={color(4)}>~</span>$
        ls
      </span>
      <span>
        drwxr-xr-x{"  "}
        <b style={color(4)}>docs</b>
        {"  "}
        <b style={color(4)}>src</b>
      </span>
      <span>-rw-r--r--{"  "}README.md</span>
    </span>
  );
}

/** Theme choice as preview cards: each card shows the dark variant on the
 * left and the light variant on the right, with the name below. Radio
 * group semantics: one tab stop, arrow keys move and select. */
export function ThemePicker({
  value,
  onChange,
}: {
  value: ThemeName;
  onChange: (value: ThemeName) => void;
}) {
  const choose = (index: number, group: HTMLElement | null) => {
    const next = names[(index + names.length) % names.length];
    onChange(next);
    group?.querySelector<HTMLElement>(`[data-theme-name="${next}"]`)?.focus();
  };
  const keyDown = (event: React.KeyboardEvent, index: number) => {
    const group = event.currentTarget.parentElement;
    if (event.key === "ArrowRight" || event.key === "ArrowDown")
      choose(index + 1, group);
    else if (event.key === "ArrowLeft" || event.key === "ArrowUp")
      choose(index - 1, group);
    else if (event.key === "Home") choose(0, group);
    else if (event.key === "End") choose(names.length - 1, group);
    else return;
    event.preventDefault();
  };
  return (
    <div className="theme-cards" role="radiogroup" aria-label="Theme">
      {names.map((name, index) => {
        const checked = value === name;
        return (
          <button
            key={name}
            type="button"
            role="radio"
            aria-checked={checked}
            tabIndex={checked ? 0 : -1}
            data-theme-name={name}
            className="theme-card"
            onClick={() => onChange(name)}
            onKeyDown={(event) => keyDown(event, index)}
          >
            <span className="theme-samples" aria-hidden="true">
              <Sample name={name} mode="dark" />
              <Sample name={name} mode="light" />
            </span>
            <span className="theme-name">
              {themes[name].label}
              {checked && <Icon name="check" size="sm" />}
            </span>
          </button>
        );
      })}
    </div>
  );
}
