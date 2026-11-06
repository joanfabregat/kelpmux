# Native client foundation

Kelpmux owns long-running PTYs and Alacritty terminal interpretation. Clients own presentation. Native mode has no tmux or WezTerm runtime dependency. The composed ANSI frontend is protocol v58; the separate experimental pane stream remains v1. Pane metadata adds a default-false `parked` field. Parking leaves PTY contents, status and stream subscriptions alive; restoring a pane returns it to the composed UI. A matching frontend/server pair is required because mouse routing shares their UI geometry and dialog types. Pane streaming shipped in beta.3; the updated composed status geometry requires a current dev build. A graphical desktop/mobile client and MCP/device tools are not implemented.

The broader proposed contract is [ADR 0001](adr/0001-native-client-protocol-and-device-capabilities.md). It covers pane lifecycle, independent device sizes, live agent status, notifications, remote Vitals and a future MCP/device request channel. The ADR deliberately distinguishes those proposals from the implemented requests below.

The browser client and authenticated management adapter live in KelpHub. Shared portable types and terminal client state live in [KelpClient](../../client/README.md). Desktop/mobile implementation choices remain deferred. The composed-screen TUI compatibility implementation is retained; see [MIGRATION.md](../../../MIGRATION.md) for current component ownership.

Screen input modes include an additive, default-zero `keyboard_flags` bitfield using the five Kitty keyboard enhancement flags. The composed frontend negotiates those flags with the outer terminal while retaining key disambiguation for its own controls. Native clients must preserve modified Enter events and distinguish key release from key press. Pane snapshots preserve default foreground/background colors so the attached terminal supplies its theme. Explicit application colors and OSC palette changes remain pane-local.

The composed frontend queries OSC 10/11 and sends bounded `ThemeColor { background: bool, rgb: [u8; 3] }` reports after attachment. These are metadata, never PTY input, and work independently of active dialogs. The server retains each attached client's reports and applies the TUI controller's colors as emulator query fallbacks except where a stream lease supplies its own theme. Application OSC colors take priority, while default snapshot cells remain `Default`. An unsupported or silent terminal retains documented emulator fallbacks. Theme reports do not force an existing application to repeat its startup probes or replace explicit application RGB colors.

## SSH command channel

`kelpserver bridge --workspace NAME` relays bounded native-stream records between standard input/output and the existing owned private Unix socket. It does not start a workspace or attach an ANSI frontend. A future native client opens it through one authenticated SSH command channel without an outer PTY:

```sh
ssh -T HOST kelpserver bridge --workspace NAME
```

Use the installed binary's absolute path if it is not in the remote noninteractive PATH. SSH supplies authentication, encryption and transport; there is no additional public port. Shell banners must not appear on stdout. Bridge diagnostics go to stderr, and a TTY endpoint is rejected before terminal-mode changes. EOF detaches that connection and releases its leases, preserving workspace processes. Unacknowledged requests are not automatically replayed. The bridge does not provide mobile roaming or background push delivery.

## Framing and negotiation

Every record has a four-byte unsigned big-endian payload length followed by UTF-8 JSON, with a 16 MiB maximum payload. JSON examples below are payloads, not newline-delimited wire records. Use the same framing over the local Unix socket or SSH bridge.

```json
{"Stream":{"type":"hello","version":1}}
```

The server responds with `Stream` / `welcome`, containing stream version, connection/client ID, a random server epoch, and supported capabilities. Send Hello exactly once before other stream requests. Legacy TUI/management messages are rejected on a negotiated stream connection and by the bridge. Unsupported versions fail clearly. Current capabilities are `pane-snapshots`, `pane-patches`, `pane-leases`, `metadata`, `notifications`, `vitals`, `hyperlinks`, `program-status` and `controller-theme`; do not infer support for undeclared future capabilities.

Pane IDs are scoped to the Welcome epoch. A new epoch invalidates previous screen baselines and ownership. Clients must negotiate anew, obtain snapshots, and deliberately rebuild desired control state rather than replaying old mutations. Experimental v1 does not yet carry correlation IDs or an expected epoch on every mutation; these remain part of public-contract maturation in the ADR.

## Pane subscriptions and updates

```json
{"Stream":{"type":"subscribe","panes":[1,2]}}
```

Subscribe replaces the complete subscription set. Updates contain an authoritative `subscribed` list, full metadata for current panes, leases, notifications, Vitals, and snapshots or patches for subscribed terminal screens. Metadata-only subscriptions use an empty list. The reference client prunes cached screens according to each received update's subscription list, safely processing older in-flight updates before a replacement set arrives.

A full pane snapshot establishes revision 1. A patch names the pane, base revision and resulting revision; it applies only to that exact pane baseline in the current server epoch. Geometry changes, explicit `resync` and backpressure can produce replacement snapshots. Snapshots contain cells with colors/attributes/widths, cursor and input modes; they do not contain native headers, tabs or footer cells. Alacritty remains the only emulator answering application terminal queries.

There are at most ten subscribed panes and 100,000 aggregate subscribed screen cells per connection. Oversized subscriptions are rejected. A later size change exceeding an observer's aggregate budget disconnects that observer instead of retaining unbounded data. The server retains one in-flight packet and one replaceable aggregate update. The bridge has separate bounded queues in each direction and yields between decode batches. A permanently slow client can be disconnected and resynchronize on reconnect.

