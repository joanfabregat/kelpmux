//! The stream TUI's controller against a real, disposable KelpServer
//! (`target/<profile>/kelpserver`, built with `cargo build -p kelpserver
//! --bin kelpserver`; `kelpmux server` serves the same code when it is
//! absent): tabs, parking, closing, scratch and persisted UI state across
//! reattach and server restarts.
use kelpmux::native::{
    connection::connect,
    protocol::*,
    stream::{self, Event, Request},
    tui::{
        controller::{Controller, Options},
        state::{self, Store},
    },
};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn server_binary() -> PathBuf {
    let kelpmux = PathBuf::from(env!("CARGO_BIN_EXE_kelpmux"));
    let kelpserver = kelpmux.with_file_name("kelpserver");
    if kelpserver.exists() {
        kelpserver
    } else {
        kelpmux
    }
}

struct Server {
    root: PathBuf,
    child: Option<Child>,
}
impl Server {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "kelpmux-stream-tui-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let mut server = Self { root, child: None };
        server.start();
        server
    }
    fn state(&self) -> PathBuf {
        self.root.join("state")
    }
    fn socket(&self) -> PathBuf {
        self.root.join("state/control.sock")
    }
    fn start(&mut self) {
        let child = Command::new(server_binary())
            .args(["server", "--state-dir"])
            .arg(self.state())
            .arg("--socket")
            .arg(self.socket())
            .args(["--shell", "/bin/sh", "--cwd"])
            .arg(&self.root)
            .env("SHELL", "/bin/sh")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        self.child = Some(child);
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(self.socket()).is_err() {
            assert!(
                self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "server exited"
            );
            assert!(Instant::now() < deadline, "server did not start");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn stop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if let Ok(mut socket) = UnixStream::connect(self.socket()) {
            let _ = write_message(&mut socket, &ClientMessage::Command(Action::Shutdown));
        }
        let until = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > until {
                let _ = child.kill();
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = child.wait();
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A controller attached over a real stream connection.
struct Tui {
    socket: UnixStream,
    buffer: Vec<u8>,
    c: Controller,
}
impl Tui {
    fn attach(server: &Server) -> Self {
        let mut socket = connect(&server.socket()).unwrap();
        write_message(
            &mut socket,
            &ClientMessage::Stream(Request::Hello {
                version: stream::VERSION,
            }),
        )
        .unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let store = Store::open(&server.state()).unwrap();
        let mut c = Controller::new(
            160,
            48,
            store,
            Options {
                workspace: "test".into(),
                ..Options::default()
            },
        )
        .unwrap();
        let mut tui = Self {
            socket,
            buffer: Vec::new(),
            c: {
                c.dirty = false;
                c
            },
        };
        let welcome = tui.next(Duration::from_secs(5)).expect("welcome");
        match welcome {
            ServerMessage::Stream(Event::Welcome {
                client,
                epoch,
                capabilities,
                server_version,
                ..
            }) => tui
                .c
                .welcome(client, epoch, capabilities, server_version)
                .unwrap(),
            other => panic!("unexpected {other:?}"),
        }
        tui.until("first update", |c| c.synced());
        tui
    }
    fn next(&mut self, wait: Duration) -> Option<ServerMessage> {
        let deadline = Instant::now() + wait;
        loop {
            if self.buffer.len() >= 4 {
                let size = u32::from_be_bytes(self.buffer[..4].try_into().unwrap()) as usize;
                if self.buffer.len() >= 4 + size {
                    let bytes: Vec<u8> = self.buffer.drain(..4 + size).skip(4).collect();
                    return Some(serde_json::from_slice(&bytes).unwrap());
                }
            }
            let mut chunk = [0; 65536];
            match self.socket.read(&mut chunk) {
                Ok(0) => return None,
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    if Instant::now() >= deadline {
                        return None;
                    }
                }
                Err(e) => panic!("{e}"),
            }
        }
    }
    /// Sends pending requests and applies events until `done` holds.
    fn until(&mut self, what: &str, done: impl Fn(&Controller) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            for request in self.c.take_requests() {
                write_message(&mut self.socket, &ClientMessage::Stream(request)).unwrap();
            }
            self.socket.flush().unwrap();
            if done(&self.c) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            if let Some(ServerMessage::Stream(event)) = self.next(Duration::from_millis(50)) {
                self.c.event(event).unwrap();
            }
        }
    }
    fn act(&mut self, action: Action) {
        self.c.message(ClientMessage::Action(action));
        assert_eq!(self.c.notice, None);
    }
    fn tabs(&self) -> Vec<PaneId> {
        let scratch = self.c.workspace.ui.scratch;
        self.c
            .workspace
            .panes
            .iter()
            .filter(|p| Some(p.id) != scratch)
            .map(|p| p.id)
            .collect()
    }
    fn screen_text(&self, pane: PaneId) -> String {
        self.c
            .screens
            .get(&pane)
            .map(|s| s.cells.iter().map(|c| c.text.as_str()).collect())
            .unwrap_or_default()
    }
}

#[test]
fn tabs_scratch_and_input_over_a_real_server() {
    let server = Server::new();
    let mut tui = Tui::attach(&server);
    let first = tui.tabs();
    assert!(!first.is_empty(), "the server starts with a shell");
    let one = first[0];
    // Typing claims and sizes the pane; its screen arrives over the stream.
    tui.c
        .message(ClientMessage::Input(b"echo stream-$((40+2))\r".to_vec()));
    tui.until("echoed output", |c| {
        c.screens.get(&one).is_some_and(|s| {
            s.cells
                .iter()
                .map(|c| c.text.as_str())
                .collect::<String>()
                .contains("stream-42")
        })
    });
    assert!(tui.screen_text(one).contains("stream-42"));
    assert_eq!(tui.c.leases().get(&one), Some(&tui.c.client));
    assert!(tui.c.frame().controller);

    // New tab: spawned by the server, ordered and selected by the client.
    tui.act(Action::NewTab);
    tui.until("new tab", |c| c.workspace.panes.len() == first.len() + 1);
    let two = *tui.tabs().last().unwrap();
    tui.until("new tab focus", |c| c.workspace.active == two);
    tui.act(Action::NewTab);
    tui.until("third tab", |c| c.workspace.panes.len() == first.len() + 2);
    let three = *tui.tabs().last().unwrap();

    // Park the middle tab and restore it: it goes last.
    tui.act(Action::Park(two));
    assert!(
        tui.c
            .workspace
            .panes
            .iter()
            .any(|p| p.id == two && p.parked)
    );
    tui.act(Action::Restore(two));
    assert_eq!(tui.tabs().last(), Some(&two));
    assert_eq!(tui.c.workspace.active, two);

    // Closing the current tab returns to the previously selected one.
    tui.act(Action::Focus(three));
    tui.act(Action::Focus(two));
    tui.act(Action::ClosePaneRequest(two));
    tui.act(Action::ClosePaneConfirm(two));
    tui.until("closed tab", |c| {
        !c.workspace.panes.iter().any(|p| p.id == two)
    });
    assert_eq!(tui.c.workspace.active, three);

    // Scratch is a server pane the client marks and hides from the tabs.
    tui.act(Action::Scratch);
    tui.until("scratch", |c| c.workspace.ui.scratch.is_some());
    assert!(tui.c.workspace.ui.scratch_visible);
    let scratch = tui.c.workspace.ui.scratch.unwrap();
    assert!(!tui.tabs().contains(&scratch));
    tui.act(Action::Scratch);
    assert!(!tui.c.workspace.ui.scratch_visible);

    // Closed sessions come from the server's record.
    tui.act(Action::Recovery);
    tui.until("closed sessions", |c| !c.workspace.ui.recovery_loading);
    // Whether the just-closed shell is listed depends on the server having
    // observed it before the close; the request itself must complete.
    assert!(tui.c.notice.is_none());

    // Persisted, private UI state.
    tui.c.store.flush().unwrap();
    let path = server.state().join(state::FILE);
    let mode = fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let document = state::load(&path).unwrap();
    let epoch = &document.epochs[&tui.c.epoch];
    assert_eq!(epoch.order, tui.tabs());
    assert!(epoch.panes[&scratch].scratch);
    assert!(document.migrated);
    assert_eq!(document.layout.len(), tui.tabs().len());

    // A second client, same server run: same order, same scratch.
    let order = tui.tabs();
    drop(tui);
    let tui = Tui::attach(&server);
    assert_eq!(tui.tabs(), order);
    assert_eq!(tui.c.workspace.ui.scratch, Some(scratch));
}

#[test]
fn layout_template_survives_a_server_restart() {
    let mut server = Server::new();
    let mut tui = Tui::attach(&server);
    tui.act(Action::NewTab);
    tui.until("second tab", |c| c.workspace.panes.len() == 2);
    let tabs = tui.tabs();
    tui.act(Action::Park(tabs[0]));
    tui.c.store.flush().unwrap();
    let old_epoch = tui.c.epoch.clone();
    drop(tui);
    server.stop();
    server.start();
    let mut tui = Tui::attach(&server);
    assert_ne!(tui.c.epoch, old_epoch);
    // Respawned (or adopted) tabs follow the template: two tabs, the first
    // parked.
    tui.until("template tabs", |c| c.workspace.panes.len() == 2);
    let tabs = tui.tabs();
    assert!(
        tui.c
            .workspace
            .panes
            .iter()
            .any(|p| p.id == tabs[0] && p.parked)
    );
    tui.c.store.flush().unwrap();
    let document = state::load(&server.state().join(state::FILE)).unwrap();
    assert!(!document.epochs.contains_key(&old_epoch));
    assert_eq!(document.layout.len(), 2);
    assert!(document.layout[0].parked);
}

#[test]
fn legacy_tab_layout_is_migrated_once() {
    let server = Server::new();
    let directory = server.state();
    // The composed server persisted its layout on start.
    let layout = state::legacy_layout(&directory);
    let store = kelpmux::native::tui::attach::open_store(&directory).unwrap();
    assert_eq!(store.document.layout.len(), layout.len());
    assert!(!path_exists(&directory.join(state::FILE)) || store.document.migrated);
}

fn path_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}
