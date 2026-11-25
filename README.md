# KelpMux

KelpMux is the open-source terminal interface for persistent workspaces. This repository contains the TUI, the terminals-only web client, and the components they share with KelpHub: the terminal server (KelpServer), the protocol crate, the Rust/WASM client core and the browser terminal layer. The TUI provides grid and sidebar views, agent activity, parking, recently closed sessions, recovery and system metrics. Detaching leaves shells and agents running.

TUI feature development is frozen while the separately maintained KelpHub web application evolves. Compatibility, security and necessary fixes remain in scope; feature parity with the web application is not required.

## Architecture

| Package | Location | Responsibility |
| --- | --- | --- |
| `kelpmux` | `src/`, `web/` | Terminal interface, web client (bridge, relay assets and browser UI) and optional legacy tmux adapter |
| `kelpserver` | [crates/server](crates/server) | Library and `kelpserver` executable: PTYs, authoritative terminal state (Alacritty's engine), persistent sessions, the pane stream and its session API, the SSH bridge |
| `kelpclient-protocol` | [crates/protocol](crates/protocol) | Portable wire types, the stream version constant, validation and the binary screen codec, with no server, client-state or WASM dependency |
| `kelpclient` | [crates/client](crates/client) | Shared terminal client state, selection/history utilities and WASM bindings, used natively by the TUI and compiled to `client-core.wasm` for browsers |
| `@kelpcore/web` (npm, private) | [web-core](web-core) | Browser terminal layer: renderer, in-browser SSH, the shared worker session, input encoding, the `Terminal` React component and the file browser client and editor |

The four crates form one Cargo workspace and the two npm packages (`web`, `web-core`) one npm workspace with a single root `package-lock.json`, so the server, the TUI, `client-core.wasm` and the browser layer always come from the same commit. KelpServer has no dependency on any client; its stream contract is in [docs/server/native-client-protocol.md](docs/server/native-client-protocol.md), its session API in [docs/server/session-api.md](docs/server/session-api.md) and its internals in [docs/server/architecture.md](docs/server/architecture.md). The shared components lived in the separate KelpCore repository until they were merged back here with their history ([ADR 0004](docs/adr/0004-kelpcore-merged-into-kelpmux.md)); KelpCore's ADRs are kept in [docs/adr/core](docs/adr/core). The `web/` directory holds the terminals-only browser client (see [docs/web.md](docs/web.md)). KelpHub owns the full web application with project management, authenticated management backend and its hosting. See [MIGRATION.md](MIGRATION.md) for history provenance and the removal of the composed-screen protocol.

## Build and run

```sh
cargo build --workspace --release --locked
cargo build -p kelpserver --bin kelpserver --release --locked
cargo test --workspace --locked
```

The second command builds the KelpServer executable from `crates/server` into `target/release/kelpserver`. The native lifecycle tests start `kelpserver` from beside the test build of `kelpmux`, so build it for the test profile too (`cargo build -p kelpserver --bin kelpserver --locked`) before `cargo test`. Install `target/release/kelpmux` and `target/release/kelpserver` together, or configure `KELPSERVER_BINARY` with the server path. Build the portable browser core with `cargo build -p kelpclient --release --locked --target wasm32-unknown-unknown` after installing that Rust target. Run builds in your environment's approved container workflow. Release archives include both executables.

```sh
kelpmux start --workspace trial
kelpmux attach --workspace trial
```

`kelpmux attach` runs the TUI, a client of KelpServer's pane stream that renders locally and keeps its own UI state in `tui-state.json`. KelpMux requires KelpServer 0.3 or later, which speaks only the pane stream (the composed-screen protocol 58 is gone) and starts every workspace without panes: the first TUI or web client creates the tabs from its saved layout or the workspace's template. The former composed TUI was removed: `--legacy` is refused, `--stream` is accepted without effect, and `--startup-panes` accepts only `none`. `kelpmux ctl`, `stop` and the web bridge use the same stream session API. See [the native guide](docs/native.md#the-tui-a-pane-stream-client) for the parity table and the one-time migration.

`Ctrl-B`, then `d` detaches. `g` selects grid, `z` reopens the latest useful closed tab, `p` parks a tab and `u` shows parked tabs. Stop a disposable workspace with `kelpmux stop --workspace trial`; stopping ends its processes. Existing settings and XDG state paths are preserved. A server crash or reboot requires recovery/resume; metadata does not preserve live processes.

The local checkout is `~/repos/kelp/mux`. Source operations do not replace running dev or pre-prod installations. The legacy [native guide](docs/native.md) describes the retained interaction model; component ownership/build instructions in MIGRATION.md supersede its combined-binary packaging assumptions.

## Web client

KelpMux also has an optional browser client for the same workspaces: terminals in tabs or a live grid, search, selection and copy, files and images dropped onto a terminal uploaded to the host with their paths pasted into it, a file browser and text editor for the server user's files, themes and fonts, signed in with a hardware security key through browser-side OpenSSH. `kelpmux web-relay --config PATH` serves it and relays SSH bytes; OpenSSH runs `kelpmux web-bridge --workspace NAME` for the signed-in user. The TUI and browsers can be used alone or together on the same panes, each with its own layout. Building it, relay configuration, YubiKey registration and the security boundaries are in [docs/web.md](docs/web.md); [ADR 0003](docs/adr/0003-web-client-in-kelpmux.md) records the scope. A build without the web assets still builds and runs the TUI.

User settings live in `~/.config/kelpmux/config.toml`, edited by hand, by agents or in the web client's Settings, and applied live; `kelpmux config check` validates it. Every key, and how agents should change it, is documented in [docs/config.md](docs/config.md).

## Security and contributions

Use [SECURITY.md](SECURITY.md) for private vulnerability reporting. Follow [CONTRIBUTING.md](CONTRIBUTING.md) and [repository security policy](docs/repository-security.md). Licensed under [Apache-2.0](LICENSE); see NOTICE for attribution.
