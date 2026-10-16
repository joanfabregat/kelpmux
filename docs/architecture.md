# Architecture

Native Kelpmux is a Linux terminal multiplexer with a persistent server and separate attached clients. The server owns PTYs, interactive shells, terminal emulation, workspace geometry, agent metadata and client authorization. The frontend owns raw terminal input, escape framing, local shortcut routing and ANSI rendering. The existing tmux adapter remains a separate compatibility path.

## Owned server and terminal engine

Each pane has a stable numeric ID, child process identity, PTY master and Alacritty terminal engine. The server drains PTY output into the engine, queues responses to terminal queries back to the application, and composes snapshots into client frames. ANSI styles and application colors, Unicode cell widths, cursor state, primary/alternate screens, history, selection and input modes belong to the engine. The engine handles fragmented terminal output; the compositor sanitizes labels and preserves wide-cell boundaries when clipping views.

PTY lifetime belongs to the server. Detaching or losing a frontend leaves its shells and agents running, and reconnecting sees the same pane IDs and process IDs. Explicit shutdown terminates owned terminals. The metadata journal does not reconstruct PTYs after shutdown, a crash or reboot.

The server polls nonblocking sockets and PTYs, bounds messages and per-client queues, and disconnects malformed or slow clients. A separate metadata worker avoids holding the terminal loop behind process scanning or a journal lock. Its bounded request/result channels let the terminal loop skip work rather than wait for discovery. Rendering uses periodic ticks to animate working markers and sends composed frames when state changes.

## Private protocol and control

A versioned length-prefixed JSON protocol runs over a same-user Unix socket. Dedicated directories and socket files are private; instance locks, owner checks, peer credentials and symlink rejection guard the endpoint. Frame messages include pane rectangles, terminal input modes, reading state, active pane, controller status and help state. Raw input/paste payloads are bounded to 64 KiB and protocol frames have a separate size bound.

Exactly one attached client controls interactive input and backend dimensions. Other clients observe and cannot mutate state through the interactive action/input channel. Takeover is explicit, including after the controller disconnects, and resizes the workspace to the new controller. Observer rendering clips or pads the same backend frame; observer resize messages do not resize PTYs. A separate same-user `ctl` command channel permits explicit administration without acquiring the interactive controller role.

The frontend enters raw mode, draws changed ANSI cells and restores its terminal state on normal exit and handled termination. Its incremental decoder recognizes SGR mouse reports and bracketed paste across reads. Escape timeout allows standalone Escape while retaining ordinary Alt sequences. Paste payloads bypass prefix routing entirely; oversized paste is truncated without releasing remaining payload as application keys or actions. The router owns Ctrl-B sequences and preserves application editing bytes outside its named bindings.

## Workspace geometry and input

The pure workspace layer computes balanced geometry from unparked terminal count and controller dimensions. Rows divide the full width evenly, including partial rows, and rebalance as terminals or dimensions change. One row per pane is reserved for its header and the last terminal row for a shared footer; vertical separators use solid quarter-cell strokes. Pane identity and palette stay with the process while swaps change logical order and displayed numbers.

Grid and tabs are alternate representations of the same terminal identities. Tab view draws only the selected terminal and adaptive top, left or bottom navigation; hidden unparked panes keep their base-grid PTY dimensions. Parked panes retain their last dimensions while output and metadata continue, and remain available to pane stream clients. Reading width defaults to 120 columns, with full width available for tables. Changing representation preserves processes. Character geometry selects automatic navigation placement, with an explicit override through the configuration dialog. Pane-addressed decoded screen inspection is independent of composed UI frames; see [native client boundary](native-client-protocol.md).

Header controls, tabs, sidebar targets and footer actions share hit geometry with the input router. When application mouse reporting is enabled, the server translates frontend coordinates into terminal coordinates and honors click/drag/motion modes. Otherwise the router emits body-local selection actions and copies on release; Shift selects or scrolls locally even in mouse-aware applications. Help, configuration and confirmations use centered dialogs that consume application input and close through their controls or outside clicks. A close request initially selects Close; Enter confirms and Escape cancels. The server accepts confirmation only for that dialog's pane before ending its owned process tree. Explicitly closing the last tab creates a replacement shell first.

## Automatic grids and process safety

New Tab prepares a PTY before committing the new terminal. Changing the grid view preserves the inventory of processes. Manual native layout and boundary-resize requests are rejected. Park/Restore modifies visibility and preserves IDs, PTYs, output and status; observers require control before changing it. Parking is not process suspension. Exact startup tab counts replace saved geometry, with legacy grid settings accepted as migration input. Shell exit removes its terminal and rebalances; explicit close requires confirmation. Last-shell exit detaches clients normally when there is no scratch terminal to preserve.

Screen resizes preserve every process, including foreground, background and stopped jobs. The separate legacy tmux helper retains its original occupancy guard; it does not control native workspaces.

