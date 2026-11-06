# Two-repository architecture

KelpMux contains the OSS terminal foundation and TUI. KelpHub contains the private web application and authenticated management service. The temporary standalone server and client repositories are consolidated here as internal Cargo crates; there are no operational dependencies on those repositories.

## History and attribution

The original extraction source is the preserved KelpMux development commit `1033e4b`, including the latest local web work rather than the older pre-prod beta. The original checkout and running dev/pre-prod installations are unchanged. The complete original development snapshot is retained privately in KelpHub's `archive/pre-split-development` branch.

The earlier extraction imported path-filtered histories into each component. Those commits preserve original authors, dates and messages and record `Source-Repository` and `Source-Commit`. Filtering changed commit IDs, so the migration operator re-signed those derived commits; those signatures attest to extraction rather than representing the original upstream signatures.

This consolidation imports the complete public server and client main histories using non-squashed subtree merges into `crates/server` and `crates/client`. Their existing commit IDs and signatures remain intact. A subsequent ordinary commit moves the portable protocol package to `crates/protocol` and removes redundant repository policies and lockfiles. A merge commit must retain the subtree ancestry on main; squash-merging this consolidation would discard that ancestry. Subsequent ordinary development can use squash merges.

Unfiltered original development refs and private web files must never be pushed to this public repository. Preserve LICENSE/NOTICE and third-party attribution. Private repository hosting does not revoke an imported file's license.

## Boundaries and compatibility

`crates/server` owns PTYs, terminal emulation, sessions, activity discovery and the generic SSH byte relay. `crates/protocol` contains portable wire types and validation without client-state or WASM dependencies. `crates/client` provides terminal client state and Rust/WASM bindings; the server uses it only in compatibility tests. The root package contains the frozen TUI and legacy tmux adapter.

The TUI still uses the composed-screen protocol and server-side layout/rendering compatibility implementation. Moving that implementation client-side would be a separate architecture change. Package names and executable names remain unchanged. The local terminal protocol remains version 58 and terminal stream remains version 1. Existing XDG paths, workspace identities, SSH authentication, ownership and recovery semantics remain unchanged. No runtime state migration, release or deployment is part of source consolidation.

KelpHub owns authenticated management, uploads, browser code and application hosting. It pins a single immutable KelpMux Git revision for its Rust server dependency and WASM source checkout. Its web host supplies embedded assets to the public generic relay. The relay contains no KelpHub asset bundle or enrollment API. Workspace and file operations require SSH authentication.

## Build and validation

From this workspace, run `cargo build --workspace --release --locked`, `cargo test --workspace --locked`, formatting, Clippy for all workspace targets and dependency audits in the approved container environment. Build browser bindings with `cargo build -p kelpclient --release --locked --target wasm32-unknown-unknown`. CI also covers Linux x86-64/ARM64, Rust 1.85, WASM and the legacy tmux adapter.

Install `kelpserver` beside `kelpmux`, or set `KELPSERVER_BINARY` explicitly. `kelpmux start` and `ssh-entry` launch the standalone server; `attach` connects without owning its PTY lifetime. Explicit `--server-binary` remains supported. `kelpmux server` remains a compatibility dispatch. For a standalone daemon the client authenticates the live peer and checks protocol compatibility instead of trying to execute the server as a TUI.

KelpHub builds the same client crate to WASM, validates its web assets and embeds them in its executable. Web hosting and `web-bridge` use the KelpHub binary. A compatible web-role restart can preserve terminal processes; a terminal-daemon restart ends its shells/jobs. Permanent hosting and beta promotion remain separate tasks.
