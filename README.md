# KelpMux

KelpMux is the open-source terminal interface to persistent Kelp terminal workspaces. It provides grid and sidebar views, agent activity, parking, recently closed sessions, recovery and system metrics. Detaching leaves shells and agents running.

Feature development is frozen while the web-first KelpHub application evolves. Compatibility, security and necessary fixes remain in scope; feature parity with the web application is not required.

## Components

- [KelpServer](https://github.com/joanfabregat/kelpserver) owns PTYs, authoritative terminal state and persistent sessions.
- [KelpClient](https://github.com/joanfabregat/kelpclient) provides shared protocol/types and Rust/WASM terminal client libraries.
- KelpHub provides the separately maintained web interface and management service.

This repository retains the existing TUI and optional legacy tmux adapter. It no longer owns the terminal server implementation or browser/management code. See [MIGRATION.md](MIGRATION.md) for source-history provenance and the intentionally retained composed-screen compatibility boundary.

## Build and run

Build/install `kelpserver` alongside `kelpmux`, or configure `KELPSERVER_BINARY`. See [MIGRATION.md](MIGRATION.md) for build commands and validation requirements. No split-component release has been published yet.

```sh
kelpmux start --workspace trial
kelpmux attach --workspace trial
```

`Ctrl-B`, then `d` detaches. `g` selects grid, `z` reopens the latest useful closed tab, `p` parks a tab and `u` shows parked tabs. Stop a disposable workspace with `kelpmux stop --workspace trial`; stopping ends its processes. Existing settings and XDG state paths are preserved. A server crash or reboot requires recovery/resume; metadata does not preserve live processes.

The local checkout is `~/repos/kelp/mux`. Source operations do not replace running dev or pre-prod installations. The legacy [native guide](docs/native.md) describes the retained interaction model; component ownership/build instructions in MIGRATION.md supersede its combined-binary packaging assumptions.

## Security and contributions

Use [SECURITY.md](SECURITY.md) for private vulnerability reporting. Follow [CONTRIBUTING.md](CONTRIBUTING.md) and [repository security policy](docs/repository-security.md). Licensed under [Apache-2.0](LICENSE); see NOTICE for attribution.