## Discovery and journal

Native discovery reuses the process and transcript evidence adapters for Claude/Codex. The outermost recognizable interactive agent under a pane shell supplies metadata; headless tasks and shared app/MCP servers are excluded. PID plus process birth ticks guards against reuse. Session ID, title and lifecycle are evidence-derived, and missing or unbound evidence stays unknown. Some Codex shared-daemon TUIs therefore have unknown pane-bound session identity.

Native metadata discovery operates without requiring hooks. The optional `native-event` receiver accepts bounded JSON on standard input and resolves the pane from an explicit ID or the server-provided `KELPMUX_PANE` environment variable. It validates the registered agent kind, live process birth identity and session conflicts before updating metadata. Events arriving before discovery registers the agent fail promptly. Nonblocking journal access prevents a hook from waiting indefinitely for a held lock; the worker picks up accepted metadata on its next scan, normally within two seconds. Shared-daemon session ownership still requires verifiable pane-bound evidence.

The worker and native hook receiver use the registry and JSONL lifecycle journal under `STATE_DIR/native`, with private files, atomic registry replacement, locking and seven-day retention. `native-event --state-dir` takes the server state root; the existing journal/name commands take the nested `STATE_DIR/native` registry path. Current live agents remain registered beyond seven days. No prompt, response, tool argument, credential or transcript is stored in the journal. Hook setup is explicit and existing legacy tmux hook command examples require replacement before native use.

## Legacy compatibility and distribution

Legacy commands use an explicitly selected unmodified tmux 3.8+ executable and socket. tmux owns those PTYs, terminal rendering, layout, mouse, key tables and client size policy. Kelpmux supplies metadata, guarded layout helpers, floating reading geometry, optional lifecycle hooks and generated example bindings. Native and legacy state should be isolated; neither path automatically replaces configuration, starts a daemon or migrates an existing workspace.

CI tests real native PTYs, emulation and input on Linux x86-64 and ARM64, plus Rust 1.85 and dependency advisories/licenses. Separate compatibility jobs build checksum-verified upstream tmux 3.8 and exercise private tmux servers. Release archives package Kelpmux, project licensing and documentation with SHA-256 checksums. tmux is an optional separately installed legacy dependency and is not bundled in native archives. Starting a named workspace launches its private persistent server explicitly; nothing installs a system service, launches a user agent or performs automatic recovery.

## Beta workspace lifecycle and UI configuration

A named workspace selects its private state directory and socket. `start` creates an explicit persistent instance, `attach` connects without replacing the server, `status` inspects it, and `stop` requests shutdown of that instance. Existing foreground `server` and explicit socket commands remain independent. Startup validates names and live endpoints, avoids duplicate instances, and reports failures rather than silently replacing another workspace. `ssh-entry` is a login entry point that starts or attaches the selected workspace; SSH authentication and terminal transport remain the client's responsibility.

UI configuration is bounded JSON, validated before the server starts. It defines a prefix, reading dimensions and explicit key/action bindings. Application bytes outside those bindings pass through; terminal escape/paste framing remains separate from shortcut interpretation. UI selectors share geometry for grouped settings, parked-tab restore targets, agent attention/finished filters and pane movement. Display overlays preserve pane identity and workspace membership.

CI cache keys separate compiler versions, architectures and test jobs. PR runs never save cache artifacts, release compilation has read-only repository permissions, and release uploading occurs in a distinct minimal job after both architecture builds succeed. Fresh advisory checks run even when the pinned audit executable or compiled targets are restored.

## Native phase-two boundaries

Activity history persists bounded lifecycle metadata separately from terminal history. Discovery produces deduplicated transitions; initial state does not count as a fresh completion. Acknowledgements are persisted, and sound is opt-in in the frontend. Recovery collection uses a bounded background job with cancellation and nonblocking journal access, keeping the terminal I/O loop responsive.

Scratch is a separate persistent PTY whose visibility overlays the base view. It is excluded from the automatic grid and parking. Closing the last main terminal creates a replacement main shell; a main-shell exit also retains a main shell when scratch exists, preserving its process. Templates prepare new PTYs with validated names and directories; they never reconfigure an existing live instance.

Terminal history, search and selection remain in memory. Alternate-screen entry retains a bounded primary-history snapshot for search; revealing that history waits until the application returns to its primary screen. No search text or terminal contents enter the recovery journal. View modes and widths are remembered by column profile. Parking survives frontend reconnects while the owning daemon remains alive.

Composed frame deltas carry checked base/revision numbers and validated changed-cell indices. Full snapshots reset the baseline; backpressured queue replacement uses a snapshot rather than a patch whose base could be missing. Content-free diagnostic counters and a synthetic payload benchmark expose these mechanics without inspecting agent text. Independent native clients will require per-pane subscriptions beyond the current composed-frame protocol.
