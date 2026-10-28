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

The grid is an overview of the unparked terminals represented by tabs. Geometry is automatic from their count and the controller's character dimensions, and recomputed when tabs change or the window resizes. Columns stay aligned across every row. Five terminals occupy five cells of a six-cell grid; unused cells show a dim Inactive placeholder without creating a shell or accepting clicks. Narrow screens favor fewer columns. Switching views preserves processes and identities. Only the active terminal is drawn in tab view; hidden terminals continue running. Parked terminals retain their last PTY size and are excluded from the grid. Manual layout selection and separator resizing are removed.

Restoring from grid view keeps the grid open. Parking and restoration confirmations use the same violet as Parked. Use the ⊟ tab control or prefix then `p` to request parking a tab. The violet Parked control in the status bar or prefix then `u` opens a table with neutral tab surfaces, aligned names and a single-character working animation; select a row or its ↥ restore button to request restoration. Each entry also has a full-height × Close button that opens the ordinary close confirmation, leaving the tab parked until confirmed. Both actions open a compact confirmation: Enter confirms, Esc cancels. Explicit administrative `ctl park`/`ctl restore` commands remain direct operations. Parking hides the tab and preserves its PTY, process, output, status and session identity. It does not suspend processes. State survives client detach/reconnect while the server lives; a daemon stop or crash still needs ordinary session recovery. Parked tabs do not occupy visible grid slots, there is no fixed tab quota, and their attention notifications remain available. All tabs may be parked; the footer and New tab controls remain reachable, or use prefix then Enter to create a tab. Observers cannot change parking without taking control.

In horizontal-tabs and grid modes, the footer uses a lighter gray background across the whole bottom row. Padded Settings, compact Grid/Tabs, violet Parked and Help/alert controls in that order stay on the left; Vitals remain on the right. Left-tabs mode puts metrics and buttons at the bottom of the sidebar. Alerts turn red while unread. Dialogs, help and scratch overlays dim the surrounding composed display without changing the terminal buffers; the active overlay retains its normal colors. Dimming approximates terminal default colors with neutral RGB values and restores the original cell styles when the overlay closes.

Selecting a tab or focusing a grid pane marks that pane’s existing notifications read, retaining the history. New events remain unread until another explicit selection or acknowledgement. The Alerts button indicates unread events belonging to verified current tabs; historical entries do not keep it highlighted.

Help uses aligned Key and Action columns, alternating row backgrounds and a scroll range indicator; arrow keys or the wheel reveal the remaining entries. Settings offers notification sound with separate On/Off buttons. The whole colored button is clickable; selecting the current value leaves it unchanged.

Settings and Help use the base color of their corresponding footer buttons for their headers. Close confirmation headers use red; the parked list, parking and restoration use violet. Confirmation buttons are three rows high when space permits, falling back to shorter buttons on compact screens.

The frontend requests enhanced keyboard reporting from capable SSH terminals, forwards the active application's requested keyboard flags and restores the previous mode on exit. CSI-u and xterm modifyOtherKeys Shift+Enter events become a newline for legacy input, or remain enhanced events for applications that request them. Mux shortcuts and dialogs accept the corresponding enhanced key encodings; key releases do not execute a shortcut twice. Pasted bytes remain literal. A client that sends identical bytes for Enter and Shift+Enter cannot expose the distinction to Kelpmux.

Pane terminals inherit the attached terminal’s default foreground and background; Kelpmux does not paint an opaque terminal body. On attachment the ANSI frontend queries the outer terminal with OSC 10/11. Valid replies provide the controller's foreground/background for application color queries, so applications can choose visible input shading against the actual theme. Observers keep their reports separate until takeover; new panes inherit the current controller's query colors. Application OSC palette changes take precedence, and default cells still inherit the outer theme. Unsupported terminals use the emulator fallback palette; no query reply is invented. Malformed, oversized and incomplete responses are bounded and discarded rather than typed into an application, and pasted bytes remain literal. Color reporting is currently a composed-frontend feature, not per-pane native-stream theme ownership. Child shells identify `TERM_PROGRAM=kelpmux`. Applications that cache their initial palette or keyboard probe need a fresh start after attachment to detect these changes.

