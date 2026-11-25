//! Session API on the stream protocol (KelpServer ADR 0002, steps 1–3).
use kelpserver::native::{
    protocol::*,
    server::{ServerConfig, serve},
    stream::{self, CloseReason, Event, PaneSnapshot, Request, StartupPanes, StopReason, Update},
};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

struct Daemon {
    root: PathBuf,
    thread: Option<thread::JoinHandle<kelpserver::Result<()>>>,
    /// The connection that created the two seeded panes.
    seeder: Option<u64>,
    /// Dropped last, after the server stopped.
    _watchdog: Watchdog,
}
impl Daemon {
    /// A server with two panes (1 and 2), created by a seeding client.
    fn start() -> Self {
        Self::start_with(|_| {})
    }
    /// `prepare` may seed the private state directory before the server starts.
    fn start_with(prepare: impl FnOnce(&std::path::Path)) -> Self {
        let mut daemon = Self::empty(prepare);
        let mut seeder = daemon.peer();
        assert_eq!(seeder.spawn("seed-1", &[]), 1);
        assert_eq!(seeder.spawn("seed-2", &[]), 2);
        daemon.seeder = Some(seeder.id);
        daemon
    }
    /// A server as it starts: without panes.
    fn empty(prepare: impl FnOnce(&std::path::Path)) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "kelpserver-session-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let state = root.join("state");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        prepare(&state);
        let config = ServerConfig {
            socket: root.join("control.sock"),
            state_dir: state,
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
            seeder: None,
            _watchdog: Watchdog::start(),
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
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        s
    }
    fn peer(&self) -> Peer {
        let mut p = Peer {
            socket: self.socket(),
            id: 0,
            epoch: String::new(),
            capabilities: Vec::new(),
            server_version: None,
            welcome: None,
            update: None,
            screens: BTreeMap::new(),
        };
        p.send(Request::Hello {
            version: stream::VERSION,
        });
        loop {
            let event = p.next();
            if let Event::Welcome {
                client,
                epoch,
                capabilities,
                server_version,
                ..
            } = event.clone()
            {
                p.welcome = Some(event);
                p.id = client;
                p.epoch = epoch;
                p.capabilities = capabilities;
                p.server_version = server_version;
                break;
            }
        }
        p
    }
    /// The pane's current screen, observed by a separate stream client.
    fn inspect(&self, pane: u64) -> Screen {
        let mut observer = self.peer();
        observer.send(Request::Subscribe { panes: vec![pane] });
        observer.screen_until(pane, |_| true);
        observer.screens[&pane].screen.clone()
    }
    fn wait_stopped(&mut self) {
        let thread = self.thread.take().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !thread.is_finished() {
            assert!(Instant::now() < deadline, "server did not stop");
            thread::sleep(Duration::from_millis(20));
        }
        assert!(thread.join().unwrap().is_ok());
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
    capabilities: Vec<String>,
    server_version: Option<String>,
    welcome: Option<Event>,
    update: Option<Update>,
    screens: BTreeMap<u64, PaneSnapshot>,
}
impl Peer {
    /// Welcome's step 2 fields: startup policy, recovery signal, limits and
    /// notification run.
    fn welcome_fields(
        &self,
    ) -> (
        Option<StartupPanes>,
        bool,
        Option<stream::Limits>,
        Option<String>,
    ) {
        match self.welcome.clone().unwrap() {
            Event::Welcome {
                startup_panes,
                recovery_pending,
                limits,
                notification_run,
                ..
            } => (startup_panes, recovery_pending, limits, notification_run),
            _ => unreachable!(),
        }
    }
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
            for snapshot in &u.snapshots {
                self.screens.insert(snapshot.pane, snapshot.clone());
            }
            for patch in &u.patches {
                let next = stream::apply(&self.screens[&patch.pane], patch).unwrap();
                self.screens.insert(patch.pane, next);
            }
            self.update = Some(u.clone());
        }
        event
    }
    /// The first event matching `pick`, failing on any error event.
    fn until<T>(&mut self, mut pick: impl FnMut(&Event) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "timed out waiting for an event");
            let event = self.next();
            if let Event::Error { message, .. } = &event {
                panic!("{message}");
            }
            if let Some(value) = pick(&event) {
                return value;
            }
        }
    }
    /// The first update satisfying `predicate`.
    fn update_until(&mut self, predicate: impl Fn(&Update) -> bool) -> Update {
        if let Some(update) = self.update.clone().filter(|u| predicate(u)) {
            return update;
        }
        self.until(|event| match event {
            Event::Update(u) if predicate(u) => Some(u.clone()),
            _ => None,
        })
    }
    fn error(&mut self, r: Request) -> (String, Option<String>) {
        self.send(r);
        loop {
            if let Event::Error { message, request } = self.next() {
                return (message, request);
            }
        }
    }
    fn lease(&mut self, pane: u64) {
        self.send(Request::Claim {
            pane,
            takeover: false,
        });
        self.until(|event| match event {
            Event::Lease {
                pane: p,
                owner: Some(owner),
            } if *p == pane => Some(*owner),
            _ => None,
        });
    }
    fn spawn(&mut self, request: &str, argv: &[&str]) -> u64 {
        self.send(Request::Spawn {
            request: request.into(),
            cwd: None,
            argv: argv.iter().map(|a| (*a).into()).collect(),
            shell: None,
        });
        self.until(|event| match event {
            Event::Spawned { request: r, pane } if r == request => Some(*pane),
            _ => None,
        })
    }
    fn screen_until(&mut self, pane: u64, predicate: impl Fn(&Screen) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self
            .screens
            .get(&pane)
            .is_some_and(|s| predicate(&s.screen))
        {
            assert!(Instant::now() < deadline, "screen condition not reached");
            self.next();
        }
    }
}
fn text(s: &Screen) -> String {
    s.cells.iter().map(|c| c.text.as_str()).collect()
}

