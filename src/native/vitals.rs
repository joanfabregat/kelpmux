//! In-process Linux metrics adapted from Vitals (MIT); see assets/licenses/vitals-MIT.txt.
//! No subprocesses or CPU state files. Filesystem sampling is cached independently.
pub mod clock;
pub mod cpu;
pub mod disk;
pub mod mem;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vitals {
    pub cpu: Option<u8>,
    pub memory: Option<u8>,
    pub swap: Option<u8>,
    pub disks: Vec<u8>,
    pub date: Option<String>,
    pub time: Option<String>,
}
pub struct Sampler {
    proc_root: PathBuf,
    cpu: Option<cpu::State>,
    metrics_at: Option<Instant>,
    disks_at: Option<Instant>,
    current: Vitals,
}
impl Default for Sampler {
    fn default() -> Self {
        Self::with_proc_root(Path::new("/proc"))
    }
}
impl Sampler {
    pub fn with_proc_root(proc_root: &Path) -> Self {
        Self {
            proc_root: proc_root.into(),
            cpu: None,
            metrics_at: None,
            disks_at: None,
            current: Vitals::default(),
        }
    }
    pub fn sample(&mut self) -> Vitals {
        self.sample_at(Instant::now())
    }
    /// Explicit monotonic clock keeps cache tests deterministic without sleeps.
    pub fn sample_at(&mut self, time: Instant) -> Vitals {
        if self
            .metrics_at
            .is_none_or(|last| time.saturating_duration_since(last) >= Duration::from_secs(1))
        {
            self.current.cpu = fs::read_to_string(self.proc_root.join("stat"))
                .ok()
                .and_then(|text| cpu::parse_stat(&text))
                .map(|sample| {
                    let (percent, state) = cpu::percent(self.cpu, sample);
                    if let Some(state) = state {
                        self.cpu = Some(state);
                    }
                    percent
                });
            let memory = fs::read_to_string(self.proc_root.join("meminfo"))
                .ok()
                .and_then(|text| mem::parse_meminfo(&text));
            self.current.memory = memory.map(|usage| usage.memory);
            self.current.swap = memory.map(|usage| usage.swap);
            let clock = clock::now();
            self.current.date = clock.map(clock::LocalTime::date);
            self.current.time = clock.map(clock::LocalTime::time);
            self.metrics_at = Some(time);
        }
        if self
            .disks_at
            .is_none_or(|last| time.saturating_duration_since(last) >= Duration::from_secs(5))
        {
            self.current.disks = fs::read_to_string(self.proc_root.join("self/mounts"))
                .ok()
                .map(|mounts| disk::readings(&mounts, 10 << 30))
                .unwrap_or_default();
            self.disks_at = Some(time);
        }
        self.current.clone()
    }
}
pub fn bar_glyph(percent: u8) -> char {
    match (u32::from(percent.min(100)) * 8 + 50) / 100 {
        0 | 1 => '▁',
        2 => '▂',
        3 => '▃',
        4 => '▄',
        5 => '▅',
        6 => '▆',
        7 => '▇',
        _ => '█',
    }
}
pub fn pie_glyph(percent: u8) -> char {
    match percent {
        88.. => '●',
        63.. => '◕',
        38.. => '◑',
        13.. => '◔',
        _ => '○',
    }
}
/// Plain text and indexed foreground colors; callers own layout and styling.
pub fn render_segments(vitals: &Vitals) -> Vec<(String, u8)> {
    let mut segments = Vec::new();
    for (label, percent, color) in [
        ("cpu", vitals.cpu, 114),
        ("mem", vitals.memory, 141),
        ("swap", vitals.swap, 73),
    ] {
        if let Some(percent) = percent {
            let percent = percent.min(100);
            segments.push((format!("{label} {} {percent}%", bar_glyph(percent)), color));
        }
    }
    if !vitals.disks.is_empty() {
        segments.push(("disk".into(), 222));
    }
    for &percent in &vitals.disks {
        let percent = percent.min(100);
        segments.push((
            format!("{} {percent}%", pie_glyph(percent)),
            if percent >= 90 { 196 } else { 222 },
        ));
    }
    if let Some(date) = &vitals.date {
        segments.push((date.clone(), 39));
    }
    if let Some(time) = &vitals.time {
        segments.push((time.clone(), 250));
    }
    segments
}
