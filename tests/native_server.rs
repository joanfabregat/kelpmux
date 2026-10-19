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
            initial_weights: Default::default(),
            template: None,
            socket: root.join("native.sock"),
            state_dir: root.join("state"),
            shell: "/bin/sh".into(),
            cwd: root.clone(),
            cols: 80,
            rows: 24,
            ui: Default::default(),
            initial_grid: (1, 1),
            initial_tabs: None,
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
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            match UnixStream::connect(self.root.join("native.sock")) {
                Ok(stream) => break stream,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                    ) && Instant::now() < deadline =>
                {
                    // bind creates the path just before listen makes it ready.
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("daemon did not accept connections: {error}"),
            }
        };
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
    use std::{cell::RefCell, collections::BTreeMap, os::fd::AsRawFd};
    thread_local! {static FRAMES:RefCell<BTreeMap<i32,(u64,Frame)>>=const { RefCell::new(BTreeMap::new()) };}
    let message = read_message(stream).unwrap().expect("server disconnected");
    FRAMES.with(|frames| {
        let mut frames = frames.borrow_mut();
        match message {
            ServerMessage::Frame(frame) => {
                frames.insert(stream.as_raw_fd(), (1, frame.clone()));
                ServerMessage::Frame(frame)
            }
            ServerMessage::Patch(patch) => {
                let (revision, previous) =
                    frames.get(&stream.as_raw_fd()).expect("patch before frame");
                let frame = kelpmux::native::delta::apply(previous, *revision, &patch).unwrap();
                frames.insert(stream.as_raw_fd(), (patch.revision, frame.clone()));
                ServerMessage::Frame(frame)
            }
            other => other,
        }
    })
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
            b"i=0; while [ $i -lt 30 ]; do printf 'output %s\\n' \"$i\"; i=$((i+1)); sleep 0.04; done; printf 'DONE_''UNBLOCKED\\n'\n".to_vec(),
        ),
    )
    .unwrap();
    // Repeated dirty frames exceed an unread Unix socket's send buffer. Wait
    // for actual PTY output completion before testing the control connection.
    wait_frame(&mut controller, |frame| {
        frame
            .cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
            .contains("DONE_UNBLOCKED")
    });
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
        initial_weights: Default::default(),
        template: None,
        socket: daemon.root.join("native.sock"),
        state_dir: daemon.root.join("state"),
        shell: "/bin/sh".into(),
        cwd: daemon.root.clone(),
        cols: 80,
        rows: 24,
        ui: Default::default(),
        initial_grid: (1, 1),
        initial_tabs: None,
    };
    assert!(serve(config).is_err());
    let target = daemon.root.join("foreign");
    fs::write(&target, b"preserve").unwrap();
    let link = daemon.root.join("link.sock");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let config = ServerConfig {
        initial_weights: Default::default(),
        template: None,
        socket: link,
        state_dir: daemon.root.join("other-state"),
        shell: "/bin/sh".into(),
        cwd: daemon.root.clone(),
        cols: 80,
        rows: 24,
        ui: Default::default(),
        initial_grid: (1, 1),
        initial_tabs: None,
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
fn manual_layout_is_rejected_and_preserves_background_jobs() {
    let daemon = Daemon::start();
    let (mut stream, owner, _) = daemon.attach();
    let mut ctl = daemon.connect();
    write_message(&mut ctl, &ClientMessage::Command(Action::NewTab)).unwrap();
    let (panes, controller) = state(&mut ctl);
    assert_eq!(panes.len(), 2);
    assert_eq!(controller, Some(owner));
    start_background(&mut stream, &panes[0]);
    write_message(
        &mut stream,
        &ClientMessage::Action(Action::Focus(panes[1].id)),
    )
    .unwrap();
    write_message(&mut stream, &ClientMessage::Input(b"sleep 60 &\n".to_vec())).unwrap();
    wait_process(panes[1].pid, |tree| {
        kelpmux::process::descendants(tree, panes[1].pid)
            .iter()
            .any(|pid| tree.get(pid).is_some_and(|p| p.command == "sleep"))
    });
    let tree = kelpmux::process::snapshot(std::path::Path::new("/proc")).unwrap();
    let job = kelpmux::process::descendants(&tree, panes[1].pid)
        .into_iter()
        .find(|pid| tree.get(pid).is_some_and(|p| p.command == "sleep"))
        .unwrap();
    let job_start = tree[&job].start;
    write_message(
        &mut ctl,
        &ClientMessage::Command(Action::Grid { cols: 1, rows: 1 }),
    )
    .unwrap();
    loop {
        if let ServerMessage::Error(error) = next(&mut ctl) {
            assert!(error.contains("automatic"));
            break;
        }
    }
    let (after, _) = state(&mut ctl);
    assert_eq!(
        kelpmux::process::read(std::path::Path::new("/proc"), job)
            .unwrap()
            .start,
        job_start
    );
    assert_eq!(
        after.iter().map(|p| (p.id, p.pid)).collect::<Vec<_>>(),
        panes.iter().map(|p| (p.id, p.pid)).collect::<Vec<_>>()
    );
    write_message(
        &mut stream,
        &ClientMessage::Input(b"kill %1; wait\n".to_vec()),
    )
    .unwrap();
    wait_process(panes[1].pid, |tree| {
        kelpmux::process::idle(tree, panes[1].pid, false)
    });
    command(&mut ctl, Action::ClosePaneRequest(panes[1].id)).unwrap();
    assert_eq!(
        command(&mut ctl, Action::ClosePaneConfirm(panes[1].id))
            .unwrap()
            .0
            .len(),
        1
    );
}

#[test]
fn journal_lock_does_not_stall_pty_loop_or_shutdown() {
    let daemon = Daemon::start();
    let root = daemon.root.join("state/native");
    kelpmux::journal::private_dir(&root).unwrap();
    // The initial metadata pass may briefly own this lock, especially on ARM.
    // Acquire it with a bounded retry before testing the server while we hold it.
    let deadline = Instant::now() + Duration::from_secs(5);
    let lock = loop {
        match kelpmux::journal::Lock::acquire(&root.join("registry.lock"), false) {
            Ok(lock) => break lock,
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "fixture could not acquire journal lock: {error}"
                );
                thread::sleep(Duration::from_millis(10));
            }
        }
    };
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

