import { Icon } from "./status";

/** The installed app's title bar, shown only when Chrome draws the window
 * controls over the page (`display-mode: window-controls-overlay`, the
 * manifest's `display_override`); everywhere else it is `display: none`
 * (styles/titlebar.css). It fills the title bar area between the OS
 * controls and is the window's drag region, so it holds nothing
 * interactive: the KelpMux mark and name, then the place shown. Its
 * background is `--background`, the colour main.tsx also gives the
 * `theme-color` meta, with which Chrome paints the controls. The window
 * title (document.title) still names the app for assistive technology. */
export function TitleBar({ location }: { location: string }) {
  return (
    <div className="titlebar" aria-hidden="true">
      <span className="titlebar-brand">
        <span className="logo-mark">
          <Icon name="layers" size="sm" />
        </span>
        <span className="titlebar-name">KelpMux</span>
      </span>
      {location && <span className="titlebar-location">{location}</span>}
    </div>
  );
}
