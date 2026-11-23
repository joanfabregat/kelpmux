import type React from "react";
import { SharedSession as Session } from "@kelpcore/web/shared-session.ts";
import type { Store } from "@kelpcore/web/Terminal.tsx";
import type { Connection } from "@kelpcore/web/types.ts";
import { modes, terminalColors, type Appearance } from "./appearance";
import { ThemePicker } from "./ThemePicker";
import { Settings } from "./Settings";
import { FontPicker } from "./FontPicker";
import { Icon } from "./status";
import { shortcuts } from "./shortcuts";
import type { Preferences } from "./layout-store";
import type { HardwareKeyboard } from "@kelpcore/web/touch-keys.ts";
import { UploadSettings } from "./Upload";
import { idleMinutes, minutesText } from "./idle";
import {
  NOTIFICATION_EVENTS,
  notificationEventHint,
  notificationEventLabel,
} from "./toasts";

const hardwareKeyboardChoices: [HardwareKeyboard, string][] = [
  ["auto", "Auto"],
  ["on", "On"],
  ["off", "Off"],
];

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
  uploadDestination: string;
  setUploadDestination: (path: string) => void;
  uploadUnavailable: string | null;
  /** A touch device: the hardware keyboard setting applies. */
  touch: boolean;
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
  uploadDestination,
  setUploadDestination,
  uploadUnavailable,
  touch,
}: ConfigPanelProps) {
  const idle = idleMinutes(connection);
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
          Play a sound for the events below
        </label>
        <h3>Events</h3>
        <div role="group" aria-label="Notification events">
          {NOTIFICATION_EVENTS.map((event) => (
            <label
              key={event}
              className="close-setting"
              data-notification-event={event}
            >
              <input
                type="checkbox"
                checked={preferences.notificationEvents[event]}
                onChange={(change) =>
                  setPreferences({
                    ...preferences,
                    notificationEvents: {
                      ...preferences.notificationEvents,
                      [event]: change.target.checked,
                    },
                  })
                }
              />
              <span>
                <b>{notificationEventLabel[event]}</b>: when{" "}
                {notificationEventHint[event]}
              </span>
            </label>
          ))}
        </div>
        <p className="setting-hint">
          An enabled event shows a toast in the top right corner, one per
          terminal, except for the terminal you are reading; click it to open
          that terminal. A toast goes away when its notifications are read, here
          or in any other window, device or the TUI. A bell also marks the
          browser tab's icon until it is read.
        </p>
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
          These preferences, parked terminals and the terminal order are saved
          in this browser. Notification read marks are shared with the TUI:
          showing a terminal here (or focusing its grid tile) marks its
          notifications read in both, as the TUI does when it shows the
          terminal.
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
          order, parking, grid and focus are separate from the TUI's.
        </p>
        {touch && (
          <>
            <h3>Hardware keyboard</h3>
            <div
              className="seg setting-seg"
              role="group"
              aria-label="Hardware keyboard"
            >
              {hardwareKeyboardChoices.map(([value, label]) => (
                <button
                  key={value}
                  aria-pressed={preferences.hardwareKeyboard === value}
                  onClick={() =>
                    setPreferences({ ...preferences, hardwareKeyboard: value })
                  }
                >
                  {label}
                </button>
              ))}
            </div>
            <p className="setting-hint">
              Auto: terminals open for reading, and the first key typed on a
              hardware keyboard switches to input and reaches the terminal. On:
              terminals open ready for a hardware keyboard without showing the
              on-screen keyboard; if Safari does not let the page take the keys
              at once, the first key typed does. Off: tap Input or a key bar key
              first. Saved in this browser.
            </p>
          </>
        )}
      </section>
      <section hidden={settingsSection !== "uploads"}>
        <UploadSettings
          destination={uploadDestination}
          onChange={setUploadDestination}
          unavailable={uploadUnavailable}
        />
      </section>
      <section hidden={settingsSection !== "keys"}>{keyContent}</section>
      <section hidden={settingsSection !== "connection"}>
        <h3>Browser session</h3>
        <p className="setting-hint">
          {store.session instanceof Session && store.session.host
            ? "Connection tab — keep this tab open while using other KelpMux windows."
            : "Using the SSH connection held by another KelpMux tab."}{" "}
          After {minutesText(idle.warning)} without interaction every window
          warns, and the shared session locks {minutesText(idle.lock)} later.
          Logging out disconnects every shared window; terminals keep running.
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
