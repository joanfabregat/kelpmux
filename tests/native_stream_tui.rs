//! The stream TUI's controller against a real, disposable KelpServer
//! (`target/<profile>/kelpserver`, built with `cargo build -p kelpserver
//! --bin kelpserver`; `kelpmux server` serves the same code when it is
//! absent): tabs, parking, closing, scratch and persisted UI state across
//! reattach and server restarts, the startup policy, recovery after a crash,
//! spawning with another shell and the server-advertised limits.
use kelpmux::native::{
    connection::connect,
    protocol::*,
    stream::{self, Event, Request},
    tui::{
        attach::Records,
        controller::{Controller, Options},
        state::{self, LayoutTab},
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

/// A KelpServer 0.3, which starts without panes: the TUI creates them.
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
            for request in [
                Request::Hello {
                    version: stream::VERSION,
                },
                Request::Shutdown {
                    request: "stop".into(),
                },
            ] {
                let _ = write_message(&mut socket, &ClientMessage::Stream(request));
            }
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
impl Server {
    /// Ends the server without an orderly shutdown, as a crash would.
    fn crash(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
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
    records: Records,
    /// Binary screen records received (`binary-screens`).
    binary: usize,
}
impl Tui {
    fn attach(server: &Server) -> Self {
        Self::attach_with(
            server,
            Options {
                workspace: "test".into(),
                default_shell: Some("/bin/sh".into()),
                ..Options::default()
            },
        )
    }
    fn attach_with(server: &Server, options: Options) -> Self {
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
        // As `kelpmux attach` does: migrates the composed UI's files once.
        let store = kelpmux::native::tui::attach::open_store(&server.state()).unwrap();
        let mut c = Controller::new(160, 48, store, options).unwrap();
        let mut tui = Self {
            socket,
            buffer: Vec::new(),
            records: Records::default(),
            binary: 0,
            c: {
                c.dirty = false;
                c
            },
        };
        let welcome = tui.next(Duration::from_secs(5)).expect("welcome");
        match welcome {
            ServerMessage::Stream(welcome @ Event::Welcome { .. }) => {
                tui.c.welcome(welcome).unwrap()
            }
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
                    // As `kelpmux attach` reads: binary screen records
                    // complete the update before them.
                    let record = if kelpclient_protocol::screens::is_binary_record(&bytes) {
                        self.binary += 1;
                        ServerRecord::Screens(bytes)
                    } else {
                        ServerRecord::Message(serde_json::from_slice(&bytes).unwrap())
                    };
                    match self
                        .records
                        .accept(record, self.c.limits.subscribed_cells)
                        .unwrap()
                    {
                        Some(message) => return Some(message),
                        None => continue,
                    }
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
    /// The one shell the TUI creates in a server without panes.
    fn first_shell(&mut self) -> PaneId {
        self.until("the first shell", |c| c.workspace.panes.len() == 1);
        let first = self.tabs()[0];
        self.until("focus", |c| c.workspace.active == first);
        first
    }
    #[track_caller]
    fn act(&mut self, action: Action) {
        self.c.message(UiMessage::Action(action));
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
    let one = tui.first_shell();
    let first = tui.tabs();
    // Typing claims and sizes the pane; its screen arrives over the stream.
    tui.c
        .message(UiMessage::Input(b"echo stream-$((40+2))\r".to_vec()));
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
    // The server offers `binary-screens`, so the screens arrived as binary
    // records, decoded as `kelpmux attach` decodes them.
    assert!(tui.binary > 0);
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
    tui.first_shell();
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
    // The new run starts empty; the TUI recreates the template's tabs, the
    // first parked.
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

/// The tab layout a KelpServer 0.2 composed UI saved (`tab-layout.json`)
/// becomes the TUI's layout once: the first attach creates its tabs.
#[test]
fn legacy_tab_layout_is_migrated_once() {
    let server = Server::new();
    let directory = server.state();
    let cwd = server.root.to_string_lossy().into_owned();
    let legacy = serde_json::json!({
        "template": null,
        "cwd": cwd,
        "shell": "/bin/sh",
        "panes": [
            {"name": "Build", "cwd": cwd, "shell": "/bin/sh"},
            {"name": "Logs", "cwd": cwd, "shell": "/bin/sh"},
        ],
    });
    kelpmux::journal::atomic(&directory.join("tab-layout.json"), &legacy).unwrap();
    assert_eq!(state::legacy_layout(&directory).len(), 2);
    let mut tui = Tui::attach(&server);
    tui.until("migrated tabs", |c| {
        c.workspace
            .panes
            .iter()
            .map(|p| p.title.as_str())
            .eq(["Build", "Logs"])
    });
    assert!(tui.c.store.document.migrated);
    tui.c.store.flush().unwrap();
    drop(tui);
    // Migrated once: the legacy file is no longer needed.
    fs::remove_file(directory.join("tab-layout.json")).unwrap();
    let store = kelpmux::native::tui::attach::open_store(&directory).unwrap();
    assert!(store.document.migrated);
    assert_eq!(store.document.layout.len(), 2);
    assert!(path_exists(&directory.join(state::FILE)));
}

#[test]
fn two_tuis_share_arrangement_changes_live() {
    let server = Server::new();
    let mut a = Tui::attach(&server);
    a.first_shell();
    a.act(Action::NewTab);
    a.until("second tab", |c| c.workspace.panes.len() == 2);
    a.c.exchange_state().unwrap();
    let mut b = Tui::attach(&server);
    let tabs = a.tabs();
    assert_eq!(b.tabs(), tabs);
    b.c.exchange_state().unwrap();
    // A reorders and parks; B picks it up on its next exchange, attached.
    a.act(Action::Reorder {
        source: tabs[1],
        before: Some(tabs[0]),
    });
    a.act(Action::Park(tabs[0]));
    a.until("requests sent", |_| true);
    a.c.exchange_state().unwrap();
    b.until("requests sent", |_| true);
    b.c.exchange_state().unwrap();
    assert_eq!(b.tabs(), vec![tabs[1], tabs[0]]);
    assert!(
        b.c.workspace
            .panes
            .iter()
            .any(|p| p.id == tabs[0] && p.parked)
    );
}

fn path_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// A server starts without panes (KelpServer 0.3) and gets its tabs from
/// the TUI: one shell without a layout, then the saved layout after a
/// restart.
#[test]
fn the_tui_creates_one_shell_then_its_saved_layout() {
    let mut server = Server::new();
    let mut tui = Tui::attach(&server);
    assert_eq!(tui.c.startup_panes, Some(stream::StartupPanes::None));
    tui.first_shell();
    tui.act(Action::NewTab);
    tui.until("second tab", |c| c.workspace.panes.len() == 2);
    tui.act(Action::Park(tui.tabs()[0]));
    tui.c.store.flush().unwrap();
    drop(tui);
    server.stop();
    server.start();
    let mut tui = Tui::attach(&server);
    tui.until("saved tabs", |c| c.workspace.panes.len() == 2);
    // No startup shell was adopted: exactly the saved layout, first parked.
    let tabs = tui.tabs();
    assert_eq!(tabs.len(), 2);
    assert!(
        tui.c
            .workspace
            .panes
            .iter()
            .any(|p| p.id == tabs[0] && p.parked)
    );
}

/// Template tabs that name a shell are created with it (`spawn-shell`); a
/// shell that no longer works falls back to the workspace's shell.
#[test]
fn template_tabs_spawn_their_shell_and_fall_back_without_it() {
    use std::os::unix::fs::PermissionsExt;
    let server = Server::new();
    let custom = server.root.join("custom-shell");
    fs::write(&custom, "#!/bin/sh\nexec /bin/sh \"$@\"\n").unwrap();
    fs::set_permissions(&custom, fs::Permissions::from_mode(0o700)).unwrap();
    let custom = custom.to_string_lossy().into_owned();
    let cwd = server.root.to_string_lossy().into_owned();
    let tab = |name: &str, shell: &str| LayoutTab {
        cwd: cwd.clone(),
        shell: Some(shell.into()),
        name: Some(name.into()),
        ..LayoutTab::default()
    };
    let mut tui = Tui::attach_with(
        &server,
        Options {
            workspace: "test".into(),
            default_shell: Some("/bin/sh".into()),
            template: vec![
                tab("custom", &custom),
                tab("gone", "/nonexistent/kelpmux-shell"),
            ],
            ..Options::default()
        },
    );
    tui.until("named template tabs", |c| {
        c.workspace
            .panes
            .iter()
            .map(|p| p.title.as_str())
            .eq(["custom", "gone"])
    });
    let notice = tui.c.notice.clone().unwrap_or_default();
    assert!(notice.contains("Tab 2"), "{notice}");
    // The saved layout keeps the custom shell and drops the failed one.
    tui.until("layout", |c| {
        c.store.document.layout.len() == 2 && c.store.document.layout[0].shell.is_some()
    });
    assert_eq!(tui.c.store.document.layout[0].shell, Some(custom));
    assert_eq!(tui.c.store.document.layout[1].shell, None);
}

/// After an unclean shutdown the controlling TUI opens the Closed dialog
/// once and clears the signal for every client.
#[test]
fn recovery_after_a_crash_opens_closed_once() {
    let mut server = Server::new();
    let tui = Tui::attach(&server);
    drop(tui);
    server.crash();
    server.start();
    let mut tui = Tui::attach(&server);
    assert_eq!(
        tui.c.workspace.ui.dialog.map(|d| d.kind),
        Some(kelpmux::native::ui::DialogKind::Recovery)
    );
    tui.until("closed sessions", |c| !c.workspace.ui.recovery_loading);
    drop(tui);
    let tui = Tui::attach(&server);
    assert_eq!(
        tui.c.workspace.ui.dialog, None,
        "dismissed for every client"
    );
}

/// Welcome advertises the subscription limits and the notification run;
/// the server observes unused shells.
#[test]
fn limits_notification_run_and_unused_shells_come_from_the_server() {
    let server = Server::new();
    let mut tui = Tui::attach(&server);
    assert_eq!(tui.c.limits.subscriptions, 64);
    assert_eq!(tui.c.limits.subscribed_cells, 100_000);
    assert!(!tui.c.workspace.ui.notification_run.is_empty());
    let first = tui.first_shell();
    tui.until("unused shell", |c| c.workspace.ui.unused.contains(&first));
    tui.c.message(UiMessage::Input(b"sleep 30\r".to_vec()));
    tui.until("shell in use", |c| !c.workspace.ui.unused.contains(&first));
}
