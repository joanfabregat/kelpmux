use kelpmux::native::{
    protocol::*,
    server::{ServerConfig, serve},
};
use std::{
    fs,
    io::Write,
    os::unix::{fs::PermissionsExt, net::UnixStream},
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
            "kelpmux-native-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let config = ServerConfig {
            socket: root.join("native.sock"),
            state_dir: root.join("state"),
            shell: "/bin/sh".into(),
            cwd: root.clone(),
            cols: 80,
            rows: 24,
        };
        let handle = thread::spawn(move || serve(config));
        let daemon = Self {
            root,
            thread: Some(handle),
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !daemon.root.join("native.sock").exists() {
            assert!(Instant::now() < deadline, "daemon socket did not appear");
            thread::sleep(Duration::from_millis(10));
        }
        daemon
    }
    fn connect(&self) -> UnixStream {
        let stream = UnixStream::connect(self.root.join("native.sock")).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
    }
    fn attach(&self) -> (UnixStream, u64, bool) {
        let mut stream = self.connect();
        write_message(
            &mut stream,
            &ClientMessage::Attach {
                version: VERSION,
                cols: 80,
                rows: 24,
                takeover: false,
            },
        )
        .unwrap();
        match next(&mut stream) {
            ServerMessage::Welcome {
                client_id,
                controller,
                ..
            } => (stream, client_id, controller),
            other => panic!("unexpected welcome: {other:?}"),
        }
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        if let Ok(mut stream) = UnixStream::connect(self.root.join("native.sock")) {
            let _ = write_message(&mut stream, &ClientMessage::Command(Action::Shutdown));
        }
        if let Some(handle) = self.thread.take() {
            assert!(handle.join().unwrap().is_ok(), "daemon failed");
        }
        fs::remove_dir_all(&self.root).unwrap();
    }
}
fn next(stream: &mut UnixStream) -> ServerMessage {
    read_message(stream).unwrap().expect("server disconnected")
}
fn state(stream: &mut UnixStream) -> (Vec<PaneInfo>, Option<u64>) {
    write_message(stream, &ClientMessage::Inspect).unwrap();
    loop {
        if let ServerMessage::State {
            panes, controller, ..
        } = next(stream)
        {
            return (panes, controller);
        }
    }
}
fn wait_controller(stream: &mut UnixStream, expected: Option<u64>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if state(stream).1 == expected {
            return;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn reconnect_keeps_shell_pid_and_pane_identity() {
    let daemon = Daemon::start();
    assert_eq!(
        fs::metadata(daemon.root.join("native.sock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let (mut first, id, controller) = daemon.attach();
    assert!(controller);
    let (before, owner) = state(&mut first);
    assert_eq!(owner, Some(id));
    assert_eq!(before.len(), 1);
    write_message(
        &mut first,
        &ClientMessage::Input(b"printf alive\\n\n".to_vec()),
    )
    .unwrap();
    drop(first);
    let mut inspector = daemon.connect();
    wait_controller(&mut inspector, None);
    let (mut reattached, _, controller) = daemon.attach();
    assert!(controller);
    let (after, _) = state(&mut reattached);
    assert_eq!(before[0].id, after[0].id);
    assert_eq!(before[0].pid, after[0].pid);
    assert!(!after[0].dead);
}
#[test]
fn observers_need_explicit_takeover_and_inspect_never_owns_control() {
    let daemon = Daemon::start();
    let mut inspector = daemon.connect();
    assert_eq!(state(&mut inspector).1, None);
    let (mut first, first_id, _) = daemon.attach();
    let (mut observer, second_id, control) = daemon.attach();
    assert!(!control);
    assert_eq!(state(&mut observer).1, Some(first_id));
    write_message(&mut observer, &ClientMessage::Input(b"exit\n".to_vec())).unwrap();
    loop {
        if let ServerMessage::Error(error) = next(&mut observer) {
            assert!(error.contains("observer"));
            break;
        }
    }
    write_message(&mut observer, &ClientMessage::Action(Action::Takeover)).unwrap();
    wait_controller(&mut observer, Some(second_id));
    assert_eq!(state(&mut first).1, Some(second_id));
    write_message(&mut observer, &ClientMessage::Action(Action::Detach)).unwrap();
    wait_controller(&mut first, None);
    assert!(!state(&mut first).0[0].dead);
}
#[test]
fn malformed_clients_and_unread_observer_do_not_block_server() {
    let daemon = Daemon::start();
    let (mut controller, _, _) = daemon.attach();
    let (_slow, _, _) = daemon.attach();
    let mut malformed = daemon.connect();
    malformed.write_all(&u32::MAX.to_be_bytes()).unwrap();
    drop(malformed);
    write_message(
        &mut controller,
        &ClientMessage::Input(
            b"i=0; while [ $i -lt 1000 ]; do printf 'output\\n'; i=$((i+1)); done\n".to_vec(),
        ),
    )
    .unwrap();
    write_message(&mut controller, &ClientMessage::Ping).unwrap();
    loop {
        if matches!(next(&mut controller), ServerMessage::Pong) {
            break;
        }
    }
    assert!(!state(&mut controller).0[0].dead);
}
#[test]
fn active_server_lock_and_symlink_socket_are_rejected() {
    let daemon = Daemon::start();
    let config = ServerConfig {
        socket: daemon.root.join("native.sock"),
        state_dir: daemon.root.join("state"),
        shell: "/bin/sh".into(),
        cwd: daemon.root.clone(),
        cols: 80,
        rows: 24,
    };
    assert!(serve(config).is_err());
    let target = daemon.root.join("foreign");
    fs::write(&target, b"preserve").unwrap();
    let link = daemon.root.join("link.sock");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let config = ServerConfig {
        socket: link,
        state_dir: daemon.root.join("other-state"),
        shell: "/bin/sh".into(),
        cwd: daemon.root.clone(),
        cols: 80,
        rows: 24,
    };
    assert!(serve(config).is_err());
    assert_eq!(fs::read(target).unwrap(), b"preserve");
}
#[test]
fn pty_output_colors_and_terminal_queries_reach_the_live_engine() {
    let daemon = Daemon::start();
    let (mut stream, _, _) = daemon.attach();
    // The shell emits a cursor-position query; its read verifies the engine wrote a response into the PTY.
    write_message(&mut stream, &ClientMessage::Input(b"stty -echo -icanon min 1 time 0; printf '\\033[31mCOLOR\\033[0m\\033[6n'; reply=$(dd bs=1 count=6 2>/dev/null); printf '\\nQ''UERY_OK\\n'; stty sane\n".to_vec())).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut color = false;
    let mut query = false;
    while !color || !query {
        assert!(
            Instant::now() < deadline,
            "terminal color/query response missing"
        );
        if let ServerMessage::Frame(frame) = next(&mut stream) {
            color |= frame
                .cells
                .iter()
                .any(|c| c.text == "C" && c.fg == Color::Indexed(1));
            let text: String = frame.cells.iter().map(|c| c.text.as_str()).collect();
            query |= text.contains("QUERY_OK");
        }
    }
}

#[test]
fn grid_shrink_is_transactional_and_preserves_background_jobs() {
    let daemon = Daemon::start();
    let (mut stream, owner, _) = daemon.attach();
    let mut ctl = daemon.connect();
    write_message(
        &mut ctl,
        &ClientMessage::Command(Action::Grid { cols: 2, rows: 1 }),
    )
    .unwrap();
    let (panes, controller) = state(&mut ctl);
    assert_eq!(panes.len(), 2);
    assert_eq!(controller, Some(owner));
    write_message(
        &mut stream,
        &ClientMessage::Action(Action::Focus(panes[1].id)),
    )
    .unwrap();
    write_message(&mut stream, &ClientMessage::Input(b"sleep 60 &\n".to_vec())).unwrap();
    thread::sleep(Duration::from_millis(100));
    write_message(
        &mut ctl,
        &ClientMessage::Command(Action::Grid { cols: 1, rows: 1 }),
    )
    .unwrap();
    loop {
        if let ServerMessage::Error(error) = next(&mut ctl) {
            assert!(error.contains("occupied"));
            break;
        }
    }
    let (after, _) = state(&mut ctl);
    assert_eq!(
        after.iter().map(|p| (p.id, p.pid)).collect::<Vec<_>>(),
        panes.iter().map(|p| (p.id, p.pid)).collect::<Vec<_>>()
    );
    write_message(
        &mut stream,
        &ClientMessage::Input(b"kill %1; wait\n".to_vec()),
    )
    .unwrap();
    thread::sleep(Duration::from_millis(150));
    write_message(
        &mut ctl,
        &ClientMessage::Command(Action::Grid { cols: 1, rows: 1 }),
    )
    .unwrap();
    assert_eq!(state(&mut ctl).0.len(), 1);
}

#[test]
fn journal_lock_does_not_stall_pty_loop_or_shutdown() {
    let daemon = Daemon::start();
    let root = daemon.root.join("state/native");
    kelpmux::journal::private_dir(&root).unwrap();
    let lock = kelpmux::journal::Lock::acquire(&root.join("registry.lock"), false).unwrap();
    let (mut stream, _, _) = daemon.attach();
    thread::sleep(Duration::from_millis(2100));
    let start = Instant::now();
    write_message(&mut stream, &ClientMessage::Ping).unwrap();
    loop {
        if matches!(next(&mut stream), ServerMessage::Pong) {
            break;
        }
    }
    assert!(start.elapsed() < Duration::from_secs(1));
    drop(daemon);
    drop(lock);
}