The two layouts are left sidebar tabs and the adaptive grid. The sidebar is used at every terminal size; previous/next arrows appear when the tab list overflows, and scrolling the list preserves the active terminal. Legacy `auto`, `top` and `bottom` tab preferences load as `left` and save in that canonical form. Tab placement is no longer a settings choice or a default keyboard shortcut. Existing custom `tab-position` bindings remain harmless compatibility actions. Help, configuration and confirmations are dialogs, while agent terminals themselves use tabs rather than floating popups.

Tab reading width defaults to 120 columns; full width is available for tables. Changing this width adjusts the visible terminal, not the number of running terminals. Closing a tab asks for confirmation before ending its process and owned jobs. Cancel and Escape leave it running. Other tabs remain alive; closing the last tab creates a replacement shell before removing the old one, so a spawn failure cannot leave the workspace unusable.

Tabs, grid headers and parked entries use neutral surfaces derived from the client terminal theme. A brighter fill and edge mark the selected terminal. Tab titles use white on dark surfaces and dark text on light surfaces; selected numbers remain yellow, while inactive numbers in the sidebar and grid use the same blue accent. Old saved pane palette values are accepted for compatibility but do not change the native UI. Clicking a number focuses that terminal. Working terminals use a stationary ten-frame Braille dot spinner; there is no animated background. A static marker is not proof of completion: the agent overview retains state and evidence details. The inline bell indicates unread notification history. Sidebar tabs put number, activity and controls above the conversation name, with a separate folder row below and compact cards on short screens. Agent OSC titles lose a leading braille spinner and matching folder suffix without changing explicit names or inferring activity. Terminal bodies have no blank side margins. The footer includes Vitals-derived CPU, memory, swap and disk usage, clipped to available width. CPU sampling stays in memory; disk sampling is cached.

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
| Prefix then Tab / Shift+Tab | Next / previous unparked tab, including numbers beyond nine |
| Prefix then Page Up / Page Down | Previous / next grid page when panes exceed window capacity |
| Prefix then `p` / `u` | Park active tab / restore list |
| Prefix then `a`, `!`, `n` | Agents, attention, finished filters |
| Prefix then `s` | Graphical pane movement; choose destination |
| Prefix then `h`, `j`, `k`, `l` | Directional focus |
| Prefix then `+`, `-` | Increase / decrease reading width |
| Prefix then `,` | Configuration panel |
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

The native grid favors complete rectangles with even row lengths, then uses terminal proportions to choose their orientation. Nine tabs form 3×3 on normal windows instead of a 5+4 split; extremely wide or tall windows may use a complete single-row or single-column layout. Twelve tabs use 4×3 on a 240×70 terminal and 3×4 on a 120×80 terminal. When the window cannot fit every pane at a minimum of 12 columns and two content rows plus its header, the grid displays a page containing the selected tab. Other shells keep running at their last useful size. Tab cycling, grid-page shortcuts and the overview reach the remaining tabs. Resizing recomputes the grid without restarting shells. Settings and templates accept positive startup counts beyond nine; shortcuts 1–9 remain quick access to the first nine unparked tabs.

The [workspace example](../examples/native-workspace.json) contains a positive startup `tab_count` without a fixed tab quota and UI settings. Pass it with `start --config PATH` or foreground `server --config PATH`. Saved effective shell, working directory and UI settings are used when restarting a named workspace. Older `grid` settings are accepted as initial tab-count migration input; geometry and weights are omitted from newly saved settings. Explicit `--shell` and `--cwd` override configuration.

UI settings include a control-key prefix number (`2` is Ctrl-B) and an ASCII key-to-action `bindings` map. The settings panel offers notification sound with explicit On/Off radio choices. Clicking an already selected value leaves it unchanged; Enter toggles the setting. The panel fits the minimum 40-column, ten-row client. Supported action names are validated at startup; unsupported options and dimensions fail clearly. Bindings can use `unbound` to remove an existing shortcut. Interactive help reflects the configured prefix and bindings. Configuration files are regular JSON files bounded to 64 KiB. Legacy tab placement, reading dimensions and wrapping preferences are accepted for compatibility; tab bodies always use the available content area. Obsolete width settings are omitted when saving settings.

Sidebar tab accents use a half-cell strip while the sidebar divider uses a thin centered stroke, without a box-drawing line over a colored background. Its exact pixel thickness depends on the terminal font and display scale; a terminal UI cannot guarantee a two-pixel stroke.

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

