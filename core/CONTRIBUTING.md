# Contributing

Use the pinned Rust toolchain, commit `Cargo.lock` and `package-lock.json`, and keep changes focused. Build and execute code in the container workflow required by your environment. Tests use disposable shells and require no installation or connection to a real workspace.

Run formatting, Clippy for all workspace targets with warnings denied and all workspace tests before pushing. Native PTY, bridge and stream tests require Linux and standard shells. Every fixture owns private sockets and state, cleans up its server, and must be safe alongside another fixture. Agent discovery tests use synthetic `/proc` and metadata files, never real credentials or transcripts.

This repository owns the server contract. Keep `kelpserver` and `kelpclient-protocol` free of client crates and of client presentation or WASM code; tests must use the server's own types or the protocol crate. Protocol changes must update [the stream contract](crates/server/docs/native-client-protocol.md) and say whether they are additive (a new Welcome capability or optional field) or require a version change. A change to `STREAM_PROTOCOL_VERSION`, or to public Rust APIs used by clients, must name the KelpMux and KelpHub changes needed to consume it; clients move to a new revision explicitly.

New discovery support needs fixtures for missing fields, stale PID identity, multiple sessions in one directory, unsupported schemas, ambiguous evidence and session conflicts. Never use terminal output or the latest transcript as an identity guess. Never persist prompts or entire hook payloads. Size and lease changes need real PTY tests proving process identity survives. Input changes need fragmented escape/paste tests. Startup changes need reconnect, duplicate instance, explicit stop and independent workspace coverage.

CI runs formatting, Clippy and all tests on Linux x86-64 and ARM64, full Rust 1.85 compatibility, the `kelpclient` WASM build, the browser layer's locked npm install, audit, type check and unit tests, RustSec advisories and the dependency license policy. Independent fixtures use two test threads. Dependency advisories are fetched and checked on every run.

Dependency additions must be maintained, audited, and compatible with Apache-2.0 distribution. License changes require updating the explicit CI allowlist. Actions stay pinned to reviewed commits. Cache writes belong only to successful trusted events; do not cache secrets, audit results or mutable advisories.

Never start, stop or restart a workspace you did not create for a test. Stopping a workspace ends its shells and jobs.
