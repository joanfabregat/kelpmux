import type React from "react";
import {
  sshKey,
  register,
  selectExistingKey,
  registeredKeys,
} from "./terminal/connection";
import { unbase64 } from "./terminal/ssh/wire.mjs";
import type { Connection, Identity } from "./terminal/types";
import { KeyMark } from "./KeyMark";
import { keyColors, keyEmojis, keyColor } from "./terminal/key-style";
import { useToast } from "./Toast";
import { Icon } from "./status";

export type KeySettingsProps = {
  /** `register`: the sign-in page's registration dialog only. */
  mode: "register" | "settings";
  online: boolean;
  signedInKey: Pick<Identity, "credentialId" | "label"> | null;
  knownKeys: Identity[];
  identity: Identity | null;
  connection: Connection | null;
  connecting: boolean;
  error: string;
  setError: (value: string) => void;
  fail: (error: unknown) => void;
  keyBusy: boolean;
  setKeyBusy: (value: boolean) => void;
  keySelected: boolean;
  setKeySelected: (value: boolean) => void;
  shownKey: string;
  setShownKey: (value: string) => void;
  keyToDelete: string;
  setKeyToDelete: (value: string) => void;
  savedIdentities: () => Identity[];
  saveIdentity: (id: Identity) => void;
  updateKey: (
    id: string,
    patch: Partial<Pick<Identity, "label" | "emoji" | "color">>,
    sync?: boolean,
  ) => void;
  /** Called when a key's display profile is committed (no-op in KelpMux: profiles stay in this browser). */
  syncKey: (id: string) => void;
  preferKey: (key: Identity) => void;
  deleteKey: (id: string) => void;
};

/** Security keys cached in this browser (Settings → Security keys), and the
 * sign-in screen's registration dialog. */
