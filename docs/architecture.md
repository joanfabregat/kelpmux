# Architecture

Kelpmux is a small Rust helper beside unmodified tmux 3.8+. tmux owns PTYs, terminal emulation, agent rendering, focus, mouse input, native layouts, floating panes, zoom, clients and application processes. User-owned tmux configuration owns the interface. Kelpmux owns agent metadata, guarded grid operations, optional journal retention, reading geometry commands, and the short-lived grid-card dialog. The foreground discovery daemon is a metadata poller, not a terminal server or agent launcher.

## Discovery and journal

A scan deduplicates linked tmux panes, snapshots Linux processes, and walks descendants from each pane shell. The outermost interactive Claude/Codex process is the pane's agent; headless jobs and shared app/MCP servers are excluded. PID plus process birth ticks prevents PID reuse confusion. Metadata adapters produce session ID, name and lifecycle evidence; unfamiliar or missing evidence is unknown.

The daemon writes native pane options: `@kelpmux-name`, `@kelpmux-status`, `@kelpmux-color`, `@kelpmux-color-muted` and `@kelpmux-session`. tmux chooses the bright or muted title background on focus changes without waiting for a daemon scan; the full title fill is independent of neutral one-column borders. Optional hooks supply stronger identity/lifecycle evidence without launching agents through Kelpmux.

Registry replacement is private, atomic and file-locked. The JSONL journal records metadata changes and prunes after seven days. It stores no transcript or hook payload. Current live records remain even for longer-running agents. Corrupt registry data fails visibly. Discovery fixtures never use actual user credentials or transcripts.

## Native reading view

The grid chooser uses a temporary native modal pane with close-on-outside-click and Escape cancellation. Its Rust renderer draws only six preset cards and handles keyboard/mouse input on its own PTY; it never proxies agent output. A private window-option request joins the dialog to its invoking helper. The preset is applied only after that dialog pane disappears, so it cannot count as an occupied agent or be removed as a shrink candidate. Request options are cleared and occupancy is validated again on apply. Pane movement uses native `display-panes` previews and digit/click selection with stable source/destination IDs; no text command prompt is involved.

`break-pane -W` lifts the existing pane out of its tile while retaining its native window, pane ID, index, PID and scrollback. `resize-pane` sets the width cap and height, and `move-pane -P centre` centers it. Native zoom gives explicit fullscreen and automatically uses all columns when the window is smaller than the cap. A native `window-resized` hook calls a short-lived geometry helper to restore the cap when the screen grows; explicit fullscreen remains fullscreen.

The saved tile is owned by tmux. A process-free empty pane (`new-pane -E`) is created floating, then tiled into the original slot through the native JSON layout. Creating it floating first preserves every original pane index. The slot has a black background and no header; it has no running command and is excluded from helper pane inventories and discovery. Other background panes keep their original geometry. Returning deletes only the marked, still-process-free slot, then `join-pane -s %ID -t %ID` restores the source. Discovery cleans orphaned empty slots under the workspace lock and relinquishes any slot repurposed with a process.

To prevent uneven-split rounding drift, the helper stores the original native layout and pane/index inventory in pane options before lifting. It reapplies that layout only if the original inventory still matches and no unrelated floating panes remain. Otherwise it preserves external additions/removals through native reinsertion. Closing or interrupting the helper cannot kill an agent or strand it in a temporary session. Failed slot setup attempts to return the source immediately.

Reading commands are serialized by a workspace lock. No command remains running while reading. There is no nested client, popup attachment, placeholder process, modal JSON file, dedicated key table, or recovery command. A floating pane created outside Kelpmux is not taken over. State surviving a helper interruption is ordinary tmux state; `grid` or native unzoom/join can restore it.

Native reading belongs to a window. Clients showing that same window see the same layout and reading pane, subject to tmux's normal window-size policy. Changing screens never introduces another terminal emulator or environment. Shortcuts and application input use the ordinary tmux key tables.

## Guarded grids

Grid planning produces balanced native layout strings. Growing splits a pane and applies the preset; shrinking identifies the complete candidate set, checks foreground/background/stopped jobs, revalidates PID birth and pane identity, and asks tmux to check the shell immediately before removal. Highest numbered unused shells are removed first. Floating panes, managed reading state and zoom make a layout change refuse without removing panes.

There is no atomic operating-system/tmux transaction for occupancy and removal. Concurrent input or external native mutations can race the final check; interruption can leave a partial grid. Fully detached processes outside the pane's process tree are not tracked. Runnable shells are considered occupied. A blocked shell builtin can resemble an idle prompt without shell cooperation. No process is frozen to manufacture a transaction.

## Configuration and distribution

`--tmux-binary` selects an explicit upstream executable; generated bindings carry it with the socket and state directory. This allows a privately built tmux alongside apt's version. `tmux-config` prints an optional appearance/binding example; `--bindings-only` avoids changing user appearance. Neither output is sourced automatically. Dotfiles remain the source of truth for final shortcuts and visual choices.

Linux x86-64 and ARM64 CI build a SHA-256-pinned upstream release and run real private-server terminal tests. Release packages include the same unmodified tmux build with its own copyright/license. Rust dependencies retain their licenses. No fork, service unit, installation, configuration replacement, agent launch or automatic recovery is performed.
