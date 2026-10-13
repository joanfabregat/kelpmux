# Native terminal workspace

The native alpha runs a foreground Linux server that owns interactive shell PTYs and terminal state. Attached clients render its frames and route input. Detaching a client preserves the same shells, pane IDs, scrollback and processes while that server remains alive. Stopping the server ends its owned terminals; restarting does not recover them from the journal.

## Commands

```sh
kelpmux server [--socket PATH] [--state-dir PATH] [--shell PATH] [--cwd PATH] [--size COLSxROWS]
kelpmux attach [--socket PATH] [--takeover]
kelpmux ctl [--socket PATH] list
kelpmux ctl [--socket PATH] layout 1x1
kelpmux ctl [--socket PATH] layout 2x1
kelpmux ctl [--socket PATH] layout 2x2
kelpmux ctl [--socket PATH] focus ID
kelpmux ctl [--socket PATH] read ID
kelpmux ctl [--socket PATH] fullscreen ID
kelpmux ctl [--socket PATH] close
kelpmux ctl [--socket PATH] shutdown
```

`server` creates one shell in a `1x1` grid and stays in the foreground. Select a dedicated socket and state directory for each instance. Clients and `ctl` connect to an existing server and never start one implicitly. No command installs configuration or a background service. Use `ctl list` to obtain stable numeric pane IDs; IDs differ from the current one-based pane numbers used by shortcuts.

`focus` selects the pane. `read` toggles capped reading mode, and `fullscreen` toggles full width versus capped reading. `close` returns the reading pane to its tile without terminating it. `layout` applies one of the three supported column × row shapes. `shutdown` terminates the server and its owned shells/jobs, so detach when you want them to continue.

The default control socket is `$XDG_STATE_HOME/kelpmux/native/control.sock`, falling back to `~/.local/state/kelpmux/native/control.sock`. Custom paths must live in dedicated private directories owned by the current user. The socket accepts same-user clients; path ownership, socket permissions, symlink checks and instance locks prevent attaching through a foreign endpoint or starting a second server on the same state. Metadata registry and journal files currently live under `STATE_DIR/native`, including when an explicit state directory already has a `native` suffix.

## Clients and terminal sizes

The first eligible attached client controls the workspace. Additional clients observe it and cannot send application input or change it until they explicitly take control with `--takeover` or the takeover shortcut. An observer does not silently inherit control when the controller detaches. `ctl` is an explicit same-user administrative connection and can inspect/change the workspace without becoming its interactive controller.

Controller dimensions determine the backend PTY sizes. Taking control uses the new client's dimensions. Observers clip or pad the server's frame to their own terminal; their resizes leave application geometry intact. Supported sizes are 40–400 columns and 10–160 rows. Resizing to a smaller supported terminal keeps every pane and the current grid shape.

The last row is the footer. Each tile has a header, with a neutral one-column separator between grid columns. Capped reading is centered, uses at most 120 content columns, and occupies about 95% of the available height. Small terminals use all their available width. Fullscreen uses the entire work area above the footer and retains its header. The selected source remains a black tile in the background grid; other panes keep their PTY sizes. Returning or closing the view restores the same pane to its original slot.

## Keyboard and mouse

The prefix is `Ctrl-B`; release it, then press the next key. An ordinary Escape is passed to the application outside help and scrolling mode. Application Alt-f, Alt-b, modified arrows, Shift-Enter and Ctrl-j retain their original input bytes. Option/Alt must send Meta escape sequences for Alt shortcuts to work.

| Shortcut | Action |
| --- | --- |
| Prefix then `z`, Alt+Tab or Alt+0 | Toggle capped reading view for the active pane |
| Prefix then `1`…`9`, or Alt+1…9 | Read that numbered pane; selecting the same reading pane returns to grid |
| Prefix then `f` | Toggle fullscreen / capped reading |
| Prefix then `g` | Cycle `1x1`, `2x1`, `2x2`, subject to occupancy checks |
| Prefix then `s`, source digit, destination digit | Swap pane positions while retaining their IDs, colors and focus |
| Prefix then `d` | Detach this client |
| Prefix then `t` or `T` | Explicitly take control |
| Prefix then `?`, or footer `?` | Toggle shortcut help |
| Prefix then `[` | Enter history scrolling mode |
| Arrows / Page Up / Page Down in scrolling mode | Scroll history |
| `y` in scrolling mode, or prefix then `y` | Copy the selection |
| Escape or `q` in scrolling mode | Leave scrolling mode |
| Prefix then Ctrl-B | Send a literal Ctrl-B to the application |

