# ADR 0002: KelpServer is a generic session server

- Status: Accepted by Joan 2026-11-20 (America/Los_Angeles). Step 1 implemented; step 2 server support implemented 2026-11-21 (the KelpMux side of step 2 is separate); step 3 implemented 2026-11-21 (KelpServer 0.3.0-beta.1), its client leftovers removed 2026-11-21 (KelpServer 0.3.0-beta.2); step 4 not started.
- Related: [ADR 0001](0001-native-client-protocol-and-device-capabilities.md), KelpMux [ADR 0003 (web client)](https://github.com/joanfabregat/kelpmux/blob/main/docs/adr/0003-web-client-in-kelpmux.md), KelpHub ADR 0007 (multi-tenant, Kubernetes-only; private repository).

## Context

KelpServer was extracted from KelpMux on 2026-11-20 so that KelpMux (TUI and web client) and KelpHub depend on one server contract. The extracted code still works like tmux: besides holding the sessions, it owns the TUI's user interface. The TUI is a thin client of frames the server composes (local protocol 58). Tabs and their order, parking, focus, dialogs, grid geometry, scratch and the screen rendering all live in the server, about 6,500 of 18,200 lines (`render.rs`, `workspace.rs`, `ui.rs`) plus a large part of `server.rs`.

The web client already uses the per-pane stream protocol and keeps its own layout. KelpHub will run KelpServer inside each workspace pod and has its own UI.

## Decision

**Boundary rule:** KelpServer contains only what KelpMux and KelpHub share. Everything specific to KelpMux lives in KelpMux; everything specific to KelpHub lives in KelpHub.

KelpServer keeps:

- Sessions: PTYs and processes, pane create/close/list with working directory, arguments and a sanitized environment, terminal emulation, scrollback/history, search, the per-pane stream protocol with per-client leases and sizes, resize.
- Records of closed sessions needed to resume or rerun them.
- Pane observations: running program, agent detection, activity and notifications, shared by every client (143a).
- Server lifecycle: start, stop, upgrade, status, journal.

KelpServer drops, moving them to KelpMux:

- The composed protocol (local protocol 58), `render.rs` and the server-side TUI state: tabs, tab order, names, colours, parking, focus and focus history, dialogs, grid geometry, scratch, templates and TUI settings. The TUI becomes a stream-protocol client that renders locally, like the web client.
- The web relay (`web_relay.rs`), which serves KelpMux's web UI (145a).

Client UI state is owned by each client, keyed by pane identity (server epoch plus pane ID): names, colours, order and parking are not shared between clients (144b). The TUI persists its own state on disk; the web keeps it per browser. Consequence: a name or colour set in the TUI does not appear in the web, and two TUI instances share only what they persist in the same file.

The composed protocol is removed once the new TUI works (146a). Running pre-prod installations keep their current server and TUI build until they are switched deliberately; a new TUI requires a new server.

## Migration

1. Define the session API needed by a full TUI on the stream protocol (pane lifecycle, observations, closed-session records), additively, while protocol 58 still works. Version the stream protocol if the contract changes.
2. In KelpMux, rebuild the TUI as a stream client with local rendering and local UI state, moving `render.rs`, `workspace.rs` and `ui.rs` logic there. Move the web relay to KelpMux.
3. Remove protocol 58 and the UI code from KelpServer; bump the major version.
4. KelpHub consumes the session API from its pods.

The tab behavior merged in [kelpserver#1](https://github.com/joanfabregat/kelpserver/pull/1) (unparked tabs go last, closing or parking refocuses the previous tab) moves with the TUI code in step 2.

## Open points

Both points were decided during step 1; see [Implementation notes](#implementation-notes).

- Exact split of `server.rs`, `status.rs` (host vitals) and `discovery.rs` (agent and tmux discovery) between the two sides; decided during step 1 by the boundary rule.
- Whether closed-session records stay generic or become client-owned.

## Implementation notes

### Step 1: session API (2026-11-20)

The pane stream now carries the session API: pane lifecycle events and per-pane session data, `spawn` and `close`, closed-session records (`closed_sessions`, `restore`), history and multi-pane search, lease-bound scrolling, `acknowledge_all`, opt-in host status, `shutdown` and `server_stopping`, and the server version in Welcome. Everything is additive and capability-gated; the stream stays at version 1, events of new types reach only connections that opted in or sent the request, and protocol 58 is unchanged. The contract is in [native-client-protocol.md](../native-client-protocol.md#session-api); the capability inventory, the decisions and the gaps left for step 2 are in [session-api.md](../session-api.md). No UI code moved and the server stores no new client UI state.

Parking needs no server concept: a client unsubscribes from a hidden pane and keeps its lease (fixed size) or releases it (base geometry). The scratch terminal is an ordinary pane created with `spawn`. Tab order can default to pane ID order, which is creation order within an epoch.

The open points were decided by the boundary rule:

- **`server.rs`, `status.rs` and `discovery.rs`.** `discovery.rs` (Claude/Codex agent discovery and polling) produces pane observations shared by every client and stays. `status.rs` splits: host sampling (`Monitor`, `Snapshot`, `Process`, `Disk`) stays and is exposed by `host-status`; its presentation (`View`, `Sort`, column and layout helpers, formatting) moves to KelpMux with the Status dialog in step 2. `server.rs` keeps PTYs, the stream and its leases, spawn/close/restore, discovery, history and activity; composed-frame production, `mouse`, the `Action` handling, the TUI controller and theme fallback, title animation, unused-shell hints and the UI parts of `persist_geometry` leave in step 3, after the TUI no longer needs them.
- **Closed-session records stay in the server and stay generic.** They are derived from server observations (foreground programs, agent sessions), are useful to KelpMux and KelpHub alike, and are addressed by an opaque `key`; only their presentation (restore-choice labels, copying the resume command) is client UI.

### Step 2 server support, 2026-11-21

The KelpMux stream TUI's parity gaps that need the server are closed, additively and behind new capabilities; the stream stays at version 1 and protocol 58 is unchanged. `--startup-panes none` lets the client that starts the server own the initial layout: the server starts with zero panes (nothing in it needs one) and leaves the composed UI's saved tab layout alone. Welcome now carries the startup policy, the unclean-shutdown recovery signal (cleared by `dismiss_recovery` or the composed popup), the subscription limits (64 panes, up from 10; the 100,000-cell bound stays because of the 16 MiB record limit) and the notification run that notification origins are matched against. `spawn` accepts a validated `shell`. Per-pane session data gains `unused_shell`, the composed UI's hint as an observation, and `spawned_by`, the creating stream connection for the epoch. All of these are session facts or lifecycle signals shared by any client, so none adds client UI state to the server. Styled history and the Codex conversation picker are not part of step 2. Details are in [session-api.md](../session-api.md#step-2-server-support-2026-11-21).

### Step 3: protocol 58 and the UI code removed, 2026-11-21

KelpServer 0.3.0-beta.1 speaks only the pane stream (version 1, unchanged). Contract: [native-client-protocol.md](../native-client-protocol.md#step-3-generic-server-03); inventory: [session-api.md](../session-api.md#step-3-2026-11-21).

Removed from the server: `render.rs` (composed frames, the controller's mouse routing, title animation, the DEV badge), `workspace.rs` (tabs, order, parking, focus history, grid geometry, scratch), `ui.rs` (dialogs, status View/Sort and layout, settings, sound and visual notifications), `delta.rs` (composed frame deltas), the `Action` handling, the TUI controller and its theme fallback, the UI parts of `persist_geometry` and `tab-layout.json` writing, templates as a startup source, the composed startup layout, the unused-shell UI hint and the recovery popup UI, every composed `ClientMessage`/`ServerMessage` variant (`Attach`, `Action`, `Command`, `Input`, `Inspect`, `InspectPane`, `History`, `SearchPane`, `SpawnPane`, `Frame`, `Welcome`, `State`, …), the `pane-spawn` capability, `protocol::VERSION`, `protocol::validate_size`, `LOCAL_PROTOCOL_VERSION`, the selection and legacy search APIs of the engine, the `benchmark` diagnostics and the corresponding tests.

Decisions taken in this step:

- **Startup.** The server always starts with zero panes; `--startup-panes none` stays accepted and `layout` fails with an error. `--size` is the base size of unleased panes.
- **Closing the last pane** still creates a replacement shell first (reported by `pane_created`, without `spawned_by`); the last pane's exit stops the server.
- **Administration on the stream.** `shutdown`, a new `diagnostics` request (capability `diagnostics`) and the first `Update` serve `kelpserver start`, `status`, `stop` and `ctl list|diagnostics|shutdown`. The one-shot `"Diagnostics"` record stays for tools without a stream session. `stop` keeps a legacy shutdown fallback only for a running server without `server-lifecycle`.
- **Clear errors.** An undecodable record (any composed-protocol message) gets `{"Error":"…"}` naming the removal and the connection closes. The SSH bridge reports a workspace whose server does not speak the stream.
- **Versions.** `kelpserver` 0.3.0-beta.1; `kelpclient-protocol` 0.2.0 without `LOCAL_PROTOCOL_VERSION`. `upgrade::Identity.protocol` now records the stream version (1); 58 identifies a 0.2 server, which also speaks stream version 1. `kelpserver status` prints "stream protocol 1" or "local protocol 58".
- **Kept for KelpMux, not used by the server:** `templates` (a startup layout format the client may use), the vitals presentation helpers, `history::RestoreChoice`, `upgrade::attach_matching` and the `PaneInfo` fields `parked`, `number` and `palette` (filled with neutral values). They can move to KelpMux when it no longer imports them. Removed 2026-11-21; see [Client leftovers removed](#client-leftovers-removed-2026-11-21).
- **Web relay.** `web_relay.rs` and `kelpserver web-relay` stay for now although this ADR moves them to KelpMux (step 2); KelpMux already has its own copy, so removing the server's copy is a follow-up. Removed 2026-11-21 with the other client leftovers.

Public modules after step 3 (`kelpserver::native`): `activity`, `background`, `bridge`, `cli`, `connection` (`connect`, `response`, `Session`), `diagnostics`, `engine`, `history`, `hooks`, `lifecycle` (`Settings {shell, cwd}`, `StartRequest`, `start`, `stop`, `workspace_path`, `detach`, `private_state`, `save_pid`, `begin_run`, `finish_run`), `program_status`, `protocol` (envelope, framing, `MAX_MESSAGE`, `MAX_INPUT`, `MouseEvent`, `MouseKind`, `Notification`, `NotificationOrigin`, `PaneSearchMatch` and the re-exported `kelpclient_protocol` types), `recovery`, `reference`, `server` (`ServerConfig`, `serve`), `shell`, `status` (`Monitor`, `Snapshot`, `Process`, `Disk`), `stream`, `templates`, `tracking`, `upgrade`, `vitals` and `web_relay`. Crate root: `discovery`, `journal`, `model`, `process` and the helpers `Result`, `label`, `now`, `session_id`, `shell_quote`.

### Uploads capability, 2026-11-21, decision 151a

File and image uploads (files dropped onto a terminal in a web client) are a KelpServer stream capability, `uploads`, because both clients need them and the files must land where the shells run: jbox for KelpMux, the workspace pod for KelpHub, which may have no SSH daemon. Under the boundary rule they are shared, generic session functionality: the server stores files in private per-batch directories and returns absolute paths; the drop UI, progress display and pasting the shell-quoted paths into the pane stay client UI. The stream stays at version 1: four correlated or flow-controlled requests (`upload_begin`, `upload_chunk`, `upload_finish`, `upload_cancel`), their replies (`upload_started`, `upload_progress`, `upload_done`, `upload_failed`, `done`) and Welcome `upload_limits`. Batches are scoped to the connection; completed batches are kept for a configurable retention (`--upload-retention-days`, default 7) and only registered batch directories are ever deleted. Any stream connection may upload, the same trust as typing into a shell. Contract: [native-client-protocol.md](../native-client-protocol.md#uploads-uploads); inventory: [session-api.md](../session-api.md#uploads-2026-11-21-decision-151a).

### Client leftovers removed, 2026-11-21

KelpServer 0.3.0-beta.2 and `kelpclient-protocol` 0.3.0 remove what step 3 kept only because KelpMux imported it. KelpMux copies what it still needs when it moves its pin. Inventory: [session-api.md](../session-api.md#client-leftovers-removed-2026-11-21).

Removed: `native::web_relay` and `kelpserver web-relay` (KelpMux owns `kelpmux web-relay`), `native::templates`, the vitals presentation functions (`bar_glyph`, `pie_glyph`, `render_segments`, `is_clock_segment`), `history::RestoreChoice` and `Entry::restore_choices`, `upgrade::attach_matching` and `upgrade::matching_binary`, the crate-root `label` (tmux format escaping), the always-empty alias parameter of `tracking::tab_name` and `tracking::shell_tab_name`, and in `kelpclient-protocol` the `PaneInfo` fields `parked`, `number` and `palette`, `Status::text`, `Status::marker`, `WORKING_FRAMES` and `StatusSource::label`.

Kept by the boundary rule: the structured `Vitals` in every `Update` and its `Sampler`; closed-session records with `Entry::command` and `Entry::previous_command`, which define `resume` and `rerun`; `upgrade::Identity`, `save`, `identify`, `protocol_label`, `speaks_stream` and `LEGACY_COMPOSED_PROTOCOL`, which the server's own lifecycle and `status` use and any client needs to check a running server; `PaneInfo.title`, the server's observation of the pane title.

Wire effect: `Update.panes` no longer carries `parked`, `number` or `palette`. The stream stays at version 1; a client decoding with `kelpclient-protocol` 0.2.0 requires `number` and `palette`, so clients move their protocol pin with the server. Welcome `startup_panes` now decodes the 0.2 value `layout` and unknown values (`StartupPanes::Layout`, `StartupPanes::Unknown`) instead of failing the whole Welcome; a 0.3 server still sends only `none`.

Public modules after this change (`kelpserver::native`): `activity`, `background`, `bridge`, `cli`, `connection` (`connect`, `response`, `Session`), `diagnostics`, `engine`, `history` (`Entry`, `History`, `collect`), `hooks`, `lifecycle` (`Settings {shell, cwd}`, `StartRequest`, `start`, `stop`, `workspace_path`, `detach`, `private_state`, `save_pid`, `begin_run`, `finish_run`), `program_status`, `protocol` (envelope, framing, `MAX_MESSAGE`, `MAX_INPUT`, `MouseEvent`, `MouseKind`, `Notification`, `NotificationOrigin`, `PaneSearchMatch` and the re-exported `kelpclient_protocol` types), `recovery`, `reference`, `server` (`ServerConfig`, `serve`), `shell`, `status` (`Monitor`, `Snapshot`, `Process`, `Disk`), `stream`, `tracking`, `upgrade` (`Identity`, `LEGACY_COMPOSED_PROTOCOL`, `save`, `identify`) and `vitals` (`Vitals`, `Sampler`, `clock`, `cpu`, `disk`, `mem`). Crate root: `discovery`, `journal`, `model`, `process` and the helpers `Result`, `now`, `session_id`, `shell_quote`.
