# Web client

KelpMux includes an optional browser client for the same persistent workspaces as the TUI. It is terminals only, with the TUI's feature set adapted to the browser (see [Feature parity with the TUI](#feature-parity-with-the-tui)): a left sidebar of terminals with agent state, a live grid, parking, a Terminals table (every terminal with its agent state, folder, Git repository and branch, running program and last activity), per-terminal notifications, recently closed terminals, search, selection and copy, links, and tab colours, keyboard scroll and selection modes, [file and image uploads](#uploads) dropped onto a terminal, settings for appearance, alerts, keyboard shortcuts, uploads and security keys. It has no projects, file browser, editor, or Git views or operations; those belong to KelpHub. (The Terminals table only shows, read-only, which repository and branch a terminal's folder is in, with a link to GitHub.) [ADR 0003](adr/0003-web-client-in-kelpmux.md) records why it lives here and what it must not become.

The browser signs in with a hardware security key (WebAuthn) and runs OpenSSH itself, compiled to WebAssembly. The SSH session ends in the browser; nothing on the server side ever holds the session keys or a private key.

## Components

```text
browser ── HTTPS + WSS ──▶ kelpmux web-relay ── TCP ──▶ sshd ──▶ kelpmux web-bridge ──▶ workspace socket ──▶ KelpServer
 (React UI, OpenSSH/WASSH,      (static assets,              (user's          (stdio, same-user        (PTYs, emulation,
  WASM terminal core)            byte relay)                  account)         peer check)               processes)
```

