# ADR 0002: Four first-class clients and browser SSH

- Status: Proposed. Client targets are recorded here; implementation stacks and browser SSH authentication require validation.
- Date: 2026-10-27 (America/Los_Angeles).
- Related: [ADR 0001: Native clients, independent panes and device capabilities](0001-native-client-protocol-and-device-capabilities.md).
- Scope: macOS, iPadOS, iOS and web clients for the same persistent Kelpmux workspaces.

## Context

Kelpmux currently provides a terminal frontend and an experimental independent-pane stream. ADR 0001 defines the shared server, pane ownership, terminal synchronization and future device-capability boundaries. It does not choose the complete set of graphical clients or explain how a browser reaches an SSH server.

We want four first-class clients. Each should expose persistent terminals, agent status, notifications and remote metrics through interactions appropriate to its device. A browser client should have an HTML interface, a portable terminal core compiled to WebAssembly, selectable terminal text and hardware-key authentication compatible with the SSH model.

These are proposed clients, not shipped applications. The [implemented stream contract](../native-client-protocol.md) remains the source of truth for available server operations.

## Decision

Build four client experiences on one application protocol and an authoritative remote terminal server:

| Client | Presentation and interaction | Connection and authentication target |
| --- | --- | --- |
| macOS | Native desktop interface, multiple windows, keyboard and pointer, grids and tabs. | Direct SSH command channel; device-local Secure Enclave-backed signing as required by ADR 0001. |
| iPadOS | Tablet layouts, touch selection, hardware keyboard and pointer, multitasking and changing window sizes. | Direct SSH command channel; the same native key requirements. |
| iOS | Focused small-screen layouts, touch selection, software and hardware keyboards, explicit reconnect after suspension. | Direct SSH command channel; the same native key requirements. |
| Web | Responsive HTML/CSS application interface and a WASM terminal client connected to a Canvas/WebGL renderer. | Browser-terminated SSH over a WSS byte relay; separately enrolled WebAuthn hardware-key credentials. |

iOS and iPadOS may share a universal application and substantial code. They retain separate layouts and acceptance criteria. Four clients does not require four independent implementations of protocol, screen state or synchronization.

### Shared core and platform boundaries

The server retains PTYs and Alacritty as the authoritative terminal emulator. Clients consume decoded cells, cursor state and input modes. Compiling a client core to WASM does not imply porting the whole server or running a second emulator that could duplicate terminal-query responses.

Extract browser-safe Rust components for protocol models, snapshot/delta validation, screen state and input encoding. Share selection algorithms where practical. Native bindings and WASM bindings adapt that core to platform interfaces; the binding technology and renderer library remain implementation choices to validate. Platform code owns layout, fonts, accessibility, clipboard integration, focus and device permissions.

All clients preserve ADR 0001's epochs, pane identity, bounded queues, explicit input/size claims and reconnect semantics. Observing a pane never silently takes ownership or resizes its PTY. Closing a local view and terminating a remote pane remain separate operations.

### Browser transport and SSH trust

Use a normal HTTPS application with a WSS-to-TCP byte relay. A browser SSH implementation terminates SSH locally and opens one exec channel running `kelpmux bridge --workspace NAME` without an outer PTY. The existing framed application protocol travels inside that channel. The relay forwards SSH bytes and does not hold the user's long-term SSH private key.