fn command(
    stream: &mut UnixStream,
    action: Action,
) -> Result<(Vec<PaneInfo>, Option<u64>), String> {
    write_message(stream, &ClientMessage::Command(action)).unwrap();
    loop {
        match next(stream) {
            ServerMessage::State {
                panes, controller, ..
            } => return Ok((panes, controller)),
            ServerMessage::Error(message) => return Err(message),
            _ => (),
        }
    }
}
fn wait_process(
    pid: u32,
    predicate: impl Fn(&std::collections::BTreeMap<u32, kelpmux::process::Process>) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let tree = kelpmux::process::snapshot(std::path::Path::new("/proc")).unwrap();
        assert!(tree.contains_key(&pid), "pane shell exited");
        if predicate(&tree) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "process state did not reach fixture barrier"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
fn create_tabs(stream: &mut UnixStream, cols: u8, rows: u8) -> Vec<PaneInfo> {
    let count = usize::from(cols) * usize::from(rows);
    let mut panes = state(stream).0;
    assert!(panes.len() <= count);
    while panes.len() < count {
        panes = command(stream, Action::NewTab).unwrap().0;
    }
    panes
}
fn wait_frame(stream: &mut UnixStream, predicate: impl Fn(&Frame) -> bool) -> Frame {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "expected server frame missing");
        if let ServerMessage::Frame(frame) = next(stream) {
            if predicate(&frame) {
                return frame;
            }
        }
    }
}
#[test]
fn first_grid_mouse_gesture_focuses_without_reaching_a_mouse_reporting_pty() {
    let daemon = Daemon::start();
    let (mut stream, _, _) = daemon.attach();
    let panes = create_tabs(&mut stream, 2, 1);
    let target = panes[1].id;
    let capture = daemon.root.join("mouse-byte");
    write_message(&mut stream, &ClientMessage::Input(format!(
        "stty -echo -icanon min 1 time 0; printf '\\033[?1000h\\033[?1006hMOUSE_''READY\\n'; dd bs=1 count=1 of={} 2>/dev/null; printf '\\033[?1000l\\033[?1006lMOUSE_''RECEIVED\\n'; stty sane\n", capture.display()
    ).into_bytes())).unwrap();
    wait_frame(&mut stream, |frame| {
        frame
            .panes
            .iter()
            .any(|view| view.pane.id == target && view.modes.mouse_sgr)
    });
    command(&mut stream, Action::CloseReading).unwrap();
    command(&mut stream, Action::Focus(panes[0].id)).unwrap();
    let grid = wait_frame(&mut stream, |frame| {
        frame.reading == ReadingMode::Grid && frame.active == panes[0].id
    });
    let body = grid
        .panes
        .iter()
        .find(|view| view.pane.id == target)
        .unwrap()
        .body;
    let press = MouseEvent {
        position: Point {
            x: body.x + 2,
            y: body.y + 2,
        },
        kind: MouseKind::Press,
        button: 0,
        shift: false,
        alt: false,
        ctrl: false,
    };
    for kind in [MouseKind::Press, MouseKind::Move, MouseKind::Release] {
        write_message(
            &mut stream,
            &ClientMessage::Mouse(MouseEvent { kind, ..press }),
        )
        .unwrap();
    }
    wait_frame(&mut stream, |frame| frame.active == target);
    // Give the fixture reader time to consume any mistakenly forwarded event.
    let deadline = Instant::now() + Duration::from_millis(200);
    while Instant::now() < deadline {
        assert!(fs::read(&capture).unwrap_or_default().is_empty());
        thread::sleep(Duration::from_millis(10));
    }
    write_message(&mut stream, &ClientMessage::Mouse(press)).unwrap();
    wait_frame(&mut stream, |frame| {
        frame
            .cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
            .contains("MOUSE_RECEIVED")
    });
    assert_eq!(fs::read(capture).unwrap(), b"\x1b");
}

