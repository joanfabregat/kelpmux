# ADR 0001: Native clients, independent panes and device capabilities

- Status: Proposed. The native stream foundation is experimental; the full contract is not frozen.
- Date: 2026-10-14 (America/Los_Angeles).
- Updated: 2026-10-17 (America/Los_Angeles), client-approved SSH execution requests.
- Scope: Native Kelpmux. tmux compatibility remains an independent optional adapter.

## Context

Kelpmux owns persistent PTYs and Alacritty terminal state. A terminal frontend currently presents a workspace as a grid or responsive tabs. Future desktop, iPad and iPhone clients should own their native presentation, scrolling and interaction, while connecting to the same long-running remote terminals. A Mac might display pane 1 at 240 × 70 cells while an iPhone displays pane 2 at 50 × 25 cells. Agent state, notifications and the remote machine's Vitals metrics must remain available independently of which terminal is visible.

Agents will eventually need a standard MCP interface for asking the user questions and requesting device capabilities such as clipboard access, screenshots, location, camera capture, notifications and client-initiated SSH commands. This ADR reserves that boundary without implementing a native app, MCP server or device access now.

The native client should also be able to present an embedded browser that remote agents can inspect and interact with through MCP. This must operate on the page the user sees on their device, while acknowledging that an iOS/iPadOS WebKit view does not provide the same debugging interface as a Chromium browser on the remote host.

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

Native clients must support SSH authentication with non-exportable Secure Enclave-backed keys. The client performs signing on the device through an appropriate platform key provider, including any required user approval; neither private keys nor an exportable substitute are sent to jbox. This is part of establishing the SSH connection, independent of the later MCP/device-request channel. SSH library selection must demonstrate compatibility with Joan's existing Secure Enclave key workflow and accepted server-side public keys before adoption. Platform key access, supported signing algorithms, cancellation and reconnect behavior require device acceptance tests; password authentication or a software-key replacement is not an equivalent substitute.

The bridge must bound messages and queues, validate the endpoint and handshake, preserve input ordering, and handle EOF, disconnect and backpressure without terminating the workspace. Framing limits are negotiated or explicitly versioned. Large terminal/attachment traffic must not starve input, cancellation, question responses or acknowledgements. The initial transport remains reliable and ordered; no Mosh-like roaming or offline native push is implied.

A small reference client and actual subprocess/pipe tests should validate the bridge before a graphical client is built. The implementation checklist and native-client guide must distinguish implemented requests from proposals in this ADR.

## MCP and native device capabilities (future)

Run a local stdio MCP adapter on the remote host, for example the future command `kelpmux mcp`. The agent is the MCP client; Kelpmux is its MCP server. The adapter translates standard MCP tool calls into Kelpmux device requests targeted at a connected native client. It does not expose the native stream protocol as an MCP replacement, and the native app need not itself become the agent's MCP host.

Proposed tools include `ask_user`, `clipboard_read`, `clipboard_write`, `screenshot`, `location`, `camera_capture` and `notify`. These are Kelpmux tool contracts, not capabilities automatically supplied by the MCP standard. A question tool can route a form or choices to the native app and return its structured answer. MCP elicitation is a separate optional mechanism whose user interaction is normally hosted by the MCP client; adopting it must not mistakenly send the question back only to a hidden agent TUI.

Each request identifies the workspace, epoch, originating pane and intended device, with a request ID, deadline and cancellation state. Prefer an explicitly selected device or the pane's current owner; never silently route a camera/location request to every connected device. Multiple devices must not race to answer or capture. No connected or capable device yields an explicit unavailable response. Platform suspension, permission denial, cancellation and expiration are normal outcomes. A failed or timed-out capture must not be automatically replayed after reconnect.

The native device advertises supported capabilities. Remote requests do not grant permissions. Clipboard reads, screen capture, location and camera access require visible device-side authorization and the platform's own permission rules; grants are scoped, revocable and default to a single request unless the user explicitly chooses otherwise. Screenshots distinguish a rendered terminal capture from a device/screen capture. Camera capture returns only the user-approved result; ongoing camera streams are outside this proposal. Notification delivery obeys the device's notification settings.