## Independent ownership and size

`claim {pane,takeover}` explicitly acquires one pane; `release {pane}` relinquishes it. `resize {pane,cols,rows}`, `input {pane,bytes}`, `paste {pane,bytes}` and `mouse {pane,event}` require the caller's lease. Input/paste are bounded to 64 KiB. Paste honors the pane's bracketed-paste mode; native mouse events currently support SGR encoding. An observer cannot write or resize a pane.

The optional `controller-theme` capability adds `theme {pane,foreground:[r,g,b],background:[r,g,b]}` with byte-valued RGB channels. It requires the caller's pane lease and sets that pane's OSC 10/11 query fallbacks. The browser reports its theme after claiming and when its appearance changes. Takeover or release drops the old lease theme, restoring the next controller's colors. Do not send this request to retained servers that omit the capability.

Client A can own pane 1 at 240 × 70 cells while client B owns pane 2 at 40 × 12 cells. Native pane sizes permit 2–400 columns and 1–160 rows, independently of the ANSI frontend's 40 × 10 minimum window. Each pane still has exactly one underlying PTY size. Observers of that same pane clip/scale locally and cannot impose a competing size.

Leases last until release, takeover or disconnect; release/disconnect restores the current base workspace geometry. Stream clients issue explicit Claim messages, which a UI may send automatically on focus or input. The active attached TUI controller reclaims a pane when the user types, pastes, focuses it or interacts with its mouse; background resize, mouse movement and focus-out never steal a lease. The browser retains its last geometry and claims on blur or document hiding; actual foreground input or another explicit claim can still take over. Release is idempotent for an already-free pane, including a blur racing a TUI reclaim. Safe grid shrink excludes leased panes. `kelpmux ctl --workspace NAME reclaim ID` remains an explicit administration shortcut. Attached TUI observers cannot bypass control using management messages. Workspace administration and explicit shutdown remain separate same-user local operations; this is not isolation against hostile processes running as that user.

## State, notifications and Vitals

The `program-status` capability adds `terminal` observation evidence from pane-local OSC 7501 reports. Current dev clients must recognize this status-source value; the experimental stream remains v1 and is not a frozen compatibility contract. The composed frontend/server pair uses v58. Terminal activity takes precedence over metadata polling without implying that a conversation ID is known.

Pane metadata carries identity, agent kind, session ID if known, status, activity times and status observation evidence. Evidence distinguishes process, lifecycle hook, live record, agent-server and transcript observations. `checked_at` records a check; optional `event_at` records an actual known event time. Reading an old transcript again does not invent a new lifecycle event. Unknown or stale evidence remains visible in the overview, and an idle observation does not become a completed turn automatically.

Updates include the latest 128 retained notifications and explicit `acknowledge {notification}`. This is a bounded snapshot of notification history, not exactly-once event delivery or a question/answer RPC. Existing seven-day retention and persisted unread state remain. No native device notification or push service is invoked.

Vitals are typed values from the existing in-process sampler, independent of footer styling: CPU, memory, swap and disk percentages, plus server-local date/time strings. Missing values stay unavailable. Sample timestamps, byte totals, mount labels, timezone-aware conversion and richer metrics are proposed additions; clients must not treat the current clock strings as timezone-independent timestamps.

## Reference client and inspection

The Rust `native::reference::State` validates welcome/epoch, membership, screen/cell bounds, leases and patch baselines before committing changes. Failed updates leave previous baselines intact. `stream-inspect` is a read-only one-shot reference client with an absolute operation deadline:

```sh
kelpserver stream-inspect --workspace NAME
kelpserver stream-inspect --workspace NAME 1 2
```

With no pane IDs it returns metadata only; explicit IDs add those panes' terminal cells to JSON stdout. It does not claim, resize, become the TUI controller, start a server or persist terminal contents. Automated tests use actual bridge subprocess pipes, disposable PTYs and typed reference state; they do not open a real SSH login or access a native device.

The older `InspectPane` and bounded `History {pane,start,limit}` requests remain local inspection seams. `SearchPane {pane,query,limit}` returns `PaneSearch {pane,hits,truncated}` with case-insensitive literal occurrences; queries must contain 1–256 bytes without control characters and the limit is at most 64 matches. Each match carries its source buffer, immutable context rows, and inclusive cell coordinates relative to that context. This prevents ongoing output or alternate-screen transitions from moving the target between search and reveal. Context text has a combined 4 MiB budget; `truncated` indicates that a match or context budget stopped the search. Search does not change another client's viewport or input ownership. Ordinary history pages still use relative line indices and plain text; stable, styled native-history pagination is not implemented. Native pane create/rename/terminate commands, device capability requests, attachments, MCP, native apps and final public-protocol compatibility guarantees remain future work described in ADR 0001.

### Hyperlink cells

The server's `hyperlinks` capability indicates support for an optional `hyperlink` URI string per cell. Missing fields mean no target; explicit targets and recognized plain HTTP(S) URLs are carried independently of visible text. Targets are bounded to 2,048 bytes, require a URI scheme and reject whitespace and control characters. Deltas include target-only changes. Native clients open links through their own user interaction rather than requesting browser execution on the server. Older cell encodings remain decodable.
