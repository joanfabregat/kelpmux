//! File uploads on the stream protocol (capability `uploads`).
use base64::{Engine, engine::general_purpose::STANDARD};
use kelpserver::native::{
    protocol::*,
    server::{ServerConfig, serve},
    stream::{self, Event, Request},
    uploads::{self, FileSpec, Registered},
};
use std::{
    fs,
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

struct Daemon {
    root: PathBuf,
    thread: Option<thread::JoinHandle<kelpserver::Result<()>>>,
}
impl Daemon {
    fn start() -> Self {
        Self::start_with(Some(7), |_, _| {})
    }
    /// `prepare` gets the state directory and the upload root before the
    /// server starts.
    fn start_with(retention_days: Option<u32>, prepare: impl FnOnce(&Path, &Path)) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "kelpserver-uploads-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        // Destinations are answered in the canonical spelling.
        let root = fs::canonicalize(&root).unwrap();
        let state = root.join("state");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        let uploads = root.join("uploads");
        let extra = root.with_extension("extra");
        let _ = fs::remove_dir_all(&extra);
        fs::create_dir(&extra).unwrap();
        prepare(&state, &uploads);
        let config = ServerConfig {
            socket: root.join("control.sock"),
            state_dir: state,
            shell: "/bin/sh".into(),
            cwd: root.clone(),
            cols: 80,
            rows: 24,
            uploads: uploads::Settings {
                dir: Some(uploads),
                retention_days,
            },
            // Destinations must lie inside a file root: the test's folder,
            // or its sibling `<folder>.extra`, an extra root.
            files: kelpserver::native::files::Settings {
                root: Some(root.clone()),
                extra_roots: vec![extra],
                ..Default::default()
            },
        };
        let handle = thread::spawn(move || serve(config));
        let daemon = Self {
            root,
            thread: Some(handle),
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(daemon.root.join("control.sock")).is_err() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        daemon
    }
    /// The extra file root, beside the test's folder.
    fn extra(&self) -> PathBuf {
        self.root.with_extension("extra")
    }
    fn uploads(&self) -> PathBuf {
        self.root.join("uploads")
    }
    fn registry(&self) -> Vec<Registered> {
        serde_json::from_slice(&fs::read(self.root.join("state/uploads.json")).unwrap()).unwrap()
    }
    fn peer(&self) -> Peer {
        let socket = UnixStream::connect(self.root.join("control.sock")).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut peer = Peer {
            socket,
            welcome: None,
            next_request: 0,
        };
        peer.send(Request::Hello {
            version: stream::VERSION,
        });
        loop {
            let event = peer.next();
            if matches!(event, Event::Welcome { .. }) {
                peer.welcome = Some(event);
                return peer;
            }
        }
    }
}
impl Drop for Daemon {
    /// Ask for a shutdown and wait for its reply; retried, and bounded, so a
    /// lost request fails the test with a reason instead of hanging it.
    fn drop(&mut self) {
        let mut attempts = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.thread.as_ref().is_some_and(|t| !t.is_finished()) {
            if Instant::now() >= deadline {
                if !thread::panicking() {
                    panic!("server did not stop: {attempts:?}");
                }
                return;
            }
            let outcome = (|| -> Result<(), String> {
                let mut s = UnixStream::connect(self.root.join("control.sock"))
                    .map_err(|e| format!("connect: {e}"))?;
                s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                for request in [
                    Request::Hello {
                        version: stream::VERSION,
                    },
                    Request::Shutdown {
                        request: "drop".into(),
                    },
                ] {
                    write_message(&mut s, &ClientMessage::Stream(request))
                        .map_err(|e| format!("write: {e}"))?;
                }
                loop {
                    match read_message::<ServerMessage>(&mut s) {
                        Ok(Some(ServerMessage::Stream(Event::Done { .. }))) => return Ok(()),
                        Ok(Some(_)) => {}
                        Ok(None) => return Err("closed before the reply".into()),
                        Err(e) => return Err(format!("read: {e}")),
                    }
                }
            })();
            if let Err(error) = outcome {
                attempts.push(error);
                thread::sleep(Duration::from_millis(100));
                continue;
            }
            let stopped = Instant::now() + Duration::from_secs(5);
            while self.thread.as_ref().is_some_and(|t| !t.is_finished()) && Instant::now() < stopped
            {
                thread::sleep(Duration::from_millis(10));
            }
        }
        if let Some(t) = self.thread.take() {
            assert!(t.join().unwrap().is_ok());
        }
        let _ = fs::remove_dir_all(&self.root);
        let _ = fs::remove_dir_all(self.extra());
    }
}

struct Peer {
    socket: UnixStream,
    welcome: Option<Event>,
    next_request: u64,
}
impl Peer {
    fn send(&mut self, request: Request) {
        write_message(&mut self.socket, &ClientMessage::Stream(request)).unwrap();
    }
    /// The next event other than a metadata update.
    fn next(&mut self) -> Event {
        loop {
            match read_message(&mut self.socket).unwrap().unwrap() {
                ServerMessage::Stream(Event::Update(_)) => continue,
                ServerMessage::Stream(event) => return event,
                other => panic!("non-stream message {other:?}"),
            }
        }
    }
    fn id(&mut self) -> String {
        self.next_request += 1;
        format!("u{}", self.next_request)
    }
    fn begin(
        &mut self,
        files: &[(&str, u64)],
        dest: Option<&Path>,
    ) -> Result<(u64, PathBuf), String> {
        let request = self.id();
        self.send(Request::UploadBegin {
            request: request.clone(),
            files: files
                .iter()
                .map(|(name, size)| FileSpec {
                    name: (*name).into(),
                    size: *size,
                })
                .collect(),
            dest: dest.map(Path::to_path_buf),
        });
        match self.next() {
            Event::UploadStarted {
                request: echoed,
                batch,
                dir,
            } if echoed == request => Ok((batch, dir)),
            Event::Error {
                message,
                request: Some(echoed),
            } if echoed == request => Err(message),
            other => panic!("unexpected reply {other:?}"),
        }
    }
    fn chunk(&mut self, batch: u64, file_index: usize, offset: u64, bytes: &[u8]) {
        self.send(Request::UploadChunk {
            batch,
            file_index,
            offset,
            data: STANDARD.encode(bytes),
        });
    }
    fn finish(&mut self, batch: u64) -> Result<Vec<PathBuf>, String> {
        let request = self.id();
        self.send(Request::UploadFinish {
            request: request.clone(),
            batch,
        });
        loop {
            match self.next() {
                Event::UploadDone {
                    request: echoed,
                    batch: done,
                    paths,
                } if echoed == request => {
                    assert_eq!(done, batch);
                    return Ok(paths);
                }
                Event::Error {
                    message,
                    request: Some(echoed),
                } if echoed == request => return Err(message),
                Event::UploadProgress { .. } => {}
                other => panic!("unexpected reply {other:?}"),
            }
        }
    }
    fn cancel(&mut self, batch: u64) -> Result<(), String> {
        let request = self.id();
        self.send(Request::UploadCancel {
            request: request.clone(),
            batch,
        });
        match self.next() {
            Event::Done { request: echoed } if echoed == request => Ok(()),
            Event::Error {
                message,
                request: Some(echoed),
            } if echoed == request => Err(message),
            other => panic!("unexpected reply {other:?}"),
        }
    }
    /// Upload whole files in order, keeping at most `window` chunks in flight.
    fn upload(&mut self, batch: u64, files: &[&[u8]], window: usize) {
        let chunk = uploads::MAX_CHUNK_BYTES;
        let mut pending = std::collections::VecDeque::new();
        for (index, bytes) in files.iter().enumerate() {
            for (n, part) in bytes.chunks(chunk).enumerate() {
                if pending.len() == window {
                    self.acknowledged(batch, pending.pop_front().unwrap());
                }
                let offset = (n * chunk) as u64;
                self.chunk(batch, index, offset, part);
                pending.push_back((index, offset + part.len() as u64));
            }
        }
        while let Some(expected) = pending.pop_front() {
            self.acknowledged(batch, expected);
        }
    }
    fn acknowledged(&mut self, batch: u64, (index, received): (usize, u64)) {
        match self.next() {
            Event::UploadProgress {
                batch: acked,
                file_index,
                received: total,
            } => assert_eq!((acked, file_index, total), (batch, index, received)),
            other => panic!("expected upload_progress, got {other:?}"),
        }
    }
    /// The next correlated reply to a diagnostics request, asserting that no
    /// other event arrives first.
    fn barrier(&mut self) {
        let request = self.id();
        self.send(Request::Diagnostics {
            request: request.clone(),
        });
        match self.next() {
            Event::Diagnostics {
                request: echoed, ..
            } if echoed == request => {}
            other => panic!("unexpected event before the barrier: {other:?}"),
        }
    }
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u32).wrapping_mul(2654435761).to_be_bytes()[0] ^ seed)
        .collect()
}
fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().mode() & 0o777
}