SSH authenticates a remote account, not every individual agent under that account. Environment variables such as `KELPMUX_PANE` identify context but are forgeable by same-user processes. Do not claim hostile-agent isolation from a pane ID or discovered session ID. A future broker must bind requests to an adapter session and present the available provenance honestly; stronger workload isolation requires a separately designed identity boundary. Tool annotations are descriptive, not authorization.

Questions, clipboard contents, screenshots, coordinates and camera images are not persisted in the recovery journal, diagnostics or general logs. Retain only minimal capability-operation metadata needed for delivery/audit, with a defined short lifetime. Transfer approved attachments with explicit type/size limits, bounded memory, cancellation and expiry; do not introduce an arbitrary local-file or URL fetch capability through attachment references. MCP implementation must pin and test a supported specification revision rather than depend on an unversioned draft.

## Client-approved SSH execution through MCP (future)

A remote agent may request that a selected native client execute an SSH command on a client-approved destination. This is a device capability carried through the existing MCP adapter and correlated device-request channel, not unrestricted local shell execution or permission for the remote host to use the client's SSH identities. It can expose the client's network reachability and destination privileges, so it is disabled by default and enabled separately for each remote connection. Start with client-defined destination profiles and named operations with validated parameters; arbitrary commands remain a separate advanced permission, outside the initial implementation. A proposed tool is `ssh_execute`; the exact tool name and schema remain unfrozen.

The request and approval flow is:

1. The agent submits the destination profile, requested operation and parameters, reason, intended device and deadline through MCP. The broker includes the available workspace, epoch and requesting-agent provenance; a claimed pane or session name is not proof of agent identity.
2. The native client validates its local policy and resolves the operation into an immutable execution request. A trusted client-side dialog shows the requesting remote connection and available agent provenance, target host/port and account, SSH identity, exact resulting command and what output will be returned to the remote agent. Agent-supplied text is rendered as untrusted text and cannot replace the trusted approval controls.
3. The user approves or rejects that exact request once. Approval is bound to its request ID and resolved execution details, including output-return scope. Changed parameters, identity, destination or command require a new approval; a duplicate request cannot reuse an already consumed grant. The remote host and agent cannot approve their own requests. Rejection or expiration returns an explicit result without execution.
4. After approval, the client opens its own SSH connection using the selected locally held key, including Secure Enclave-backed signing where supported, and executes the approved operation. Destination host keys must be verified; an unknown or changed key requires a separate trusted client-side decision. Private keys stay on the client, SSH agent forwarding is disabled, and remote-supplied SSH options, proxy commands and local command hooks are not accepted. A key-provider authentication prompt is additional to command approval, not a substitute for it.
5. The client returns bounded, approved stdout/stderr and execution status through the correlated response channel. Expose timeout, cancellation and progress where supported. Returning output is itself a disclosure to the remote agent and must respect the approved scope; no automatic copying of unrelated local files or credentials is implied. Treat returned command output as untrusted data rather than permission for another action.

Execution has client-enforced time and output limits and a visible cancel control. Losing the Kelpmux connection must not cause automatic execution, duplicate execution or replay after reconnect. If the client cannot determine whether an approved command started or finished, report an explicit unknown outcome; do not claim exactly-once execution across a crash or retry it automatically. Closing an SSH connection or requesting cancellation does not prove that a command or its descendants stopped on the destination. Any fresh execution requires fresh approval.

Keep a short-lived audit record of request identity, destination profile, decision, timing and outcome. Raw commands, parameter values, reason text and output may contain secrets and stay out of recovery journals and general diagnostics; retention or export of sensitive details requires a separate explicit policy. No local process-launch API is assumed: platform and SSH-library selection must demonstrate client-originated SSH execution, key-provider integration and cancellation on the supported devices before advertising the capability.

Acceptance tests must cover default denial, disallowed destinations and operations, invalid parameters, honest provenance, approval UI rendering, rejection/expiry, mutation after approval, duplicate requests, one-use grants, key/host verification, output disclosure and limits, cancellation, disconnect before and after execution, and unknown outcomes without replay. Test the broker and client policy with simulated execution before enabling real device SSH operations. No SSH execution capability is enabled by this ADR.

## Embedded browser and MCP automation (future)

Use a client-owned embedded browser, initially Apple's public `WKWebView` APIs on Apple platforms. The remote stdio MCP adapter routes correlated browser requests over the authenticated Kelpmux connection to the selected native client. Results return over the same logical request/attachment channels. No additional public listener, separate device login or Mac debugging connection is required for these app-owned tools. This is a proposed architecture, not an implemented browser or a promise of Chrome DevTools feature parity.