#[test]
fn controller_cursor_footer_and_observer_geometry_survive_takeover() {
    let daemon = Daemon::start();
    let (mut controller, _, _) = daemon.attach();
    let controlled = wait_frame(&mut controller, |frame| {
        frame.controller && frame.cursor.is_some_and(|c| c.visible)
    });
    let footer: String = controlled.cells
        [(controlled.rows as usize - 1) * controlled.cols as usize..]
        .iter()
        .map(|cell| cell.text.as_str())
        .collect();
    assert!(!footer.contains("Obs"));
    let mut observer = daemon.connect();
    write_message(
        &mut observer,
        &ClientMessage::Attach {
            version: VERSION,
            cols: 100,
            rows: 30,
            takeover: false,
        },
    )
    .unwrap();
    assert!(matches!(
        next(&mut observer),
        ServerMessage::Welcome {
            controller: false,
            ..
        }
    ));
    let observed = wait_frame(&mut observer, |frame| !frame.controller);
    assert_eq!((observed.cols, observed.rows), (80, 24));
    assert!(observed.cursor.is_none_or(|cursor| !cursor.visible));
    let footer: String = observed.cells[(observed.rows as usize - 1) * observed.cols as usize..]
        .iter()
        .map(|cell| cell.text.as_str())
        .collect();
    assert!(footer.contains("Obs"));
    write_message(
        &mut observer,
        &ClientMessage::Resize {
            cols: 110,
            rows: 35,
        },
    )
    .unwrap();
    write_message(&mut observer, &ClientMessage::Ping).unwrap();
    loop {
        match next(&mut observer) {
            ServerMessage::Pong => break,
            ServerMessage::Error(error) => panic!("observer resize rejected: {error}"),
            _ => (),
        }
    }
    write_message(&mut controller, &ClientMessage::Action(Action::Help)).unwrap();
    assert_eq!(wait_frame(&mut controller, |frame| frame.help).cols, 80);
    write_message(&mut controller, &ClientMessage::Action(Action::Help)).unwrap();
    write_message(&mut observer, &ClientMessage::Action(Action::Takeover)).unwrap();
    let taken = wait_frame(&mut observer, |frame| {
        frame.controller && frame.cols == 110 && frame.rows == 35 && !frame.help
    });
    assert!(taken.cursor.is_some_and(|cursor| cursor.visible));
    assert!(
        !wait_frame(&mut controller, |frame| !frame.controller
            && frame.cols == 110)
        .controller
    );
}
#[test]
fn batched_input_then_rejected_layout_does_not_discard_input() {
    let daemon = Daemon::start();
    let (mut stream, _, _) = daemon.attach();
    let panes = command(&mut stream, Action::NewTab).unwrap().0;
    start_background(&mut stream, &panes[0]);
    write_message(
        &mut stream,
        &ClientMessage::Action(Action::Focus(panes[1].id)),
    )
    .unwrap();
    wait_process(panes[1].pid, |tree| {
        kelpmux::process::idle(tree, panes[1].pid, false)
    });
    let mut batch = vec![];
    write_message(&mut batch, &ClientMessage::Input(b"sleep 60\n".to_vec())).unwrap();
    write_message(
        &mut batch,
        &ClientMessage::Action(Action::Grid { cols: 1, rows: 1 }),
    )
    .unwrap();
    stream.write_all(&batch).unwrap();
    loop {
        if let ServerMessage::Error(error) = next(&mut stream) {
            assert!(error.contains("automatic"));
            break;
        }
    }
    assert_eq!(state(&mut stream).0.len(), 2);
    wait_process(panes[1].pid, |tree| {
        kelpmux::process::descendants(tree, panes[1].pid)
            .iter()
            .any(|pid| tree.get(pid).is_some_and(|p| p.command == "sleep"))
    });
}
#[test]
fn foreground_and_stopped_jobs_survive_rejected_manual_layout() {
    for stopped in [false, true] {
        let daemon = Daemon::start();
        let (mut stream, _, _) = daemon.attach();
        let mut ctl = daemon.connect();
        let panes = command(&mut ctl, Action::NewTab).unwrap().0;
        start_background(&mut stream, &panes[0]);
        write_message(
            &mut stream,
            &ClientMessage::Action(Action::Focus(panes[1].id)),
        )
        .unwrap();
        write_message(&mut stream, &ClientMessage::Input(b"sleep 60\n".to_vec())).unwrap();
        let shell = panes[1].pid;
        wait_process(shell, |tree| {
            tree.get(&shell)
                .is_some_and(|root| root.foreground != root.group)
                && kelpmux::process::descendants(tree, shell).len() > 1
        });
        if stopped {
            write_message(&mut stream, &ClientMessage::Input(vec![0x1a])).unwrap();
            wait_process(shell, |tree| {
                kelpmux::process::descendants(tree, shell)
                    .iter()
                    .any(|pid| *pid != shell && tree.get(pid).is_some_and(|p| p.state == 'T'))
            });
        }
        let before = kelpmux::process::snapshot(std::path::Path::new("/proc")).unwrap();
        let job = kelpmux::process::descendants(&before, shell)
            .into_iter()
            .find(|pid| *pid != shell && before[pid].command == "sleep")
            .unwrap();
        let start = before[&job].start;
        assert!(
            command(&mut ctl, Action::Grid { cols: 1, rows: 1 })
                .unwrap_err()
                .contains("automatic")
        );
        let live = kelpmux::process::read(std::path::Path::new("/proc"), job).unwrap();
        assert_eq!(live.start, start);
        if stopped {
            assert_eq!(live.state, 'T');
        }
        assert_eq!(state(&mut ctl).0.len(), 2);
    }
}

