# Component extraction

The extraction source is the preserved KelpMux development commit `1033e4b` (including the latest local web work), not the older pre-prod beta. The original checkout and running pre-prod installation are unchanged.

## History and attribution

`history/kelpmux-source` is a path-filtered export of relevant upstream history, merged into this repository's extraction branch. Original authors, author dates and commit messages are retained. Every exported commit records its original `Source-Repository` and `Source-Commit`. Filtering changes commit IDs, so the migration operator re-signed the derived commits; those signatures attest to the extraction and are not the original upstream signatures. The source checkout remains available for comparison.

Only the selected paths and their reachable history are imported. Unfiltered source refs and private web files must never be pushed to an OSS component repository. Preserve LICENSE/NOTICE and third-party attribution when moving files. Private repository hosting does not revoke an imported file's license.

A merge commit is required to retain this history in the default branch. Squash-merging the extraction would discard its ancestry. Subsequent ordinary feature changes can use squash merges.

## Compatibility

The local terminal protocol remains version 58 and the independent terminal stream remains version 1. Existing XDG state paths, workspace identities, SSH authentication, terminal ownership and recovery semantics are preserved. This extraction introduces no state migration, production upgrade or new management feature.

The frozen TUI still uses the composed-screen protocol and its server-side layout/rendering compatibility implementation. That implementation intentionally remains in KelpServer for this extraction; moving it client-side would be a separate TUI architecture change. KelpMux's frontend, input decoder and hover handling are separate from the standalone server executable.

KelpHub's authenticated management endpoint and managed uploads are private application code. Its web host uses the public generic relay with application-supplied embedded assets. The relay contains no KelpHub asset bundle or enrollment API. No workspace or file operation is exposed through unauthenticated HTTP.

## Dependencies and validation

Cross-repository dependencies use immutable Git revisions. KelpClient's protocol package contains wire types and validation without client state or WASM dependencies; the server uses that package. KelpMux uses the Rust client package and KelpHub builds the same core to WASM. KelpServer references client-state utilities only in tests for server/client history compatibility.

Run formatting, Clippy, applicable Rust tests and dependency audits in the approved container environment. KelpHub additionally runs TypeScript checks, JavaScript/WASM tests and browser/SSH integration tests. Build the web bundle before compiling the KelpHub executable to embed it. Physical browser/security-key acceptance remains separate from emulated authenticator tests.

Do not replace a running pre-prod daemon as part of source extraction. A compatible web-role restart can leave terminal processes running; a terminal-daemon restart ends its shells/jobs. Permanent hosting and beta promotion are separate deployment tasks.

## KelpMux build and installation

From this repository: `cargo build --locked -p kelpserver --bin kelpserver`, then `cargo test --locked` and `cargo build --release --locked`. Build/install `kelpserver` beside `kelpmux`, or set `KELPSERVER_BINARY` to an explicit server path. `kelpmux start` and `ssh-entry` launch that standalone server; `attach` connects to it without owning its PTY lifetime. Explicit `--server-binary` remains supported. The `kelpmux server` entry is retained as a compatibility dispatch for existing tests/commands, not the recommended daemon installation.

Legacy combined-binary installations keep their existing retained-client selection. For a split `kelpserver` daemon, the client authenticates the live peer and requires a compatible protocol instead of trying to execute the server as a TUI. Private web roles moved to `kelphub`; use its binary for web hosting and web-bridge commands.