#[test]
fn uploads_capability_and_limits_are_advertised() {
    let daemon = Daemon::start();
    let peer = daemon.peer();
    match peer.welcome.clone().unwrap() {
        Event::Welcome {
            capabilities,
            upload_limits,
            ..
        } => {
            assert!(capabilities.iter().any(|c| c == "uploads"));
            let limits = upload_limits.unwrap();
            assert_eq!(limits, uploads::LIMITS);
            assert_eq!(limits.max_files, 64);
            assert_eq!(limits.max_file_bytes, 100 * 1024 * 1024);
            assert_eq!(limits.max_chunk_bytes, 256 * 1024);
            // A base64 chunk record stays far below the 16 MiB record cap.
            assert!(limits.max_chunk_bytes.div_ceil(3) * 4 + 1024 < MAX_MESSAGE / 32);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn two_files_upload_across_chunks_and_are_published_privately() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let big = pattern(uploads::MAX_CHUNK_BYTES * 2 + 12345, 7);
    let small = b"hello upload\n".to_vec();
    let (batch, dir) = peer
        .begin(
            &[
                ("photo.png", big.len() as u64),
                ("notes.txt", small.len() as u64),
                ("empty", 0),
            ],
            None,
        )
        .unwrap();
    assert_eq!(dir.parent().unwrap(), daemon.uploads());
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
    peer.upload(batch, &[&big, &small], uploads::MAX_CHUNKS_IN_FLIGHT);
    let paths = peer.finish(batch).unwrap();
    assert_eq!(
        paths,
        vec![
            dir.join("photo.png"),
            dir.join("notes.txt"),
            dir.join("empty")
        ]
    );
    assert!(paths.iter().all(|p| p.is_absolute()));
    assert_eq!(fs::read(&paths[0]).unwrap(), big);
    assert_eq!(fs::read(&paths[1]).unwrap(), small);
    assert_eq!(fs::read(&paths[2]).unwrap(), b"");
    for path in &paths {
        assert_eq!(mode(path), 0o600);
    }
    // Only the published files remain: no temporary names.
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 3);
    let registry = daemon.registry();
    assert_eq!(registry.len(), 1);
    assert_eq!(registry[0].path, dir);
    assert_eq!(registry[0].ino, fs::metadata(&dir).unwrap().ino());
    assert!(registry[0].completed_at.is_some());
    // A finished batch is no longer addressable.
    assert!(peer.finish(batch).is_err());
}

#[test]
fn names_are_sanitized_and_deduplicated() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let names = [
        "../../evil.txt",
        "evil.txt",
        ".hidden",
        "a\u{7}b\u{202e}c.png",
        "..",
        "dir/sub/",
        "C:\\Users\\me\\report.pdf",
    ];
    let files: Vec<(&str, u64)> = names.iter().map(|n| (*n, 1)).collect();
    let (batch, dir) = peer.begin(&files, None).unwrap();
    for index in 0..names.len() {
        peer.chunk(batch, index, 0, b"x");
        peer.acknowledged(batch, (index, 1));
    }
    let paths = peer.finish(batch).unwrap();
    let stored: Vec<String> = paths
        .iter()
        .map(|p| {
            assert_eq!(p.parent().unwrap(), dir);
            p.file_name().unwrap().to_string_lossy().into_owned()
        })
        .collect();
    assert_eq!(
        stored,
        [
            "evil.txt",
            "evil-2.txt",
            "hidden",
            "abc.png",
            "upload",
            "upload-2",
            "report.pdf"
        ]
    );
    let long = format!("{}.tar.gz", "n".repeat(400));
    let (batch, _) = peer.begin(&[(&long, 1), (&long, 1)], None).unwrap();
    peer.upload(batch, &[b"a", b"b"], 2);
    let paths = peer.finish(batch).unwrap();
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.len() <= uploads::MAX_NAME_BYTES, "{name}");
        assert!(name.ends_with(".gz"));
    }
    assert_ne!(paths[0], paths[1]);
}

