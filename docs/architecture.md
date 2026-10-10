# Architecture

tmux owns PTYs, terminal emulation, layouts, clients, and all application processes. Kelpmux owns agent metadata, guarded workspace operations, reading-view lifecycle, and optional integration output. The daemon is a foreground poller, not a terminal server and not an agent launcher.

## Discovery and journal

Each scan lists and deduplicates tmux panes (including linked windows), snapshots Linux processes, and walks descendants from each pane's shell. The outermost interactive Claude/Codex process is the pane's agent. Headless executions and shared app/MCP servers are excluded. PID plus Linux process birth ticks prevents PID-reuse confusion. Supported metadata adapters produce a session ID, name, and status; missing or unfamiliar fields become unknown.

Registry writes use a file lock, private atomic replacement, and filesystem sync. The append-only JSONL journal records discovered/changed/renamed/hook/exited transitions. It stores metadata, never hook payloads. Pruning retains seven days; the registry retains currently live processes and recent ended processes. A corrupt registry fails visibly instead of being silently overwritten.

The discovery schema belongs to external tools and can change. Fixtures isolate adapters from the user's actual home directory. Local Codex daemon queries only initialize a metadata client and read an already identified thread with turns excluded; they never start/resume a thread. No metadata query can infer ownership safely when several TUIs share a server and expose no identity.

## Reading view

A window-scoped lock permits one reading controller per origin window. A temporary session holds the selected pane; a sleeping placeholder preserves the origin's slot. Pane swaps keep the original PTY, PID, and scrollback. Modal state is persisted before the swap so interruption can be repaired.

The outer client displays a tmux popup containing another tmux client attached to the temporary session. On full/restore/select actions, the inner client detaches; the controller reopens the popup at the requested dimensions or swaps another agent in. The normal view uses at most the configured content width plus border columns. Small screens and fullscreen omit the border. The existing tmux client handles live terminal resizing; the next reopen recomputes the width cap.

Restore swaps the original pane back and deletes only the expected temporary session. Unexpected panes are retained for manual recovery. A crash after temporary session creation but before its state file is written can leave an unused session named `kelpmux-*`; no agent has been moved at that point.

## Grid changes

Custom tmux layout strings include tmux's checksum and equally distributed row/column sizes. Growing splits a wide pane and then applies the requested final grid. Shrinking computes the complete removal set, checks `/proc` for foreground/background/stopped jobs, revalidates pane and process identity, and uses a tmux-side foreground-shell check. Operations take a workspace lock and refuse native zoom or an open reading view.

These checks are conservative, but tmux does not offer transactional process deletion. Concurrent user input or native tmux mutations can race the check. Applications that detach entirely from their pane shell and controlling terminal are outside pane occupancy tracking. No shell process is frozen: tmux automatically resumes most stopped pane shells, which would make a SIGSTOP-based guard misleading.

## Boundaries

The first beta supports one tmux socket per state directory, Linux process inspection, and up to nine slots in the provided presets. It does not manage system services, modify user configuration, launch agents, restart failed agents, record terminal output, or provide nested scratch popups. The Rust modules deliberately keep discovery, persistence, tmux transport, and workspace behavior separate so adapters can evolve without replacing tmux.