fn start_background(stream: &mut UnixStream, pane: &PaneInfo) {
    write_message(stream, &ClientMessage::Action(Action::Focus(pane.id))).unwrap();
    write_message(stream, &ClientMessage::Input(b"sleep 60 &\n".to_vec())).unwrap();
    wait_process(pane.pid, |tree| {
        kelpmux::process::descendants(tree, pane.pid)
            .iter()
            .any(|pid| {
                tree.get(pid)
                    .is_some_and(|process| process.command == "sleep")
            })
    });
}
#[test]
fn idle_shell_label_survives_prompt_titles_and_returns_after_application_exit() {
    let daemon = Daemon::start();
    let (mut stream, _, _) = daemon.attach();
    write_message(
        &mut stream,
        &ClientMessage::Input(
            b"printf '\\033]0;joan@jbox: ~/scratch\\007'; printf 'PROMPT_%s\\n' ready\n".to_vec(),
        ),
    )
    .unwrap();
    wait_frame(&mut stream, |frame| {
        let output: String = frame.cells.iter().map(|cell| cell.text.as_str()).collect();
        output.contains("PROMPT_ready")
            && frame.panes.iter().any(|view| {
                view.pane.status == kelpmux::model::Status::Idle && view.pane.title == "shell"
            })
    });
    write_message(
        &mut stream,
        &ClientMessage::Input(b"printf '\\033]0;Application title\\007'; sleep 30\n".to_vec()),
    )
    .unwrap();
    wait_frame(&mut stream, |frame| {
        frame
            .panes
            .iter()
            .any(|view| view.pane.title == "Application title")
    });
    write_message(&mut stream, &ClientMessage::Input(vec![3])).unwrap();
    wait_frame(&mut stream, |frame| {
        frame.panes.iter().any(|view| {
            view.pane.status == kelpmux::model::Status::Idle && view.pane.title == "shell"
        })
    });
}