#[test]
fn declared_sizes_are_enforced() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    // Overflow: more bytes than declared fails the batch and removes it.
    let (batch, dir) = peer.begin(&[("a.bin", 10)], None).unwrap();
    peer.chunk(batch, 0, 0, &[1; 11]);
    match peer.next() {
        Event::UploadFailed {
            batch: failed,
            message,
        } => {
            assert_eq!(failed, batch);
            assert!(message.contains("declared size"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert!(!dir.exists());
    // Chunks still in flight for the failed batch are dropped silently.
    peer.chunk(batch, 0, 0, &[1; 5]);
    peer.barrier();
    // Underflow: finishing an incomplete batch fails and removes it.
    let (batch, dir) = peer.begin(&[("b.bin", 10)], None).unwrap();
    peer.chunk(batch, 0, 0, &[2; 5]);
    peer.acknowledged(batch, (0, 5));
    let error = peer.finish(batch).unwrap_err();
    assert!(error.contains("incomplete"), "{error}");
    assert!(!dir.exists());
    // Offsets are strictly sequential.
    let (batch, dir) = peer.begin(&[("c.bin", 10)], None).unwrap();
    peer.chunk(batch, 0, 3, &[3; 2]);
    assert!(matches!(peer.next(), Event::UploadFailed { .. }));
    assert!(!dir.exists());
    // Files are sent in order.
    let (batch, _) = peer.begin(&[("d.bin", 2), ("e.bin", 2)], None).unwrap();
    peer.chunk(batch, 1, 0, &[4; 2]);
    assert!(matches!(peer.next(), Event::UploadFailed { .. }));
    // Invalid base64 fails the batch.
    let (batch, _) = peer.begin(&[("f.bin", 2)], None).unwrap();
    peer.send(Request::UploadChunk {
        batch,
        file_index: 0,
        offset: 0,
        data: "not base64!".into(),
    });
    assert!(matches!(peer.next(), Event::UploadFailed { .. }));
    // Declarations beyond the limits are refused up front.
    assert!(
        peer.begin(&[("big", uploads::MAX_FILE_BYTES + 1)], None)
            .is_err()
    );
    assert!(peer.begin(&[], None).is_err());
    let many: Vec<(&str, u64)> = (0..=uploads::MAX_FILES).map(|_| ("x", 1)).collect();
    assert!(peer.begin(&many, None).is_err());
    let batch_limit: Vec<(&str, u64)> = (0..11).map(|_| ("x", uploads::MAX_FILE_BYTES)).collect();
    assert!(peer.begin(&batch_limit, None).is_err());
    // An unknown batch fails without affecting anything else.
    peer.chunk(9999, 0, 0, b"x");
    assert!(matches!(
        peer.next(),
        Event::UploadFailed { batch: 9999, .. }
    ));
    assert_eq!(fs::read_dir(daemon.uploads()).unwrap().count(), 0);
    assert!(daemon.registry().is_empty());
}

#[test]
fn cancel_removes_the_batch_and_batches_belong_to_their_connection() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let (batch, dir) = peer
        .begin(&[("a.bin", uploads::MAX_CHUNK_BYTES as u64 * 2)], None)
        .unwrap();
    peer.chunk(batch, 0, 0, &pattern(uploads::MAX_CHUNK_BYTES, 1));
    peer.acknowledged(batch, (0, uploads::MAX_CHUNK_BYTES as u64));
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    // Another connection can neither write, finish nor cancel it.
    let mut other = daemon.peer();
    other.chunk(batch, 0, uploads::MAX_CHUNK_BYTES as u64, b"x");
    assert!(matches!(other.next(), Event::UploadFailed { .. }));
    assert!(other.finish(batch).is_err());
    assert!(other.cancel(batch).is_err());
    assert!(dir.exists());
    peer.cancel(batch).unwrap();
    assert!(!dir.exists());
    assert!(daemon.registry().is_empty());
    // A cancelled batch's in-flight chunk is dropped; cancelling again succeeds.
    peer.chunk(batch, 0, uploads::MAX_CHUNK_BYTES as u64, b"x");
    peer.barrier();
    peer.cancel(batch).unwrap();
    // At most four unfinished batches per connection.
    for _ in 0..uploads::MAX_BATCHES {
        peer.begin(&[("x", 1)], None).unwrap();
    }
    assert!(
        peer.begin(&[("x", 1)], None)
            .unwrap_err()
            .contains("per connection")
    );
    assert!(other.begin(&[("x", 1)], None).is_ok());
}

#[test]
fn a_disconnect_removes_unfinished_batches() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let (batch, dir) = peer.begin(&[("a.bin", 100)], None).unwrap();
    peer.chunk(batch, 0, 0, &[1; 50]);
    peer.acknowledged(batch, (0, 50));
    let (done, done_dir) = peer.begin(&[("b.bin", 1)], None).unwrap();
    peer.upload(done, &[b"b"], 1);
    peer.finish(done).unwrap();
    assert!(dir.join(".kelpserver-upload-0.part").exists());
    drop(peer);
    let deadline = Instant::now() + Duration::from_secs(5);
    while dir.exists() {
        assert!(Instant::now() < deadline, "partial batch was not removed");
        thread::sleep(Duration::from_millis(20));
    }
    // The completed batch stays.
    assert!(done_dir.join("b.bin").exists());
    let registry = daemon.registry();
    assert_eq!(registry.len(), 1);
    assert_eq!(registry[0].path, done_dir);
}

#[test]
fn destinations_must_be_existing_owned_absolute_directories() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let base = daemon.root.join("dest");
    fs::create_dir(&base).unwrap();
    let (batch, dir) = peer.begin(&[("a.txt", 1)], Some(&base)).unwrap();
    assert_eq!(dir.parent().unwrap(), base);
    assert_eq!(mode(&dir), 0o700);
    peer.upload(batch, &[b"a"], 1);
    assert_eq!(peer.finish(batch).unwrap(), vec![dir.join("a.txt")]);
    // Relative, missing, non-normalized, a file, a symlink.
    assert!(peer.begin(&[("a", 1)], Some(Path::new("dest"))).is_err());
    assert!(
        peer.begin(&[("a", 1)], Some(&daemon.root.join("missing")))
            .is_err()
    );
    assert!(
        peer.begin(&[("a", 1)], Some(&base.join("..").join("dest")))
            .is_err()
    );
    let file = daemon.root.join("file");
    fs::write(&file, b"x").unwrap();
    assert!(peer.begin(&[("a", 1)], Some(&file)).is_err());
    let link = daemon.root.join("link");
    std::os::unix::fs::symlink(&base, &link).unwrap();
    let error = peer.begin(&[("a", 1)], Some(&link)).unwrap_err();
    assert!(error.contains("symlink"), "{error}");
    // Not owned by the server's user.
    let foreign = if unsafe { libc::geteuid() } == 0 {
        let foreign = daemon.root.join("foreign");
        fs::create_dir(&foreign).unwrap();
        std::os::unix::fs::chown(&foreign, Some(65534), Some(65534))
            .ok()
            .map(|_| foreign)
    } else {
        Some(PathBuf::from("/"))
    };
    if let Some(foreign) = foreign {
        assert!(peer.begin(&[("a", 1)], Some(&foreign)).is_err());
    }
    // Nothing was created by the refused requests.
    assert_eq!(fs::read_dir(&base).unwrap().count(), 1);
}

