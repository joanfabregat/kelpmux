# ADR 0001: Native clients, independent panes and device capabilities

- Status: Proposed. The native stream foundation is experimental; the full contract is not frozen.
- Date: 2026-10-14 (America/Los_Angeles).
- Scope: Native Kelpmux. tmux compatibility remains an independent optional adapter.

## Context

Kelpmux owns persistent PTYs and Alacritty terminal state. A terminal frontend currently presents a workspace as a grid or responsive tabs. Future desktop, iPad and iPhone clients should own their native presentation, scrolling and interaction, while connecting to the same long-running remote terminals. A Mac might display pane 1 at 240 × 70 cells while an iPhone displays pane 2 at 50 × 25 cells. Agent state, notifications and the remote machine's Vitals metrics must remain available independently of which terminal is visible.

Agents will eventually need a standard MCP interface for asking the user questions and requesting device capabilities such as clipboard access, screenshots, location, camera capture and notifications. This ADR reserves that boundary without implementing a native app, MCP server or device access now.

## Decision

Use a Kelpmux-owned application protocol over SSH, with standard framing and serialization components. Do not depend on tmux or WezTerm protocols or implementations. Keep the application protocol independent from SSH so disposable Unix-socket tests and future transports can exercise the same contracts. The first encoding is bounded, length-prefixed JSON; a future compact encoding must be negotiated rather than silently replacing it.

Alacritty on the server remains the authoritative emulator. Clients receive decoded terminal cells, cursor and input modes rather than a second raw VT stream. This prevents duplicate replies to application terminal queries. Client UI elements, including tabs, sidebars, grids, settings and confirmation dialogs, are not baked into terminal screen updates.

## Objects, identity and versions

A workspace is a persistent group of terminals and metadata. A pane owns one PTY and terminal history. A tab is a client-side representation of a pane; closing a local view is distinct from terminating the pane. A future tab grouping multiple panes must reference pane IDs explicitly rather than redefine PTY ownership.

A server epoch changes on server restart. Pane IDs and terminal revisions are scoped to that epoch; clients must never apply an update from another epoch to an existing screen. Reconnecting requires negotiation and a fresh snapshot unless an explicitly supported bounded replay can prove continuity. Session identifiers for Codex and Claude remain metadata, not terminal identity or authorization tokens.

The composed ANSI frontend protocol and the experimental native stream protocol have separate version numbers. Negotiate an application version and capabilities before subscriptions or mutations. Required unsupported features fail clearly; optional unavailable features remain visibly unavailable. Published versions must document compatibility and migrations. Keep old installed binaries immutable so an existing server can retain its matching client.

## Logical channels

Channels are logical message families carried over one SSH command channel, not separate public ports or separate SSH logins. Every request has a correlation ID in the eventual public contract; every response identifies its request. Event families use independent sequence/revision semantics rather than assuming a terminal revision orders all metadata.

| Channel | Responsibilities |
| --- | --- |
| Workspace/pane control | Enumerate, create, name, reorder and terminate panes; attach/detach views; claim/release input and size ownership; explicit confirmations for destructive actions. |
| Terminal | Subscribe to selected pane IDs; snapshots, deltas, cursor/modes, history pagination, search and selection; pane-addressed input, paste and mouse. |
| Agent state | Initial metadata snapshot and live changes to agent kind, name, session ID, lifecycle state, evidence source and observation freshness. |
| Notifications | Questions, completion/failure/bell events, unread state, acknowledgement and bounded replay after reconnect. Delivery, acknowledgement and answering a question are distinct actions. |
| Remote vitals | Typed CPU, memory, swap, disk and clock readings with units, sample timestamps and explicit unavailable values, independent of status-bar styling. |
| Device requests | Correlated requests, approval/denial, progress, cancellation, expiration and responses for capabilities exposed by a selected native client. |
| Attachments | Bounded, typed transfer of approved images or other device results without blocking control messages behind a large payload. |

The initial stream implementation may combine multiple logical families into a bounded update message. This does not freeze the final envelope or imply that every family above is implemented.

## Pane control and independent geometry

Input and terminal-size ownership are per pane, not per workspace or device. Client A may own pane 1 while client B owns pane 2. Other clients may observe either pane. Merely subscribing or opening a tab must not seize input or resize an application.

Each pane has exactly one underlying PTY size at a time. Multiple viewers of the same pane can clip, scale or locally scroll its content, but cannot demand simultaneous incompatible PTY sizes. The explicit size owner chooses the pane's column/row dimensions within negotiated bounds. Resizing pane 2 on an iPhone must not resize pane 1 on a Mac. Pixel dimensions, font scaling, safe areas and tab placement belong to each native client.

