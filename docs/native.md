# Native terminal workspace

The Linux server owns persistent shell PTYs and Alacritty terminal state. Attached clients render the interface and route input. Detaching preserves terminals, pane IDs, history and processes; stopping a workspace ends its processes. A crash or reboot requires starting another workspace and resuming agents through their own TUIs.

## Named workspaces

```sh
kelpmux start --workspace beta-trial --config examples/native-workspace.json
kelpmux attach --workspace beta-trial
kelpmux status --workspace beta-trial
kelpmux recover --workspace beta-trial
kelpmux stop --workspace beta-trial
```

`start` launches a detached server independently of the starting shell. Repeating it reuses a live instance. `attach` requires an existing workspace. `ssh-entry` starts or attaches; `status` reports its current pane processes; `recover` prints seven-day session metadata and copyable resume commands without executing them. `stop` requests shutdown, ending all owned shells and jobs. Use detach when they should continue.

The default name is `main`. State lives under `$XDG_STATE_HOME/kelpmux/workspaces/NAME`, or `~/.local/state/kelpmux/workspaces/NAME`; its socket is `control.sock`. Names contain 1–48 letters, numbers, hyphens or underscores. Persistent effective settings are in `workspace.json`, startup diagnostics in `server.log`. Private state directories, owned sockets, instance locks and same-user peer checks guard endpoints. Keep independent alpha and tmux instances alive during beta trials; no command migrates their terminals.

Foreground operation remains available:

```sh
kelpmux server --socket /absolute/private/control.sock --state-dir /absolute/private/state --shell /bin/zsh --cwd /home/joan/repos --size 160x50
kelpmux attach --socket /absolute/private/control.sock
kelpmux ctl --socket /absolute/private/control.sock list
kelpmux ctl --socket /absolute/private/control.sock layout 3x2
kelpmux ctl --socket /absolute/private/control.sock focus ID
kelpmux ctl --socket /absolute/private/control.sock read ID
kelpmux ctl --socket /absolute/private/control.sock fullscreen ID
kelpmux ctl --socket /absolute/private/control.sock close
```

`ctl close` returns to the grid; it does not terminate a terminal. Stable pane IDs differ from the current one-based tab numbers. Management commands are explicit local administration and do not take interactive control.

## Grid and tabs

The grid is an overview of the same terminals represented by tabs. The six supported column × row grids are `1x1`, `2x1`, `2x2`, `3x2`, `2x3` and `3x3`. Switching view preserves processes, identities and the underlying grid. Only the active terminal is drawn in tab view; hidden terminals continue running. Their PTYs retain base-grid dimensions while the visible terminal uses its view dimensions.

Tab placement can be `auto`, `top`, `left` or `bottom`. Auto uses available character geometry rather than claiming to identify the device: narrow terminals get a compact previous/current/next strip, medium landscape terminals can use a sidebar, and broad windows use a top strip. Explicit placement overrides this. Settings opens a popup configuration panel with clickable placement choices; changes are retained in workspace settings for restart. Help, configuration and confirmations are dialogs, while agent terminals themselves use tabs rather than floating popups.

Tab reading width defaults to 120 columns; full width is available for tables. Changing this width adjusts the visible terminal, not the number of running terminals. Closing a tab asks for confirmation before ending its process and owned jobs. Cancel and Escape leave it running. Other tabs remain alive; closing the last tab creates a replacement shell before removing the old one, so a spawn failure cannot leave the workspace unusable.

Pane colors are stable by identity. The selected title is white and others gray; the number has a distinct color. Agent status uses a braille working spinner and static attention/completion/failure/idle/exited markers. The footer includes Vitals-derived CPU, memory, swap, disk usage and local clock readings, clipped to the available width. CPU sampling stays in memory; disk sampling is cached.

## Keys and mouse

The default prefix is Ctrl-B; release it before the next key. Ordinary application Alt-f, Alt-b and modified arrows preserve their bytes. Bracketed paste bypasses shortcut routing, remains literal and is bounded to 64 KiB.

| Shortcut | Action |
| --- | --- |
| Prefix then `z`, Alt+Tab or Alt+0 | Switch grid / tabs |
| Prefix then `1`…`9`, or Alt+1…9 | Select the numbered terminal in tab view |
| Prefix then `f` | Toggle full / capped terminal width |
| Prefix then `g` | Graphical six-grid picker |
| Prefix then `a`, `!`, `n` | Agents, attention, finished filters |
| Prefix then `s` | Graphical pane movement; choose destination |
| Prefix then `h`, `j`, `k`, `l` | Directional focus |
| Prefix then `+`, `-` | Increase / decrease reading width |
| Prefix then `,` | Configuration panel |
| Prefix then `?` | Sticky help |
| Prefix then `d` | Detach without terminating processes |
| Prefix then `t` or `T` | Explicitly take control |
| Prefix then `[` | History scrolling mode |
| Scroll-mode arrows / Page Up / Page Down | Move through history |
| `y` in scrolling mode, or prefix then `y` | Copy selection |
| Escape or `q` in scrolling mode | Leave scrolling mode |
| Prefix then Ctrl-B | Send a literal Ctrl-B |