#[test]
fn destinations_must_lie_inside_the_file_root() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    // A folder owned by this user outside the root (a sibling of it).
    let outside = daemon.root.with_extension("outside");
    let _ = fs::remove_dir_all(&outside);
    fs::create_dir(&outside).unwrap();
    let error = peer.begin(&[("a", 1)], Some(&outside)).unwrap_err();
    assert!(error.contains("outside the file root"), "{error}");
    // Through a symlink in an intermediate component that leads outside.
    let link = daemon.root.join("out");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    fs::create_dir(outside.join("sub")).unwrap();
    assert!(peer.begin(&[("a", 1)], Some(&link.join("sub"))).is_err());
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    // A folder inside, reached through a symlink that stays inside.
    let inside = daemon.root.join("inside");
    fs::create_dir(&inside).unwrap();
    std::os::unix::fs::symlink("inside", daemon.root.join("in")).unwrap();
    fs::create_dir(inside.join("sub")).unwrap();
    let (batch, dir) = peer
        .begin(&[("a.txt", 1)], Some(&daemon.root.join("in/sub")))
        .unwrap();
    assert_eq!(dir.parent().unwrap(), daemon.root.join("in/sub"));
    peer.upload(batch, &[b"a"], 1);
    peer.finish(batch).unwrap();
    assert_eq!(fs::read_dir(inside.join("sub")).unwrap().count(), 1);
    fs::remove_dir_all(&outside).unwrap();
}

