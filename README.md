# KelpClient

KelpClient is the open-source collection of portable terminal client libraries shared by KelpMux and KelpHub. It is a library project, not another running service.

## Status

This repository is a scaffold for extracting the existing `kelpmux-client-core` and shared protocol definitions from [KelpMux](https://github.com/joanfabregat/kelpmux). No implementation or published packages have been migrated yet.

## Scope

- Shared terminal protocol types, framing and validation.
- Client-side screen snapshots, incremental updates and revision tracking.
- Terminal selection and Unicode-aware history layout utilities.
- Rust libraries and WASM bindings for browser clients.

Separate packages will keep protocol/types usable by KelpServer without depending on client state or WASM. KelpMux will use Rust libraries; KelpHub will use the same terminal core through WASM and a browser adapter. Platform rendering, SSH authentication, fonts, clipboard integration and application presentation stay outside the portable core.

The authoritative emulator and PTYs belong to [KelpServer](https://github.com/joanfabregat/kelpserver). Project, repository and worktree management models belong to KelpHub, not this terminal SDK.

## Compatibility

Library releases must identify compatible protocol versions and capabilities. Cross-repository dependencies must use published versions or immutable revisions, rather than a moving default branch. Existing TUI behavior must remain covered while web features evolve.

## Development

The local checkout is `~/repos/kelp/client`. Build and validation instructions will be added with the implementation. See [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md). Licensed under [Apache-2.0](LICENSE).
