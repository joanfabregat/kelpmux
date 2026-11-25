// The archive format on disposable folders, the checksums against known
// values, and the job lock and state files. The upload, the verification
// and restoring against a stand-in for Cloud Storage are in
// tests/archive_gcs.rs.
use super::*;
use std::{
    os::unix::fs::{MetadataExt, PermissionsExt},
    sync::atomic::AtomicU64,
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kelpmux-archive-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn checksums_and_times_match_known_values() {
    // The CRC32C check value, and GCS's base64 of an empty object's.
    assert_eq!(tarball::crc32c(b"123456789"), 0xE306_9283);
    assert_eq!(tarball::crc32c_base64(tarball::crc32c(b"")), "AAAAAA==");
    let mut split = tarball::Crc32c::default();
    split.update(b"1234");
    split.update(b"56789");
    assert_eq!(split.value(), 0xE306_9283);
    let mut digests = tarball::Digests::default();
    digests.update(b"abc");
    let (sha, crc, length) = digests.finish();
    assert_eq!(
        sha,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!((crc, length), (tarball::crc32c(b"abc"), 3));
    assert_eq!(utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(utc(1_795_509_000), "2026-11-24T08:30:00Z");
    assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
    assert_eq!(
        gcs::encode("kelpmux/p/a b.tar.gz"),
        "kelpmux%2Fp%2Fa%20b.tar.gz"
    );
}

fn tree(folder: &Path) {
    fs::create_dir_all(folder.join("src/deep/er")).unwrap();
    fs::create_dir(folder.join("empty")).unwrap();
    fs::write(folder.join("README"), "hello\n").unwrap();
    fs::write(folder.join("src/main.rs"), "fn main() {}\n").unwrap();
    let script = folder.join("src/run.sh");
    fs::write(&script, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o750)).unwrap();
    // A name longer than a ustar header holds, and a long link target.
    let long = "n".repeat(120);
    fs::write(folder.join("src/deep/er").join(&long), vec![7u8; 70_000]).unwrap();
    std::os::unix::fs::symlink(
        format!("../{}", "t".repeat(150)),
        folder.join("src/long-link"),
    )
    .unwrap();
    std::os::unix::fs::symlink("/etc/passwd", folder.join("absolute-link")).unwrap();
    std::os::unix::fs::symlink("README", folder.join("relative-link")).unwrap();
    std::os::unix::fs::symlink("missing", folder.join("dangling")).unwrap();
    // A FIFO is skipped.
    let fifo = std::ffi::CString::new(folder.join("pipe").as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    // A read-only folder, and an old modification time.
    fs::set_permissions(folder.join("empty"), fs::Permissions::from_mode(0o555)).unwrap();
    let old = std::ffi::CString::new(folder.join("README").as_os_str().as_encoded_bytes()).unwrap();
    let times = [
        libc::timespec {
            tv_sec: 1_000_000_000,
            tv_nsec: 0,
        },
        libc::timespec {
            tv_sec: 1_000_000_000,
            tv_nsec: 0,
        },
    ];
    assert_eq!(
        unsafe { libc::utimensat(libc::AT_FDCWD, old.as_ptr(), times.as_ptr(), 0) },
        0
    );
}

#[test]
fn a_folder_round_trips_with_links_modes_and_times() {
    let temp = Temp::new();
    let source = temp.0.join("source");
    tree(&source);
    fs::set_permissions(&source, fs::Permissions::from_mode(0o750)).unwrap();
    let out = temp.0.join("a.tar.gz");
    let progress = Progress::default();
    let packed = tarball::pack(&source, File::create(&out).unwrap(), &progress).unwrap();
    assert_eq!(packed.skipped, vec!["pipe".to_owned()]);
    assert_eq!(packed.bytes, 6 + 13 + 10 + 70_000);
    assert_eq!(progress.bytes(), packed.bytes);
    assert_eq!(packed.size, fs::metadata(&out).unwrap().len());
    let mut digests = tarball::Digests::default();
    digests.update(&fs::read(&out).unwrap());
    let (sha, crc, size) = digests.finish();
    assert_eq!(
        (sha, crc, size),
        (packed.sha256.clone(), packed.crc32c, packed.size)
    );
    // Counted like the tree.
    let tally = tarball::tally(&source, (i64::MAX, 0)).unwrap();
    assert_eq!((tally.entries, tally.bytes), (packed.entries, packed.bytes));
    assert!(!tally.changed_since);
    let dest = temp.0.join("dest");
    fs::create_dir(&dest).unwrap();
    let unpacked = tarball::unpack(
        File::open(&out).unwrap(),
        &dest,
        tarball::Limits {
            entries: packed.entries,
            bytes: packed.bytes,
        },
        &Progress::default(),
    )
    .unwrap();
    assert_eq!(unpacked.entries, packed.entries);
    // The folder's own mode travels as the `./` entry.
    assert_eq!(unpacked.root_mode, Some(0o750));
    assert_eq!(unpacked.bytes, packed.bytes);
    let long = "n".repeat(120);
    assert_eq!(
        fs::read(dest.join("src/deep/er").join(&long)).unwrap(),
        vec![7u8; 70_000]
    );
    assert_eq!(fs::read_to_string(dest.join("README")).unwrap(), "hello\n");
    assert_eq!(
        fs::metadata(dest.join("README")).unwrap().mtime(),
        1_000_000_000
    );
    assert_eq!(
        fs::metadata(dest.join("src/run.sh")).unwrap().mode() & 0o777,
        0o750
    );
    assert_eq!(
        fs::metadata(dest.join("empty")).unwrap().mode() & 0o777,
        0o555
    );
    for (link, target) in [
        ("absolute-link", "/etc/passwd".to_owned()),
        ("relative-link", "README".to_owned()),
        ("dangling", "missing".to_owned()),
        ("src/long-link", format!("../{}", "t".repeat(150))),
    ] {
        let meta = fs::symlink_metadata(dest.join(link)).unwrap();
        assert!(meta.file_type().is_symlink(), "{link}");
        assert_eq!(
            fs::read_link(dest.join(link)).unwrap(),
            PathBuf::from(target)
        );
    }
    assert!(fs::symlink_metadata(dest.join("pipe")).is_err());
    fs::set_permissions(dest.join("empty"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(source.join("empty"), fs::Permissions::from_mode(0o755)).unwrap();
    // A change after a time shows in the tally.
    let since = tarball::ctime_now();
    std::thread::sleep(Duration::from_millis(20));
    fs::write(source.join("README"), "changed\n").unwrap();
    assert!(tarball::tally(&source, since).unwrap().changed_since);
}

/// A tar.gz of raw members: (name, kind, link target, content), names
/// written into the header as they are (no path checks).
fn raw_archive(path: &Path, members: &[(&str, tar::EntryType, &str, &[u8])]) {
    let file = File::create(path).unwrap();
    let gzip = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    let mut builder = tar::Builder::new(gzip);
    for (name, kind, link, content) in members {
        let mut header = tar::Header::new_old();
        {
            let old = header.as_old_mut();
            old.name[..name.len()].copy_from_slice(name.as_bytes());
            old.linkname[..link.len()].copy_from_slice(link.as_bytes());
        }
        header.set_entry_type(*kind);
        header.set_mode(0o644);
        header.set_size(content.len() as u64);
        header.set_cksum();
        builder.append(&header, *content).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap();
}

#[test]
fn unsafe_members_are_refused_and_nothing_escapes() {
    let temp = Temp::new();
    let outside = temp.0.join("outside");
    fs::create_dir(&outside).unwrap();
    for (case, members, expected) in [
        (
            "dotdot",
            vec![("../escaped", tar::EntryType::Regular, "", &b"x"[..])],
            "contains `..`",
        ),
        (
            "absolute",
            vec![(
                "/tmp/kelpmux-absolute",
                tar::EntryType::Regular,
                "",
                &b"x"[..],
            )],
            "absolute path",
        ),
        (
            // A link to outside, then a file through it.
            "through-link",
            vec![
                (
                    "out",
                    tar::EntryType::Symlink,
                    outside.to_str().unwrap(),
                    &b""[..],
                ),
                ("out/planted", tar::EntryType::Regular, "", &b"x"[..]),
            ],
            "not a folder",
        ),
        (
            "replace",
            vec![
                ("twice", tar::EntryType::Regular, "", &b"x"[..]),
                ("twice", tar::EntryType::Regular, "", &b"y"[..]),
            ],
            "exists",
        ),
        (
            "hard-link",
            vec![("hard", tar::EntryType::Link, "/etc/passwd", &b""[..])],
            "does not restore",
        ),
    ] {
        let archive = temp.0.join(format!("{case}.tar.gz"));
        raw_archive(&archive, &members);
        let dest = temp.0.join(case);
        fs::create_dir(&dest).unwrap();
        let error = tarball::unpack(
            File::open(&archive).unwrap(),
            &dest,
            LOOSE,
            &Progress::default(),
        )
        .unwrap_err();
        assert!(error.contains(expected), "{case}: {error}");
    }
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    assert!(!temp.0.join("escaped").exists());
    assert!(!Path::new("/tmp/kelpmux-absolute").exists());
    assert_eq!(
        tarball::member_path(Path::new("./a/./b")).unwrap(),
        Some(PathBuf::from("a/b"))
    );
    assert_eq!(tarball::member_path(Path::new(".")).unwrap(), None);
}

#[test]
fn packing_stops_when_cancelled() {
    let temp = Temp::new();
    let source = temp.0.join("source");
    tree(&source);
    let progress = Progress::default();
    progress.cancel();
    let error = tarball::pack(
        &source,
        File::create(temp.0.join("c.tar.gz")).unwrap(),
        &progress,
    )
    .unwrap_err();
    assert_eq!(error, "cancelled");
    fs::set_permissions(source.join("empty"), fs::Permissions::from_mode(0o755)).unwrap();
}

fn context(temp: &Temp) -> Context {
    Context {
        config_path: temp.0.join("config/config.toml"),
        env: user_config::Env {
            home: temp.0.join("home"),
            check_dirs: false,
        },
        state: temp.0.join("state"),
        cache: temp.0.join("cache"),
        store: None,
    }
}

#[test]
fn one_job_per_project_with_its_state_and_cancel_request() {
    let temp = Temp::new();
    let ctx = context(&temp);
    let job = Job::start(&ctx, "kelp", "archive", None).unwrap();
    assert_eq!(
        Job::start(&ctx, "kelp", "restore", None).err().as_deref(),
        Some(LOCK_BUSY)
    );
    // Another project is free.
    let other = Job::start(&ctx, "web", "delete", None).unwrap();
    other.finish(&Ok(json!({"deleted": true})));
    job.phase("packing", "Packing", Some(10), true);
    job.progress().add_bytes(4);
    let listed = jobs(&ctx);
    let running = listed["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["project"] == "kelp")
        .unwrap()
        .clone();
    assert_eq!(running["state"], "running");
    assert_eq!(running["phase"], "packing");
    // A cancel request reaches the job's progress.
    cancel(&ctx, "kelp").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !job.progress().cancelled() {
        assert!(Instant::now() < deadline, "the cancel request was not seen");
        thread::sleep(Duration::from_millis(20));
    }
    let state = job.finish(&Err("cancelled".into()));
    assert_eq!(state["state"], "cancelled");
    assert!(cancel(&ctx, "kelp").unwrap_err().contains("not running"));
    // Removal cannot be cancelled.
    let job = Job::start(&ctx, "kelp", "delete", None).unwrap();
    job.phase("removing", "Deleting", None, false);
    assert!(
        cancel(&ctx, "kelp")
            .unwrap_err()
            .contains("no longer be cancelled")
    );
    drop(job);
    // A job whose process ended while running shows as failed.
    let mut stale = jobs(&ctx);
    let stale = stale["jobs"]
        .as_array_mut()
        .unwrap()
        .iter()
        .find(|j| j["project"] == "kelp")
        .unwrap()
        .clone();
    assert_eq!(stale["state"], "failed", "{stale}");
    assert!(Job::start(&ctx, "../x", "archive", None).is_err());
}

#[test]
fn objects_of_another_project_are_never_restored() {
    let record = ArchiveRecord {
        backend: "gcs".into(),
        bucket: "b-1".into(),
        object: "old/kelp/1.tar.gz".into(),
        size: 1,
        sha256: "0".repeat(64),
        crc32c: "AAAAAA==".into(),
        archived_at: utc(0),
        entries: None,
        unpacked_bytes: None,
    };
    assert!(object_allowed(
        "kelpmux/kelp/2.tar.gz",
        "kelp",
        "kelpmux",
        None
    ));
    assert!(object_allowed(
        "old/kelp/1.tar.gz",
        "kelp",
        "kelpmux",
        Some(&record)
    ));
    assert!(!object_allowed(
        "kelpmux/kelpweb/2.tar.gz",
        "kelp",
        "kelpmux",
        None
    ));
    assert!(!object_allowed(
        "kelpmux/kelp/../web/2.tar.gz",
        "kelp",
        "kelpmux",
        None
    ));
    assert!(!object_allowed(
        "other/kelp/2.tar.gz",
        "kelp",
        "kelpmux",
        Some(&record)
    ));
}

const LOOSE: tarball::Limits = tarball::Limits {
    entries: 1000,
    bytes: 1 << 30,
};

#[test]
fn a_decompression_bomb_stops_at_its_limits() {
    use std::io::Read as _;
    let temp = Temp::new();
    // 64 MiB of zeros compress to a few dozen KiB.
    let bomb = temp.0.join("bomb.tar.gz");
    {
        let gzip = flate2::write::GzEncoder::new(
            File::create(&bomb).unwrap(),
            flate2::Compression::best(),
        );
        let mut builder = tar::Builder::new(gzip);
        let mut header = tar::Header::new_gnu();
        header.set_size(64 << 20);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        builder
            .append_data(&mut header, "zeros", std::io::repeat(0).take(64 << 20))
            .unwrap();
        for n in 0..50 {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            builder
                .append_data(&mut header, format!("empty-{n}"), std::io::empty())
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
    }
    let compressed = fs::metadata(&bomb).unwrap().len();
    assert!(compressed < 1 << 20, "{compressed}");
    for (limits, expected) in [
        (
            tarball::Limits {
                entries: 1000,
                bytes: 1 << 20,
            },
            "more bytes",
        ),
        (
            tarball::Limits {
                entries: 10,
                bytes: 1 << 30,
            },
            "more entries",
        ),
    ] {
        let dest = temp.0.join(format!("dest-{}", limits.entries));
        fs::create_dir(&dest).unwrap();
        let error = tarball::unpack(
            File::open(&bomb).unwrap(),
            &dest,
            limits,
            &Progress::default(),
        )
        .unwrap_err();
        assert!(error.contains(expected), "{error}");
        // Stopped early: at most the limit was written.
        let written = fs::metadata(dest.join("zeros"))
            .map(|m| m.len())
            .unwrap_or(0);
        assert!(written <= 1 << 30);
    }
    // Within the counts it was made with, it unpacks.
    let dest = temp.0.join("whole");
    fs::create_dir(&dest).unwrap();
    let whole = tarball::Limits {
        entries: 51,
        bytes: 64 << 20,
    };
    let unpacked = tarball::unpack(
        File::open(&bomb).unwrap(),
        &dest,
        whole,
        &Progress::default(),
    )
    .unwrap();
    assert_eq!((unpacked.entries, unpacked.bytes), (51, 64 << 20));
    // An archive without a `./` entry says nothing of the folder's mode.
    assert_eq!(unpacked.root_mode, None);
}

#[test]
fn restore_limits_come_from_the_record_the_metadata_or_a_ceiling() {
    let mut object = gcs::Object {
        name: "kelpmux/p/1.tar.gz".into(),
        size: 1000,
        crc32c: "AAAAAA==".into(),
        generation: "1".into(),
        updated: String::new(),
        sha256: None,
        metadata: serde_json::Map::new(),
    };
    let free = 100 << 30;
    // No counts: 64 times the size plus 1 GiB, and the entry ceiling.
    let limits = restore_limits(None, &object, free).unwrap();
    assert_eq!(
        (limits.entries, limits.bytes),
        (MAX_ENTRIES, 64_000 + (1 << 30))
    );
    // ... never past the free space less the reserve.
    let limits = restore_limits(None, &object, RESERVE + 10).unwrap();
    assert_eq!(limits.bytes, 10);
    // The object's metadata.
    object.metadata.insert("kelpmux-entries".into(), json!("7"));
    object
        .metadata
        .insert("kelpmux-bytes".into(), json!("4096"));
    let limits = restore_limits(None, &object, free).unwrap();
    assert_eq!((limits.entries, limits.bytes), (7, 4096));
    // The record wins for its own object.
    let record = ArchiveRecord {
        backend: "gcs".into(),
        bucket: "b-1".into(),
        object: object.name.clone(),
        size: 1000,
        sha256: "0".repeat(64),
        crc32c: "AAAAAA==".into(),
        archived_at: utc(0),
        entries: Some(5),
        unpacked_bytes: Some(2048),
    };
    let limits = restore_limits(Some(&record), &object, free).unwrap();
    assert_eq!((limits.entries, limits.bytes), (5, 2048));
    // Not enough room: refused before anything is written.
    let error = restore_limits(Some(&record), &object, RESERVE + 100).unwrap_err();
    assert!(error.contains("only"), "{error}");
}
