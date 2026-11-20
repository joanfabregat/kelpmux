import type React from "react";
import type { Connection, Identity } from "./terminal/types";
import { KeyMark } from "./KeyMark";
import { keyColor } from "./terminal/key-style";
import { Icon } from "./status";

/** KelpMux mark and wordmark, the same look as the rail header. */
function LoginLogo() {
  return (
    <span className="logo">
      <span className="logo-mark" aria-hidden="true">
        <Icon name="layers" size="sm" />
      </span>
      <span className="logo-text">KelpMux</span>
    </span>
  );
}

export function PopupConnecting({ onFallback }: { onFallback: () => void }) {
  return (
    <main className="popup-connecting" role="status" aria-live="polite">
      <LoginLogo />
      <span className="connecting-spinner" aria-hidden="true" />
      <h1>Connecting to your tab…</h1>
      <p>Reusing your open KelpMux connection.</p>
      <button className="subtle" onClick={() => onFallback()}>
        Sign in instead
      </button>
    </main>
  );
}

export type LoginScreenProps = {
  connection: Connection | null;
  /** The last key used in this browser, validated against the cache. */
  remembered: Identity | null;
  error: string;
  status: string;
  connecting: boolean;
  keyBusy: boolean;
  keyModal: boolean;
  keyManagement: React.ReactNode;
  sign: { run: () => Promise<void>; cancel: () => void } | null;
  signBusy: boolean;
  onSignIn: () => void;
  onDiscover: () => void;
  onRegister: () => void;
  onCancelConnection: () => void;
  onRetrySign: () => void;
  onCancelSign: () => void;
};

export function LoginScreen({
  connection,
  remembered,
  error,
  status,
  connecting,
  keyBusy,
  keyModal,
  keyManagement,
  sign,
  signBusy,
  onSignIn,
  onDiscover,
  onRegister,
  onCancelConnection,
  onRetrySign,
  onCancelSign,
}: LoginScreenProps) {
  const busy = !connection || connecting || keyBusy;
  return (
    <main className="login">
      <div className="login-card">
        <div className="login-brand">
          <LoginLogo />
          {connection?.workspace && (
            <span className="env-badge">{connection.workspace}</span>
          )}
        </div>
        <div className="login-head">
          <h1>Sign in</h1>
          <p className="login-target">
            <Icon name="terminal" size="sm" />
            <span>
              {connection?.username || "…"}@{connection?.workspace || "dev"}
            </span>
          </p>
        </div>
        {!keyModal && error && <div className="notice error">{error}</div>}
        {remembered ? (
          <button
            className="connect-primary key-login preferred"
            style={
              {
                "--key-color": keyColor(remembered.color),
              } as React.CSSProperties
            }
            aria-label="Sign in with security key"
            aria-describedby="remembered-key"
            disabled={busy}
            onClick={onSignIn}
            autoFocus
          >
            <KeyMark identity={remembered} />
            <span className="key-login-label">
              <strong>Sign in with security key</strong>
              <small id="remembered-key">
                {connecting
                  ? `Connecting with ${remembered.label || "your security key"}…`
                  : `Last used: ${remembered.label || "Security key"}`}
              </small>
            </span>
            <span className="key-login-arrow" aria-hidden="true">
              <Icon name="arrow-right" />
            </span>
          </button>
        ) : (
          <button
            className="connect-primary"
            disabled={busy}
            onClick={onSignIn}
          >
            <Icon name="key-round" />
            Sign in with security key
          </button>
        )}
        <p className="login-key-status">
          {remembered
            ? "Touch your security key when prompted. Using a different key? Choose Use another security key."
            : "Choose your security key, touch it to identify it, then touch it again to approve the SSH login."}
        </p>
        <div className="actions login-secondary">
          {remembered && (
            <button className="subtle" disabled={busy} onClick={onDiscover}>
              Use another security key
            </button>
          )}
          <button className="subtle" disabled={busy} onClick={onRegister}>
            Register a new key
          </button>
          {connecting && (
            <button className="ghost" onClick={onCancelConnection}>
              Cancel connection
            </button>
          )}
        </div>
        {keyManagement}
        {sign && (
          <div className="sign-prompt">
            <p>
              <Icon name="key-round" />
              <span>
                {signBusy
                  ? "Approve the security-key request in your browser."
                  : "Your browser may need another click to request your YubiKey signature."}
              </span>
            </p>
            <div className="actions">
              <button disabled={signBusy} onClick={onRetrySign}>
                Sign SSH login
              </button>
              <button className="subtle" onClick={onCancelSign}>
                Cancel
              </button>
            </div>
          </div>
        )}
        {status && <pre className="connection-status">{status}</pre>}
        <p className="footnote">
          Private keys stay on your YubiKey. Your sessions remain on the server
          when this browser closes. No web enrollment endpoint, password login
          or SSH-agent forwarding. This browser caches only public key data.
        </p>
      </div>
    </main>
  );
}
