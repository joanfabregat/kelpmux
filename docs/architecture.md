# Architecture

Native Kelpmux is a Linux terminal multiplexer with a persistent server and separate attached clients. The server, [KelpServer](https://github.com/joanfabregat/kelpserver), lives in its own repository and owns PTYs, interactive shells, terminal emulation, workspace geometry, tab parking and focus, agent metadata, activity history and client authorization. Its design is described in [KelpServer's architecture](https://github.com/joanfabregat/kelpserver/blob/main/docs/architecture.md). This repository contains the frontend (TUI), the web client and the optional tmux adapter. The frontend owns raw terminal input, escape framing, local shortcut routing and ANSI rendering. KelpMux builds against the KelpServer revision pinned in `Cargo.toml`.

## Server boundary

The TUI attaches to KelpServer over its versioned, length-prefixed JSON protocol on a same-user Unix socket (composed protocol v58). The server composes frames, including pane rectangles, terminal input modes, reading state, active pane, controller status and help state; the frontend draws them. Exactly one attached client controls interactive input and backend dimensions; other clients observe. Takeover is explicit, and a separate same-user `ctl` command channel permits explicit administration without acquiring the interactive controller role. PTY lifetime belongs to the server: detaching or losing a frontend leaves its shells and agents running. The independent pane stream (v1) is documented in the [KelpServer stream contract](https://github.com/joanfabregat/kelpserver/blob/main/crates/server/docs/native-client-protocol.md).

Grid geometry, tab order, parking, close handling, scratch terminals, discovery, the metadata journal, activity history and recovery are server behavior; see the KelpServer documentation for their rules.

## Frontend input and rendering

The frontend enters raw mode, draws changed ANSI cells and restores its terminal state on normal exit and handled termination. Its incremental decoder recognizes SGR mouse reports and bracketed paste across reads. Escape timeout allows standalone Escape while retaining ordinary Alt sequences. Paste payloads bypass prefix routing entirely; oversized paste is truncated without releasing remaining payload as application keys or actions. The router owns Ctrl-B sequences and preserves application editing bytes outside its named bindings.

Header controls, tabs, sidebar targets and footer actions share hit geometry with the input router. When application mouse reporting is enabled, the server translates frontend coordinates into terminal coordinates and honors click/drag/motion modes. Otherwise the router emits body-local selection actions and copies on release; Shift selects or scrolls locally even in mouse-aware applications. Help, configuration and confirmations use centered dialogs that consume application input and close through their controls or outside clicks. A close request initially selects Close; Enter confirms and Escape cancels. The server accepts confirmation only for that dialog's pane before ending its owned process tree.

## Workspace lifecycle and UI configuration

A named workspace selects its private state directory and socket. `kelpmux start` creates an explicit persistent instance by launching the `kelpserver` executable (beside `kelpmux`, or `KELPSERVER_BINARY`, or `--server-binary`), `attach` connects without replacing the server, `status` inspects it, and `stop` requests shutdown of that instance. Startup validates names and live endpoints, avoids duplicate instances, and reports failures rather than silently replacing another workspace. `ssh-entry` is a login entry point that starts or attaches the selected workspace; SSH authentication and terminal transport remain the client's responsibility.

UI configuration is bounded JSON, validated before the server starts. It defines a prefix, reading dimensions and explicit key/action bindings. Application bytes outside those bindings pass through; terminal escape/paste framing remains separate from shortcut interpretation. UI selectors share geometry for grouped settings, parked-tab restore targets, agent attention/finished filters and pane movement.

## Legacy compatibility and distribution

Legacy commands use an explicitly selected unmodified tmux 3.8+ executable and socket. tmux owns those PTYs, terminal rendering, layout, mouse, key tables and client size policy. Kelpmux supplies metadata, guarded layout helpers, floating reading geometry, optional lifecycle hooks and generated example bindings. Native and legacy state should be isolated; neither path automatically replaces configuration, starts a daemon or migrates an existing workspace.

CI tests real native PTYs, emulation and input on Linux x86-64 and ARM64 against the pinned KelpServer, plus Rust 1.85 and dependency advisories/licenses. Separate compatibility jobs build checksum-verified upstream tmux 3.8 and exercise private tmux servers. Release archives package `kelpmux` and the pinned `kelpserver`, project licensing and documentation with SHA-256 checksums. tmux is an optional separately installed legacy dependency and is not bundled in native archives. Starting a named workspace launches its private persistent server explicitly; nothing installs a system service, launches a user agent or performs automatic recovery.

CI cache keys separate compiler versions, architectures and test jobs. PR runs never save cache artifacts, release compilation has read-only repository permissions, and release uploading occurs in a distinct minimal job after both architecture builds succeed. Fresh advisory checks run even when the pinned audit executable or compiled targets are restored.
