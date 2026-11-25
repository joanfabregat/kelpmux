# @kelpcore/web

The browser terminal layer of the KelpMux web client, also meant for KelpHub's: WebGL/Canvas renderer, in-browser SSH with WebAuthn keys, the shared worker session, keyboard, mouse, wheel and touch-key encoding, uploads, the `Terminal` React component, and the file browser's request client, path and document models and CodeMirror 6 text editor (`files/`, derived from KelpHub). It is a member of the repository's npm workspace (root `package.json` and `package-lock.json`), and the web client depends on it as `"@kelpcore/web": "file:../web-core"`. The package name is historical (it was published from the KelpCore repository); the package is private and never published.

The package exposes its TypeScript sources through `exports` (`"./*": "./src/*"`), so imports name the file with its extension, for example `@kelpcore/web/renderer.ts`, `@kelpcore/web/Terminal.tsx`, `@kelpcore/web/ssh/wire.mjs` and `@kelpcore/web/terminal.css`. The application's bundler compiles them; there is no build step in this package. `react` (19) is a peer dependency. The CodeMirror 6 and Lezer packages that `files/editor.tsx` imports are regular dependencies (exact versions); an application should load the editor lazily (`import("@kelpcore/web/files/editor.tsx")`) so they stay out of its main bundle, and ship their MIT license texts.

## What the application serves

The browser layer loads these at runtime from the page's origin; the application provides and serves them:

| Path | Content |
| --- | --- |
| `/client-core.wasm` | [`kelpclient`](../crates/client) built for `wasm32-unknown-unknown` from the same commit as the server |
| `/vendor/libapps/wassh/js/` (`process.js`, `syscall_handler.js`, `worker.js` and their imports) and `/vendor/libapps/wasi-js-bindings/` | Chromium libapps WASSH and WASI bindings (BSD), at a pinned libapps revision |
| `/vendor/ssh.wasm` | The OpenSSH WASM client from the Secure Shell release (OpenSSH with OpenSSL, zlib and ldns, each under its own license) |
| `/ssh` | A WebSocket relay to the SSH endpoint |

The browser `Session` opts in to [binary screens](../docs/server/binary-screens.md) whenever Welcome lists `binary-screens`, and the WASM core decodes them. Everything between the page and the server must therefore forward binary records unchanged: `kelpserver bridge` does, and so must an application's own bridge (`kelpmux web-bridge` does). Deploy the web assets and the bridge from the same build; a bridge that re-serializes every record as JSON breaks the stream once the browser opts in.

In-browser SSH also needs a cross-origin isolated page (`SharedArrayBuffer`). `scripts/fetch-web-ssh.sh` fetches and verifies the vendored SSH assets; their license texts must be served with the application.

## Checks

After `npm ci --ignore-scripts` at the repository root, `npm run check` type-checks `src/` and `npm test` runs the unit tests (`test/*.test.mjs`, Node 22.18 or later for TypeScript type stripping) from this directory. Browser tests of the complete web client live in `web/test/`. Run them in your environment's approved container workflow. Licensed under [Apache-2.0](../LICENSE); see [NOTICE](../NOTICE).
