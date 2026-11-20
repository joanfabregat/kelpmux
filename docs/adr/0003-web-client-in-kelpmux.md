# ADR 0003: A terminals-only web client in KelpMux

- Status: Accepted.
- Date: 2026-11-20 (America/Los_Angeles).
- Related: [ADR 0001](0001-native-client-protocol-and-device-capabilities.md), [ADR 0002](0002-four-clients-and-web-ssh.md), [web client guide](../web.md).

## Context

The browser client, its SSH bridge and its hosting were developed in KelpHub, the private web application, which grew project, workspace, Git, file and agent management around the terminal. KelpMux, the public terminal foundation, kept only the generic SSH byte relay. Users of the open-source KelpMux had no browser access to their workspaces, and the reusable terminal pieces lived inside a private application tangled with management code.

ADR 0002 established the browser architecture this decision keeps: browser-terminated OpenSSH over a WSS byte relay, WebAuthn-backed `sk-ecdsa` keys, an operator-pinned host key, and a portable WASM terminal core.

## Decision

1. KelpMux ships a web client. It is the open-source "simple" version: terminals only. It has no projects or workspace management, no GitHub integration, no file browser or editor, no Git views and no uploads, and its settings cover only appearance, keyboard shortcuts, security keys and the browser session. Those product features stay in KelpHub.
2. The executable gains `kelpmux web-bridge --workspace NAME`, a terminal-only stdio bridge that OpenSSH runs for the authenticated user. It attaches to an existing workspace socket with a same-user peer check, relays the independent pane stream and offers only `state`, `new`, `close`, `search`, `history` and `status`. It has no TCP listener and never starts or restarts a workspace server. `kelpmux web-relay --config PATH` serves the client's assets, embedded at build time from `web/dist`, through the existing `kelpserver` relay. A build without `web/dist` still builds and runs the TUI; the relay then refuses to start with an explicit missing-assets error.
3. The reusable terminal pieces live in `web/src/terminal/`, with no imports from application code: SSH/WebAuthn connection and key discovery, the shared-connection worker, WASM core loading, renderer, keyboard/mouse/wheel input, selection, screen cache, grid columns, the terminal components and pane find. KelpHub may consume this directory at a pinned KelpMux revision together with the `client-core.wasm` built from that revision. It is not published to npm yet.
4. The TUI and the web client are independent clients of the same KelpServer. A user can run either alone or both together on the same panes and processes. Layout (tabs, grid, order and focus) is per client: the web client stores it per browser in `localStorage`, and neither client changes the other's layout. KelpMux gets no database for web state.
5. Pane input and size follow the existing per-pane leases: whichever client last focused or typed into a pane holds its input and its single PTY size; the TUI controller's own input takes a pane back and restores its geometry. No server or protocol change was needed for this; the composed TUI protocol stays at version 58 and the independent stream at version 1, compatible with existing TUI clients and with KelpHub's pinned revision.
6. TUI feature development stays paused. The web client adds no TUI features.

## Consequences

- KelpMux users get browser access with hardware-key SSH, without KelpHub. KelpHub keeps its management features and can reuse the terminal component instead of maintaining a private copy.
- The public repository now carries a JavaScript dependency graph (React, Vite, TypeScript, Playwright for tests) and pinned third-party WASSH/OpenSSH WASM assets. CI builds the WASM core, fetches and verifies the SSH assets, installs locked npm dependencies without scripts, audits them, runs unit and WASM tests, type-checks, builds and embeds the result. The OpenSSL copy inside the upstream OpenSSH WASM archive still needs its own review before exposure beyond private networks.
- Creating and closing terminals from the browser affects every client; this is shared state by nature. The browser's close goes through the server's existing confirmation step and returns the pane's lease first.
- Renaming and splitting panes are not offered: the server has no rename operation and its layout is automatic. Adding them would be a protocol change for a later decision.
- The bridge's `Web` request API is versioned (`api_version` 1) and deliberately small. KelpHub's management bridge remains separate; the two share only the stream protocol and the terminal component.
