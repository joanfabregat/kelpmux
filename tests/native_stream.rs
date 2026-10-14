use kelpmux::native::{
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
    thread: Option<thread::JoinHandle<kelpmux::Result<()>>>,
}
impl Daemon {
    fn start() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "kelpmux-stream-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let config = ServerConfig {
            initial_weights: Default::default(),
            template: None,
            socket: root.join("control.sock"),
            state_dir: root.join("state"),
            shell: "/bin/sh".into(),
            cwd: root.clone(),
            cols: 100,
            rows: 30,
            ui: Default::default(),
            initial_grid: (2, 1),
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
    fn inspect(&self, pane: u64) -> Screen {
        let mut s = self.socket();
        write_message(&mut s, &ClientMessage::InspectPane { pane }).unwrap();
        match read_message(&mut s).unwrap().unwrap() {
            ServerMessage::PaneState { screen, .. } => screen,
            other => panic!("{other:?}"),
        }
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        if let Ok(mut s) = UnixStream::connect(self.root.join("control.sock")) {
            let _ = write_message(&mut s, &ClientMessage::Command(Action::Shutdown));
        }
        if let Some(t) = self.thread.take() {
            assert!(t.join().unwrap().is_ok());
        }
        fs::remove_dir_all(&self.root).unwrap();
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
                Event::Error { message } => panic!("{message}"),
                _ => {}
            }
        }
    }
    fn error(&mut self, r: Request) -> String {
        self.send(r);
        loop {
            if let Event::Error { message } = self.next() {
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
fn handshake_bounds_mixed_modes_resync_and_tui_coexistence() {
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
    let mut tui = d.socket();
    write_message(
        &mut tui,
        &ClientMessage::Attach {
            version: VERSION,
            cols: 80,
            rows: 24,
            takeover: false,
        },
    )
    .unwrap();
    loop {
        if matches!(
            read_message::<ServerMessage>(&mut tui).unwrap(),
            Some(ServerMessage::Welcome { .. })
        ) {
            break;
        }
    }
    write_message(&mut tui, &ClientMessage::Input(b"BAD\n".to_vec())).unwrap();
    loop {
        if let Some(ServerMessage::Error(e)) = read_message(&mut tui).unwrap() {
            assert!(e.contains("claim"));
            break;
        }
    }
    assert_eq!(d.inspect(1).cols, 140);
    let mut observer = d.socket();
    write_message(
        &mut observer,
        &ClientMessage::Attach {
            version: VERSION,
            cols: 80,
            rows: 24,
            takeover: false,
        },
    )
    .unwrap();
    loop {
        if matches!(
            read_message::<ServerMessage>(&mut observer).unwrap(),
            Some(ServerMessage::Welcome { .. })
        ) {
            break;
        }
    }
    write_message(&mut observer, &ClientMessage::Command(Action::ClaimPane(1))).unwrap();
    loop {
        if let Some(ServerMessage::Error(e)) = read_message(&mut observer).unwrap() {
            assert!(e.contains("observer"));
            break;
        }
    }
    assert_eq!(d.inspect(1).cols, 140);
    write_message(&mut a.socket, &ClientMessage::Command(Action::Shutdown)).unwrap();
    loop {
        if let Some(ServerMessage::Error(e)) = read_message(&mut a.socket).unwrap() {
            assert!(e.contains("stream peers"));
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
    write_message(&mut tui, &ClientMessage::Action(Action::ClaimPane(1))).unwrap();
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
    use kelpmux::{
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