Prefix then `o` opens an overview with agent names, status and last activity. Prefix then `b` opens unread notifications for current tabs, newest first: question, explicit completion, failure, exit and agent bell events. Click Show history or press `h` to include acknowledged and historical events; click Mark all read or press `a` to acknowledge the entire history. Each opening defaults to unread events. Prefix then uppercase `N` jumps to the next question and uppercase `P` to the next completion. Selecting a notification acknowledges it and focuses its terminal; unread markers survive detaching and server restarts. Initial discovery of an already-finished agent does not generate a completion flood. Settings includes optional notification sound, disabled by default. The controlling frontend emits a standard terminal bell when a new unread notification arrives; observers remain silent and initial attachment does not replay existing alerts. The terminal client decides whether that bell produces sound, a visual flash, or nothing. Notifications are metadata, bounded to 256 entries and seven days.

Prefix then `r` loads the recovery dashboard from the current and sibling named workspaces, grouped by workspace and working directory. Loading runs outside the terminal I/O loop; held journals are skipped rather than blocking agents. Choose a session and copy its validated resume command through OSC 52. This never executes a command. Recorded status is an observation, not proof that a process survived a crash. Unknown session IDs remain unknown. The command-line `recover` exposes the same grouped metadata.

## Scratch, search and selection

Prefix then `c` toggles a persistent scratch terminal overlay above either grid or tabs. Hiding it preserves its shell and jobs, and restores the underlying terminal. It has its own close confirmation; closing it ends its jobs. It does not consume a main-grid slot, affect safe-shrink eligibility or replace the last main shell. Prefix then `f` expands scratch to the available width and height while it is visible. Agent terminals remain inline tabs.

Prefix then `/` opens history search. Type a literal query, use Tab to switch current/all-terminal scope, and Enter to search or reveal the selected result; Ctrl-Y copies a result. Results are bounded to 100. Primary history is retained while an application uses an alternate screen, but revealing primary history requires leaving that application's alternate screen first. Matching text can still be copied. Search is memory-only and does not write transcripts to the journal.

Prefix then Space starts keyboard selection at the visible cursor. Arrow keys extend it; `y` or Enter copies and exits, while Escape cancels selection routing. Prefix then `[` remains ordinary history scrolling. Mouse selection and OSC 52 depend on terminal-client support. Copy payloads are bounded to 64 KiB. Alacritty keeps a bounded in-memory scrollback; stopping a workspace discards it.

## Templates, adaptive views and resizing

`start --template PATH` initializes a new workspace from named panes with absolute working directories and shell paths. Legacy template `grid` supplies its initial number of shell slots; the live layout is computed automatically. Starting an already-live workspace never replaces its terminals. Template names remain stable despite shell-title changes. Effective settings retain the template and exact tab count for restart. See [template example](../examples/native-template.json); replace its directories with your own. `kelpmux templates --workspace NAME` lists validated JSON templates in that workspace's private `templates` directory; it does not launch them.

On narrow terminals, automatic view selection uses sidebar tabs without removing terminals. View mode is remembered separately for compact, medium and wide column profiles. Grid cells rebalance evenly as tabs and client dimensions change. Drag a pane header onto another to swap their logical positions; prefix then `s` remains a graphical keyboard alternative.

## Incremental updates and diagnostics

The server sends full frames for new geometry or resynchronization and revision-checked patches for smaller changes. Queues remain bounded; a slow observer receives a replacement snapshot rather than an unbounded patch backlog. `kelpmux benchmark` prints synthetic one-cell payload sizes and diff timings across supported terminal profiles without starting a workspace. These are payload measurements, not a network latency promise. `kelpmux diagnostics --workspace NAME` reports geometry, uptime, client/pane counts and frame/PTY byte counters, excluding terminal contents, paths, names and session IDs. Frame counters describe generated updates for currently connected clients.

## Unreleased reliability and native-client foundation

Development beta.3 adds an experimental pane-stream protocol, SSH stdio bridge and read-only reference client. See [native-client foundation](native-client-protocol.md) for implemented requests, ownership, size limits and deferred features, and [ADR 0001](adr/0001-native-client-protocol-and-device-capabilities.md) for the proposed native/MCP/device architecture. These changes are not active in an existing beta.2 or alpha server.