#[test]
fn welcome_advertises_session_capabilities_and_server_version() {
    let daemon = Daemon::start();
    let peer = daemon.peer();
    for capability in [
        "pane-lifecycle",
        "closed-sessions",
        "pane-history",
        "pane-scroll",
        "notification-ack-all",
        "host-status",
        "server-lifecycle",
        "diagnostics",
    ] {
        assert!(
            peer.capabilities.iter().any(|c| c == capability),
            "{capability}"
        );
    }
    assert_eq!(
        peer.server_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
}

/// A v1 client that opts in to nothing must never see an event type it does
/// not know, even while another client creates and closes panes.
#[test]
fn clients_that_do_not_opt_in_receive_only_v1_events() {
    let daemon = Daemon::start();
    let mut old = daemon.peer();
    old.send(Request::Subscribe { panes: vec![] });
    let mut new = daemon.peer();
    new.send(Request::SessionEvents { enabled: true });
    let pane = new.spawn("compat-1", &[]);
    new.send(Request::Close {
        request: "compat-2".into(),
        pane,
    });
    new.until(|event| {
        matches!(event, Event::Done { request } if request == "compat-2").then_some(())
    });
    // The raw JSON is what a v1 client decodes.
    let deadline = Instant::now() + Duration::from_secs(4);
    old.socket
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    while Instant::now() < deadline {
        let mut header = [0u8; 4];
        if std::io::Read::read_exact(&mut old.socket, &mut header).is_err() {
            continue;
        }
        let mut payload = vec![0; u32::from_be_bytes(header) as usize];
        std::io::Read::read_exact(&mut old.socket, &mut payload).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        let kind = value["Stream"]["type"].as_str().unwrap().to_owned();
        assert!(
            ["welcome", "update", "lease", "clipboard", "error"].contains(&kind.as_str()),
            "{kind}"
        );
        if kind == "update" {
            assert!(value["Stream"].get("sessions").is_none());
            assert!(value["Stream"].get("host").is_none());
        }
        if kind == "welcome" {
            assert_eq!(value["Stream"]["version"], 1);
        }
    }
}

#[test]
fn spawn_and_close_emit_ordered_lifecycle_events_with_session_data() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    peer.send(Request::SessionEvents { enabled: true });
    let initial = peer.update_until(|u| u.sessions.len() == 2);
    let ids: Vec<_> = initial.sessions.iter().map(|s| s.pane).collect();
    assert_eq!(ids, vec![1, 2], "IDs give creation order");
    assert!(
        initial
            .sessions
            .iter()
            .all(|s| s.created_at > 0 && s.argv.is_empty())
    );

    peer.send(Request::Spawn {
        request: "spawn-1".into(),
        cwd: Some(daemon.root.clone()),
        argv: vec![
            "sh".into(),
            "-c".into(),
            "echo SPAWNED; exec sleep 300".into(),
        ],
        shell: None,
    });
    let mut created = None;
    let mut spawned = None;
    while created.is_none() || spawned.is_none() {
        match peer.until(|e| match e {
            Event::PaneCreated { .. } | Event::Spawned { .. } => Some(e.clone()),
            _ => None,
        }) {
            Event::PaneCreated { session } => {
                assert!(spawned.is_none(), "creation event precedes the reply");
                created = Some(session);
            }
            Event::Spawned { pane, .. } => spawned = Some(pane),
            _ => unreachable!(),
        }
    }
    let session = created.unwrap();
    let pane = spawned.unwrap();
    assert_eq!(session.pane, pane);
    assert!(pane > 2, "IDs are never reused");
    assert_eq!(session.argv, ["sh", "-c", "echo SPAWNED; exec sleep 300"]);
    assert_eq!(
        session.launch_cwd,
        daemon.root.canonicalize().unwrap().to_string_lossy()
    );
    assert!(session.startup.as_deref().unwrap().contains("SPAWNED"));
    // The first update after the event already contains the pane.
    let update = peer.until(|e| match e {
        Event::Update(u) => Some(u.clone()),
        _ => None,
    });
    assert!(update.panes.iter().any(|p| p.id == pane));
    assert!(update.sessions.iter().any(|s| s.pane == pane));

    // An idempotent retry returns the same pane without creating another.
    assert_eq!(
        peer.spawn("spawn-1", &["sh", "-c", "echo SPAWNED; exec sleep 300"]),
        pane
    );
    let (message, request) = peer.error(Request::Spawn {
        request: "spawn-1".into(),
        cwd: None,
        argv: vec![],
        shell: None,
    });
    assert!(message.contains("different parameters"), "{message}");
    assert_eq!(request.as_deref(), Some("spawn-1"));

    // Another client's lease protects the pane from being closed.
    let mut other = daemon.peer();
    other.lease(pane);
    let (message, request) = peer.error(Request::Close {
        request: "close-1".into(),
        pane,
    });
    assert!(message.contains("leased"), "{message}");
    assert_eq!(request.as_deref(), Some("close-1"));
    drop(other);
    // The server notices the disconnect asynchronously; retry until the lease is gone.
    let deadline = Instant::now() + Duration::from_secs(5);
    let closed = 'retry: loop {
        assert!(Instant::now() < deadline, "lease was not released");
        peer.send(Request::Close {
            request: "close-2".into(),
            pane,
        });
        loop {
            match peer.next() {
                Event::Error { request, .. } if request.as_deref() == Some("close-2") => {
                    thread::sleep(Duration::from_millis(50));
                    continue 'retry;
                }
                event @ Event::PaneClosed { .. } => break 'retry event,
                _ => {}
            }
        }
    };
    let Event::PaneClosed {
        pane: closed,
        reason,
        record,
        ..
    } = closed
    else {
        unreachable!()
    };
    assert_eq!(closed, pane);
    assert_eq!(reason, CloseReason::Closed);
    assert!(record.is_none(), "no foreground program was observed");
    peer.until(|e| matches!(e, Event::Done { request } if request == "close-2").then_some(()));
    let update = peer.until(|e| match e {
        Event::Update(u) => Some(u.clone()),
        _ => None,
    });
    assert!(!update.panes.iter().any(|p| p.id == pane));
    assert!(!update.sessions.iter().any(|s| s.pane == pane));
    let (message, _) = peer.error(Request::Close {
        request: "close-3".into(),
        pane,
    });
    assert!(message.contains("unknown pane"), "{message}");
}

