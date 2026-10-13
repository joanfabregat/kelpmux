# Contributing

Use the pinned Rust toolchain, commit `Cargo.lock`, and keep changes focused. Build and execute code in the container workflow required by your environment. Tests use disposable shells and require no installation or connection to a real workspace.

Run formatting, Clippy with warnings denied, ordinary tests, and the explicitly enabled legacy tmux test suite. Native PTY and attach tests require Linux and standard shells; legacy integration also requires tmux 3.8+. Every fixture owns private sockets and removes its own server. Agent discovery tests use synthetic `/proc` and metadata files, never real credentials or transcripts.

New discovery support should include fixtures for missing fields, stale PID identity, multiple sessions in one directory, and unsupported schemas. Never use terminal output or the latest transcript as an identity guess. Never persist prompts or entire hook payloads. Workspace changes should prove process survival and refusal for busy panes with real PTY tests, plus tmux tests when changing the legacy adapter.

CI builds on Linux x86-64 and ARM64 and checks Rust 1.85 compatibility. Dependency additions must be maintained, audited, and compatible with Apache-2.0 distribution. License changes require updating the explicit CI allowlist. Actions must stay pinned to reviewed commits. Release builds are described in [docs/releases.md](docs/releases.md).
