//! Host status presentation for the TUI's Status dialog: views, sorting,
//! column layout and formatting. Moved from KelpServer `status.rs` (ADR 0002
//! step 2); host sampling (`Monitor`, `Snapshot`, `Process`, `Disk`) stays in
//! the server and reaches stream clients through `host-status`. The web
//! bridge samples with the same `Monitor` for its `status` operation.
use serde::{Deserialize, Serialize};

pub use kelpserver::native::status::{Disk, Monitor, Process, Snapshot};

/// The Status dialog's view: the main summary or disk and process I/O.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum View {
    #[default]
    Main,
    Io,
}
/// The process table's sort column.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sort {
    Pid,
    #[default]
    Cpu,
    Memory,
    Read,
    Write,
}
pub fn bytes(value: u64) -> String {
    let units = ["B", "K", "M", "G", "T"];
    let mut n = value as f64;
    let mut unit = 0;
    while n >= 1024.0 && unit < 4 {
        n /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value}B")
    } else {
        format!("{n:.1}{}", units[unit])
    }
}
pub fn percent(value: Option<u32>) -> String {
    value
        .map(|v| format!("{}.{:01}", v / 10, v % 10))
        .unwrap_or_else(|| "—".into())
}
pub fn rate(value: Option<u64>) -> String {
    value
        .map(|v| format!("{}/s", bytes(v)))
        .unwrap_or_else(|| "—".into())
}
pub fn processes<'a>(
    snapshot: &'a Snapshot,
    query: &str,
    sort: Sort,
    reverse: bool,
) -> Vec<&'a Process> {
    let query = query.to_lowercase();
    let mut rows: Vec<_> = snapshot
        .processes
        .iter()
        .filter(|p| {
            format!("{} {} {}", p.pid, p.user, p.command)
                .to_lowercase()
                .contains(&query)
        })
        .collect();
    rows.sort_by(|a, b| {
        let order = match sort {
            Sort::Pid => a.pid.cmp(&b.pid),
            Sort::Cpu => a.cpu.cmp(&b.cpu),
            Sort::Memory => a.resident.cmp(&b.resident),
            Sort::Read => a.read.cmp(&b.read),
            Sort::Write => a.write.cmp(&b.write),
        };
        (if reverse { order.reverse() } else { order }).then_with(|| a.pid.cmp(&b.pid))
    });
    rows
}

