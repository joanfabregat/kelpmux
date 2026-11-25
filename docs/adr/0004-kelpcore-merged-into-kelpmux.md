# ADR 0004: KelpCore merged into KelpMux

- Status: Accepted.
- Date: 2026-11-24 (America/Los_Angeles).
- Supersedes: [KelpCore ADR 0003](core/0003-kelpcore-shared-repository.md) (KelpCore holds the server and every client-shared component).
- Related: [ADR 0003](0003-web-client-in-kelpmux.md), [KelpCore ADR 0002](core/0002-generic-session-server.md), KelpHub ADR 0007 (multi-tenant, Kubernetes-only; private repository).

## Context

KelpCore ADR 0003 made KelpCore the home of the session server (KelpServer), the protocol crate, the Rust/WASM client core and the browser terminal layer, with KelpMux and KelpHub each pinning one KelpCore revision for all of them. KelpHub is paused (KelpHub ADR 0007), so KelpMux was KelpCore's only active consumer. Nearly every feature touched both repositories: a KelpCore pull request, then a KelpMux pull request moving the Cargo `rev` of three crates and the `@kelpcore/web` npm Git pin to the same commit, refreshing both lockfiles, and keeping two CI configurations, two security policies and two sets of contribution rules in step. The pin bump carried no review value of its own, and the CI check that the four pins agreed existed only because they could disagree.

## Decision

1. KelpCore is merged into KelpMux with its complete history: a signed, non-squashed subtree merge of KelpCore `main` (`5ca7b6d`) under `core/`, followed by one ordinary commit that moves everything into place with `git mv`, so `git log --follow` and blame keep working.
2. The crates become members of KelpMux's Cargo workspace at `crates/server` (`kelpserver`), `crates/protocol` (`kelpclient-protocol`) and `crates/client` (`kelpclient`). Crate names and versions are unchanged, and `[workspace.dependencies]` refers to them by path. One `Cargo.lock` covers the workspace; the merge changed no third-party version.
3. The browser terminal layer moves to `web-core/` and keeps its package name, `@kelpcore/web`, to avoid churn in imports; it stays private and unpublished. The root `package.json` makes `web` and `web-core` one npm workspace with a single root `package-lock.json`, and `web/package.json` depends on `"@kelpcore/web": "file:../web-core"`. A workspace (rather than a plain `file:` link with its own install) hoists React and CodeMirror once, so the bundle cannot hold two copies of either.
4. KelpCore's ADRs keep their numbers under `docs/adr/core/`; its server documentation moves to `docs/server/`, which release archives ship with the rest of `docs/`. Security policy, contribution rules, README, NOTICE and CI are merged into KelpMux's; the identical LICENSE, toolchain file, Vitals license and hook examples are kept once.
5. The Rust toolchain stays 1.99.0 (both repositories already used it). The workspace MSRV checked in CI is KelpMux's 1.89, the higher of the two; the moved crates still declare `rust-version = "1.85"`, because raising it enables Clippy lints (`collapsible_if` with let chains) that would rewrite moved code in the same commit.
6. KelpCore ADR 0003's boundary rule still applies inside this repository: `kelpserver` and `kelpclient-protocol` depend on no client crate and hold no presentation or WASM code; `kelpclient` and `@kelpcore/web` hold no application presentation (TUI, web layout, relay, navigation), which stays in `src/` and `web/`.

## Consequences

- A change to the server, the protocol, the client core or the browser layer lands with its KelpMux consumer in one pull request and one CI run. There are no pins to move, and CI now checks that `Cargo.lock` and `package-lock.json` resolve the four packages to this workspace.
- KelpMux's CI runs the server, protocol and client core tests on x86-64 and ARM64, the browser layer's type check and unit tests, and the dependency license policy over the whole graph; KelpCore's separate Rust 1.85 job is dropped (see decision 5).
- The `joanfabregat/kelpcore` repository is to be archived (read-only); its history continues here. KelpHub still builds against its old KelpServer pins; when it resumes it takes the server, the client core and the browser layer from one KelpMux commit.
- The public repository now carries the server's code and documentation again, as before the KelpServer split; nothing private moved, since KelpCore was public too.