Click a pane header to focus it. The rightmost header controls open reading and fullscreen; a capped view adds return and close controls, and fullscreen has restore-cap and close controls. Reading-view controls preserve the pane process. Numbered selection follows the current logical order after a swap.

The mouse wheel scrolls history when the application has not requested mouse reporting. Application mouse reporting otherwise receives body-relative mouse input; hold Shift to use local history scrolling or text selection instead. Dragging with the left button in local selection mode selects text and releasing copies it through the frontend's clipboard mechanism. Bracketed paste remains literal, including embedded shortcut bytes. Paste payloads are bounded to 64 KiB; excess bytes are discarded until the paste end marker, so they cannot become commands.

Help is sticky: close it with Escape, `q`, `?`, its × button, or an outside click. Help consumes application input while visible. Border dragging, a graphical pane-map chooser, custom grid shapes and a full tmux key-table/configuration system remain deferred.

## Occupancy and lifecycle limits

Growing creates interactive shells. Shrinking plans the complete removal set and removes only recognized idle interactive shells. Foreground applications, background jobs and stopped jobs make a pane occupied and cause a shrink that needs their slots to refuse. Stable process identity is checked before removal. If insufficient shells are idle, the original panes and layout remain.

Linux process and foreground-group checks cannot prove that every blocked shell built-in is an idle prompt. Fully detached processes outside the pane's tracked tree are also outside this safeguard. Avoid launching commands into proposed removal panes during a shrink. No terminal-size reduction removes a pane.

Claude/Codex discovery reuses the existing process and transcript evidence adapters. Agent status is independent of the pane's palette: working uses a braille spinner, while static markers identify input needed, completed, failed, idle, exited or unknown. Session identities that cannot be linked to a pane remain unknown; shared-daemon Codex TUIs are a known example. Optional native lifecycle hooks can supply stronger evidence after discovery has registered the agent, as described below.

The metadata journal retains seven days of lifecycle changes and live registry entries. It stores no terminal transcript, prompt, response, credentials or recovery image. A detached client can reconnect to the running server; a server crash or machine reboot requires a new workspace and the agent's own resume mechanism. Keep alpha trials isolated from existing tmux workspaces and production agents until you have validated your terminal and shell behavior.

## Optional native lifecycle hooks

```sh
kelpmux native-event codex|claude [--pane ID] [--state-dir SERVER_STATE]
```

This command reads one JSON hook payload from standard input, bounded to 64 KiB. Choose `codex` or `claude` for the emitting agent. The pane defaults to `KELPMUX_PANE`, which the server sets to the pane's `%ID` in its child shell environment. `--pane ID` explicitly selects that pane when needed. The state root defaults to `$XDG_STATE_HOME/kelpmux/native`, or `~/.local/state/kelpmux/native`; for a custom server state root, pass the same `--state-dir` value used by `server`, without adding the journal's extra `native` directory.

Hooks are optional and are never installed automatically. Wait for the first discovery scan to register the live agent before relying on them. The receiver checks the actual agent PID and process birth identity, registered agent kind and session conflicts; an unknown agent, mismatched identity or held journal lock makes it fail quickly. Accepted events are reflected by the metadata worker on its next scan, normally within two seconds. These checks do not grant a shared Codex daemon ownership of an otherwise unidentifiable pane-bound session.

The [Codex](../examples/codex-hooks.toml) and [Claude](../examples/claude-hooks.json) examples currently invoke the legacy tmux hook receiver. Keep their event configuration as appropriate for your agent, but replace the command for native use. For example, an explicitly configured Codex command running inside a native pane can be:

```sh
timeout 3s /absolute/path/to/kelpmux native-event codex \
  --state-dir "$HOME/.local/state/kelpmux-native-alpha" || true
```

Use `claude` for a Claude hook command. The agent supplies JSON on standard input; do not replace it with an invented session ID. The absolute binary path and explicit server state root keep an isolated trial separate from legacy state. See [discovery evidence and payload details](discovery.md).

The existing journal and name commands work directly against the native registry without invoking tmux. Here `SERVER_STATE` is the server's state root, while `%ID` is the stable pane ID with a percent prefix:

```sh
kelpmux --state-dir "$SERVER_STATE/native" journal
kelpmux --state-dir "$SERVER_STATE/native" name --pane %ID 'Auth migration'
```

This extra `native` suffix belongs to the metadata registry path for `journal` and `name`; it is not part of `native-event --state-dir` or `server --state-dir`.