#[test]
fn child_shell_has_native_color_environment_without_ssh_autoattach_markers() {
    let daemon = Daemon::start();
    let (mut stream, _, _) = daemon.attach();
    write_message(
        &mut stream,
        &ClientMessage::Input(
            b"printf 'ENV:%s:%s:%s:%s:%s:%s:%s:%s\\n' \"$TERM\" \"$COLORTERM\" \"${NO_COLOR-unset}\" \"${SSH_CONNECTION-unset}\" \"${SSH_CLIENT-unset}\" \"${SSH_TTY-unset}\" \"$KELPMUX\" \"$KELPMUX_PANE\"\n".to_vec(),
        ),
    )
    .unwrap();
    wait_frame(&mut stream, |frame| {
        frame
            .cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
            .contains("ENV:xterm-256color:truecolor:unset:unset:unset:unset:1:%1")
    });
}

#[test]
fn controller_theme_queries_survive_observers_takeover_and_new_panes() {
    fn probe(daemon: &Daemon, client: &mut UnixStream, index: u8) -> String {
        let script = format!(
            "stty -echo -icanon min 1 time 0; printf '\\033]11;?\\033\\\\'; reply=$(timeout --foreground 2 dd bs=1 count=25 2>/dev/null); printf %s \"$reply\" > theme-{index}; stty sane\n"
        );
        write_message(client, &ClientMessage::Input(script.into_bytes())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(text) = fs::read_to_string(daemon.root.join(format!("theme-{index}"))) {
                if text.len() == 25 {
                    return text;
                }
            }
            assert!(Instant::now() < deadline, "theme query did not finish");
            thread::sleep(Duration::from_millis(10));
        }
    }
    let daemon = Daemon::start();
    let (mut first, _, _) = daemon.attach();
    write_message(
        &mut first,
        &ClientMessage::ThemeColor {
            background: true,
            rgb: [20, 30, 40],
        },
    )
    .unwrap();
    assert!(probe(&daemon, &mut first, 0).contains("1414/1e1e/2828"));
    let (mut observer, _, controller) = daemon.attach();
    assert!(!controller);
    write_message(
        &mut observer,
        &ClientMessage::ThemeColor {
            background: true,
            rgb: [80, 90, 100],
        },
    )
    .unwrap();
    assert!(probe(&daemon, &mut first, 1).contains("1414/1e1e/2828"));
    write_message(&mut observer, &ClientMessage::Action(Action::Takeover)).unwrap();
    assert!(probe(&daemon, &mut observer, 2).contains("5050/5a5a/6464"));
    write_message(&mut observer, &ClientMessage::Action(Action::NewTab)).unwrap();
    assert_eq!(state(&mut observer).0.len(), 2);
    assert!(probe(&daemon, &mut observer, 3).contains("5050/5a5a/6464"));
}