Claims and takeover are explicit. Disconnect releases that connection's claims, with a documented fallback to the terminal frontend's base geometry. An active terminal frontend must not implicitly overwrite another client's pane size or send application input to its pane. A future split between input and size leases is possible but must preserve the single-size rule. Heartbeat and reconnect grace periods require explicit negotiated behavior; they must not manufacture a second owner.

Create/rename/reorder/close operations must validate current epoch and pane identity. Closing a view preserves its PTY; terminating a pane uses a separate explicit operation and confirmation token bound to the target and current request. Grid shrink retains the existing busy-job guard. Local trusted administration remains available, but a negotiated native stream connection cannot bypass ownership by switching to legacy management messages.

## Terminal history and synchronization

Screen snapshots establish a baseline; patches name both their base and resulting revision. Geometry or buffer changes may require a new full snapshot. Missing, duplicate or stale revisions must trigger rejection/resynchronization, not a partially corrupted display. Slow clients receive bounded replacement snapshots rather than an unbounded delta backlog, and cannot stall PTY reads or other subscribers.

History for native scrolling must eventually use stable row identifiers plus a buffer generation. Current negative scrollback offsets are a bounded inspection primitive, not durable bookmarks: output, eviction, alternate screens and reflow can invalidate them. Future paginated styled history should return explicit expired/stale-anchor errors and preserve character widths and attributes. History remains bounded in memory and does not enter the metadata journal.

## Live agent state, notifications and vitals

Status updates carry the observed state and its evidence. An idle agent is not automatically a completed turn; terminal output activity is not proof that an agent is still working or done. Hooks, process-bound live records, supported daemon metadata and transcript observations should be distinguishable. Missing or stale evidence stays uncertain, and ambiguous session IDs stay unknown. Markers and spinner animation are client presentation choices.

Notifications have stable IDs within their workspace/event namespace, bounded seven-day retention and deduplicated replay. Reconnection must not replay old completion events as new sounds or duplicate native notifications. Current shared unread state is suitable for the single-user workspace; per-device delivered state and push delivery are future extensions. Question contents and sensitive capability results must not be added to the recovery journal.

Vitals originate on the remote machine. Send typed readings rather than colored status-bar strings; preserve unavailable values and sampling age. The existing Vitals-derived sampler and terminal footer remain the source for initial CPU, memory, swap, disk percentages and remote date/time. Byte totals, mount labels, uptime and richer metrics can be added as negotiated capabilities. Native clients format their own colors, units and time zones.

## SSH bridge and reference client

Use one authenticated SSH command request without allocating an outer PTY. A planned invocation is `ssh -T HOST kelpmux bridge --workspace main`. SSH supplies authentication, encryption and transport; no new publicly listening service or additional login is required. The bridge connects only to the existing owned private Unix socket. Standard output contains protocol records only; diagnostics go to standard error. Shell banners must not contaminate the command channel.

The bridge must bound messages and queues, validate the endpoint and handshake, preserve input ordering, and handle EOF, disconnect and backpressure without terminating the workspace. Framing limits are negotiated or explicitly versioned. Large terminal/attachment traffic must not starve input, cancellation, question responses or acknowledgements. The initial transport remains reliable and ordered; no Mosh-like roaming or offline native push is implied.

A small reference client and actual subprocess/pipe tests should validate the bridge before a graphical client is built. The implementation checklist and native-client guide must distinguish implemented requests from proposals in this ADR.

## MCP and native device capabilities (future)

Run a local stdio MCP adapter on the remote host, for example the future command `kelpmux mcp`. The agent is the MCP client; Kelpmux is its MCP server. The adapter translates standard MCP tool calls into Kelpmux device requests targeted at a connected native client. It does not expose the native stream protocol as an MCP replacement, and the native app need not itself become the agent's MCP host.

Proposed tools include `ask_user`, `clipboard_read`, `clipboard_write`, `screenshot`, `location`, `camera_capture` and `notify`. These are Kelpmux tool contracts, not capabilities automatically supplied by the MCP standard. A question tool can route a form or choices to the native app and return its structured answer. MCP elicitation is a separate optional mechanism whose user interaction is normally hosted by the MCP client; adopting it must not mistakenly send the question back only to a hidden agent TUI.

Each request identifies the workspace, epoch, originating pane and intended device, with a request ID, deadline and cancellation state. Prefer an explicitly selected device or the pane's current owner; never silently route a camera/location request to every connected device. Multiple devices must not race to answer or capture. No connected or capable device yields an explicit unavailable response. Platform suspension, permission denial, cancellation and expiration are normal outcomes. A failed or timed-out capture must not be automatically replayed after reconnect.

