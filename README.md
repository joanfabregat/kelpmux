<h1 align="center">Kelpmux</h1>

<p align="center">
  <img src="assets/kelpmux-logo.png" alt="Kelpmux logo: terminal panes intertwined with kelp" width="320">
</p>

<p align="center">
  <a href="https://github.com/joanfabregat/kelpmux/actions/workflows/ci.yml"><img src="https://github.com/joanfabregat/kelpmux/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI status"></a>
  <a href="https://github.com/joanfabregat/kelpmux/actions/workflows/release-build.yml"><img src="https://github.com/joanfabregat/kelpmux/actions/workflows/release-build.yml/badge.svg?branch=main" alt="Release build status"></a>
  <a href="https://github.com/joanfabregat/kelpmux/blob/main/Cargo.toml"><img src="https://img.shields.io/badge/version-0.1.0--beta.1-blue" alt="Version 0.1.0-beta.1"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/joanfabregat/kelpmux" alt="License: Apache-2.0"></a>
  <a href="#build-and-validate"><img src="https://img.shields.io/badge/Rust-1.85%2B-orange?logo=rust" alt="Rust 1.85 or newer"></a>
  <a href="#first-beta-scope"><img src="https://img.shields.io/badge/tmux-3.3%2B-green" alt="tmux 3.3 or newer"></a>
  <a href="docs/releases.md"><img src="https://img.shields.io/badge/Linux-x86__64%20%7C%20ARM64-blue?logo=linux" alt="Linux x86-64 and ARM64"></a>
</p>

An agent-aware Rust controller for tmux, inspired by Monterey's kelp forests. Apache-2.0 licensed.

Kelpmux keeps tmux in charge of terminals and processes. It adds a comfortable reading view, agent status headers, guarded grid changes, and a seven-day session journal. Agents can start normally: process discovery is the default, and hooks are optional.

## First beta scope

- Centered reading popup capped at 120 content columns, with fullscreen and restore controls. Smaller terminals use their available width. Other panes keep running and modern tmux redraws the visible background.
- Switch directly to panes 1–9 from the grid or reading popup. Keyboard controls and clickable shortcut help are provided.
- Colored headers distinguish working, input needed, turn finished, idle, failed, and unknown states.
- Grids: `1x1`, `2x1`, `2x2`, `3x2`, `2x3`, `3x3` (columns × rows). Growing creates shells. Shrinking requires enough unused shells and refuses busy panes, including background and stopped jobs.
- Journal agent type, session ID when identifiable, name, directory, pane/window IDs, process identity, and lifecycle changes. Resume sessions using the agent's own TUI.

**Linux only for this beta**, with tmux 3.3 or newer and readable `/proc` for your processes. The terminal client can be on any device. CI exercises tmux on Linux x86-64 and ARM64. Scratch terminals, nested popups, dragging popup borders, macOS process discovery, and automatic session recovery are deferred. Popup width is configurable when opening it; fullscreen/restore resizes the view on demand.

**Discovery limitation:** Claude's current live records expose PID-bound session IDs, names, and status. Codex can be identified from an explicit resume ID, a primary process `CODEX_THREAD_ID`, or an open rollout file. Some Codex shared-daemon TUIs expose none of these; their session ID and status remain unknown. Kelpmux does not guess from directory or the newest transcript. See [discovery and optional hooks](docs/discovery.md) before depending on the journal for every Codex session.

## Build and validate

Rust 1.85 or newer is required; the repository pins the development/CI toolchain. There are three direct dependencies: `serde`, `serde_json`, and `libc`. The lockfile is committed.

