<h1 align="center">Kelpmux</h1>

<p align="center">
  <img src="assets/kelpmux-logo.png" alt="Kelpmux logo: terminal panes intertwined with kelp" width="320">
</p>

<p align="center">
  <a href="https://github.com/joanfabregat/kelpmux/actions/workflows/ci.yml"><img src="https://github.com/joanfabregat/kelpmux/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI status"></a>
  <a href="https://github.com/joanfabregat/kelpmux/actions/workflows/release-build.yml"><img src="https://github.com/joanfabregat/kelpmux/actions/workflows/release-build.yml/badge.svg?branch=main" alt="Release build status"></a>
  <a href="docs/releases.md"><img src="https://img.shields.io/badge/version-0.2.0--alpha.1-blue" alt="Version 0.2.0-alpha.1"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/joanfabregat/kelpmux" alt="License: Apache-2.0"></a>
  <a href="#build-and-validate"><img src="https://img.shields.io/badge/Rust-1.85%2B-orange?logo=rust" alt="Rust 1.85 or newer"></a>
  <a href="docs/native.md"><img src="https://img.shields.io/badge/Linux-x86__64%20%7C%20ARM64-blue?logo=linux" alt="Linux x86-64 and ARM64"></a>
</p>

Kelpmux is a persistent terminal workspace for coding agents, inspired by Monterey's kelp forests. The **0.2.0-alpha.1 native preview** owns its PTYs, terminal emulation, grid, reading views, input and attached clients. Shells and agents keep running when a client detaches, while the foreground server remains alive. Apache-2.0 licensed.

## Native alpha

- Balanced `1x1`, `2x1` and `2x2` grids. Growing creates shells; shrinking refuses panes with foreground, background or stopped jobs.
- A centered reading view capped at 120 content columns, plus explicit fullscreen with a visible header. The source tile stays in place and other panes keep their terminal sizes. Returning restores the selected pane to its tile.
- Stable pane IDs and palette colors across focus changes and swaps. Headers distinguish the pane number, title and lifecycle marker; working agents have a braille spinner.
- ANSI application colors, Unicode text, terminal history, alternate screens, mouse reporting and bracketed paste through an Alacritty terminal engine. Application word-editing keys pass through.
- One controlling client, with additional clients as observers. Taking control is explicit; controller dimensions set the workspace size. Smaller observers see a clipped view and larger observers see padding.
- Claude/Codex metadata discovery and optional lifecycle hooks, with a seven-day metadata journal. Session identity stays unknown when the evidence cannot tie it to a pane.

The server runs on **Linux**, with readable `/proc`; CI covers x86-64 and ARM64. Native mode requires no tmux installation. This is an alpha with three grid presets, one workspace per server and no automatic recovery after the server exits. The seven-day journal contains metadata, not recoverable terminal processes or transcripts.

## Start an isolated workspace

Build or unpack the binary, then start the server in a dedicated terminal. All paths below are explicit so the trial uses separate state:

```sh
kelpmux server --socket "$HOME/.local/state/kelpmux-native-alpha/control.sock" \
  --state-dir "$HOME/.local/state/kelpmux-native-alpha" \
  --shell /bin/bash --cwd "$HOME" --size 120x40
```

From another terminal:

```sh
kelpmux attach --socket "$HOME/.local/state/kelpmux-native-alpha/control.sock"
```

The server starts with one shell. `Ctrl-B`, then `g` cycles `1x1 → 2x1 → 2x2 → 1x1`, subject to shrink safety. `Ctrl-B`, then `z` opens reading mode, `f` toggles fullscreen, and `d` detaches. Detaching leaves the shell and server running. Use `attach --takeover` or `Ctrl-B`, then `t` to take control from another client. Header buttons and the footer `?` expose reading controls and shortcut help.

Inspect or control the same server with `kelpmux ctl --socket PATH list`, `layout 2x2`, `focus ID`, `read ID`, `fullscreen ID`, `close`, or `shutdown`. `shutdown` ends the server and its owned terminals; `close` only returns a reading view to the grid. These commands never start a server automatically. See [native commands, shortcuts and limitations](docs/native.md).

## Build and validate

Rust 1.85+ is required. CI pins development Rust 1.99.0, tests the minimum version, audits the locked dependencies, and checks their license expressions. Run dependency-bearing commands in your approved container workflow where required.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo test --locked --test native_server --test native_engine --test native_workspace
cargo build --locked --release
```

Native tests use disposable owned PTYs, shells and private sockets. They exercise reconnect identity, controller/observer behavior, malformed clients, live ANSI output and terminal queries, guarded shrinking, terminal emulation, composition and input routing. They do not launch real agents or attach to an existing user workspace. Linux x86-64 and ARM64 CI also retains the separate [tmux compatibility tests](tests/tmux.rs).

## Legacy tmux compatibility

The older metadata/helper commands remain available for existing tmux 3.8+ workspaces: `daemon`, `scan`, `list`, `tmux-config`, reading helpers and guarded layouts. tmux owns those terminals and their configuration. Review generated configuration before sourcing it; Kelpmux never changes dotfiles or starts discovery automatically. Legacy mode requires a separately installed tmux; native release archives do not bundle it. See the [optional hook examples](examples), [discovery details](docs/discovery.md), and `kelpmux --help` for legacy commands. Keep existing tmux sessions and their state separate from a native trial.

## State and releases

Native state defaults to `$XDG_STATE_HOME/kelpmux/native`, or `~/.local/state/kelpmux/native`, with `control.sock` inside it. State is private and server instances should use separate socket and state paths. The discovery worker writes its metadata registry and journal under `STATE_DIR/native`; no prompts, responses, credentials or transcripts are stored. Optional `native-event` hooks require explicit setup and a discovered agent; the existing hook examples target legacy tmux and need their command replaced for native use. See [native hook setup, journal commands and limitations](docs/native.md) and [architecture](docs/architecture.md).

Release builds validate the code and produce GNU/Linux x86-64 and ARM64 archives, documentation, licenses and SHA-256 checksums. A version string does not publish a release, install a binary or start a service. See [release procedure](docs/releases.md).