Standard cross-browser pages cannot open arbitrary TCP sockets. Chrome's Direct Sockets capability is restricted to installed Isolated Web Apps and is not the baseline for this client. See [Chrome's Direct Sockets documentation](https://developer.chrome.com/docs/iwa/direct-sockets).

The browser must verify the target SSH host key before authenticating. Enrollment must make the initial host-key trust decision explicit; a changed key must not be silently accepted. Pinning persistence and recovery are part of the browser-client design.

The relay must have configured SSH destinations, access control, origin validation and bounded connection/traffic queues. It must not become an unrestricted TCP proxy. Start with private/VPN access; exposing a public service requires a separate deployment decision. This adds an HTTPS/WSS endpoint for web access while preserving the existing direct-SSH path for native clients.

The web origin delivering JavaScript and WASM is a trusted component: a compromised frontend can observe terminal contents and input even when the relay only sees SSH ciphertext. Browser-terminated SSH does not remove that distribution trust boundary.

### Hardware-key authentication from the browser

Validate WebAuthn-backed ECDSA/P-256 SSH authentication before committing to a browser SSH library. OpenSSH defines the signature algorithm `webauthn-sk-ecdsa-sha2-nistp256@openssh.com` specifically for browser WebAuthn assertions. Library support for the required signing callback and wire format must be demonstrated, not inferred from generic SSH or WebAuthn support. See [OpenSSH's FIDO/WebAuthn protocol](https://github.com/openssh/openssh-portable/blob/master/PROTOCOL.u2f).

Enroll a separate browser credential scoped to the application's relying-party domain and register its corresponding SSH public key on the target host. Do not assume the browser can reuse an existing `ssh:` FIDO credential or an Apple Secure Enclave SSH key. WebAuthn credentials are relying-party scoped; OpenSSH's ordinary FIDO application option requires an `ssh:` prefix. See the [WebAuthn specification](https://www.w3.org/TR/webauthn-3/) and [ssh-keygen manual](https://man.openbsd.org/ssh-keygen.1).

The proof must cover challenge construction, origin/application handling, public-key conversion, user verification, cancellation and key revocation/recovery. A physical YubiKey-style authenticator is the intended flow. Supporting WebAuthn alone does not prove a credential is hardware-bound or non-exportable; enrollment and any attestation policy must state what is actually enforced.

WebAuthn login to a gateway that opens SSH or attaches a local workspace is a possible alternative. It gives the gateway a different authority over sessions and credentials. It is not a silent fallback if browser SSH proves impractical; adopting it requires revisiting this decision.

### Selection, accessibility and lifecycle

Terminal text must remain selectable and copyable. A Canvas/WebGL renderer needs selection backed by terminal cells and history rather than ordinary DOM text selection. Selection must handle wrapped logical lines, wide characters, grapheme boundaries, scrollback, keyboard navigation and touch handles. Copy returns logical text, not a screenshot. HTML application controls retain normal browser behavior where appropriate.

The renderer also needs an accessibility representation for screen readers. Clipboard operations must respect platform permissions and user gestures, with an explicit unavailable state. Styled history with stable anchors remains a shared server prerequisite for reliable selection across paginated scrollback; the current inspection offsets are not durable anchors.

Device requests follow ADR 0001's negotiated capabilities and explicit approvals. Web capabilities need not equal native capabilities. Mobile suspension and browser background throttling require reconnect and fresh state; neither background execution nor push delivery is assumed. Reconnect must not replay unacknowledged destructive actions or device requests.

## Implementation sequence and acceptance

1. Prove browser SSH authentication with a real physical security key over the WSS relay, including host-key validation, cancellation and unsupported-browser behavior. Select compatible SSH and rendering components only after that proof.
2. Extract the portable client core and finish the shared APIs needed for complete clients: pane creation/naming/termination, stable styled-history pagination and public protocol/versioning guarantees. Experimental stream v1 is a foundation, not the finished client contract.
3. Build thin clients for all four targets against that shared contract. Validate Apple builds through an approved build pipeline and test on real macOS, iPadOS and iOS devices, plus the supported browser/security-key combinations.
4. Exercise simultaneous viewing, explicit takeover, independent pane geometry, reconnect after suspension/server restart, bounded slow-client behavior, Unicode selection/copy, agent status, notifications and metrics. Workspace processes must survive ordinary client disconnects.

The ADR remains Proposed until the authentication proof and client-core feasibility checks resolve the architectural uncertainties. This document does not select packages, deploy a relay, enroll credentials or declare the server-side API complete.

## Consequences and remaining choices

One server and shared protocol prevent client-specific terminal behavior and duplicated agent polling. Platform interfaces can evolve independently while preserving workspace identity. Four experiences still require four sets of interaction and accessibility validation; WASM does not remove that work or guarantee rendering performance.

Follow-up decisions must select Apple bindings, the browser SSH stack and renderer, supported browsers and hardware-key transports, the web origin/relying-party domain, credential enrollment/recovery, host-key storage and relay hosting. The implementation must preserve the native Secure Enclave workflow while introducing the separate browser hardware-key flow.
