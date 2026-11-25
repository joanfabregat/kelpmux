# Architecture

> Server-relevant sections of [KelpMux docs/architecture.md](https://github.com/joanfabregat/kelpmux/blob/eb31da5/docs/architecture.md) at `eb31da5`, copied when KelpServer moved to its own repository, with later server changes from KelpMux `cfe28de` (pane environment, startup-dialog activity). Frontend, TUI layout and legacy tmux material stays in KelpMux. "Kelpmux" below refers to the combined product at the time of writing; the behavior described is implemented by KelpServer. Since 0.3 ([ADR 0002](../crates/server/docs/adr/0002-generic-session-server.md) step 3) the server has no composed UI; the sections below were updated accordingly.

KelpServer is the persistent server of native Kelp workspaces. It owns PTYs, interactive shells, terminal emulation, per-pane leases and sizes, agent metadata and client authorization. Clients (the KelpMux TUI and web client, browser clients through `bridge`/`web-bridge`, and KelpHub) own presentation, layout, input framing and all UI state.

## Owned server and terminal engine

Each pane has a stable numeric ID, child process identity, PTY master and Alacritty terminal engine. The server drains PTY output into the engine, queues responses to terminal queries back to the application, and sends pane snapshots and patches to stream subscribers. ANSI styles and application colors, Unicode cell widths, cursor state, primary/alternate screens, history and input modes belong to the engine. The engine handles fragmented terminal output. Selection is computed by clients on their own snapshots.

PTY lifetime belongs to the server. Detaching or losing a frontend leaves its shells and agents running, and reconnecting sees the same pane IDs and process IDs. Explicit shutdown terminates owned terminals. The metadata journal does not reconstruct PTYs after shutdown, a crash or reboot.

The server polls nonblocking sockets and PTYs, bounds messages and per-client queues, and disconnects malformed or slow clients. A separate metadata worker avoids holding the terminal loop behind process scanning or a journal lock. Its bounded request/result channels let the terminal loop skip work rather than wait for discovery. Updates are sent when state changes.

## Socket protocol and control

The versioned pane stream (version 1) runs as length-prefixed JSON over a same-user Unix socket, or through `kelpserver bridge` over SSH. Dedicated directories and socket files are private; instance locks, owner checks, peer credentials and symlink rejection guard the endpoint. Raw input/paste payloads are bounded to 64 KiB and records have a separate size bound. A record the server cannot decode, including any message of the composed protocol 58 that 0.3 removed, gets an `Error` reply and the connection closes.

Each pane has at most one lease owner, which alone may type into it, resize it or set its theme fallbacks. Other connections observe it. Takeover is explicit. Release or disconnect returns the pane to the server's base size (`--size`). Administration (`shutdown`, `diagnostics`) uses the same stream connection; there is no separate controller role.

See [the stream contract](../crates/server/docs/native-client-protocol.md).

## Pane sizes and process safety

The server keeps no layout. Each pane's PTY has the size of its lease, or the base size without one; clients arrange panes, tabs, parking and focus themselves, keyed by pane identity. A server starts with zero panes and clients create them with `spawn`. `close` terminates a pane's process group; closing the last pane first creates a replacement shell, and the last pane's own exit stops the server.

Screen resizes preserve every process, including foreground, background and stopped jobs. The separate legacy tmux helper retains its original occupancy guard; it does not control native workspaces.

Panes are new top-level terminals, so their shells do not inherit the session markers of whoever started the server. KelpServer removes `CLAUDECODE`, every `CLAUDE_CODE_*` variable (for example `CLAUDE_CODE_CHILD_SESSION`, which makes Claude Code treat a new session as a child and turn off transcript saving, and `CLAUDE_CODE_MESSAGING_SOCKET`), `CLAUDE_PID`, `CLAUDE_EFFORT`, `CODEX_SESSION_ID`, `CODEX_THREAD_ID`, `CODEX_TOOL_CALL_ID`, `CODEX_CI`, `TMUX`, `TMUX_PANE`, `STY`, `SSH_CONNECTION`, `SSH_CLIENT`, `SSH_TTY` and `NO_COLOR`. User configuration is kept, including API keys, `CLAUDE_CONFIG_DIR` and `CODEX_HOME`, so a server started from inside an agent or tmux session still gives its panes a clean, configured environment. `start` also launches the background server process itself without the agent and multiplexer markers.

## Discovery and journal

Native discovery reuses the process and transcript evidence adapters for Claude/Codex. The outermost recognizable interactive agent under a pane shell supplies metadata; headless tasks and shared app/MCP servers are excluded. PID plus process birth ticks guards against reuse. Session ID, title and lifecycle are evidence-derived, and missing or unbound evidence stays unknown. Some Codex shared-daemon TUIs therefore have unknown pane-bound session identity.

Native metadata discovery operates without requiring hooks. The optional `native-event` receiver accepts bounded JSON on standard input and resolves the pane from an explicit ID or the server-provided `KELPMUX_PANE` environment variable. It validates the registered agent kind, live process birth identity and session conflicts before updating metadata. Events arriving before discovery registers the agent fail promptly. Nonblocking journal access prevents a hook from waiting indefinitely for a held lock; the worker picks up accepted metadata on its next scan, normally within two seconds. Shared-daemon session ownership still requires verifiable pane-bound evidence.

The worker and native hook receiver use the registry and JSONL lifecycle journal under `STATE_DIR/native`, with private files, atomic registry replacement, locking and seven-day retention. `native-event --state-dir` takes the server state root; the existing journal/name commands take the nested `STATE_DIR/native` registry path. Current live agents remain registered beyond seven days. No prompt, response, tool argument, credential or transcript is stored in the journal. Hook setup is explicit and existing legacy tmux hook command examples require replacement before native use.

## Workspace lifecycle

A named workspace selects its private state directory and socket. `start` creates an explicit persistent instance, `attach` connects without replacing the server, `status` inspects it, and `stop` requests shutdown of that instance; all of them use the stream. Existing foreground `server` and explicit socket commands remain independent. Startup validates names and live endpoints, avoids duplicate instances, and reports failures rather than silently replacing another workspace. `ssh-entry` is a login entry point that starts or attaches the selected workspace; SSH authentication and terminal transport remain the client's responsibility. `attach` and `ssh-entry` are KelpMux commands; `kelpserver` provides `start`, `status`, `stop`, the foreground `server` and the `bridge`.

## Phase-two boundaries

Activity history persists bounded lifecycle metadata separately from terminal history. Discovery produces deduplicated transitions; initial state does not count as a fresh completion, and an agent first seen waiting for input before it has a session (a startup dialog such as Claude Code's folder trust prompt) does not alert until it asks again later. Acknowledgements are persisted; sound and other alerts are client UI. Recovery collection uses a bounded background job with cancellation and nonblocking journal access, keeping the terminal I/O loop responsive.

Terminal history and search remain in memory. Alternate-screen entry retains a bounded primary-history snapshot for search; revealing that history waits until the application returns to its primary screen. No search text or terminal contents enter the recovery journal.

Pane patches carry checked base/revision numbers and validated changed-cell indices. Full snapshots reset the baseline; backpressured queue replacement uses a snapshot rather than a patch whose base could be missing. Content-free diagnostic counters (`diagnostics`) expose server health without inspecting agent text.
