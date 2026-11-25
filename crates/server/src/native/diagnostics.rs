//! Bounded diagnostic metadata. No terminal cells, prompts, paths or session IDs.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// The server's package version.
    pub version: String,
    /// The stream protocol version.
    pub protocol: u16,
    /// The base pane size (`--size`) of panes without a lease.
    pub cols: u16,
    pub rows: u16,
    pub panes: usize,
    pub clients: usize,
    pub uptime_seconds: u64,
    /// PTY output bytes read since the server started.
    pub pty_bytes: u64,
}
