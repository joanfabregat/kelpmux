//! Bounded diagnostic metadata. No terminal cells, prompts, paths or session IDs.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    pub version: String,
    pub protocol: u16,
    pub cols: u16,
    pub rows: u16,
    pub grid: (u8, u8),
    pub panes: usize,
    pub clients: usize,
    pub uptime_seconds: u64,
    pub full_frames: u64,
    pub patch_frames: u64,
    pub full_bytes: u64,
    pub patch_bytes: u64,
    pub pty_bytes: u64,
}
#[derive(Debug, Serialize)]
pub struct Benchmark {
    pub cols: u16,
    pub rows: u16,
    pub full_bytes: usize,
    pub patch_bytes: usize,
    pub elapsed_micros: u128,
}
pub fn benchmark() -> crate::Result<Vec<Benchmark>> {
    use super::protocol::*;
    let mut results = Vec::new();
    for (cols, rows) in [(40, 10), (80, 24), (160, 50), (240, 70), (400, 160)] {
        let old = Frame {
            cols,
            rows,
            cells: vec![Cell::default(); usize::from(cols) * usize::from(rows)],
            cursor: None,
            panes: Vec::new(),
            active: 0,
            reading: ReadingMode::Grid,
            controller: true,
            help: false,
            notice: None,
            ui: Default::default(),
        };
        let mut next = old.clone();
        next.cells[usize::from(cols) + 1].text = "X".into();
        let start = std::time::Instant::now();
        let patch =
            super::delta::diff(&old, &next, 1).ok_or("benchmark did not produce a patch")?;
        let elapsed_micros = start.elapsed().as_micros();
        results.push(Benchmark {
            cols,
            rows,
            full_bytes: serde_json::to_vec(&next)?.len(),
            patch_bytes: serde_json::to_vec(&patch)?.len(),
            elapsed_micros,
        });
    }
    Ok(results)
}