#[test]
fn exited_panes_report_status_then_close() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    peer.send(Request::SessionEvents { enabled: true });
    // The startup command runs before `exec $SHELL -i`; `exit` ends the shell.
    let pane = peer.spawn("exit-1", &["exit", "7"]);
    let (code, signal) = peer.until(|e| match e {
        Event::PaneExited {
            pane: p,
            code,
            signal,
            ..
        } if *p == pane => Some((*code, *signal)),
        Event::PaneClosed { pane: p, .. } if *p == pane => panic!("closed before exit"),
        _ => None,
    });
    assert_eq!((code, signal), (Some(7), None));
    let reason = peer.until(|e| match e {
        Event::PaneClosed {
            pane: p, reason, ..
        } if *p == pane => Some(*reason),
        _ => None,
    });
    assert_eq!(reason, CloseReason::Exited);
}

#[test]
fn history_search_scroll_and_reveal_on_the_stream() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    peer.send(Request::Subscribe { panes: vec![1] });
    peer.lease(1);
    peer.send(Request::Input {
        pane: 1,
        // Quotes keep the typed command line from matching the output.
        bytes: b"i=0; while [ $i -lt 80 ]; do echo row-$i; i=$((i+1)); done; echo NEEDLE-O''NE\n"
            .to_vec(),
    });
    peer.screen_until(1, |s| text(s).contains("NEEDLE-ONE"));
    peer.send(Request::History {
        request: "h1".into(),
        pane: 1,
        start: -1000,
        limit: 200,
    });
    let lines = peer.until(|e| match e {
        Event::History {
            request,
            pane: 1,
            lines,
        } if request == "h1" => Some(lines.clone()),
        _ => None,
    });
    assert!(lines.iter().any(|(_, line)| line == "row-0"), "{lines:?}");
    assert!(lines.windows(2).all(|w| w[0].0 < w[1].0));
    let (message, request) = peer.error(Request::History {
        request: "h2".into(),
        pane: 1,
        start: 0,
        limit: 201,
    });
    assert!(message.contains("200"), "{message}");
    assert_eq!(request.as_deref(), Some("h2"));

    // Search across every pane, and across an explicit subset.
    let mut other = daemon.peer();
    other.send(Request::Subscribe { panes: vec![2] });
    other.lease(2);
    other.send(Request::Input {
        pane: 2,
        bytes: b"echo needle-T''WO\n".to_vec(),
    });
    other.screen_until(2, |s| text(s).contains("needle-TWO"));
    peer.send(Request::Search {
        request: "s1".into(),
        panes: vec![],
        query: "needle".into(),
        limit: 64,
    });
    let (results, truncated) = peer.until(|e| match e {
        Event::Search {
            request,
            results,
            truncated,
        } if request == "s1" => Some((results.clone(), *truncated)),
        _ => None,
    });
    assert!(!truncated);
    let panes: Vec<_> = results.iter().map(|r| r.pane).collect();
    assert_eq!(panes, vec![1, 2], "creation order");
    assert!(results.iter().all(|r| !r.hits.is_empty()));
    peer.send(Request::Search {
        request: "s2".into(),
        panes: vec![2],
        query: "NEEDLE".into(),
        limit: 1,
    });
    let results = peer.until(|e| match e {
        Event::Search {
            request, results, ..
        } if request == "s2" => Some(results.clone()),
        _ => None,
    });
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].pane, 2);
    assert_eq!(results[0].hits.len(), 1);
    let (message, request) = peer.error(Request::Search {
        request: "s3".into(),
        panes: vec![99],
        query: "x".into(),
        limit: 1,
    });
    assert!(message.contains("invalid pane search"), "{message}");
    assert_eq!(request.as_deref(), Some("s3"));
    let (message, request) = peer.error(Request::Search {
        request: "bad id!".into(),
        panes: vec![],
        query: "x".into(),
        limit: 1,
    });
    assert!(message.contains("request identifier"), "{message}");
    assert_eq!(request, None);

    // Scrolling needs the lease and moves the pane's shared viewport.
    let (message, _) = other.error(Request::Scroll { pane: 1, lines: 10 });
    assert!(message.contains("lease"), "{message}");
    peer.send(Request::Scroll { pane: 1, lines: 30 });
    peer.screen_until(1, |s| {
        !text(s).contains("NEEDLE-ONE") && text(s).contains("row-")
    });
    peer.send(Request::Reveal { pane: 1, line: 0 });
    peer.screen_until(1, |s| text(s).contains("NEEDLE-ONE"));
}

