<h1 align="center">Kelpmux</h1>

<p align="center">
  <img src="assets/kelpmux-logo.png" alt="Kelpmux logo: terminal panes intertwined with kelp" width="320">
</p>

<p align="center">
  <a href="https://github.com/joanfabregat/kelpmux/actions/workflows/ci.yml"><img src="https://github.com/joanfabregat/kelpmux/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI status"></a>
  <a href="https://github.com/joanfabregat/kelpmux/actions/workflows/release-build.yml"><img src="https://github.com/joanfabregat/kelpmux/actions/workflows/release-build.yml/badge.svg?branch=main" alt="Release build status"></a>
  <a href="docs/releases.md"><img src="https://img.shields.io/badge/version-0.2.0--beta.3-blue" alt="Version 0.2.0-beta.3"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/joanfabregat/kelpmux" alt="License: Apache-2.0"></a>
  <a href="#build-and-validate"><img src="https://img.shields.io/badge/Rust-1.85%2B-orange?logo=rust" alt="Rust 1.85 or newer"></a>
  <a href="docs/native.md"><img src="https://img.shields.io/badge/Linux-x86__64%20%7C%20ARM64-blue?logo=linux" alt="Linux x86-64 and ARM64"></a>
</p>

Kelpmux is a persistent terminal workspace for coding agents, inspired by Monterey's kelp forests. The native Rust server owns its PTYs, Alacritty terminal emulation, grids, reading views and agent metadata. Detaching a client leaves shells and agents running. Apache-2.0 licensed; native mode needs no tmux.

## Native beta

- Automatic balanced grids computed from unparked tabs and terminal dimensions. Resizing the view preserves processes.
- Park tabs behind a restore list while their terminals, output and agent status continue running.
- Grid overview and responsive tabs with top, left or compact bottom navigation; capped or full terminal width while preserving every process.
- Popup configuration and close-tab confirmations, plus a Vitals-derived CPU, memory, swap, disk and clock footer.
- Stable pane IDs and colors, distinct pane numbers, lifecycle markers and a braille working spinner. Agent selection filters questions and finished sessions; missing session evidence stays unknown.
- Configurable shortcuts and graphical pane movement, preserving application word-editing keys outside explicit bindings.
- Named persistent workspaces with explicit start, attach, status and stop commands. One controller, additional observers and explicit takeover.
- Alacritty application colors, Unicode text, history, alternate screens, terminal query replies, application mouse reporting, local selection and bracketed paste.
- Claude/Codex discovery without wrappers, optional lifecycle hooks and a seven-day metadata journal. No prompts, responses, credentials or transcripts are stored in the journal.
- Overview, persistent unread notifications with optional sound, and a recovery dashboard grouped by workspace and project with copyable resume commands.
- A persistent scratch terminal popup, history search across one or all terminals, and keyboard or mouse selection with OSC 52 copy.
- Startup tab counts, named terminal templates and directories; remembered view widths and automatic equalization.
- Revision-checked incremental frames with bounded slow-client resynchronization, a reproducible payload benchmark and content-free diagnostics.

The server runs on **Linux** with readable `/proc`; CI tests x86-64 and ARM64. The server must stay alive to preserve terminals. A crash or reboot does not recreate terminal processes; the journal identifies sessions for manual resumption in their agent TUIs. Physical RootShell/device validation is separate from automated PTY profiles.

## Start an isolated workspace

Build or unpack the binary, then use a new workspace name. Existing alpha and tmux workspaces can continue independently:

```sh
kelpmux start --workspace beta-trial
kelpmux attach --workspace beta-trial
```

Inspect it with `kelpmux status --workspace beta-trial`. `Ctrl-B`, then `d` detaches without ending processes. `kelpmux stop --workspace beta-trial` ends that workspace and its owned shells; stop only a disposable trial when finished testing.