New servers save private executable/version/protocol identity metadata. `attach` and `ssh-entry` authenticate the live socket and select its exact retained executable under the private installed versions tree, without restarting it. Binary selection pins the executable inode and rejects unsafe, replaced or missing installations. Alpha/development binaries outside that tree require their matching client explicitly. `status` reports server/client versions on stderr while preserving JSON stdout. `--current-client` deliberately skips automatic binary selection for compatible-client troubleshooting; socket ownership and protocol negotiation remain mandatory and mismatches fail before raw terminal mode. A custom socket infers adjacent state unless a workspace/state was supplied; use `--state-dir` when state lives elsewhere.

The overview now shows status evidence and stale/unknown checks. Idle does not imply completion. Compatibility fixtures exercise arbitrary Unicode fragmentation, colors, alternate screens and history; nine-PTY stress tests cover noisy output, reconnects and resizing. Bounded history tests do not establish a hard total-RSS limit. No live workspace is upgraded automatically, and no native app, MCP service or device permissions are installed.

## Unreleased beta.3 reading and tab controls

Terminal content fills the available content area from its left edge to the right edge of the window. Left-tabs mode reserves only the sidebar and its divider; horizontal-tabs mode uses the full terminal width. There is no reading-width limit, right divider, or width toggle. Applications wrap at the actual PTY width using their own terminal autowrap mode.

The `+` button creates one new shell tab. In left-sidebar mode, New tab touches the last visible tab and is three rows tall on sufficiently high terminals, compact on short screens. Parked is a one-row violet footer button available in every view; its count appears as (N) only when at least one tab is parked. New tab uses a neutral background; in the sidebar it follows the client terminal theme. Horizontal tab bars use two rows, place the five-column New tab button directly against the last visible tab, with remaining space reserved for overflow navigation. Overflow arrows bracket the visible tab list. The mouse wheel and overflow arrows scroll that list without selecting another terminal or resizing its PTY; clicking a tab selects it. Selecting a tab or creating a new tab brings that selection into view. Parked entries use three rows each, with aligned names and a stationary single-character spinner while Working, and full-height Restore and Close buttons. Existing processes are preserved, and the grid rebalances automatically. There is no fixed tab quota, and parked tabs do not consume visible grid slots. Scratch remains separate. Vertical tabs use up to three character rows, with number, activity and park/close controls above the conversation name and a separate folder row below it, and collapse on compact displays. Long titles pause, scroll slowly once, then return to a static label; changing the title starts a new pass. The compact close confirmation is at most 52 columns by nine rows and presents blue Cancel and red Close tab buttons side by side, with Close selected initially.

The sidebar uses a quarter-cell strip aligned to the tab edge; grid separators use a centered light vertical stroke and shaded header rows. Tab selection uses a brighter fill. Sidebar tabs use a separately colored thin underline along the bottom edge of the folder row, preserving folder text, text color and tab height. The exact stroke thickness depends on the terminal font. Grid rows meet directly at shaded pane headers, without blank separator rows. Vertical grid strips use the neutral focus shade of the pane on their left. Inactive grid terminal colors blend mildly (25%) toward the client’s background, retaining their hues; selecting a pane restores its original application colors without changing its terminal buffer. Separator cells belong to Kelpmux and are outside application input coordinates.

OSC 8 hyperlink targets survive terminal snapshots, composition, deltas and ANSI rendering. Plain HTTP(S) URLs also become links, including soft-wrapped URLs within a pane. Surrounding parentheses, nested brackets, quotes and adjacent prose are excluded from link targets; balanced parentheses inside URLs and IPv6 address brackets are preserved. Opening a link uses the outer terminal's supported link gesture, often a modifier-click or touch menu when mouse reporting is active. Kelpmux does not launch a browser on the remote host. Explicit targets are limited to 2,048 bytes and reject whitespace/control characters. A matching new server and frontend are needed to use these changes; reconnecting to a running alpha keeps its original popup UI. Settings/help/confirmation dialogs remain popups in the tab-based version.

A one-shell starting configuration is available at [examples/native-tabs.json](../examples/native-tabs.json). Pass it with `ssh-entry --workspace NEW_NAME --config /absolute/path/to/examples/native-tabs.json` when explicitly starting a separate demo. It opens vertical tabs; use the grid/tab shortcut to see their underlying layout. Use a new workspace name rather than restarting a workspace containing agents.