#[test]
fn acknowledge_all_marks_every_notification_read() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let daemon = Daemon::start_with(|state| {
        let events: Vec<_> = (1..=2)
            .map(|id| {
                serde_json::json!({
                    "id": id, "pane": 1, "title": format!("seeded {id}"),
                    "status": "turn_finished", "at": now, "read": false, "workspace": "test"
                })
            })
            .collect();
        let path = state.join("activity.json");
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "initialized": true, "next_id": 3, "events": events, "seen": {}
            }))
            .unwrap(),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    });
    let mut peer = daemon.peer();
    peer.send(Request::Subscribe { panes: vec![] });
    peer.update_until(|u| u.notifications.len() == 2 && u.notifications.iter().all(|n| !n.read));
    peer.send(Request::AcknowledgeAll);
    peer.update_until(|u| u.notifications.len() == 2 && u.notifications.iter().all(|n| n.read));
}

#[test]
fn host_status_is_opt_in() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    peer.send(Request::Subscribe { panes: vec![] });
    let first = peer.update_until(|_| true);
    assert!(first.host.is_none());
    peer.send(Request::HostStatus { enabled: true });
    let update = peer.update_until(|u| u.host.as_ref().is_some_and(|h| h.ready));
    let host = update.host.unwrap();
    assert!(host.memory_total > 0);
    assert!(!host.cpus.is_empty());
    peer.send(Request::HostStatus { enabled: false });
    peer.update_until(|u| u.host.is_none());
}