#[test]
fn startup_color_and_keyboard_probe_replies_support_a_shaded_composer() {
    let daemon = Daemon::start();
    let (mut stream, _, _) = daemon.attach();
    let script = b"stty -echo -icanon min 1 time 0; printf '\\033]10;?\\033\\\\\\033]11;?\\033\\\\\\033[?u'; reply=$(timeout --foreground 2 dd bs=1 count=55 2>/dev/null); printf %s \"$reply\" > probe-reply; printf '\\033[2;1HPRO''BE_COLORS_OK\\033[8;1H\\033[48;2;31;31;31m\\033[2K SHADE \\033[0m'; stty sane\n";
    write_message(&mut stream, &ClientMessage::Input(script.to_vec())).unwrap();
    let frame = wait_frame(&mut stream, |f| {
        f.cells
            .iter()
            .map(|c| c.text.as_str())
            .collect::<String>()
            .contains("PROBE_COLORS_OK")
    });
    let body = frame.panes[0].body;
    let reply = fs::read_to_string(daemon.root.join("probe-reply")).unwrap();
    assert!(reply.contains("rgb:e5e5/e5e5/e5e5"), "{reply:?}");
    assert!(reply.contains("rgb:0000/0000/0000"), "{reply:?}");
    assert!(reply.contains("\x1b[?0u"), "{reply:?}");
    assert_eq!(
        frame.cells[usize::from((body.y + 7) * frame.cols + body.x + 30)].bg,
        Color::Rgb(31, 31, 31)
    );
    assert_eq!(
        frame.cells[usize::from(body.y * frame.cols + body.x)].bg,
        Color::Default
    );
}

#[test]
fn pane_screen_inspection_is_independent_of_tabs_and_does_not_take_control() {
    let daemon = Daemon::start();
    let (mut controller, owner, _) = daemon.attach();
    let panes = create_tabs(&mut controller, 2, 1);
    command(&mut controller, Action::Read(panes[0].id)).unwrap();
    let mut inspector = daemon.connect();
    write_message(
        &mut inspector,
        &ClientMessage::InspectPane { pane: panes[1].id },
    )
    .unwrap();
    match next(&mut inspector) {
        ServerMessage::PaneState { pane, screen } => {
            assert_eq!(pane.id, panes[1].id);
            assert!(screen.cols > 0 && screen.rows > 0);
            assert_eq!(
                screen.cells.len(),
                usize::from(screen.cols) * usize::from(screen.rows)
            );
            let text: String = screen.cells.iter().map(|cell| cell.text.as_str()).collect();
            assert!(!text.contains("Grid ·"));
        }
        other => panic!("unexpected pane response: {other:?}"),
    }
    assert_eq!(state(&mut inspector).1, Some(owner));
    write_message(
        &mut inspector,
        &ClientMessage::InspectPane { pane: u64::MAX },
    )
    .unwrap();
    assert!(matches!(next(&mut inspector), ServerMessage::Error(_)));
}

#[test]
fn close_tab_requires_matching_confirmation_and_preserves_other_processes() {
    let daemon = Daemon::start();
    let (mut stream, _, _) = daemon.attach();
    let panes = create_tabs(&mut stream, 2, 1);
    assert!(
        command(&mut stream, Action::ClosePaneConfirm(panes[0].id))
            .unwrap_err()
            .contains("confirmation")
    );
    assert_eq!(state(&mut stream).0.len(), 2);
    command(&mut stream, Action::Park(panes[0].id)).unwrap();
    command(&mut stream, Action::ClosePaneRequest(panes[0].id)).unwrap();
    assert!(
        state(&mut stream)
            .0
            .iter()
            .any(|pane| pane.id == panes[0].id && pane.parked)
    );
    assert!(command(&mut stream, Action::ClosePaneConfirm(panes[1].id)).is_err());
    let (remaining, _) = command(&mut stream, Action::ClosePaneConfirm(panes[0].id)).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].pid, panes[1].pid);
    command(&mut stream, Action::ClosePaneRequest(panes[1].id)).unwrap();
    let (replacement, _) = command(&mut stream, Action::ClosePaneConfirm(panes[1].id)).unwrap();
    assert_eq!(replacement.len(), 1);
    assert_ne!(replacement[0].id, panes[1].id);
}