## Permanent demo launch command

Use `/home/joan/repos/kelpmux/scripts/demo.sh` as the SSH launch command on jbox. The launcher resolves `target/demos/current` once, prints the selected build and connects to `demo-BUILD_ID`. Reconnecting to the same build keeps its workspace and saved settings; promoting a different build starts a separate workspace while earlier agents remain running. The launcher never downloads, compiles, promotes, stops or restarts anything. `--takeover` explicitly requests control, and `--print-command` previews the resolved command without starting it.

For another checkout, use its absolute `scripts/demo.sh` path. A maintainer must first build and validate an immutable `target/demos/BUILD_ID/kelpmux`, then atomically select its directory with the `current` symlink. Merely changing that pointer does not affect an attached client or its server. Keep old build files while their workspaces run.

Idle shell tabs display muted italic `shell` instead of a shell prompt’s user/host OSC title. The folder subtitle remains visible. Explicit tab names and template names take precedence; running applications may still supply their own terminal titles, and discovered agents retain their conversation names. When an application exits back to an idle shell, the label returns to `shell`.

The left tab bar derives its surface from each client’s reported terminal background, slightly lighter for dark themes or darker for light themes. Clients that do not report a background retain the neutral fallback. The tab bar uses the client terminal theme: inactive tabs have the darkest surface, boundary strokes a slightly lighter surface, and selected tabs a lighter surface again. Light themes reverse the shade progression. Text keeps its contrasting color and there is no thick focus strip. Park and Close use purple and red symbols on the surrounding tab or grid-header background, with their padded click areas preserved. Grid headers expose only Park and Close on the right. Use the footer or keyboard to switch views. Clicking a tab or grid number selects that terminal; old saved palette values remain compatible but no longer recolor tabs. Park and Close retain their confirmation dialogs.

In grid view, the first click on an unfocused terminal only focuses it. The associated drag and release are consumed too; another click interacts with the application or starts text selection. This applies in the frontend and to direct composed-UI mouse packets handled by the daemon. Native pane-stream clients retain their independent input protocol.

Canceling restoration returns to the parked list with the same entry selected. The empty list shows a centered explanation rather than empty table headings. Parking controls and the list header share violet; the footer’s entire Parked hit target is filled so it meets Help without a gray gap. Tab-location choices in Settings use one background, radio markers show the saved choice and a yellow label shows keyboard focus.

## System status popup

Click the metrics in the sidebar or grid footer to open live system status. CPU opens the Main process view sorted by CPU; memory and swap select memory sorting, and disks open I/O. The popup adapts to the terminal size, showing a compact table on small screens and htop-style CPU, RAM, swap and disk bars with colored pipes, blank tracks and readings inside brackets above the process table on larger screens. Wide popups use three equal columns. CPU meters occupy the first two columns with RAM and swap below; disk rows pair across those same columns, showing the mount path followed by a bar with usage percentage and available/total space in parentheses. Load, uptime/date/time, network rates and disk throughput start at the top of the third column. Task counts sit at the right of the search toolbar above the process table and yield to long searches or narrow windows. The PID field fits the largest sampled PID with a separating space before USER. Narrow meter cells show percentages without memory byte totals to keep labels and bars readable. Date/time includes the server’s local timezone abbreviation and shares a line with uptime separated by · when the details column is wide enough; narrower columns wrap uptime onto the next line. The footer and sidebar show metrics without date or time; the sidebar returns the removed clock row to the tab list. The metric area uses only the rows it needs. When space is limited, remaining CPU cores share an averaged bar so RAM, swap and the process table stay visible.

Tab switches Main/I/O. Click a column heading or use `c` (CPU), `m` (memory), `p` (PID), `r` (reads), or `w` (writes) to sort; repeating a sort reverses its direction. `/` starts a PID, user or executable search; Enter finishes editing, Backspace removes a character, and Escape closes the popup. Arrow keys, Page Up/Down, Home/End and the mouse wheel scroll the table. The close button or a click outside also closes it. Process executable names are displayed without command arguments.

