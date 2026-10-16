//! Real frontend processes on disposable PTYs and a same-user fixture socket.
use kelpmux::{model::Status, native::protocol::*};
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    socket: PathBuf,
    listener: UnixListener,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "km-client-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let socket = root.join("control.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        Self {
            root,
            socket,
            listener,
        }
    }
    fn accept(&self) -> UnixStream {
        let (socket, _) = self.listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
struct Pty {
    master: File,
    slave: File,
    child: Child,
    original: libc::termios,
    output: Vec<u8>,
}
impl Pty {
    fn spawn(path: &PathBuf, cols: u16, rows: u16) -> Self {
        let mut master = -1;
        let mut slave = -1;
        let size = libc::winsize {
            ws_col: cols,
            ws_row: rows,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    &size,
                )
            },
            0
        );
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        let mut original = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut original) },
            0
        );
        let mut command = Command::new(env!("CARGO_BIN_EXE_kelpmux"));
        command
            // Deliberately use this binary against a protocol fixture rather than
            // selecting the test harness executable as a retained Kelpmux client.
            .args(["attach", "--current-client", "--socket"])
            .arg(path)
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave.try_clone().unwrap()));
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
        assert_eq!(
            unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
        Self {
            master,
            slave,
            child,
            original,
            output: vec![],
        }
    }
    fn drain(&mut self) {
        let mut bytes = [0; 8192];
        loop {
            match self.master.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => self.output.extend_from_slice(&bytes[..n]),
                Err(_) => break,
            }
        }
    }
    fn ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.drain();
            if self.output.windows(4).any(|b| b == b"\x1b[2J") {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "frontend did not render: {}",
                String::from_utf8_lossy(&self.output)
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn input(&mut self, mut bytes: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !bytes.is_empty() {
            match self.master.write(bytes) {
                Ok(0) => panic!("frontend input closed"),
                Ok(count) => bytes = &bytes[count..],
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    self.drain();
                    assert!(Instant::now() < deadline, "frontend did not consume input");
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("frontend input failed: {error}"),
            }
        }
    }
    fn finish(&mut self, success: bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.drain();
            if let Some(status) = self.child.try_wait().unwrap() {
                assert_eq!(
                    status.success(),
                    success,
                    "{}",
                    String::from_utf8_lossy(&self.output)
                );
                break;
            }
            assert!(Instant::now() < deadline, "frontend did not exit");
            thread::sleep(Duration::from_millis(5));
        }
        self.drain();
        let mut termios = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::tcgetattr(self.slave.as_raw_fd(), &mut termios) },
            0
        );
        assert_eq!(termios.c_lflag, self.original.c_lflag);
        assert_eq!(termios.c_iflag, self.original.c_iflag);
        assert_eq!(termios.c_oflag, self.original.c_oflag);
        assert_eq!(termios.c_cc, self.original.c_cc);
    }
}
impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn frame(controller: bool) -> Frame {
    let pane = PaneInfo {
        id: 7,
        number: 1,
        pid: 1,
        cwd: "/fixture".into(),
        title: "fixture".into(),
        palette: 0,
        status: Status::Idle,
        session_id: None,
        dead: false,
        ..PaneInfo::default()
    };
    Frame {
        cols: 80,
        rows: 24,
        cells: vec![Cell::default(); 80 * 24],
        cursor: Some(Cursor {
            position: Point { x: 1, y: 1 },
            visible: true,
        }),
        panes: vec![PaneView {
            pane,
            rect: Rect {
                x: 0,
                y: 0,
                cols: 80,
                rows: 23,
            },
            body: Rect {
                x: 0,
                y: 1,
                cols: 80,
                rows: 22,
            },
            overlay: false,
            modes: InputModes {
                mouse_click: true,
                focus: true,
                ..InputModes::default()
            },
        }],
        active: 7,
        reading: ReadingMode::Grid,
        controller,
        help: false,
        notice: None,
        ui: Default::default(),
    }
}
fn welcome(socket: &mut UnixStream, controller: bool) {
    assert!(matches!(
        read_message::<ClientMessage>(socket).unwrap(),
        Some(ClientMessage::Attach {
            version: VERSION,
            ..
        })
    ));
    write_message(
        socket,
        &ServerMessage::Welcome {
            version: VERSION,
            client_id: 1,
            controller,
        },
    )
    .unwrap();
    // Fragmented framed JSON exercises incremental socket reception.
    let data = serde_json::to_vec(&ServerMessage::Frame(frame(controller))).unwrap();
    let header = (data.len() as u32).to_be_bytes();
    socket.write_all(&header[..2]).unwrap();
    socket.write_all(&header[2..]).unwrap();
    for chunk in data.chunks(307) {
        socket.write_all(chunk).unwrap();
    }
}
#[test]
fn attach_keyboard_paste_focus_mouse_resize_detach_reconnect_restores_tty() {
    let fixture = Fixture::new();
    let path = fixture.socket.clone();
    let (sender, receiver) = mpsc::channel();
    let backend = thread::spawn(move || {
        for _ in 0..2 {
            let mut socket = fixture.accept();
            welcome(&mut socket, true);
            write_message(
                &mut socket,
                &ServerMessage::Clipboard("fixture copy".into()),
            )
            .unwrap();
            while let Some(message) = read_message::<ClientMessage>(&mut socket).unwrap() {
                let detach = matches!(message, ClientMessage::Action(Action::Detach));
                sender.send(message).unwrap();
                if detach {
                    write_message(&mut socket, &ServerMessage::Detached).unwrap();
                    break;
                }
            }
        }
    });
    let mut tty = Pty::spawn(&path, 80, 24);
    tty.ready();
    tty.input(b"hello\x1b[200~a\x02d\x1b[201~\x1b[I\x1b[<0;3;3M");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut keyboard = false;
    let mut paste = false;
    let mut focus = false;
    let mut mouse = false;
    while !(keyboard && paste && focus && mouse) {
        assert!(Instant::now() < deadline);
        match receiver.recv_timeout(Duration::from_secs(1)).unwrap() {
            ClientMessage::Input(bytes) => {
                keyboard |= bytes.windows(5).any(|b| b == b"hello");
                focus |= bytes.windows(3).any(|b| b == b"\x1b[I");
            }
            ClientMessage::Paste(bytes) => {
                assert_eq!(bytes, b"a\x02d");
                paste = true;
            }
            ClientMessage::Mouse(event) => {
                assert_eq!(event.position, Point { x: 2, y: 2 });
                mouse = true;
            }
            _ => {}
        }
    }
    let size = libc::winsize {
        ws_col: 100,
        ws_row: 30,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe { libc::ioctl(tty.slave.as_raw_fd(), libc::TIOCSWINSZ, &size) },
        0
    );
    unsafe {
        libc::kill(tty.child.id() as i32, libc::SIGWINCH);
    }
    loop {
        if matches!(
            receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
            ClientMessage::Resize {
                cols: 100,
                rows: 30
            }
        ) {
            break;
        }
    }
    tty.input(b"\x02d");
    tty.finish(true);
    assert!(tty.output.windows(8).any(|b| b == b"\x1b[?1049l"));
    assert!(String::from_utf8_lossy(&tty.output).contains("Zml4dHVyZSBjb3B5"));
    let mut again = Pty::spawn(&path, 80, 24);
    again.ready();
    again.input(b"\x02d");
    again.finish(true);
    backend.join().unwrap();
}
#[test]
fn observer_resize_does_not_resize_backend_and_signal_restores_tty() {
    let fixture = Fixture::new();
    let path = fixture.socket.clone();
    let (sender, receiver) = mpsc::channel();
    let backend = thread::spawn(move || {
        let mut socket = fixture.accept();
        welcome(&mut socket, false);
        while let Ok(Some(message)) = read_message::<ClientMessage>(&mut socket) {
            sender.send(message).unwrap();
        }
    });
    let mut tty = Pty::spawn(&path, 40, 12);
    tty.ready();
    let size = libc::winsize {
        ws_col: 50,
        ws_row: 15,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        libc::ioctl(tty.slave.as_raw_fd(), libc::TIOCSWINSZ, &size);
        libc::kill(tty.child.id() as i32, libc::SIGWINCH);
    }
    tty.input(b"blocked\x1b[200~blocked\x1b[201~");
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
        ClientMessage::Resize { cols: 50, rows: 15 }
    ));
    assert!(receiver.recv_timeout(Duration::from_millis(150)).is_err());
    unsafe {
        libc::kill(tty.child.id() as i32, libc::SIGTERM);
    }
    tty.finish(true);
    backend.join().unwrap();
    assert!(String::from_utf8_lossy(&tty.output).contains("\x1b[?1049l"));
}
#[test]
fn explicit_current_client_still_rejects_protocol_mismatch_before_raw_mode() {
    let fixture = Fixture::new();
    let path = fixture.socket.clone();
    let backend = thread::spawn(move || {
        let mut socket = fixture.accept();
        assert!(matches!(
            read_message::<ClientMessage>(&mut socket).unwrap(),
            Some(ClientMessage::Attach {
                version: VERSION,
                ..
            })
        ));
        write_message(
            &mut socket,
            &ServerMessage::Welcome {
                version: VERSION + 1,
                client_id: 1,
                controller: true,
            },
        )
        .unwrap();
    });
    let mut tty = Pty::spawn(&path, 80, 24);
    tty.finish(false);
    let output = String::from_utf8_lossy(&tty.output);
    assert!(output.contains(&format!(
        "workspace uses native protocol {}, client uses {VERSION}",
        VERSION + 1
    )));
    assert!(!output.contains("\x1b[?1049h"));
    backend.join().unwrap();
}
#[test]
fn handshake_timeout_never_enters_raw_or_alternate_screen() {
    let fixture = Fixture::new();
    let path = fixture.socket.clone();
    let backend = thread::spawn(move || {
        let mut socket = fixture.accept();
        assert!(
            read_message::<ClientMessage>(&mut socket)
                .unwrap()
                .is_some()
        );
        thread::sleep(Duration::from_millis(3300));
    });
    let mut tty = Pty::spawn(&path, 80, 24);
    tty.finish(false);
    assert!(!String::from_utf8_lossy(&tty.output).contains("\x1b[?1049h"));
    backend.join().unwrap();
}
#[test]
fn management_uses_command_without_attach_and_validates_arguments() {
    let fixture = Fixture::new();
    let path = fixture.socket.clone();
    let backend = thread::spawn(move || {
        let mut socket = fixture.accept();
        assert!(matches!(
            read_message::<ClientMessage>(&mut socket).unwrap(),
            Some(ClientMessage::Command(Action::NewTab))
        ));
        write_message(
            &mut socket,
            &ServerMessage::State {
                panes: vec![],
                active: 7,
                reading: ReadingMode::Grid,
                controller: Some(99),
            },
        )
        .unwrap();
    });
    let result = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
        .args(["ctl", "--socket"])
        .arg(&path)
        .args(["new"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("99"));
    backend.join().unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
        .args(["ctl", "layout", "3x7"])
        .output()
        .unwrap();
    assert!(!result.status.success());
}

#[test]
fn early_input_is_buffered_action_error_stays_attached_and_eof_restores_tty() {
    let fixture = Fixture::new();
    let path = fixture.socket.clone();
    let (sent, received) = mpsc::channel();
    let backend = thread::spawn(move || {
        let mut socket = fixture.accept();
        assert!(matches!(
            read_message::<ClientMessage>(&mut socket).unwrap(),
            Some(ClientMessage::Attach { .. })
        ));
        write_message(
            &mut socket,
            &ServerMessage::Welcome {
                version: VERSION,
                client_id: 1,
                controller: true,
            },
        )
        .unwrap();
        thread::sleep(Duration::from_millis(150));
        write_message(&mut socket, &ServerMessage::Frame(frame(true))).unwrap();
        assert!(
            matches!(read_message::<ClientMessage>(&mut socket).unwrap(),Some(ClientMessage::Input(bytes)) if bytes==b"early")
        );
        write_message(
            &mut socket,
            &ServerMessage::Error("occupied grid refused".into()),
        )
        .unwrap();
        sent.send(()).unwrap();
        assert!(
            matches!(read_message::<ClientMessage>(&mut socket).unwrap(),Some(ClientMessage::Input(bytes)) if bytes==b"after")
        );
        // Ordinary backend EOF does not issue a shutdown command or own PTYs.
    });
    let mut tty = Pty::spawn(&path, 80, 24);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        tty.drain();
        if String::from_utf8_lossy(&tty.output).contains("\x1b[?1049h") {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    tty.input(b"early");
    received.recv_timeout(Duration::from_secs(3)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        tty.drain();
        let mut display = kelpmux::native::engine::Terminal::new(80, 24);
        display.feed(&tty.output);
        let screen = display.snapshot();
        let footer = screen.cells[23 * 80..]
            .iter()
            .map(|c| c.text.as_str())
            .collect::<String>();
        if footer.contains("occupied grid refused") {
            break;
        }
        assert!(Instant::now() < deadline, "action error was not displayed");
        thread::sleep(Duration::from_millis(5));
    }
    assert!(tty.child.try_wait().unwrap().is_none());
    tty.input(b"after");
    tty.finish(true);
    backend.join().unwrap();
}

#[test]
fn client_refuses_public_socket_and_symlink_before_connecting() {
    let fixture = Fixture::new();
    fs::set_permissions(&fixture.socket, fs::Permissions::from_mode(0o666)).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
        .args(["ctl", "--socket"])
        .arg(&fixture.socket)
        .arg("list")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("private"));
    fs::set_permissions(&fixture.socket, fs::Permissions::from_mode(0o600)).unwrap();
    let alias = fixture.root.join("alias.sock");
    std::os::unix::fs::symlink(&fixture.socket, &alias).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
        .args(["ctl", "--socket"])
        .arg(&alias)
        .arg("list")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("symlink"));
}
#[test]
fn malformed_frame_restores_tty_without_passing_application_escapes() {
    let fixture = Fixture::new();
    let path = fixture.socket.clone();
    let backend = thread::spawn(move || {
        let mut socket = fixture.accept();
        assert!(
            read_message::<ClientMessage>(&mut socket)
                .unwrap()
                .is_some()
        );
        write_message(
            &mut socket,
            &ServerMessage::Welcome {
                version: VERSION,
                client_id: 1,
                controller: true,
            },
        )
        .unwrap();
        let mut frame = frame(true);
        frame.cells[0].text = "\x1b]2;injected\x07".into();
        write_message(&mut socket, &ServerMessage::Frame(frame)).unwrap();
        thread::sleep(Duration::from_millis(100));
    });
    let mut tty = Pty::spawn(&path, 80, 24);
    tty.finish(false);
    assert!(!String::from_utf8_lossy(&tty.output).contains("injected"));
    backend.join().unwrap();
}

