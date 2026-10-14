//! Bounded diagnostic metadata. No terminal cells, prompts, paths or session IDs.
use serde::{Serialize,Deserialize};
#[derive(Debug,Clone,Default,Serialize,Deserialize)]
pub struct Report {
    pub version:String,
    pub protocol:u16,
    pub cols:u16,
    pub rows:u16,
    pub grid:(u8,u8),
    pub panes:usize,
    pub clients:usize,
    pub uptime_seconds:u64,
    pub full_frames:u64,
    pub patch_frames:u64,
    pub full_bytes:u64,
    pub patch_bytes:u64,
    pub pty_bytes:u64,
}
