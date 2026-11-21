# Web client

KelpMux includes an optional browser client for the same persistent workspaces as the TUI. It is terminals only, with the TUI's feature set adapted to the browser (see [Feature parity with the TUI](#feature-parity-with-the-tui)): a left sidebar of terminals with agent state, a live grid, parking, agents, notifications, recently closed terminals, overview, search, selection and copy, links, and tab colours, keyboard scroll and selection modes, settings for appearance, alerts, keyboard shortcuts and security keys. It has no projects, file browser, editor, Git views or uploads; those belong to KelpHub. [ADR 0003](adr/0003-web-client-in-kelpmux.md) records why it lives here and what it must not become.

The browser signs in with a hardware security key (WebAuthn) and runs OpenSSH itself, compiled to WebAssembly. The SSH session ends in the browser; nothing on the server side ever holds the session keys or a private key.

## Components

```text
browser ── HTTPS + WSS ──▶ kelpmux web-relay ── TCP ──▶ sshd ──▶ kelpmux web-bridge ──▶ workspace socket ──▶ KelpServer
 (React UI, OpenSSH/WASSH,      (static assets,              (user's          (stdio, same-user        (PTYs, emulation,
  WASM terminal core)            byte relay)                  account)         peer check)               processes)
```

- `web/`: the React/TypeScript browser client. The reusable terminal pieces are in `web/src/terminal/` (see [Reusable terminal component](#reusable-terminal-component)); the rest of `web/src` is the KelpMux application shell around them.
- `kelpclient` (`crates/client`), compiled to `wasm32-unknown-unknown` and served as `client-core.wasm`: stream framing, validation, history layout and selection shared with native Rust clients.
- `kelpmux web-relay --config PATH`: serves the embedded static assets and forwards WebSocket binary bytes to one operator-configured SSH destination. It has no key enrollment API, holds no SSH private key, never terminates SSH and has no code path to a workspace socket. Before forwarding the server's first SSH reply it checks that the host key matches the configured `ssh_host_key`; browser OpenSSH verifies the same key and its signature independently.
- `kelpmux web-bridge --workspace NAME`: run by OpenSSH as the authenticated Linux user, without a PTY. It connects to that workspace's existing private socket, checks that the peer is a process of the same user and stays bound to that process, then relays the independent pane stream plus a small set of request-correlated operations. It never listens on TCP and never stops, restarts or replaces a workspace server. If its workspace's server is not running (no socket, or a socket nothing listens on), it tells the browser, which shows a **Start workspace** button; only that explicit click sends `start`, which runs the same code path as `kelpmux start --workspace NAME` for the bridge's own configured workspace and nothing else (no workspace name, server binary or replacement comes from the browser), and is refused while a server is running. The bridge then connects as usual. KelpServer removes agent and multiplexer session markers from the environment of the server it starts and of every pane shell (see [native.md](native.md)).

The `Web` operations the bridge accepts (API version 2) are `state`, `new` (a shell in a new pane, optionally in a given absolute directory), `close` (explicitly confirmed), `search`, `history`, `status` (system status and processes), `color` (set a terminal's tab colour, a TUI palette index 0–8, through the server's existing `TabColorSet` action), `closed` (the recently closed terminals and previous runs the TUI's Recovery lists, with their restore choices) and `restore` (explicitly confirmed: resume an agent conversation, rerun a command or open a shell in the original folder, in a new pane through the idempotent `pane-spawn` request) and `start` (only while the workspace server is stopped; see above). While stopped, the bridge answers the stream `hello` with a `{"Workspace":{"running":false,"workspace":NAME}}` notice and accepts only `state` and `start`. Stream requests, notification acknowledgements included, are forwarded unchanged; read marks are shared with the TUI. Every other method is refused with `unsupported web operation`. A version 2 web build refuses a version 1 bridge (and the reverse) with a message asking for matching versions.

## Security boundaries

- Authentication is SSH public-key authentication by the host's OpenSSH server. There is no web password, cookie or session token. Each new SSH connection authenticates again.
- A key authorized for a Linux account grants that account's terminals, which run commands with that user's authority. Operators who want the key limited to the web bridge can authorize it with `restrict,verify-required,command="/path/to/kelpmux web-bridge --workspace NAME"`; this prevents unrelated SSH commands and forwarding, but a shell in a pane is still a shell.
- Run the relay under a separate restricted service identity (or an equivalent container) with no access to user home directories, credentials, workspace sockets or other processes. Putting the relay and the bridge in one executable is packaging, not isolation.
- The origin that serves the JavaScript and WebAssembly is trusted: a compromised frontend can read terminal contents and input even though the relay only sees SSH ciphertext. Serve it over HTTPS only, from a trusted build.
- The relay sets cross-origin isolation headers (required by WASSH), a restrictive Content-Security-Policy, `nosniff` and `no-referrer`, checks the `Origin` of WebSocket upgrades, and bounds headers, frames, buffers, connections and time.
- Browser storage holds only public data: credential handles, public keys, SSH fingerprints, key names/icons/colours, appearance and this browser's layout. No private key, SSH transport state or token is stored, exported or shared with other pages.
- The upstream OpenSSH WASM build (`0.82`, OpenSSH 10.6 with OpenSSL 1.1.1w) and the WASSH JavaScript are fetched at pinned, checksum-verified revisions. RustSec and npm audits do not cover the compiled libraries inside that archive; treat the legacy OpenSSL copy as needing its own review before exposing the client beyond a private network.

## Build

The relay serves assets embedded at compile time from `web/dist`. A checkout without `web/dist` builds and runs the TUI and server normally; `kelpmux web-relay` then refuses to start with an explicit "no web client assets" error.

Prerequisites: Rust with the `wasm32-unknown-unknown` target and Node 24. Keep dependency fetching and auditing separate from building, and run builds in your usual isolated environment:

```sh
# 1. Portable terminal core for the browser.
cargo build --locked -p kelpclient --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/kelpclient.wasm web/public/client-core.wasm

# 2. Pinned WASSH and OpenSSH WASM (download and verify only; executes nothing).
bash scripts/fetch-web-ssh.sh

# 3. Locked JavaScript dependencies, without install scripts, then audit.
(cd web && npm ci --ignore-scripts && npm audit --audit-level=high)

# 4. Test, type-check and build the static client.
(cd web && npm test && npm run check && npm run build)

# 5. Build kelpmux, which embeds web/dist.
cargo build --locked --release --bin kelpmux
```

Build the web assets before compiling the executable that embeds them; rebuild `kelpmux` after every web change. `node_modules`, `web/dist`, `web/public/vendor` and `web/public/client-core.wasm` are generated and ignored by Git.

Browser-level tests of the terminal component (WebGL/Canvas renderer, inset geometry, trusted copy events) run with Playwright in Chromium and WebKit and need no network: `node test/renderer.mjs` and `node test/terminal.mjs` in `web/`, plus `node test/app.mjs` after `npm run build` (the built client boots cross-origin isolated to the sign-in page with its worker and WASM core), using the `mcr.microsoft.com/playwright:v1.64.0-noble` image or locally installed Playwright browsers. Rust integration tests drive `kelpmux web-bridge` over real pipes against a disposable server (`cargo test --test native_web_bridge`).

## Relay configuration

Start the workspace with the ordinary launcher (`kelpmux start --workspace main`), make sure OpenSSH accepts key logins for the account, then run the relay with a JSON configuration (see [examples/web-relay.json](../examples/web-relay.json)):

```json
{
  "enabled": true,
  "listen": "127.0.0.1:8080",
  "origin": "https://terminal.example.com",
  "ssh_target": "127.0.0.1:22",
  "ssh_host_key": "ssh-ed25519 AAAA…",
  "username": "alice",
  "bridge_command": "/opt/kelpmux/current/kelpmux web-bridge --workspace main",
  "workspace": "main",
  "dev": false
}
```

```sh
kelpmux web-relay --config /etc/kelpmux/web.json
```

- `enabled` must be `true`; the relay refuses to start otherwise.
- `listen` is the local address; put a TLS-terminating reverse proxy with WebSocket support in front of it.
- `origin` is the exact HTTPS origin users open. Its host name is also the WebAuthn relying-party ID, so keys registered for one origin do not work on another.
- `ssh_target` is the one SSH destination the relay may reach. `ssh_host_key` is that server's Ed25519 public key (algorithm and base64, from `/etc/ssh/ssh_host_ed25519_key.pub` obtained through a trusted path), not a fingerprint and never a private key.
- `username` is the Linux account; `bridge_command` is the exact command the browser asks OpenSSH to run (use absolute paths: a noninteractive SSH session may have a minimal `PATH`, and shell startup files must not print to standard output). `workspace` names the workspace for display.
- `dev` only adds a DEV badge to the interface.

Hosting is the operator's responsibility; nothing installs a service. Start with private or VPN-only access.

## Registering a YubiKey

1. Open the HTTPS origin. On the sign-in page choose **Register a new key**, then **Register new key**, and touch the key. This creates a discoverable ECDSA P-256 credential with required user verification, scoped to this origin. It grants nothing on its own.
2. Copy the displayed OpenSSH public key (`sk-ecdsa-sha2-nistp256@openssh.com …`) and add it to the account's SSH authorization through your usual mechanism (for example `~/.ssh/authorized_keys`, optionally prefixed with `restrict,verify-required,command="…"` as described above). The web client never edits SSH authorization.
3. Choose **Sign in with security key** and touch the key. When this browser remembers the last key used, one touch signs the login. **Use another security key** (or a new browser) first asks the authenticator which credential to use; because WebAuthn assertions do not return the public key, the browser recovers the candidate public keys from that signature and offers them all to OpenSSH, which asks the server which one is authorized before requesting a second touch to sign. Nothing needs importing on a new browser.
4. If the server accepts none of the candidates, the page says the key is not authorized for the account.

Register and authorize each physical key separately. Settings → Security keys names keys, picks the one used for the next sign-in, shows public keys and exports or imports public registrations; these details stay in this browser. Removing a key there does not delete the credential from the authenticator or revoke its SSH authorization. Removing the SSH authorization stops new logins; it does not end existing SSH sessions.

## Using it alongside the TUI

The TUI and the web client are independent clients of the same KelpServer. You can use the TUI alone, the web alone, or both at once on the same panes and processes:

- Layout is per client. The TUI keeps its grid, tabs, parking and focus; each browser keeps its own view mode, terminal order, parked terminals, focused terminal, sidebar width and folding in `localStorage` (layout per workspace, discarded when the server restarts with a new epoch). Neither changes the other's layout. Parking in the browser hides a terminal from this browser's list and grid only; it keeps running and stays visible in the TUI. Panes parked in the TUI appear in the browser with a "parked in TUI" badge and stay parked in the TUI.
- Notification read marks are shared. Showing a terminal in the browser marks its notifications read, as focusing it does in the TUI, and the Notifications dialog's **Mark read** acknowledges them for every client. Sound and desktop alerts are this browser's own preferences (Settings → Notifications), off by default.
- Creating or closing a terminal is shared: a new pane appears in every client, and closing one (always confirmed) ends its processes for everyone.
- Input and size follow whichever client last used a pane. When a browser focuses a terminal or types into it, it claims that pane's input and size lease and resizes the PTY to its own view. When the TUI's controlling client then types, pastes, clicks or focuses that pane, the TUI takes it back and the pane returns to the TUI's geometry. The live grid is a foreground layout too: while the browser window is focused and visible, it sizes each tile's pane to that tile (claiming it), so opening the grid in a browser resizes the panes it shows until the TUI uses them again. Passive rendering in a background window, mouse movement, window blur and focus-out never take a pane. Each pane has exactly one PTY size; a client that does not hold the lease shows the pane clipped or padded. Different panes can be held by different clients at the same time.
- The status bar says whether this browser holds the focused terminal. Closing the browser or losing the connection releases its leases; terminals keep running and return to the TUI's geometry.
- Only one TUI client is the controller (input and workspace size); further TUI clients are observers, as before. Web clients are not TUI controllers or observers and do not affect that role.

Other same-origin tabs and **Open in a window** popups reuse the SSH connection held by the first tab through a SharedWorker; keep that tab open. The shared session locks after 15 minutes without deliberate input. **Log out all browser windows** (Settings → Connection) or `Ctrl-B d` closes SSH for every window of the browser; workspace processes keep running.

## Keyboard

`Ctrl-B` is the browser's prefix, and the keys after it follow the TUI's default bindings wherever the browser has the same action: `Enter` new terminal, `Tab`/`Shift+Tab` next/previous terminal, `1`…`9` terminal by number, `g`/`e` live grid, `f` full width (the focused terminal without the sidebar, the TUI's fullscreen), `h`/`j`/`k`/`l` or arrows move the grid focus, `PgUp`/`PgDn` grid pages, `N` next question, `P` next completion, `a` agents, `!` agents needing attention, `n` finished agents, `b` notifications, `o` overview, `r` recently closed, `z` reopen the last closed terminal, `/` search terminal history, `[` scroll mode, `Space` selection mode, `H`/`J`/`K`/`L` grow or shrink the focused grid column or row, `=` equal grid tiles, `p` park, `u` parked terminals, `s` move, `x` close (confirmed), `,` settings, `?` help, `d` disconnect, and `Ctrl-B` twice sends `Ctrl-B` to the terminal. Outside the prefix, `Alt+1`…`Alt+0` open terminals 1–10, `Alt+Tab` or `Ctrl/Cmd+Shift+G` toggle one terminal / live grid, and `Ctrl/Cmd+F` finds in the focused terminal. In scroll mode, `PgUp`/`PgDn`, `Ctrl-U`/`Ctrl-D`, `Ctrl-B`/`Ctrl-F`, `j`/`k` or arrows scroll, `g`/`G` jump to the oldest line or the live screen, `Space` or `v` switches to selection, and `Esc` or `q` leaves. In selection mode, `h`/`j`/`k`/`l` or arrows move the cursor (scrolling at the edges), `0`/`$` line start and end, `w`/`b` eight columns, `Space`/`v` set or clear the anchor, `y` or `Enter` copy and leave, `Esc` or `q` leave. Every other key goes to the focused terminal with full modifier and Kitty keyboard-protocol support. Settings → Keyboard and Help list these bindings. The TUI's own bindings and configuration file are unchanged; custom TUI bindings are not read by the browser.

The mouse covers the rest: click a sidebar row to open a terminal, drag rows (or grid tile headers) to reorder, right-click a row or tile for its menu (open, open in a window, find, move up/down, park/unpark, close), middle-click a row to close it, click the sidebar's state counts to jump to the next terminal in that state, and click or drag the sidebar's edge to fold or resize it.

## Feature parity with the TUI

The browser has the TUI's feature set adapted to web conventions. Status: **done** existed before this work; **added** is implemented in the parity branch; **deferred** gives the reason.

| TUI feature | Web equivalent | Status |
| --- | --- | --- |
| Left tab sidebar with numbers, tab colours, agent state marks, provider and folder | Sidebar terminal list in this browser's order with number, TUI tab colour (read-only), state mark, provider logo, title and folder | added |
| Sidebar resize and fold | Always-visible fold handle on the sidebar edge (also on the folded rail, to unfold it); drag or click the edge to resize; Settings → Notifications → Sidebar; saved per browser | added |
| Workspace state counts in the sidebar | State counts beside the workspace name; click to jump to the next terminal in that state | added |
| Vitals in the sidebar footer, status/process view | Sidebar footer meters (status bar while folded or full width); System status dialog with processes | added (dialog done) |
| Tabs view / reading one pane (`Ctrl-B 1`…`9`, `Tab`) | One-terminal view; `Ctrl-B` digits, `Tab`/`Shift+Tab`, `Alt` digits | done; TUI keys added |
| Fullscreen (`Ctrl-B f`) | Full width: focused terminal without the sidebar | added |
| Grid (`Ctrl-B g`/`e`), grid focus `h`/`j`/`k`/`l`, pages | Live grid with pages, focus ring and `h`/`j`/`k`/`l`/arrow focus | done; focus keys added |
| Grid swap by dragging | Drag tile headers to reorder | added |
| Grid tile weights (`Ctrl-B H`/`J`/`K`/`L`) | `Ctrl-B H`/`L` widen or narrow the focused column, `K`/`J` the focused row, `=` equalizes; per browser, reset when the grid shape changes | added |
| New tab, close tab (confirmed) | New terminal; close confirmed | done |
| Park / unpark, parked list (`p`, `u`) | Park from the row, tile header, menu or `Ctrl-B p`; collapsible Parked section and Parked dialog (`Ctrl-B u`) | added (per browser) |
| Move picker (`s`), drag reorder | Move dialog (`Ctrl-B s`), drag rows, menu Move up/down | added |
| Agents list with all / attention / finished filters (`a`, `!`, `n`) | Agents dialog with the same filters | added |
| Next question / next completion (`N`, `P`) | Same keys; also the sidebar state counts | added |
| Attention markers and finished markers | State marks in rows, tiles, top bar; tab title count | added |
| Notifications list, acknowledge, acknowledge all, history (`b`) | Notifications dialog: unread / all, mark read, mark all read, open | added |
| Notification sound toggle | Sound and desktop alerts per browser | added |
| Focusing a pane marks its notifications read | Showing a terminal acknowledges its unread notifications | added |
| Scratch popup (`c`) | None; a TUI scratch pane shows as an ordinary terminal | removed (owner decision): the web client has no scratch popup |
| Recovery: recently closed and previous runs, copy command, reopen / rerun / folder (`r`) | Recently closed dialog with copy and resume / rerun / folder | added |
| Reopen last closed (`z`) | `Ctrl-B z` | added |
| Workspace search with focused / all scope (`/`) | Search dialog (Tab toggles scope); a hit opens the terminal with find prefilled | added |
| Find in pane | `Ctrl/Cmd+F`, find bar | done |
| Overview (`o`) | Overview dialog (state, provider, conversation, folder, lease owner) | added |
| Help (`?`) | Help dialog with the bindings | added |
| Settings (`,`): accent, tab position, wrap, prefix, bindings | Settings: appearance, theme, font, alerts, sidebar, keyboard list, security keys, browser session | done; alerts added. TUI-only display settings (accent, tab position, wrap, custom prefix) do not apply to the browser |
| Selection mode (`Space`), scroll mode (`[`), copy (`y`) | Mouse selection and copy, Shift+drag, wheel and Shift+wheel history; keyboard scroll (`Ctrl-B [`) and selection (`Ctrl-B Space`) modes with `y` copy | added |
| Agent link picker (`i`) | None | not planned (owner decision) |
| Tab colour picker | Menu → Tab colour…, the TUI palette, shared with the TUI | added (existing `TabColorSet` action) |
| Detach (`d`) | Disconnect (`Ctrl-B d`) | done |
| Takeover (`t`), controller / observer | Per-pane input and size leases | not applicable: browsers never hold the TUI controller role |
| Templates, startup commands | None | deferred: a template is a `kelpmux start --template` option loaded into the workspace settings when the server starts; there is no runtime operation to apply one, so it does not fit a bridge operation cleanly |
| `ssh-entry` (start the workspace server if needed, then attach) | Sign in; if the workspace is stopped, **Start workspace** (explicit click) starts it the same way as `kelpmux start`, then attaches | added: never replaces a running server and starts only the bridge's configured workspace |
| Mobile layout | None | out of scope |

## Reusable terminal component

`web/src/terminal/` contains the pieces a different web application (KelpHub) can consume at a pinned KelpMux revision: SSH/WebAuthn connection and key discovery, the SharedWorker session broker, WASM client-core loading, the WebGL/Canvas renderer, keyboard, mouse and wheel encoding, selection and copy, the screen cache, grid column layout, the `Terminal`/`TerminalTile` React components and pane find. Nothing in that directory imports application code. It is not published to npm yet; consumers copy or vendor it at a fixed revision together with the matching `client-core.wasm` built from the same revision.

## Current limits

- Up to eight terminals are shown per grid page (the stream allows ten subscriptions per connection, shared with popups); larger workspaces page.
- Panes cannot be renamed or split from the web: KelpServer has no rename operation, and its layout is automatic. Pane titles follow the running program.
- History search uses the server's bounded literal search (64 matches per terminal); the Search dialog searches at most 32 terminals and shows at most 200 hits. Older history pages are plain text.
- Restoring a closed terminal from the browser does not remove it from the TUI's Recently closed list (the server owns that file); restoring an agent conversation that is already open opens the existing terminal instead.
- Notification state for a pane is matched by pane and process ID; the browser does not see the server's run identifier, so a notification from a previous server run whose pane and process IDs both recur could be shown until it is read.
- Real hardware-key and Safari behaviour need manual acceptance; automated tests use virtual authenticators only where noted.
