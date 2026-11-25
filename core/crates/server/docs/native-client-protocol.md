# Native client foundation

KelpServer owns long-running PTYs and Alacritty terminal interpretation. Clients own presentation. Native mode has no tmux or WezTerm runtime dependency. Since 0.3 ([ADR 0002](adr/0002-generic-session-server.md) step 3) the server speaks only the pane stream, version 1, on its local socket and through the SSH bridge: the composed-screen protocol 58 and all server-side UI (tabs, parking, dialogs, layout, frames) were removed. A record of the old protocol gets an `Error` reply and the connection closes. A graphical desktop/mobile client and MCP/device tools are not implemented.

The broader proposed contract is [ADR 0001](adr/0001-native-client-protocol-and-device-capabilities.md). It covers pane lifecycle, independent device sizes, live agent status, notifications, remote Vitals and a future MCP/device request channel. The ADR deliberately distinguishes those proposals from the implemented requests below.

KelpMux's TUI and terminals-only browser client are stream clients that render locally; the browser client and its `kelpmux web-bridge` are described in [KelpMux docs/web.md](https://github.com/joanfabregat/kelpmux/blob/main/docs/web.md). KelpHub keeps its own browser application and authenticated management adapter. Shared portable types and terminal client state live in [KelpClient](https://github.com/joanfabregat/kelpmux/blob/main/crates/client/README.md) in KelpMux. Desktop/mobile implementation choices remain deferred.

Screen input modes include an additive, default-zero `keyboard_flags` bitfield using the five Kitty keyboard enhancement flags. Native clients must preserve modified Enter events and distinguish key release from key press. Pane snapshots preserve default foreground/background colors so the client's terminal or renderer supplies its theme. Explicit application colors and OSC palette changes remain pane-local.

A pane's lease owner can supply its OSC 10/11 query fallbacks with the `controller-theme` request (below). Application OSC colors take priority, while default snapshot cells remain `Default`. A pane without a lease theme answers with documented emulator fallbacks. Theme reports do not force an existing application to repeat its startup probes or replace explicit application RGB colors.

## SSH command channel

`kelpserver bridge --workspace NAME` relays bounded native-stream records between standard input/output and the existing owned private Unix socket. It does not start a workspace. A future native client opens it through one authenticated SSH command channel without an outer PTY:

```sh
ssh -T HOST kelpserver bridge --workspace NAME
```

Use the installed binary's absolute path if it is not in the remote noninteractive PATH. SSH supplies authentication, encryption and transport; there is no additional public port. Shell banners must not appear on stdout. Bridge diagnostics go to stderr, and a TTY endpoint is rejected before terminal-mode changes. EOF detaches that connection and releases its leases, preserving workspace processes. Unacknowledged requests are not automatically replayed. The bridge does not provide mobile roaming or background push delivery.

## Framing and negotiation

Every record has a four-byte unsigned big-endian payload length followed by UTF-8 JSON, with a 16 MiB maximum payload. JSON examples below are payloads, not newline-delimited wire records. Use the same framing over the local Unix socket or SSH bridge. The one exception is the binary screen record of the `binary-screens` capability: its payload starts with the byte `0x00`, which no JSON payload starts with, and the server sends it only to a connection that opted in, directly after an `update` (see [binary-screens.md](binary-screens.md)). Clients never send binary records.

```json
{"Stream":{"type":"hello","version":1}}
```