/// Parking is client-only. A client that hides a pane keeps its lease (and so
/// its size) while unsubscribing; the server needs no "parked" concept.
#[test]
fn a_leased_but_unsubscribed_pane_keeps_its_size() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    peer.send(Request::Subscribe { panes: vec![1] });
    peer.lease(1);
    peer.send(Request::Resize {
        pane: 1,
        cols: 57,
        rows: 19,
    });
    peer.screen_until(1, |s| s.cols == 57 && s.rows == 19);
    peer.send(Request::Subscribe { panes: vec![] });
    peer.update_until(|u| u.subscribed.is_empty());
    thread::sleep(Duration::from_millis(300));
    let screen = daemon.inspect(1);
    assert_eq!((screen.cols, screen.rows), (57, 19));
    // Releasing returns the pane to the server's base size (`--size`).
    peer.send(Request::Release { pane: 1 });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let screen = daemon.inspect(1);
        if (screen.cols, screen.rows) == (100, 30) {
            break;
        }
        assert!(Instant::now() < deadline, "{}x{}", screen.cols, screen.rows);
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn closed_sessions_list_and_restore_by_record() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    peer.send(Request::SessionEvents { enabled: true });
    let pane = peer.spawn("prog-1", &[]);
    peer.lease(pane);
    peer.send(Request::Input {
        pane,
        bytes: b"sleep 300\n".to_vec(),
    });
    // Discovery observes the foreground program within a few seconds.
    peer.update_until(|u| u.panes.iter().any(|p| p.id == pane && p.command.is_some()));
    // One more discovery cycle so the history observation is saved.
    thread::sleep(Duration::from_millis(2500));
    peer.send(Request::Close {
        request: "close-prog".into(),
        pane,
    });
    let record = peer.until(|e| match e {
        Event::PaneClosed {
            pane: p, record, ..
        } if *p == pane => Some(record.clone()),
        _ => None,
    });
    let record = record.expect("a closed program leaves a record");
    assert_eq!(record, format!("{}:{pane}", peer.epoch));

    let (message, request) = peer.error(Request::Restore {
        request: "early".into(),
        record: record.clone(),
        rerun: true,
    });
    assert!(message.contains("list closed sessions"), "{message}");
    assert_eq!(request.as_deref(), Some("early"));

    peer.send(Request::ClosedSessions {
        request: "list-1".into(),
    });
    let records = peer.until(|e| match e {
        Event::ClosedSessions { request, records } if request == "list-1" => Some(records.clone()),
        _ => None,
    });
    let listed = records
        .iter()
        .find(|r| r.record.key == record)
        .expect("record is listed");
    assert_eq!(listed.record.argv, ["sleep", "300"]);
    assert_eq!(listed.record.reason, "Closed");
    assert_eq!(listed.rerun.as_deref(), Some("'sleep' '300'"));
    assert!(listed.resume.is_none());

    peer.send(Request::Restore {
        request: "restore-1".into(),
        record: record.clone(),
        rerun: true,
    });
    let (restored, existing) = peer.until(|e| match e {
        Event::Restored {
            request,
            record: r,
            pane,
            existing,
        } if request == "restore-1" => {
            assert_eq!(r, &record);
            Some((*pane, *existing))
        }
        _ => None,
    });
    assert!(!existing);
    assert!(restored > pane);
    let update = peer.update_until(|u| u.sessions.iter().any(|s| s.pane == restored));
    let session = update.sessions.iter().find(|s| s.pane == restored).unwrap();
    assert_eq!(session.startup.as_deref(), Some("'sleep' '300'"));

    // A record is consumed by restoring it.
    let (message, _) = peer.error(Request::Restore {
        request: "restore-2".into(),
        record: record.clone(),
        rerun: true,
    });
    assert!(
        message.contains("unknown closed-session record"),
        "{message}"
    );
    peer.send(Request::ClosedSessions {
        request: "list-2".into(),
    });
    let records = peer.until(|e| match e {
        Event::ClosedSessions { request, records } if request == "list-2" => Some(records.clone()),
        _ => None,
    });
    assert!(!records.iter().any(|r| r.record.key == record));
}

#[test]
fn shutdown_request_announces_the_stop() {
    let mut daemon = Daemon::start();
    let mut peer = daemon.peer();
    peer.send(Request::SessionEvents { enabled: true });
    peer.send(Request::Shutdown {
        request: "stop-1".into(),
    });
    peer.until(|e| matches!(e, Event::Done { request } if request == "stop-1").then_some(()));
    let reason = peer.until(|e| match e {
        Event::ServerStopping { reason } => Some(*reason),
        _ => None,
    });
    assert_eq!(reason, StopReason::Shutdown);
    daemon.wait_stopped();
}

