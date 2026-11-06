# KelpServer

The persistent terminal server crate and executable in the [KelpMux workspace](../../README.md). It owns PTYs, process lifecycle, input ownership, resizing, authoritative terminal emulation, scrollback, search, parking, recovery metadata and agent activity reporting. It remains useful without KelpHub. Recovery metadata cannot preserve live processes across crashes or reboot.

The server uses the sibling [protocol crate](../protocol) for wire types; [client state](../client) is a test-only dependency. The frozen TUI remains in the workspace root, including its retained composed-screen compatibility contract. Project/Git workflows, uploads and authenticated management belong to KelpHub. The generic SSH byte relay is public server code; application assets and deployment packaging belong to KelpHub. A browser cannot bypass SSH authentication to access workspaces.

Build from the workspace root with `cargo build -p kelpserver --release --locked`. Install `kelpserver` beside `kelpmux`, or set `KELPSERVER_BINARY`. See [migration/build details](../../MIGRATION.md), [contribution policy](../../CONTRIBUTING.md), [security policy](../../SECURITY.md) and the [independent terminal stream contract](docs/native-client-protocol.md). Licensed under [Apache-2.0](LICENSE).
