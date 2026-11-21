# ADR 0003: A terminals-only web client in KelpMux

- Status: Accepted.
- Date: 2026-11-20 (America/Los_Angeles).
- Related: [ADR 0001](0001-native-client-protocol-and-device-capabilities.md), [ADR 0002](0002-four-clients-and-web-ssh.md), [web client guide](../web.md).

## Context

The browser client, its SSH bridge and its hosting were developed in KelpHub, the private web application, which grew project, workspace, Git, file and agent management around the terminal. KelpMux, the public terminal foundation, kept only the generic SSH byte relay. Users of the open-source KelpMux had no browser access to their workspaces, and the reusable terminal pieces lived inside a private application tangled with management code.

ADR 0002 established the browser architecture this decision keeps: browser-terminated OpenSSH over a WSS byte relay, WebAuthn-backed `sk-ecdsa` keys, an operator-pinned host key, and a portable WASM terminal core.

## Decision

1. KelpMux ships a web client. It is the open-source "simple" version: terminals only. It has no projects or workspace management, no GitHub integration, no file browser or editor, no Git views and no uploads, and its settings cover only appearance, keyboard shortcuts, security keys and the browser session. Those product features stay in KelpHub.
2. The executable gains `kelpmux web-bridge --workspace NAME`, a terminal-only stdio bridge that OpenSSH runs for the authenticated user. It attaches to an existing workspace socket with a same-user peer check, relays the independent pane stream and offers only `state`, `new`, `close`, `search`, `history` and `status`. It has no TCP listener and never starts or restarts a workspace server (refined in the addendum below: it starts its own workspace on an explicit user request). `kelpmux web-relay --config PATH` serves the client's assets, embedded at build time from `web/dist`, through the existing `kelpserver` relay. A build without `web/dist` still builds and runs the TUI; the relay then refuses to start with an explicit missing-assets error.
3. The reusable terminal pieces live in `web/src/terminal/`, with no imports from application code: SSH/WebAuthn connection and key discovery, the shared-connection worker, WASM core loading, renderer, keyboard/mouse/wheel input, selection, screen cache, grid columns, the terminal components and pane find. KelpHub may consume this directory at a pinned KelpMux revision together with the `client-core.wasm` built from that revision. It is not published to npm yet.
4. The TUI and the web client are independent clients of the same KelpServer. A user can run either alone or both together on the same panes and processes. Layout (tabs, grid, order and focus) is per client: the web client stores it per browser in `localStorage`, and neither client changes the other's layout. KelpMux gets no database for web state.
5. Pane input and size follow the existing per-pane leases: whichever client last focused or typed into a pane holds its input and its single PTY size; the TUI controller's own input takes a pane back and restores its geometry. No server or protocol change was needed for this; the composed TUI protocol stays at version 58 and the independent stream at version 1, compatible with existing TUI clients and with KelpHub's pinned revision.
6. TUI feature development stays paused. The web client adds no TUI features.

## Consequences

- KelpMux users get browser access with hardware-key SSH, without KelpHub. KelpHub keeps its management features and can reuse the terminal component instead of maintaining a private copy.
- The public repository now carries a JavaScript dependency graph (React, Vite, TypeScript, Playwright for tests) and pinned third-party WASSH/OpenSSH WASM assets. CI builds the WASM core, fetches and verifies the SSH assets, installs locked npm dependencies without scripts, audits them, runs unit and WASM tests, type-checks, builds and embeds the result. The OpenSSL copy inside the upstream OpenSSH WASM archive still needs its own review before exposure beyond private networks.
- Creating and closing terminals from the browser affects every client; this is shared state by nature. The browser's close goes through the server's existing confirmation step and returns the pane's lease first.
- Renaming and splitting panes are not offered: the server has no rename operation and its layout is automatic. Adding them would be a protocol change for a later decision.
- The bridge's `Web` request API is versioned (`api_version` 1, then 2 after the addendum below) and deliberately small. KelpHub's management bridge remains separate; the two share only the stream protocol and the terminal component.

## Addendum: TUI feature parity in the browser

Date: 2026-11-20 (America/Los_Angeles).

The owner asked for the web client to offer the TUI's feature set, adapted to the browser, with the same general interface (left sidebar, grid). This refines decision 6: the web client gains equivalents of existing TUI features; it still adds no product features of its own, and the TUI is unchanged. The [parity table](../web.md#feature-parity-with-the-tui) lists each TUI feature, its browser equivalent and what is deferred.

- Layout stays per client. Browser parking, terminal order, sidebar width and folding, and alert preferences are stored per browser in `localStorage`; they never change the TUI's layout, and KelpMux still has no database for web state.
- Notification read marks are workspace state, shared with the TUI. The bridge now forwards the stream's existing notification acknowledgements instead of refusing them.
- The bridge's `Web` API moves to version 2 with four operations: `color`, `closed`, `restore` and `start`. Recovery reads the same private history file as the TUI and restores through the existing idempotent `pane-spawn` request; it does not drive TUI dialogs or change the TUI's view. `color` sends the server's existing `TabColorSet` action, so tab colours stay shared with the TUI.
- The web client has no scratch terminal (owner decision). A scratch pane created in the TUI shows as an ordinary terminal; the TUI keeps its scratch popup.
- Decision 2's rule "never starts or restarts a workspace server" becomes: the bridge **starts the server only on an explicit user request, only for its configured workspace, and never replaces a running one** (owner decision). When the workspace socket is missing or nothing listens on it, the bridge answers the browser's stream hello with a `Workspace` notice and accepts only `state` and `start`; the browser shows a **Start workspace** button. `start` runs the same code path as `kelpmux start --workspace NAME` with the bridge's own state directory and socket; the request may carry no workspace, server binary or replacement option, and `start` is refused while a server is running. The bridge then connects as usual. It still never stops, restarts or replaces a server.
- KelpServer removes the starter's agent and multiplexer session markers (`CLAUDECODE`, `CLAUDE_CODE_*`, `CLAUDE_PID`, `CLAUDE_EFFORT`, `CODEX_SESSION_ID`, `CODEX_THREAD_ID`, `CODEX_TOOL_CALL_ID`, `CODEX_CI`, `TMUX`, `TMUX_PANE`, `STY`) from the environment of the server it starts and of every pane shell, so an agent in a pane does not attach to the session that started the workspace. User configuration such as API keys and `CLAUDE_CONFIG_DIR` is kept.
- Activity notifications treat an agent first seen waiting at a startup dialog, before it has a session, as baseline rather than a question, so starting an agent never notifies; later questions and completions still do, in the TUI and the browser alike.
- None of these server changes alters a protocol: the composed TUI protocol stays at version 58 and the stream at version 1.