/// A client that sends a request and disconnects without reading its
/// earlier replies resets the connection (Linux reports `ECONNRESET` to
/// the server once the client's unread data is discarded). The request
/// read before the reset is still handled: the test harness stops its
/// server exactly like this, and losing the `shutdown` hung the tests.
#[test]
fn a_request_sent_just_before_a_reset_is_handled() {
    use std::os::fd::AsRawFd;
    let mut daemon = Daemon::empty(|_| {});
    let mut socket = daemon.socket();
    write_message(
        &mut socket,
        &ClientMessage::Stream(Request::Hello {
            version: stream::VERSION,
        }),
    )
    .unwrap();
    // Wait, without reading it, until the welcome is in the socket.
    let mut ready = libc::pollfd {
        fd: socket.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    assert_eq!(
        unsafe { libc::poll(&mut ready, 1, 10_000) },
        1,
        "no welcome"
    );
    write_message(
        &mut socket,
        &ClientMessage::Stream(Request::Shutdown {
            request: "reset".into(),
        }),
    )
    .unwrap();
    drop(socket);
    daemon.wait_stopped();
}

#[test]
fn last_pane_exit_announces_the_stop() {
    let mut daemon = Daemon::start();
    let mut peer = daemon.peer();
    peer.send(Request::SessionEvents { enabled: true });
    peer.send(Request::Subscribe { panes: vec![1, 2] });
    for pane in [1, 2] {
        peer.lease(pane);
        peer.send(Request::Input {
            pane,
            bytes: b"exit\n".to_vec(),
        });
    }
    let reason = peer.until(|e| match e {
        Event::ServerStopping { reason } => Some(*reason),
        _ => None,
    });
    assert_eq!(reason, StopReason::LastPaneExited);
    daemon.wait_stopped();
}

#[test]
fn new_requests_require_hello() {
    let daemon = Daemon::start();
    let mut socket = daemon.socket();
    write_message(
        &mut socket,
        &ClientMessage::Stream(Request::ClosedSessions {
            request: "x".into(),
        }),
    )
    .unwrap();
    match read_message::<ServerMessage>(&mut socket).unwrap().unwrap() {
        ServerMessage::Stream(Event::Error { message, request }) => {
            assert!(message.contains("hello"), "{message}");
            assert_eq!(request.as_deref(), Some("x"));
        }
        other => panic!("{other:?}"),
    }
    // The pane stream itself still works afterwards for a proper client.
    let mut peer = daemon.peer();
    peer.send(Request::Subscribe { panes: vec![1] });
    peer.screen_until(1, |_| true);
    assert_eq!(daemon.inspect(1).cols, peer.screens[&1].screen.cols);
}

// Step 2 server support (ADR 0002): startup policy, recovery signal, spawn
// shell, unused-shell and spawned-by observations, limits, notification run.

fn private_file(path: &std::path::Path, contents: &[u8]) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn welcome_advertises_step2_capabilities_limits_and_notification_run() {
    let daemon = Daemon::start();
    let peer = daemon.peer();
    for capability in [
        "startup-policy",
        "recovery-signal",
        "spawn-shell",
        "unused-shell",
        "stream-limits",
        "notification-run",
        "spawned-by",
    ] {
        assert!(
            peer.capabilities.iter().any(|c| c == capability),
            "{capability}"
        );
    }
    // Step 3 removed the composed administration request.
    assert!(!peer.capabilities.iter().any(|c| c == "pane-spawn"));
    let (startup, recovery, limits, run) = peer.welcome_fields();
    assert_eq!(startup, Some(StartupPanes::None));
    assert!(!recovery, "a first run is clean");
    assert_eq!(limits, Some(stream::LIMITS));
    assert!(
        limits.is_some_and(|l| l.subscriptions >= 24),
        "a 24-pane grid fits"
    );
    assert!(run.is_some_and(|run| !run.is_empty()));
    // A clean run omits the recovery field from the wire.
    let json = serde_json::to_value(peer.welcome.as_ref().unwrap()).unwrap();
    assert!(json.get("recovery_pending").is_none(), "{json}");
    assert_eq!(json["startup_panes"], "none");
}

#[test]
fn startup_policy_parses_only_none() {
    assert_eq!("none".parse::<StartupPanes>().unwrap(), StartupPanes::None);
    let error = "layout".parse::<StartupPanes>().unwrap_err().to_string();
    assert!(error.contains("removed"), "{error}");
    assert!("zero".parse::<StartupPanes>().is_err());
    assert_eq!(StartupPanes::default(), StartupPanes::None);
}

/// A running 0.2 server started with `--startup-panes layout` still says so
/// in Welcome; a 0.3 client must decode that Welcome (and an unknown future
/// policy) instead of failing to attach, list or stop the workspace.
#[test]
fn welcome_from_older_or_newer_servers_decodes_any_startup_policy() {
    for (value, expected) in [
        ("none", StartupPanes::None),
        ("layout", StartupPanes::Layout),
        ("from-template", StartupPanes::Unknown),
    ] {
        let json = format!(
            r#"{{"Stream":{{"type":"welcome","version":1,"client":4,"epoch":"e","capabilities":["pane-lifecycle","startup-policy"],"server_version":"0.2.0","startup_panes":"{value}"}}}}"#
        );
        let ServerMessage::Stream(Event::Welcome { startup_panes, .. }) =
            serde_json::from_str::<ServerMessage>(&json).unwrap()
        else {
            panic!("not a welcome: {json}");
        };
        assert_eq!(startup_panes, Some(expected), "{value}");
    }
    // A 0.3 server keeps sending only "none".
    assert_eq!(
        serde_json::to_value(StartupPanes::None).unwrap(),
        serde_json::json!("none")
    );
    assert!("layout".parse::<StartupPanes>().is_err());
}

/// The server starts with no pane and stays usable; a client creates its
/// layout with `spawn`. Files a client keeps in the state directory, such as
/// a saved layout or its own `workspace.json` keys, are left alone.
#[test]
fn a_server_starts_empty_and_leaves_client_files_alone() {
    let layout = br#"{"seeded":"client tab layout"}"#;
    let daemon = Daemon::empty(|state| {
        private_file(&state.join("tab-layout.json"), layout);
        private_file(&state.join("workspace.json"), br#"{"tab_count":3}"#);
    });
    let state = daemon.root.join("state");
    let mut peer = daemon.peer();
    assert_eq!(peer.welcome_fields().0, Some(StartupPanes::None));
    peer.send(Request::SessionEvents { enabled: true });
    peer.send(Request::Subscribe { panes: vec![] });
    let first = peer.update_until(|_| true);
    assert!(first.panes.is_empty(), "{:?}", first.panes);
    assert!(first.sessions.is_empty());

    let a = peer.spawn("none-1", &[]);
    let b = peer.spawn("none-2", &[]);
    assert_eq!((a, b), (1, 2), "the client's panes are the first ones");
    peer.send(Request::Subscribe { panes: vec![a, b] });
    peer.screen_until(b, |s| (s.cols, s.rows) == (100, 30));
    peer.send(Request::Close {
        request: "none-3".into(),
        pane: a,
    });
    peer.until(|e| matches!(e, Event::Done { request } if request == "none-3").then_some(()));
    assert_eq!(fs::read(state.join("tab-layout.json")).unwrap(), layout);
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(state.join("workspace.json")).unwrap()).unwrap();
    assert_eq!(settings, serde_json::json!({"tab_count": 3}));
}

#[test]
fn recovery_signal_is_shared_and_dismissable() {
    let unclean =
        |state: &std::path::Path| private_file(&state.join("server.running.json"), b"true");
    // A stream client sees the signal and dismisses it for everyone.
    let daemon = Daemon::start_with(unclean);
    let mut peer = daemon.peer();
    assert!(peer.welcome_fields().1);
    let json = serde_json::to_value(peer.welcome.as_ref().unwrap()).unwrap();
    assert_eq!(json["recovery_pending"], true);
    assert!(daemon.peer().welcome_fields().1, "shown to every client");
    peer.send(Request::DismissRecovery {
        request: "recovery-1".into(),
    });
    peer.until(|e| matches!(e, Event::Done { request } if request == "recovery-1").then_some(()));
    assert!(!daemon.peer().welcome_fields().1);
}

/// A record of the composed protocol 58 (removed in 0.3) gets a clear error
/// instead of a silent disconnect or a hang, then the connection closes.
#[test]
fn composed_protocol_messages_get_a_clear_error() {
    let daemon = Daemon::start();
    let mut tui = daemon.socket();
    write_message(
        &mut tui,
        &serde_json::json!({"Attach": {"version": 58, "cols": 120, "rows": 40, "takeover": false}}),
    )
    .unwrap();
    match read_message::<ServerMessage>(&mut tui).unwrap() {
        Some(ServerMessage::Error(error)) => {
            assert!(error.contains("protocol 58"), "{error}");
            assert!(error.contains("stream"), "{error}");
        }
        other => panic!("{other:?}"),
    }
    assert!(read_message::<ServerMessage>(&mut tui).unwrap().is_none());
    // Stream clients are unaffected.
    daemon.peer();
}

#[test]
fn diagnostics_are_a_stream_request() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    assert!(peer.capabilities.iter().any(|c| c == "diagnostics"));
    peer.send(Request::Diagnostics {
        request: "diag-1".into(),
    });
    let report = peer.until(|e| match e {
        Event::Diagnostics { request, report } if request == "diag-1" => Some(report.clone()),
        _ => None,
    });
    assert_eq!(report.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(report.protocol, stream::VERSION);
    assert_eq!((report.cols, report.rows, report.panes), (100, 30, 2));
    assert!(report.clients >= 1);
}

#[test]
fn spawn_accepts_only_a_valid_shell() {
    let daemon = Daemon::start();
    let shell = ["/bin/bash", "/usr/bin/bash", "/bin/dash", "/usr/bin/dash"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .expect("a second shell");
    let mut peer = daemon.peer();
    peer.send(Request::SessionEvents { enabled: true });
    let spawn = |request: &str, shell: Option<PathBuf>| Request::Spawn {
        request: request.into(),
        cwd: None,
        argv: vec![],
        shell,
    };
    peer.send(spawn("shell-1", Some(shell.clone())));
    let pane = peer.until(|e| match e {
        Event::Spawned { request, pane } if request == "shell-1" => Some(*pane),
        _ => None,
    });
    let update = peer.update_until(|u| u.sessions.iter().any(|s| s.pane == pane));
    let session = update.sessions.iter().find(|s| s.pane == pane).unwrap();
    assert_eq!(session.shell, shell.to_string_lossy());

    // The shell is part of the retry identity.
    peer.send(spawn("shell-1", Some(shell.clone())));
    let again = peer.until(|e| match e {
        Event::Spawned { request, pane } if request == "shell-1" => Some(*pane),
        _ => None,
    });
    assert_eq!(again, pane);
    let (message, _) = peer.error(spawn("shell-1", None));
    assert!(message.contains("different parameters"), "{message}");

    let plain = daemon.root.join("not-executable");
    private_file(&plain, b"#!/bin/sh\n");
    for (index, bad) in [
        PathBuf::from("sh"),
        daemon.root.join("missing-shell"),
        plain,
        daemon.root.clone(),
    ]
    .into_iter()
    .enumerate()
    {
        let request = format!("bad-shell-{index}");
        let (message, failed) = peer.error(spawn(&request, Some(bad.clone())));
        assert!(message.contains("shell"), "{bad:?}: {message}");
        assert_eq!(failed.as_deref(), Some(request.as_str()));
    }
    // Without a shell, the request serializes as before.
    let json = serde_json::to_value(spawn("x", None)).unwrap();
    assert!(json.get("shell").is_none(), "{json}");
}

#[test]
fn unused_shell_is_observed_for_session_event_clients() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    peer.send(Request::SessionEvents { enabled: true });
    peer.update_until(|u| u.sessions.iter().any(|s| s.pane == 1 && s.unused_shell));
    // Holding the lease does not hide the observation from its owner.
    peer.send(Request::Subscribe { panes: vec![1] });
    peer.lease(1);
    peer.send(Request::Input {
        pane: 1,
        bytes: b"sleep 300\n".to_vec(),
    });
    peer.update_until(|u| u.sessions.iter().any(|s| s.pane == 1 && !s.unused_shell));
}

#[test]
fn sessions_record_the_spawning_connection() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let mut other = daemon.peer();
    assert_ne!(peer.id, other.id);
    peer.send(Request::SessionEvents { enabled: true });
    let mine = peer.spawn("by-1", &[]);
    let theirs = other.spawn("by-2", &[]);
    let update = peer.update_until(|u| u.sessions.iter().any(|s| s.pane == theirs));
    let by = |pane: u64| {
        update
            .sessions
            .iter()
            .find(|s| s.pane == pane)
            .unwrap()
            .spawned_by
    };
    assert_eq!(by(1), daemon.seeder);
    assert_eq!(by(mine), Some(peer.id));
    assert_eq!(by(theirs), Some(other.id));
}

