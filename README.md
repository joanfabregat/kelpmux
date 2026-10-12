<h1 align="center">Kelpmux</h1>

<p align="center">
  <img src="assets/kelpmux-logo.png" alt="Kelpmux logo: terminal panes intertwined with kelp" width="320">
</p>

<p align="center">
  <a href="https://github.com/joanfabregat/kelpmux/actions/workflows/ci.yml"><img src="https://github.com/joanfabregat/kelpmux/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI status"></a>
  <a href="https://github.com/joanfabregat/kelpmux/actions/workflows/release-build.yml"><img src="https://github.com/joanfabregat/kelpmux/actions/workflows/release-build.yml/badge.svg?branch=main" alt="Release build status"></a>
  <a href="https://github.com/joanfabregat/kelpmux/blob/main/Cargo.toml"><img src="https://img.shields.io/badge/version-0.1.0--beta.2-blue" alt="Version 0.1.0-beta.2"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/joanfabregat/kelpmux" alt="License: Apache-2.0"></a>
  <a href="#build-and-validate"><img src="https://img.shields.io/badge/Rust-1.85%2B-orange?logo=rust" alt="Rust 1.85 or newer"></a>
  <a href="#native-tmux-scope"><img src="https://img.shields.io/badge/tmux-3.8%2B-green" alt="tmux 3.8 or newer"></a>
  <a href="docs/releases.md"><img src="https://img.shields.io/badge/Linux-x86__64%20%7C%20ARM64-blue?logo=linux" alt="Linux x86-64 and ARM64"></a>
</p>

Agent metadata and safe workspace helpers for tmux, inspired by Monterey's kelp forests. Apache-2.0 licensed.

**tmux owns the terminals and the interface.** Kelpmux supplies agent metadata, guarded grid changes, a seven-day session journal, and small commands for native floating-pane geometry. Your tmux configuration owns appearance, focus, mouse handling, shortcuts, and help. Agents launch normally; discovery is the default and lifecycle hooks are optional.

## Native tmux scope

- Native floating reading view capped at 120 content columns. Other panes remain visible and keep running. Small windows use native fullscreen zoom automatically; larger windows restore the width cap. Explicit fullscreen stays fullscreen until restored.
- The same pane, process, index, and scrollback remain in the same window. Returning restores its grid position. No nested client, temporary session, sleeping placeholder, separate key table, or reading controller stays running.
- Agent status, session name and identifiable session ID are written to native pane options. The optional appearance example uses a status-colored title background, a distinct number badge, a focus arrow, and white/gray default text while preserving application colors.
- Guarded grids: `1x1`, `2x1`, `2x2`, `3x2`, `2x3`, `3x3` (columns × rows). Growing adds shells; shrinking removes only enough unused shells. Foreground, background and stopped jobs all make a pane occupied.
- Seven-day metadata journal: agent type, session ID when identifiable, name, directory, pane/window IDs, process identity, and lifecycle changes. Resume using the agent's own TUI.

**Linux and tmux 3.8+**, with readable `/proc`. Terminal clients may run on any device. Reading mode changes the native window, so clients viewing that same window share its layout and reading pane; tmux's `window-size` setting decides its dimensions. Scratch-terminal bindings and automatic agent recovery remain deferred.

**Discovery limitation:** some Codex shared-daemon TUIs expose no pane-bound session identity. Their ID and status remain unknown rather than being guessed from a directory or the newest transcript. See [discovery and optional hooks](docs/discovery.md).

## Build and validate