The server responds with `Stream` / `welcome`, containing stream version, connection/client ID, a random server epoch, and supported capabilities. Send Hello exactly once before other stream requests. Unsupported versions fail clearly. Current capabilities are `pane-snapshots`, `pane-patches`, `pane-leases`, `metadata`, `notifications`, `vitals`, `hyperlinks`, `program-status`, `controller-theme` and `clipboard-write`, plus the [session API](#session-api) capabilities `pane-lifecycle`, `closed-sessions`, `pane-history`, `pane-scroll`, `notification-ack-all`, `host-status`, `server-lifecycle`, `startup-policy`, `recovery-signal`, `spawn-shell`, `unused-shell`, `stream-limits`, `notification-run`, `spawned-by`, `diagnostics`, `uploads`, `binary-screens`, `files` and `bell-merge`; do not infer support for undeclared future capabilities. A server with `server-lifecycle` also puts its package version in Welcome `server_version`.

The socket envelope has three client records and three server records. A client sends `{"Stream":{…}}` requests, or the one-shot `"Diagnostics"` on a connection that has not sent Hello. The server sends `{"Stream":{…}}` events, `{"Diagnostics":{…}}` in reply to the one-shot request, and `{"Error":"…"}` for a record it cannot accept. A record it cannot decode, such as one of the removed composed protocol 58 (`Attach`, `Command`, `Inspect`, `SpawnPane`, …), gets an `Error` that names the removal and the connection closes, so an old client reports an incompatible server instead of hanging. A stream connection that sends `"Diagnostics"` gets an `Error` and keeps its connection. The bridge accepts only stream requests.

Clients recognizing `clipboard-write` may opt in with `{"Stream":{"type":"clipboard","enabled":true}}`. Application OSC 52 writes then produce `clipboard` events containing `pane` and UTF-8 `text` (at most 64 KiB), delivered only to that pane's current lease owner. Observers and clients that have not opted in receive no clipboard data. Writes from panes without a stream owner are dropped. Clipboard events are ephemeral: never replay them in snapshots, history or reconnect state. Clipboard reads remain disabled. Browser clients must provide an explicit copy fallback when platform user-activation rules prevent an asynchronous write.

The `pane-spawn` capability and its separate-connection `SpawnPane` request were removed in 0.3; create panes with the stream `spawn` request of the [session API](#pane-lifecycle-pane-lifecycle).

Pane IDs are scoped to the Welcome epoch. Within an epoch they increase monotonically and are never reused, so they also give creation order. A new epoch invalidates previous screen baselines and ownership. Clients must negotiate anew, obtain snapshots, and deliberately rebuild desired control state rather than replaying old mutations. Client UI state keyed by pane identity (epoch plus pane ID) does not survive a new epoch. The v1 operations carry no correlation ID or expected epoch; the session API requests carry a correlation ID (below). An expected epoch on every mutation remains part of public-contract maturation in the ADR.

## Pane subscriptions and updates

```json
{"Stream":{"type":"subscribe","panes":[1,2]}}
```

Subscribe replaces the complete subscription set. Updates contain an authoritative `subscribed` list, full metadata for current panes, leases, notifications, Vitals, and snapshots or patches for subscribed terminal screens. Metadata-only subscriptions use an empty list. The reference client prunes cached screens according to each received update's subscription list, safely processing older in-flight updates before a replacement set arrives.

A full pane snapshot establishes revision 1. A patch names the pane, base revision and resulting revision; it applies only to that exact pane baseline in the current server epoch. Geometry changes, explicit `resync` and backpressure can produce replacement snapshots. Snapshots contain cells with colors/attributes/widths, cursor and input modes; they do not contain native headers, tabs or footer cells. Alacritty remains the only emulator answering application terminal queries.

There are at most 64 subscribed panes and 100,000 aggregate subscribed screen cells per connection (ten panes before `stream-limits`); a server with `stream-limits` advertises both in Welcome `limits`, so clients should read them rather than hardcode them. The cell budget keeps a backpressured update, which carries every subscribed screen as a snapshot, under the 16 MiB record limit (a cell serializes to about 100–160 bytes); it does not limit grids, because the panes displayed on one terminal together cover at most that terminal (the largest native pane is 400 × 160 = 64,000 cells). Oversized subscriptions are rejected. A later size change exceeding an observer's aggregate budget disconnects that observer instead of retaining unbounded data. The server retains one in-flight packet and one replaceable aggregate update. The bridge has separate bounded queues in each direction and yields between decode batches. A permanently slow client can be disconnected and resynchronize on reconnect.

## Independent ownership and size

`claim {pane,takeover}` explicitly acquires one pane; `release {pane}` relinquishes it. `resize {pane,cols,rows}`, `input {pane,bytes}`, `paste {pane,bytes}` and `mouse {pane,event}` require the caller's lease. Input/paste are bounded to 64 KiB. Paste honors the pane's bracketed-paste mode; native mouse events currently support SGR encoding. An observer cannot write or resize a pane.

The optional `controller-theme` capability adds `theme {pane,foreground:[r,g,b],background:[r,g,b]}` with byte-valued RGB channels. It requires the caller's pane lease and sets that pane's OSC 10/11 query fallbacks. The browser reports its theme after claiming and when its appearance changes. Takeover or release drops the old lease theme, restoring the next controller's colors. Do not send this request to retained servers that omit the capability.

Client A can own pane 1 at 240 × 70 cells while client B owns pane 2 at 40 × 12 cells. Native pane sizes permit 2–400 columns and 1–160 rows. Each pane still has exactly one underlying PTY size. Observers of that same pane clip/scale locally and cannot impose a competing size.

Leases last until release, takeover or disconnect; release or disconnect returns the pane to the server's base size, set by `--size` (default 120 × 40). Stream clients issue explicit Claim messages, which a UI may send automatically on focus or input; background resize, mouse movement and focus-out should never steal a lease. The browser retains its last geometry and claims on blur or document hiding; actual foreground input or another explicit claim can still take over. Release is idempotent for an already-free pane. Workspace administration and explicit shutdown are same-user operations on the stream (`shutdown`, `diagnostics`); this is not isolation against hostile processes running as that user.

## State, notifications and Vitals

The `program-status` capability adds `terminal` observation evidence from pane-local OSC 7501 reports. Current dev clients must recognize this status-source value; the experimental stream remains v1 and is not a frozen compatibility contract. Terminal activity takes precedence over metadata polling without implying that a conversation ID is known.

Pane metadata carries identity, agent kind, session ID if known, status, activity times and status observation evidence. Evidence distinguishes process, lifecycle hook, live record, agent-server and transcript observations. `checked_at` records a check; optional `event_at` records an actual known event time. Reading an old transcript again does not invent a new lifecycle event. Unknown or stale evidence remains visible in the overview, and an idle observation does not become a completed turn automatically.

Updates include the latest 128 retained notifications and explicit `acknowledge {notification}`. This is a bounded snapshot of notification history, not exactly-once event delivery or a question/answer RPC. A bell and the typed notification it accompanies are one notification (see [bell merge](#notifications-host-status-and-server-lifecycle)). Existing seven-day retention and persisted unread state remain. No native device notification or push service is invoked.

Vitals are typed values from the existing in-process sampler, independent of footer styling: CPU, memory, swap and disk percentages, plus server-local date/time strings. Missing values stay unavailable. Sample timestamps, byte totals, mount labels, timezone-aware conversion and richer metrics are proposed additions; clients must not treat the current clock strings as timezone-independent timestamps.

## Reference client and inspection

The Rust `native::reference::State` validates welcome/epoch, membership, screen/cell bounds, leases and patch baselines before committing changes. Failed updates leave previous baselines intact. `stream-inspect` is a read-only one-shot reference client with an absolute operation deadline:

```sh
kelpserver stream-inspect --workspace NAME
kelpserver stream-inspect --workspace NAME 1 2
```

With no pane IDs it returns metadata only; explicit IDs add those panes' terminal cells to JSON stdout. It does not claim, resize, start a server or persist terminal contents. Automated tests use actual bridge subprocess pipes, disposable PTYs and typed reference state; they do not open a real SSH login or access a native device.

History and search are stream requests (`pane-history`, below). Search returns case-insensitive literal occurrences; queries must contain 1–256 bytes without control characters. Each match carries its source buffer, immutable context rows, and inclusive cell coordinates relative to that context, so ongoing output or alternate-screen transitions cannot move the target between search and reveal. Search does not change another client's viewport or input ownership. History pages use relative line indices and plain text; stable, styled native-history pagination is not implemented. Pane renaming is client UI. Device capability requests, MCP, native apps and final public-protocol compatibility guarantees remain future work described in ADR 0001.

### Hyperlink cells

The server's `hyperlinks` capability indicates support for an optional `hyperlink` URI string per cell. Missing fields mean no target; explicit targets and recognized plain HTTP(S) URLs are carried independently of visible text. Targets are bounded to 2,048 bytes, require a URI scheme and reject whitespace and control characters. Patches include target-only changes. Native clients open links through their own user interaction rather than requesting browser execution on the server. Older cell encodings remain decodable.

## Session API

The session API is step 1 of [ADR 0002](adr/0002-generic-session-server.md): the session-level operations a full TUI needs to run as a stream client with local rendering and local UI state. Server support for step 2 (the stream TUI's parity gaps) adds the [step 2 capabilities](#step-2-additions). The classification of every TUI capability, and what remains, is in [session-api.md](session-api.md). [Step 3](#step-3-generic-server-03) removed the composed protocol 58 and added `diagnostics`.

### Compatibility

The stream stays at version 1; nothing in v1 changed meaning. Every addition is either a new optional field or a new request or event type behind one of these Welcome capabilities:

| Capability | Requests | Events and fields |
| --- | --- | --- |
| `pane-lifecycle` | `session_events {enabled}`, `spawn`, `close` | `pane_created`, `pane_exited`, `pane_closed`, `spawned`, `done`, `Update.sessions` |
| `closed-sessions` | `closed_sessions`, `restore` | `closed_sessions`, `restored` |
| `pane-history` | `history`, `search` | `history`, `search` |
| `pane-scroll` | `scroll`, `reveal` | none |
| `notification-ack-all` | `acknowledge_all` | none |
| `host-status` | `host_status {enabled}` | `Update.host` |
| `server-lifecycle` | `shutdown` | `server_stopping`, Welcome `server_version` |
| `startup-policy` | none (server option `--startup-panes none`) | Welcome `startup_panes` (always `none` since 0.3) |
| `recovery-signal` | `dismiss_recovery` | Welcome `recovery_pending`, `done` |
| `spawn-shell` | `spawn` field `shell` | none |
| `unused-shell` | none | `PaneSession.unused_shell` |
| `stream-limits` | none | Welcome `limits` |
| `notification-run` | none | Welcome `notification_run` |
| `spawned-by` | none | `PaneSession.spawned_by` |
| `diagnostics` | `diagnostics` | `diagnostics` |
| `uploads` | `upload_begin`, `upload_chunk`, `upload_finish`, `upload_cancel` | `upload_started`, `upload_progress`, `upload_done`, `upload_failed`, `done`, Welcome `upload_limits` |
| `binary-screens` | `binary_screens {enabled}` | `Update.binary_screens`, and a binary screen record after each update ([binary-screens.md](binary-screens.md)) |
| `files` | `file_list`, `file_stat`, `file_read`, `file_write`, `file_mkdir`, `file_rename`, `file_trash`, `file_delete` | `file_list`, `file_stat`, `file_content`, `file_written`, `file_removed`, `file_error`, `done`, Welcome `file_limits` |
| `bell-merge` | none | `Notification.bell`; a bell accompanying a typed notification is merged into it ([Notifications](#notifications-host-status-and-server-lifecycle)) |

Rules that keep older peers working:

- A connection receives an event of a new type only after it opts in (`session_events`, `host_status`) or as the reply to a request it sent. Existing clients that reject unknown event types (KelpClient, KelpHub's bridge) therefore never see one. `Update.sessions` and `Update.host` are omitted unless enabled. Step 2 adds no event type; step 3 adds the `diagnostics` reply; `uploads` and `files` add only replies to their own requests. Binary screen records reach only a connection that sent `binary_screens {enabled: true}`, and every one directly follows an `update` with `binary_screens: true`.
- New fields on existing messages (`Welcome.server_version`, `Welcome.startup_panes`, `Welcome.recovery_pending`, `Welcome.limits`, `Welcome.notification_run`, `Welcome.upload_limits`, `Welcome.file_limits`, `Error.request`, `Update.sessions`, `Update.host`, `Update.binary_screens`, `PaneSession.unused_shell`, `PaneSession.spawned_by`, `Notification.bell`) are optional; decoders that ignore unknown fields are unaffected. Absent fields are omitted from the wire (`recovery_pending`, `binary_screens` and `bell` when false, `spawned_by` when unknown).
- `spawn` without a shell serializes exactly as before, so it stays valid for a server without `spawn-shell`; with `shell`, such a server rejects the request.
- A server without a capability disconnects a connection that sends an unknown request type. Check the Welcome capabilities before sending any request listed above.
- The SSH bridge relays every stream request after Hello, so the session API works through `kelpserver bridge` unchanged. It relays binary screen records to the client without parsing them.

In the Rust API, `Event::Welcome`, `Event::Error` and `Update` gained fields and `Request`/`Event` gained variants, so exhaustive matches and struct literals in consumers need updating when they move their pin. Step 2 adds fields to `Event::Welcome`, `Request::Spawn`, `PaneSession` and `ServerConfig`, and the `Request::DismissRecovery` variant; patterns with `..` are unaffected. Step 3 (0.3.0, a breaking release) removes `StartupPanes::Layout`, the composed `ClientMessage`/`ServerMessage` variants (only `Stream`, `Error` and `Diagnostics` remain) and the UI fields of `ServerConfig` (it keeps `socket`, `state_dir`, `shell`, `cwd`, `cols` and `rows`), and adds `Request::Diagnostics` and `Event::Diagnostics`. 0.3.0-beta.2 removes the client presentation left in step 3: the `native::web_relay` and `native::templates` modules, the vitals presentation functions, `history::RestoreChoice` and `Entry::restore_choices`, `upgrade::attach_matching` and `upgrade::matching_binary`, the crate-root `label`, the alias parameter of `tracking::tab_name` and `tracking::shell_tab_name`, and in `kelpclient-protocol` 0.3.0 the `PaneInfo` fields `parked`, `number` and `palette`, `Status::text`, `Status::marker`, `WORKING_FRAMES` and `StatusSource::label`. The `uploads` capability adds `Event::Welcome.upload_limits`, `ServerConfig.uploads` (`uploads::Settings`), the `lifecycle::Settings` fields `upload_dir` and `upload_retention_days`, the `Request::Upload{Begin,Chunk,Finish,Cancel}` and `Event::Upload{Started,Progress,Done,Failed}` variants and the `uploads` module. The `binary-screens` capability adds `Request::BinaryScreens`, `Update.binary_screens`, `protocol::read_frame`, `protocol::read_server_record` and `protocol::ServerRecord`, and in `kelpclient-protocol` the `screens` module and `Hash` on `Color`; `PaneSnapshot` and `PanePatch` moved to `kelpclient-protocol` and stay re-exported from `stream`. The `files` capability adds `Event::Welcome.file_limits`, `ServerConfig.files` (`files::Settings`), the `Request::File{List,Stat,Read,Write,Mkdir,Rename,Trash,Delete}` and `Event::File{List,Stat,Content,Written,Removed,Error}` variants and the `files` module. The `bell-merge` capability adds the `Notification.bell` field (struct literals need it) and `activity::BELL_PAIR_WINDOW`. The file root (no new capability: `file_limits.root` and the `outside_root` code) adds `files::Limits.root` (struct literals need it, and `Limits` is no longer `Copy`), `files::Settings.root`, `lifecycle::Settings.files_root`, `files::Code::OutsideRoot`, `files::Root` and `files::no_root`; `files::execute`, `downloads::start` and `uploads::Uploads::begin` take the root as a new argument. The closed-session conversation list adds `history::Entry.sessions` (struct literals need it), `history::Session`, `history::SESSION_LIMIT`, `history::LAST_SEEN_STEP` and `History::observe_at`; the `closed_sessions` records gain the additive `sessions` field.

### Correlation and errors

Requests that have a reply carry a client-chosen `request` string of 1–128 ASCII letters, digits, `-` or `_`. The reply echoes it, and so does `{"type":"error","message":…,"request":…}` when such a request fails. File requests (`files`) fail with a structured `{"type":"file_error","request":…,"error":{"code":…}}` instead; see [Files](#files-files). A malformed identifier fails with an error without `request`. Errors of v1 requests still have no `request`. Fire-and-forget requests (`scroll`, `reveal`, `acknowledge_all`, the opt-ins) have no identifier and no reply, like `resize` or `acknowledge`.

### Pane lifecycle (`pane-lifecycle`)

```json
{"Stream":{"type":"session_events","enabled":true}}
```

After the opt-in, each update carries `sessions`, one `PaneSession` per live pane in ID order: `pane`, `created_at` (Unix seconds), `launch_cwd`, `shell`, `argv` (the `spawn` arguments, otherwise empty), `startup` (the command line typed into the shell at startup, if any) and `terminal_title` (the application's latest OSC title, unmodified). `Update.panes` keeps carrying the observations: current cwd, process, agent kind and session, status and evidence, activity times and shell command.

Lifecycle events follow, for every pane whatever created or closed it:

- `{"type":"pane_created","session":{…}}`;
- `{"type":"pane_exited","pane":3,"at":…,"code":0,"signal":null}`, when the process exits, always followed by `pane_closed`;
- `{"type":"pane_closed","pane":3,"at":…,"reason":"exited"|"closed","record":"<key>"|null}`, where `record` names the closed-session record created for the pane, if any.

Events are reliable messages. A replaceable update that was built before an event and is still pending is discarded with that connection's screen baselines, so the next update (a full one) already reflects the event. Updates that were already on the wire may precede it.

```json
{"Stream":{"type":"spawn","request":"r1","cwd":"/home/me/src","argv":["htop"]}}
{"Stream":{"type":"close","request":"r2","pane":3}}
```

`spawn` takes an optional absolute `cwd` and an `argv` list; `cwd` defaults to the server's launch directory and an empty `argv` opens an interactive shell. Arguments are quoted as data and run before `exec $SHELL -i`. The server removes agent and multiplexer session markers from the environment; clients cannot pass variables. A retry with the same `request`, directory and arguments returns the same pane (bounded 512-entry cache per epoch). The reply is `spawned {request, pane}`, after `pane_created`.

`close` terminates the pane's process group and removes the pane; the reply is `done {request}`, after `pane_closed`. It is refused while another connection holds the pane's lease. Asking the user for confirmation is client UI. Closing the last pane first creates a replacement shell, so a running server never drops to zero panes through `close`; the last pane's own exit stops the server.

### Closed sessions (`closed-sessions`)

```json
{"Stream":{"type":"closed_sessions","request":"r3"}}
{"Stream":{"type":"restore","request":"r4","record":"<key>","rerun":false}}
```

`closed_sessions` replies `closed_sessions {request, records}` once the existing background collection finishes (at most 256 records, newest first, from this workspace and sibling workspaces of the same user, seven-day retention). A record has `key`, `workspace`, `pane` (its old ID), `cwd`, `name`, `kind`, `session_id`, `argv`, `closed_at`, `reason`, `sessions`, and the derived `resume` (the agent resume command) and `rerun` (the program command line). `session_id`, `kind` and `resume` describe the latest conversation only. `restore` addresses a record of the latest list by `key`; it resumes an agent conversation if known, reruns a program only with `rerun: true`, and otherwise opens a shell in the record's folder. The reply is `restored {request, record, pane, existing}`; `existing: true` means the conversation was already open in `pane` and nothing was started. A restored record is consumed. A conversation running in another workspace is refused.

`sessions` lists every agent conversation the pane hosted, oldest first, so a pane that ran a second conversation (after `/clear` or a second `claude` run) still offers the first: `[{"kind":"claude","id":"…","cwd":"/project","first_seen":1764000000,"last_seen":1764000600}]`. A conversation is appended when the pane reports a valid session ID different from the last recorded one; seeing the same ID again only advances `last_seen`, in steps of at least 60 seconds. `cwd` is the pane's folder when the conversation was first seen, which is where a resume must run. Times are epoch seconds. At most 32 conversations are kept per record, dropping the oldest. The field is additive: it is an empty list for records written before it existed and for programs, and `restore` still resumes the latest conversation; a client that wants an earlier one runs its resume command (for example `claude --resume <id>`) in that conversation's `cwd` itself.

### History, search and scrolling (`pane-history`, `pane-scroll`)

```json
{"Stream":{"type":"history","request":"r5","pane":1,"start":-500,"limit":200}}
{"Stream":{"type":"search","request":"r6","panes":[],"query":"error","limit":64}}
```

`history` returns `history {request, pane, lines}`: plain-text rows with relative line indices (at most 200). `search` returns `search {request, results, truncated}`; each result is `{pane, hits, truncated}` with the immutable-context matches described in [Reference client and inspection](#reference-client-and-inspection). An empty `panes` list searches every pane in ID (creation) order. One reply holds at most `limit` (1–64) matches and 4 MiB of context; `truncated` reports that a limit stopped the search. Neither request needs a lease or changes any viewport.

`scroll {pane, lines}` and `reveal {pane, line}` require the pane's lease and move its emulator viewport, which every subscriber of that pane sees in its snapshots. `reveal` takes a primary-history line (a `history` index, or a match's `first_line + start.y`) and fails while the application uses the alternate screen. They exist until styled history lets a client render scrollback itself.

### Notifications, host status and server lifecycle

`acknowledge_all` marks every retained notification read (`notification-ack-all`).

**Bell merge (`bell-merge`, [ADR 0003](../../../docs/adr/0003-kelpcore-shared-repository.md) section 3).** A terminal bell is recorded as a notification with status `unknown` and a title starting `Bell · `. When a bell and a typed agent notification (`input_needed`, `turn_finished`, `failed` or `exited`) of the same pane and process (same `pane`, `origin.run` and `origin.pid`) are at most 2 seconds apart (`at`), in either order, the server keeps one notification: the typed one, with `bell: true`. A bell first is replaced: its record is removed and the typed notification is recorded with a new ID, so a client sees a new event (it alerts as the typed event). A typed notification first absorbs the bell: the same record gains `bell: true` and keeps its ID and read state, and no bell record appears. A bell with no typed notification in the window stays a bell record, as before; a bell of another pane or process never merges. Merging happens when the second event arrives, without delaying either, so the bell is not held back waiting for a companion. Acknowledging the merged record clears the event; an `acknowledge` of a bell ID that was replaced after the client saw it is applied to the record it was merged into (this mapping lives in memory for the server's run). Older clients keep working: they see fewer bell records and ignore `bell`; their own pairing of a bell with a typed notification finds nothing to pair. The bell de-duplication (a pane's bells less than 2 seconds apart record once) is unchanged.

`{"type":"host_status","enabled":true}` adds `host` to that connection's updates (`host-status`): per-CPU usage, memory and swap bytes, load, uptime, task counts, network and disk I/O rates, mounted disks and the process table, sampled on demand about once a second while at least one connection wants it. `ready` is false until the first sample. Views, sorting and formatting are client UI.

`shutdown {request}` replies `done` and stops the server and its panes (`server-lifecycle`). A connection that enabled `session_events` receives `server_stopping {reason: "shutdown"|"last_pane_exited"}` before its socket closes; the server gives queued messages up to 200 ms. There is no upgrade event: a live server is never upgraded in place. A client detects a different server by its epoch and `server_version`, and its features by the capabilities.

### Step 2 additions

Server support for ADR 0002 step 2, so a stream TUI reaches parity with the composed one. All of it is additive under the capabilities in the table above.

```json
{"Stream":{"type":"welcome","version":1,"client":4,"epoch":"…","capabilities":[…],"server_version":"0.3.0-beta.1","startup_panes":"none","recovery_pending":true,"limits":{"subscriptions":64,"subscribed_cells":100000},"notification_run":"…"}}
```

**Startup policy (`startup-policy`).** In step 2, `kelpserver server` and `kelpserver start` accepted `--startup-panes layout|none`, where `layout` kept the composed UI's startup tabs. Since 0.3 the server always starts with zero panes and the client that started it creates its layout with `spawn`; `--startup-panes none` is still accepted and `layout` fails with an error. The server works without panes: only the last pane's exit (not an empty pane list) stops it. It never reads or writes `tab-layout.json` or client keys in `workspace.json`. Welcome `startup_panes` is always `none` from a 0.3 server. Clients still decode other values: a running 0.2 server started with `--startup-panes layout` sends `layout` (`StartupPanes::Layout`, 0.2 servers only), and any other value decodes as `StartupPanes::Unknown`, so the Welcome of an older or newer server never fails to decode (since 0.3.0-beta.2). Closing the last pane with `close` still creates a replacement shell first.

**Recovery signal (`recovery-signal`).** Welcome `recovery_pending: true` means the previous server run ended without an orderly shutdown (its run marker was still present) and no client has shown or dismissed recovery yet. The candidates are the closed-session records, including those marked `Previous run`: list them with `closed_sessions`. `{"type":"dismiss_recovery","request":"r7"}` replies `done` and clears the signal for every client. The signal never becomes true during a run, so Welcome is its only carrier.

**Spawn with a shell (`spawn-shell`).** `spawn` takes an optional `shell`, an absolute path to an existing executable regular file (the same check `start` applies to the workspace shell). It runs instead of the server's shell, including after the arguments (`exec <shell> -i`), and becomes the pane's `PaneSession.shell`. Another value fails with an error naming the shell. The retry identity is the request, directory, arguments and shell.

```json
{"Stream":{"type":"spawn","request":"r8","cwd":"/home/me","argv":[],"shell":"/usr/bin/fish"}}
```

**Unused shell (`unused-shell`).** `PaneSession.unused_shell` is an observation: the pane's shell has no child process and no other process on its terminal, has seen no input for 200 ms with no input pending, and two separate idle observations agree. It is sampled every 500 ms (process table) and 100 ms (idle observations) while a stream connection has `session_events` enabled, and otherwise stays false. A lease does not hide it, because a stream TUI leases the panes it shows; clients combine it with `Update.leases` if they need to.

**Limits (`stream-limits`).** Welcome `limits {subscriptions, subscribed_cells}` advertises the subscription bounds described in [Pane subscriptions and updates](#pane-subscriptions-and-updates): 64 panes and 100,000 cells. Servers without the capability allow ten panes.

**Notification run (`notification-run`).** Notifications carry `origin {run, pid}`. Welcome `notification_run` is the current run: a notification refers to a live pane when `origin.run` equals it, `origin.pid` equals the pane's `pid` and `pane` equals the pane ID. Older notifications, or those from a previous run, no longer refer to a live pane even if a pane has the same ID. This lets a client jump to a notification's pane and badge panes without guessing the run from the notification list.

**Spawned by (`spawned-by`).** `PaneSession.spawned_by` is the Welcome `client` ID of the stream connection whose `spawn` or `restore` created the pane, so a client can recognize the panes it created (for example for its layout, or its scratch terminal). Client IDs are per connection: a client that reconnects gets a new ID, so it must remember its previous IDs for the epoch to recognize its earlier panes. It is absent for panes the server created itself (the replacement shell after closing the last pane). It is not persisted: a new epoch starts without it, and a client ID is not an identity across connections.

### Step 3: generic server (0.3)

KelpServer 0.3.0 (ADR 0002 step 3) removes the composed protocol 58 and the server-side UI. The stream stays at version 1 and every capability above keeps its meaning; the only stream addition is `diagnostics`.

```json
{"Stream":{"type":"diagnostics","request":"r9"}}
{"Stream":{"type":"diagnostics","request":"r9","report":{"version":"0.3.0-beta.1","protocol":1,"cols":120,"rows":40,"panes":2,"clients":1,"uptime_seconds":61,"pty_bytes":40960}}}
```

**Diagnostics (`diagnostics`).** `diagnostics {request}` replies with content-free counters: server version, stream version, base size, pane and connection counts, uptime and PTY bytes read. No terminal cells, prompts, paths or session IDs. The same report answers the one-shot `"Diagnostics"` record on a connection without Hello, for administration tools that do not open a stream session.

**Startup and base size.** A server starts with zero panes. `--size` sets the base size of panes without a lease (2–400 columns, 1–160 rows). Nothing reads or writes client UI files: tab layouts, templates, sidebar preferences and UI settings in `workspace.json` belong to the client. `kelpserver start` stores only `shell` and `cwd` (and, when given, the [upload](#uploads-uploads) options `upload_dir` and `upload_retention_days` and the [file root](#files-files) `files_root`) in `workspace.json` and preserves its other keys.

**Administration.** `kelpserver start`, `status`, `stop` and `ctl list|diagnostics|shutdown` use the stream (`status` and `ctl list` read the first update; `stop` and `ctl shutdown` send `shutdown`). `stop` falls back to the legacy shutdown command only for a running server that lacks `server-lifecycle`. The upgrade identity file `server.identity.json` records the stream version in `protocol` for 0.3 servers (58 means a composed-protocol server), and `kelpserver status` prints it as "stream protocol 1" or "local protocol 58". A running 0.2 server (protocol 58) also speaks stream version 1, so a 0.3 stream client keeps using it within its advertised capabilities; an old composed client connecting to a 0.3 server gets the `Error` described under [Framing and negotiation](#framing-and-negotiation). `upgrade::identify` authenticates the running server and returns this identity; choosing a compatible client binary (formerly `upgrade::attach_matching`) belongs to the client since 0.3.0-beta.2.

**Pane metadata without client UI fields (0.3.0-beta.2).** `PaneInfo` in `Update.panes` (and `ctl list`) carries `id`, `pid`, `cwd`, `title`, `status`, `observation`, `session_id`, `dead`, `kind`, `last_activity`, `status_since` and the optional `command`. The composed-UI fields `parked`, `number` and `palette` are gone (`kelpclient-protocol` 0.3.0): tab numbers, parking and colours are client state keyed by pane ID, and creation order is pane ID order within an epoch. The stream version stays 1 because every other field keeps its meaning, but a client whose decoder requires `number` or `palette` (as `kelpclient-protocol` 0.2.0 does, since those fields have no serde default) cannot decode a 0.3.0-beta.2 update; update the client's protocol pin with the server. A newer client ignores the extra fields an older server still sends.

### Uploads (`uploads`)

A client that lets the user drop files onto a terminal sends them to the server with these requests; the files land on the host where the server and its shells run (the user's machine for a local server, the workspace pod for a containerized one, which needs no SSH daemon). The client owns the drop UI and, once the batch is done, pastes the shell-quoted paths into the pane itself (for example with `paste`). Uploads are not tied to a pane and need no lease. Decision 151a, [ADR 0002](adr/0002-generic-session-server.md#uploads-capability-2026-11-21-decision-151a).

**Authorization.** Any stream connection may upload. This is the same trust as typing into a shell, which any stream connection may do after claiming a pane: a client able to send input can already write any file the server's user can write. Clients that relay other users' input (a multi-user web front end) must apply their own policy before relaying upload requests.

**Limits.** Welcome carries `upload_limits`; read them rather than hardcoding them:

```json
{"Stream":{"type":"welcome",…,"capabilities":[…,"uploads"],"upload_limits":{"max_files":64,"max_file_bytes":104857600,"max_batch_bytes":1073741824,"max_chunk_bytes":262144,"max_chunks_in_flight":8,"max_batches":4}}}
```

- `max_files`: files per batch (at least one).
- `max_file_bytes`: largest declared file (100 MiB); `max_batch_bytes`: sum of a batch's sizes (1 GiB).
- `max_chunk_bytes`: largest decoded chunk (256 KiB). Its base64 record is about 342 KiB, far below the 16 MiB record limit, so a chunk never delays pane updates for long.
- `max_chunks_in_flight`: the flow-control window (8), below.
- `max_batches`: unfinished batches per connection (4). The server also allows at most 16 unfinished batches in total.

**Begin.** Declare every file with its exact size; `dest` is optional.

```json
{"Stream":{"type":"upload_begin","request":"u1","files":[{"name":"photo.png","size":600000},{"name":"notes.txt","size":13}],"dest":"/home/me/project"}}
{"Stream":{"type":"upload_started","request":"u1","batch":1,"dir":"/home/me/.cache/kelpserver/uploads/1795300000-3f9c0a1b2c3d4e5f"}}
```

The server creates a new private (0700) directory for the batch, with a unique name (`<unix seconds>-<random hex>`), created and opened without following a symlink. Without `dest` it is created in the server's upload directory: `$XDG_CACHE_HOME/kelpserver/uploads` or `~/.cache/kelpserver/uploads` (created if needed and made 0700), or the directory set with `kelpserver server|start --upload-dir PATH` or `upload_dir` in the configuration file. `dest` must be an absolute path without `.` or `..` components naming an existing directory owned by the server's user inside the [file root](#files-files) (Welcome `file_limits.root`), resolved beneath it like a file operation's path; a symlink at its final component is refused, and so is one on the way that leads outside the root. The upload directory itself is server configuration and need not lie inside the file root. `batch` is a number unique within the server run, addressed only by the connection that began it. Refusals (limits, sizes, destination, too many batches, not enough free space for the declared total) are `error {request}` replies and create nothing.

File names are sanitized: only the base name is kept (after the last `/` or `\`); NUL, control and bidirectional formatting characters are removed; surrounding whitespace and leading dots are removed, so a name is never `.`, `..` or hidden; an empty result becomes `upload`; names are bounded to 200 bytes, keeping an extension of up to 32 bytes. Duplicates within a batch get `-2`, `-3`, … before the extension (`a.txt`, `a-2.txt`). The final names are those of the `upload_done` paths.

**Chunks and flow control.** Send each file's bytes as base64 (standard alphabet, padded) in order: files in request order (zero-byte files need no chunk), and each file's chunks at strictly sequential offsets starting at 0. Each chunk holds 1 to `max_chunk_bytes` decoded bytes.

```json
{"Stream":{"type":"upload_chunk","batch":1,"file_index":0,"offset":0,"data":"iVBORw0KGgo…"}}
{"Stream":{"type":"upload_progress","batch":1,"file_index":0,"received":262144}}
```

The server acknowledges every accepted chunk with exactly one `upload_progress`, in order, carrying the bytes of that file received so far. **Flow-control rule:** a client sends at most `max_chunks_in_flight` chunks that have not yet been acknowledged; after each `upload_progress` it may send one more. The server queues at most 32 reliable messages per connection, so a client that ignores the window and stops reading can be disconnected as a slow peer (which removes its unfinished batches). Chunks carry no `request` identifier.

A chunk that is out of order, at the wrong offset, not valid base64, empty or too large, beyond its file's declared size, or for a file index that does not exist fails the batch: the server removes the batch with its files and replies

```json
{"Stream":{"type":"upload_failed","batch":1,"message":"upload exceeds the declared size of file 0"}}
```

Chunks of that batch still in flight when it failed or was cancelled are dropped without a reply. A chunk for a batch this connection never had (or another connection's batch) gets `upload_failed` and changes nothing.

**Finish.** When every file has its declared size:

```json
{"Stream":{"type":"upload_finish","request":"u2","batch":1}}
{"Stream":{"type":"upload_done","request":"u2","batch":1,"paths":["/home/me/project/1795300000-3f9c0a1b2c3d4e5f/photo.png","/home/me/project/1795300000-3f9c0a1b2c3d4e5f/notes.txt"]}}
```

`paths` are absolute, in request order. Files are written to private temporary names inside the batch directory (`.kelpserver-upload-<index>.part`, mode 0600, created with `O_EXCL` and `O_NOFOLLOW`), synced when complete, and renamed to their final names on finish without replacing anything (`RENAME_NOREPLACE`), then the directory is synced. Published files keep mode 0600. A finish with a file still missing bytes is an `error {request}` ("upload is incomplete") and removes the batch; any other failure of a finish also removes it. Quote the paths for the shell before pasting them (for example `'…'` with `'` written as `'\''`).

**Cancel.**

```json
{"Stream":{"type":"upload_cancel","request":"u3","batch":1}}
{"Stream":{"type":"done","request":"u3"}}
```

removes the batch and its files. Cancelling a batch of this connection that already failed or was cancelled also replies `done`; an unknown batch is an `error {request}`.

**Connection scope.** A batch belongs to its connection. When the connection ends (disconnect, slow-peer disconnect, server shutdown), its unfinished batches are removed with their partial files. A completed batch stays on disk.

**Retention.** The server records each batch it creates in `uploads.json` in its state directory: the batch directory's path, device and inode, creation time and, once finished, completion time. At startup and then hourly it deletes completed batches older than the retention (default 7 days; `--upload-retention-days N` or `upload_retention_days` in the configuration file; `0` keeps them forever) and unfinished batches left by a previous run. Only registered directories whose device and inode still match, owned by the server's user, are deleted; a registered path that is now missing, a symlink or another directory is forgotten without deleting anything. Deletion works through directory descriptors and never follows symlinks (a symlink inside a batch is removed, not its target). Files that are not registered batches, including anything else in the upload directory or in a `dest`, are never deleted. `kelpserver start --upload-dir` and `--upload-retention-days` store `upload_dir` and `upload_retention_days` in `workspace.json`; they take effect when the server next starts.

### Files (`files`)

File browsers and editors in clients list, read and change files on the host where the server and its shells run (the user's machine for a local server, the workspace pod for a containerized one). The client owns the browser and editor UI. File requests are not tied to a pane and need no lease. ADR 0003 section 3 ([KelpCore ADR 0003](../../../docs/adr/0003-kelpcore-shared-repository.md)).

**Authorization and threat model.** Any stream connection may use them, with the same trust as uploads and as typing into a shell: a connection that can send input to a pane can already read and change every file the server's user can. The operations run with the server's own credentials and never elevate; the security boundary stays the authenticated stream connection (the private socket, or SSH for a bridge). They are confined to the file root (below), which bounds what the file browser shows and changes, not what the user can reach: the same connection can type any command into a shell. What they also guarantee is that they never act through a symlink at the final component of a change, never follow a symlink while deleting a tree, write atomically, and stay within bounded resources. Clients that relay other users' requests (a multi-user web front end) must apply their own policy before relaying file requests.

**Limits.** Welcome carries `file_limits`; read them rather than hardcoding them:

```json
{"Stream":{"type":"welcome",…,"capabilities":[…,"files"],"file_limits":{"max_read_bytes":8388608,"max_write_bytes":8388608,"max_list_entries":5000,"max_paths":256,"max_in_flight":4,"max_path_bytes":4096,"max_depth":64,"root":"/home/me","roots":[{"path":"/home/me","label":"Home"},{"path":"/mnt/data","label":"data"}]}}}
```

- `max_read_bytes`: largest file `file_read` returns (8 MiB). Its base64 form fits one 16 MiB record.
- `max_write_bytes`: largest decoded `file_write` content (8 MiB).
- `max_list_entries`: entries in one `file_list` reply (5,000; a listing scans at most 200,000 directory entries).
- `max_paths`: paths in one `file_trash` or `file_delete` (256).
- `max_in_flight`: file requests of one connection running at once (4). The server runs at most 16 in total, of which at most 4 `file_delete`. A request over a bound gets `file_error` `busy` at once; retry after a reply arrives. A request counts until its reply has entered the connection's output queue, so a connection that does not read its replies keeps its slots.
- `max_path_bytes`: longest path (4,096 bytes).
- `max_depth`: deepest directory nesting `file_delete` removes (64).
- `root`: the file root (below), canonical. Absent from servers before it, and from a server that could not resolve its root at startup, which then refuses every file request with `outside_root`.
- `roots`: every file root a path may lie in, as `{path, label}`, the primary root (`root`) first, then the extra roots (below) in configured order. `path` is canonical; `label` is a short name for a file browser: `Home` for the primary root when it is the server user's home directory, otherwise the folder's name, or its full path when that name is empty (`/`) or shared by another root. Absent from servers before extra roots, and whenever `root` is absent; a client without it uses `root` alone.

**File root.** Every path of every file request (both ends of a rename, every path of a trash, delete or download, an upload `dest`) must lie inside the file root: the server user's home directory (`HOME`, or the password database's entry when `HOME` is unset or relative), or the folder set with `kelpserver server|start --files-root PATH` or `files_root` in the configuration file. The server resolves it once at startup (symlinks in its own path resolved) and advertises it as `file_limits.root`; a client starts browsing there and offers nothing outside it. A path that lies outside after normalisation (`/`, `/etc/passwd`, `/home/meow` next to `/home/me`) fails with `outside_root`, and a batch with one such path is refused whole. Paths are then resolved beneath the root: with `openat2(RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS)` where the kernel has it, otherwise (and for an absolute symlink, which `RESOLVE_BENEATH` refuses even when it points inside) by opening one component at a time with `O_NOFOLLOW` and resolving each symlink in the server (at most 40), an absolute target only when it names a path inside the root and `..` never above it. A symlink that leads outside fails with `outside_root` when something is opened through it; it is still listed and stat'ed as a link with its `target`. Mount points inside the root stay usable. The root itself can be listed and stat'ed; writing, creating, renaming, moving, trashing, deleting or downloading it is refused with `invalid_request` (a symlink to it is only a link). When the root is reached through a symlinked spelling of it (a `HOME` of `/home/me` that is a link to `/data/home/me`), that spelling is accepted and replies use the canonical one; `~` expands to the canonical home directory.

**Extra roots.** Further folders outside the primary root, such as a data drive at `/mnt/data`, can be opened to file requests with `kelpserver server|start --files-extra-root PATH` (repeatable) or `files_extra_roots` (an array of paths) in the configuration file; as for `--files-root`, the command line wins, and its list replaces the file's whole list. Each one is made absolute and resolved at startup like the primary root. One that is missing, not a directory, inside the primary root, containing the primary root, repeating another extra root or inside another extra root (the outer one is kept) is skipped with a warning in the server's log, and the server still starts; at most 64 are used. Every rule of the primary root applies to each extra root on its own: a path lies in at most one root (the longest matching root or configured spelling decides) and is resolved beneath that root only, so a symlink in an extra root that points into the primary root, into another extra root or anywhere else outside fails with `outside_root`, and the same goes for a symlink in the primary root that points into an extra root. An extra root itself can be listed but not changed (`invalid_request`). `file_rename` whose two ends lie in different roots fails with `cross_root` and changes nothing; nothing is copied between roots. `file_trash` never moves an entry into another root either: an entry may go to a trash inside its own root, and an entry of the primary root also to a trash outside every root (the default home trash when the primary root is not the home directory); otherwise that path fails with `cross_root` and stays in place, and the client offers a permanent delete. With the default home trash inside the home directory as primary root, entries of extra roots are therefore never trashed. An entry on another file system than the trash still fails with `cross_device`. Each root is also held to the directory it was at startup (device and inode): if a root, or a folder above it, is later replaced (by a symlink to another folder, say), every request in that root fails with `outside_root` until the server restarts.

**Paths.** Every path is a string: absolute (`/home/me/notes.txt`), or `~` / `~/…`, relative to the server user's home directory. The server normalises it lexically: repeated `/`, `.` components and a trailing `/` are dropped. `..` components, `~user`, relative paths, NUL and paths over `max_path_bytes` are refused (`invalid_path`). Replies carry the normalised absolute path, so a client learns what `~` expanded to. Names that are not UTF-8 cannot be addressed and are left out of listings (counted in `skipped`).

**Symlinks.** Intermediate components are resolved like a shell does (a symlinked parent directory works), within the file root. Reads and listings follow a symlink at the final component too, within the root; `file_stat` and listing entries describe the symlink itself and report its `target` as text, without following it. Every change acts on the final component through its parent directory's descriptor and never follows a symlink there: `file_write` to a symlink fails with `symlink`; `file_rename`, `file_trash` and `file_delete` move or remove the link itself, never its target; recursive deletion opens each directory with `O_NOFOLLOW`, so a symlink inside a tree is unlinked, not followed.

**Entries.** `file_list` and `file_stat` describe entries as:

```json
{"name":"run.sh","kind":"file","size":120,"modified":1795300000,"mode":493}
{"name":"current","kind":"symlink","size":7,"modified":1795300000,"mode":511,"target":"release"}
```

`kind` is `file`, `dir`, `symlink` or `other` (FIFO, socket, device). `modified` is in Unix seconds. `mode` holds the permission bits including set-id and sticky (`st_mode & 0o7777`); the entry is executable when `mode & 0o111` is not zero. `target` (symlinks only) is the link text, at most 4,096 bytes, with invalid UTF-8 replaced.

**List.**

```json
{"Stream":{"type":"file_list","request":"f1","path":"~/project"}}
{"Stream":{"type":"file_list","request":"f1","path":"/home/me/project","entries":[{"name":"src","kind":"dir",…},{"name":"README.md","kind":"file",…}],"truncated":false,"skipped":0}}
```

Directories come first, then every other entry, each group in byte order of the name; a symlink to a directory sorts with the files. A larger directory returns its first `max_list_entries` entries in this order: the server selects them while scanning, holding and examining only those. `truncated` is true when entries were left out (over `max_list_entries`, the scan bound, or the record budget); past the scan bound, the entries are the first of those scanned, in the directory's own order.

**Stat.** `{"type":"file_stat","request":"f2","path":"/home/me/project/run.sh"}` replies `{"type":"file_stat","request":"f2","path":"/home/me/project/run.sh","entry":{…}}`. The root has its own name (`/` when the root is `/`).

**Read.**

```json
{"Stream":{"type":"file_read","request":"f3","path":"/home/me/project/README.md"}}
{"Stream":{"type":"file_content","request":"f3","path":"/home/me/project/README.md","size":12,"modified":1795300000,"mode":420,"hash":"e3b0…","binary":false,"encoding":"utf8","content":"# Project\n\n"}}
```

Only regular files are read (`is_a_directory`, `not_a_file` otherwise); the type is checked before the file is opened, so reading a FIFO or device path never opens it. The whole file is returned or nothing: `max_bytes` (optional, at most `max_read_bytes`, which is also the default) refuses a larger file with `too_large`, whose `size` gives the file's size, so a client can offer a preview with a smaller bound or decline. `hash` is the lowercase hexadecimal SHA-256 of the content. `binary` is true when the bytes contain NUL or are not valid UTF-8. `encoding` says how `content` is carried: `utf8` (the text itself) for text, `base64` (standard alphabet, padded) for binary content, and also for text whose JSON escaping would not fit the record (text made mostly of control characters).

**Write.**

```json
{"Stream":{"type":"file_write","request":"f4","path":"/home/me/project/README.md","content":"# Project\n","base_hash":"e3b0…"}}
{"Stream":{"type":"file_written","request":"f4","path":"/home/me/project/README.md","hash":"5f1b…","size":10,"modified":1795300100,"created":false}}
```

`encoding` (`utf8` by default, or `base64`) says how `content` is carried; use `base64` for binary content, or for text whose JSON escaping would exceed the 16 MiB record. Without `create`, the file must exist and be a regular file owned by the server's user, not a symlink, and writable by its owner: a read-only file is refused with `permission_denied`, as a shell's `>` refuses it (unless the server runs as root). It is replaced atomically: the content goes to a new temporary file in the same directory (`.kelpserver-<random>.tmp`, created with `O_EXCL` and `O_NOFOLLOW`), which takes the existing file's group (when the user may set it) and permission bits (without set-id bits), is synced, and is renamed over the target, then the directory is synced. Readers see the old or the new content, never a mix. A replacement is a new inode, so hard links to the old file keep the old content, the owner is the server's user, and extended attributes and ACLs are not carried over. With `base_hash`, the write happens only if the current content's SHA-256 equals it (compared case-insensitively); otherwise nothing is written and the reply is `file_error` `conflict` carrying `current_hash`. A file now larger than `max_read_bytes` is a `conflict` without hashing it (no client can have read it whole), with `size` and without `current_hash`. Just before the rename, the file's device, inode, size, modification and change times are compared with those seen when it was hashed; any difference (the file replaced, or changed in place) is a `conflict` without `current_hash`, and nothing is written. Only a change landing between that comparison and the rename (microseconds) is overwritten. Without `base_hash` the write is unconditional, except that a file replaced by another inode meanwhile is refused as `conflict`.

With `"create":true` the file must not exist (`exists` otherwise, also for a dangling symlink): the temporary file gets mode 0666 less the server's umask, like a file a shell creates, and is renamed into place without replacing anything (`RENAME_NOREPLACE`; on a file system without it, a check followed by a rename). `base_hash` cannot be combined with `create` (`invalid_request`). The parent directory must exist.

**Create a folder.** `{"type":"file_mkdir","request":"f5","path":"/home/me/project/new"}` replies `done`. One level only: a missing parent is `not_found`, an existing entry `exists`. The mode is 0777 less the umask.

**Rename and move.**

```json
{"Stream":{"type":"file_rename","request":"f6","from":"/home/me/a.txt","to":"/home/me/docs/a.txt"}}
{"Stream":{"type":"done","request":"f6"}}
```

Moves an entry (a directory with its contents, a symlink as a link) within one file system; across file systems it fails with `cross_device` and nothing changes (a client can copy and delete itself). An existing `to` is never replaced unless `"overwrite":true`, which uses the system's rename rules (a file or link replaces a file, a directory replaces an empty directory). The refusal is `exists` with `path` set to `to`.

**Move to the trash.**

```json
{"Stream":{"type":"file_trash","request":"f7","paths":["/home/me/old.txt","/home/me/build"]}}
{"Stream":{"type":"file_removed","request":"f7","removed":[{"path":"/home/me/old.txt","trashed_as":"/home/me/.local/share/Trash/files/old.txt"}],"failed":[{"path":"/home/me/build","error":{"code":"permission_denied","message":"…","path":"/home/me/build"}}]}}
```

Entries go to the [freedesktop.org home trash](https://specifications.freedesktop.org/trash-spec/latest/): `$XDG_DATA_HOME/Trash` (when absolute), otherwise `~/.local/share/Trash`, whose `files/` and `info/` directories are created private (0700) when missing. For each path the server first reserves `info/<name>.trashinfo` with `O_EXCL` and writes and syncs

```
[Trash Info]
Path=/home/me/my%20notes.txt
DeletionDate=2026-11-23T10:04:05
```

(`Path` is the normalised absolute path, percent-encoded as in a file URL: every byte except `A–Z a–z 0–9 - . _ ~ /`; `DeletionDate` is the server's local time), and syncs `info/`, then renames the entry to `files/<name>` without replacing anything and syncs `files/` and the entry's former directory. A name already taken in `files/` or `info/` gets a number before its extension: `a.txt`, `a.2.txt`, `a.3.txt`, …; names are shortened so the `.trashinfo` name stays within 255 bytes. If the move fails, the info file is removed again. Desktop file managers show and restore these entries. Only the home trash is used: an entry on another file system than the trash fails with `cross_device` (per-volume `.Trash-$uid` directories are not implemented; delete such entries instead). The `directorysizes` cache is not written; the specification makes it optional.

**Delete permanently.** `{"type":"file_delete","request":"f8","paths":[…]}` removes each entry at once: a file or symlink is unlinked, a directory is removed with its whole tree, never following a symlink (a symlink to a directory removes the link only; a symlink inside the tree is unlinked, its target is untouched). A tree nested deeper than `max_depth` fails with `too_large`, and like `rm --one-file-system` a deletion never enters another file system: a mount point inside the tree fails with `cross_device`. A directory that keeps listing entries that cannot be removed fails with `io` instead of being retried forever. Whatever was removed before such a failure stays removed, as with any failure part way through a tree (for example a directory without write permission). The reply is `file_removed` without `trashed_as`.

Neither removes a file root itself, primary or extra (`invalid_request`), nor `/` or the server user's home directory under any path that names that directory, such as a bind mount (`invalid_request`); a symlink to one is only a link and is removed as one.

For both, the paths (1 to `max_paths`) are validated first: one invalid path refuses the whole request with `file_error` and removes nothing. Then every path is attempted in order; each one is in `removed` or, with its error, in `failed`, so one missing entry does not stop the others.

**Errors.** A failed or refused file request gets one reply. Apart from a tree deletion that fails part way, it changes nothing:

```json
{"Stream":{"type":"file_error","request":"f4","error":{"code":"conflict","message":"the file changed since it was read","path":"/home/me/project/README.md","current_hash":"9a0c…"}}}
```

`code` is one of `invalid_path`, `invalid_request`, `invalid_content` (base64 that does not decode), `not_found`, `permission_denied`, `not_a_directory`, `is_a_directory`, `not_a_file`, `symlink`, `exists`, `not_empty`, `conflict` (with `current_hash` when the content was hashed), `too_large` (with `size` for a file that is too large to read), `cross_device`, `read_only`, `no_space`, `busy`, `io` (any other system error; `message` has its description), `outside_root` (the path, or a symlink on the way to it, leads outside the file roots or out of the root the path lies in; added with `file_limits.root`, so a client that does not know it shows `message`) and `cross_root` (the two ends of a `file_rename` lie in different file roots; added with `file_limits.roots`, `path` is the target). `path` names the path the error concerns when there is one. A malformed `request` identifier is still a plain `error` without `request`, as for every request.

**Threading.** File requests run on worker threads, so a slow disk or a large delete never delays pane output. Replies may therefore arrive in a different order than the requests and between other events; match them by `request`. A reply that does not fit the connection's output queue (several large reads finishing together, or a burst of `busy` refusals) waits for the queue to drain instead of disconnecting the client; a connection with more than 256 refusals waiting is disconnected as a slow peer. Running requests cannot be cancelled: one stuck on an unresponsive file system (a hung network mount) keeps its slot until the system call returns, and enough of them make every file request answer `busy` until it does or the server restarts. A server stopping while a request runs does not wait for it, and its reply is lost (a delete may then be partial).

### Downloads (`file-downloads`)

A client saves files from the server host on the user's computer: a single regular file as itself, or files and folders as one gzip-compressed tar archive (`.tar.gz`). The server streams the bytes in chunks that the client acknowledges, the reverse of uploads, so nothing is built whole in memory or on disk and a slow client slows its download, never the server. Downloads are an addition to `files`: same authorization, same paths and file root, same errors. The root itself is not downloaded (`invalid_request`); select the entries inside it. ADR 0003 section 3 ([KelpCore ADR 0003](../../../docs/adr/0003-kelpcore-shared-repository.md)).

**Limits.** A server with the capability adds `downloads` to Welcome `file_limits`; a client offers downloads only when both are present:

```json
{"Stream":{"type":"welcome",…,"capabilities":[…,"files","file-downloads"],"file_limits":{…,"downloads":{"chunk_bytes":262144,"max_chunks_in_flight":8,"max_downloads":2,"max_archive_entries":100000,"max_archive_bytes":17179869184}}}}
```

- `chunk_bytes`: decoded bytes per chunk (256 KiB, about 342 KiB of base64, like an upload chunk).
- `max_chunks_in_flight`: the flow-control window (8), below.
- `max_downloads`: unfinished downloads per connection (2). A download the connection cancelled (or that the server ended early) still counts against it until its thread returns, so cancelling and restarting never runs more threads than this. The server runs at most 8 download threads in total, counting cancelled downloads whose thread has not returned yet. A download over a bound gets `file_error` `busy` at once. Downloads do not take the `max_in_flight` slots of other file requests, so browsing stays responsive during a long download.
- `max_archive_entries`: files, folders and symlinks in one archive (100,000); `max_archive_bytes`: the sum of the archived files' sizes (16 GiB). Folders nest at most `max_depth` levels (64). A single file has no size bound.
- Not advertised: each walk of the tree (the planning walk, then the archive walk) lists at most 200,000 names, counting what the entry bound does not (devices, FIFOs, sockets and names that could not be examined), and holds at most `max_archive_entries` listed names not yet processed across all the folders it has open. Both refuse the archive with `too_large`, like the advertised bounds.

**Begin.**

```json
{"Stream":{"type":"file_download_begin","request":"d1","paths":["/home/me/notes.pdf"]}}
{"Stream":{"type":"file_download_started","request":"d1","download":1,"name":"notes.pdf","size":48213}}
{"Stream":{"type":"file_download_begin","request":"d2","paths":["~/project"],"archive":"tar.gz"}}
{"Stream":{"type":"file_download_started","request":"d2","download":2,"name":"project.tar.gz","archive":"tar.gz","entries":214,"input_bytes":3811077}}
```

Without `archive`, `paths` names exactly one regular file. It is opened like `file_read` opens one: a final symlink is followed, the type is checked before the file is opened (`is_a_directory` for a folder, `not_a_file` for a FIFO, socket or device, which is never opened), then checked again on the open descriptor. `size` is its size when opened; the bytes sent are whatever the file holds when read, so `file_download_done.size` can differ if the file changed.

With `"archive":"tar.gz"`, `paths` (1 to `max_paths`) name files, folders or symlinks, each archived under its own name (a folder with its whole tree below it); two selected entries with the same name are refused (`invalid_request`). Before the first byte, a planning walk counts the entries and input bytes and refuses an archive over the bounds or `max_depth` with `too_large`, so a client learns that before saving anything. `entries` and `input_bytes` report what it found; chunks of an archive carry `input`, the input bytes read so far, for progress (the compressed size is not known in advance). `name` is the suggested file name: the file's name, `<folder>.tar.gz`, `<name>.tar.gz` for one selected file or symlink, `<their folder>.tar.gz` for several entries of one folder, or `download.tar.gz`.

`download` is a number unique within the server run, addressed only by the connection that began the download. Failures before the start (paths, types, bounds, `busy`) are `file_error {request, error}` without `download`, as for every file request.

**Chunks and flow control.**

```json
{"Stream":{"type":"file_download_chunk","download":2,"offset":0,"data":"H4sIAAAAAAAA/+y9…","input":262144}}
{"Stream":{"type":"file_download_ack","download":2,"offset":262144}}
```

`data` is standard padded base64 of 1 to `chunk_bytes` bytes at `offset`; chunks arrive in order at contiguous offsets. **Flow-control rule:** the server sends at most `max_chunks_in_flight` chunks that are not acknowledged; the client acknowledges each chunk, in order, with its end offset (`offset` + its length, the bytes received so far), and should do so once it has stored the chunk, so a slow disk or a busy page slows the server rather than filling memory. An acknowledgement that is not the oldest unacknowledged chunk's end fails the download (`file_error` `invalid_request` with `download`); one for a download that ended, or of another connection, is ignored. Acknowledgements carry no `request` and get no reply. The server takes a chunk from the reading thread only while the window and the connection's output queue have room, never ahead of a pending screen update, and with at most two chunks' worth of download events (about 700 KB) in the connection's output queue across all its downloads, so a download neither disconnects its client nor delays a pane update by more than two chunks. **Stall timeout:** a download waiting for its client (a chunk unacknowledged, or an event waiting for room in the output queue) with no acknowledgement and no event taken for 10 minutes is stopped with `file_error` `io` (with `download` once started), which frees its slot and thread; a client that stops reading or acknowledging cannot hold them longer. A client that gives up on a `file_download_begin` (no reply in time) should cancel the download if its `file_download_started` arrives later.

**End.**

```json
{"Stream":{"type":"file_download_done","request":"d2","download":2,"size":1094730,"hash":"9a0c…","skipped":1,"changed":0}}
```

`size` and `hash` (lowercase hexadecimal SHA-256) cover the bytes sent, so a client can check what it saved. `skipped` counts archive entries left out: devices, FIFOs and sockets (never opened), entries the server's user may not read (permission denied on a file or folder), and the contents of mount points. `changed` counts files that changed size or modification time while being read. A failure after the start (an I/O error, an archive that grew past a bound while streaming) is `file_error {request, download, error}`; nothing more follows for that download, and the client discards what it received.

**Cancel.**

```json
{"Stream":{"type":"file_download_cancel","request":"d3","download":2}}
{"Stream":{"type":"done","request":"d3"}}
```

stops the download: no further event of it follows the `done` (chunks queued before it are still delivered first). Cancelling a download that already ended, or one this connection never had, also replies `done`. A disconnect cancels the connection's downloads. The reading thread stops at its next read or chunk, or at the next name of a planning or archive walk; one stuck on an unresponsive file system keeps its server-wide thread slot until the system call returns.

**Archive format.** POSIX tar (ustar headers), gzip-compressed (deflate level 2, streamed). A pax extended header (`x`) carries the `path` of an entry whose name is longer than 100 bytes or not printable ASCII, the `linkpath` of such a symlink target, a `size` of 8 GiB or more, and an `mtime` before 1970 or past the octal field, preceded by `hdrcharset=BINARY` when a name or target is not UTF-8; GNU tar, bsdtar, 7-Zip and Python's `tarfile` read them. For readers that ignore pax, the ustar fields of such an entry hold a safe stand-in: the name split into `prefix` and `name` on a `/` when it fits, otherwise its last components that fit; printable ASCII only, never absolute and without `.` or `..` components (a long symlink target likewise). Folders (`5`, name ending in `/`) precede their contents; each folder's entries are in byte order of their names. Entries carry the permission bits (`mode & 0o777`, without set-id or sticky bits) and the modification time in seconds; owner and group are 0 with empty names, so the archive does not disclose the server's account names, and an extracting user owns what they extract. The walk never follows a symlink: symlinks are stored as links (`2`) with their target text, and folders and files are opened through their parent's descriptor with `O_NOFOLLOW`. It never enters another file system: a mount point inside a folder is stored as an empty folder and counted in `skipped`, like `tar --one-file-system`. Each folder is entered only if the descriptor it opens is the folder its listing described (same device and inode) on the walk's file system, which catches a mount point whose `lstat` describes the covered folder and a folder replaced between the listing and its turn; such a folder is likewise stored empty and counted in `skipped`. Entries are examined without triggering an automount. A file's header records its size when it is opened: a file that grows while being read is cut at that size, one that shrinks is padded with zero bytes, and both count in `changed`, so the archive always stays well formed. An entry that disappears between the listing and its turn is left out silently. An archive that grows past a bound while streaming fails with `too_large` (no partial archive is ever reported as done).

**Threading.** Each download runs on its own thread, which reads files and compresses; the event loop only moves finished chunks to the connection, so pane output is never delayed by a slow disk or by compression.

### Parking and hidden panes

The server has no parking concept. A client that hides a pane unsubscribes from it and either keeps its lease, so its size stays fixed while hidden, or releases it, so the pane returns to the server's base size.
