# Agent polling adapters

Agent discovery, the polling adapter registry, OSC 7501 terminal program status, conversation metadata and notification history are KelpServer behavior. They are documented in [KelpServer's agent polling guide](server/agent-polling.md).

## TUI controls

For an unidentified Codex TUI, prefix then `i` opens a read-only picker of loaded daemon conversations for the active agent tab. Alternatively open the workspace overview with prefix then `o`, select a tab and press `l`. Enter or a row click links the selected conversation; the first picker row clears an explicit link and restores automatic discovery. The notification popup identifies a matching current tab by its displayed number, including parked tabs; unavailable targets are labeled historical.

The legacy tmux discovery path is described in [discovery.md](discovery.md).