The embedded browser must support YubiKey-backed website authentication, including WebAuthn/FIDO2 security-key ceremonies on the supported client platforms. Hardware interaction and any PIN, touch or user-presence prompts remain under the platform/user's control; an agent may initiate the website flow but cannot bypass or synthesize hardware approval. Validate the intended physical connection methods on real devices, including USB-C and NFC where supported by the device/platform. Browser/library selection must demonstrate successful registration and sign-in with the intended YubiKeys, as well as cancellation, absent-key and unsupported-transport behavior. Do not silently substitute another authenticator. Smart-card/PIV or other YubiKey modes are separate capabilities and are not implied by WebAuthn support.

Browser views have identities separate from terminal pane IDs. Requests identify the workspace/epoch, originating adapter session, device, browser view and document generation. Navigation changes the document generation; stale element references and outstanding page-specific actions must fail explicitly rather than operate on a replacement document. A browser view must not be silently retargeted when the user selects a different terminal or browser tab. Browser state and page content do not become PTY output or recovery-journal entries.

Expose one browser tool family with negotiated capabilities and explicit backend selection. Proposed operations include opening/navigating a view, obtaining a bounded DOM/text snapshot, interacting with elements, running page JavaScript, taking a page screenshot, and subscribing to supported console/network events. Exact tool names and schemas remain unfrozen. Return backend identity, current URL/document identity and capture limitations with results; an unsupported operation must return an explicit unavailable result, never silently run against a different browser.

| Backend | Intended capabilities | Limits |
| --- | --- | --- |
| Native embedded WebKit | The page visible in the client; navigation, DOM inspection and scripted interaction through JavaScript, page screenshots, and instrumented console/fetch/XHR observations. | Public `WKWebView` APIs do not expose an in-app Chrome DevTools Protocol endpoint. Script instrumentation is partial and must not be labeled a complete network trace. Some interactions require platform/user involvement. |
| Optional remote browser | A browser on jbox, with its supported debugging protocol, for comprehensive network/debugging operations and unattended inspection. | It is a separate browser session with separate cookies, viewport and engine behavior. It does not demonstrate what the embedded device page actually did. Its runtime and sandboxing require a separate implementation decision. |

Apple's `isInspectable` enables Safari Web Inspector inspection through a connected Mac; it is not our in-app MCP transport or a requirement for ordinary embedded-browser tools. `WKURLSchemeHandler` handles custom schemes and is not a general HTTP/HTTPS interception API. WebKit offers per-data-store proxy configuration on iOS/iPadOS 17 and later, so an optional debugging proxy is a possible broader-capture path. Decrypting HTTPS traffic would require an explicit certificate-trust setup; proxy support alone does not provide response bodies or complete capture. Proxy deployment, coverage and trust are deferred, and the initial embedded implementation must report the narrower instrumentation coverage honestly.

Browser automation follows the device-request authorization, cancellation, deadlines and attachment limits above. Scope any reusable grant to a named device/browser session and make it revocable. Script execution and page interaction can use signed-in website authority, so the client must visibly indicate agent control and allow it to be stopped. Page content is untrusted data and cannot grant browser/device permissions or authorize a different MCP operation. Browser screenshots capture the browser view, distinct from terminal or whole-device screenshots. Do not copy cookies, credentials or browser profiles between embedded and remote backends implicitly.

Console/network subscriptions use bounded queues, sequence/gap information and explicit capture coverage. Sensitive headers, request/response bodies, console output and screenshots stay out of general diagnostics and the recovery journal; collecting or exporting them requires a scoped capture policy. Client suspension or disconnect makes device browser tools unavailable or expires outstanding requests; continuous background automation is not assumed. Reconnect does not automatically repeat navigation, form submission or other page actions.

Deliver the embedded browser and basic inspection tools first. Add remote debugging and optional proxy capture as separately negotiated extensions. Validate request routing across multiple devices/views, document-generation invalidation, denied/revoked grants, disconnect/cancellation without replay, bounded large results and event gaps, unsupported capabilities, capture redaction and separation of embedded versus remote sessions. Device tests must exercise actual WebKit navigation, DOM interaction, screenshots and instrumentation limits before advertising those capabilities.