Tabs, footer controls and grid cards are clickable. Grid choices requiring too many occupied panes to disappear are disabled, and the server independently checks occupancy again before applying them. Picker navigation uses arrows and Enter; numbered targets refer to the current logical order. Dialogs consume application input and close through Escape, their close control or outside clicks; an outside click cancels a confirmation.

Application mouse reporting receives body-relative events when enabled. Otherwise the wheel scrolls history and local drags select text. Hold Shift to select or scroll locally even in a mouse-aware application. Releasing a local selection copies through OSC 52; clipboard support depends on the SSH terminal client.

## Configuration

The [workspace example](../examples/native-workspace.json) contains a startup grid and UI settings. Pass it with `start --config PATH` or foreground `server --config PATH`. Saved effective shell, working directory and UI settings are used when restarting a named workspace. Explicit `--shell` and `--cwd` override configuration.

UI settings include a control-key prefix number (`2` is Ctrl-B), `tab_position`, `read_width` and an ASCII key-to-action `bindings` map. Supported action names are validated at startup; unsupported options and dimensions fail clearly. Bindings can use `unbound` to remove an existing shortcut. Interactive help reflects the configured prefix and bindings. Configuration files are regular JSON files bounded to 64 KiB. The legacy `read_height_percent` option is accepted for compatibility; tabs always use the available content height.

## SSH and RootShell

A normal interactive SSH login may automatically attach tmux before a terminal app sends its launch command. Sending a command after connection can therefore type it into an agent instead of running it in a shell. Do not use that route for Kelpmux.

On Joan's managed jbox shell, a dedicated RootShell profile can use:

- Terminal Type → Custom → `xterm-kelpmux`.
- Auto-start multiplexer → Off.
- Launch Command → empty.

The early shell entry resets `TERM` to `xterm-256color` and executes `~/.local/share/kelpmux/kelpmux ssh-entry --workspace main`, preserving the existing interactive SSH and Secure Enclave authentication route. Normal profiles retain their tmux route. A missing executable leaves an ordinary shell with a visible message. Kelpmux child terminals use normal terminal settings and remove SSH markers, preventing recursive attachment. The project does not install this machine-specific shell hook automatically.

Already connected through tmux? Its command prompt can detach and execute the native client without sending input into an application:

```text
detach-client -E 'exec /home/joan/.local/share/kelpmux/kelpmux ssh-entry --workspace main'
```

Enter that at Ctrl-B then `:` in tmux, not at an agent prompt. A future native client will use a separate SSH protocol channel; see [native client boundary](native-client-protocol.md).

## Control, sizing and safe shrink

The first eligible attached client controls the workspace. Additional clients observe it and cannot send application input or change it without explicit takeover. Controller dimensions determine PTY sizes; observer resizes only clip or pad their display. Supported sizes are 40–400 columns and 10–160 rows. A window resize never removes a terminal.

Growing creates interactive shells. Shrinking removes only the required number of recognized idle shells, regardless of their positions; busy terminals retain their identities and processes. Foreground applications, background jobs and stopped jobs count as occupied. Pending input and a short input cooldown prevent immediate shrink from discarding a just-submitted command. Insufficient unused terminals leave the old grid and all processes unchanged. Linux process checks cannot prove that every blocked shell built-in is an idle prompt, and fully detached processes outside the tracked tree are outside this guard.

## Discovery and recovery metadata

Claude Code and Codex are discovered without requiring a Kelpmux launcher. Process identity, session files and supported daemon metadata are evidence sources; unknown or ambiguous session IDs remain unknown. A filtered agent picker surfaces attention and finished sessions. Optional hooks provide stronger lifecycle evidence after discovery registers an agent:

```sh
kelpmux native-event codex --pane ID --state-dir SERVER_STATE
kelpmux native-event claude --pane ID --state-dir SERVER_STATE
```

The hook reads agent JSON from standard input, bounded to 64 KiB. Pane and state default to `KELPMUX_PANE` and `KELPMUX_STATE_DIR`, set by the server. Explicit flags override them. No hooks are installed automatically. The receiver validates process birth identity, registered kind and session conflicts; held journal locks fail quickly. See [discovery details](discovery.md) for supported evidence and hook payloads.

Metadata lives under `SERVER_STATE/native`. The seven-day journal records agent kind, name, session identity, pane/workspace placement and lifecycle times, with no terminal transcript, prompts, responses or credentials. Recovery prints copyable `codex resume SESSION` or `claude --resume SESSION` commands when an ID is known. It never executes them or restores processes. Existing metadata commands work directly against the nested registry:

```sh
kelpmux --state-dir "$SERVER_STATE/native" journal
kelpmux --state-dir "$SERVER_STATE/native" name --pane %ID 'Auth migration'
```

## Validation boundary

Automated fixtures use disposable shell PTYs and private sockets. They cover terminal emulation, color/query replies, input/paste/mouse framing, reconnect and ownership, safe grids, responsive geometry, settings/confirmation routing, discovery metadata, bounded RPC replies and named lifecycle behavior. They do not launch real Claude/Codex sessions. Hands-on RootShell, the 32-inch monitor, MacBook Air and tablet/phone UI validation remain separate from simulated character-size profiles.
