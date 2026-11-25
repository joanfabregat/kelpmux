# Session API inventory

This is step 1 of [ADR 0002](adr/0002-generic-session-server.md): the inventory of what today's KelpMux TUI obtains from the composed protocol (local protocol 58), sorted by the boundary rule, and what the pane stream now carries so a TUI can run as a stream client with local rendering and local UI state. The wire contract is in [native-client-protocol.md](native-client-protocol.md#session-api); this page records the classification and the gaps. The [step 2 server support](#step-2-server-support-2026-11-21) closes the gaps the stream TUI found; rows marked "Step 2" record it. [Step 3](#step-3-2026-11-21) removed the composed protocol from the server (KelpServer 0.3); the "Before" column now describes history.

"Session" means the server keeps it. "Client UI" means each client owns it, keyed by pane identity (server epoch plus pane ID), and the server stores none of it. "Before" is the stream protocol at `0837dd2`; "Step 1" is what this change adds. All additions are additive and capability-gated; the stream version stays 1.

| Capability | Session or client UI | On the stream before step 1 | Step 1 addition |
| --- | --- | --- | --- |
| Server epoch and pane identity | Session | Yes: Welcome `epoch`; pane IDs scoped to it | Documented that pane IDs increase monotonically and are never reused within an epoch, so they also give creation order. |
| Server version | Session (lifecycle) | No | Welcome `server_version` (`server-lifecycle`). |
| Pane list with process, cwd, agent kind, session ID, status, evidence, activity times and shell command | Session (observations) | Yes: `Update.panes` on every update, including a metadata-only subscription (`subscribe []`) | None. Since 0.3.0-beta.2 `PaneInfo` has no composed-UI fields (`parked`, `number`, `palette` and title aliases were removed); tab numbers, parking and colours are client state keyed by pane ID. |
| Pane session facts: creation time, launch directory, shell, spawn arguments, startup command, raw terminal title | Session | No | `Update.sessions` (`PaneSession`) after `session_events` opt-in (`pane-lifecycle`). |
| Pane created | Session | No (a new ID appeared in `Update.panes`) | `pane_created {session}` event. |
| Pane exited (exit code or signal) | Session | No (`dead`/`Exited` status, briefly) | `pane_exited {pane, at, code, signal}` event. |
| Pane closed (by exit or by a client), with its closed-session record | Session | No (the ID disappeared) | `pane_closed {pane, at, reason, record}` event. Events invalidate a pending update so the next update follows them. |
| Create a pane (cwd, argv; sanitized environment) | Session | Only `SpawnPane` on a separate same-UID management connection, not through the SSH bridge (removed in step 3) | `spawn {request, cwd?, argv}` → `spawned {request, pane}`, with the validation and retry cache `SpawnPane` had. The environment is sanitized by the server (session markers removed); clients cannot pass variables. Step 2: optional `shell` (`spawn-shell`), part of the retry identity; `PaneSession.spawned_by` names the creating connection (`spawned-by`). |
| Close (terminate) a pane | Session | No | `close {request, pane}` → `done {request}`; refused while another connection holds the lease. The confirmation dialog is client UI. |
| Closed-session records: list | Session | No (composed Recovery dialog only) | `closed_sessions {request}` → `closed_sessions {request, records}`, collected by the existing background job. Records carry `key`, workspace, cwd, name, kind, session ID, argv, close time, reason, `sessions` (every agent conversation the pane hosted, oldest first, each with kind, ID, first-seen folder and first/last seen times), and the derived `resume` and `rerun` commands. |
| Closed-session records: restore or rerun | Session | No | `restore {request, record, rerun}` → `restored {request, record, pane, existing}`. Records are addressed by `key`, never by a list index. |
| Restore-choice labels, copy resume command | Client UI | n/a | Derived by the client from `resume`/`rerun`. |
| Notifications feed (latest 128) | Session | Yes: `Update.notifications` | None. Step 2: Welcome `notification_run` (`notification-run`), the run that notification origins are matched against. |
| Acknowledge one notification | Session | Yes: `acknowledge` | None. |
| Acknowledge all | Session | No | `acknowledge_all` (`notification-ack-all`). |
| Notification sound, "next attention/finished" navigation, unread badges | Client UI | n/a | Computed from notifications and pane status. |
| History | Session | No on the stream (`History` on a separate connection only) | `history {request, pane, start, limit}` → `history {request, pane, lines}` (plain text, at most 200 rows). |
| Search one pane or across panes | Session | No on the stream (`SearchPane` on a separate connection only) | `search {request, panes, query, limit}` → `search {request, results, truncated}`; an empty pane list searches every pane in creation order; 64 matches and 4 MiB of context per reply. |
| Scrollback viewing and revealing a search match | Session today (emulator viewport) | No | `scroll {pane, lines}` and `reveal {pane, line}`, both lease-bound (`pane-scroll`). See "Remaining gaps" for styled history. |
| Selection and copy | Client UI | Snapshots and OSC 52 `clipboard-write` | None: selection is computed on the client's own snapshot and history text. |
| Terminal screen, patches, input, paste, mouse, resize, per-pane leases and sizes, controller theme | Session | Yes | None. |
| Host vitals (CPU, memory, swap, disk %, clock) | Session (host observation) | Yes: `Update.vitals` | None. |
| Host status (per-CPU, memory and swap bytes, load, uptime, tasks, network and disk I/O, mounts, process table) | Session (host observation) | No (composed Status dialog only) | `host_status {enabled}` opt-in adds `Update.host` (`host-status`). Sorting, views and layout are client UI. |
| Shutdown request | Session (lifecycle) | No (separate management `Command(Shutdown)`) | `shutdown {request}` → `done`, then `server_stopping` (`server-lifecycle`). |
| Shutdown and last-pane-exit signals | Session (lifecycle) | No (the socket closed) | `server_stopping {reason}` after `session_events` opt-in; queued events get a bounded 200 ms flush before the sockets close. |
| Upgrade | Session (lifecycle) | No | No server event: a live server is never upgraded in place. Clients compare Welcome `server_version` and stream capabilities, and a new server is a new epoch. |
| Tabs, tab order, names, colours, focus and focus history, reading/grid modes, grid geometry, sidebar, dialogs, help, title animation, spinner, templates, TUI settings | Client UI | n/a | None. Tab order defaults to pane ID (creation) order; names start from `PaneInfo.title` / `PaneSession.terminal_title`. |
| Parking | Client UI | n/a | None. The server needs no "parked" concept: see below. |
| Scratch terminal | Client UI (which pane is "scratch") | n/a | None. A client creates it with `spawn` and marks it locally; for the server it is an ordinary pane. |
| Agent conversation link (Codex picker) | Session (discovery input) | No | Dropped: not planned (step 2 decision). |
| Unused-shell hint | Session (observation) | No (composed UI only) | Step 2: `PaneSession.unused_shell` (`unused-shell`). |
| Startup panes (composed startup tabs, `tab-layout.json`) | Session (startup policy); the layout itself is client UI | No | Step 2: `--startup-panes layout\|none` and Welcome `startup_panes` (`startup-policy`). Step 3: always zero panes; `layout` removed. |
| Recovery popup after an unclean shutdown | Session (lifecycle signal); the popup is client UI | No (composed UI only) | Step 2: Welcome `recovery_pending` and `dismiss_recovery` (`recovery-signal`). |
| Subscription bounds | Session (resource limits) | Hardcoded: 10 panes, 100,000 cells | Step 2: 64 panes, Welcome `limits` (`stream-limits`). |