#[test]
fn destinations_may_lie_inside_an_extra_root() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let extra = daemon.extra();
    fs::create_dir(extra.join("drop")).unwrap();
    let (batch, dir) = peer
        .begin(&[("a.txt", 1)], Some(&extra.join("drop")))
        .unwrap();
    assert_eq!(dir.parent().unwrap(), extra.join("drop"));
    peer.upload(batch, &[b"a"], 1);
    peer.finish(batch).unwrap();
    assert_eq!(fs::read_dir(extra.join("drop")).unwrap().count(), 1);
    // A symlink in the extra root never leads into the primary root.
    fs::create_dir(daemon.root.join("target")).unwrap();
    std::os::unix::fs::symlink(daemon.root.join("target"), extra.join("to-root")).unwrap();
    let name = daemon.root.file_name().unwrap().to_str().unwrap();
    std::os::unix::fs::symlink(format!("../{name}"), extra.join("up")).unwrap();
    fs::create_dir(daemon.root.join("target/sub")).unwrap();
    for dest in [extra.join("to-root/sub"), extra.join("up/target/sub")] {
        assert!(peer.begin(&[("a", 1)], Some(&dest)).is_err(), "{dest:?}");
    }
    assert_eq!(
        fs::read_dir(daemon.root.join("target/sub"))
            .unwrap()
            .count(),
        0
    );
}

