import type { ReactNode } from "react";
import { Icon } from "./status";
import type { IconName } from "./icons";
const sections: readonly (readonly [string, string, IconName])[] = [
  ["appearance", "Appearance", "monitor"],
  ["theme", "Theme", "palette"],
  ["shortcuts", "Keyboard", "book-open"],
  ["keys", "Security keys", "key-round"],
  ["connection", "Connection", "plug"],
];
/** Event that asks the app to open Settings at a section. */
export const OPEN_SETTINGS = "kelpmux:open-settings";
/** Opens Settings at `section` from anywhere. */
export function openSettings(section: string) {
  window.dispatchEvent(new CustomEvent(OPEN_SETTINGS, { detail: section }));
}
export function Settings({
  active,
  onChange,
  children,
}: {
  active: string;
  onChange: (value: string) => void;
  children: ReactNode;
}) {
  return (
    <div className="settings-layout">
      <nav
        className="settings-nav"
        role="tablist"
        aria-label="Settings categories"
        aria-orientation="vertical"
      >
        {sections.map(([id, label, icon], index) => (
          <button
            key={id}
            id={`settings-${id}`}
            role="tab"
            aria-selected={active === id}
            aria-controls="settings-panel"
            tabIndex={active === id ? 0 : -1}
            onClick={() => onChange(id)}
            onKeyDown={(event) => {
              let next = index;
              // Left/Right also move: on narrow screens the list is a
              // horizontal strip.
              if (event.key === "ArrowDown" || event.key === "ArrowRight")
                next = (index + 1) % sections.length;
              else if (event.key === "ArrowUp" || event.key === "ArrowLeft")
                next = (index + sections.length - 1) % sections.length;
              else if (event.key === "Home") next = 0;
              else if (event.key === "End") next = sections.length - 1;
              else return;
              event.preventDefault();
              onChange(sections[next][0]);
              const tab = document.getElementById(
                `settings-${sections[next][0]}`,
              );
              tab?.focus();
              tab?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
            }}
          >
            <Icon name={icon} size="sm" />
            {label}
          </button>
        ))}
      </nav>
      <div
        className="settings-panel"
        id="settings-panel"
        role="tabpanel"
        aria-labelledby={`settings-${active}`}
      >
        {children}
      </div>
    </div>
  );
}
