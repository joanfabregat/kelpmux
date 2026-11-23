# KelpMux

KelpMux is the open-source terminal interface for persistent workspaces. This repository contains the TUI and the terminals-only web client. The terminal server ([KelpServer](https://github.com/joanfabregat/kelpcore)), the protocol crate, the shared Rust/WASM client library and the browser terminal layer live in the [KelpCore](https://github.com/joanfabregat/kelpcore) repository and are pinned dependencies. The TUI provides grid and sidebar views, agent activity, parking, recently closed sessions, recovery and system metrics. Detaching leaves shells and agents running.

TUI feature development is frozen while the separately maintained KelpHub web application evolves. Compatibility, security and necessary fixes remain in scope; feature parity with the web application is not required.

## Workspace

| Package | Location | Responsibility |
| --- | --- | --- |
| `kelpmux` | `src/`, `web/` | Terminal interface, web client (bridge, relay assets and browser UI) and optional legacy tmux adapter |
| `kelpclient` | [joanfabregat/kelpcore](https://github.com/joanfabregat/kelpcore) (Git dependency) | Shared terminal state, selection/history utilities and WASM bindings |
| `kelpserver` | [joanfabregat/kelpcore](https://github.com/joanfabregat/kelpcore) (Git dependency, 0.3) | PTYs, authoritative terminal state, persistent sessions and the pane stream session API |
| `kelpclient-protocol` | [joanfabregat/kelpcore](https://github.com/joanfabregat/kelpcore) (Git dependency) | Portable wire types and validation |

The `web/` directory holds the terminals-only browser client (see [docs/web.md](docs/web.md)); its terminal layer is the `@kelpcore/web` npm package from KelpCore, pinned in `web/package.json`. KelpHub owns the full web application with project management, authenticated management backend and its hosting. See [MIGRATION.md](MIGRATION.md) for history provenance and the removal of the composed-screen protocol.

### KelpCore pin

`kelpserver`, `kelpclient-protocol` and `kelpclient` are declared once, in `[workspace.dependencies]` of the root `Cargo.toml`, with the identical KelpCore Git URL and `rev`. Keep the URL string and revision identical for all three, or Cargo builds two protocol crates. The browser's `client-core.wasm` (built from `kelpclient`) must use protocol types from the same KelpCore revision as the server it talks to. The web client's `@kelpcore/web` dependency in `web/package.json` pins the same KelpCore commit. To move to a newer KelpCore, change the `rev` of all three entries, run `cargo update -p kelpserver -p kelpclient-protocol -p kelpclient`, update the `@kelpcore/web` commit and `web/package-lock.json` (`npm install --ignore-scripts --package-lock-only`), and review and audit both lockfile changes.

## Build and run

```sh
cargo build --workspace --release --locked
cargo build -p kelpserver --bin kelpserver --release --locked
cargo test --workspace --locked
```

The second command builds the pinned KelpServer executable from the locked dependency into `target/release/kelpserver`. The native lifecycle tests start `kelpserver` from beside the test build of `kelpmux`, so build it for the test profile too (`cargo build -p kelpserver --bin kelpserver --locked`) before `cargo test`. Install `target/release/kelpmux` and `target/release/kelpserver` together, or configure `KELPSERVER_BINARY` with the server path. Build the portable browser core with `cargo build -p kelpclient --release --locked --target wasm32-unknown-unknown` after installing that Rust target. Run builds in your environment's approved container workflow. Release archives include both executables.

```sh
kelpmux start --workspace trial
kelpmux attach --workspace trial
```

`kelpmux attach` runs the TUI, a client of KelpServer's pane stream that renders locally and keeps its own UI state in `tui-state.json`. KelpMux requires KelpServer 0.3 or later, which speaks only the pane stream (the composed-screen protocol 58 is gone) and starts every workspace without panes: the first TUI or web client creates the tabs from its saved layout or the workspace's template. The former composed TUI was removed: `--legacy` is refused, `--stream` is accepted without effect, and `--startup-panes` accepts only `none`. `kelpmux ctl`, `stop` and the web bridge use the same stream session API. See [the native guide](docs/native.md#the-tui-a-pane-stream-client) for the parity table and the one-time migration.

`Ctrl-B`, then `d` detaches. `g` selects grid, `z` reopens the latest useful closed tab, `p` parks a tab and `u` shows parked tabs. Stop a disposable workspace with `kelpmux stop --workspace trial`; stopping ends its processes. Existing settings and XDG state paths are preserved. A server crash or reboot requires recovery/resume; metadata does not preserve live processes.

The local checkout is `~/repos/kelp/mux`. Source operations do not replace running dev or pre-prod installations. The legacy [native guide](docs/native.md) describes the retained interaction model; component ownership/build instructions in MIGRATION.md supersede its combined-binary packaging assumptions.

## Web client

KelpMux also has an optional browser client for the same workspaces: terminals in tabs or a live grid, search, selection and copy, files and images dropped onto a terminal uploaded to the host with their paths pasted into it, a file browser and text editor for the server user's files, themes and fonts, signed in with a hardware security key through browser-side OpenSSH. `kelpmux web-relay --config PATH` serves it and relays SSH bytes; OpenSSH runs `kelpmux web-bridge --workspace NAME` for the signed-in user. The TUI and browsers can be used alone or together on the same panes, each with its own layout. Building it, relay configuration, YubiKey registration and the security boundaries are in [docs/web.md](docs/web.md); [ADR 0003](docs/adr/0003-web-client-in-kelpmux.md) records the scope. A build without the web assets still builds and runs the TUI.

## Security and contributions

Use [SECURITY.md](SECURITY.md) for private vulnerability reporting. Follow [CONTRIBUTING.md](CONTRIBUTING.md) and [repository security policy](docs/repository-security.md). Licensed under [Apache-2.0](LICENSE); see NOTICE for attribution.