The native device advertises supported capabilities. Remote requests do not grant permissions. Clipboard reads, screen capture, location and camera access require visible device-side authorization and the platform's own permission rules; grants are scoped, revocable and default to a single request unless the user explicitly chooses otherwise. Screenshots distinguish a rendered terminal capture from a device/screen capture. Camera capture returns only the user-approved result; ongoing camera streams are outside this proposal. Notification delivery obeys the device's notification settings.

SSH authenticates a remote account, not every individual agent under that account. Environment variables such as `KELPMUX_PANE` identify context but are forgeable by same-user processes. Do not claim hostile-agent isolation from a pane ID or discovered session ID. A future broker must bind requests to an adapter session and present the available provenance honestly; stronger workload isolation requires a separately designed identity boundary. Tool annotations are descriptive, not authorization.

Questions, clipboard contents, screenshots, coordinates and camera images are not persisted in the recovery journal, diagnostics or general logs. Retain only minimal capability-operation metadata needed for delivery/audit, with a defined short lifetime. Transfer approved attachments with explicit type/size limits, bounded memory, cancellation and expiry; do not introduce an arbitrary local-file or URL fetch capability through attachment references. MCP implementation must pin and test a supported specification revision rather than depend on an unversioned draft.

## Delivery plan and validation

1. Reliability: matching-client discovery, explicit version errors, process-preserving upgrade behavior, deterministic terminal fixtures, seeded stress tests, bounded history and performance measurements.
2. Experimental native foundation: pane subscriptions/snapshots/deltas, per-pane ownership and sizes, status/notification/vitals updates, SSH stdio bridge and a reference client.
3. Contract maturation: stable styled-history anchors, request correlation, native pane lifecycle controls, capability negotiation and independently tested reconnect/queue policies.
4. Native clients: platform presentation, native scrolling, accessibility and device-specific lifecycle behavior.
5. MCP/device broker: negotiated tools and explicit device permissions, bounded/cancellable requests and attachment handling. No device access is enabled by this ADR.

Required contract tests include two clients owning different panes at different sizes; a third read-only observer; rejected unauthorized input/resize; explicit takeover/release/disconnect; cross-epoch rejection; snapshot/patch replay under slow consumers; Unicode/colors/alternate buffers; status uncertainty and lost hooks; notification replay; missing/stale vitals; bridge EOF and binary framing; version mismatch without raw-terminal corruption; and simulated device approval/denial/cancellation/expiry before any real device API is introduced.

## Alternatives and consequences

Reusing tmux control mode would require tmux command/notification semantics and a compatibility maintenance commitment. Reusing WezTerm's codec would couple Kelpmux to its terminal/workspace types. Remote-display protocols do not directly provide our pane ownership, structured history and agent metadata model. A small owned protocol fits the existing Alacritty engine, but makes specification, compatibility testing and version negotiation our responsibility.

The protocol remains experimental until a real native client validates it. The MCP/device section is a proposed boundary, not an implementation promise or implicit permission grant. A browser UI, native app, SSH library, MCP SDK, compact encoding and background push infrastructure are separate decisions.

## References

- [Current native-client guide](../native-client-protocol.md).
- [tmux control mode](https://github.com/tmux/tmux/wiki/Control-Mode).
- [WezTerm codec source](https://github.com/wezterm/wezterm/blob/main/codec/src/lib.rs).
- [MCP tools, pinned 2025-11-25 reference](https://modelcontextprotocol.io/specification/2025-11-25/server/tools).
- [MCP elicitation, pinned 2025-11-25 reference](https://modelcontextprotocol.io/specification/2025-11-25/client/elicitation).
- [MCP transports, pinned 2025-11-25 reference](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).

## Initial-foundation limits

The first implementation supports subscriptions, snapshots/deltas, per-pane connection-lived ownership, pane input/resize, metadata, notification snapshots/acknowledgement and existing Vitals values. Native pane creation/termination, stable styled-history anchors, request correlation, MCP and device capabilities remain proposed. Current Vitals contain percentages and server-local clock strings; timestamps, explicit units and timezone-aware conversion require additional fields.

Initial v1 records use FIFO transport queues and may be large. The future device-request/attachment phase must introduce bounded bulk chunks and scheduling before claiming responsive control delivery under heavy transfer. Scheduling cannot remove TCP head-of-line blocking. Multiple SSH channels on the same authenticated connection remain an option if profiling justifies separate flow-control windows; no extra authentication or publicly listening port is implied.
