# KelpClient

The portable terminal client crate in the [KelpMux workspace](../../README.md), shared by the TUI and KelpHub. This is a Rust library and WASM module, not a running service or standalone repository.

It handles client screen snapshots, incremental updates, revision tracking, framing, selection and Unicode-aware history layout. Portable wire types and validation live in the sibling [protocol crate](../protocol); PTYs and authoritative emulation live in the [server crate](../server). Browser rendering, SSH authentication, fonts, clipboard integration and application presentation stay outside this core.

Build from the workspace root with `cargo build -p kelpclient --release --locked --target wasm32-unknown-unknown`. Install that Rust target first. KelpHub pins the workspace Git revision for both server dependencies and WASM builds. Preserve protocol compatibility when changing either side.

See [migration provenance](../../MIGRATION.md), [contribution policy](../../CONTRIBUTING.md) and [security policy](../../SECURITY.md). Licensed under [Apache-2.0](LICENSE).