#[test]
fn configuration_tab_position_persists_for_the_named_workspace() {
    let daemon = Daemon::start();
    let (mut stream, _, _) = daemon.attach();
    command(&mut stream, Action::Config).unwrap();
    command(
        &mut stream,
        Action::TabPositionSet(kelpmux::native::ui::TabPosition::Left),
    )
    .unwrap();
    let settings =
        kelpmux::native::lifecycle::Settings::load(&daemon.root.join("state/workspace.json"))
            .unwrap();
    assert_eq!(
        settings.ui.tab_position,
        kelpmux::native::ui::TabPosition::Left
    );
}

#[test]
fn new_tabs_create_one_pty_preserve_existing_jobs_and_stop_at_nine() {
    let daemon = Daemon::start();
    let mut admin = daemon.connect();
    let (initial, _) = command(&mut admin, Action::WrapToggle).unwrap();
    let original = (initial[0].id, initial[0].pid);
    let (colored, _) = command(
        &mut admin,
        Action::TabColorSet {
            pane: original.0,
            color: 4,
        },
    )
    .unwrap();
    assert_eq!(colored[0].palette, 4);
    assert_eq!((colored[0].id, colored[0].pid), original);
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(daemon.root.join("state/workspace.json")).unwrap())
            .unwrap();
    assert_eq!(settings["ui"]["wrap_lines"], false);
    assert_eq!(settings["ui"]["pane_colors"][original.0.to_string()], 4);
    command(&mut admin, Action::CloseReading).unwrap();
    for count in 2..=9 {
        let (panes, _) = command(&mut admin, Action::NewTab).unwrap();
        assert_eq!(panes.len(), count);
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(daemon.root.join("state/workspace.json")).unwrap())
                .unwrap();
        assert_eq!(saved["ui"]["view_modes"][1], true);
        assert_eq!((panes[0].id, panes[0].pid), original);
        assert_eq!(panes[0].palette, 4);
        for (index, pane) in panes.iter().enumerate() {
            assert_eq!(pane.number, index + 1);
            assert!(std::path::Path::new(&format!("/proc/{}", pane.pid)).exists());
        }
    }
    assert!(
        command(&mut admin, Action::NewTab)
            .unwrap_err()
            .contains("nine")
    );
    let (panes, _) = command(&mut admin, Action::Config).unwrap();
    assert_eq!(panes.len(), 9);
    assert_eq!((panes[0].id, panes[0].pid), original);
    command(&mut admin, Action::ClosePaneRequest(original.0)).unwrap();
    command(&mut admin, Action::ClosePaneConfirm(original.0)).unwrap();
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(daemon.root.join("state/workspace.json")).unwrap())
            .unwrap();
    assert!(
        settings["ui"]["pane_colors"]
            .get(original.0.to_string())
            .is_none()
    );
}

#[test]
fn controller_and_observer_surfaces_use_their_own_reported_themes() {
    let daemon = Daemon::start();
    let (mut controller, _, _) = daemon.attach();
    write_message(&mut controller, &ClientMessage::Action(Action::Read(1))).unwrap();
    wait_frame(&mut controller, |frame| {
        frame.reading == ReadingMode::Tabs(1)
    });
    let (mut observer, _, _) = daemon.attach();
    for (client, background, expected) in [
        (&mut controller, [12, 18, 24], Color::Rgb(26, 32, 38)),
        (&mut observer, [246, 244, 240], Color::Rgb(232, 230, 226)),
    ] {
        write_message(
            client,
            &ClientMessage::ThemeColor {
                background: true,
                rgb: background,
            },
        )
        .unwrap();
        let frame = wait_frame(client, |frame| {
            frame.ui.terminal_theme[1] == Some(background)
        });
        assert_eq!(frame.cells[usize::from(frame.cols - 1)].bg, expected);
    }
    write_message(&mut controller, &ClientMessage::Action(Action::NewTab)).unwrap();
    let frame = wait_frame(&mut controller, |frame| frame.panes.len() == 2);
    assert_eq!(frame.ui.terminal_theme[1], Some([12, 18, 24]));
    assert_eq!(
        frame.cells[usize::from(frame.cols - 1)].bg,
        Color::Rgb(26, 32, 38)
    );
}
