# ADR 0003: KelpCore holds the server and every client-shared component

- Status: Accepted by Joan 2026-11-22 (America/Los_Angeles); phase A (client core and browser layer moved) implemented 2026-11-22; binary screens (section 2): client adoption (client core decoding, browser opt-in) implemented 2026-11-22; files operations (section 3) implemented 2026-11-23; bell merge (section 3) implemented 2026-11-23; file downloads (section 3, `file-downloads`) implemented 2026-11-23; file root (section 3, file operations confined to the home folder) implemented 2026-11-23.
- Numbering continues KelpServer's ADRs ([0001](../../crates/server/docs/adr/0001-native-client-protocol-and-device-capabilities.md), [0002](../../crates/server/docs/adr/0002-generic-session-server.md)), which stay where they are.
- Related: KelpMux [ADR 0003 (web client)](https://github.com/joanfabregat/kelpmux/blob/main/docs/adr/0003-web-client-in-kelpmux.md), KelpHub ADR 0007 (multi-tenant, Kubernetes-only; private repository).

## Context

The Kelp project had three repositories: KelpServer (the session server and the `kelpclient-protocol` wire types), KelpMux (TUI, web client, the `kelpclient` Rust/WASM client core) and KelpHub (private, paused, being rebuilt as a multi-tenant Kubernetes product). KelpMux and KelpHub pin KelpServer by Git revision.

Code shared by the two applications beyond the server was shared by copying. On 2026-11-22 KelpHub's web client held about 4,000 lines copied from KelpMux's `web/src/terminal/`: `renderer.ts`, `p256.ts`, `keyboard.ts`, `mouse.ts` and the in-browser SSH (`ssh/`) were byte-identical, `connection.ts` differed by two lines, and `Terminal.tsx` and `session-worker.ts` had drifted by about 300 lines each. KelpMux is now getting a file browser and editor that exist only in KelpHub, and toasts and notification rules that KelpHub will need too.

Separately, the stream protocol sends every terminal cell as a JSON object of about 96 bytes. A full-window snapshot (about 300×80 cells) is about 2.3 MB, sent whenever a client subscribes to a pane, the pane's size changes or a client falls behind. In the browser that costs about 250 ms when switching from the grid to one terminal or between tabs, against under 100 ms in the TUI, which reads the same JSON over a local socket.

## Decision

### 1. One shared repository, KelpCore

The KelpServer repository becomes **KelpCore** (`joanfabregat/kelpcore`, public, Apache-2.0). Its history was pushed unchanged on 2026-11-22; `joanfabregat/kelpserver` is archived once KelpMux depends on KelpCore. KelpCore holds the server and everything KelpMux and KelpHub share; KelpMux and KelpHub keep only their own application.

**Boundary rule:** a component belongs in KelpCore when both applications use it, or when it defines how clients talk to the server. Application layout, navigation and product features stay in each application. KelpServer's boundary rule (ADR 0002) still applies to the server package itself: the server has no UI.

| Package | Location | Content |
| --- | --- | --- |
| `kelpserver` | `crates/server` | The session server, unchanged in scope (ADR 0002), plus the server-side operations added below. Executable still named `kelpserver`. |
| `kelpclient-protocol` | `crates/protocol` | Wire types, framing and the binary screen codec (section 2). |
| `kelpclient` | `crates/client` | The Rust client core used natively by the TUI and compiled to WASM for browsers (screen model, selection, history layout, decoding). Moved from KelpMux. |
| `@kelpcore/web` (npm, not published) | `web/` | The browser layer both web clients use: the WebGL/Canvas renderer, the in-browser SSH, the shared worker and session, keyboard, mouse, wheel and touch keys, and the terminal component. Moved from KelpMux's `web/src/terminal/`. Later: the file browser and editor UI, toasts, notification rules. |

Package and executable names are kept, so consumers change only the repository URL of their pins.

**Consumption.** Rust crates are pinned by exact Git revision, as today. The web package is an npm Git dependency pinned to the same commit (`github:joanfabregat/kelpcore#<sha>` from a root `package.json` that exposes `web/`); it ships TypeScript sources that the application's Vite build compiles, so there is no separate build or registry. An application pins one KelpCore revision for all packages.

**Order of work for a cross-cutting change:** KelpCore first (server, protocol, client core and web layer in one pull request when they change together), then the applications bump their pin.

KelpMux moves first. KelpHub switches to KelpCore when it resumes; until then it keeps building against its current pins (an archived repository stays fetchable).

### 2. A binary screen format in the stream protocol

Screens (snapshots and patches) get a compact binary encoding. Control messages (pane lists, leases, notifications, requests and replies) stay JSON: they are small and change shape often.

- **Snapshot:** the pane's size and cursor, a table of the distinct cell styles used (foreground, background, attributes, underline colour, hyperlink), then each row as runs of `style index + UTF-8 text`, with trailing default blanks omitted. Wide characters keep their width information.
- **Patch:** the rows changed since the client's last revision, each re-encoded whole, plus cursor and modes. A row is a few dozen bytes, so this is simpler than per-cell changes and not larger in practice.
- **Framing:** the existing 4-byte length prefix and 16 MiB limit stay. A binary record is distinguished from a JSON record by a framing marker defined in `kelpclient-protocol` and is only ever sent to a connection that asked for it: the server advertises the `binary-screens` capability in `Welcome`, and the client then sends a `binary_screens` request (as with `session_events`). A field in `Hello` would not do, because older servers reject unknown `Hello` fields. Other connections keep receiving JSON, so the server, KelpMux and KelpHub can migrate independently.
- **One codec, in Rust,** in `kelpclient-protocol`: the server encodes, the TUI decodes natively and browsers decode through the `kelpclient` WASM core. Bridges forward binary records without parsing them.
- **In the browser,** the WASM core decodes into typed arrays, and the shared worker transfers (not copies) them to each window.

Expected size of a 300×80 snapshot: tens of kilobytes instead of 2.3 MB. This is an estimate to be measured by the implementation.

Raw PTY output was considered and rejected as the screen transport: clients attaching mid-session need a snapshot anyway, a busy pane would send every byte instead of at most 30 diffs a second, and every client would have to run an identical terminal emulator.

### 3. Server operations shared by every client

New session-API operations live in `kelpserver` because they act on the machine where the shells run (the user's host for KelpMux, the workspace pod for KelpHub) and every client reaches the server the same way. Bridges relay them unchanged, as they already relay uploads. Each is an optional capability.

- **Files:** list, stat, read, write (atomic replace with a content-hash conflict check), create folder, rename and move, move to trash (freedesktop trash in `~/.local/share/Trash`) and immediate delete. The design starts from KelpHub's `files.rs` (descriptor-relative opens, no symlink following, atomic writes, hard limits on sizes and entry counts), moved into KelpCore with Joan's approval (decision 185b). File operations grant no access beyond the shell the same authenticated user already has; the security boundary remains the authenticated stream connection. Uploads keep using the existing `upload_*` requests.
- **Notifications:** a bell and the agent notification it accompanies are merged by the server, so every client shows one event (decision 205a).

## Consequences

- KelpMux and KelpHub each pin one KelpCore revision instead of KelpServer plus copied code.
- A change crossing the protocol, client core and browser layer lands in one KelpCore pull request, checked by one CI.
- KelpCore's CI gains a Node job for `web/` next to the Rust jobs.
- Server releases and shared-library changes share one history and version line; the server keeps its own crate version.
- Documentation naming KelpServer as a repository (the Kelp `CLAUDE.md`, KelpMux docs and ADR 0003, KelpHub docs) is updated to KelpCore as the migration lands. The product and executable name KelpServer stays.

## Addendum 2026-11-23: file browser and editor UI

The owner moved the reusable part of KelpHub's file browser and editor into `@kelpcore/web` as `web/src/files/`, with the copyright holder's approval (see `NOTICE`), so that the KelpMux web client's new Files view and KelpHub share it: the `files` stream request client (`api.ts`, with request correlation, `max_in_flight` queuing and busy retries), path helpers and shell quoting for typing paths into terminals (`paths.ts`), the open-document model and language detection (`model.ts`, `languages.ts`), and KelpHub's CodeMirror 6 `TextEditor` and theme (`editor.tsx`, `theme.ts`) without the diff view and changes list, which stay in KelpHub. The views around them (browser, editor frame, sidebar) stay in each application. The CodeMirror and Lezer packages become dependencies of `@kelpcore/web`. The shared worker relays the `file_*` requests and routes each reply to the window that asked, the WASM core validates the file replies, and `request-limits.ts` lets a `file_write` carry up to 6 × 2 MiB + 64 KiB of encoded content. No server or protocol change.

## Addendum 2026-11-23: file operations confined to a root folder

At the owner's request ("the file browser should be gated to the user's home folder"), every file operation (list, stat, read, write, create folder, rename and move at both ends, trash, delete, download, and a client-chosen upload destination) is confined to a file root: the server user's home directory by default, or the folder named by `kelpserver server|start --files-root` (`files_root` in `workspace.json`, `files::Settings::root`), so KelpHub can later confine a workspace pod to its workspace. The server resolves the root once at startup (canonical, symlinks resolved) and advertises it as Welcome `file_limits.root`. A path outside it fails with the new `file_error` code `outside_root`. Paths are resolved beneath the root with `openat2` (`RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS`), falling back, for an absolute symlink or a kernel without `openat2`, to a component-by-component `O_NOFOLLOW` walk that resolves symlinks itself and refuses any that leads outside. Symlinks pointing outside are still listed and stat'ed as links; nothing is read, written, listed or downloaded through them. The root itself can be browsed but not changed. This limits the file browser, not the user: the same connection can still type `cat /etc/passwd` into a shell, so it is no security boundary between the user and the rest of the host (see `SECURITY.md`). Additive: one optional field, one error code, one server option.

## Addendum 2026-11-24: extra file roots

So KelpMux can let the user browse folders outside the home directory (a data drive such as `/mnt/data`) while the home directory stays the primary root, the server accepts extra roots: `kelpserver server|start --files-extra-root PATH` (repeatable; `files_extra_roots` in `workspace.json`, `files::Settings::extra_roots`). Each is resolved at startup like the primary root; one that is missing, not a directory, or overlaps the primary root or another extra root is skipped with a logged warning (of two nested extra roots the outer one is kept; one containing the primary root is skipped). Every path lies in exactly one root and is resolved beneath that root only, so a symlink never leads from one root into another (`outside_root`), and a rename between two roots fails with the new `file_error` code `cross_root` (nothing is copied). Welcome `file_limits.roots` lists every root as `{path, label}`, the primary first; `file_limits.root` keeps its meaning. Additive: one optional field, one error code, one server option. Details: [session-api.md](../../crates/server/docs/session-api.md).
