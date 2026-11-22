import type React from "react";
import { SharedSession as Session } from "./terminal/shared-session";
import type { Store } from "./terminal/Terminal";
import type { Connection } from "./terminal/types";
import { modes, terminalColors, type Appearance } from "./appearance";
import { ThemePicker } from "./ThemePicker";
import { Settings } from "./Settings";
import { FontPicker } from "./FontPicker";
import { Icon } from "./status";
import { shortcuts } from "./shortcuts";
import type { Preferences } from "./layout-store";

export type ConfigPanelProps = {
  store: Store;
  connection: Connection | null;
  appearance: Appearance;
  setAppearance: (appearance: Appearance) => void;
  section: string;
  setSection: (section: string) => void;
  confirmClose: boolean;
  setConfirmClose: (value: boolean) => void;
  keyContent: React.ReactNode;
  preferences: Preferences;
  setPreferences: (preferences: Preferences) => void;
};

/** Settings dialog body. Everything here is saved in this browser only;
 * the terminal server and other clients (the TUI included) are unaffected. */
export function ConfigPanel({
  store,
  connection,
  appearance,
  setAppearance,
  section: settingsSection,
  setSection: setSettingsSection,
  confirmClose,
  setConfirmClose,
  keyContent,
  preferences,
  setPreferences,
}: ConfigPanelProps) {
  return (
    <Settings active={settingsSection} onChange={setSettingsSection}>
      <section hidden={settingsSection !== "appearance"}>
        <h3>Mode</h3>
        <div className="seg setting-seg" role="group" aria-label="Mode">
          {modes.map(([mode, label]) => (
            <button
              key={mode}
              aria-pressed={appearance.mode === mode}
              onClick={() => setAppearance({ ...appearance, mode })}
            >
              {label}
            </button>
          ))}
        </div>
        <h3>Terminal color</h3>
        <div
          className="seg setting-seg"
          role="group"
          aria-label="Terminal color"
        >
          {terminalColors.map(([terminal, label]) => (
            <button
              key={terminal}
              aria-pressed={appearance.terminal === terminal}
              onClick={() => setAppearance({ ...appearance, terminal })}
            >
              {label}
            </button>
          ))}
        </div>
        <div className="font-settings">
          <FontPicker
            value={appearance.font}
            onChange={(font) => setAppearance({ ...appearance, font })}
          />
          <label className="size-setting">
            Font size
            <select
              aria-label="Font size"
              value={appearance.size}
              onChange={(event) =>
                setAppearance({
                  ...appearance,
                  size: Number(event.target.value),
                })
              }
            >
              {Array.from({ length: 19 }, (_, i) => i + 10).map((size) => (
                <option key={size} value={size}>
                  {size} px
                </option>
              ))}
            </select>
          </label>
        </div>
        <p className="setting-hint">
          Display settings are saved in this browser. JetBrains Mono is bundled;
          other choices use installed system fonts.
        </p>
      </section>
      <section hidden={settingsSection !== "theme"}>
        <h3>Theme</h3>
        <ThemePicker
          value={appearance.theme}
          onChange={(theme) => setAppearance({ ...appearance, theme })}
        />
        <p className="setting-hint">
          Each card shows the theme's dark and light variants. Mode in
          Appearance chooses which one is used.
        </p>
      </section>
      <section hidden={settingsSection !== "alerts"}>
        <h3>Attention alerts</h3>
        <label className="close-setting">
          <input
            type="checkbox"
            checked={preferences.sound}
            onChange={(event) =>
              setPreferences({ ...preferences, sound: event.target.checked })
            }
          />
          Play a sound when a terminal asks a question, finishes or fails
        </label>
        <label className="close-setting">
          <input
            type="checkbox"
            checked={preferences.desktop}
            onChange={async (event) => {
              const desktop = event.target.checked;
              if (
                desktop &&
                "Notification" in window &&
                Notification.permission === "default"
              )
                await Notification.requestPermission().catch(() => "denied");
              setPreferences({ ...preferences, desktop });
            }}
          />
          Show a desktop notification while this tab is in the background
        </label>
        {preferences.desktop &&
          "Notification" in window &&
          Notification.permission === "denied" && (
            <p className="setting-hint">
              This browser blocks notifications for this site; allow them in its
              site settings.
            </p>
          )}
        <h3>Sidebar</h3>
        <label className="close-setting">
          <input
            type="checkbox"
            checked={preferences.sidebarFolded}
            onChange={(event) =>
              setPreferences({
                ...preferences,
                sidebarFolded: event.target.checked,
              })
            }
          />
          Fold the sidebar to its marks (click or drag its edge too)
        </label>
        <p className="setting-hint">
          These preferences, parked terminals, tab colours and the terminal
          order are saved in this browser. Notification read marks are shared
          with the TUI: showing a terminal here (or focusing its grid tile)
          marks its notifications read in both, as the TUI does when it shows
          the terminal.
        </p>
      </section>
      <section hidden={settingsSection !== "shortcuts"}>
        <h3>Browser shortcuts</h3>
        <table className="shortcut-table">
          <tbody>
            {shortcuts.map(([keys, action]) => (
              <tr key={keys}>
                <th scope="row">
                  <kbd>{keys}</kbd>
                </th>
                <td>{action}</td>
              </tr>
            ))}
          </tbody>
        </table>
        <p className="setting-hint">
          Every other key goes to the focused terminal. The TUI's own bindings
          (its prefix and configuration file) are unchanged; this browser's
          tabs, grid and focus are separate from the TUI's.
        </p>
      </section>
      <section hidden={settingsSection !== "keys"}>{keyContent}</section>
      <section hidden={settingsSection !== "connection"}>
        <h3>Browser session</h3>
        <p className="setting-hint">
          {store.session instanceof Session && store.session.host
            ? "Connection tab — keep this tab open while using other KelpMux windows."
            : "Using the SSH connection held by another KelpMux tab."}{" "}
          Shared sessions lock after 15 minutes without interaction. Logging out
          disconnects every shared window; terminals keep running.
        </p>
        <label className="close-setting">
          <input
            type="checkbox"
            checked={confirmClose}
            onChange={(event) => setConfirmClose(event.target.checked)}
          />
          Confirm before closing or reloading this browser tab
        </label>
        <h3>Connection</h3>
        <p className="connection-line mono">{connection?.username} · SSH</p>
        <button className="subtle" onClick={() => store.session?.close()}>
          <Icon name="log-out" size="sm" />
          Log out all browser windows
        </button>
      </section>
    </Settings>
  );
}
