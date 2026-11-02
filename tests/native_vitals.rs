use kelpmux::native::vitals::{
    self, Sampler, Vitals, bar_glyph,
    cpu::{Sample, State},
    disk::{Space, df_percent},
    pie_glyph, render_segments,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kelpmux-vitals-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("self")).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn cpu_rounding_short_intervals_and_reset_use_in_memory_state() {
    let sample = Sample {
        total: 1200,
        idle: 850,
        cpus: 2,
    };
    let old = State {
        total: 1000,
        idle: 800,
        percent: 42,
    };
    assert_eq!(vitals::cpu::percent(Some(old), sample).0, 75);
    assert_eq!(
        vitals::cpu::percent(
            Some(old),
            Sample {
                total: 1099,
                idle: 850,
                cpus: 2
            }
        ),
        (42, None)
    );
    assert_eq!(
        vitals::cpu::percent(
            Some(old),
            Sample {
                total: 10,
                idle: 5,
                cpus: 2
            }
        )
        .0,
        0
    );
    assert!(vitals::cpu::parse_stat("cpu 18446744073709551615 1 1 1\n").is_none());
    assert_eq!(vitals::mem::rounded_percent(u64::MAX, u64::MAX), 100);
    assert_eq!(
        df_percent(Space {
            size: u64::MAX,
            free: 0,
            available: u64::MAX
        }),
        Some(50)
    );
    assert_eq!(
        df_percent(Space {
            size: 0,
            free: 0,
            available: 0
        }),
        None
    );
}
#[test]
fn sampler_caches_metrics_without_creating_any_cpu_state_file() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("stat"),
        "cpu 100 0 0 900\ncpu0 100 0 0 900\n",
    )
    .unwrap();
    fs::write(
        fixture.0.join("meminfo"),
        "MemTotal: 1000 kB\nMemAvailable: 750 kB\n",
    )
    .unwrap();
    fs::write(fixture.0.join("self/mounts"), "").unwrap();
    let begin = Instant::now();
    let mut sampler = Sampler::with_proc_root(&fixture.0);
    let first = sampler.sample_at(begin);
    assert_eq!(first.cpu, Some(0));
    assert_eq!(first.memory, Some(25));
    assert_eq!(first.swap, Some(0));
    fs::write(
        fixture.0.join("stat"),
        "cpu 175 0 0 925\ncpu0 175 0 0 925\n",
    )
    .unwrap();
    fs::write(
        fixture.0.join("meminfo"),
        "MemTotal: 1000 kB\nMemAvailable: 500 kB\n",
    )
    .unwrap();
    assert_eq!(sampler.sample_at(begin + Duration::from_millis(125)), first);
    let next = sampler.sample_at(begin + Duration::from_secs(1));
    assert_eq!(next.cpu, Some(75));
    assert_eq!(next.memory, Some(50));
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 3);
}
#[test]
fn disk_refresh_has_an_independent_five_second_cache() {
    let fixture = Fixture::new();
    let space = vitals::disk::statvfs("/").unwrap();
    // Match a real filesystem only if the runner root meets the minimum size.
    fs::write(
        fixture.0.join("self/mounts"),
        "/dev/missing /nonexistent-kelpmux-mount ext4 rw 0 0\n/dev/fixture / ext4 rw 0 0\n",
    )
    .unwrap();
    let begin = Instant::now();
    let mut sampler = Sampler::with_proc_root(&fixture.0);
    let first = sampler.sample_at(begin);
    assert_eq!(first.disks.len(), usize::from(space.size >= 10 << 30));
    let labeled = sampler.disk_readings().to_vec();
    assert_eq!(
        labeled,
        first
            .disks
            .iter()
            .map(|percent| ("/".into(), *percent))
            .collect::<Vec<_>>()
    );
    fs::write(fixture.0.join("self/mounts"), "").unwrap();
    assert_eq!(
        sampler.sample_at(begin + Duration::from_secs(4)).disks,
        first.disks
    );
    assert_eq!(sampler.disk_readings(), labeled);
    assert!(
        sampler
            .sample_at(begin + Duration::from_secs(5))
            .disks
            .is_empty()
    );
    assert!(sampler.disk_readings().is_empty());
}
#[test]
fn unavailable_metrics_disappear_instead_of_reporting_invented_zero() {
    let fixture = Fixture::new();
    let mut sampler = Sampler::with_proc_root(&fixture.0);
    let snapshot = sampler.sample();
    assert_eq!(snapshot.cpu, None);
    assert_eq!(snapshot.memory, None);
    assert_eq!(snapshot.swap, None);
    assert!(snapshot.disks.is_empty());
    assert_eq!(snapshot.date.as_ref().unwrap().len(), 10);
    let time = snapshot.time.as_ref().unwrap();
    assert_eq!(time.as_bytes()[2], b':');
    assert!(!time.split_once(' ').unwrap().1.is_empty());
    assert!(render_segments(&Vitals::default()).is_empty());
}
#[test]
fn glyphs_color_segments_and_alert_boundary_match_vitals() {
    assert_eq!(
        [bar_glyph(0), bar_glyph(19), bar_glyph(50), bar_glyph(94)],
        ['▁', '▂', '▄', '█']
    );
    assert_eq!(
        [pie_glyph(12), pie_glyph(13), pie_glyph(63), pie_glyph(88)],
        ['○', '◔', '◕', '●']
    );
    let data = Vitals {
        cpu: Some(26),
        memory: Some(7),
        swap: Some(0),
        disks: vec![89, 90],
        date: Some("2026-10-05".into()),
        time: Some("21:01".into()),
    };
    assert_eq!(
        render_segments(&data),
        vec![
            ("cpu ▂ 26%".into(), 114),
            ("mem ▁ 7%".into(), 141),
            ("swap ▁ 0%".into(), 73),
            ("disk".into(), 222),
            ("● 89%".into(), 222),
            ("● 90%".into(), 196),
            ("2026-10-05".into(), 39),
            ("21:01".into(), 250)
        ]
    );
    assert_eq!(
        serde_json::from_str::<Vitals>(&serde_json::to_string(&data).unwrap()).unwrap(),
        data
    );
}
#[test]
fn mount_selection_unescapes_paths_and_skips_duplicate_or_virtual_devices() {
    let mounts = vitals::disk::parse_mounts(
        "/dev/root / ext4 rw 0 0\n/dev/root /bind ext4 rw 0 0\n/dev/data /my\\040data ext4 rw 0 0\n/dev/loop /snap squashfs ro 0 0\noverlay /overlay overlay rw 0 0\n",
    );
    assert_eq!(
        mounts.iter().map(|m| m.target.as_str()).collect::<Vec<_>>(),
        vec!["/", "/my data"]
    );
    assert_eq!(
        vitals::clock::LocalTime {
            year: 2026,
            month: 1,
            day: 2,
            hour: 3,
            minute: 4,
            zone: "UTC".into(),
        }
        .time(),
        "03:04"
    );
    assert_eq!(
        vitals::clock::LocalTime {
            year: 2026,
            month: 1,
            day: 2,
            hour: 3,
            minute: 4,
            zone: "UTC".into(),
        }
        .date(),
        "2026-01-02"
    );
}
