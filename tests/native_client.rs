//! Real `kelpmux attach` and `ctl` processes on disposable PTYs and a
//! same-user fixture socket: the TUI's handshake never leaves the terminal in
//! raw or alternate-screen mode, and management commands validate their
//! arguments. The TUI itself is tested in `native_stream_tui.rs`.
// The fixture imitates KelpServer: `ctl` speaks its composed
// administration, the TUI its stream.
use kelpserver::native::{
    protocol::*,
    stream::{Event, Request},
};
use std::{
    fs::{self, File},
    io::Read,
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
    sync::atomic::{AtomicU64, Ordering},
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
/// A server that answers the stream Hello with `welcome`.
fn answer_hello(fixture: Fixture, welcome: Event) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut socket = fixture.accept();
        assert!(matches!(
            read_message::<ClientMessage>(&mut socket).unwrap(),
            Some(ClientMessage::Stream(Request::Hello { version: 1 }))
        ));
        write_message(&mut socket, &ServerMessage::Stream(welcome)).unwrap();
        // Keep the socket open until the client gives up.
        let _ = read_message::<ClientMessage>(&mut socket);
    })
}
#[test]
fn a_server_without_the_session_api_is_refused_before_raw_mode() {
    let fixture = Fixture::new();
    let path = fixture.socket.clone();
    let backend = answer_hello(
        fixture,
        Event::Welcome {
            version: 1,
            client: 1,
            epoch: "e".into(),
            capabilities: vec!["pane-snapshots".into()],
            server_version: None,
            startup_panes: None,
            recovery_pending: false,
            limits: None,
            notification_run: None,
        },
    );
    let mut tty = Pty::spawn(&path, 80, 24);
    tty.finish(false);
    let output = String::from_utf8_lossy(&tty.output);
    assert!(output.contains("lacks the TUI's session API"), "{output}");
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
fn legacy_attach_is_refused_with_an_explanation() {
    let result = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
        .args(["attach", "--legacy"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("composed TUI was removed"));
}