export function KeySettings({
  mode,
  online,
  signedInKey,
  knownKeys,
  identity,
  connection,
  connecting,
  error,
  setError,
  fail,
  keyBusy,
  setKeyBusy,
  keySelected,
  setKeySelected,
  shownKey,
  setShownKey,
  keyToDelete,
  setKeyToDelete,
  savedIdentities,
  saveIdentity,
  updateKey,
  syncKey,
  preferKey,
  deleteKey,
}: KeySettingsProps) {
  const toast = useToast();
  const shownIdentity =
    knownKeys.find((key) => key.credentialId === shownKey) || identity;
  const signedKeyIndex = knownKeys.findIndex(
    (key) => key.credentialId === signedInKey?.credentialId,
  );
  const signedKeyName = signedInKey
    ? knownKeys[signedKeyIndex]?.label ||
      signedInKey.label ||
      (signedKeyIndex >= 0
        ? `Security key ${signedKeyIndex + 1}`
        : "Key removed from this browser")
    : "Checking connection key…";
  const registerButton = (
    <button
      disabled={!connection || connecting || keyBusy}
      onClick={async () => {
        setError("");
        setKeyBusy(true);
        setKeySelected(false);
        try {
          if (knownKeys.length >= 32)
            throw Error("This browser supports up to 32 saved keys.");
          const id = await register(connection!);
          saveIdentity(id);
          toast("Key registration saved", { key: "keys" });
        } catch (error) {
          fail(error);
        } finally {
          setKeyBusy(false);
        }
      }}
    >
      Register new key
    </button>
  );
  const busyHint = keyBusy && (
    <p>
      Choose your YubiKey in the browser prompt and touch it when requested.
    </p>
  );
  const publicKey = keySelected && shownIdentity && connection && (
    <section>
      <h2>Public key to authorize</h2>
      <p>
        Add this public key through your SSH key manager or cloud console, then
        sign in. The username is included at the end of the key.
      </p>
      <textarea
        className="key-entry"
        readOnly
        value={`${sshKey(shownIdentity)} ${connection.username}`}
        aria-label="Public SSH key"
      />
      <button
        className="subtle"
        onClick={() =>
          navigator.clipboard
            .writeText(`${sshKey(shownIdentity)} ${connection.username}`)
            .catch(fail)
        }
      >
        <Icon name="copy" size="sm" />
        Copy public key
      </button>
    </section>
  );
  if (mode === "register")
    return (
      <div className="key-modal-body">
        {error && <div className="notice error">{error}</div>}
        <section>
          <p>
            {keySelected && identity
              ? "Your new key is registered. Authorize its public key below."
              : `Create a credential for this website on your security key, then authorize its public key for ${connection?.username || "this account"}. A key that is already authorized needs no registration: close this dialog and choose Sign in with security key.`}
          </p>
          <div className="actions">{registerButton}</div>
          {busyHint}
        </section>
        {publicKey}
      </div>
    );
  return (
    <div className="key-modal-body">
      {online && (
        <div className="signed-key-summary">
          <strong>
            <Icon name="circle-check" size="sm" />
            Connection signed in with: {signedKeyName}
          </strong>
          <p>
            Changing the preferred key applies to your next sign-in. It does not
            change this connection.
          </p>
        </div>
      )}
      {error && <div className="notice error">{error}</div>}
      <section>
        <h2>Security keys</h2>
        <p>
          {keySelected && identity
            ? "Your registered key is ready. Its public key is shown below."
            : "Keys that sign in from this browser are remembered here automatically. Names, icons and colours are saved in this browser only."}
        </p>
        <div className="actions">
          <button
            className="subtle"
            disabled={!connection || connecting || keyBusy}
            onClick={async () => {
              setError("");
              setKeyBusy(true);
              setKeySelected(false);
              try {
                const saved = savedIdentities();
                if (
                  identity &&
                  !saved.some(
                    (item) => item.credentialId === identity.credentialId,
                  )
                )
                  saved.push(identity);
                saveIdentity(await selectExistingKey(connection!, saved));
                toast("Existing key selected", { key: "keys" });
              } catch (error) {
                fail(error);
              } finally {
                setKeyBusy(false);
              }
            }}
          >
            Use existing key
          </button>
          {registerButton}
        </div>
        {busyHint}
        {knownKeys.length > 0 && (
          <div className="saved-keys">
            <h3>Saved keys ({knownKeys.length})</h3>
            <p className="muted">
              Choose the preferred key for your next sign-in. The connection
              badge identifies the key used by your current SSH session.
            </p>
            <div className="saved-key-list">
              {knownKeys.map((key, index) => (
                <div
                  className="saved-key list-row"
                  key={key.credentialId}
                  style={
                    {
                      "--key-color": keyColor(key.color),
                    } as React.CSSProperties
                  }
                >
                  <div className="key-settings-main">
                    <label>
                      <span className="key-name-caption">
                        <KeyMark identity={key} />
                        Key {index + 1}
                      </span>
                      <input
                        aria-label={`Saved key name ${index + 1}`}
                        maxLength={40}
                        placeholder={`Security key ${index + 1}`}
                        value={key.label || ""}
                        disabled={keyBusy || connecting}
                        onFocus={(event) => {
                          event.currentTarget.dataset.before =
                            event.currentTarget.value;
                        }}
                        onBlur={(event) => {
                          if (
                            event.currentTarget.dataset.before !==
                            event.currentTarget.value
                          ) {
                            syncKey(key.credentialId);
                            toast("Key name saved", { key: "key-name" });
                          }
                        }}
                        onChange={(event) =>
                          updateKey(key.credentialId, {
                            label: event.target.value
                              .replace(/[\x00-\x1f\x7f]/g, "")
                              .slice(0, 40),
                          })
                        }
                      />
                      <span className="key-badges">
                        {online &&
                          signedInKey?.credentialId === key.credentialId && (
                            <span className="badge accent key-current">
                              Used for this connection
                            </span>
                          )}
                        {identity?.credentialId === key.credentialId && (
                          <span className="badge key-preferred">
                            Preferred for next sign-in
                          </span>
                        )}
                      </span>
                    </label>
                    <div className="key-appearance">
                      <label>
                        Icon
                        <select
                          aria-label={`Saved key icon ${index + 1}`}
                          value={key.emoji || ""}
                          disabled={keyBusy || connecting}
                          onChange={(e) => {
                            updateKey(
                              key.credentialId,
                              { emoji: e.target.value },
                              true,
                            );
                            toast("Key icon saved", { key: "key-icon" });
                          }}
                        >
                          {keyEmojis.map((emoji) => (
                            <option key={emoji.value} value={emoji.value}>
                              {emoji.value} {emoji.label}
                            </option>
                          ))}
                        </select>
                      </label>
                      <fieldset>
                        <legend>Color</legend>
                        <div className="key-colors">
                          {Object.entries(keyColors).map(([name, color]) => (
                            <button
                              type="button"
                              key={name}
                              className="key-color"
                              style={
                                { "--swatch": color } as React.CSSProperties
                              }
                              aria-label={`Key ${index + 1} color ${name}`}
                              aria-pressed={(key.color || "blue") === name}
                              disabled={keyBusy || connecting}
                              onClick={() => {
                                updateKey(
                                  key.credentialId,
                                  { color: name },
                                  true,
                                );
                                toast("Key color saved", { key: "key-color" });
                              }}
                            />
                          ))}
                        </div>
                      </fieldset>
                    </div>
                  </div>
                  <div className="key-actions">
                    {identity?.credentialId !== key.credentialId && (
                      <button
                        className="subtle small"
                        disabled={keyBusy || connecting}
                        onClick={() => preferKey(key)}
                      >
                        Use for next sign-in
                      </button>
                    )}
                    <button
                      type="button"
                      className="subtle small"
                      disabled={keyBusy || connecting}
                      onClick={() => {
                        setShownKey(key.credentialId);
                        setKeySelected(true);
                      }}
                    >
                      {shownIdentity?.credentialId === key.credentialId &&
                      keySelected
                        ? "Key shown"
                        : "Show public key"}
                    </button>
                    {keyToDelete === key.credentialId ? (
                      <div className="key-delete-confirm">
                        <p>
                          Remove this key from this browser and delete its
                          shared name, icon and colour? This neither revokes its
                          SSH authorization nor removes the credential from the
                          authenticator, and your current connection is
                          unchanged.
                        </p>
                        <button
                          className="subtle small"
                          onClick={() => setKeyToDelete("")}
                        >
                          Cancel
                        </button>
                        <button
                          className="danger small"
                          onClick={() => deleteKey(key.credentialId)}
                        >
                          Remove saved key
                        </button>
                      </div>
                    ) : (
                      <button
                        className="danger small"
                        disabled={keyBusy || connecting}
                        onClick={() => setKeyToDelete(key.credentialId)}
                      >
                        Delete
                      </button>
                    )}
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}
        <details>
          <summary>Use your keys in another browser</summary>
          <p>
            Save the registration file here, then import it in the other
            browser. The private key stays on your YubiKey.
          </p>
          <div className="actions">
            <button
              className="subtle"
              disabled={!connection || connecting || keyBusy}
              onClick={() =>
                document.getElementById("import-connection")?.click()
              }
            >
              <Icon name="upload" size="sm" />
              Import registration
            </button>
            {identity && (
              <button
                className="subtle"
                onClick={() => {
                  const url = URL.createObjectURL(
                    new Blob([JSON.stringify(identity, null, 2)], {
                      type: "application/json",
                    }),
                  );
                  const a = document.createElement("a");
                  a.href = url;
                  a.download = "kelpmux-registration.json";
                  a.click();
                  setTimeout(() => URL.revokeObjectURL(url), 1000);
                }}
              >
                <Icon name="download" size="sm" />
                Save registration
              </button>
            )}
            {knownKeys.length > 1 && (
              <button
                type="button"
                className="subtle"
                onClick={() => {
                  const url = URL.createObjectURL(
                    new Blob(
                      [
                        JSON.stringify(
                          { version: 1, keys: knownKeys },
                          null,
                          2,
                        ),
                      ],
                      { type: "application/json" },
                    ),
                  );
                  const link = document.createElement("a");
                  link.href = url;
                  link.download = "kelpmux-registrations.json";
                  link.click();
                  setTimeout(() => URL.revokeObjectURL(url), 1000);
                }}
              >
                <Icon name="download" size="sm" />
                Save all registrations
              </button>
            )}
          </div>
        </details>
        <input
          hidden
          id="import-connection"
          type="file"
          accept="application/json"
          onChange={async (e) => {
            try {
              const f = e.target.files?.[0];
              if (!f || f.size > 131072)
                throw Error("Invalid registration file");
              const value = JSON.parse(await f.text());
              const imported =
                value.version === 1 && Array.isArray(value.keys)
                  ? value.keys
                  : [value];
              if (!imported.length || imported.length > 32)
                throw Error("Import between 1 and 32 registrations.");
              for (const id of imported) {
                if (
                  id.version !== 1 ||
                  id.rpId !== connection?.rpId ||
                  id.origin !== connection?.origin ||
                  unbase64(id.point).length !== 65 ||
                  unbase64(id.credentialId).length === 0 ||
                  unbase64(id.credentialId).length > 1024
                )
                  throw Error("Credential does not belong to this server");
                await crypto.subtle.importKey(
                  "raw",
                  unbase64(id.point),
                  { name: "ECDSA", namedCurve: "P-256" },
                  false,
                  ["verify"],
                );
              }
              const valid = registeredKeys(connection!, imported);
              if (valid.length !== imported.length)
                throw Error("Invalid or duplicate registrations.");
              const merged = registeredKeys(
                connection!,
                savedIdentities(),
                identity,
              );
              if (
                new Set([...merged, ...valid].map((key) => key.credentialId))
                  .size > 32
              )
                throw Error("This browser supports up to 32 saved keys.");
              for (const id of valid) saveIdentity(id);
              toast("Key registrations imported", { key: "keys" });
              setError("");
            } catch (error) {
              fail(error);
            }
          }}
        />
      </section>
      {publicKey}
    </div>
  );
}
