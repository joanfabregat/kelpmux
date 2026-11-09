# KelpMux

KelpMux is the open-source terminal foundation for persistent workspaces. This repository contains the terminal server, shared Rust/WASM client libraries and TUI. The TUI provides grid and sidebar views, agent activity, parking, recently closed sessions, recovery and system metrics. Detaching leaves shells and agents running.

TUI feature development is frozen while the separately maintained KelpHub web application evolves. Compatibility, security and necessary fixes remain in scope; feature parity with the web application is not required.

## Workspace

| Package | Location | Responsibility |
| --- | --- | --- |
| `kelpmux` | `src/` | Terminal interface and optional legacy tmux adapter |
| `kelpserver` | [crates/server](crates/server) | PTYs, authoritative terminal state, persistent sessions and generic SSH byte relay |
| `kelpclient` | [crates/client](crates/client) | Shared terminal state, selection/history utilities and WASM bindings |
| `kelpclient-protocol` | [crates/protocol](crates/protocol) | Portable wire types and validation |

These are modules in one Cargo workspace, not separate source repositories. KelpHub owns the web interface, authenticated management backend, uploads and application hosting. It depends on this workspace at an immutable Git revision. See [MIGRATION.md](MIGRATION.md) for history provenance and the intentionally retained composed-screen TUI compatibility boundary.

## Build and run

```sh
cargo build --workspace --release --locked
cargo test --workspace --locked
```

Install `target/release/kelpmux` and `target/release/kelpserver` together, or configure `KELPSERVER_BINARY` with the server path. Build the portable browser core with `cargo build -p kelpclient --release --locked --target wasm32-unknown-unknown` after installing that Rust target. Run builds in your environment's approved container workflow. Version `0.2.0-beta.7` is the first beta packaged from this consolidated workspace; its Linux archives include both executables.

```sh
kelpmux start --workspace trial
kelpmux attach --workspace trial
```

`Ctrl-B`, then `d` detaches. `g` selects grid, `z` reopens the latest useful closed tab, `p` parks a tab and `u` shows parked tabs. Stop a disposable workspace with `kelpmux stop --workspace trial`; stopping ends its processes. Existing settings and XDG state paths are preserved. A server crash or reboot requires recovery/resume; metadata does not preserve live processes.

The local checkout is `~/repos/kelp/mux`. Source operations do not replace running dev or pre-prod installations. The legacy [native guide](docs/native.md) describes the retained interaction model; component ownership/build instructions in MIGRATION.md supersede its combined-binary packaging assumptions.

## Security and contributions

Use [SECURITY.md](SECURITY.md) for private vulnerability reporting. Follow [CONTRIBUTING.md](CONTRIBUTING.md) and [repository security policy](docs/repository-security.md). Licensed under [Apache-2.0](LICENSE); see NOTICE for attribution.