`Ctrl-B`, then `g` or `e` opens the automatic grid; `z` switches grid/tabs; `f` toggles full terminal width; `?` opens help. Prefix then Enter creates a tab, `p` requests parking the active tab, and `u` opens the parked list. The ⊟ tab control requests parking; the Parked control in the tab bar opens the colored restore table. Parking and restoring each require confirmation: Enter confirms, Esc cancels. `Ctrl-B`, then `a` opens the agent switcher, `!` filters attention and `n` filters finished sessions; `s` opens pane movement. Use `attach --takeover` or `Ctrl-B`, then `t` to take control from another client. Controller dimensions determine geometry; observers see a clipped or padded frame.

`kelpmux ssh-entry --workspace beta-trial` attaches or starts the named workspace from an SSH shell. Automatic SSH routing requires explicit login-hook configuration; a launch command injected into an already running tmux agent is not a shell command. Keep existing SSH authentication unchanged and choose Kelpmux before automatic tmux attachment. See [native commands, configuration and SSH setup](docs/native.md).

The foreground `server --socket PATH --state-dir PATH` and explicit `ctl --socket PATH` interface remain available for controlled development and independent endpoints. Commands never migrate a tmux workspace or install agent hooks automatically.

## Build and validate

Rust 1.85+ is required. CI pins development Rust 1.99.0, tests minimum Rust, audits locked dependencies against fresh RustSec advisories, and checks their license expressions. Run dependency-bearing commands in your approved container workflow where required.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release
```

Native fixtures use disposable PTYs, shells and private sockets/state. They test reconnect identity, controller/observer behavior, malformed/slow clients, ANSI/query handling, fragmented input, paste, automatic grids, parking, exit cleanup, responsive tabs/dialogs, agent filters, configuration and workspace lifecycle. They do not launch real agents or modify an existing workspace. Small and large terminal profiles check rendering and resize behavior; the [release checklist](docs/releases.md) separately covers hands-on RootShell and display checks.

Quality, native architecture, minimum-Rust, dependency policy and legacy integration jobs run in parallel. Reviewed cache actions restore locked dependency sources and job/toolchain-specific build outputs; only successful trusted events save caches. Current advisories and audit results are never cached. Release jobs produce versioned archives, license texts and verified SHA-256 checksums.

## Legacy tmux compatibility

Older metadata/helper commands remain available for existing tmux 3.8+ workspaces: `daemon`, `scan`, `list`, `tmux-config`, reading helpers and guarded layouts. tmux owns those terminals and their configuration. Review generated configuration before sourcing it; Kelpmux never changes dotfiles or starts discovery automatically. Legacy mode requires a separately installed tmux; native archives do not bundle it. See [hook examples](examples), [discovery details](docs/discovery.md), and `kelpmux --help`. Keep legacy state separate from a native trial.

## State and releases

Native state uses the XDG state directory or `~/.local/state/kelpmux`; workspace names select independent private endpoints. The discovery worker writes its private registry and seven-day journal under each server's state root. Explicit foreground servers can choose a dedicated socket and state directory. See [architecture](docs/architecture.md) for protocol, geometry, discovery and process-safety boundaries.

Release builds validate the repository and package GNU/Linux x86-64 and ARM64 binaries, documentation, locked dependency licenses and checksums. A version string alone does not publish a release, install a binary or start a service. See the [release procedure](docs/releases.md).

## Unreleased native-client foundation

The planned graphical clients are macOS, iPadOS, iOS and web. The [four-client architecture proposal](docs/adr/0002-four-clients-and-web-ssh.md) complements the native protocol ADR with a shared client core, browser SSH authentication and terminal text selection requirements.

Development beta.3 adds experimental independent pane streams, per-pane control/size leases, an SSH stdio bridge, read-only reference inspection and matching-client attachment without restarting workspaces. It also fixes a fragmented-Unicode rendering bug and adds conformance and nine-pane stress coverage. Existing beta.2/alpha sessions keep their current behavior until explicitly replaced. See the [implemented stream contract](docs/native-client-protocol.md) and [proposed native/MCP/device ADR](docs/adr/0001-native-client-protocol-and-device-capabilities.md). Native applications and MCP/device access remain future work.
