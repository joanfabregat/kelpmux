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

The pure workspace layer supports balanced `1x1`, `2x1`, `2x2`, `3x2`, `2x3` and `3x3` grids. One row per pane is reserved for its header and the last terminal row for a shared footer; columns have one neutral separator. Pane identity and palette stay with the process while swaps change logical order and displayed numbers.

Grid and tabs are alternate representations of the same terminal identities. Tab view draws only the selected terminal and adaptive top, left or bottom navigation; hidden panes keep their base-grid PTY dimensions. Reading width defaults to 120 columns, with full width available for tables. Changing representation does not terminate processes or alter the saved grid. Character geometry selects automatic navigation placement, with an explicit override through the configuration dialog. Pane-addressed decoded screen inspection is independent of composed UI frames; see [native client boundary](native-client-protocol.md).

Header controls, tabs, sidebar targets and footer actions share hit geometry with the input router. When application mouse reporting is enabled, the server translates frontend coordinates into terminal coordinates and honors click/drag/motion modes. Otherwise the router emits body-local selection actions and copies on release; Shift selects or scrolls locally even in mouse-aware applications. Help, configuration and confirmations use centered dialogs that consume application input and close through their controls or outside clicks. A close request opens a default-cancel confirmation; the server accepts close confirmation only for that dialog's pane before ending its owned process tree. Closing the last tab creates a replacement shell first.

## Guarded grids and process safety

The server prepares PTYs before committing a growth operation, then applies the complete grid. The pure layer refuses a preset whose pane count was not prepared. Shrinking snapshots Linux processes and validates the complete candidate set, retaining foreground, background and stopped jobs. Removal requires a recognized idle interactive shell, matching process birth identity and foreground process group. Highest numbered unused candidates are preferred. Insufficient idle shells cause the resize to refuse without removing occupied panes or changing geometry.

Process checks have a race with new shell input and cannot identify every blocked built-in without shell cooperation. Fully detached processes outside the tracked tree are also outside the occupancy guard. The implementation does not freeze processes to manufacture a transaction. Screen resizes are independent of pane-count changes and preserve every pane even on small supported terminals.

## Discovery and journal

Native discovery reuses the process and transcript evidence adapters for Claude/Codex. The outermost recognizable interactive agent under a pane shell supplies metadata; headless tasks and shared app/MCP servers are excluded. PID plus process birth ticks guards against reuse. Session ID, title and lifecycle are evidence-derived, and missing or unbound evidence stays unknown. Some Codex shared-daemon TUIs therefore have unknown pane-bound session identity.

Native metadata discovery operates without requiring hooks. The optional `native-event` receiver accepts bounded JSON on standard input and resolves the pane from an explicit ID or the server-provided `KELPMUX_PANE` environment variable. It validates the registered agent kind, live process birth identity and session conflicts before updating metadata. Events arriving before discovery registers the agent fail promptly. Nonblocking journal access prevents a hook from waiting indefinitely for a held lock; the worker picks up accepted metadata on its next scan, normally within two seconds. Shared-daemon session ownership still requires verifiable pane-bound evidence.

The worker and native hook receiver use the registry and JSONL lifecycle journal under `STATE_DIR/native`, with private files, atomic registry replacement, locking and seven-day retention. `native-event --state-dir` takes the server state root; the existing journal/name commands take the nested `STATE_DIR/native` registry path. Current live agents remain registered beyond seven days. No prompt, response, tool argument, credential or transcript is stored in the journal. Hook setup is explicit and existing legacy tmux hook command examples require replacement before native use.

## Legacy compatibility and distribution

Legacy commands use an explicitly selected unmodified tmux 3.8+ executable and socket. tmux owns those PTYs, terminal rendering, layout, mouse, key tables and client size policy. Kelpmux supplies metadata, guarded layout helpers, floating reading geometry, optional lifecycle hooks and generated example bindings. Native and legacy state should be isolated; neither path automatically replaces configuration, starts a daemon or migrates an existing workspace.

CI tests real native PTYs, emulation and input on Linux x86-64 and ARM64, plus Rust 1.85 and dependency advisories/licenses. Separate compatibility jobs build checksum-verified upstream tmux 3.8 and exercise private tmux servers. Release archives package Kelpmux, project licensing and documentation with SHA-256 checksums. tmux is an optional separately installed legacy dependency and is not bundled in native archives. Starting a named workspace launches its private persistent server explicitly; nothing installs a system service, launches a user agent or performs automatic recovery.

## Beta workspace lifecycle and UI configuration

A named workspace selects its private state directory and socket. `start` creates an explicit persistent instance, `attach` connects without replacing the server, `status` inspects it, and `stop` requests shutdown of that instance. Existing foreground `server` and explicit socket commands remain independent. Startup validates names and live endpoints, avoids duplicate instances, and reports failures rather than silently replacing another workspace. `ssh-entry` is a login entry point that starts or attaches the selected workspace; SSH authentication and terminal transport remain the client's responsibility.

UI configuration is bounded JSON, validated before the server starts. It defines a prefix, reading dimensions and explicit key/action bindings. Application bytes outside those bindings pass through; terminal escape/paste framing remains separate from shortcut interpretation. UI selectors share a model for keyboard and mouse hit testing, full-card selection, disabled grid choices, agent attention/finished filters and pane movement. Display overlays do not change pane identity or move processes between workspaces.

CI cache keys separate compiler versions, architectures and test jobs. PR runs never save cache artifacts, release compilation has read-only repository permissions, and release uploading occurs in a distinct minimal job after both architecture builds succeed. Fresh advisory checks run even when the pinned audit executable or compiled targets are restored.