Rust 1.85+ is required; CI pins the development toolchain and tests the minimum version. The three direct Rust dependencies are `serde`, `serde_json`, and `libc`; the lockfile is committed. Run builds and dependency-bearing commands in your approved container workflow where required.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release
cargo test --locked --test tmux -- --ignored --test-threads=1
```

Real terminal tests require tmux 3.8+ on PATH, `/bin/bash`, and disposable private sockets. They never attach to your workspace or launch real agents. They cover grid safety, native floating/zoom/restore, pane identity and original layout, keyboard commands, small/large terminal resizing, metadata discovery, and user configuration preservation.

### Build tmux without replacing the system package

`scripts/build-tmux.sh` builds unmodified upstream tmux 3.8 into an explicit user prefix. It verifies the archive against the SHA-256 digest published for the GitHub release before extracting or building. Build prerequisites are a C compiler, make, pkg-config, bison, libevent development files and ncurses development files. No Rust code is linked into tmux and no patch is applied.

Fetch the archive separately, then build offline in your approved container/build environment:

```sh
curl -fsSL https://github.com/tmux/tmux/releases/download/3.8/tmux-3.8.tar.gz -o tmux-3.8.tar.gz
scripts/build-tmux.sh tmux-3.8.tar.gz /absolute/path/to/private-prefix
```

Use a disk-backed `TMPDIR` for the build where `/tmp` is size-limited. The output includes `bin/tmux`, upstream copyright/license, source digest, and manual. It does not modify apt, global PATH, or any system tmux server. The binary requires compatible glibc, libevent and libtinfo runtime libraries.

## Explicit setup

Setup never edits dotfiles, sources configuration, starts a server, or starts discovery automatically. For a private test, use an explicit tmux binary, separate socket, and separate Kelpmux state directory. Clear inherited automation color-disabling variables before creating shells:

```sh
env -u NO_COLOR /absolute/path/to/tmux/bin/tmux -L kelpmux-preview -f /absolute/path/to/test.tmux.conf new-session -d -s beta
kelpmux --tmux-binary /absolute/path/to/tmux/bin/tmux --tmux-socket /absolute/path/to/socket --state-dir /absolute/path/to/preview-state daemon
```

The daemon runs in the foreground and polls every two seconds. Run it in a dedicated tmux window or your chosen supervisor. `Ctrl+C` stops metadata discovery; tmux panes and agents remain. `scan` runs one pass; `list` reads the latest metadata without scanning.

Review the generated example before sourcing it. It uses absolute helper/state paths and preserves an explicitly selected tmux binary/socket:

```sh
kelpmux tmux-config > kelpmux-example.tmux.conf
kelpmux tmux-config --bindings-only > kelpmux-bindings.tmux.conf
```

The full example includes optional header appearance and a single clickable `?` for shortcut help. `--bindings-only` leaves appearance, pane numbering, mouse settings, status line and text styles under your existing configuration. Both variants explicitly replace the documented bindings and install a `window-resized[80]` hook to refit native reading panes. Regenerate after moving the helper or changing the binary/socket/state paths. Nothing enables mouse handling or starts discovery; retain `set -g mouse on` in your own configuration if desired.

The example assumes the default `Ctrl+B` prefix:

| Shortcut | Action |
| --- | --- |
| Prefix then `z`, or Option/Alt+Tab / Option/Alt+0 | Toggle reading view / return to grid |
| Prefix then `p`, number, Enter; or Option/Alt+1…9 | Read a numbered pane |
| Prefix then `f`, or Option/Alt+F | Native fullscreen |
| Prefix then uppercase `R`, or Option/Alt+R in reading view | Restore capped reading width |
| Prefix then `g` | Guarded grid-size menu; return to grid first |
| Prefix then `?`, or click/tap `?` | Shortcut help |

Normal tmux keyboard and mouse behavior remain in charge. Escape goes to the application; it does not close a separate popup. Alt+R passes through outside reading mode. Existing `h`/`v`, pane swap, balance, copy mode, and other bindings can stay in your own configuration. Option must send Meta/Alt sequences for those shortcuts to work.

CLI commands use stable native IDs:

```sh
kelpmux read --pane %4 --width 120
kelpmux fullscreen --pane %4
kelpmux restore --pane %4
kelpmux grid --window @0
kelpmux select 2 --window @0
kelpmux layout 3x2 --window @0 --dry-run
kelpmux layout 3x2 --window @0
kelpmux name --pane %4 'Auth migration'
kelpmux journal
```

`zoom` remains an alias for `read`. Inspect IDs with `tmux list-panes -F '#{pane_id} #{window_id} #{pane_index}'`. Pass the same `--tmux-binary`, `--tmux-socket` and `--state-dir` to every command controlling a private server.

## Safety and recovery

An unused pane is a recognized interactive shell in its own foreground process group with no descendants. Growing creates shells; shrinking validates the complete removal set, rechecks process identity and occupancy, and performs a final tmux-side shell check. Grid changes refuse floating panes, reading mode and native zoom. Unrelated floating panes are not taken over or removed by reading commands.

**tmux has no atomic “remove only if no jobs exist” operation.** Do not start commands in removal candidates or mutate the same layout concurrently during a shrink. A command can race the final check, and interrupted removals can leave a partially reduced grid. Runnable shells are treated as occupied; blocked shell built-ins may still resemble an idle prompt without shell cooperation.

Reading helpers exit after each action. If interrupted, the pane remains a native tmux pane with its process intact. `kelpmux grid --window @0` returns managed reading panes; native `join-pane -s %ID -t %ID` can also return a floating pane after unzooming. No separate modal-state file or recovery controller exists. Original geometry is reapplied only if the pane/index inventory still matches and no unrelated floating panes exist; otherwise native reinsertion preserves the changed workspace.

State defaults to `$XDG_STATE_HOME/kelpmux` or `~/.local/state/kelpmux` with private directories/files. Journal entries older than seven days are pruned during scanning or inspection. Live agents remain in the registry even when running longer than seven days. Kelpmux stores no prompts, responses, tool arguments, credentials or transcripts.

## CI and releases

CI checks formatting, Clippy, unit tests, Rust 1.85, release compilation, RustSec advisories and Rust dependency licenses. Native terminal tests build checksum-verified upstream tmux 3.8 on Linux x86-64 and ARM64. Actions are pinned to reviewed commits.

Tags `v*`, published releases and manual dispatch validate and build Linux archives with Kelpmux, the unmodified tmux 3.8 binary, upstream license/source digest, documentation and checksums. These binaries are installed and started explicitly; apt's tmux remains independent. See [release procedure](docs/releases.md).

Main is the **0.1.0-beta.2 development preview**. The published [v0.1.0-beta.1](https://github.com/joanfabregat/kelpmux/releases/tag/v0.1.0-beta.1) still uses the earlier nested-popup implementation. Keep its helper/configuration separate when trying this native version, and do not mix their state directories. Existing legacy tmux sessions can keep running with the old helper while you test a new server.