## Delivery plan and validation

1. Reliability: matching-client discovery, explicit version errors, process-preserving upgrade behavior, deterministic terminal fixtures, seeded stress tests, bounded history and performance measurements.
2. Experimental native foundation: pane subscriptions/snapshots/deltas, per-pane ownership and sizes, status/notification/vitals updates, SSH stdio bridge and a reference client.
3. Contract maturation: stable styled-history anchors, request correlation, native pane lifecycle controls, capability negotiation and independently tested reconnect/queue policies.
4. Native clients: platform presentation, native scrolling, accessibility, device-specific lifecycle behavior and validated Secure Enclave-backed SSH authentication.
5. MCP/device broker: negotiated tools and explicit device permissions, bounded/cancellable requests and attachment handling. Client-originated SSH execution starts with named operations and per-request local approval, subject to the policy and acceptance tests above. No device access is enabled by this ADR.
6. Browser client and MCP tools: embedded WebKit views, YubiKey-backed WebAuthn authentication and basic page inspection first; optional remote debugging and broader network capture only through explicitly advertised extensions.

Required contract tests include two clients owning different panes at different sizes; a third read-only observer; rejected unauthorized input/resize; explicit takeover/release/disconnect; cross-epoch rejection; snapshot/patch replay under slow consumers; Unicode/colors/alternate buffers; status uncertainty and lost hooks; notification replay; missing/stale vitals; bridge EOF and binary framing; version mismatch without raw-terminal corruption; and simulated device approval/denial/cancellation/expiry before any real device API is introduced.

## Alternatives and consequences

Reusing tmux control mode would require tmux command/notification semantics and a compatibility maintenance commitment. Reusing WezTerm's codec would couple Kelpmux to its terminal/workspace types. Remote-display protocols do not directly provide our pane ownership, structured history and agent metadata model. A small owned protocol fits the existing Alacritty engine, but makes specification, compatibility testing and version negotiation our responsibility.

The protocol remains experimental until a real native client validates it. The MCP/device and browser sections are proposed boundaries, not implementation promises or implicit permission grants. Native app implementation, a remote browser runtime, proxy infrastructure, an SSH library, an MCP SDK, compact encoding and background push infrastructure remain separate decisions.

## References

- [Current native-client guide](../native-client-protocol.md).
- [tmux control mode](https://github.com/tmux/tmux/wiki/Control-Mode).
- [WezTerm codec source](https://github.com/wezterm/wezterm/blob/main/codec/src/lib.rs).
- [MCP tools, pinned 2025-11-25 reference](https://modelcontextprotocol.io/specification/2025-11-25/server/tools).
- [MCP elicitation, pinned 2025-11-25 reference](https://modelcontextprotocol.io/specification/2025-11-25/client/elicitation).
- [MCP transports, pinned 2025-11-25 reference](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).
- [OpenSSH authentication and agent-forwarding considerations](https://man.openbsd.org/ssh).
- [Apple WKWebView: navigation, JavaScript evaluation and snapshots](https://developer.apple.com/documentation/webkit/wkwebview).
- [Apple inspection of iOS/iPadOS content](https://developer.apple.com/documentation/safari-developer-tools/inspecting-ios).
- [Apple custom URL scheme handlers](https://developer.apple.com/documentation/webkit/wkurlschemehandler).
- [Apple per-data-store proxy configuration](https://developer.apple.com/documentation/webkit/wkwebsitedatastore/proxyconfigurations-6g21z).

## Initial-foundation limits

The first implementation supports subscriptions, snapshots/deltas, per-pane connection-lived ownership, pane input/resize, metadata, notification snapshots/acknowledgement and existing Vitals values. Native pane creation/termination, stable styled-history anchors, request correlation, MCP and device capabilities remain proposed. Current Vitals contain percentages and server-local clock strings; timestamps, explicit units and timezone-aware conversion require additional fields.

Initial v1 records use FIFO transport queues and may be large. The future device-request/attachment phase must introduce bounded bulk chunks and scheduling before claiming responsive control delivery under heavy transfer. Scheduling cannot remove TCP head-of-line blocking. Multiple SSH channels on the same authenticated connection remain an option if profiling justifies separate flow-control windows; no extra authentication or publicly listening port is implied.