struct LiveServer {
    root: PathBuf,
    child: Child,
    socket: PathBuf,
}
impl LiveServer {
    fn start() -> Self {
        let root = std::env::temp_dir().join(format!(
            "km-live-client-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let socket = root.join("live.sock");
        let log = File::create(root.join("server.log")).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
            .args(["server", "--socket"])
            .arg(&socket)
            .arg("--state-dir")
            .arg(root.join("state"))
            .args(["--shell", "/bin/sh", "--cwd"])
            .arg(&root)
            .args(["--size", "80x24"])
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap();
        let mut server = Self {
            root,
            child,
            socket,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        // A socket path becomes visible between bind and listen; wait for an
        // accepted connection before launching the real attach frontend.
        while UnixStream::connect(&server.socket).is_err() {
            assert!(
                server.child.try_wait().unwrap().is_none(),
                "{}",
                fs::read_to_string(server.root.join("server.log")).unwrap()
            );
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        server
    }
    fn ctl(&self, args: &[&str]) -> ServerMessage {
        let result = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
            .args(["ctl", "--socket"])
            .arg(&self.socket)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice(&result.stdout).unwrap()
    }
    fn state(&self) -> (Vec<PaneInfo>, ReadingMode, Option<u64>) {
        match self.ctl(&["list"]) {
            ServerMessage::State {
                panes,
                reading,
                controller,
                ..
            } => (panes, reading, controller),
            _ => panic!("missing state"),
        }
    }
}
impl Drop for LiveServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[track_caller]
fn wait_state(
    server: &LiveServer,
    tty: &mut Pty,
    mut condition: impl FnMut(&[PaneInfo], ReadingMode, Option<u64>) -> bool,
) -> Vec<PaneInfo> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        tty.drain();
        let (panes, reading, controller) = server.state();
        if condition(&panes, reading, controller) {
            return panes;
        }
        if Instant::now() >= deadline {
            let mut display = kelpmux::native::engine::Terminal::new(80, 24);
            display.feed(&tty.output);
            let text: String = display
                .snapshot()
                .cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect();
            panic!("live state did not converge: {reading:?}, terminal: {text}");
        }
        assert!(
            Instant::now() < deadline,
            "live state did not converge: panes={} reading={reading:?} controller={controller:?}",
            panes.len()
        );
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn live_server_attach_layout_read_fullscreen_detach_reconnect_preserves_shell() {
    let mut server = LiveServer::start();
    let mut tty = Pty::spawn(&server.socket, 80, 24);
    tty.ready();
    let panes = wait_state(&server, &mut tty, |panes, _, controller| {
        panes.len() == 1 && controller.is_some()
    });
    let original = panes[0].clone();
    tty.input(b"printf 'LIVE_READY\\n'\r");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        tty.drain();
        let mut display = kelpmux::native::engine::Terminal::new(80, 24);
        display.feed(&tty.output);
        let text = display
            .snapshot()
            .cells
            .iter()
            .map(|c| c.text.as_str())
            .collect::<String>();
        if text.contains("LIVE_READY") {
            break;
        }
        assert!(Instant::now() < deadline, "live shell output missing");
        thread::sleep(Duration::from_millis(10));
    }
    tty.input(b"\x02\r");
    wait_state(&server, &mut tty, |panes, _, _| panes.len() == 2);
    tty.input(b"\x02g");
    wait_state(&server, &mut tty, |_, reading, _| {
        reading == ReadingMode::Grid
    });
    let read = header_button(&server, original.id, ReadingMode::Grid, 0);
    wait_header(
        &mut tty,
        usize::from(read.y) * 80 + usize::from(read.x),
        "↗",
    );
    tty.input(format!("\x1b[<0;{};{}M", read.x + 1, read.y + 1).as_bytes());
    wait_state(&server, &mut tty, |_, reading, _| {
        reading == ReadingMode::Tabs(original.id)
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        tty.drain();
        let mut display = kelpmux::native::engine::Terminal::new(80, 24);
        display.feed(&tty.output);
        if display.snapshot().cells[2].fg == Color::Indexed(226) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "selected tab did not reach frontend"
        );
        thread::sleep(Duration::from_millis(10));
    }
    // The footer exposes full width while tabs remain visible.
    let full = Point { x: 17, y: 23 };
    wait_header(
        &mut tty,
        usize::from(full.y) * 80 + usize::from(full.x),
        "⛶",
    );
    tty.input(format!("\x1b[<0;{};{}M", full.x + 1, full.y + 1).as_bytes());
    wait_state(&server, &mut tty, |_, reading, _| {
        reading == ReadingMode::Fullscreen(original.id)
    });
    // Exercise the real frontend footer translation, not just router helpers.
    tty.input(b"\x1b[<0;2;24M");
    wait_text(&mut tty, "Tab placement");
    let bounds = kelpmux::native::workspace::dialog_bounds_for(
        80,
        24,
        kelpmux::native::ui::DialogKind::Config,
    );
    let choice = kelpmux::native::workspace::config_targets(bounds)
        .into_iter()
        .find(|(index, _, _)| *index == 2)
        .unwrap()
        .2;
    tty.input(format!("\x1b[<0;{};{}M", choice.x + 1, choice.y + 1).as_bytes());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        tty.drain();
        if kelpmux::native::lifecycle::Settings::load(&server.root.join("state/workspace.json"))
            .is_ok_and(|settings| {
                settings.ui.tab_position == kelpmux::native::ui::TabPosition::Left
            })
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "click did not save tab placement"
        );
        thread::sleep(Duration::from_millis(10));
    }
    tty.input(b"\x1b[<0;1;1M");
    tty.drain();
    server.ctl(&["close"]);
    wait_state(&server, &mut tty, |_, reading, _| {
        reading == ReadingMode::Grid
    });
    tty.input(b"\x02d");
    tty.finish(true);
    let panes = wait_state(&server, &mut tty, |_, _, controller| controller.is_none());
    assert_eq!(panes[0].id, original.id);
    assert_eq!(panes[0].pid, original.pid);
    let mut reconnect = Pty::spawn(&server.socket, 80, 24);
    reconnect.ready();
    let panes = wait_state(&server, &mut reconnect, |_, _, controller| {
        controller.is_some()
    });
    assert_eq!(panes[0].pid, original.pid);
    reconnect.input(b"printf 'LIVE_RECONNECTED\\n'\r");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        reconnect.drain();
        let mut display = kelpmux::native::engine::Terminal::new(80, 24);
        display.feed(&reconnect.output);
        let text = display
            .snapshot()
            .cells
            .iter()
            .map(|c| c.text.as_str())
            .collect::<String>();
        if text.contains("LIVE_RECONNECTED") {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    reconnect.input(b"\x02d");
    reconnect.finish(true);
    server.ctl(&["shutdown"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = server.child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
}
fn wait_header(tty: &mut Pty, index: usize, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        tty.drain();
        let mut display = kelpmux::native::engine::Terminal::new(80, 24);
        display.feed(&tty.output);
        if display.snapshot().cells[index].text == label {
            return;
        }
        assert!(Instant::now() < deadline, "header did not converge");
        thread::sleep(Duration::from_millis(10));
    }
}
fn wait_text(tty: &mut Pty, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        tty.drain();
        let mut display = kelpmux::native::engine::Terminal::new(80, 24);
        display.feed(&tty.output);
        let text: String = display
            .snapshot()
            .cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect();
        if text.contains(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "frontend text missing: {expected}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
fn header_button(server: &LiveServer, id: PaneId, reading: ReadingMode, slot: u16) -> Point {
    let (panes, _, _) = server.state();
    let mut workspace = kelpmux::native::workspace::Workspace::new(80, 24).unwrap();
    workspace.panes = panes;
    workspace.active = id;
    workspace.grid_cols = 2;
    workspace.reading = reading;
    let views = workspace.layout();
    let view = views
        .iter()
        .rev()
        .find(|view| view.pane.id == id && view.rect.cols > 0)
        .unwrap();
    let buttons = 2;
    Point {
        x: view.rect.x + view.rect.cols - buttons * 3 + slot * 3 + 1,
        y: view.rect.y,
    }
}

#[test]
fn termination_with_backpressured_display_restores_raw_mode_promptly() {
    let fixture = Fixture::new();
    let path = fixture.socket.clone();
    let (sender, receiver) = mpsc::channel();
    let backend = thread::spawn(move || {
        let mut socket = fixture.accept();
        assert!(
            read_message::<ClientMessage>(&mut socket)
                .unwrap()
                .is_some()
        );
        write_message(
            &mut socket,
            &ServerMessage::Welcome {
                version: VERSION,
                client_id: 1,
                controller: true,
            },
        )
        .unwrap();
        let mut large = frame(true);
        large.cols = 400;
        large.rows = 160;
        large.cells = vec![Cell::default(); 400 * 160];
        write_message(&mut socket, &ServerMessage::Frame(large)).unwrap();
        sender.send(()).unwrap();
        while read_message::<ClientMessage>(&mut socket)
            .ok()
            .flatten()
            .is_some()
        {}
    });
    let mut tty = Pty::spawn(&path, 400, 160);
    receiver.recv_timeout(Duration::from_secs(3)).unwrap();
    thread::sleep(Duration::from_millis(100));
    // Do not read the master until the child exits: its large paint is blocked.
    unsafe {
        libc::kill(tty.child.id() as i32, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = tty.child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "attach ignored termination while stdout was blocked"
        );
        thread::sleep(Duration::from_millis(10));
    }
    tty.finish(true);
    backend.join().unwrap();
}

#[test]
fn maximum_literal_paste_fits_the_wire_queue_and_keeps_client_attached() {
    let fixture = Fixture::new();
    let path = fixture.socket.clone();
    let (sent, received) = mpsc::channel();
    let backend = thread::spawn(move || {
        let mut socket = fixture.accept();
        welcome(&mut socket, true);
        let message = read_message::<ClientMessage>(&mut socket).unwrap().unwrap();
        assert!(
            matches!(message, ClientMessage::Paste(ref bytes) if bytes.len() == MAX_INPUT && bytes.iter().all(|byte| *byte == b'x'))
        );
        sent.send(()).unwrap();
        assert!(matches!(
            read_message::<ClientMessage>(&mut socket).unwrap(),
            Some(ClientMessage::Action(Action::Detach))
        ));
        write_message(&mut socket, &ServerMessage::Detached).unwrap();
    });
    let mut tty = Pty::spawn(&path, 80, 24);
    tty.ready();
    let mut paste = b"\x1b[200~".to_vec();
    paste.extend(std::iter::repeat_n(b'x', MAX_INPUT));
    paste.extend_from_slice(b"\x1b[201~");
    tty.input(&paste);
    received.recv_timeout(Duration::from_secs(5)).unwrap();
    tty.input(b"\x02d");
    tty.finish(true);
    backend.join().unwrap();
}
