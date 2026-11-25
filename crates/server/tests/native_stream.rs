use kelpclient_protocol::screens;
use kelpserver::native::{
    protocol::*,
    server::{ServerConfig, serve},
    stream::{self, Event, PaneSnapshot, Request},
};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::net::UnixStream,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
struct Daemon {
    root: PathBuf,
    thread: Option<thread::JoinHandle<kelpserver::Result<()>>>,
    /// Dropped last, after the server stopped.
    _watchdog: Watchdog,
}
impl Daemon {
    /// A server with two shell panes (1 and 2), created by a stream client.
    fn start() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "kelpmux-stream-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let config = ServerConfig {
            socket: root.join("control.sock"),
            state_dir: root.join("state"),
            shell: "/bin/sh".into(),
            cwd: root.clone(),
            cols: 100,
            rows: 30,
            uploads: Default::default(),
            files: Default::default(),
        };
        let handle = thread::spawn(move || serve(config));
        let daemon = Self {
            root,
            thread: Some(handle),
            _watchdog: Watchdog::start(),
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(daemon.root.join("control.sock")).is_err() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        let mut seeder = daemon.peer();
        for (request, expected) in [("seed-1", 1), ("seed-2", 2)] {
            seeder.send(Request::Spawn {
                request: request.into(),
                cwd: None,
                argv: vec![],
                shell: None,
            });
            loop {
                if let Event::Spawned { pane, .. } = seeder.next() {
                    assert_eq!(pane, expected);
                    break;
                }
            }
        }
        daemon
    }
    fn socket(&self) -> UnixStream {
        let s = UnixStream::connect(self.root.join("control.sock")).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        s
    }
    fn peer(&self) -> Peer {
        let mut p = Peer {
            socket: self.socket(),
            id: 0,
            epoch: String::new(),
            screens: BTreeMap::new(),
        };
        p.send(Request::Hello {
            version: stream::VERSION,
        });
        loop {
            if let Event::Welcome { client, epoch, .. } = p.next() {
                p.id = client;
                p.epoch = epoch;
                break;
            }
        }
        p
    }
    /// The pane's current screen, observed by a separate stream client.
    fn inspect(&self, pane: u64) -> Screen {
        let mut observer = self.peer();
        observer.send(Request::Subscribe { panes: vec![pane] });
        observer.until(pane, |_| true);
        observer.screens[&pane].screen.clone()
    }
}
impl Drop for Daemon {
    /// Ask the server to stop and wait for it, bounded: a lost request fails
    /// the test with a reason instead of hanging it (kelpcore#4).
    fn drop(&mut self) {
        if let Ok(mut s) = UnixStream::connect(self.root.join("control.sock")) {
            for request in [
                Request::Hello {
                    version: stream::VERSION,
                },
                Request::Shutdown {
                    request: "drop".into(),
                },
            ] {
                let _ = write_message(&mut s, &ClientMessage::Stream(request));
            }
        }
        if let Some(t) = self.thread.take() {
            let deadline = Instant::now() + Duration::from_secs(30);
            while !t.is_finished() {
                if Instant::now() >= deadline {
                    if !thread::panicking() {
                        panic!("server did not stop within 30 s of the shutdown request");
                    }
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
            let result = t.join();
            if !thread::panicking() {
                assert!(result.unwrap().is_ok());
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
/// Ends the whole test binary, naming the test, when a daemon outlives
/// `LIMIT`: a wait without its own deadline fails in minutes instead of
/// holding CI until the job timeout (kelpcore#4).
struct Watchdog {
    _alive: std::sync::mpsc::Sender<()>,
}
impl Watchdog {
    const LIMIT: Duration = Duration::from_secs(120);
    fn start() -> Self {
        let test = thread::current().name().unwrap_or("a test").to_owned();
        let (sender, receiver) = std::sync::mpsc::channel::<()>();
        thread::spawn(move || {
            // Disconnected: the daemon was dropped in time.
            if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                receiver.recv_timeout(Self::LIMIT)
            {
                use std::io::Write;
                // Straight to stderr: libtest captures `eprintln!`.
                let _ = writeln!(
                    std::io::stderr(),
                    "{test} still has its server after {:?}: a wait is stuck; exiting",
                    Self::LIMIT
                );
                std::process::exit(101);
            }
        });
        Self { _alive: sender }
    }
}
struct Peer {
    socket: UnixStream,
    id: u64,
    epoch: String,
    screens: BTreeMap<u64, PaneSnapshot>,
}
impl Peer {
    fn send(&mut self, r: Request) {
        write_message(&mut self.socket, &ClientMessage::Stream(r)).unwrap();
    }
    fn next(&mut self) -> Event {
        let event = match read_message(&mut self.socket).unwrap().unwrap() {
            ServerMessage::Stream(e) => e,
            other => panic!("non-stream message {other:?}"),
        };
        if let Event::Update(u) = &event {
            assert_eq!(u.epoch, self.epoch);
            assert!(u.panes.len() <= 10);
            assert!(u.notifications.len() <= 128);
            for snapshot in &u.snapshots {
                self.screens.insert(snapshot.pane, snapshot.clone());
            }
            for patch in &u.patches {
                let next = stream::apply(&self.screens[&patch.pane], patch).unwrap();
                self.screens.insert(patch.pane, next);
            }
        }
        event
    }
    fn lease(&mut self, pane: u64, takeover: bool) {
        self.send(Request::Claim { pane, takeover });
        loop {
            match self.next() {
                Event::Lease {
                    pane: p,
                    owner: Some(owner),
                } if p == pane => {
                    assert_eq!(owner, self.id);
                    break;
                }
                Event::Error { message, .. } => panic!("{message}"),
                _ => {}
            }
        }
    }
    fn error(&mut self, r: Request) -> String {
        self.send(r);
        loop {
            if let Event::Error { message, .. } = self.next() {
                return message;
            }
        }
    }
    fn until(&mut self, pane: u64, predicate: impl Fn(&Screen) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if self
                .screens
                .get(&pane)
                .is_some_and(|s| predicate(&s.screen))
            {
                return;
            }
            assert!(Instant::now() < deadline);
            self.next();
        }
    }
}
fn text(s: &Screen) -> String {
    s.cells.iter().map(|c| c.text.as_str()).collect()
}
#[test]
fn legacy_mouse_events_reach_ncurses_without_utf8_reencoding() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    peer.send(Request::Subscribe { panes: vec![1] });
    peer.lease(1, false);
    peer.send(Request::Input { pane: 1, bytes: b"stty -icanon -echo; printf '\\033[?1000hMOUSE_%s' READY; dd bs=1 count=12 2>/dev/null | od -An -tu1; printf '\\033[?1000lMOUSE_%s' DONE; stty sane\n".to_vec() });
    peer.until(1, |s| {
        s.modes.mouse_click && text(s).contains("MOUSE_READY")
    });
    for kind in [MouseKind::Press, MouseKind::Release] {
        peer.send(Request::Mouse {
            pane: 1,
            event: MouseEvent {
                position: Point { x: 9, y: 4 },
                kind,
                button: 0,
                shift: false,
                alt: false,
                ctrl: false,
            },
        });
    }
    peer.until(1, |s| text(s).contains("MOUSE_DONE"));
    let output = text(&daemon.inspect(1));
    let words: Vec<_> = output.split_whitespace().collect();
    assert!(
        words.windows(12).any(|words| words
            == [
                "27", "91", "77", "32", "42", "37", "27", "91", "77", "35", "42", "37"
            ]),
        "{output}"
    );
}
#[test]
fn independent_pane_leases_resize_input_observers_takeover_and_disconnect() {
    let d = Daemon::start();
    let mut a = d.peer();
    let mut b = d.peer();
    let mut observer = d.peer();
    assert_eq!(a.epoch, b.epoch);
    a.send(Request::Subscribe { panes: vec![1, 2] });
    b.send(Request::Subscribe { panes: vec![1, 2] });
    observer.send(Request::Subscribe { panes: vec![1, 2] });
    let original = d.inspect(1);
    a.lease(1, false);
    b.lease(2, false);
    a.send(Request::Resize {
        pane: 1,
        cols: 160,
        rows: 50,
    });
    b.send(Request::Resize {
        pane: 2,
        cols: 40,
        rows: 12,
    });
    a.until(1, |s| s.cols == 160 && s.rows == 50);
    b.until(2, |s| s.cols == 40 && s.rows == 12);
    assert!(
        observer
            .error(Request::Input {
                pane: 1,
                bytes: b"echo BAD\n".to_vec()
            })
            .contains("lease")
    );
    assert!(
        observer
            .error(Request::Resize {
                pane: 2,
                cols: 90,
                rows: 30
            })
            .contains("lease")
    );
    assert!(
        b.error(Request::Claim {
            pane: 1,
            takeover: false
        })
        .contains("already leased")
    );
    a.send(Request::Input {
        pane: 1,
        bytes: b"printf '\\033[2J\\033[HMAC_ONE\\n'\n".to_vec(),
    });
    b.send(Request::Paste {
        pane: 2,
        bytes: b"printf '\\033[2J\\033[HIPHONE_TWO\\n'\n".to_vec(),
    });
    a.until(1, |s| text(s).contains("MAC_ONE"));
    b.until(2, |s| text(s).contains("IPHONE_TWO"));
    assert_eq!(d.inspect(1).cols, 160);
    assert_eq!(d.inspect(2).cols, 40);
    observer.lease(1, true);
    assert!(
        a.error(Request::Input {
            pane: 1,
            bytes: vec![3]
        })
        .contains("lease")
    );
    observer.send(Request::Release { pane: 1 });
    loop {
        if let Event::Lease {
            pane: 1,
            owner: None,
        } = observer.next()
        {
            break;
        }
    }
    assert_eq!(d.inspect(1).cols, original.cols);
    drop(b);
    let deadline = Instant::now() + Duration::from_secs(5);
    while d.inspect(2).cols == 40 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn controller_theme_is_owned_per_pane_and_replaced_on_takeover() {
    let daemon = Daemon::start();
    let mut a = daemon.peer();
    let mut b = daemon.peer();
    a.send(Request::Subscribe { panes: vec![1] });
    b.send(Request::Subscribe { panes: vec![1] });
    assert!(
        b.error(Request::Theme {
            pane: 1,
            foreground: [10, 20, 30],
            background: [40, 50, 60]
        })
        .contains("lease")
    );
    a.lease(1, false);
    a.send(Request::Theme {
        pane: 1,
        foreground: [11, 22, 33],
        background: [44, 55, 66],
    });
    a.send(Request::Input {
        pane: 1,
        bytes: b"printf '\\033[7mTHEME_PROBE\\033[0m\\n'\n".to_vec(),
    });
    a.until(1, |screen| {
        screen
            .cells
            .iter()
            .any(|cell| cell.text == "T" && cell.bg == Color::Rgb(11, 22, 33))
    });
    b.lease(1, true);
    b.send(Request::Theme {
        pane: 1,
        foreground: [77, 88, 99],
        background: [1, 2, 3],
    });
    b.until(1, |screen| {
        screen
            .cells
            .iter()
            .any(|cell| cell.text == "T" && cell.bg == Color::Rgb(77, 88, 99))
    });
    assert!(
        a.error(Request::Theme {
            pane: 1,
            foreground: [10, 20, 30],
            background: [40, 50, 60]
        })
        .contains("lease")
    );
}
#[test]
fn handshake_bounds_leases_and_resync() {
    let d = Daemon::start();
    let mut raw = d.socket();
    write_message(
        &mut raw,
        &ClientMessage::Stream(Request::Claim {
            pane: 1,
            takeover: false,
        }),
    )
    .unwrap();
    assert!(matches!(
        read_message::<ServerMessage>(&mut raw).unwrap(),
        Some(ServerMessage::Stream(Event::Error { .. }))
    ));
    let mut a = d.peer();
    assert!(
        a.error(Request::Subscribe { panes: vec![1, 1] })
            .contains("invalid")
    );
    assert!(
        a.error(Request::Subscribe { panes: vec![99] })
            .contains("invalid")
    );
    a.send(Request::Subscribe { panes: vec![1] });
    a.lease(1, false);
    a.send(Request::Resize {
        pane: 1,
        cols: 140,
        rows: 40,
    });
    a.until(1, |s| s.cols == 140);
    // An observer cannot type into or resize a leased pane.
    let mut observer = d.peer();
    assert!(
        observer
            .error(Request::Input {
                pane: 1,
                bytes: b"x".to_vec(),
            })
            .contains("lease")
    );
    assert!(
        observer
            .error(Request::Resize {
                pane: 1,
                cols: 80,
                rows: 24,
            })
            .contains("lease")
    );
    assert_eq!(d.inspect(1).cols, 140);
    // The one-shot diagnostics request belongs to connections without Hello;
    // a stream peer gets an error and keeps its connection.
    write_message(&mut a.socket, &ClientMessage::Diagnostics).unwrap();
    loop {
        if let Some(ServerMessage::Error(e)) = read_message(&mut a.socket).unwrap() {
            assert!(e.contains("stream peers"), "{e}");
            break;
        }
    }
    a.send(Request::Resync { pane: 1 });
    loop {
        if let Event::Update(u) = a.next() {
            if u.snapshots.iter().any(|s| s.pane == 1) {
                break;
            }
        }
    }
    // A takeover moves the lease; the previous owner can no longer type.
    observer.lease(1, true);
    observer.send(Request::Resize {
        pane: 1,
        cols: 80,
        rows: 24,
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while d.inspect(1).cols == 140 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        a.error(Request::Input {
            pane: 1,
            bytes: vec![3]
        })
        .contains("lease")
    );
}
#[test]
fn checked_patch_rejects_stale_duplicate_and_invalid_cells() {
    let s = Screen {
        cols: 2,
        rows: 1,
        cells: vec![Cell::default(); 2],
        cursor: None,
        modes: InputModes::default(),
    };
    let old = PaneSnapshot {
        pane: 7,
        revision: 1,
        screen: s.clone(),
    };
    let mut next = s;
    next.cells[0].text = "x".into();
    let patch = stream::diff(&old, &next);
    assert!(patch.is_none(), "dense update becomes snapshot");
    let mut p = stream::PanePatch {
        pane: 7,
        base: 1,
        revision: 2,
        cursor: None,
        modes: InputModes::default(),
        changes: vec![(0, next.cells[0].clone())],
    };
    assert_eq!(stream::apply(&old, &p).unwrap().screen, next);
    p.base = 0;
    assert!(stream::apply(&old, &p).is_err());
    p.base = 1;
    p.changes.push(p.changes[0].clone());
    assert!(stream::apply(&old, &p).is_err());
    let unknown = r#"{"type":"input","pane":1,"bytes":[],"extra":true}"#;
    assert!(serde_json::from_str::<Request>(unknown).is_err());
}

#[test]
fn slow_reader_gets_coherent_resnapshots_and_does_not_block_other_clients() {
    let d = Daemon::start();
    let mut slow = d.peer();
    let mut other = d.peer();
    slow.send(Request::Subscribe { panes: vec![1] });
    slow.lease(1, false);
    slow.send(Request::Resize {
        pane: 1,
        cols: 160,
        rows: 50,
    });
    slow.until(1, |screen| screen.cols == 160);
    slow.send(Request::Input { pane: 1, bytes: b"i=0; while [ \"$i\" -lt 1200 ]; do printf '%080d\\n' \"$i\"; i=$((i+1)); done; printf 'STREAM_DONE\\n'\n".to_vec() });
    thread::sleep(Duration::from_millis(1200));
    other.lease(2, false);
    other.send(Request::Subscribe { panes: vec![2] });
    other.send(Request::Input {
        pane: 2,
        bytes: b"printf 'OTHER_RESPONSIVE\\n'\n".to_vec(),
    });
    other.until(2, |screen| text(screen).contains("OTHER_RESPONSIVE"));
    slow.until(1, |screen| text(screen).contains("STREAM_DONE"));
    // Every received patch was applied to an actual delivered baseline by Peer::next.
    slow.send(Request::Resync { pane: 1 });
    loop {
        if let Event::Update(update) = slow.next() {
            if update.snapshots.iter().any(|s| s.pane == 1) {
                break;
            }
        }
    }
}

#[test]
fn metadata_notifications_and_vitals_stream_without_composed_ui() {
    use kelpserver::{
        model::{Kind, Status},
        native::hooks,
    };
    let d = Daemon::start();
    fs::copy("/bin/bash", d.root.join("codex")).unwrap();
    let mut peer = d.peer();
    peer.send(Request::Subscribe { panes: Vec::new() });
    peer.lease(1, false);
    peer.send(Request::Input {
        pane: 1,
        bytes: format!(
            "{} -c 'while :; do sleep 0.1; done' _ resume 11111111-1111-4111-8111-111111111111\n",
            d.root.join("codex").display()
        )
        .into_bytes(),
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        assert!(Instant::now() < deadline);
        if let Event::Update(update) = peer.next() {
            assert!(update.snapshots.is_empty() && update.patches.is_empty());
            assert!(update.vitals.cpu.is_none_or(|cpu| cpu <= 100));
            if update
                .panes
                .iter()
                .any(|pane| pane.id == 1 && pane.kind == Some(Kind::Codex))
            {
                break;
            }
        }
    }
    for event in ["UserPromptSubmit", "PermissionRequest"] {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let result = hooks::receive(
                &d.root.join("state"),
                "%1",
                Kind::Codex,
                &serde_json::json!({"session_id":"11111111-1111-4111-8111-111111111111", "hook_event_name":event}),
            );
            if result.is_ok() {
                break;
            }
            assert!(Instant::now() < deadline, "{result:?}");
            thread::sleep(Duration::from_millis(20));
        }
        loop {
            if let Event::Update(update) = peer.next() {
                if update.panes.iter().any(|pane| {
                    pane.id == 1
                        && pane.status
                            == if event == "PermissionRequest" {
                                Status::InputNeeded
                            } else {
                                Status::Working
                            }
                }) {
                    break;
                }
            }
        }
    }
    let notification = loop {
        if let Event::Update(update) = peer.next() {
            if let Some(event) = update
                .notifications
                .iter()
                .find(|event| event.pane == 1 && event.status == Status::InputNeeded)
            {
                break event.id;
            }
        }
    };
    peer.send(Request::Acknowledge { notification });
    loop {
        if let Event::Update(update) = peer.next() {
            if update
                .notifications
                .iter()
                .any(|event| event.id == notification && event.read)
            {
                break;
            }
        }
    }
}

#[test]
fn snapshots_and_sparse_patches_are_bounded_and_epoch_changes_on_restart() {
    let old = PaneSnapshot {
        pane: 1,
        revision: 1,
        screen: Screen {
            cols: 40,
            rows: 10,
            cells: vec![Cell::default(); 400],
            cursor: None,
            modes: InputModes::default(),
        },
    };
    stream::validate(&old).unwrap();
    let mut changed = old.screen.clone();
    changed.cells[398].text = "界".into();
    changed.cells[398].width = 2;
    changed.cells[399].text.clear();
    changed.cells[399].width = 0;
    let patch = stream::diff(&old, &changed).unwrap();
    assert_eq!(patch.base, 1);
    assert_eq!(stream::apply(&old, &patch).unwrap().screen, changed);
    let mut orphan = old.clone();
    orphan.screen.cells[399].text = "界".into();
    orphan.screen.cells[399].width = 2;
    assert!(stream::validate(&orphan).is_err());
    let mut malformed = old.clone();
    malformed.screen.cells.clear();
    assert!(stream::validate(&malformed).is_err());
    malformed = old.clone();
    malformed.screen.cells[0].text = "\x1b[31m".into();
    assert!(stream::validate(&malformed).is_err());
    let d = Daemon::start();
    let first = d.peer().epoch;
    drop(d);
    let d = Daemon::start();
    assert_ne!(first, d.peer().epoch);
    let mut peer = d.socket();
    write_message(
        &mut peer,
        &ClientMessage::Stream(Request::Hello { version: 999 }),
    )
    .unwrap();
    assert!(matches!(
        read_message::<ServerMessage>(&mut peer).unwrap(),
        Some(ServerMessage::Stream(Event::Error { .. }))
    ));
}

#[test]
fn small_panes_and_large_request_batches_preserve_leases_and_processes() {
    use std::io::Write;
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    peer.lease(1, false);
    peer.send(Request::Resize {
        pane: 1,
        cols: 12,
        rows: 3,
    });
    peer.send(Request::Subscribe { panes: vec![1] });
    peer.until(1, |screen| screen.cols == 12 && screen.rows == 3);
    assert_eq!(daemon.inspect(1).cols, 12);
    let mut batch = Vec::new();
    for _ in 0..99 {
        write_message(
            &mut batch,
            &ClientMessage::Stream(Request::Subscribe { panes: vec![] }),
        )
        .unwrap();
    }
    write_message(
        &mut batch,
        &ClientMessage::Stream(Request::Subscribe { panes: vec![2] }),
    )
    .unwrap();
    peer.socket.write_all(&batch).unwrap();
    peer.until(2, |_| true);
    assert_eq!(daemon.inspect(1).cols, 12);
}

#[test]
fn clipboard_writes_require_opt_in_and_only_reach_the_pane_owner() {
    let daemon = Daemon::start();
    let mut owner = daemon.peer();
    let mut observer = daemon.peer();
    owner.send(Request::Subscribe { panes: vec![1] });
    observer.send(Request::Subscribe { panes: vec![1, 2] });
    owner.lease(1, false);
    observer.lease(2, false);
    observer.send(Request::Clipboard { enabled: true });
    owner.send(Request::Input {
        pane: 1,
        bytes: b"printf '\\033]52;c;aGVsbG8=\\007'; printf 'DEFAULT_%s\\n' DONE\n".to_vec(),
    });
    loop {
        assert!(!matches!(owner.next(), Event::Clipboard { .. }));
        if owner
            .screens
            .get(&1)
            .is_some_and(|s| text(&s.screen).contains("DEFAULT_DONE"))
        {
            break;
        }
    }
    owner.send(Request::Clipboard { enabled: true });
    owner.send(Request::Input {
        pane: 1,
        bytes: b"printf '\\033]52;c;aGVsbG8=\\007'\n".to_vec(),
    });
    loop {
        if let Event::Clipboard { pane, text } = owner.next() {
            assert_eq!(pane, 1);
            assert_eq!(text, "hello");
            break;
        }
    }
    observer.send(Request::Input {
        pane: 2,
        bytes: b"printf 'OBSERVER_%s\\n' BARRIER\n".to_vec(),
    });
    loop {
        assert!(!matches!(observer.next(), Event::Clipboard { .. }));
        if observer
            .screens
            .get(&2)
            .is_some_and(|s| text(&s.screen).contains("OBSERVER_BARRIER"))
        {
            break;
        }
    }
    owner.send(Request::Clipboard { enabled: false });
    owner.send(Request::Input {
        pane: 1,
        bytes: b"printf '\\033]52;c;aGVsbG8=\\007'; printf 'DISABLED_%s\\n' DONE\n".to_vec(),
    });
    loop {
        assert!(!matches!(owner.next(), Event::Clipboard { .. }));
        if owner
            .screens
            .get(&1)
            .is_some_and(|s| text(&s.screen).contains("DISABLED_DONE"))
        {
            break;
        }
    }
}

#[test]
fn structured_creation_sets_cwd_quotes_arguments_and_retries_without_stealing_a_lease() {
    let daemon = Daemon::start();
    let cwd = daemon.root.join("working folder ' ; literal");
    fs::create_dir(&cwd).unwrap();
    let sentinel = daemon.root.join("must-not-exist");
    let argument = format!("literal '; touch {}; #", sentinel.display());
    let request = Request::Spawn {
        request: "creation-test".into(),
        cwd: Some(cwd.clone()),
        argv: vec![
            "/usr/bin/printf".into(),
            "CREATED_%s\\n".into(),
            argument.clone(),
        ],
        shell: None,
    };
    let mut owner = daemon.peer();
    owner.send(Request::Subscribe { panes: vec![1] });
    owner.lease(1, false);
    owner.send(Request::Resize {
        pane: 1,
        cols: 37,
        rows: 8,
    });
    owner.until(1, |screen| screen.cols == 37 && screen.rows == 8);
    // Each creation runs on its own connection, like separate clients.
    let create = |request: Request| -> Result<u64, String> {
        let mut peer = daemon.peer();
        peer.send(request);
        loop {
            match peer.next() {
                Event::Spawned { request, pane } => {
                    assert_eq!(request, "creation-test");
                    return Ok(pane);
                }
                Event::Error { message, .. } => return Err(message),
                _ => {}
            }
        }
    };
    let created = create(request.clone()).unwrap();
    assert_eq!(created, 3);
    assert_eq!(create(request).unwrap(), created);
    let mut view = daemon.peer();
    view.send(Request::Subscribe {
        panes: vec![created],
    });
    let update = loop {
        if let Event::Update(u) = view.next() {
            break u;
        }
    };
    assert_eq!(update.panes.len(), 3);
    let info = update.panes.iter().find(|p| p.id == created).unwrap();
    assert_eq!(
        fs::read_link(format!("/proc/{}/cwd", info.pid)).unwrap(),
        cwd
    );
    assert_eq!(daemon.inspect(1).cols, 37);
    assert_eq!(daemon.inspect(1).rows, 8);
    view.until(created, |screen| text(screen).contains("CREATED_literal"));
    assert!(!sentinel.exists());
    // The same identifier with other parameters, and a relative directory,
    // are rejected.
    assert!(
        create(Request::Spawn {
            request: "creation-test".into(),
            cwd: Some(cwd.clone()),
            argv: vec![],
            shell: None,
        })
        .is_err()
    );
    assert!(
        create(Request::Spawn {
            request: "bad-directory".into(),
            cwd: Some(PathBuf::from("relative")),
            argv: vec![],
            shell: None,
        })
        .is_err()
    );
}

#[test]
fn shell_commands_report_their_start_return_and_alternate_screen() {
    let d = Daemon::start();
    let pane = || {
        let mut observer = d.peer();
        observer.send(Request::Subscribe { panes: vec![] });
        loop {
            if let Event::Update(u) = observer.next() {
                break u.panes.into_iter().find(|p| p.id == 1).unwrap();
            }
        }
    };
    // A fresh peer per command keeps each lease owner's unread event stream
    // short. `after` is the earliest acceptable start in whole seconds;
    // process start times are derived from clock ticks, so a command begun
    // just after a second boundary may be reported in the previous second.
    let run = |line: &[u8], after: u64| {
        let mut peer = d.peer();
        peer.lease(1, false);
        peer.send(Request::Input {
            pane: 1,
            bytes: line.to_vec(),
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let info = pane();
            if let Some(command) = info
                .command
                .clone()
                .filter(|c| c.started_at + 1 >= after && c.ended_at.is_some())
            {
                assert_eq!(info.kind, None);
                return command;
            }
            assert!(
                Instant::now() < deadline,
                "{info:?}\n{:?}",
                text(&d.inspect(1))
            );
            thread::sleep(Duration::from_millis(100));
        }
    };
    let start = kelpserver::now();
    let plain = run(b"sleep 3\n", start);
    let ended = plain.ended_at.unwrap();
    assert!((2..=5).contains(&(ended - plain.started_at)), "{plain:?}");
    assert!(!plain.alternate_screen);
    // A full-screen program that leaves the alternate screen before the
    // next status poll is still reported.
    let full = run(b"printf '\\033[?1049hFULL\\033[?1049l'; sleep 3\n", ended);
    assert!(full.alternate_screen, "{full:?}");
    assert!(full.started_at >= ended);
}

/// A stream client reading records on a thread, optionally with
/// `binary-screens`. The channel holds one record, so a test that stops
/// pumping also stops the client from reading its socket.
struct Observer {
    records: std::sync::mpsc::Receiver<ServerRecord>,
    binary: bool,
    screens: BTreeMap<u64, PaneSnapshot>,
    /// An update announced a binary record that has not arrived yet.
    pending: bool,
    updates: usize,
    snapshots: usize,
    patches: usize,
    socket: UnixStream,
}
impl Observer {
    fn connect(daemon: &Daemon, binary: bool, panes: Vec<u64>) -> Self {
        let mut socket = daemon.socket();
        let mut reader = socket.try_clone().unwrap();
        let keep = socket.try_clone().unwrap();
        reader.set_read_timeout(None).unwrap();
        let (sender, records) = std::sync::mpsc::sync_channel(1);
        thread::spawn(move || {
            while let Ok(Some(record)) = read_server_record(&mut reader) {
                if sender.send(record).is_err() {
                    break;
                }
            }
        });
        let mut send = |request| {
            write_message(&mut socket, &ClientMessage::Stream(request)).unwrap();
        };
        send(Request::Hello {
            version: stream::VERSION,
        });
        let observer = Self {
            records,
            binary,
            screens: BTreeMap::new(),
            pending: false,
            updates: 0,
            snapshots: 0,
            patches: 0,
            socket: keep,
        };
        let ServerRecord::Message(ServerMessage::Stream(Event::Welcome { capabilities, .. })) =
            observer
                .records
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
        else {
            panic!("expected welcome");
        };
        assert!(capabilities.iter().any(|c| c == screens::CAPABILITY));
        if binary {
            send(Request::BinaryScreens { enabled: true });
        }
        send(Request::Subscribe { panes });
        observer
    }
    /// Process one record if one arrives within `timeout`.
    fn pump(&mut self, timeout: Duration) -> bool {
        let Ok(record) = self.records.recv_timeout(timeout) else {
            return false;
        };
        if self.pending {
            assert!(
                matches!(record, ServerRecord::Screens(_)),
                "a binary update must be followed by its screens"
            );
        }
        match record {
            ServerRecord::Message(ServerMessage::Stream(Event::Update(update))) => {
                self.updates += 1;
                if update.binary_screens {
                    assert!(self.binary, "binary screens without opting in");
                    assert!(update.snapshots.is_empty() && update.patches.is_empty());
                    self.pending = true;
                } else {
                    // The opt-in precedes the subscription, so every screen is binary.
                    assert!(
                        !self.binary || update.snapshots.is_empty() && update.patches.is_empty()
                    );
                    for snapshot in update.snapshots {
                        self.screens.insert(snapshot.pane, snapshot);
                    }
                    for patch in update.patches {
                        let next = stream::apply(&self.screens[&patch.pane], &patch).unwrap();
                        self.screens.insert(patch.pane, next);
                    }
                }
            }
            ServerRecord::Screens(bytes) => {
                assert!(self.binary && self.pending, "unexpected binary record");
                self.pending = false;
                for message in screens::decode(&bytes, screens::DEFAULT_MAX_CELLS).unwrap() {
                    match message {
                        screens::ScreenMessage::Snapshot(snapshot) => {
                            stream::validate(&snapshot).unwrap();
                            self.snapshots += 1;
                            self.screens.insert(snapshot.pane, snapshot);
                        }
                        screens::ScreenMessage::Patch(patch) => {
                            let next = patch.apply(&self.screens[&patch.pane]).unwrap();
                            stream::validate(&next).unwrap();
                            self.patches += 1;
                            self.screens.insert(patch.pane, next);
                        }
                    }
                }
            }
            ServerRecord::Message(_) => {}
        }
        true
    }
    fn send(&mut self, request: Request) {
        write_message(&mut self.socket, &ClientMessage::Stream(request)).unwrap();
    }
    fn screen(&self, pane: u64) -> Option<&Screen> {
        self.screens.get(&pane).map(|snapshot| &snapshot.screen)
    }
}
/// Pump both observers until both show the same quiet screen of `pane`
/// satisfying `predicate`.
fn converge(a: &mut Observer, b: &mut Observer, pane: u64, predicate: impl Fn(&Screen) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut quiet = 0;
    loop {
        let progressed = a.pump(Duration::from_millis(40)) | b.pump(Duration::from_millis(40));
        quiet = if progressed { 0 } else { quiet + 1 };
        if quiet >= 5
            && !a.pending
            && !b.pending
            && a.screen(pane).is_some_and(&predicate)
            && a.screen(pane) == b.screen(pane)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "screens did not converge:\nA {:?}\nB {:?}",
            a.screen(pane).map(text),
            b.screen(pane).map(text)
        );
    }
}

#[test]
fn binary_screens_decode_to_the_screens_json_clients_receive() {
    let daemon = Daemon::start();
    let mut json = Observer::connect(&daemon, false, vec![1]);
    let mut binary = Observer::connect(&daemon, true, vec![1]);
    let mut control = daemon.peer();
    control.lease(1, false);
    // Colours, a hyperlink, wide and combining characters.
    control.send(Request::Input {
        pane: 1,
        bytes: "printf '\\033[1;31mred\\033[0m \\033[48;2;10;20;30mrgb\\033[0m 漢字 e\\314\\201 \\033]8;;https://example.com/x\\033\\\\link\\033]8;;\\033\\\\ %s\\n' MARK_ONE\n"
            .as_bytes()
            .to_vec(),
    });
    converge(&mut json, &mut binary, 1, |s| {
        text(s).contains("MARK_ONE")
            && s.cells.iter().any(|c| c.width == 2)
            && s.cells.iter().any(|c| c.hyperlink.is_some())
    });
    // Small changes arrive as row patches.
    control.send(Request::Input {
        pane: 1,
        bytes: b"echo MARK_TWO\n".to_vec(),
    });
    converge(&mut json, &mut binary, 1, |s| text(s).contains("MARK_TWO"));
    assert!(binary.patches > 0, "no binary patch was exercised");
    // A resize produces a full snapshot.
    let before = binary.snapshots;
    control.send(Request::Resize {
        pane: 1,
        cols: 160,
        rows: 50,
    });
    converge(&mut json, &mut binary, 1, |s| s.cols == 160 && s.rows == 50);
    assert!(binary.snapshots > before);
    // Backpressure: the binary client stops reading while a large pane
    // changes continuously. Its socket fills, so the server keeps replacing
    // its one pending update (with snapshots) instead of queueing every
    // frame. The JSON client unsubscribes meanwhile so that the server's
    // frame rate does not depend on JSON encoding of the large pane.
    json.send(Request::Subscribe { panes: vec![] });
    control.send(Request::Resize {
        pane: 1,
        cols: 400,
        rows: 150,
    });
    control.send(Request::Input {
        pane: 1,
        bytes: b"l=abcdefghijklmnopqrstuvwxyz0123456789; l=$l$l$l$l$l$l$l$l$l$l; i=0; end=$(($(date +%s) + 2)); while [ \"$(date +%s)\" -lt \"$end\" ]; do printf '\\033[3%dm%d %s\\033[0m\\n' $((i % 8)) \"$i\" \"$l\"; i=$((i+1)); done; echo MARK_''DONE\n".to_vec(),
    });
    let binary_before = binary.updates;
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(3000) {
        json.pump(Duration::from_millis(20));
    }
    json.send(Request::Subscribe { panes: vec![1] });
    converge(&mut json, &mut binary, 1, |s| {
        s.cols == 400 && text(s).contains("MARK_DONE")
    });
    // The output (1–2 s, 30 frames a second) ended while paused; a client
    // without replacement would receive every frame.

    let received = binary.updates - binary_before;
    assert!(
        received < 20,
        "the paused client received {received} updates; expected coalescing"
    );
    assert!(json.snapshots == 0 && json.patches == 0);
}