- `web/`: the React/TypeScript browser client. The reusable terminal pieces are in `web/src/terminal/` (see [Reusable terminal component](#reusable-terminal-component)); the rest of `web/src` is the KelpMux application shell around them.
- `kelpclient` (`crates/client`), compiled to `wasm32-unknown-unknown` and served as `client-core.wasm`: stream framing, validation, history layout and selection shared with native Rust clients.
- KelpServer comes from the [kelpserver](https://github.com/joanfabregat/kelpserver) repository at the revision pinned in `Cargo.toml`; `client-core.wasm` must be built with `kelpclient-protocol` from that same KelpServer revision. KelpMux requires KelpServer 0.3 or later, which speaks only the pane stream (no composed protocol 58) and starts a workspace without panes: the first client creates them. The web relay is KelpMux's own (`src/native/web_relay.rs`); KelpServer removed its copy in 0.3.0-beta.2. Since that release (`kelpclient-protocol` 0.3.0) the pane list has no `number` or `parked`: the browser numbers terminals by their position in its own order (`numberPanes` in `web/src/layout-store.ts`), so without a stored order the numbers are the creation order, as before, and it keeps parking in its layout store.
- `kelpmux web-relay --config PATH`: serves the embedded static assets and forwards WebSocket binary bytes to one operator-configured SSH destination. It has no key enrollment API, holds no SSH private key, never terminates SSH and has no code path to a workspace socket. Before forwarding the server's first SSH reply it checks that the host key matches the configured `ssh_host_key`; browser OpenSSH verifies the same key and its signature independently.
- `kelpmux web-bridge --workspace NAME`: run by OpenSSH as the authenticated Linux user, without a PTY. It connects to that workspace's existing private socket, checks that the peer is a process of the same user and stays bound to that process, then relays the independent pane stream plus a small set of request-correlated operations. It never listens on TCP and never stops, restarts or replaces a workspace server. If its workspace's server is not running (no socket, or a socket nothing listens on), it tells the browser, which shows a **Start workspace** button; only that explicit click sends `start`, which runs the same code path as `kelpmux start --workspace NAME` for the bridge's own configured workspace and nothing else (no workspace name, server binary or replacement comes from the browser), and is refused while a server is running. The bridge then connects as usual. KelpServer removes agent and multiplexer session markers from the environment of the server it starts and of every pane shell (see [native.md](native.md)).

The `Web` operations the bridge accepts (API version 3, minor 1) are `state`, `new` (a shell in a new pane, optionally in a given absolute directory), `close` (explicitly confirmed), `search`, `history`, `status` (system status and processes), `closed` (the server's closed-session records the TUI's Recovery lists: recently closed terminals and previous runs, with their restore choices) and `restore` (explicitly confirmed: resume an agent conversation, rerun a command or open a shell in the original folder, in a new pane through the server's `restore`), `repos` (see below), `commands` (each live pane's foreground program, keyed by pane id: the base name of the first argument of its terminal's foreground process-group leader, or `null` when the pane's own process is in front, such as a shell at its prompt; process identities come from the server, never the browser, and arguments are never returned because they can hold secrets; at most 128 panes) and `start` (only while the workspace server is stopped; see above). Each operation that reaches the server uses KelpServer's stream session API on a short-lived connection of its own, checked to reach the same server process and epoch as the relayed stream: `new` is `spawn` (idempotent per browser request identity), `close` takes the pane's lease and sends `close`, `search` and `history` are the stream requests of the same names, `closed` is `closed_sessions`, `restore` is `restore` for a record of the server's latest list, and `commands` reads the pane list of the first update. While stopped, the bridge answers the stream `hello` with a `{"Workspace":{"running":false,"workspace":NAME}}` notice and accepts only `state` and `start`. Stream requests, notification acknowledgements included, are forwarded unchanged; read marks are shared with the TUI. Every other method is refused with `unsupported web operation`. A web build refuses a bridge of another API version (and the reverse) with a message asking for matching versions; both ship in the same `kelpmux` binary. Version 3 removed `color`: tab colours are each client's own state ([ADR 0002](https://github.com/joanfabregat/kelpserver/blob/main/crates/server/docs/adr/0002-generic-session-server.md), decision 144b) and the browser keeps them with its layout. Operations added within a version are additive: `state` reports `api_minor` (1 in version 3) and its `operations` list, and the web client uses an optional operation (`repos`, `commands`, `uploads`) only when that list names it, so an older bridge of the same version still serves a newer web build with those columns left plain and uploads off. Minor 1 added `uploads`, which is not a `Web` method: it says the bridge relays KelpServer's upload stream requests (`upload_begin`, `upload_chunk`, `upload_finish`, `upload_cancel`) and their events like any other stream request, unchanged, on the relayed connection (a bridge of minor 0 cannot parse them and would end the session). See [Uploads](#uploads).

`repos {"paths": [...]}` takes pane working directories (which the browser already has) and returns `{"repos": [...]}` in the same order, each entry `{"root", "branch", "detached", "github"}` or `null`. `root` is the directory holding `.git`; `branch` is the checked-out branch (`refs/heads/` removed); `detached` is the first twelve hex digits of a detached HEAD; `github` is `owner/repo` when the preferred remote (`origin`, else the first remote in the config) is on GitHub in its `https://`, `ssh://` or `git@github.com:` form. Credentials in a remote URL and non-GitHub remote URLs are never returned; such a repository shows its root without a link. The bridge computes this itself; KelpServer is not involved. It only reads files and never runs `git` or any other process: `.git` (a directory, or a file whose `gitdir:` names a linked worktree's or submodule's Git directory), `HEAD`, a worktree's `commondir` and the shared `config`. It is bounded and conservative: at most 64 paths per call (more is an error), only absolute paths without `.` or `..` components that are directories owned by the bridge's user, at most 32 directories walked up from each, Git metadata owned by another user or reached through a symlinked `.git`, `HEAD` or `config` is ignored, files are opened without following symlinks and without blocking (a FIFO cannot stall the bridge), `HEAD`, `.git` and `commondir` above 4 KiB and a config above 64 KiB are not parsed (the repository is still reported, without a remote), and config `include`s and `insteadOf` rewrites are not followed. Results are cached for one call only.

## Security boundaries

- Authentication is SSH public-key authentication by the host's OpenSSH server. There is no web password, cookie or session token. Each new SSH connection authenticates again.
- A key authorized for a Linux account grants that account's terminals, which run commands with that user's authority. Operators who want the key limited to the web bridge can authorize it with `restrict,verify-required,command="/path/to/kelpmux web-bridge --workspace NAME"`; this prevents unrelated SSH commands and forwarding, but a shell in a pane is still a shell.
- Run the relay under a separate restricted service identity (or an equivalent container) with no access to user home directories, credentials, workspace sockets or other processes. Putting the relay and the bridge in one executable is packaging, not isolation.
- The origin that serves the JavaScript and WebAssembly is trusted: a compromised frontend can read terminal contents and input even though the relay only sees SSH ciphertext. Serve it over HTTPS only, from a trusted build.
- The relay sets cross-origin isolation headers (required by WASSH), a restrictive Content-Security-Policy, `nosniff` and `no-referrer`, checks the `Origin` of WebSocket upgrades, and bounds headers, frames, buffers, connections and time.
- Browser storage holds only public data: credential handles, public keys, SSH fingerprints, key names/icons/colours, appearance, this browser's layout and its default upload folder. No private key, SSH transport state or token is stored, exported or shared with other pages.
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

# 4. Test, type-check, lint the React hook rules (oxlint) and build the static client.
(cd web && npm test && npm run check && npm run lint && npm run build)

# 5. Build kelpmux, which embeds web/dist.
cargo build --locked --release --bin kelpmux
```

Build the web assets before compiling the executable that embeds them; rebuild `kelpmux` after every web change. `node_modules`, `web/dist`, `web/sourcemaps`, `web/public/vendor` and `web/public/client-core.wasm` are generated and ignored by Git. `npm run build` also writes the client's source maps to `web/sourcemaps/` (never to `web/dist`, so they are neither embedded nor served) and its release name to `web/sourcemaps/RELEASE`; see [Error reporting](#error-reporting).

Browser-level tests of the terminal component (WebGL/Canvas renderer, inset geometry, trusted copy events) run with Playwright in Chromium and WebKit and need no network: `node test/renderer.mjs` and `node test/terminal.mjs` in `web/`, plus `node test/app.mjs` and `node test/mobile.mjs` after `npm run build` (the built client boots cross-origin isolated to the sign-in page with its worker and WASM core; with iPhone and iPad emulation in both orientations it fits without horizontal scrolling, picks the phone or tablet layout and serves a valid manifest and icons under the relay's exact headers; the touch terminal and key bar keep reading mode, enter input mode, apply the sticky Ctrl, scroll history and copy a long-press selection), `node test/upload.mjs` (the upload dialog against a fake server: a drop opens it with the terminal, names, sizes and destination, chunks stay within the flow-control window, the quoted paths are pasted, limits and destinations are checked before sending, cancel sends `upload_cancel`, a closed terminal shows the paths with copy buttons and a server without `uploads` shows why), and `node test/errors.mjs` (optional error reporting against a local fake ingest server: with a DSN an uncaught error sends exactly one envelope with the build's release, the `web` component, no breadcrumbs, user or query string; without one the reporting code is never loaded and nothing is sent), `node test/sidebar.mjs` (the sidebar with no full width mode, its two-column tools above the vitals and the account at the bottom on desktop, folded, iPad portrait and phone, and per-terminal notifications: tab and tile bells, showing a terminal or focusing a tile marks its notifications read, the header bell's panel opens on History, mark one and all read; `SIDEBAR_SCREENSHOTS=dir` saves screenshots), using the `mcr.microsoft.com/playwright:v1.64.0-noble` image or locally installed Playwright browsers. Rust integration tests drive `kelpmux web-bridge` over real pipes against a disposable server (`cargo test --test native_web_bridge`).

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
  "dev": false,
  "glitchtip_dsn": null,
  "error_environment": null
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
- `glitchtip_dsn` (optional, default unset) turns on [error reporting](#error-reporting) to a GlitchTip project; `error_environment` (optional, default unset) names the environment events are filed under, such as `preprod`. Without a DSN nothing is reported and the browser loads no reporting code.

Hosting is the operator's responsibility; nothing installs a service. Start with private or VPN-only access.

## Error reporting

Optional and off by default: KelpMux can report errors to a [GlitchTip](https://glitchtip.com) project (self-hosted and Sentry-compatible) with the Sentry SDK. It reports errors and crashes only. There is no performance tracing, session replay, profiling, session tracking or cron monitoring (GlitchTip has none of the last three anyway).

**Status.** The web client reports today (component `web`). Reporting from the `kelpmux` executable itself, meaning the relay's own errors and panics (`relay`), `web-bridge` (`bridge`) and TUI panics (`tui`), is pending: its implementation has not been chosen yet. `src/native/error_reporting.rs` already holds the validated settings and the release name it will use; until it lands, the relay only hands its DSN to the browser, and `KELPMUX_GLITCHTIP_DSN` is not read (see [native.md](native.md#error-reporting)).

**Configuration.** Set `glitchtip_dsn` in the relay's configuration to the project's public DSN, `https://KEY@HOST[:PORT]/PROJECT_ID`, and optionally `error_environment` (1–64 letters, digits, `.`, `_` or `-`). The relay checks both at startup and refuses an `http://` DSN or one with a secret key, since the DSN is handed to every browser. When a DSN is set, `/connection.json`, the bootstrap document the browser already reads from the relay, gains `"errors": {"dsn": …, "environment": …}`, and the relay adds the DSN's origin (`https://HOST[:PORT]`) to the `connect-src` of its Content-Security-Policy. Without a DSN neither changes, the browser never loads the reporting code (a separate chunk, `assets/error-reporting-*.js`, about 64 kB, 22 kB gzipped) and nothing is sent. Reports are cross-origin `fetch` requests in CORS mode, which cross-origin isolation (COOP/COEP) allows; GlitchTip answers them with the needed CORS headers.

**What is sent.** Uncaught errors and unhandled promise rejections, with their `cause` chain and duplicates dropped, through a bare `BrowserClient` with only those integrations. That means no breadcrumbs (console, DOM, fetch, navigation), no user, IP inference (`infer_ip: never`), cookies, headers, request bodies or query parameters, no client reports, and none of the SDK's default integrations. A `beforeSend` hook additionally removes `user`, `breadcrumbs` and `extra`, reduces `request` to its URL, and strips query strings and fragments from that URL and from every stack frame's file name. Events are tagged `component: web` and carry the build's release and the configured environment. KelpMux never adds terminal content (text, input, titles, folders or commands) as tags, extra data or context. An error message is sent as thrown, so error messages must not include terminal content either.

**Pages and workers.** Every page of the client, popped-out terminals included, starts reporting the same way after reading `/connection.json`. The SharedWorker that holds the SSH connection does not load the SDK: keeping it out of the page that holds SSH transport state is simpler and safer, and the worker's failures already reach the pages that use it, which show them.

**Release name.** `kelpmux@VERSION+SHA`: the version from `Cargo.toml` and the first 12 hex digits of the Git commit, for example `kelpmux@0.2.0-beta.7+c5b489c1a2b3`. Without Git information it is `kelpmux@VERSION`. `KELPMUX_GIT_SHA` supplies the commit when the build has no Git checkout, and `KELPMUX_RELEASE` overrides the whole name (letters, digits, `.`, `_`, `@`, `+`, `-`; at most 200). The web build (`web/scripts/release.mjs`, through Vite's `define`) and the Rust build (`build.rs`, as `kelpmux::native::error_reporting::RELEASE`) compute it the same way, so one build reports one release. When the web build ran without Git (in a container without the repository's `.git`, for example) and the Rust build with it, the two would differ: `build.rs` warns when `web/sourcemaps/RELEASE` names another release. A deploy should set `KELPMUX_RELEASE` (or `KELPMUX_GIT_SHA`) for both builds.

**Source maps.** For each build, the deploy uploads the client's source maps to the GlitchTip project for the release in `web/sourcemaps/RELEASE`, before or when it ships that build. Vite builds them as hidden maps (no `sourceMappingURL` comment) and moves them out of `web/dist`, so the relay neither embeds nor serves them. The browser loads scripts from `/assets/`, so stack frames reference `https://ORIGIN/assets/NAME.js`. Upload, under the URL prefix `~/assets/`:

- each minified script `web/dist/assets/NAME.js` as `~/assets/NAME.js`: `index-*.js` (the application), `error-reporting-*.js` and `session-worker-*.js`;
- its map `web/sourcemaps/assets/NAME.js.map` as `~/assets/NAME.js.map`.

Because the scripts carry no `sourceMappingURL`, the uploader must link each script to its map: `sentry-cli` does so for `NAME.js` and `NAME.js.map` in the same directory. For example, stage both into one directory, then run `sentry-cli --url https://GLITCHTIP sourcemaps upload --release "$(cat web/sourcemaps/RELEASE)" --url-prefix '~/assets' STAGING_DIR`. With another uploader, give each script the artifact header `Sourcemap: NAME.js.map`. Never copy the maps into `web/dist`: `build.rs` skips `*.map` files, but anything else there would be served publicly.

## Registering a YubiKey

1. Open the HTTPS origin. On the sign-in page choose **Register a new key**, then **Register new key**, and touch the key. This creates a discoverable ECDSA P-256 credential with required user verification, scoped to this origin. It grants nothing on its own.
2. Copy the displayed OpenSSH public key (`sk-ecdsa-sha2-nistp256@openssh.com …`) and add it to the account's SSH authorization through your usual mechanism (for example `~/.ssh/authorized_keys`, optionally prefixed with `restrict,verify-required,command="…"` as described above). The web client never edits SSH authorization.
3. Choose **Sign in with security key** and touch the key. When this browser remembers the last key used, one touch signs the login. **Use another security key** (or a new browser) first asks the authenticator which credential to use; because WebAuthn assertions do not return the public key, the browser recovers the candidate public keys from that signature and offers them all to OpenSSH, which asks the server which one is authorized before requesting a second touch to sign. Nothing needs importing on a new browser.
4. If the server accepts none of the candidates, the page says the key is not authorized for the account.

Register and authorize each physical key separately. Settings → Security keys names keys, picks the one used for the next sign-in, shows public keys and exports or imports public registrations; these details stay in this browser. Removing a key there does not delete the credential from the authenticator or revoke its SSH authorization. Removing the SSH authorization stops new logins; it does not end existing SSH sessions.

## Using it alongside the TUI

The TUI and the web client are independent clients of the same KelpServer. You can use the TUI alone, the web alone, or both at once on the same panes and processes:

- Layout is per client. The TUI keeps its grid, tabs, parking, tab colours and focus; each browser keeps its own view mode, terminal order, parked terminals, tab colours, focused terminal, sidebar width and folding in `localStorage` (layout per workspace, discarded when the server restarts with a new epoch). Neither changes the other's layout. A terminal without a colour picked in this browser takes the TUI's default for its pane ID, so both start from the same colours; a colour picked in one client does not appear in the other. Parking in the browser hides a terminal from this browser's list and grid only; it keeps running and stays visible in the TUI. Parking in the TUI likewise stays in the TUI: parking is each client's own state, and KelpServer 0.3 reports every pane as unparked.
- Notification read marks are shared. Showing a terminal in the browser marks its notifications read for every client, as the TUI does when it shows one: the terminal shown on its own (or in its popup window), or in the live grid only the focused tile; the other tiles keep their bells until they are focused. A notification that arrives for the shown terminal is marked read at once while the browser window is visible and focused; one that arrives while the window is hidden or in the background keeps the bell until the window is looked at again, and a hidden window never marks anything read. The bell in a terminal's header opens its notifications, mainly as a history (**History** by default, with **Unread**, **Mark read** and **Mark all read** too). The TUI's marks show in the browser and the browser's in the TUI. Sound and desktop alerts are this browser's own preferences (Settings → Notifications), off by default.
- Creating or closing a terminal is shared: a new pane appears in every client, and closing one (always confirmed) ends its processes for everyone.
- Input and size follow whichever client last used a pane. When a browser focuses a terminal or types into it, it claims that pane's input and size lease and resizes the PTY to its own view. When the TUI's controlling client then types, pastes, clicks or focuses that pane, the TUI takes it back and the pane returns to the TUI's geometry. The live grid is a foreground layout too: while the browser window is focused and visible, it sizes each tile's pane to that tile (claiming it), so opening the grid in a browser resizes the panes it shows until the TUI uses them again. Passive rendering in a background window, mouse movement, window blur and focus-out never take a pane. Each pane has exactly one PTY size; a client that does not hold the lease shows the pane clipped or padded. Different panes can be held by different clients at the same time.
- The focused terminal's header shows a **TUI** or **Other browser** badge while this browser does not hold its input and size (hover it for details). Closing the browser or losing the connection releases its leases; terminals keep running and return to the TUI's geometry.
- Only one TUI client is the controller (input and workspace size); further TUI clients are observers, as before. Web clients are not TUI controllers or observers and do not affect that role.

Other same-origin tabs and **Open in a window** popups reuse the SSH connection held by the first tab through a SharedWorker; keep that tab open. The shared session locks after 15 minutes without deliberate input. **Log out all browser windows** (Settings → Connection) or `Ctrl-B d` closes SSH for every window of the browser; workspace processes keep running.

## Uploads

Files and images dropped onto a terminal are uploaded to the host where its shells run, and their paths are pasted into that terminal, through KelpServer's `uploads` stream capability (decision 151a; see the [KelpServer protocol](https://github.com/joanfabregat/kelpserver/blob/main/crates/server/docs/native-client-protocol.md#uploads-uploads)). There is no other file transfer and no file browser.

- **Drop targets.** Drop one or more files or images onto a live terminal: the one shown on its own, or any tile of the live grid. Pasting an image (a clipboard with files and no text, such as a screenshot) into a terminal does the same. Every terminal's menu (right-click, or **⋯** on phones and tablets) also has **Upload files…**, which opens the system file picker, so iPhones and iPads, which have no drag and drop onto a page, can upload too. Files dropped elsewhere on the page are ignored rather than opened by the browser.
- **Dialog.** Nothing is sent until the upload dialog is confirmed. It shows the target terminal, each file's name and size, the total and the destination folder. Folders cannot be uploaded: a dropped folder is listed as skipped. The server's advertised Welcome `upload_limits` are enforced before anything is sent (64 files, 100 MiB per file and 1 GiB per drop by default); a drop over a limit explains which one and cannot be started.
- **Destination.** By default each drop gets its own new private folder in the server's upload folder (`~/.cache/kelpserver/uploads` unless the server is configured otherwise with `kelpmux start --upload-dir`; KelpServer deletes completed uploads there after its retention, 7 days by default, set with `--upload-retention-days`; see [native.md](native.md#named-workspaces)). Settings → **Uploads** sets a default destination folder instead: an absolute path on the server host, which must be an existing folder owned by the account (KelpServer checks this), sent as `dest`; each drop then gets its own private folder inside it. The setting is kept in this browser only. The dialog can change the folder for one drop, with **Remember as the default destination in this browser**.
- **Progress and cancel.** The dialog shows each file's progress and the total. The client sends chunks of at most 128 KiB (or the server's smaller `max_chunk_bytes`) and never has more than the server's `max_chunks_in_flight` unacknowledged on the connection; windows sharing one SSH connection share that window through the SharedWorker, which also routes each upload's events to the window that began it and cancels the unfinished uploads of a window that closes. **Cancel upload** sends `upload_cancel`, and the server removes the batch with its partial files. A refusal (a missing or foreign destination, too many uploads at once, not enough space) or a failure is shown in the dialog, and nothing is pasted.
- **Paste.** When the upload is done, the absolute paths are quoted for POSIX shells (`'…'`, with `'` written as `'\''`, unless a path only has safe characters) and pasted, space-separated, into the terminal the files were dropped on, as a paste: bracketed when the program has bracketed paste on, and without Enter, so nothing runs until you press it. Pasting takes the terminal's input and size, as typing does. If that terminal closed during the upload, the dialog lists the paths with copy buttons instead.
- **Requirements.** The server must offer the `uploads` capability and the bridge must report `uploads` in its operations (API 3, minor 1); otherwise a drop opens the dialog with the reason (restart the workspace with a current KelpServer, or install matching KelpMux versions) and sends nothing.

Uploading is the same trust as typing into a shell: anyone signed in can already write any file the account can write. The paths come from the server and are checked by the WASM core (absolute, no control characters) before they can be pasted.

## Layout

On a desktop or tablet the left sidebar is the only navigation; there is no top bar and no status bar. The terminal area shows the focused terminal (its header has the number, state, title, folder, a **TUI** or **Other browser** badge when that client holds its input and size, and the notifications bell, Find, Park, Open in a window and Close) or the live grid. From top to bottom the sidebar holds:

- KelpMux, the **DEV** badge on a development relay, and the state counts (click one to jump to the next terminal in that state), with no separator line before the tabs.
- The terminal tabs in this browser's order, with the highlight on the focused terminal, also in the grid: pressing, focusing or right-clicking a tile, or moving the grid focus with `Ctrl-B h`/`j`/`k`/`l`, moves it. **+ New terminal** follows right under the tabs, like the TUI's "+" row, then the collapsible **Parked** section. A tab with unread notifications that is not shown (another tab, or a grid tile other than the focused one) shows a ringing bell (with the count when there are several), as does its grid tile header.
- The tools in two columns, like the TUI's sidebar buttons, with short labels: **Grid** (Live grid; pressed while the grid shows; with several pages it takes the whole row with the previous / next page buttons and the page number), **Terminals**, **Closed** (Recently closed), **Search** and **Help**. Each tooltip gives the full name and shortcut. They scroll on their own when the window is short, so the tabs keep most of the height; with ten tabs they fit an 800 pixel tall window without scrolling.
- The vitals, drawn like the TUI's sidebar status: CPU (green) and RAM (purple) meters with their percentages, then the first two disks as pies filled by usage like the TUI's `○ ◔ ◑ ◕ ●` (yellow, red from 90 % used), each with its mount point and percentage in a tooltip. The colours are the TUI's in dark mode and darker shades of the same hues in light mode. Clicking the vitals opens System status.
- At the very bottom, the account (the user name) and its **Settings** (`Ctrl-B ,`).

A narrow fold handle sits vertically centred on the sidebar's edge; the folded rail keeps the numbers, state marks and unread bells, the tools' icons in one column, compact vitals (CPU and RAM percentages and the disk pies) and the account's initial. Clicking the edge also folds it and dragging it resizes. There is no full width mode: the folded rail is the smallest sidebar. The workspace name is not shown anywhere in the main interface: one relay serves one workspace, so the sign-in screen names the user and the relay's host instead, the **Workspace not running** screen names the workspace it would start, and Recently closed names a workspace only on an entry that came from another one.

## Keyboard

`Ctrl-B` is the browser's prefix, and the keys after it follow the TUI's default bindings wherever the browser has the same action: `Enter` new terminal, `Tab`/`Shift+Tab` next/previous terminal, `1`…`9` terminal by number, `g`/`e` live grid, `h`/`j`/`k`/`l` or arrows move the grid focus, `PgUp`/`PgDn` grid pages, `N` next question, `P` next completion, `o` the Terminals table, `a` the same table showing agents, `!` agents needing attention, `n` finished agents, `b` the focused terminal's notifications, `r` recently closed, `z` reopen the last closed terminal, `/` search terminal history, `[` scroll mode, `Space` selection mode, `H`/`J`/`K`/`L` grow or shrink the focused grid column or row, `=` equal grid tiles, `p` park, `u` parked terminals, `s` move, `x` close (confirmed), `,` settings, `?` help, `d` disconnect, and `Ctrl-B` twice sends `Ctrl-B` to the terminal. Outside the prefix, `Alt+1`…`Alt+0` open terminals 1–10, `Alt+Tab` or `Ctrl/Cmd+Shift+G` toggle one terminal / live grid, and `Ctrl/Cmd+F` finds in the focused terminal. In scroll mode, `PgUp`/`PgDn`, `Ctrl-U`/`Ctrl-D`, `Ctrl-B`/`Ctrl-F`, `j`/`k` or arrows scroll, `g`/`G` jump to the oldest line or the live screen, `Space` or `v` switches to selection, and `Esc` or `q` leaves. In selection mode, `h`/`j`/`k`/`l` or arrows move the cursor (scrolling at the edges), `0`/`$` line start and end, `w`/`b` eight columns, `Space`/`v` set or clear the anchor, `y` or `Enter` copy and leave, `Esc` or `q` leave. Every other key goes to the focused terminal with full modifier and Kitty keyboard-protocol support. Settings → Keyboard and Help list these bindings. The TUI's own bindings and configuration file are unchanged; custom TUI bindings are not read by the browser.

The mouse covers the rest: click a sidebar row to open a terminal, drag rows (or grid tile headers) to reorder, right-click a row or tile for its menu (open, open in a window, find, move up/down, park/unpark, tab colour, notifications, upload files, close), drop files onto a terminal or tile to upload them, middle-click a row to close it, click the sidebar's state counts to jump to the next terminal in that state, click the fold handle or the sidebar's edge to fold it, and drag the edge to resize it.

## Phones and tablets

The same client adapts to iPhone and iPad (Safari first; [ADR 0003](adr/0003-web-client-in-kelpmux.md#addendum-phones-and-tablets) records the decisions). The layout follows the viewport and the primary pointer (`web/src/device.ts`): a touch screen whose short side is under 600 CSS pixels is a **phone** (every iPhone, both orientations), any other touch screen a **tablet** (every iPad), and a mouse-driven window narrower than 600 pixels also gets the phone layout.

- **Phone.** The home screen is the terminal list: the sidebar's content (state counts, terminals, **+ New terminal**, parked terminals, then the tools in two columns, the vitals and the account with settings; no Grid) at full width, scrolling as one list. Tapping a terminal opens it full screen; **‹ Terminals** returns to the list, the bell opens the terminal's notifications (ringing, with the count, while some are unread), and **⋯** opens the terminal's menu (find, move, park, tab colour, notifications, upload files, close). There is no grid. The terminal shown is sized to the phone through the normal per-pane lease, exactly as a desktop browser's focused terminal is.
- **Tablet.** The desktop layout, sidebar and grid, with larger tap targets. Landscape and portrait are both supported; in portrait the sidebar starts folded each time the tablet turns to portrait (that fold is not saved), and the fold handle on the sidebar's edge unfolds it. Landscape uses the saved sidebar preference.
- **Touch on a terminal.** A tap selects the terminal and takes its lease without showing the keyboard (an application that requested mouse reporting receives the tap as a click; a link opens). One finger dragged up or down scrolls the history, as the wheel does. A long press selects the word under the finger; dragging extends the selection, and lifting copies it. Pinch zoom is not supported. Sidebar rows scroll on touch instead of reordering; their **⋯** menu has Move up / Move down.
- **Keyboard.** Opening a terminal never brings up the on-screen keyboard: reading is the default. The key bar under the terminal has **Input**, which shows the keyboard (and becomes a hide-keyboard button while it is up); tapping any key on the bar also enters input mode. The bar holds Esc, Tab, Ctrl, the arrows, `^B`, Paste, Enter, Yes and No. Ctrl is sticky: it applies to the next key from the bar or the keyboard, then releases. `^B` arms the KelpMux prefix, so the next key typed is a prefix key as on a desktop. Paste reads the clipboard (Safari asks first). Yes and No send `1` and Escape to Claude Code, whose questions are numbered menus, and `y` and `n` to anything else; they do not add Enter. The bar follows the `visualViewport`, so it stays directly above the iOS keyboard, and the layout respects the safe-area insets. These rules apply to the iPad too, including with a hardware keyboard (tap Input first).
- **Installable web app.** `manifest.webmanifest` (standalone display, theme colours, 192 and 512 pixel icons, a maskable icon) plus the iOS meta tags and a 180 pixel `apple-touch-icon` let Safari's **Add to Home Screen** install KelpMux as its own app. There is no service worker: nothing is cached offline (the client needs its relay anyway), and the cross-origin-isolation and asset headers stay exactly as the relay sets them. There are no push notifications. The PNG icons are rendered from `web/public/icons/icon.svg` by `web/scripts/render-icons.mjs` and committed.
- **Sign-in** is the same YubiKey WebAuthn flow, over NFC or USB-C; iCloud Keychain passkeys are not supported as SSH keys. The sign-in and key-registration screens fit a phone and scroll instead of overflowing.

The relay serves the new files with its existing rules: `.svg` as `image/svg+xml`, but `.png` and `.webmanifest` as `application/octet-stream` with `nosniff`. Browsers still decode the icons and parse the manifest (the Playwright test checks this against the relay's exact headers), but the relay should add `image/png` and `application/manifest+json`; that is a server change outside `web/`.

## Feature parity with the TUI

The browser has the TUI's feature set adapted to web conventions. Status: **done** existed before this work; **added** is implemented in the parity branch; **deferred** gives the reason.

| TUI feature | Web equivalent | Status |
| --- | --- | --- |
| Left tab sidebar with numbers, tab colours, agent state marks, provider and folder | Sidebar terminal list in this browser's order with number, tab colour (this browser's), state mark, provider logo, title and folder | added |
| Sidebar resize and fold | Narrow fold handle vertically centred on the sidebar edge (also on the folded rail, to unfold it); drag or click the edge to resize; Settings → Notifications → Sidebar; saved per browser | added |
| Workspace state counts in the sidebar | State counts at the top of the sidebar (no workspace name); click to jump to the next terminal in that state | added |
| Vitals in the sidebar footer, status/process view | Sidebar footer in the TUI's colours: CPU and RAM meters, two disk pies with mount and percentage tooltips (compact on the folded rail); System status dialog with processes | added (dialog done) |
| Tabs view / reading one pane (`Ctrl-B 1`…`9`, `Tab`) | One-terminal view; `Ctrl-B` digits, `Tab`/`Shift+Tab`, `Alt` digits | done; TUI keys added |
| Fullscreen (`Ctrl-B f`) | None: the folded rail is the smallest sidebar | removed (owner decision) |
| Grid (`Ctrl-B g`/`e`), grid focus `h`/`j`/`k`/`l`, pages | Live grid (sidebar entry) with pages (buttons on that entry), focus ring and `h`/`j`/`k`/`l`/arrow focus; the sidebar highlight follows the focused tile | done; focus keys added |
| Grid swap by dragging | Drag tile headers to reorder | added |
| Grid tile weights (`Ctrl-B H`/`J`/`K`/`L`) | `Ctrl-B H`/`L` widen or narrow the focused column, `K`/`J` the focused row, `=` equalizes; per browser, reset when the grid shape changes | added |
| New tab (the sidebar's "+" row), close tab (confirmed) | **+ New terminal** right under the tabs; close confirmed | done |
| Park / unpark, parked list (`p`, `u`) | Park from the row, tile header, menu or `Ctrl-B p`; collapsible Parked section and Parked dialog (`Ctrl-B u`) | added (per browser) |
| Move picker (`s`), drag reorder | Move dialog (`Ctrl-B s`), drag rows, menu Move up/down | added |
| Agents list with all / attention / finished filters (`a`, `!`, `n`) | Terminals table with the Agents, Attention and Finished filter chips preselected | added; one table replaces the Agents and Overview dialogs (owner decision) |
| Next question / next completion (`N`, `P`) | Same keys; also the sidebar state counts | added |
| Attention markers and finished markers | State marks in rows, tiles, terminal headers; tab title count | added |
| Notifications list, acknowledge, acknowledge all, history (`b`) | Per terminal: a bell on its sidebar tab and grid tile while it has unread notifications and is not shown; the bell in its header (or `Ctrl-B b`, or its menu) opens its own notifications: history (the default) / unread, mark read, mark all read. No global Notifications page | added; per terminal (owner decision) |
| Notification sound toggle | Sound and desktop alerts per browser | added |
| Focusing a pane marks its notifications read | Showing a terminal marks them read: the terminal shown on its own or in its window, or the focused grid tile (other tiles keep their bells); only while the window is visible, and arrivals only while it is also focused | done (restored, owner decision) |
| Scratch popup (`c`) | None; a TUI scratch pane shows as an ordinary terminal | removed (owner decision): the web client has no scratch popup |
| Recovery: recently closed and previous runs, copy command, reopen / rerun / folder (`r`) | Recently closed dialog with copy and resume / rerun / folder | added |
| Reopen last closed (`z`) | `Ctrl-B z` | added |
| Workspace search with focused / all scope (`/`) | Search dialog (Tab toggles scope); a hit opens the terminal with find prefilled | added |
| Find in pane | `Ctrl/Cmd+F`, find bar | done |
| Overview (`o`) | Terminals table (All): tab number and name, agent logo and state, folder with its Git repository, branch and GitHub `owner/repo` link (`repos`), running program (`commands`) and last activity, in tab order; a row opens its terminal | added; conversation id, status evidence and lease owner columns dropped (owner decision) |
| Help (`?`) | Help dialog with the bindings | added |
| Settings (`,`): accent, tab position, wrap, prefix, bindings | Settings: appearance, theme, font, alerts, sidebar, keyboard list, uploads, security keys, browser session | done; alerts added. TUI-only display settings (accent, tab position, wrap, custom prefix) do not apply to the browser |
| Selection mode (`Space`), scroll mode (`[`), copy (`y`) | Mouse selection and copy, Shift+drag, wheel and Shift+wheel history; keyboard scroll (`Ctrl-B [`) and selection (`Ctrl-B Space`) modes with `y` copy | added |
| Agent link picker (`i`) | None | not planned (owner decision) |
| Tab colour picker | Menu → Tab colour…, the TUI palette, kept per browser (the TUI keeps its own) | added (per browser) |
| Detach (`d`) | Disconnect (`Ctrl-B d`) | done |
| Takeover (`t`), controller / observer | Per-pane input and size leases | not applicable: browsers never hold the TUI controller role |
| Templates, startup commands | None | deferred: a template is a `kelpmux start --template` option saved in `workspace.json`, from which the TUI creates its tabs; there is no runtime operation to apply one, so it does not fit a bridge operation cleanly |
| `ssh-entry` (start the workspace server if needed, then attach) | Sign in; if the workspace is stopped, **Start workspace** (explicit click) starts it the same way as `kelpmux start`, then attaches | added: never replaces a running server and starts only the bridge's configured workspace |
| Mobile layout | Phone and tablet layouts; see [Mobile](#mobile) below | added |

### Mobile

The TUI has no mobile counterpart; this table lists how each desktop browser feature maps to the phone and tablet layouts ([Phones and tablets](#phones-and-tablets)).

| Desktop browser | Phone | Tablet | Status |
| --- | --- | --- | --- |
| Sidebar terminal list | Home screen: the sidebar's content at full width | Sidebar; folded at first in portrait | added |
| Sidebar fold handle and resize | Not applicable (no side-by-side sidebar) | Same narrow handle, taller; a portrait fold lasts for that orientation | added |
| One-terminal view | Full screen with **‹ Terminals** and a **⋯** menu | As desktop | added |
| Live grid, grid keys and tile weights | None (owner decision) | As desktop; tiles are tapped to open | added |
| Drag to reorder rows and tiles | Row menu Move up / Move down | Row menu Move up / Move down | added |
| Right-click menu, middle-click close | **⋯** button on each row and in the terminal bar, beside the notifications bell | **⋯** button on each row; tile headers keep their buttons | added |
| Keyboard focus on click | Tap selects and takes the lease, keyboard hidden; **Input** or the key bar shows it | Same | added |
| Modifier and special keys | Key bar: Esc, Tab, sticky Ctrl, arrows, `^B`, Paste, Enter, Yes, No, hide keyboard | Same | added |
| `Ctrl-B` prefix | `^B` on the key bar, then a key | Same, or a hardware keyboard | added |
| Wheel history scrolling | One-finger drag | One-finger drag | added |
| Mouse selection and copy | Long press, drag, lift to copy | Same | added |
| Find, park, tab colour, close | **⋯** menu | Tile header buttons and menu | added |
| Drop files onto a terminal to upload them | **⋯** menu → Upload files… (file picker) | Same; dropping also works where the browser allows it | added |
| Open in a window | None: an installed web app has no popup windows | None | not applicable |
| Dialogs (terminals, a terminal's notifications, search, settings…) | Full screen | As desktop | added |
| Terminals table | Full screen, one card per terminal | Table, as desktop | added |
| System status in the sidebar | Vitals at the end of the home screen list | Sidebar footer, as desktop | added |
| Browser tab title and favicon state | Not shown by an installed app; state marks in the list | Same | not applicable |
| Desktop notifications | None: no push notifications (owner decision) | Same | deferred |
| Sign-in with a YubiKey | Same flow over NFC or USB-C; screens fit the phone | Same | added |
| Install as an app | Add to Home Screen: standalone, own icon, no service worker | Same | added |
| Pinch zoom | None; font size in Settings → Appearance | Same | not planned |

## Reusable terminal component

`web/src/terminal/` contains the pieces a different web application (KelpHub) can consume at a pinned KelpMux revision: SSH/WebAuthn connection and key discovery, the SharedWorker session broker, WASM client-core loading, the WebGL/Canvas renderer, keyboard, mouse and wheel encoding, the touch key bar encoding and input mode (`touch-keys.ts`), the upload protocol, limit checks and path quoting (`uploads.ts`), selection and copy, the screen cache, grid column layout, the `Terminal`/`TerminalTile` React components and pane find. Nothing in that directory imports application code. It is not published to npm yet; consumers copy or vendor it at a fixed revision together with the matching `client-core.wasm` built from the same revision.

## Current limits

- Up to eight terminals are shown per grid page (the stream allows ten subscriptions per connection, shared with popups); larger workspaces page.
- Panes cannot be renamed or split from the web: KelpServer has no rename operation, and its layout is automatic. Pane titles follow the running program.
- History search uses the server's bounded literal search (64 matches per terminal); the Search dialog searches at most 32 terminals and shows at most 200 hits. Older history pages are plain text.
- Restoring a closed terminal from the browser does not remove it from the TUI's Recently closed list (the server owns that file); restoring an agent conversation that is already open opens the existing terminal instead.
- Notification state for a pane is matched by pane and process ID; the browser does not see the server's run identifier, so a notification from a previous server run whose pane and process IDs both recur could be shown until it is read.
- Real hardware-key and Safari behaviour need manual acceptance; automated tests use virtual authenticators only where noted.
- Phones and tablets are tested with Playwright device emulation only. The on-screen keyboard, the key bar's position above it, safe areas, Add to Home Screen, NFC and USB-C security keys and the connected phone and tablet layouts need acceptance on a real iPhone and iPad. On iOS an installed web app has its own storage, separate from Safari's: its first sign-in asks the authenticator which key to use, as a new browser does, and its layout and settings start fresh. Closing it, or iOS suspending it in the background, ends the SSH connection like closing a tab; terminals keep running.