## Parking and display leases

Parking needs no server state. The composed server never resized a parked pane: it simply left it out of the grid geometry. A stream client gets the same result from two existing generic operations:

- To stop receiving a pane's screen, remove it from `subscribe`. Metadata, notifications and lifecycle events keep flowing.
- To keep a hidden pane's size stable, keep its lease. A lease does not require a subscription, so an application in a hidden pane sees no `SIGWINCH`.
- To give up the size, `release` it; the pane returns to the server's base geometry.

A dedicated "this client doesn't display this pane" request would duplicate `subscribe` plus `release`, so none was added. The test `a_leased_but_unsubscribed_pane_keeps_its_size` covers this.

## Decisions

1. **Versioning.** The stream stays at version 1. Every addition is a new optional field or a new request/event type behind a new Welcome capability. Events of new types are sent only to a connection that opted in (`session_events`, `host_status`) or sent the correlated request, because current KelpClient and KelpHub decoders reject unknown event types. An older server disconnects on an unknown request type, so clients check capabilities first.
2. **Correlation.** New request/reply operations carry a client-chosen `request` string (1–128 of `A–Z a–z 0–9 - _`), echoed in the reply and in `error.request`. Fire-and-forget operations (`scroll`, `reveal`, `acknowledge_all`, opt-ins) have none, like the v1 operations.
3. **Lifecycle ordering.** Lifecycle events are reliable messages; a pending replaceable update built before an event is discarded together with that connection's screen baselines, so the next update is a fresh full update that already reflects the event.
4. **Closing the last pane** keeps the composed protocol's policy (confirmed in step 3): `close` on the last main pane first spawns a replacement shell (reported by `pane_created`), and the last pane's exit stops the server (`server_stopping {reason: last_pane_exited}`). A server starts with zero panes (decision 8), but `close` never leaves zero panes: a client that wants an empty server closes its panes and lets the last one exit, or stops the server.
5. **Closed-session records stay in the server** and stay generic: they are derived from server observations (foreground programs, agent sessions), shared by every client, and restored by opaque `key`. Only their presentation is client UI.
6. **Scroll stays a lease-bound server operation in step 1** because local rendering of scrollback needs styled history, which does not exist yet.
7. **Host status data stays in the server** (`status.rs` sampling); its presentation (views, sorting, columns, formatting) is client UI.
8. **Zero panes (step 2; the only startup since step 3).** A server runs with zero panes until a client spawns; nothing in the server needs a pane, and only the last pane's exit stops it. `close` on the last pane still creates a replacement shell first, so a running server never drops back to zero panes through `close`.
9. **Unused shell is an observation, recovery is a lifecycle signal (step 2).** Both are derived from server state and useful to any client; the hint's presentation and the recovery popup stay client UI. Neither depends on leases or on which client asked.
10. **Subscription bounds (step 2).** The pane bound rose from 10 to 64; the 100,000-cell bound stays, because a backpressured update carries every subscribed screen in one record capped at 16 MiB (about 100–160 JSON bytes per cell), and a grid on one terminal never covers more than that terminal (at most 64,000 cells). Hidden panes are unsubscribed rather than counted.

