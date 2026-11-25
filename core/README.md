# KelpCore

KelpCore holds the server of the Kelp project and every component its applications share: the persistent, user-owned terminal server (KelpServer), the wire protocol, the Rust/WASM client core and the browser terminal layer. KelpMux and KelpHub each pin one KelpCore revision and keep only their own application (layout, navigation, product features); the decision and its boundary rule are in [ADR 0003](docs/adr/0003-kelpcore-shared-repository.md).

KelpServer owns PTYs, process lifecycle, authoritative terminal emulation (Alacritty's engine), scrollback, search, per-pane leases and sizes, closed-session records, recovery metadata and agent activity discovery. Clients own all presentation and UI state. Detaching or losing a client leaves shells and agents running. Recovery metadata cannot preserve live processes across a server crash or reboot.

This repository was previously KelpServer, extracted from [KelpMux](https://github.com/joanfabregat/kelpmux) at commit `eb31da5`; the client core and the browser layer were moved from KelpMux at commit `7179aa3`. The history before each move stays in KelpMux.

## Packages

| Package | Location | Responsibility |
| --- | --- | --- |
| `kelpserver` | [crates/server](crates/server) | Library and `kelpserver` executable: daemon, PTYs, terminal state, the stream protocol and its session API, SSH bridge |
| `kelpclient-protocol` | [crates/protocol](crates/protocol) | Portable wire types, the stream version constant, cell validation and the binary screen codec, with no server, client-state or WASM dependency (it builds for `wasm32-unknown-unknown`) |
| `kelpclient` | [crates/client](crates/client) | The Rust client core: screen snapshots and updates, revision tracking, framing, selection and history layout. Used natively by the TUI and compiled to WASM (`client-core.wasm`) for browsers |
| `@kelpcore/web` (npm, not published) | [web](web) | The browser layer both web clients use: WebGL/Canvas renderer, in-browser SSH with WebAuthn keys, the shared worker session, keyboard, mouse, wheel and touch-key encoding, uploads, the `Terminal` React component, and the file browser's request client, path and document models and CodeMirror 6 text editor (`files/`, derived from KelpHub). TypeScript sources, compiled by the consuming application's build |

The package and executable names are historical and kept so that existing consumers change only the repository URL of their pins.

## Clients and the contract

KelpServer has no dependency on any client. Its clients are:

- **KelpMux TUI** (the `kelpmux` executable): a pane-stream client that renders locally and keeps its own UI state, and launches or reuses the standalone `kelpserver` daemon. Since 0.3 ([ADR 0002](crates/server/docs/adr/0002-generic-session-server.md) step 3) the server has no composed-screen protocol or UI code.
- **KelpMux web client** (`kelpmux web-relay` / `kelpmux web-bridge`), built on `kelpclient` and `@kelpcore/web`: uses the independent pane stream, reached over SSH through a bridge command. KelpMux owns the browser relay, its assets and every presentation helper (tab numbers, colours, status glyphs and labels, vitals formatting, restore-choice labels).
- **KelpHub**: uses the library and the pane stream behind its own authenticated management adapter. Project, Git and management features stay in KelpHub (file uploads and file operations on the host where the shells run are generic stream capabilities, `uploads` and `files`); KelpServer is not a project manager, credential broker or container orchestrator.

The contract consists of:

| Surface | Version | Stability |
| --- | --- | --- |
| Independent pane stream (`Stream` hello/welcome, snapshots, patches, leases) and its session API (pane lifecycle, closed-session records, history and search, host status, server lifecycle, uploads, binary screens, files) | `STREAM_PROTOCOL_VERSION = 1` | Experimental and not yet frozen. Optional features are advertised as Welcome capabilities; clients must not assume undeclared ones, and must not send a request whose capability is missing. Events of new types reach only connections that opted in or sent the request, so existing v1 clients are unaffected. Unsupported versions fail clearly. See [the stream contract](crates/server/docs/native-client-protocol.md) and [the session API inventory](crates/server/docs/session-api.md). |
| Framing | 4-byte big-endian length + UTF-8 JSON, 16 MiB maximum; binary screen records (payload starting with `0x00`, format version 1) only for connections that opted in to `binary-screens` | Shared by the Unix socket and `kelpserver bridge` over SSH, which relays binary records unparsed. A record the server cannot accept, such as a message of the composed protocol 58 removed in 0.3, gets `{"Error":"…"}` and the connection closes. Binary screens are negotiated per connection after Welcome; other connections receive only JSON. See [the binary screen format](crates/server/docs/binary-screens.md). |
| Rust API of `kelpserver`, `kelpclient-protocol` and `kelpclient`; TypeScript API of `@kelpcore/web` | crate and package versions | Pre-1.0 and unstable; consumers pin an exact Git revision. |
| Executable and state | `kelpserver`, XDG state paths, `KELPMUX_*` environment names | Unchanged by the extraction. |

The stream version constant lives in [`crates/protocol/src/lib.rs`](crates/protocol/src/lib.rs). The design background is in [ADR 0001](crates/server/docs/adr/0001-native-client-protocol-and-device-capabilities.md), and the server's internals in [docs/architecture.md](docs/architecture.md), [docs/discovery.md](docs/discovery.md) and [docs/agent-polling.md](docs/agent-polling.md).

## Using it from an application

Pin one exact, reviewed commit for every package; never a branch.

Rust crates are Git dependencies:

```toml
[dependencies]
kelpserver = { git = "https://github.com/joanfabregat/kelpcore", rev = "<full commit SHA>" }
kelpclient-protocol = { git = "https://github.com/joanfabregat/kelpcore", rev = "<same SHA>" }
kelpclient = { git = "https://github.com/joanfabregat/kelpcore", rev = "<same SHA>" }
```

Ship a `kelpserver` executable built from that revision beside the client (or point `KELPSERVER_BINARY` at it). Build the browser core from the same revision with `cargo build -p kelpclient --release --locked --target wasm32-unknown-unknown` and serve `target/wasm32-unknown-unknown/release/kelpclient.wasm` as `/client-core.wasm`; a WASM core built from another revision may disagree with the server about the protocol.

The browser layer is an npm Git dependency at the same commit:

```json
{
  "dependencies": {
    "@kelpcore/web": "github:joanfabregat/kelpcore#<same SHA>"
  }
}
```

The package is private (never published) and exposes its TypeScript sources through `exports` (`"./*": "./web/src/*"`), so imports name the file with its extension, for example `@kelpcore/web/renderer.ts`, `@kelpcore/web/Terminal.tsx`, `@kelpcore/web/ssh/wire.mjs` and `@kelpcore/web/terminal.css`. The application's bundler compiles them; there is no build step in this package. `react` (19) is a peer dependency. The CodeMirror 6 and Lezer packages that `files/editor.tsx` imports are regular dependencies (exact versions), installed with the package; an application should load the editor lazily (`import("@kelpcore/web/files/editor.tsx")`) so they stay out of its main bundle, and ship their MIT license texts.

### What the application serves

The browser layer loads these at runtime from the page's origin; the application provides and serves them:

| Path | Content |
| --- | --- |
| `/client-core.wasm` | `kelpclient` built for `wasm32-unknown-unknown` from the same KelpCore revision |
| `/vendor/libapps/wassh/js/` (`process.js`, `syscall_handler.js`, `worker.js` and their imports) and `/vendor/libapps/wasi-js-bindings/` | Chromium libapps WASSH and WASI bindings (BSD), at a pinned libapps revision |
| `/vendor/ssh.wasm` | The OpenSSH WASM client from the Secure Shell release (OpenSSH with OpenSSL, zlib and ldns, each under its own license) |
| `/ssh` | A WebSocket relay to the SSH endpoint |

The browser `Session` opts in to [binary screens](crates/server/docs/binary-screens.md) whenever Welcome lists `binary-screens`, and the WASM core decodes them. Everything between the page and the server must therefore forward binary records unchanged: `kelpserver bridge` does, and so must an application's own bridge (KelpMux's `kelpmux web-bridge` does since it adopted this revision). Deploy the web assets and the bridge from the same build; a bridge that re-serializes every record as JSON breaks the stream once the browser opts in.

In-browser SSH also needs a cross-origin isolated page (`SharedArrayBuffer`). KelpMux's `scripts/fetch-web-ssh.sh` fetches and verifies the vendored SSH assets; their license texts must be served with the application. They are not part of this repository.

## Build and test

```sh
cargo build --workspace --release --locked
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build -p kelpclient --release --locked --target wasm32-unknown-unknown
npm ci --ignore-scripts
npm run check   # tsc --noEmit over web/src
npm test        # node --test web/test/*.test.mjs
```

The Rust toolchain is pinned in [rust-toolchain.toml](rust-toolchain.toml); the minimum supported Rust version is 1.85. The web unit tests need Node 22.18 or later (TypeScript type stripping). Run builds in your environment's approved container workflow. Tests use disposable shells, private sockets and private state; they never connect to a real workspace. Browser tests of the complete web client live in the applications.

```sh
kelpserver start --workspace trial
kelpserver status --workspace trial          # server and stream versions, then the pane list
kelpserver diagnostics --workspace trial     # content-free counters
kelpserver stream-inspect --workspace trial
kelpserver stop --workspace trial
```

A started server has no panes until a client creates them with the stream `spawn` request. Stopping a workspace ends its processes.

## Security and contributions

Use [SECURITY.md](SECURITY.md) for private vulnerability reporting. Follow [CONTRIBUTING.md](CONTRIBUTING.md) and the [repository security policy](docs/repository-security.md). Licensed under [Apache-2.0](LICENSE); see [NOTICE](NOTICE) for attribution.