/// Closing the last pane first creates a replacement shell, which the server
/// created itself: it has no creator, and the field is omitted.
#[test]
fn closing_the_last_pane_leaves_a_replacement_shell() {
    let daemon = Daemon::empty(|_| {});
    let mut peer = daemon.peer();
    peer.send(Request::SessionEvents { enabled: true });
    let pane = peer.spawn("last-1", &[]);
    peer.send(Request::Close {
        request: "last-2".into(),
        pane,
    });
    let replacement = peer.until(|e| match e {
        Event::PaneCreated { session } => Some(session.clone()),
        _ => None,
    });
    peer.until(|e| matches!(e, Event::Done { request } if request == "last-2").then_some(()));
    assert!(replacement.pane > pane);
    assert_eq!(replacement.spawned_by, None);
    let json = serde_json::to_value(&replacement).unwrap();
    assert!(json.get("spawned_by").is_none(), "{json}");
    let update = peer.update_until(|u| u.panes.len() == 1);
    assert_eq!(update.panes[0].id, replacement.pane);
}

#[test]
fn more_than_ten_panes_fit_the_advertised_subscription_limit() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let mut panes = vec![1, 2];
    for index in 0..10 {
        panes.push(peer.spawn(&format!("grid-{index}"), &[]));
    }
    peer.send(Request::Subscribe {
        panes: panes.clone(),
    });
    let update = peer.update_until(|u| u.subscribed.len() == panes.len());
    assert_eq!(update.snapshots.len(), panes.len());
    // The reference client accepts the same update.
    let mut state = kelpserver::native::reference::State::default();
    state.apply(peer.welcome.clone().unwrap()).unwrap();
    state.apply(Event::Update(update)).unwrap();
    assert_eq!(state.screens.len(), panes.len());

    let too_many = vec![1; stream::MAX_SUBSCRIPTIONS + 1];
    let (message, _) = peer.error(Request::Subscribe { panes: too_many });
    assert!(message.contains("invalid pane subscriptions"), "{message}");
}

