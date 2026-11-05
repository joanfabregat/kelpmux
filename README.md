# KelpServer

KelpServer is the open-source persistent terminal server in the Kelp family. It will own terminal processes and state independently of any attached interface.

## Status

The component has been extracted from KelpMux development commit `1033e4b`. See [MIGRATION.md](MIGRATION.md) for history provenance, build instructions and compatibility boundaries.

## Scope

- PTYs, process lifecycle, input ownership and terminal resizing.
- Authoritative terminal emulation, screen updates, scrollback and search.
- Persistent session identity, parking, recovery metadata and agent activity reporting.
- Local and SSH-authenticated access, with a versioned application protocol.

Project management, Git/GitHub workflows, file management and management MCP tools belong to the separate KelpHub management service. The server remains useful without KelpHub. Stored recovery metadata does not preserve running processes across a server crash or host reboot.

## Related components

| Component | Responsibility |
| --- | --- |
| [KelpClient](https://github.com/joanfabregat/kelpclient) | Shared protocol types and terminal client libraries, including WASM bindings |
| [KelpMux](https://github.com/joanfabregat/kelpmux) | Existing terminal interface; feature development is frozen |
| KelpHub | Separately maintained web interface and management service |

The server will depend on shared protocol/types, not client presentation or browser bindings. Terminal and management services have separate lifecycles and authorization boundaries. Web hosting and relay deployment are packaged by KelpHub; no browser connection may bypass SSH authentication to access workspace operations.

## Development

The local checkout is `~/repos/kelp/server`. Build and compatibility instructions are in MIGRATION.md. Keep changes scoped to the extraction; do not introduce containers, remote execution or a privileged host-management service in this scaffold.

See [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md). Licensed under [Apache-2.0](LICENSE).

The [independent terminal stream contract](docs/native-client-protocol.md) documents bounded snapshots/deltas, input ownership and framing. The retained composed-screen TUI contract is documented separately in MIGRATION.md.