pub fn bounds(cols: u16, rows: u16) -> super::protocol::Rect {
    let width = cols.saturating_sub(10).min(160);
    let height = rows.saturating_sub(4).min(60);
    super::protocol::Rect {
        x: (cols - width) / 2,
        y: (rows - height) / 2,
        cols: width,
        rows: height,
    }
}
pub fn summary_columns(bounds: super::protocol::Rect) -> u16 {
    if bounds.cols >= 86 { 3 } else { 1 }
}
pub fn summary_column_width(bounds: super::protocol::Rect) -> u16 {
    if summary_columns(bounds) == 3 {
        (bounds.cols - 6) / 3
    } else {
        bounds.cols.saturating_sub(2)
    }
}
pub fn summary_detail_rows(bounds: super::protocol::Rect) -> usize {
    if summary_columns(bounds) == 3 && summary_column_width(bounds) < 38 {
        5
    } else {
        4
    }
}
pub fn summary_height(bounds: super::protocol::Rect, data: &Snapshot) -> u16 {
    let desired = if !data.ready {
        1
    } else if summary_columns(bounds) == 3 {
        (data.cpus.len().div_ceil(2) + 1 + data.disks.len().div_ceil(2))
            .max(summary_detail_rows(bounds))
    } else {
        data.cpus.len() + 2 + 4 + data.disks.len()
    };
    (desired.min(usize::from(u16::MAX)) as u16)
        .min(bounds.rows.saturating_sub(7))
        .min((bounds.rows / 2).max(1))
}
pub fn table_y(bounds: super::protocol::Rect, data: &Snapshot) -> u16 {
    bounds.y + 2 + summary_height(bounds, data)
}
pub fn view_tabs(
    bounds: super::protocol::Rect,
    data: &Snapshot,
) -> [(View, super::protocol::Rect); 2] {
    let y = bounds.y + 1 + summary_height(bounds, data);
    [
        (
            View::Main,
            super::protocol::Rect {
                x: bounds.x + 1,
                y,
                cols: 9,
                rows: 1,
            },
        ),
        (
            View::Io,
            super::protocol::Rect {
                x: bounds.x + 10,
                y,
                cols: 6,
                rows: 1,
            },
        ),
    ]
}
pub fn columns(width: u16, view: View, data: &Snapshot) -> Vec<(&'static str, u16, Option<Sort>)> {
    let pid_width = data
        .processes
        .iter()
        .map(|process| process.pid.to_string().len())
        .max()
        .unwrap_or(0)
        .max(7) as u16
        + 1;
    let mut cols = vec![("PID", pid_width, Some(Sort::Pid))];
    if width >= 65 {
        cols.push(("USER", 10, None));
    }
    if view == View::Main {
        cols.push(("CPU%", 7, Some(Sort::Cpu)));
        cols.push(("MEM%", 7, Some(Sort::Memory)));
        if width >= 80 {
            cols.push(("RES", 9, Some(Sort::Memory)));
            cols.push(("TIME", 9, None));
            cols.push(("S", 3, None));
        }
    } else {
        cols.push(("READ/s", 10, Some(Sort::Read)));
        cols.push(("WRITE/s", 10, Some(Sort::Write)));
    }
    cols.push(("COMMAND", 0, None));
    cols
}
pub fn metric_region(frame: &super::protocol::Frame) -> Option<super::protocol::Rect> {
    use super::{protocol::Rect, workspace};
    if frame.sidebar_status() && frame.ui.config.sidebar_folded {
        return None;
    }
    if frame.ui.vitals.is_empty() || frame.notice.is_some() {
        return None;
    }
    Some(if frame.sidebar_status() {
        let height = workspace::sidebar_status_rows(frame.rows);
        Rect {
            x: 0,
            y: frame.rows - height,
            cols: frame.sidebar_width() - 1,
            rows: height - 3 * workspace::sidebar_button_rows(frame.rows),
        }
    } else {
        Rect {
            x: frame.footer_cols(),
            y: frame.rows - 1,
            cols: frame.cols.saturating_sub(frame.footer_cols()),
            rows: 1,
        }
    })
}

pub fn metric_target(
    frame: &super::protocol::Frame,
    point: super::protocol::Point,
) -> Option<Sort> {
    if !metric_region(frame)?.contains(point) {
        return None;
    }
    if frame.sidebar_status() {
        if super::render::sidebar_disk_region(frame).is_some_and(|rect| rect.contains(point)) {
            return Some(Sort::Read);
        }
        let width = frame.sidebar_width() - 1;
        let start = usize::from(point.y * frame.cols);
        let text: String = frame.cells[start..start + usize::from(width)]
            .iter()
            .map(|cell| cell.text.as_str())
            .collect();
        let text = text.trim_start();
        if text.starts_with("Memory")
            || text.starts_with("Swap")
            || text.starts_with("RAM")
            || text.starts_with("SWP")
        {
            Some(Sort::Memory)
        } else if text.starts_with("Disks") || text.starts_with("DSK") {
            Some(Sort::Read)
        } else {
            Some(Sort::Cpu)
        }
    } else if point.y == frame.rows - 1 && point.x >= frame.footer_cols() {
        let cell = &frame.cells[usize::from(point.y * frame.cols + point.x)];
        Some(match cell.fg {
            super::protocol::Color::Indexed(141 | 73) => Sort::Memory,
            super::protocol::Color::Indexed(222 | 196) => Sort::Read,
            _ => Sort::Cpu,
        })
    } else {
        None
    }
}