/// `kelpserver start`, `status`, `diagnostics` and `stop` administer the
/// server over the stream protocol. `--startup-panes none` stays accepted.
#[test]
fn lifecycle_commands_use_the_stream_protocol() {
    let root =
        std::env::temp_dir().join(format!("kelpserver-session-start-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let state = root.join("state");
    let run = |args: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_kelpserver"))
            .args(args)
            .arg("--state-dir")
            .arg(&state)
            .output()
            .unwrap();
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    let (ok, _, stderr) = run(&["start", "--startup-panes", "layout"]);
    assert!(!ok && stderr.contains("removed"), "{stderr}");
    let (ok, stdout, stderr) = run(&[
        "start",
        "--shell",
        "/bin/sh",
        "--cwd",
        root.to_str().unwrap(),
        "--startup-panes",
        "none",
    ]);
    assert!(ok, "{stderr}");
    let listed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(listed["panes"], serde_json::json!([]), "{stdout}");
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(state.join("workspace.json")).unwrap()).unwrap();
    assert_eq!(settings["shell"], "/bin/sh");

    let mut socket = UnixStream::connect(state.join("control.sock")).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write_message(
        &mut socket,
        &ClientMessage::Stream(Request::Hello {
            version: stream::VERSION,
        }),
    )
    .unwrap();
    let startup = match read_message::<ServerMessage>(&mut socket).unwrap().unwrap() {
        ServerMessage::Stream(Event::Welcome { startup_panes, .. }) => startup_panes,
        other => panic!("{other:?}"),
    };
    drop(socket);
    assert_eq!(startup, Some(StartupPanes::None));

    let (ok, _, stderr) = run(&["status"]);
    assert!(ok, "{stderr}");
    assert!(
        stderr.contains(&format!(
            "server {} (stream protocol 1)",
            env!("CARGO_PKG_VERSION")
        )),
        "{stderr}"
    );
    let (ok, stdout, stderr) = run(&["diagnostics"]);
    assert!(ok, "{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["version"], env!("CARGO_PKG_VERSION"));
    let (ok, _, stderr) = run(&["stop"]);
    assert!(ok, "{stderr}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while state.join("control.sock").exists() {
        assert!(Instant::now() < deadline, "server did not stop");
        thread::sleep(Duration::from_millis(20));
    }
    let _ = fs::remove_dir_all(&root);
}
