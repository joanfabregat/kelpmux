//! Deterministic pressure tests: bounded fixtures, no real agent transcripts.
use kelpmux::{
    native::{delta, diagnostics, engine::Terminal, protocol::*, workspace::Workspace},
    process,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

fn pane(id: u64) -> PaneInfo {
    PaneInfo {
        id,
        number: id as usize,
        palette: id as u8,
        pid: id as u32,
        title: format!("synthetic-{id}"),
        ..Default::default()
    }
}
#[test]
fn seeded_layout_and_terminal_sequences_preserve_all_nine_identities() {
    let mut ws = Workspace::new(160, 50).unwrap();
    ws.panes = (1..=9).map(pane).collect();
    ws.active = 1;
    ws.apply(&Action::Grid { cols: 3, rows: 3 }).unwrap();
    let identities: BTreeSet<_> = ws.panes.iter().map(|p| (p.id, p.pid, p.palette)).collect();
    let mut engines: BTreeMap<_, _> = (1..=9).map(|id| (id, Terminal::new(40, 10))).collect();
    let mut seed = 0x4b45_4c50_4d55_5801u64;
    for step in 0..800 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let id = 1 + seed % 9;
        match seed % 8 {
            0 => {
                ws.apply(&Action::Focus(id)).unwrap();
            }
            1 => {
                ws.apply(&Action::Read(id)).unwrap();
            }
            2 => {
                ws.apply(&Action::Fullscreen(id)).unwrap();
            }
            3 => {
                ws.apply(&Action::CloseReading).unwrap();
            }
            4 => {
                ws.apply(&Action::Swap {
                    source: id,
                    target: 1 + (seed >> 8) % 9,
                })
                .unwrap();
            }
            5 => {
                let geometry = [(40, 10), (80, 24), (160, 50), (240, 70)][(seed >> 8) as usize % 4];
                ws.resize(geometry.0, geometry.1).unwrap();
            }
            6 => {
                ws.apply(&Action::CloseReading).unwrap();
                ws.apply(&Action::ResizeGrid {
                    column: Some((0, 200 + (seed % 400) as u16)),
                    row: Some((0, 200 + (seed % 400) as u16)),
                })
                .unwrap();
            }
            _ => {
                let before = ws.panes.clone();
                assert!(ws.apply(&Action::Grid { cols: 1, rows: 1 }).is_err());
                assert_eq!(ws.panes, before);
            }
        }
        assert_eq!(
            ws.panes
                .iter()
                .map(|p| (p.id, p.pid, p.palette))
                .collect::<BTreeSet<_>>(),
            identities
        );
        assert!((1..=9).contains(&ws.active));
        let views = ws.layout();
        assert_eq!(views.len(), 9);
        for view in views.iter().filter(|v| v.body.cols > 0) {
            assert!(view.body.rows > 0);
            assert!(view.body.x + view.body.cols <= ws.cols);
            assert!(view.body.y + view.body.rows < ws.rows);
        }
        for (id, cols, rows) in ws.terminal_sizes() {
            let term = engines.get_mut(&id).unwrap();
            term.resize(cols, rows);
            term.feed(format!("\r\n\x1b[38;5;{}msynthetic-{step}-界\x1b[0m", id + 30).as_bytes());
            let screen = term.snapshot();
            assert_eq!(screen.cells.len(), usize::from(cols) * usize::from(rows));
            assert!(
                screen
                    .cursor
                    .is_none_or(|c| c.position.x < cols && c.position.y < rows)
            );
        }
    }
}
#[test]
fn small_changes_remain_substantially_smaller_than_full_frames() {
    for result in diagnostics::benchmark().unwrap() {
        assert!(
            result.patch_bytes * 10 < result.full_bytes,
            "{}x{} lost incremental advantage",
            result.cols,
            result.rows
        );
    }
}

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Daemon {
    root: PathBuf,
    socket: PathBuf,
    child: Child,
}
impl Daemon {
    fn start() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kelpmux-stress-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let socket = root.join("control.sock");
        let child = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
            .args(["server", "--socket"])
            .arg(&socket)
            .arg("--state-dir")
            .arg(root.join("state"))
            .args(["--shell", "/bin/sh", "--cwd"])
            .arg(&root)
            .args(["--size", "160x50"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut server = Self {
            root,
            socket,
            child,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !server.socket.exists() {
            assert!(server.child.try_wait().unwrap().is_none(), "server exited");
            assert!(Instant::now() < deadline, "server startup timeout");
            thread::sleep(Duration::from_millis(10));
        }
        server
    }
    fn peer(&self, takeover: bool) -> Peer {
        let stream = UnixStream::connect(&self.socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut peer = Peer {
            stream,
            frame: None,
            revision: 0,
        };
        peer.send(ClientMessage::Attach {
            version: VERSION,
            cols: 160,
            rows: 50,
            takeover,
        });
        assert!(matches!(peer.read(), ServerMessage::Welcome { .. }));
        peer.wait(|_| true);
        peer
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        if let Ok(mut stream) = UnixStream::connect(&self.socket) {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
            let _ = write_message(&mut stream, &ClientMessage::Command(Action::Shutdown));
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        if self.child.try_wait().ok().flatten().is_none() {
            if let Ok(tree) = process::snapshot(std::path::Path::new("/proc")) {
                for pid in process::descendants(&tree, self.child.id()) {
                    if tree.get(&pid).is_some_and(|old| {
                        process::read(std::path::Path::new("/proc"), pid)
                            .is_ok_and(|now| now.start == old.start)
                    }) {
                        unsafe {
                            libc::kill(pid as i32, libc::SIGKILL);
                        }
                    }
                }
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
struct Peer {
    stream: UnixStream,
    frame: Option<Frame>,
    revision: u64,
}
impl Peer {
    fn send(&mut self, msg: ClientMessage) {
        write_message(&mut self.stream, &msg).unwrap();
    }
    fn read(&mut self) -> ServerMessage {
        match read_message(&mut self.stream)
            .unwrap()
            .expect("disconnected")
        {
            ServerMessage::Frame(frame) => {
                self.revision = 1;
                self.frame = Some(frame.clone());
                ServerMessage::Frame(frame)
            }
            ServerMessage::Patch(patch) => {
                let frame =
                    delta::apply(self.frame.as_ref().unwrap(), self.revision, &patch).unwrap();
                self.revision = patch.revision;
                self.frame = Some(frame.clone());
                ServerMessage::Frame(frame)
            }
            other => other,
        }
    }
    fn wait(&mut self, predicate: impl Fn(&Frame) -> bool) -> Frame {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            assert!(Instant::now() < deadline, "frame deadline exceeded");
            match self.read() {
                ServerMessage::Frame(f) if predicate(&f) => return f,
                ServerMessage::Error(e) => panic!("server error: {e}"),
                _ => (),
            }
        }
    }
    fn action(&mut self, a: Action) {
        self.send(ClientMessage::Action(a));
    }
}
#[test]
fn nine_noisy_ptys_survive_slow_observer_controller_reconnect_and_resizing() {
    let daemon = Daemon::start();
    let mut controller = daemon.peer(false);
    controller.action(Action::Grid { cols: 3, rows: 3 });
    let initial = controller.wait(|f| f.panes.len() == 9);
    let identities: BTreeSet<_> = initial
        .panes
        .iter()
        .map(|p| (p.pane.id, p.pane.pid))
        .collect();
    let _slow_observer = daemon.peer(false);
    for pane in &initial.panes {
        controller.action(Action::Focus(pane.pane.id));
        controller.send(ClientMessage::Input(format!("i=0; while [ $i -lt 240 ]; do printf '\\033[38;5;42mnoise-{}-%s\\033[0m\\n' \"$i\"; i=$((i+1)); done; printf 'DONE-{}\\n'\n",pane.pane.id,pane.pane.id).into_bytes()));
    }
    for (cols, rows) in [(80, 24), (240, 70), (40, 10), (160, 50)] {
        controller.send(ClientMessage::Resize { cols, rows });
        controller.wait(|f| f.cols == cols && f.rows == rows);
    }
    drop(controller);
    let mut reconnect = daemon.peer(true);
    let final_frame = reconnect.wait(|f| f.controller && f.panes.len() == 9);
    assert_eq!(
        final_frame
            .panes
            .iter()
            .map(|p| (p.pane.id, p.pane.pid))
            .collect::<BTreeSet<_>>(),
        identities
    );
    // Inspect authoritative histories rather than relying on render pacing or sleeps.
    let deadline = Instant::now() + Duration::from_secs(12);
    for pane in &initial.panes {
        loop {
            reconnect.send(ClientMessage::History {
                pane: pane.pane.id,
                start: -100,
                limit: 200,
            });
            let lines = loop {
                if let ServerMessage::History { pane: id, lines } = reconnect.read() {
                    assert_eq!(id, pane.pane.id);
                    break lines;
                }
            };
            if lines
                .iter()
                .any(|(_, line)| line.contains(&format!("DONE-{}", pane.pane.id)))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "PTY did not finish bounded workload"
            );
        }
    }
    reconnect.send(ClientMessage::Diagnostics);
    loop {
        if let ServerMessage::Diagnostics(report) = reconnect.read() {
            assert_eq!(report.panes, 9);
            assert!(report.pty_bytes > 9 * 1000);
            break;
        }
    }
}