/// A registered batch directory, created the way the server creates them.
fn batch_dir(parent: &Path, name: &str) -> (PathBuf, u64, u64) {
    let dir = parent.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("file.txt"), b"data").unwrap();
    let meta = fs::metadata(&dir).unwrap();
    (dir, meta.dev(), meta.ino())
}
fn write_registry(state: &Path, entries: &[Registered]) {
    let path = state.join("uploads.json");
    fs::write(&path, serde_json::to_vec(entries).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}
fn entry(path: &Path, dev: u64, ino: u64, completed_days_ago: Option<u64>) -> Registered {
    let now = kelpserver::now();
    Registered {
        path: path.to_path_buf(),
        dev,
        ino,
        created_at: now - 30 * 86_400,
        completed_at: completed_days_ago.map(|days| now - days * 86_400),
    }
}

#[test]
fn retention_removes_only_registered_expired_batches() {
    let paths = std::sync::Mutex::new(Vec::new());
    let daemon = Daemon::start_with(Some(7), |state, root| {
        let (expired, dev, ino) = batch_dir(root, "expired");
        // A symlink inside an expired batch is removed, not followed.
        let outside = root.parent().unwrap().join("outside.txt");
        fs::write(&outside, b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, expired.join("link")).unwrap();
        fs::create_dir(expired.join("nested")).unwrap();
        fs::write(expired.join("nested/inner"), b"x").unwrap();
        let (fresh, fdev, fino) = batch_dir(root, "fresh");
        let (unfinished, udev, uino) = batch_dir(root, "unfinished");
        let (unregistered, _, _) = batch_dir(root, "unregistered");
        // Registered path replaced by another directory: its inode differs.
        let (replaced, rdev, rino) = batch_dir(root, "replaced");
        let rino = rino + 1_000_000;
        // Registered path that is now a symlink to a directory elsewhere.
        let (target, tdev, tino) = batch_dir(&root.parent().unwrap().join("elsewhere"), "t");
        let symlinked = root.join("symlinked");
        std::os::unix::fs::symlink(&target, &symlinked).unwrap();
        write_registry(
            state,
            &[
                entry(&expired, dev, ino, Some(8)),
                entry(&fresh, fdev, fino, Some(1)),
                entry(&unfinished, udev, uino, None),
                entry(&replaced, rdev, rino, Some(30)),
                entry(&symlinked, tdev, tino, Some(30)),
            ],
        );
        paths.lock().unwrap().extend([
            expired,
            fresh,
            unfinished,
            unregistered,
            replaced,
            target,
            symlinked,
            outside,
        ]);
    });
    // The startup sweep runs before the first connection is served.
    let _peer = daemon.peer();
    let [
        expired,
        fresh,
        unfinished,
        unregistered,
        replaced,
        target,
        symlinked,
        outside,
    ] = <[PathBuf; 8]>::try_from(paths.into_inner().unwrap()).unwrap();
    assert!(!expired.exists());
    assert!(!unfinished.exists());
    assert!(fresh.join("file.txt").exists());
    assert!(unregistered.join("file.txt").exists());
    assert!(replaced.join("file.txt").exists());
    assert!(target.join("file.txt").exists());
    assert!(
        fs::symlink_metadata(&symlinked)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(&outside).unwrap(), b"keep");
    // Only the kept batch stays registered.
    let registry = daemon.registry();
    assert_eq!(registry.len(), 1);
    assert_eq!(registry[0].path, fresh);
}

#[test]
fn zero_retention_keeps_completed_batches() {
    let paths = std::sync::Mutex::new(Vec::new());
    let daemon = Daemon::start_with(Some(0), |state, root| {
        let (old, dev, ino) = batch_dir(root, "old");
        let (unfinished, udev, uino) = batch_dir(root, "unfinished");
        write_registry(
            state,
            &[
                entry(&old, dev, ino, Some(3000)),
                entry(&unfinished, udev, uino, None),
            ],
        );
        paths.lock().unwrap().extend([old, unfinished]);
    });
    let _peer = daemon.peer();
    let paths = paths.into_inner().unwrap();
    assert!(paths[0].join("file.txt").exists());
    // Unfinished batches of a previous run are always removed.
    assert!(!paths[1].exists());
    assert_eq!(daemon.registry().len(), 1);
}

#[test]
fn flow_control_acknowledges_every_chunk_in_order() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let data = pattern(uploads::MAX_CHUNK_BYTES * 20 + 1, 3);
    let (batch, _) = peer
        .begin(&[("data.bin", data.len() as u64)], None)
        .unwrap();
    // A full window is sent before any acknowledgement is read; every chunk
    // gets exactly one upload_progress, in order.
    peer.upload(batch, &[&data], uploads::MAX_CHUNKS_IN_FLIGHT);
    let paths = peer.finish(batch).unwrap();
    assert_eq!(fs::read(&paths[0]).unwrap(), data);
    // Pane traffic keeps flowing between chunks: the connection still gets
    // replies to other requests in order.
    peer.barrier();
}

/// Measures local throughput through the socket (not run by default):
/// `cargo test --release --test native_uploads -- --ignored --nocapture`.
#[test]
#[ignore]
fn upload_throughput() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let data = pattern(uploads::MAX_FILE_BYTES as usize, 9);
    for window in [1, uploads::MAX_CHUNKS_IN_FLIGHT] {
        let (batch, _) = peer.begin(&[("big.bin", data.len() as u64)], None).unwrap();
        let started = Instant::now();
        peer.upload(batch, &[&data], window);
        let paths = peer.finish(batch).unwrap();
        let seconds = started.elapsed().as_secs_f64();
        println!(
            "window {window}: {} MiB in {seconds:.2} s = {:.1} MB/s",
            data.len() >> 20,
            data.len() as f64 / seconds / 1e6
        );
        fs::remove_file(&paths[0]).unwrap();
    }
}