Detailed sampling runs in a background worker about once per second while the popup is open and a client is attached. Closing it stops new collection and discards the snapshot. CPU and throughput rates require two samples; missing or unreadable per-process I/O appears as a dash. Process identity uses PID plus start time, so a reused PID never inherits the old process's usage. Snapshots and search text are not saved to workspace settings.

Detached native workspaces continue draining PTYs and discovering agents, so applications and notification history survive disconnects. With no attached display or pane-stream client, the server skips metrics sampling and screen composition, pauses title/spinner animation, and polls idle-shell state once per second. Whole-process-table scans for unused-shell UI hints run only with an attached display, at most twice per second. PTY and client socket readiness still wakes the server immediately; reconnecting paints the current terminal contents. Detached workspaces are never automatically terminated just because they contain no jobs.

## Daily-use and development channels

Keep ordinary SSH/tmux as the default. Explicit `kelpmux-preprod` and `kelpmux-dev` entry commands call the installed channel manager with `launch preprod` and `launch dev`. They use the stable workspace names `preprod` and `dev`, each with its own socket, settings, logs, history and PTYs under the ordinary named-workspace state directory. Disconnecting preserves that channel's shells. New workspaces use the selected build's one-shell defaults; reconnecting preserves saved settings.

The channel manager is installed at `$XDG_DATA_HOME/kelpmux/channel-manager` (default `~/.local/share/kelpmux/channel-manager`), pointing to an immutable copy of `scripts/channels.sh`. Registered artifacts live in `kelpmux/builds/COMMIT/` with the executable, launch defaults, manager and SHA-256 manifest. Runtime selection is independent of the source checkout and `target/` cleanup. The three small entry commands are maintained in Joan's dotfiles repository.

```sh
kelpmux-dev
kelpmux-preprod
kelpmux-channels status
kelpmux-channels promote preprod
kelpmux-channels rollback preprod
```

Status reports each channel's selected build, previous build, and running build/PID/protocol when a live server identity matches. Promotion selects exactly dev's registered artifact for preprod, recording the previous selection atomically. Rollback swaps the selected and previous builds. These selection operations do not start, stop or restart a workspace immediately. Dev connections automatically apply a selected build change as described below. Reconnecting to preprod uses its retained matching client, even after promotion. Applying a different preprod server build requires explicitly running `kelpmux-channels stop preprod`, which ends that channel's shells and jobs; launch preprod again when ready. Neither channel hot-migrates live processes. A crash or reboot still requires ordinary agent session recovery.

For a new candidate, run the required local checks, build the release executable and wait for CI on that exact commit to pass. Only then register it and select it for dev. Keep old registered artifacts while any server uses them or a rollback points to them. Registration refuses to replace an existing build ID with different binary, defaults or manager contents. These commands perform no downloads, credential lookup or automatic update:

```sh
bash scripts/channels.sh register COMMIT target/release/kelpmux examples/native-tabs.json
bash scripts/channels.sh install-manager COMMIT
kelpmux-channels select dev COMMIT
# Test dev interactively before explicitly promoting it.
kelpmux-channels promote preprod
```

Use the full Git commit hash for registration. `launch CHANNEL --print-command` previews the exact SSH entry command without creating a workspace. For an SSH client profile that accepts a remote command, use `~/bin/kelpmux-preprod` or `~/bin/kelpmux-dev`; ordinary SSH logins continue to use the current tmux setup. The old demo entry remains available for its existing workspace and is not migrated or restarted by installing channels.

The dev channel enables a bold red `DEV` badge in the grid status bar and a centered red indicator in the left tab sidebar. Preprod leaves it off. `KELPMUX_DEV_BADGE=0 kelpmux-dev` disables this feature for the next server start; `1` enables it. The flag is transient, does not alter saved workspace settings, and is removed from child shells so launching another channel does not inherit the badge accidentally. An already-running server keeps its current flag until explicitly stopped and relaunched.

Starting with composed protocol v31, each fully started server writes a durable `server.running.json` marker. A completed orderly shutdown removes it. If the next server finds the marker left after a crash, forced termination or reboot, it automatically opens the Recovery popup on the first controlling attachment. Closing the popup acknowledges it for that server run; reconnects do not reopen it. Fresh workspaces and clean stop/start cycles do not trigger it. Background startup without a terminal defers the popup until attachment. Recovery still offers recorded agent resume commands and never runs them or restores shell processes automatically. Servers predating the run marker cannot retroactively identify an unclean exit.