```sh
cargo build --locked --release
cargo test --locked
cargo test --locked --test tmux -- --ignored --test-threads=1
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

Real tmux tests use private sockets, empty configuration, synthetic shells/jobs, and disposable state. They never connect to your tmux server or launch real agents. Use your approved container workflow where required. See [contributing](CONTRIBUTING.md).

## Explicit setup

Nothing installs, starts, or sources configuration automatically. After reviewing the binary and deciding to try it, put `kelpmux` on your PATH. Run discovery in a dedicated pane or your chosen process supervisor:

```sh
kelpmux daemon
```

The daemon runs in the foreground and polls every two seconds. `Ctrl+C` stops it; your tmux panes and agents remain. `kelpmux scan` performs one discovery pass. `kelpmux list` shows the latest registry as JSON; it does not scan by itself.

Generate the optional tmux fragment, review it, then merge the bindings you want into your existing configuration:

```sh
kelpmux tmux-config > kelpmux.tmux.conf
```

The fragment uses the absolute binary/state paths, sets pane numbering to 1, replaces the listed bindings, and adds top pane headers with a solid status-colored background and a distinct number badge. Reading mode retains the original grid number. Pane styles are reset to preserve application foreground and background colors. A single `?` opens shortcut help; workspace actions use keyboard shortcuts. It does not enable mouse mode or start the daemon. For touch controls, enable tmux mouse handling in your own configuration. Existing complex configurations should merge the fragment instead of sourcing it wholesale. Re-generate it after moving the binary.

| Control | Action |
| --- | --- |
| Option/Alt+Tab or Option/Alt+0 | Open reading view; return to grid from the popup |
| Option/Alt+1…9 | Open that numbered pane; switch agents inside the popup |
| Option/Alt+F | Fullscreen |
| Option/Alt+R inside popup | Restore capped reading width |
| Prefix then `z` | Toggle reading view / return to grid |
| Prefix then `p`, number, Enter | Read a numbered pane |
| Prefix then `f` / uppercase `R` | Fullscreen / restore reading width |
| Prefix then `g` | Grid size menu (from grid) |
| Prefix then `?`, or click/tap `?` | Shortcut help |

Option must send Meta/Alt sequences in your terminal. Popup bindings use a dedicated tmux key table. Applications otherwise keep their normal input. Escape can close tmux's popup and restores the grid; an agent TUI may also use Escape, so prefer Prefix then `z`.

CLI examples with stable tmux IDs:

```sh
kelpmux zoom --pane %4 --client /dev/pts/1 --width 120
kelpmux fullscreen --pane %4 --client /dev/pts/1
kelpmux layout 3x2 --window @0 --dry-run
kelpmux layout 3x2 --window @0
kelpmux name --pane %4 'Auth migration'
kelpmux journal
```

Use `tmux list-panes -F '#{pane_id} #{window_id}'` and `tmux list-clients -F '#{client_name}'` to inspect IDs. IDs above are examples. You can pass `--tmux-socket PATH` and `--state-dir PATH` for a separate server. Use a separate state directory for each tmux server; the first beta controls one server per daemon/state directory.

## Safety, persistence, and recovery

An unused pane means a recognized interactive shell with its own foreground process group and no descendants. `ping`, `htop`, an agent, background commands, and stopped jobs all make it occupied. Kelpmux validates the complete removal set before removing any pane, rechecks each victim, and asks tmux to check shell identity immediately before removal. Highest numbered idle shells are removed first.

tmux has no atomic “kill only if no jobs exist” operation. Do not type into removal candidates or mutate the window with another controller during a shrink. A concurrent command can race the final check; an operation interrupted after removals begin can leave a partial grid. Kelpmux operations are serialized, and busy panes at validation time cause a whole-operation refusal. Layout changes require returning to the grid first.

Reading mode moves the same pane into a temporary tmux session and leaves a placeholder in its grid slot. It never starts another agent or mirrors input. Returning swaps the pane back. Closing Kelpmux unexpectedly can leave the agent in its temporary session; the process survives. With no reading controller still active, run:

```sh
kelpmux recover-modal --origin @0
```

Recovery preserves unexpected extra panes rather than deleting them. See [architecture](docs/architecture.md).

State defaults to `$XDG_STATE_HOME/kelpmux` or `~/.local/state/kelpmux`, with a private directory and private files. Journal entries older than seven days are pruned during scanning or journal inspection. Current live-agent metadata remains available even for agents running longer than seven days. Retention requires Kelpmux to run; there is no separately installed cleanup task. After a crash, inspect `kelpmux journal` and resume the recorded IDs through Codex or Claude Code. Last-seen status is evidence of the last scan, not proof that a process is still alive.

Kelpmux stores no prompts, responses, tool arguments, credentials, or transcripts. Discovery may read process arguments, one specific environment field, and bounded agent metadata/rollout events in memory. Hook payloads are reduced to identity and lifecycle status before persistence.

## CI and release builds

CI checks formatting, Clippy, unit tests, private-server tmux integration on x86-64/ARM64, the minimum Rust version, release compilation, RustSec advisories, and dependency licenses. Actions are pinned to commit SHAs.

Tags matching `v*`, published releases, and manual dispatch run the same validation and produce native Linux x86-64/ARM64 archives with the binary, license, notice, README, and SHA-256 checksums. Tag/release names must match the package version, currently `v0.1.0-beta.1`. Workflow artifacts remain available for 14 days. Published-release builds also attach archives/checksums to that release. The workflow never creates a tag or publishes a release itself.

The first beta is available as [v0.1.0-beta.1](https://github.com/joanfabregat/kelpmux/releases/tag/v0.1.0-beta.1). Installation and startup remain explicit. See the [release procedure](docs/releases.md).
