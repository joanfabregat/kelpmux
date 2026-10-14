//! Actual CLI command-channel transport: pipes, framing, PTYs and bounded cleanup.
use kelpmux::native::{
    protocol::*,
    reference::State,
    stream::{self, Event, Request},
};
use std::{
    fs,
    io::{Read, Write},
    os::{fd::FromRawFd, unix::net::UnixStream},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
struct Server {
    root: PathBuf,
    child: Child,
}
struct ChildGuard(Child);
impl std::ops::Deref for ChildGuard {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}
impl std::ops::DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
fn wait(child: &mut Child) -> std::process::ExitStatus {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(s) = child.try_wait().unwrap() {
            return s;
        }
        if Instant::now() > until {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child did not finish within five seconds");
        }
        thread::sleep(Duration::from_millis(10));
    }
}
impl Server {
    fn start() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "kelpmux-bridge-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kelpmux"));
        cmd.args(["server", "--state-dir"])
            .arg(root.join("state"))
            .arg("--socket")
            .arg(root.join("state/control.sock"))
            .args(["--shell", "/bin/sh", "--cwd"])
            .arg(&root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = cmd.spawn().unwrap();
        let mut s = Self { root, child };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if UnixStream::connect(s.socket()).is_ok() {
                break;
            }
            assert!(s.child.try_wait().unwrap().is_none(), "server exited");
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        s
    }
    fn socket(&self) -> PathBuf {
        self.root.join("state/control.sock")
    }
    fn command(&self, verb: &str) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kelpmux"));
        c.arg(verb).arg("--socket").arg(self.socket());
        c
    }
    fn screen(&self) -> (PaneInfo, Screen) {
        let mut s = UnixStream::connect(self.socket()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        write_message(&mut s, &ClientMessage::InspectPane { pane: 1 }).unwrap();
        match read_message(&mut s).unwrap().unwrap() {
            ServerMessage::PaneState { pane, screen } => (pane, screen),
            other => panic!("{other:?}"),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Ok(mut s) = UnixStream::connect(self.socket()) {
            let _ = write_message(&mut s, &ClientMessage::Command(Action::Shutdown));
        }
        let until = Instant::now() + Duration::from_secs(3);
        while self.child.try_wait().unwrap().is_none() {
            if Instant::now() > until {
                let _ = self.child.kill();
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}
struct Bridge {
    child: Child,
    input: Option<ChildStdin>,
    events: Option<mpsc::Receiver<Result<ServerMessage, String>>>,
    reader: Option<thread::JoinHandle<()>>,
    state: State,
}
impl Bridge {
    fn start(s: &Server) -> Self {
        let mut child = s
            .command("bridge")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let mut output = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::sync_channel(8);
        let reader = thread::spawn(move || {
            loop {
                match read_message::<ServerMessage>(&mut output) {
                    Ok(Some(e)) => {
                        if tx.send(Ok(e)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string()));
                        break;
                    }
                }
            }
        });
        Self {
            child,
            input,
            events: Some(rx),
            reader: Some(reader),
            state: State::default(),
        }
    }
    fn send(&mut self, r: Request) {
        write_message(self.input.as_mut().unwrap(), &ClientMessage::Stream(r)).unwrap();
    }
    fn hello(&mut self, fragmented: bool) {
        let hello = ClientMessage::Stream(Request::Hello {
            version: stream::VERSION,
        });
        if fragmented {
            let mut bytes = Vec::new();
            write_message(&mut bytes, &hello).unwrap();
            for part in bytes.chunks(3) {
                self.input.as_mut().unwrap().write_all(part).unwrap();
                thread::sleep(Duration::from_millis(1));
            }
        } else {
            write_message(self.input.as_mut().unwrap(), &hello).unwrap();
        }
        self.until(|s| s.client != 0);
    }
    fn event(&mut self) -> Event {
        let message = self
            .events
            .as_ref()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .expect("bridge response timeout")
            .unwrap();
        let ServerMessage::Stream(event) = message else {
            panic!("non-stream reply {message:?}")
        };
        self.state.apply(event.clone()).unwrap();
        event
    }
    fn until(&mut self, test: impl Fn(&State) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(6);
        while !test(&self.state) {
            assert!(Instant::now() < deadline);
            self.event();
        }
    }
    fn finish(&mut self) -> std::process::ExitStatus {
        self.input.take();
        wait(&mut self.child)
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.input.take();
        if self.child.try_wait().unwrap().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        self.events.take();
        if let Some(t) = self.reader.take() {
            let _ = t.join();
        }
    }
}
fn screen_text(s: &Screen) -> String {
    s.cells.iter().map(|c| c.text.as_str()).collect()
}
#[test]
fn fragmented_bridge_streams_real_pty_releases_on_eof_and_preserves_shell() {
    let server = Server::start();
    let (original, geometry) = server.screen();
    let mut bridge = Bridge::start(&server);
    bridge.hello(true);
    bridge.send(Request::Subscribe { panes: vec![1] });
    bridge.send(Request::Claim {
        pane: 1,
        takeover: false,
    });
    bridge.send(Request::Resize {
        pane: 1,
        cols: 120,
        rows: 35,
    });
    bridge.until(|s| s.screens.get(&1).is_some_and(|p| p.screen.cols == 120));
    bridge.send(Request::Input {
        pane: 1,
        bytes: b"printf '\\033[2J\\033[HBRIDGE_READY\\n'\n".to_vec(),
    });
    bridge.until(|s| {
        s.screens
            .get(&1)
            .is_some_and(|p| screen_text(&p.screen).contains("BRIDGE_READY"))
    });
    let revision = bridge.state.screens[&1].revision;
    bridge.send(Request::Input {
        pane: 1,
        bytes: b"printf '\\033[Hchanged'\n".to_vec(),
    });
    let mut saw_patch = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !saw_patch {
        assert!(Instant::now() < deadline);
        if let Event::Update(u) = bridge.event() {
            saw_patch = u.patches.iter().any(|p| p.pane == 1 && p.base >= revision);
        }
    }
    assert!(bridge.finish().success());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (pane, screen) = server.screen();
        assert_eq!(pane.pid, original.pid);
        if screen.cols == geometry.cols {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    let mut again = Bridge::start(&server);
    again.hello(false);
    again.send(Request::Claim {
        pane: 1,
        takeover: false,
    });
    again.until(|s| s.leases.iter().any(|l| l.pane == 1 && l.owner == s.client));
}
#[test]
fn bridge_rejects_nonstream_truncated_oversized_and_wrong_handshake_without_shutdown() {
    let server = Server::start();
    let before = server.screen().0.pid;
    let mut command = Vec::new();
    write_message(&mut command, &ClientMessage::Command(Action::Shutdown)).unwrap();
    let mut wrong = Vec::new();
    write_message(
        &mut wrong,
        &ClientMessage::Stream(Request::Hello { version: 999 }),
    )
    .unwrap();
    for bytes in [
        command,
        wrong,
        vec![0, 0, 0, 20, b'{'],
        ((MAX_MESSAGE + 1) as u32).to_be_bytes().to_vec(),
        vec![0, 0, 0, 0],
        vec![0, 0, 0, 1, b'{'],
    ] {
        let mut bridge = Bridge::start(&server);
        bridge.input.as_mut().unwrap().write_all(&bytes).unwrap();
        assert!(!bridge.finish().success());
        assert_eq!(server.screen().0.pid, before);
    }
}
#[test]
fn stream_inspection_is_read_only_and_reports_current_subscriptions() {
    let server = Server::start();
    let (before, screen) = server.screen();
    let mut controller = UnixStream::connect(server.socket()).unwrap();
    controller
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write_message(
        &mut controller,
        &ClientMessage::Attach {
            version: VERSION,
            cols: 80,
            rows: 24,
            takeover: false,
        },
    )
    .unwrap();
    let owner = loop {
        if let Some(ServerMessage::Welcome { client_id, .. }) =
            read_message(&mut controller).unwrap()
        {
            break client_id;
        }
    };
    let geometry = server.screen().1;
    let mut child = server
        .command("stream-inspect")
        .arg("1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let output = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        output
            .take(MAX_MESSAGE as u64 + 1)
            .read_to_end(&mut bytes)
            .unwrap();
        let _ = tx.send(bytes);
    });
    assert!(wait(&mut child).success());
    let bytes = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    reader.join().unwrap();
    assert!(bytes.len() <= MAX_MESSAGE);
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["subscribed"], serde_json::json!([1]));
    assert_eq!(json["screens"]["1"]["screen"]["cols"], geometry.cols);
    assert!(json["leases"].as_array().unwrap().is_empty());
    let mut inspect = UnixStream::connect(server.socket()).unwrap();
    write_message(&mut inspect, &ClientMessage::Inspect).unwrap();
    let Some(ServerMessage::State {
        controller: Some(current),
        ..
    }) = read_message(&mut inspect).unwrap()
    else {
        panic!("missing controller")
    };
    assert_eq!(current, owner);
    assert_eq!(server.screen().0.pid, before.pid);
    assert_eq!(server.screen().1.cols, geometry.cols);
    assert!(screen.cols > 0);
}
#[test]
fn slow_or_broken_stdout_does_not_stall_other_clients() {
    let server = Server::start();
    let mut slow = ChildGuard(
        server
            .command("bridge")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut input = slow.stdin.take().unwrap();
    for request in [
        Request::Hello { version: 1 },
        Request::Subscribe { panes: vec![1] },
        Request::Claim {
            pane: 1,
            takeover: false,
        },
        Request::Resize {
            pane: 1,
            cols: 240,
            rows: 80,
        },
    ] {
        write_message(&mut input, &ClientMessage::Stream(request)).unwrap();
    }
    thread::sleep(Duration::from_millis(150));
    let mut observer = Bridge::start(&server);
    observer.hello(false);
    observer.send(Request::Subscribe { panes: vec![1] });
    observer.until(|s| s.screens.contains_key(&1));
    assert!(server.screen().1.cols > 0);
    drop(slow.stdout.take());
    drop(input);
    let _ = wait(&mut slow);
    assert!(server.child.id() > 0);
    let mut again = Bridge::start(&server);
    again.hello(false);
    again.send(Request::Claim {
        pane: 1,
        takeover: true,
    });
    again.until(|s| s.leases.iter().any(|l| l.owner == s.client));
}
#[test]
fn bridge_refuses_a_pty_command_channel() {
    let server = Server::start();
    let (mut master, mut slave) = (-1, -1);
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    let master = unsafe { fs::File::from_raw_fd(master) };
    let slave = unsafe { fs::File::from_raw_fd(slave) };
    let mut child = server
        .command("bridge")
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert!(!wait(&mut child).success());
    let mut text = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert!(text.contains("without a PTY"), "{text}");
    drop(master);
}

#[test]
fn bridge_pauses_a_coalesced_valid_burst_instead_of_rejecting_queue_pressure() {
    let server = Server::start();
    let mut bridge = Bridge::start(&server);
    bridge.hello(false);
    let mut bytes = Vec::new();
    // This exceeds the bridge's 32-record queue while remaining below the
    // server's deliberate 64-message flood limit, all in one 8 KiB read.
    for _ in 0..33 {
        write_message(
            &mut bytes,
            &ClientMessage::Stream(Request::Subscribe { panes: Vec::new() }),
        )
        .unwrap();
    }
    write_message(
        &mut bytes,
        &ClientMessage::Stream(Request::Subscribe { panes: vec![1] }),
    )
    .unwrap();
    assert!(bytes.len() < 8192);
    bridge.input.as_mut().unwrap().write_all(&bytes).unwrap();
    bridge.until(|state| state.screens.contains_key(&1));
    assert!(bridge.finish().success());
}

#[test]
fn bridge_reports_a_workspace_that_speaks_only_legacy_tui_messages() {
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};
    let server = Server::start();
    let socket = server.root.join("state/legacy.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let peer = thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(3);
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < until =>
                {
                    thread::sleep(Duration::from_millis(10))
                }
                other => panic!("legacy listener failed: {other:?}"),
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        assert!(matches!(
            read_message::<ClientMessage>(&mut socket).unwrap(),
            Some(ClientMessage::Stream(Request::Hello { .. }))
        ));
        write_message(
            &mut socket,
            &ServerMessage::Welcome {
                version: VERSION,
                client_id: 1,
                controller: false,
            },
        )
        .unwrap();
    });
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_kelpmux"))
            .arg("bridge")
            .arg("--socket")
            .arg(&socket)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    write_message(
        child.stdin.as_mut().unwrap(),
        &ClientMessage::Stream(Request::Hello { version: 1 }),
    )
    .unwrap();
    assert!(!wait(&mut child).success());
    let mut error = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut error)
        .unwrap();
    assert!(error.contains("native stream protocol"), "{error}");
    peer.join().unwrap();
}
