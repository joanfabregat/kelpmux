//! On-demand host status. Never persists process information or reads command arguments.
use super::vitals::{cpu, disk};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum View {
    #[default]
    Main,
    Io,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sort {
    Pid,
    #[default]
    Cpu,
    Memory,
    Read,
    Write,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    pub birth: u64,
    pub user: String,
    pub state: String,
    pub cpu: Option<u32>,
    pub memory: u32,
    pub resident: u64,
    pub runtime: u64,
    pub read: Option<u64>,
    pub write: Option<u64>,
    pub command: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Disk {
    pub mount: String,
    pub used: u64,
    pub total: u64,
    pub percent: u8,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub ready: bool,
    pub cpus: Vec<u8>,
    pub memory_used: u64,
    pub memory_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub load: String,
    pub uptime: u64,
    pub tasks: usize,
    pub running: usize,
    pub network: Option<(u64, u64)>,
    pub disk_io: Option<(u64, u64)>,
    pub disks: Vec<Disk>,
    pub processes: Vec<Process>,
}
type ProcessCounters = (u64, Option<u64>, Option<u64>);
#[derive(Default)]
struct History {
    at: Option<Instant>,
    cpus: Vec<cpu::State>,
    processes: BTreeMap<(u32, u64), ProcessCounters>,
    network: Option<(u64, u64)>,
    disk_io: Option<(u64, u64)>,
}
#[derive(Default)]
pub struct Monitor {
    discard_pending: bool,
    pending: Option<mpsc::Receiver<(Snapshot, History)>>,
    history: History,
    last: Option<Instant>,
    current: Snapshot,
}
impl Monitor {
    pub fn poll(&mut self, open: bool) -> &Snapshot {
        if !open {
            self.discard_pending = true;
            if self.pending.as_ref().is_some_and(|receiver| {
                !matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty))
            }) {
                self.pending = None;
            }
            self.history = History::default();
            self.last = None;
            self.current = Snapshot::default();
            return &self.current;
        }
        if let Some(result) = self.pending.as_ref().map(|receiver| receiver.try_recv()) {
            match result {
                Ok(result) => {
                    if !self.discard_pending {
                        (self.current, self.history) = result;
                    }
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.last = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if open
            && self.pending.is_none()
            && self
                .last
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(1))
        {
            let previous = std::mem::take(&mut self.history);
            self.discard_pending = false;
            let (sender, receiver) = mpsc::channel();
            if std::thread::Builder::new()
                .name("kelpmux-status".into())
                .spawn(move || {
                    let _ = sender.send(sample(Path::new("/proc"), previous, Instant::now()));
                })
                .is_ok()
            {
                self.pending = Some(receiver);
                self.last = Some(Instant::now());
            }
        }
        &self.current
    }
}
fn read(path: impl AsRef<Path>) -> Option<String> {
    let mut text = String::new();
    fs::File::open(path)
        .ok()?
        .take(128 * 1024)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}
fn number(fields: &[&str], index: usize) -> Option<u64> {
    fields.get(index)?.parse().ok()
}
fn delta(now: Option<u64>, before: Option<u64>, elapsed: Option<Duration>) -> Option<u64> {
    let nanos = elapsed?.as_nanos();
    if nanos == 0 {
        return None;
    }
    Some(
        (u128::from(now?.checked_sub(before?)?) * 1_000_000_000 / nanos).min(u128::from(u64::MAX))
            as u64,
    )
}
fn rates(
    now: Option<(u64, u64)>,
    before: Option<(u64, u64)>,
    elapsed: Option<Duration>,
) -> Option<(u64, u64)> {
    Some((
        delta(Some(now?.0), Some(before?.0), elapsed)?,
        delta(Some(now?.1), Some(before?.1), elapsed)?,
    ))
}
fn net(text: &str) -> Option<(u64, u64)> {
    let mut sum = (0u64, 0u64);
    let mut found = false;
    for line in text.lines() {
        let Some((name, values)) = line.split_once(':') else {
            continue;
        };
        if name.trim() == "lo" {
            continue;
        }
        let f: Vec<_> = values.split_whitespace().collect();
        sum.0 = sum.0.checked_add(number(&f, 0)?)?;
        sum.1 = sum.1.checked_add(number(&f, 8)?)?;
        found = true;
    }
    found.then_some(sum)
}
fn disk_counters(text: &str) -> Option<(u64, u64)> {
    let mut sum = (0u64, 0u64);
    let mut found = false;
    for line in text.lines() {
        let f: Vec<_> = line.split_whitespace().collect();
        let Some(name) = f.get(2) else {
            continue;
        };
        // Whole block devices only: avoid counting both a disk and its partitions.
        if name.starts_with("loop") || name.starts_with("ram") || name.starts_with("dm-") {
            continue;
        }
        let whole = if name.starts_with("nvme") || name.starts_with("mmcblk") {
            !name.contains('p')
        } else {
            !name.ends_with(|c: char| c.is_ascii_digit())
        };
        if !whole {
            continue;
        }
        sum.0 = sum.0.checked_add(number(&f, 5)?.checked_mul(512)?)?;
        sum.1 = sum.1.checked_add(number(&f, 9)?.checked_mul(512)?)?;
        found = true;
    }
    found.then_some(sum)
}
fn sample(root: &Path, previous: History, now: Instant) -> (Snapshot, History) {
    let elapsed = previous.at.map(|at| now.saturating_duration_since(at));
    let mut history = History {
        at: Some(now),
        ..Default::default()
    };
    let mut snapshot = Snapshot {
        ready: true,
        ..Default::default()
    };
    if let Some(stat) = read(root.join("stat")) {
        for (index, line) in stat
            .lines()
            .filter(|line| {
                line.strip_prefix("cpu")
                    .is_some_and(|tail| tail.starts_with(|c: char| c.is_ascii_digit()))
            })
            .enumerate()
            .take(1024)
        {
            let (_, values) = line.split_once(' ').unwrap_or(("", ""));
            if let Some(sample) = cpu::parse_stat(&format!("cpu {values}")) {
                let (percent, state) = cpu::percent(previous.cpus.get(index).copied(), sample);
                snapshot.cpus.push(percent);
                history.cpus.push(
                    state
                        .or_else(|| previous.cpus.get(index).copied())
                        .unwrap_or(cpu::State {
                            total: sample.total,
                            idle: sample.idle,
                            percent,
                        }),
                );
            }
        }
    }
    let mem: BTreeMap<_, _> = read(root.join("meminfo"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            Some((
                key.to_string(),
                value
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()?
                    .saturating_mul(1024),
            ))
        })
        .collect();
    snapshot.memory_total = *mem.get("MemTotal").unwrap_or(&0);
    snapshot.memory_used = snapshot
        .memory_total
        .saturating_sub(*mem.get("MemAvailable").unwrap_or(&snapshot.memory_total));
    snapshot.swap_total = *mem.get("SwapTotal").unwrap_or(&0);
    snapshot.swap_used = snapshot
        .swap_total
        .saturating_sub(*mem.get("SwapFree").unwrap_or(&snapshot.swap_total));
    snapshot.load = read(root.join("loadavg"))
        .unwrap_or_default()
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");
    snapshot.uptime = read(root.join("uptime"))
        .and_then(|s| s.split('.').next()?.parse().ok())
        .unwrap_or(0);
    history.network = read(root.join("net/dev")).as_deref().and_then(net);
    history.disk_io = read(root.join("diskstats"))
        .as_deref()
        .and_then(disk_counters);
    snapshot.network = rates(history.network, previous.network, elapsed);
    snapshot.disk_io = rates(history.disk_io, previous.disk_io, elapsed);
    if let Some(mounts) = read(root.join("self/mounts")) {
        for mount in disk::parse_mounts(&mounts).into_iter().take(64) {
            if let Some(space) = disk::statvfs(&mount.target).filter(|space| space.size >= 10 << 30)
            {
                snapshot.disks.push(Disk {
                    mount: mount.target,
                    used: space.size.saturating_sub(space.free),
                    total: space.size,
                    percent: disk::df_percent(space).unwrap_or(0),
                });
            }
        }
    }
    let users: BTreeMap<_, _> = read("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split(':').collect();
            Some((
                fields.get(2)?.parse::<u32>().ok()?,
                fields.first()?.to_string(),
            ))
        })
        .collect();
    // SAFETY: sysconf reads process-wide constants and has no pointer arguments.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as u64;
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.take(8192).flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let path = entry.path();
            let Some(stat) = read(path.join("stat")) else {
                continue;
            };
            let Some((name, fields)) = parse_process(&stat) else {
                continue;
            };
            let Some(birth) = number(&fields, 19) else {
                continue;
            };
            let Some(cpu_time) =
                number(&fields, 11).and_then(|a| a.checked_add(number(&fields, 12)?))
            else {
                continue;
            };
            // Kernel threads have no userspace address space.
            if number(&fields, 20) == Some(0) {
                continue;
            }
            let io: BTreeMap<_, _> = read(path.join("io"))
                .unwrap_or_default()
                .lines()
                .filter_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    Some((key.to_owned(), value.trim().parse::<u64>().ok()?))
                })
                .collect();
            let rd = io.get("read_bytes").copied();
            let wr = io.get("write_bytes").copied();
            let before = previous.processes.get(&(pid, birth)).copied();
            let status = read(path.join("status")).unwrap_or_default();
            let uid = status.lines().find_map(|line| {
                line.strip_prefix("Uid:")?
                    .split_whitespace()
                    .next()?
                    .parse::<u32>()
                    .ok()
            });
            let user = uid
                .map(|uid| users.get(&uid).cloned().unwrap_or_else(|| uid.to_string()))
                .unwrap_or_else(|| "?".into());
            let resident = number(&fields, 21).unwrap_or(0).saturating_mul(page);
            let command = fs::read_link(path.join("exe"))
                .ok()
                .map(|path| path.to_string_lossy().chars().take(240).collect())
                .unwrap_or_else(|| name.chars().take(240).collect());
            let stable = read(path.join("stat"))
                .and_then(|s| parse_process(&s).and_then(|(_, f)| number(&f, 19)))
                == Some(birth);
            if !stable {
                continue;
            }
            history.processes.insert((pid, birth), (cpu_time, rd, wr));
            snapshot.tasks += number(&fields, 17).unwrap_or(1) as usize;
            snapshot.running += usize::from(fields.first() == Some(&"R"));
            snapshot.processes.push(Process {
                pid,
                birth,
                user,
                state: fields.first().unwrap_or(&"?").to_string(),
                cpu: before
                    .and_then(|before| {
                        delta(
                            Some(cpu_time.saturating_mul(1000)),
                            Some(before.0.saturating_mul(1000)),
                            elapsed,
                        )
                    })
                    .map(|rate| (rate / ticks).min(u64::from(u32::MAX)) as u32),
                memory: (u128::from(resident) * 1000 / u128::from(snapshot.memory_total.max(1)))
                    .min(u128::from(u32::MAX)) as u32,
                resident,
                runtime: cpu_time / ticks,
                read: delta(rd, before.and_then(|b| b.1), elapsed),
                write: delta(wr, before.and_then(|b| b.2), elapsed),
                command,
            });
        }
    }
    (snapshot, history)
}
fn parse_process(stat: &str) -> Option<(&str, Vec<&str>)> {
    let left = stat.find('(')?;
    let right = stat.rfind(')')?;
    (right > left).then(|| {
        (
            &stat[left + 1..right],
            stat[right + 1..].split_whitespace().collect(),
        )
    })
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
    let width = cols.saturating_sub(4).min(160);
    let height = rows.saturating_sub(2).min(60);
    super::protocol::Rect {
        x: (cols - width) / 2,
        y: (rows - height) / 2,
        cols: width,
        rows: height,
    }
}
pub fn summary_columns(bounds: super::protocol::Rect) -> u16 {
    if bounds.cols >= 86 { 2 } else { 1 }
}
pub fn summary_height(bounds: super::protocol::Rect, data: &Snapshot) -> u16 {
    let desired = if !data.ready {
        1
    } else if summary_columns(bounds) == 2 {
        data.cpus.len().div_ceil(2) + 1 + data.disks.len().max(5)
    } else {
        data.cpus.len() + 2 + 4 + data.disks.len()
    };
    (desired.min(usize::from(u16::MAX)) as u16)
        .min(bounds.rows.saturating_sub(7))
        .min((bounds.rows / 2).max(1))
}
pub fn table_y(bounds: super::protocol::Rect, data: &Snapshot) -> u16 {
    bounds.y + 3 + summary_height(bounds, data)
}
pub fn columns(width: u16, view: View) -> Vec<(&'static str, u16, Option<Sort>)> {
    let mut cols = vec![("PID", 7, Some(Sort::Pid))];
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
pub fn metric_target(
    frame: &super::protocol::Frame,
    point: super::protocol::Point,
) -> Option<Sort> {
    if point.x >= frame.cols || point.y >= frame.rows {
        return None;
    }
    if frame.ui.vitals.is_empty() {
        return None;
    }
    if frame.sidebar_status() {
        let width = super::workspace::sidebar_width(frame.cols) - 1;
        let top = frame.rows - super::workspace::sidebar_status_rows(frame.rows);
        let end = if super::workspace::sidebar_status_rows(frame.rows) > 3 {
            frame.rows - 3
        } else {
            frame.rows - 2
        };
        if point.x >= width || point.y < top || point.y >= end {
            return None;
        }
        let start = usize::from(point.y * frame.cols);
        let text: String = frame.cells[start..start + usize::from(width)]
            .iter()
            .map(|cell| cell.text.as_str())
            .collect();
        let text = text.trim_start();
        if text.starts_with("Memory") || text.starts_with("Swap") {
            Some(Sort::Memory)
        } else if text.starts_with("Disks") {
            Some(Sort::Read)
        } else if text.starts_with("CPU") || text.starts_with("Clock") {
            Some(Sort::Cpu)
        } else {
            None
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "kelpmux-status-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("7")).unwrap();
            fs::create_dir(root.join("net")).unwrap();
            fs::create_dir(root.join("self")).unwrap();
            fs::write(
                root.join("meminfo"),
                "MemTotal: 1000 kB\nMemAvailable: 400 kB\nSwapTotal: 100 kB\nSwapFree: 75 kB\n",
            )
            .unwrap();
            fs::write(root.join("loadavg"), "0.10 0.20 0.30 1/20 9").unwrap();
            fs::write(root.join("uptime"), "90061.00 20.0").unwrap();
            fs::write(
                root.join("7/status"),
                "Uid:\t123450\t123450\t123450\t123450\n",
            )
            .unwrap();
            Self(root)
        }
        fn process(&self, birth: u64, cpu: u64, io: u64) {
            let mut fields = vec!["0".to_string(); 22];
            fields[0] = "R".into();
            fields[11] = cpu.to_string();
            fields[17] = "3".into();
            fields[19] = birth.to_string();
            fields[20] = "100000".into();
            fields[21] = "2".into();
            fs::write(
                self.0.join("7/stat"),
                format!("7 (name with ) parentheses) {}", fields.join(" ")),
            )
            .unwrap();
            fs::write(
                self.0.join("7/io"),
                format!("read_bytes: {io}\nwrite_bytes: {}\n", io * 2),
            )
            .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn samples_deltas_memory_cpu_and_reused_pids_without_guessing_missing_io() {
        let f = Fixture::new();
        f.process(10, 100, 1000);
        fs::write(f.0.join("stat"), "cpu 100 0 0 100\ncpu0 100 0 0 100\n").unwrap();
        fs::write(
            f.0.join("net/dev"),
            "eth0: 100 0 0 0 0 0 0 0 200 0 0 0 0 0 0 0\nlo: 900 0 0 0 0 0 0 0 900\n",
        )
        .unwrap();
        fs::write(
            f.0.join("diskstats"),
            "8 0 sda 0 0 10 0 0 0 20 0\n8 1 sda1 0 0 999 0 0 0 999 0\n",
        )
        .unwrap();
        let at = Instant::now();
        let (first, history) = sample(&f.0, History::default(), at);
        assert_eq!(first.processes[0].cpu, None);
        assert_eq!(first.processes[0].read, None);
        assert_eq!(first.network, None);
        assert_eq!(first.memory_used, 600 * 1024);
        assert_eq!(first.swap_used, 25 * 1024);
        assert_eq!(first.tasks, 3);
        assert_eq!(first.uptime, 90061);
        f.process(10, 150, 3000);
        fs::write(f.0.join("stat"), "cpu 175 0 0 125\ncpu0 175 0 0 125\n").unwrap();
        fs::write(
            f.0.join("net/dev"),
            "eth0: 300 0 0 0 0 0 0 0 600 0 0 0 0 0 0 0\n",
        )
        .unwrap();
        fs::write(f.0.join("diskstats"), "8 0 sda 0 0 30 0 0 0 50 0\n").unwrap();
        let (second, history) = sample(&f.0, history, at + Duration::from_secs(1));
        assert_eq!(second.cpus, vec![75]);
        assert!(second.processes[0].cpu.is_some_and(|cpu| cpu > 0));
        assert_eq!(second.processes[0].read, Some(2000));
        assert_eq!(second.processes[0].write, Some(4000));
        assert_eq!(second.network, Some((200, 400)));
        assert_eq!(second.disk_io, Some((10240, 15360)));
        f.process(99, 900, 9000);
        fs::remove_file(f.0.join("7/io")).unwrap();
        let (reused, _) = sample(&f.0, history, at + Duration::from_secs(2));
        assert_eq!(reused.processes[0].cpu, None);
        assert_eq!(reused.processes[0].read, None);
    }
    #[test]
    fn filtering_sorting_and_closed_monitor_are_bounded_and_read_only() {
        let snapshot = Snapshot {
            processes: vec![
                Process {
                    pid: 2,
                    cpu: Some(900),
                    resident: 20,
                    command: "/bin/alpha".into(),
                    ..Default::default()
                },
                Process {
                    pid: 1,
                    cpu: Some(100),
                    resident: 50,
                    command: "/bin/beta".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(processes(&snapshot, "", Sort::Cpu, true)[0].pid, 2);
        assert_eq!(processes(&snapshot, "", Sort::Memory, true)[0].pid, 1);
        assert_eq!(processes(&snapshot, "ALPHA", Sort::Pid, false).len(), 1);
        assert!(processes(&snapshot, "missing", Sort::Cpu, true).is_empty());
        assert_eq!(delta(Some(1), Some(2), Some(Duration::from_secs(1))), None);
        let mut monitor = Monitor::default();
        assert!(!monitor.poll(false).ready);
        assert!(monitor.pending.is_none());
    }
}
