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
kelpmux ctl --socket /absolute/private/control.sock new
kelpmux ctl --socket /absolute/private/control.sock park ID
kelpmux ctl --socket /absolute/private/control.sock restore ID
kelpmux ctl --socket /absolute/private/control.sock focus ID
kelpmux ctl --socket /absolute/private/control.sock read ID
kelpmux ctl --socket /absolute/private/control.sock fullscreen ID
kelpmux ctl --socket /absolute/private/control.sock close
```

`ctl close` returns to the grid; it does not terminate a terminal. Stable pane IDs differ from the current one-based tab numbers. Management commands are explicit local administration and do not take interactive control.

## Grid and tabs

The grid is an overview of the unparked terminals represented by tabs. Geometry is automatic from their count and the controller's character dimensions, and recomputed when tabs change or the window resizes. Rows divide the available width evenly, including incomplete rows: five terminals use three cells above two wider cells. Narrow screens favor fewer columns. Switching views preserves processes and identities. Only the active terminal is drawn in tab view; hidden terminals continue running. Parked terminals retain their last PTY size and are excluded from the grid. Manual layout selection and separator resizing are removed.

Use the ⊟ tab control or prefix then `p` to request parking a tab. The Parked control in the tab bar or prefix then `u` opens a table with tab colors, aligned names/statuses and animated working markers; select a row or its ↥ restore icon to request restoration. Both actions open a compact confirmation: Enter confirms, Esc cancels. Explicit administrative `ctl park`/`ctl restore` commands remain direct operations. Parking hides the tab and preserves its PTY, process, output, status and session identity. It does not suspend processes. State survives client detach/reconnect while the server lives; a daemon stop or crash still needs ordinary session recovery. Parked tabs count toward the nine-terminal limit, and their attention notifications remain available. All tabs may be parked; the tab-bar controls remain reachable, or use prefix then Enter to create a tab. Observers cannot change parking without taking control.

The footer uses a lighter gray background across the whole bottom row. Padded Settings, compact Grid/Tabs and Help/alert controls stay on the left; Vitals remain on the right. There is no full-width footer button; prefix then `f` and the terminal's right-edge +/− control still toggle the reading-width limit. Alerts turn red while unread. Dialogs, help and scratch overlays dim the surrounding composed display without changing the terminal buffers; the active overlay retains its normal colors. Dimming approximates terminal default colors with neutral RGB values and restores the original cell styles when the overlay closes.

Help uses aligned Key and Action columns, alternating row backgrounds and a scroll range indicator; arrow keys or the wheel reveal the remaining entries. Settings groups tab placement, reading width and notifications into separate zones. Choice buttons have one-column gaps and are three rows tall on sufficiently high terminals, with two-row or single-row controls on shorter screens. The whole colored button is clickable; selected values and keyboard focus remain distinct.

Tab placement can be `auto`, `top`, `left` or `bottom`. Auto uses available character geometry rather than claiming to identify the device: narrow terminals get a compact bottom strip, medium landscape terminals can use a sidebar, and broad windows use a top strip. Explicit placement overrides this. Previous/next arrows appear only when all tabs cannot fit: above and below a vertical list, or at the left and right of a horizontal strip. The active tab remains visible as navigation advances through overflow. Settings opens a popup configuration panel with clickable placement choices; changes are retained in workspace settings for restart. Help, configuration and confirmations are dialogs, while agent terminals themselves use tabs rather than floating popups.

Tab reading width defaults to 120 columns; full width is available for tables. Changing this width adjusts the visible terminal, not the number of running terminals. Closing a tab asks for confirmation before ending its process and owned jobs. Cancel and Escape leave it running. Other tabs remain alive; closing the last tab creates a replacement shell before removing the old one, so a spawn failure cannot leave the workspace unusable.

Pane colors are stable by identity. Click a tab number to open its nine-color picker; click a swatch or use arrows and Enter. Choosing a color preserves the active terminal and saves that pane's choice in workspace settings. Tab titles remain white; inactive tabs have neutral backgrounds and the selected tab uses its full muted palette background. A matching vertical line along the sidebar's right edge connects it to the selected tab. Its number has a distinct highlight with two blank columns before the activity marker. Terminal bodies have one blank column on each side, outside application mouse coordinates. Agent status uses a braille working spinner and static attention/completion/failure/idle/exited markers. The footer includes Vitals-derived CPU, memory, swap, disk usage and local clock readings, clipped to the available width. CPU sampling stays in memory; disk sampling is cached.

An explicit template name or discovered conversation name owns the tab title and cannot be overwritten by application OSC title updates. Without a verified name, an application title can supply the label. A directory name alone is not displayed as a conversation name: if neither source supplies a name, the tab shows the agent kind and a short known session ID, or `session unknown`. This does not infer a session identifier from an OSC title or solve ambiguous shared-daemon ownership; see [discovery limits](discovery.md).

## Keys and mouse

The default prefix is Ctrl-B; release it before the next key. Ordinary application Alt-f, Alt-b and modified arrows preserve their bytes. Bracketed paste bypasses shortcut routing, remains literal and is bounded to 64 KiB.

| Shortcut | Action |
| --- | --- |
| Prefix then `z`, Alt+Tab or Alt+0 | Switch grid / tabs |
| Prefix then `1`…`9`, or Alt+1…9 | Select the numbered terminal in tab view |
| Prefix then `f` | Toggle full / capped terminal width |
| Prefix then `g` or `e` | Automatic grid overview |
| Prefix then Enter | New shell tab |
| Prefix then `p` / `u` | Park active tab / restore list |
| Prefix then `a`, `!`, `n` | Agents, attention, finished filters |
| Prefix then `s` | Graphical pane movement; choose destination |
| Prefix then `h`, `j`, `k`, `l` | Directional focus |
| Prefix then `+`, `-` | Increase / decrease reading width |
| Prefix then `,` | Configuration panel |
| Prefix then `v` | Cycle tab placement |
| Prefix then `o`, `b`, `r` | Overview, notifications, recovery |
| Prefix then `N`, `P` | Next question / completion |
| Prefix then `/`, `c` | History search / persistent scratch |
| Prefix then Space | Keyboard selection |
| Prefix then `?` | Sticky help |
| Help arrows / Page Up / Page Down / wheel | Scroll help |
| Prefix then `d` | Detach without terminating processes |
| Prefix then `t` or `T` | Explicitly take control |
| Prefix then `[` | History scrolling mode |
| Scroll-mode arrows / Page Up / Page Down | Move through history |
| `y` in scrolling mode, or prefix then `y` | Copy selection |
| Escape or `q` in scrolling mode | Leave scrolling mode |
| Prefix then Ctrl-B | Send a literal Ctrl-B |

Tabs and footer controls are clickable. Picker navigation uses arrows and Enter; numbered targets refer to the current unparked order. Dialogs consume application input and close through Escape, their close control or outside clicks; an outside click cancels a confirmation. Close confirmation initially selects Close: Enter confirms, Escape cancels. An exited shell removes its tab; finishing an agent inside a still-running shell leaves the shell tab open. Exiting the last main shell ends the workspace when no scratch terminal remains. Unread notifications turn the alert button red; without alerts it uses a separate amber accent.

Application mouse reporting receives body-relative events when enabled. Otherwise the wheel scrolls history and local drags select text. Hold Shift to select or scroll locally even in a mouse-aware application. Releasing a local selection copies through OSC 52; clipboard support depends on the SSH terminal client.

## Configuration

The [workspace example](../examples/native-workspace.json) contains a startup `tab_count` (1–9) and UI settings. Pass it with `start --config PATH` or foreground `server --config PATH`. Saved effective shell, working directory and UI settings are used when restarting a named workspace. Older `grid` settings are accepted as initial tab-count migration input; geometry and weights are omitted from newly saved settings. Explicit `--shell` and `--cwd` override configuration.

UI settings include a control-key prefix number (`2` is Ctrl-B), `tab_position`, `read_width` and an ASCII key-to-action `bindings` map. The settings panel groups Tab placement, Reading width and Notifications into separate zones. Placement and explicit On/Off choices use the same filled/empty radio markers. Clicking an already selected On/Off value leaves it unchanged. Left/right arrows choose a placement, up/down moves between zones, and Enter applies the focused choice or toggles its setting. The panel fits the minimum 40-column, ten-row client. Supported action names are validated at startup; unsupported options and dimensions fail clearly. Bindings can use `unbound` to remove an existing shortcut. Interactive help reflects the configured prefix and bindings. Configuration files are regular JSON files bounded to 64 KiB. The legacy `read_height_percent` option is accepted for compatibility; tabs always use the available content height.

Tab accents and the left sidebar divider use a solid quarter-cell strip, without a box-drawing line over a colored background. Its exact pixel thickness depends on the terminal font and display scale; a terminal UI cannot guarantee a two-pixel stroke.

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

Creating a tab explicitly creates one shell. Grid resizing changes geometry and preserves every process; it does not create spare shells or remove idle ones. Closing a tab explicitly requires confirmation. Legacy native layout commands are rejected without changing any terminals; the older tmux helper retains its separate compatibility behavior.

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

## Overview, notifications and recovery

Prefix then `o` opens an overview with agent names, status and last activity. Prefix then `b` opens persistent notification history: question, explicit completion, failure, exit and agent bell events. Prefix then uppercase `N` jumps to the next question and uppercase `P` to the next completion. Selecting a notification acknowledges it and focuses its terminal; unread markers survive detaching and server restarts. Initial discovery of an already-finished agent does not generate a completion flood. Settings includes optional notification sound, disabled by default; the terminal client decides whether to play a received bell. Notifications are metadata, bounded to 256 entries and seven days.

Prefix then `r` loads the recovery dashboard from the current and sibling named workspaces, grouped by workspace and working directory. Loading runs outside the terminal I/O loop; held journals are skipped rather than blocking agents. Choose a session and copy its validated resume command through OSC 52. This never executes a command. Recorded status is an observation, not proof that a process survived a crash. Unknown session IDs remain unknown. The command-line `recover` exposes the same grouped metadata.

## Scratch, search and selection

Prefix then `c` toggles a persistent scratch terminal overlay above either grid or tabs. Hiding it preserves its shell and jobs, and restores the underlying terminal. It has its own close confirmation; closing it ends its jobs. It does not consume a main-grid slot, affect safe-shrink eligibility or replace the last main shell. Prefix then `f` expands scratch to the available width and height while it is visible. Agent terminals remain inline tabs.

Prefix then `/` opens history search. Type a literal query, use Tab to switch current/all-terminal scope, and Enter to search or reveal the selected result; Ctrl-Y copies a result. Results are bounded to 100. Primary history is retained while an application uses an alternate screen, but revealing primary history requires leaving that application's alternate screen first. Matching text can still be copied. Search is memory-only and does not write transcripts to the journal.

Prefix then Space starts keyboard selection at the visible cursor. Arrow keys extend it; `y` or Enter copies and exits, while Escape cancels selection routing. Prefix then `[` remains ordinary history scrolling. Mouse selection and OSC 52 depend on terminal-client support. Copy payloads are bounded to 64 KiB. Alacritty keeps a bounded in-memory scrollback; stopping a workspace discards it.

## Templates, adaptive views and resizing

`start --template PATH` initializes a new workspace from named panes with absolute working directories and shell paths. Legacy template `grid` supplies its initial number of shell slots; the live layout is computed automatically. Starting an already-live workspace never replaces its terminals. Template names remain stable despite shell-title changes. Effective settings retain the template and exact tab count for restart. See [template example](../examples/native-template.json); replace its directories with your own. `kelpmux templates --workspace NAME` lists validated JSON templates in that workspace's private `templates` directory; it does not launch them.

On narrow terminals, automatic view selection uses a compact tab strip without removing terminals. View mode and reading width are remembered separately for compact, medium and wide column profiles. Explicit tab placement overrides automatic placement. Grid cells rebalance evenly as tabs and client dimensions change. Drag a pane header onto another to swap their logical positions; prefix then `s` remains a graphical keyboard alternative.

## Incremental updates and diagnostics

The server sends full frames for new geometry or resynchronization and revision-checked patches for smaller changes. Queues remain bounded; a slow observer receives a replacement snapshot rather than an unbounded patch backlog. `kelpmux benchmark` prints synthetic one-cell payload sizes and diff timings across supported terminal profiles without starting a workspace. These are payload measurements, not a network latency promise. `kelpmux diagnostics --workspace NAME` reports geometry, uptime, client/pane counts and frame/PTY byte counters, excluding terminal contents, paths, names and session IDs. Frame counters describe generated updates for currently connected clients.

## Unreleased reliability and native-client foundation

Development beta.3 adds an experimental pane-stream protocol, SSH stdio bridge and read-only reference client. See [native-client foundation](native-client-protocol.md) for implemented requests, ownership, size limits and deferred features, and [ADR 0001](adr/0001-native-client-protocol-and-device-capabilities.md) for the proposed native/MCP/device architecture. These changes are not active in an existing beta.2 or alpha server.

New servers save private executable/version/protocol identity metadata. `attach` and `ssh-entry` authenticate the live socket and select its exact retained executable under the private installed versions tree, without restarting it. Binary selection pins the executable inode and rejects unsafe, replaced or missing installations. Alpha/development binaries outside that tree require their matching client explicitly. `status` reports server/client versions on stderr while preserving JSON stdout. `--current-client` deliberately skips automatic binary selection for compatible-client troubleshooting; socket ownership and protocol negotiation remain mandatory and mismatches fail before raw terminal mode. A custom socket infers adjacent state unless a workspace/state was supplied; use `--state-dir` when state lives elsewhere.

The overview now shows status evidence and stale/unknown checks. Idle does not imply completion. Compatibility fixtures exercise arbitrary Unicode fragmentation, colors, alternate screens and history; nine-PTY stress tests cover noisy output, reconnects and resizing. Bounded history tests do not establish a hard total-RSS limit. No live workspace is upgraded automatically, and no native app, MCP service or device permissions are installed.

## Unreleased beta.3 reading and tab controls

Terminal content starts at the left edge of its available content area. `wrap_lines` defaults to `true`: the PTY wraps at the configured reading width; disabling it uses the full available width. Toggle it in Settings, with Prefix then `f`, or with the control at the top of the palette-colored right divider. `+` removes the limit; `−` restores it. The divider stays reachable at the extreme right in full-width mode. A blank column separates it from terminal content in both modes. There is no wrap control in the sidebar. This controls PTY dimensions, not an application's own terminal autowrap mode.

The `+` button creates one new shell tab, below the vertical list with a blank row above it, or immediately after the visible horizontal tabs. Existing processes are preserved, and the grid rebalances automatically. At the nine-main-tab limit the button is disabled, including parked tabs. Scratch remains separate. Vertical tabs use up to three character rows, with the title below the number, status and park/close controls, and collapse on compact displays. Long titles pause, scroll slowly once, then return to a static label; changing the title starts a new pass. The compact close confirmation is at most 52 columns by eight rows and presents blue Cancel and red Close tab buttons side by side, with Close selected initially.

Vertical separators, including the full-width right divider, use solid quarter-cell strokes; their exact pixel thickness is font-dependent. Horizontal grid separators remain visible. Separator cells belong to Kelpmux and are outside application input coordinates.

OSC 8 hyperlink targets survive terminal snapshots, composition, deltas and ANSI rendering. Plain HTTP(S) URLs also become links, including soft-wrapped URLs within a pane. Opening a link uses the outer terminal's supported link gesture, often a modifier-click or touch menu when mouse reporting is active. Kelpmux does not launch a browser on the remote host. Explicit targets are limited to 2,048 bytes and reject whitespace/control characters. A matching new server and frontend are needed to use these changes; reconnecting to a running alpha keeps its original popup UI. Settings/help/confirmation dialogs remain popups in the tab-based version.

A small two-tab starting configuration is available at [examples/native-tabs.json](../examples/native-tabs.json). Pass it with `ssh-entry --workspace NEW_NAME --config /absolute/path/to/examples/native-tabs.json` when explicitly starting a separate demo. It opens vertical tabs; use the grid/tab shortcut to see their underlying layout. Use a new workspace name rather than restarting a workspace containing agents.

## Permanent demo launch command

Use `/home/joan/repos/kelpmux/scripts/demo.sh` as the SSH launch command on jbox. The launcher resolves `target/demos/current` once, prints the selected build and connects to `demo-BUILD_ID`. Reconnecting to the same build keeps its workspace and saved settings; promoting a different build starts a separate workspace while earlier agents remain running. The launcher never downloads, compiles, promotes, stops or restarts anything. `--takeover` explicitly requests control, and `--print-command` previews the resolved command without starting it.

For another checkout, use its absolute `scripts/demo.sh` path. A maintainer must first build and validate an immutable `target/demos/BUILD_ID/kelpmux`, then atomically select its directory with the `current` symlink. Merely changing that pointer does not affect an attached client or its server. Keep old build files while their workspaces run.