Dev follows the selected validated build on every connection. Before attachment, its launcher authenticates the current server, compares its executable identity with the selected artifact, and gracefully replaces it when they differ. This intentionally ends the old dev shells and jobs, disconnects old dev clients, and preserves the workspace settings. Matching builds reconnect without restarting. The startup lock serializes simultaneous dev connections so they converge on one server. Preprod does not opt into this policy and keeps running its current server until an explicit stop/start.

Publishing means building an artifact, passing CI on its exact commit, registering it, installing its matching manager, and selecting it for dev. A raw Git push alone does not install an unvalidated build. Once publishing completes, the next `kelpmux-dev` connection loads that selected build automatically; no separate stop command is needed. The installed manager uses its own native startup helper to support selecting older retained builds for a dev rollback. The helper's explicit `start --replace-running --server-binary PATH` operation validates the target and startup settings before requesting shutdown, waits for the previous endpoint and locks to close, and starts the chosen executable. No update is forced into an already-connected session until another dev connection requests it.

In the left tab bar, the DEV indicator uses red, spaced lettering on a gray panel that contrasts slightly with the sidebar. It fills up to three rows centered in unused space. A full tab list moves it to the metrics separator; crowded compact sidebars use a shorter DEV label between the metric name and bar. Grid status bars also use red lettering on gray. CPU, memory and swap use responsive horizontal bars in the sidebar, with colored filled blocks, muted empty blocks and right-aligned percentages. The sidebar shows only disk pies and percentages on one line, without mount prefixes. A Disks label is included when space allows. The disk line opens the I/O process view. The compact footer retains vertical bar glyphs with contrasting tracks.

The composed frontend requests all-motion mouse reporting (1003) alongside button/drag reporting (1002) and SGR coordinates (1006). Supporting SSH terminals can highlight clickable buttons, tabs, grid headers, metrics and dialog items on hover. Highlighting uses the existing click geometry and stays local to that frontend: it does not change tab focus, dialog selection or server state. Consecutive idle-motion events coalesce, movement inside one control does not repaint it, and hover-only paints are capped at about 30 per second. Resizing or losing terminal focus clears hover. Touch clicks do not enable hover, and normal clicks, drags and keyboard commands retain their behavior. Application bodies receive passive movement only when the application explicitly requests all-motion reporting. The client disables motion reporting on exit. Terminal support determines whether unpressed movement events are delivered at all.

Hovering over any part of the metrics highlights the whole block, including its separators, since it opens the same status popup. Clicking individual metric rows still selects their related process sort. The metrics block and New tab button share a subtler background highlight than other controls. The DEV panel remains an environment label rather than a clickable button.

For shared-daemon Codex clients whose conversation cannot be discovered automatically, prefix then `i` opens the conversation picker for the active tab; prefix then `o`, select a tab and press `l` provides the same action from overview. Choose a loaded conversation to monitor its live status, or the first row to clear an explicit link. The link belongs to that agent process and disappears when it exits. The [polling adapter interface](agent-polling.md) shares discovery and scheduling between Codex and Claude while keeping their metadata readers replaceable. Alert history now distinguishes historical entries from current tab numbers and never navigates by a reused pane ID alone. Sidebar CPU, memory and swap bars have two extra columns of left spacing; the status popup retains its htop-style meters.

In left-tabs mode, drag the sidebar's vertical divider with the left mouse button to resize it. A dotted handle and hover highlight identify the divider. The preferred column width is saved as `ui.sidebar_columns` in workspace settings and survives reconnects and restarts. Width is clamped between 18 columns and half the current window; resizing a small window temporarily clamps the displayed width without discarding the preference. An absent or null preference keeps the automatic screen-based width. Dialogs, scratch overlays and observer connections cannot resize the sidebar. The drag belongs to KelpMux and is not forwarded to the shell.

Double-click a grid header or sidebar tab to toggle full-screen zoom. The two left clicks must be within 400 ms and one character cell; dragging, controls, modified clicks and observer input do not trigger zoom. Zoom hides the sidebar, keeps the pane header and footer, and resizes only the selected PTY to the available full width. Double-clicking the zoomed header restores the previous grid or tab view. Prefix then `f` uses the same zoom state. Other shells keep running, and zoom never creates or replaces a shell.