## Step 2 server support (2026-11-21)

These gaps found by the stream TUI are closed, each behind its own capability (contract in [native-client-protocol.md](native-client-protocol.md#step-2-additions)):

- **Initial panes** (`startup-policy`): `--startup-panes none` starts without panes and leaves the composed UI's `tab-layout.json` and saved tab count untouched; Welcome `startup_panes` reports the policy.
- **Recovery popup** (`recovery-signal`): Welcome `recovery_pending` after an unclean shutdown, cleared for every client by `dismiss_recovery` or by the composed popup.
- **Spawn with another shell** (`spawn-shell`): `spawn {shell}`, validated like the workspace shell and part of the retry identity.
- **Unused-shell hint** (`unused-shell`): `PaneSession.unused_shell`, the composed UI's logic and rate limits, sampled while a `session_events` connection is attached.
- **Subscription limit** (`stream-limits`): 64 panes, 100,000 cells, advertised in Welcome `limits`.
- **Notification run** (`notification-run`): Welcome `notification_run`, the run that notification `origin.run` is compared with, so a client matches notifications to live panes like the composed UI instead of guessing the run.
- **Pane creator** (`spawned-by`): `PaneSession.spawned_by`, the Welcome `client` ID of the connection whose `spawn` or `restore` created the pane; kept for the epoch only.

## Remaining gaps

- **Styled history.** `history` returns plain text. A locally rendered scrollback with colours needs paged rows of cells with stable row anchors (ADR 0001, "Terminal history and synchronization"). Until then, a TUI scrolls with `scroll`/`reveal` and renders the scrolled snapshot. Not planned for step 2.
- **Agent conversation link.** The Codex conversation picker (`AgentLinkPicker`/`AgentLinkChoose`) is dropped from the stream TUI; no stream request is planned.
- **Diagnostics** became the stream `diagnostics` request in step 3 (the one-shot `"Diagnostics"` record stays for tools without a stream session).


## Step 3 (2026-11-21)

KelpServer 0.3.0 removes the composed protocol 58 and the server-side UI. Contract details are in [native-client-protocol.md](native-client-protocol.md#step-3-generic-server-03).

- **Removed:** the composed frames and their deltas, `Action` handling and the controller, mouse routing, title animation, the controller theme fallback, the tab/grid/parking/focus/dialog/scratch state, templates as a startup source, writing `tab-layout.json` and UI keys of `workspace.json`, the composed startup layout (`--startup-panes layout`), the unused-shell UI hint (the observation stays), the recovery popup (the signal stays), the DEV badge, sound and visual notifications, the `SpawnPane`/`InspectPane`/`History`/`SearchPane`/`Command` management requests and the `pane-spawn` capability.
- **Administration on the stream:** `shutdown`, the new `diagnostics` request (`diagnostics` capability), and the pane list from the first `Update`. `kelpserver start`, `status`, `stop` and `ctl` use them; `stop` keeps a legacy shutdown fallback for a running server without `server-lifecycle`. Diagnostics also answer a one-shot `"Diagnostics"` record without Hello.
- **Errors instead of hangs:** an undecodable record, including every composed-protocol message, gets `{"Error":"…"}` naming the removal, then the connection closes.
- **Versions:** crate `kelpserver` 0.3.0-beta.1 and `kelpclient-protocol` 0.2.0 (without `LOCAL_PROTOCOL_VERSION`); the stream stays at version 1. `server.identity.json` records stream version 1 in `protocol`, where 0.2 servers recorded 58.
- **Kept for clients, not used by the server:** `templates` (a client startup layout format), the vitals presentation helpers, `history::RestoreChoice` and the `PaneInfo` fields `parked`, `number` and `palette`, which the server now fills with neutral values (never parked, creation-order position, ID-derived palette).

## Uploads (2026-11-21, decision 151a)

File and image uploads dropped onto a terminal in a client are a server capability (`uploads`), because KelpMux and KelpHub both need them and the files must land where the shells run: the user's machine for KelpMux, the workspace pod for KelpHub, which may have no SSH daemon. Clients own the drop UI and paste the resulting shell-quoted paths into the pane. Contract: [native-client-protocol.md](native-client-protocol.md#uploads-uploads).

| Capability | Session or client UI | Addition |
| --- | --- | --- |
| Upload files to the server host | Session (files where the shells run) | `upload_begin {request, files, dest?}` → `upload_started {request, batch, dir}`; `upload_chunk {batch, file_index, offset, data}` → `upload_progress {batch, file_index, received}` (window of `max_chunks_in_flight` unacknowledged chunks); `upload_finish` → `upload_done {request, batch, paths}`; `upload_cancel` → `done`; `upload_failed {batch, message}` when a chunk fails. Welcome `upload_limits`. Batches are private per-batch directories scoped to the connection; completed batches are kept for a configurable retention (default 7 days). |
| Drop target, progress display, pasting the quoted paths | Client UI | None: the client pastes the `upload_done` paths with `paste` or `input`. |

Decisions:

1. **Encoding.** Chunks are base64 strings in the JSON record: the stream has no binary framing, and the byte arrays `input` uses cost up to four bytes per byte. A 256 KiB chunk is about 342 KiB on the wire, well under the 16 MiB record cap, so pane updates are not starved. Measured locally through the Unix socket (release build, 100 MiB file, tmpfs): about 420–540 MB/s; a debug build about 15 MB/s. No binary framing is needed.
2. **Authorization.** Any stream connection may upload: the same trust as typing into a shell, which any connection may do after claiming a pane.
3. **Order.** Files are sent in request order and each file at strictly sequential offsets, so the server holds one open file per batch.
4. **Failure.** Any failing chunk or finish removes the whole batch; there is no resume. A disconnect removes the connection's unfinished batches.
5. **Retention.** Only batches recorded in the server's `uploads.json` (path, device, inode) are ever deleted, without following symlinks. Unfinished batches of a previous run are removed at startup whatever the retention.

- **Kept for clients, not used by the server:** `templates` (a client startup layout format), the vitals presentation helpers, `history::RestoreChoice` and the `PaneInfo` fields `parked`, `number` and `palette`, which the server filled with neutral values (never parked, creation-order position, ID-derived palette). All of them were removed in 0.3.0-beta.2; see [below](#client-leftovers-removed-2026-11-21).

## Client leftovers removed (2026-11-21)

KelpServer 0.3.0-beta.2 and `kelpclient-protocol` 0.3.0 remove what step 3 kept only because KelpMux imported it; KelpMux keeps its own copies.

- **Removed:** `native::web_relay` and `kelpserver web-relay` (KelpMux has `kelpmux web-relay`), `native::templates`, the vitals presentation functions (`bar_glyph`, `pie_glyph`, `render_segments`, `is_clock_segment`), `history::RestoreChoice` and `Entry::restore_choices`, `upgrade::attach_matching` and `upgrade::matching_binary` (client binary selection), the tmux label escape `label`, the always-empty alias parameter of `tracking::tab_name` and `tracking::shell_tab_name`, and in the protocol crate the `PaneInfo` fields `parked`, `number` and `palette`, `Status::text`, `Status::marker`, `WORKING_FRAMES` and `StatusSource::label`.
- **Kept:** the structured `Vitals` sent in every `Update` and the `Sampler` (including `disk_readings`, the mount paths behind the disk percentages); closed-session records with `Entry::command` and `Entry::previous_command`, which give `resume`/`rerun` their meaning and `ClosedSession.resume`/`rerun` their values; `upgrade::Identity`, `save`, `identify`, `protocol_label` (printed by `kelpserver status`), `speaks_stream` and `LEGACY_COMPOSED_PROTOCOL`, which any client needs to check a running server; `PaneInfo.title`, which is the server's observation of the pane title, not a client name.
- **Wire effect:** `Update.panes` no longer carries `parked`, `number` or `palette`. The stream version stays 1; a client built against `kelpclient-protocol` 0.2.0 requires `number` and `palette` and must move its pin together with the server. See [native-client-protocol.md](native-client-protocol.md#step-3-generic-server-03).
- **Tolerant Welcome:** `StartupPanes` gains `Layout` (sent only by 0.2 servers started with `--startup-panes layout`) and `Unknown` (any other value), so a 0.3 client can still attach to, list and stop such a workspace. A 0.3 server still sends only `none`, and `--startup-panes` still accepts only `none`.

## Binary screens (2026-11-22, ADR 0003 section 2)

Screens (snapshots and patches) can be delivered in a compact binary encoding instead of JSON, about 150 times smaller for a text-heavy 300 × 80 screen. It is a transport option of existing session data, not a new operation, so it is a capability like the others. Contract: [binary-screens.md](binary-screens.md).

| Capability | Session or client UI | Addition |
| --- | --- | --- |
| Binary screen records | Session (transport) | `binary_screens {enabled}`; each later `update` carries `binary_screens: true` and empty `snapshots`/`patches`, and is immediately followed by one binary record (payload first byte `0x00`) with its screens. Decoder: `kelpclient_protocol::screens`. |

Decisions:

1. **Opt-in request, not a Hello field.** `hello` is decoded with unknown fields denied, so an older server would close a connection whose `hello` names the format. The client checks Welcome for `binary-screens` and then sends `binary_screens`, as with every other opt-in.
2. **One record after each update.** The JSON `update` and its binary record form one packet in the server's replaceable-update slot, so backpressure replaces both together and a client never has to match screens to metadata across updates. An update without screen changes is still followed by an empty record, so a client's rule is simply "after an `update` with `binary_screens`, read one binary record".
3. **Whole rows in patches.** The server computes cell changes as before and re-encodes each touched row whole. A row costs a few dozen bytes, so this is simpler than per-cell changes and not larger in practice.
4. **JSON unchanged.** Connections that do not opt in receive exactly the previous JSON, and the server's JSON path does the same work as before. The stream version stays 1.

## Files (2026-11-23, ADR 0003 section 3)

File browsers and editors in KelpMux (TUI and web) and later KelpHub act on the machine where the shells run: the user's host for KelpMux, the workspace pod for KelpHub. Like uploads, they are a server capability (`files`) so every client reaches them the same way, through the stream and its bridges. The design starts from KelpHub's workspace file browser (`files.rs`), moved here with Joan's approval (decision 185b); KelpHub's Git status and `HEAD` comparison stay out of scope (decision 190a). Contract: [native-client-protocol.md](native-client-protocol.md#files-files).

| Capability | Session or client UI | Addition |
| --- | --- | --- |
| List, stat and read files on the server host | Session (files where the shells run) | `file_list {request, path}` → `file_list {request, path, entries, truncated, skipped}`; `file_stat {request, path}` → `file_stat {request, path, entry}`; `file_read {request, path, max_bytes?}` → `file_content {request, path, size, modified, mode, hash, binary, encoding, content}`. Welcome `file_limits`. |
| Change files on the server host | Session | `file_write {request, path, content, encoding?, base_hash?, create?}` → `file_written {request, path, hash, size, modified, created}`; `file_mkdir {request, path}` and `file_rename {request, from, to, overwrite?}` → `done`; `file_trash {request, paths}` and `file_delete {request, paths}` → `file_removed {request, removed, failed}`. Failures: `file_error {request, error {code, message, path?, current_hash?, size?}}`. |
| Browser tree, editor, dirty state, conflict dialog, confirmation before delete, trash view and restore | Client UI | None: the client keeps the hash it read and sends it as `base_hash`; on `conflict` it shows the choice. Restoring from the trash is a `file_rename` out of `<trash>/files` (and a `file_delete` of its `info/*.trashinfo`). |

Decisions:

1. **Authorization and threat model.** Any stream connection may use the file operations: the same trust as uploads and as typing into a shell, which can already read and change every file of the server's user. They run with the server's credentials and never elevate; the security boundary remains the authenticated stream connection. Since 2026-11-23 they are confined to a file root (see [File root](#file-root-2026-11-23-adr-0003-section-3)), which limits the file browser, not the user. They must not be a way around anything the server enforces, so they never act through a symlink at the final component of a change, never follow symlinks while deleting a tree, refuse `..` and relative paths, and are bounded in sizes, entries, paths per request and concurrency. A relaying front end that serves several users applies its own policy before relaying them.
2. **Paths.** Absolute, or `~`/`~/…` relative to the server user's `HOME`; normalised lexically; `..` is refused rather than resolved, because lexical and physical `..` differ under symlinks and clients address entries they listed. Reads, stats and listings follow symlinks in intermediate components like a shell; only the final component of a change is protected.
3. **Content and hashes.** Text is UTF-8 in the JSON string; content with NUL or invalid UTF-8 (`binary: true`) is base64, as is text whose JSON escaping would overflow a record. Reads return the whole file up to 8 MiB or a `too_large` error with the size (no partial reads). `hash` is SHA-256 in lowercase hexadecimal, computed by a small in-tree implementation so the server needs no new dependency.
4. **Writes.** Atomic replacement (temporary file in the same directory, sync, rename, directory sync) keeping the permission bits; `base_hash` makes it a compare-and-swap with a `conflict` error carrying the current hash; `create` never replaces anything. Writing through a symlink is refused rather than followed. Files of another user are refused, since a replacement would change their owner, and so are read-only files, as a shell's `>` refuses them. The group is kept when the user may set it; extended attributes and ACLs are not. The compare-and-swap re-checks the file's inode, size and times just before the rename, so an in-place change after hashing is a `conflict` too; a file larger than a read can return is a `conflict` without being hashed.
5. **Rename** refuses to replace an existing target unless `overwrite` is set, and never crosses file systems (`cross_device`; no copy fallback).
6. **Trash** is the freedesktop.org home trash (`$XDG_DATA_HOME/Trash`, `~/.local/share/Trash`), so desktop file managers see and restore the entries: an `O_EXCL` `.trashinfo` reservation first, then a non-replacing rename, numbered names on collision (`a.2.txt`), the info file removed again if the move fails. This order follows the specification and Crabinet's trash (a reserved sidecar before the payload moves, rollback on failure). Per-volume trash directories are not implemented: an entry on another file system fails with `cross_device`, and the client offers a permanent delete.
7. **Delete** is immediate and recursive through directory descriptors (`O_NOFOLLOW` at every level), bounded to 64 levels, and never enters another file system (`cross_device`, like `rm --one-file-system`). Neither delete nor trash removes `/`, the server user's home directory or the file root itself.
8. **Batches** (`file_trash`, `file_delete`) validate every path first, then report each path's outcome, so a client can show which entries remain.
9. **Threading.** File operations run on worker threads, at most 4 per connection and 16 per server, of which 4 deletions (`busy` beyond), and their replies are delivered by the event loop like other correlated replies; pane output is never delayed by a slow disk. Replies are correlated, not ordered. File replies wait in a per-connection queue until the output queue has room, so large reads finishing together or a burst of `busy` refusals do not disconnect the client; an operation keeps its slot until its reply enters the output queue. The deletion bound caps the directory descriptors file operations hold (one per level of a tree), and the server raises its soft open-file limit to the hard limit (at most 65,536; shells get the original limit back) and survives `accept` running out of descriptors. There is no cancellation: an operation stuck on a hung mount keeps its slot, and enough of them make file requests answer `busy` until they return or the server restarts.
10. **Compatibility.** Additive: a new capability, new request and event types and one optional Welcome field (`file_limits`). File events reach only the connection that sent the request. The stream version stays 1.

## Downloads (2026-11-23, ADR 0003 section 3)

The file browsers save files and folders from the server host on the user's computer: one regular file as itself, folders and selections as one `.tar.gz`. Like uploads and the file operations, this is a server capability (`file-downloads`, an addition to `files`), because the files live where the shells run and KelpHub's pods may have no SSH daemon to `scp` from. Contract: [native-client-protocol.md](native-client-protocol.md#downloads-file-downloads).

| Capability | Session or client UI | Addition |
| --- | --- | --- |
| Stream a file or an archive of files and folders to the client | Session | `file_download_begin {request, paths, archive?}` → `file_download_started {request, download, name, size?, archive?, entries?, input_bytes?}`; `file_download_chunk {download, offset, data, input?}` acknowledged by `file_download_ack {download, offset}` (window of `max_chunks_in_flight`); `file_download_done {request, download, size, hash, skipped, changed}`; `file_download_cancel {request, download}` → `done`. Failures: `file_error {request, download?, error}`. Welcome `file_limits.downloads`. |
| Save picker or browser download, progress, cancel | Client UI | None: the browser streams to the file the user picked (File System Access API) or collects the bytes in a Blob up to a cap, and acknowledges each chunk once stored. |

Decisions:

1. **Flow control like uploads, in reverse.** The server streams chunks of 256 KiB and stops after 8 unacknowledged ones; the client acknowledges each chunk once stored. The reading thread produces into a bounded channel and the event loop moves a chunk to the connection only within the window, while the output queue keeps the room file replies leave (half its messages, 512 KiB of bytes), with at most two chunks' worth of download events queued per connection, and never ahead of a pending screen update. A download left waiting for its client for 10 minutes (no acknowledgement, nothing taken into the output queue) is stopped with `io`, so a client that gave up on a start or stopped reading cannot hold its slot and thread; the web client also cancels a download whose start arrives after its call timed out. A slow browser therefore slows its own download and never fills the 16 MiB output queue, disconnects or starves screen updates. Acknowledgements carry the end offset so a client cannot skip one by accident.
2. **Archive format: tar.gz.** Requested by Joan ("maybe in tar.gz format"); every platform opens it (macOS Archive Utility, Windows 11 Explorer, `tar`, 7-Zip), and a tar stream needs no central directory, so it streams without knowing sizes in advance. ZIP was not chosen: its streaming form needs data descriptors and ZIP64 bookkeeping (Crabinet's streaming ZIP writer is 900 lines) for no gain over tar.gz here. The tar writer is in-tree (ustar plus pax records, about 200 lines); the `tar` crate would add little for a writer and reads files by path. gzip uses `flate2` with its pure-Rust `miniz_oxide` backend (`default-features = false`, `rust_backend`): maintained by the Rust project (about 890 million downloads), no C library, no RustSec advisories; its `miniz_oxide` and `adler2` are already in KelpMux's lock through `backtrace`; level 2 keeps one stream above gigabit rates on one core for most of the gain of higher levels (measured by Crabinet).
3. **Bounds are refused, not truncated.** A planning walk (metadata only) counts entries and input bytes before the first byte and refuses an archive over 100,000 entries, 16 GiB of input or 64 levels with `too_large`, as well as a walk that lists more than 200,000 names (special files included) or holds more than 100,000 listed names across its open folders; a tree that grows past a bound while streaming fails the same way. A partial archive is never reported as complete. A single file has no size bound: it streams at the client's pace and can be cancelled.
4. **What an archive holds.** Regular files, folders and symlinks (as links, never followed). Devices, FIFOs and sockets are skipped without being opened; unreadable entries and the contents of mount points are skipped too (`tar --one-file-system`, with each opened folder checked against its listing's device and inode); all are counted in `skipped` so the client can say so. Permission bits and modification times are kept; owner and group are 0 without names. A file that changes while read keeps its header's size (cut or zero-padded) and counts in `changed`.
5. **Concurrency.** Downloads have their own bounds (2 per connection, 8 threads per server) instead of the 4 file-operation slots, which a long download would otherwise hold and stall browsing. Cancelling or disconnecting drops the download; its thread stops at its next read, chunk or walked name, and counts against its connection until it returns, so cancelling and restarting cannot exceed the allowance. A thread stuck on a hung mount keeps its server-wide slot, like other file operations.
6. **Compatibility.** Additive: a new capability, three requests, three events, an optional `download` field on `file_error` (only for a download that failed after it started) and an optional `downloads` object in `file_limits`, which older clients ignore. The stream version stays 1.

## File root (2026-11-23, ADR 0003 section 3)

Joan: "the file browser should be gated to the user's home folder". Every file operation, download and client-chosen upload destination is confined to one folder, enforced by the server. Contract: [native-client-protocol.md](native-client-protocol.md#files-files).

| Capability | Session or client UI | Addition |
| --- | --- | --- |
| Confine file operations to a folder | Session | Welcome `file_limits.root` (the canonical root); `file_error` code `outside_root`; `kelpserver server\|start --files-root PATH` and `files_root` in `workspace.json` (`files::Settings::root`). |
| Start browsing in the root, offer nothing outside it, hide or disable path links outside it | Client UI | None: the client reads `file_limits.root`. |

Decisions:

1. **The root.** The server user's home directory by default (`HOME`, or the password database's entry when `HOME` is unset or relative), or a configured folder, so KelpHub can confine a workspace pod to its workspace. It is resolved once at startup (canonicalised; it must be an existing directory). A server that cannot resolve it still serves terminals, logs why and refuses every file request with `outside_root`, advertising no `root`.
2. **Lexical check, then resolution beneath the root.** A normalised path must start with the root (component-wise), or with the configured spelling when that differs (a symlinked `HOME`; replies use the canonical spelling). It is then opened beneath the root: `openat2` with `RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS` (Linux 5.6+), which the kernel enforces against concurrent renames on `..`. `RESOLVE_BENEATH` refuses every absolute symlink, even one pointing inside the root (common in home directories), and some systems lack `openat2`, so on `EXDEV` or `ENOSYS` the server walks the path itself: one component at a time with `O_NOFOLLOW` from the previous directory descriptor (the kernel never follows a symlink), each symlink read and resolved in the server (an absolute target only when it lies inside the root, `..` back to the descriptor it came from, never above the root, at most 40 links). Both give the same answers; the walk decides the cases `openat2` refuses. Mutating operations resolve the parent this way and act on the final name through its descriptor, as before.
3. **Symlinks pointing outside** are listed and stat'ed as links with their target (what the user sees in `ls -l`), but listing, reading, writing, creating, renaming or downloading through them fails with `outside_root`. Deleting or trashing such a link removes the link. Archives store symlinks as links, as before.
4. **The root itself** can be listed and stat'ed, never written, renamed, moved, trashed, deleted or downloaded (`invalid_request`); the removal protection also matches it by device and inode, so a bind mount of the root inside it is protected too. Mount points inside the root stay usable (only deletions and archives stay on one file system, as before).
5. **What it is not.** The gate limits the file browser and editor; it is not a sandbox. Any stream connection can still type commands into a shell running as the same user, and the trash and upload directories are server configuration that may lie outside the root. A directory moved out of the root by another process while a path is being resolved can still be traversed (true of `RESOLVE_BENEATH` too, which guards `..` only); only the user's own processes can do that.
6. **Compatibility.** Additive on the wire: one optional `file_limits` field and one error code, which older clients show by its message. Older clients that open `/` or paths outside the home directory get `outside_root`. In the Rust API, `files::Limits` gains `root` and loses `Copy`, `files::Settings` and `lifecycle::Settings` gain a field, and `files::execute`, `downloads::start` and `uploads::Uploads::begin` take the root.

## Extra file roots (2026-11-24, ADR 0003 section 3)

KelpMux lets the user browse folders outside the home directory (a data drive at `/mnt/data`, say) while the home directory stays the primary root. The server accepts a list of extra roots beside the primary one. Contract: [native-client-protocol.md](native-client-protocol.md#files-files).

| Capability | Session or client UI | Addition |
| --- | --- | --- |
| Open further folders to file operations | Session | `kelpserver server\|start --files-extra-root PATH` (repeatable) and `files_extra_roots` in `workspace.json` (`files::Settings::extra_roots`); Welcome `file_limits.roots` (`[{path, label}]`, primary first); `file_error` code `cross_root`. |
| Show the roots as places, start in the primary one | Client UI | None: the client reads `file_limits.roots`, falling back to `file_limits.root`. |

Decisions:

1. **Configuration.** Same precedence as `files_root`: the command line's list, when given, replaces the configuration file's whole list (and `start` records it in `workspace.json`). Paths are made absolute and resolved at startup like the primary root.
2. **Disjoint roots.** An extra root that is missing or not a directory, lies inside the primary root, contains the primary root, repeats another or lies inside another extra root (the outer one is kept) is skipped with a logged warning; the server still starts. At most 64 are used. Skipping an extra root that contains the primary root is the restrictive choice: the primary root stays the boundary around the home directory, and every path belongs to exactly one root.
3. **One root per path.** A request path is confined to the root whose canonical path or configured spelling is its longest prefix, then resolved beneath that root only, with the same `openat2(RESOLVE_BENEATH)` and `O_NOFOLLOW` walk as before. A symlink from one root into another is refused (`outside_root`) like any symlink pointing outside, even though the target is reachable through its own root.
4. **No moves between roots.** `file_rename` across two roots fails with `cross_root` before anything is opened; nothing is copied. Downloads may select entries of several roots in one archive, each opened beneath its own root. Trash stays the configured (home) trash, and trashing is a move too (security review, 2026-11-24): an entry may go to a trash inside its own root, and an entry of the primary root also to a trash outside every root; any other entry fails with `cross_root` and stays in place, so with the home directory as primary root and the default trash, entries of extra roots are deleted, not trashed. An entry on another file system than the trash still fails with `cross_device`. Both ends of a rename, an overwritten target included, get the same removal protection as delete (`/`, the home directory, a root under any name).
5. **Roots pinned at startup** (security review, 2026-11-24). Each root's device and inode are recorded when the server starts, and every open of a root (the start of every resolution) must find that directory, opened without following a final symlink; removal protection compares with the recorded identity instead of looking the path up again. If someone able to change a parent folder (a shared data drive, say) replaces a root or a folder above it with a symlink elsewhere, file requests in that root fail with `outside_root` until the server restarts, rather than following it. Keeping an `O_PATH` descriptor per root would have kept serving the old directory, but ties descriptors to a cloneable configuration value; the identity check is simpler and fails closed.
6. **Labels.** `Home` for the primary root when it is the home directory, else the folder's name; an extra root's label is its folder's name, or its full path when that name is empty or shared by another root.
7. **Compatibility.** Additive on the wire: one optional `file_limits` field and one error code. `file_limits.root` keeps its meaning (the primary root). In the Rust API, `files::Limits` gains `roots`, `files::Settings` gains `extra_roots`, `lifecycle::Settings` gains `files_extra_roots`, and `files::Root` gains `with_extras`, `paths` and `named`.

## Bell merge (2026-11-23, ADR 0003 section 3)

A bell and the agent notification it accompanies are one event, merged by the server so every client shows one notification (decision 205a). Before, the server recorded a bell (`unknown` status, `Bell · ` title) and the typed notification separately, and each client paired them itself: KelpMux web's `bellCompanion` and the TUI's `notification_state` looked for a typed notification of the same pane and process within 2 seconds. Contract: [native-client-protocol.md](native-client-protocol.md#notifications-host-status-and-server-lifecycle).

| Capability | Session or client UI | Addition |
| --- | --- | --- |
| Merge a bell with the typed notification it accompanies | Session | `Notification.bell` (`bell-merge`): the typed notification of the same pane and process within 2 seconds, in either order, carries `bell: true` and no separate bell record remains. |
| Sound, toast and favicon for a merged bell | Client UI | Derived from `bell` and the typed status. |

Decisions:

1. **Which record survives.** The typed one: it says what happened (question, finished, failed, exited), which is what clients showed for a paired bell anyway. A bell first is removed and the typed notification is recorded with a new ID, so clients alert for it as a new event; a typed notification first keeps its ID and read state and only gains `bell: true`, so nothing alerts twice.
2. **No delay.** Bells are recorded at once and merged when the typed notification arrives; holding a bell back for 2 seconds to wait for a companion would delay every lone bell. The merge is decided when the second event arrives, so the server needs no timer.
3. **Identity and window.** Same pane, same server run and same process ID (`origin`), at most 2 seconds apart, the window the clients used. Bells of another pane or process, or outside the window, stay separate records.
4. **Read marks.** The merged record is one notification, so acknowledging it clears the event. An acknowledgement of a bell ID that was merged after the client saw it reaches the merged record (an in-memory map for the server's run, bounded to 64 entries).
5. **Compatibility.** Additive: a new capability and one optional field, omitted when false. Older clients see fewer bell records, and their own pairing finds nothing to pair; they show the typed notification without its bell. The stream version stays 1.
