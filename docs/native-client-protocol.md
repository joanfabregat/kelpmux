# Independent terminal stream

The canonical [terminal stream contract](https://github.com/joanfabregat/kelpserver/blob/main/crates/server/docs/native-client-protocol.md) lives in the [KelpServer repository](https://github.com/joanfabregat/kelpserver). The local composed-screen protocol is version 58; the independent terminal stream remains version 1. KelpMux builds against the KelpServer revision pinned in `Cargo.toml`, and its portable wire types (`kelpclient-protocol`) come from that same revision. See [MIGRATION.md](../MIGRATION.md) for workspace ownership and compatibility boundaries.
