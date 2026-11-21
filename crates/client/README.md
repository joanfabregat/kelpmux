# KelpClient

The portable terminal client crate in the [KelpMux workspace](../../README.md), shared by the TUI and KelpHub. This is a Rust library and WASM module, not a running service or standalone repository.

It handles client screen snapshots, incremental updates, revision tracking, framing, selection and Unicode-aware history layout. Portable wire types and validation come from the `kelpclient-protocol` crate, and PTYs and authoritative emulation from `kelpserver`; both live in the [KelpServer repository](https://github.com/joanfabregat/kelpserver) and are pinned to one revision in the workspace `Cargo.toml`. Browser rendering, SSH authentication, fonts, clipboard integration and application presentation stay outside this core.

Build from the workspace root with `cargo build -p kelpclient --release --locked --target wasm32-unknown-unknown`. Install that Rust target first. The resulting `client-core.wasm` must be built with `kelpclient-protocol` from the same KelpServer revision as the server it talks to; consumers such as KelpHub pin one KelpMux revision for the WASM build and the matching KelpServer revision for the server. Preserve protocol compatibility when changing either side.

See [migration provenance](../../MIGRATION.md), [contribution policy](../../CONTRIBUTING.md) and [security policy](../../SECURITY.md). Licensed under [Apache-2.0](LICENSE).
