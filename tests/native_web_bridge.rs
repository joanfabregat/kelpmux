//! `kelpmux web-bridge` over real pipes against a disposable server: stream
//! negotiation, terminal-only `Web` operations and request ordering.
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

struct Server {
    root: PathBuf,
    child: Child,
}
impl Server {
    fn start() -> Self {
        Self::start_with(|_| {})
    }
    /// Starts a server after `prepare` has seeded its state directory.
    fn start_with(prepare: impl FnOnce(&std::path::Path)) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "kelpmux-web-bridge-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        prepare(&root);
        let child = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
            .args(["server", "--state-dir"])
            .arg(root.join("state"))
            .arg("--socket")
            .arg(root.join("state/control.sock"))
            .args(["--shell", "/bin/sh", "--cwd"])
            .arg(&root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut server = Self { root, child };
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(server.socket()).is_err() {
            assert!(server.child.try_wait().unwrap().is_none(), "server exited");
            assert!(Instant::now() < deadline, "server did not start");
            thread::sleep(Duration::from_millis(10));
        }
        server
    }
    fn socket(&self) -> PathBuf {
        self.root.join("state/control.sock")
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Ok(mut socket) = UnixStream::connect(self.socket()) {
            shutdown(&mut socket);
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

/// Stops the server through the stream session API (`shutdown`).
fn shutdown(socket: &mut UnixStream) {
    for record in [
        json!({"Stream": {"type": "hello", "version": 1}}),
        json!({"Stream": {"type": "shutdown", "request": "test-cleanup"}}),
    ] {
        let bytes = serde_json::to_vec(&record).unwrap();
        let _ = socket.write_all(&(bytes.len() as u32).to_be_bytes());
        let _ = socket.write_all(&bytes);
    }
}

struct Bridge {
    child: Child,
    input: ChildStdin,
    events: mpsc::Receiver<Value>,
}
impl Bridge {
    fn start(server: &Server) -> Self {
        Self::at(&server.socket(), &server.root)
    }
    /// A bridge for this socket, run from `cwd` with kelpmux as its server
    /// binary (`kelpmux server` serves like `kelpserver`).
    fn at(socket: &std::path::Path, cwd: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
            .current_dir(cwd)
            .env("KELPSERVER_BINARY", env!("CARGO_BIN_EXE_kelpmux"))
            .env("SHELL", "/bin/sh")
            .arg("web-bridge")
            .arg("--socket")
            .arg(socket)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (sender, events) = mpsc::channel();
        thread::spawn(move || read_records(output, sender));
        Self {
            child,
            input,
            events,
        }
    }
    fn send(&mut self, value: Value) {
        let bytes = serde_json::to_vec(&value).unwrap();
        self.input
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .unwrap();
        self.input.write_all(&bytes).unwrap();
        self.input.flush().unwrap();
    }
    /// The next record satisfying `matches`, skipping others.
    fn expect(&self, what: &str, matches: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_else(|| panic!("timed out waiting for {what}"));
            let value = self
                .events
                .recv_timeout(remaining)
                .unwrap_or_else(|_| panic!("bridge ended before {what}"));
            if matches(&value) {
                return value;
            }
        }
    }
    fn web(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"Web": {"id": id, "method": method, "params": params}}));
        self.expect(&format!("reply {id}"), |v| v["Web"]["id"] == id)["Web"].clone()
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn read_records(mut output: ChildStdout, sender: mpsc::Sender<Value>) {
    loop {
        let mut header = [0u8; 4];
        if output.read_exact(&mut header).is_err() {
            return;
        }
        let mut body = vec![0u8; u32::from_be_bytes(header) as usize];
        if output.read_exact(&mut body).is_err() {
            return;
        }
        let Ok(value) = serde_json::from_slice(&body) else {
            return;
        };
        if sender.send(value).is_err() {
            return;
        }
    }
}
fn welcome(bridge: &mut Bridge) -> Value {
    bridge.send(json!({"Stream": {"type": "hello", "version": 1}}));
    bridge.expect("welcome", |v| v["Stream"]["type"] == "welcome")["Stream"].clone()
}
fn screen_text(update: &Value, pane: u64) -> String {
    update["Stream"]["snapshots"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["pane"] == pane)
        .flat_map(|s| s["screen"]["cells"].as_array().cloned().unwrap_or_default())
        .filter_map(|cell| cell["text"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn web_requests_need_a_negotiated_stream_first() {
    let server = Server::start();
    let mut bridge = Bridge::start(&server);
    bridge.send(json!({"Web": {"id": 1, "method": "state", "params": {}}}));
    let deadline = Instant::now() + Duration::from_secs(5);
    while bridge.child.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "bridge accepted a request before hello"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn terminal_operations_create_drive_search_and_close_panes() {
    let server = Server::start();
    let mut bridge = Bridge::start(&server);
    let welcome = welcome(&mut bridge);
    assert_eq!(welcome["version"], 1);
    let client = welcome["client"].as_u64().unwrap();
    assert!(
        welcome["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "pane-lifecycle")
    );

    let state = bridge.web(1, "state", json!({}));
    assert_eq!(state["result"]["api_version"], 3);
    assert_eq!(state["result"]["workspace"], "state");
    // Project, file and Git management do not exist here.
    for (id, method) in [
        (2, "files_list"),
        (3, "project_create"),
        (4, "upload_begin"),
    ] {
        let reply = bridge.web(id, method, json!({}));
        assert_eq!(reply["error"], "unsupported web operation");
    }
    // Request identifiers only increase; a replay is refused, not executed.
    let replay = bridge.web(4, "state", json!({}));
    assert!(replay["error"].as_str().unwrap().contains("duplicate"));

    let missing = bridge.web(5, "new", json!({}));
    assert!(missing["error"].as_str().unwrap().contains("identity"));
    let relative = bridge.web(6, "new", json!({"request": "web-test-0", "cwd": "tmp"}));
    assert!(relative["error"].as_str().unwrap().contains("absolute"));
    let created = bridge.web(
        7,
        "new",
        json!({"request": "web-test-1", "cwd": server.root.to_str().unwrap()}),
    );
    let pane = created["result"]["pane"]
        .as_u64()
        .unwrap_or_else(|| panic!("{created}"));

    bridge.send(json!({"Stream": {"type": "subscribe", "panes": [pane]}}));
    bridge.send(json!({"Stream": {"type": "claim", "pane": pane, "takeover": true}}));
    bridge.expect("lease", |v| {
        v["Stream"]["type"] == "lease"
            && v["Stream"]["pane"] == pane
            && v["Stream"]["owner"] == client
    });
    bridge.send(json!({"Stream": {"type": "resize", "pane": pane, "cols": 60, "rows": 12}}));
    let typed = b"echo kelp-web-marker\n".to_vec();
    bridge.send(json!({"Stream": {"type": "input", "pane": pane, "bytes": typed}}));
    bridge.expect("echoed output", |v| {
        v["Stream"]["type"] == "update"
            && screen_text(v, pane).matches("kelp-web-marker").count() >= 2
    });
    // An explicit resync replaces patches with a full snapshot at the
    // leased size.
    bridge.send(json!({"Stream": {"type": "resync", "pane": pane}}));
    let sized = bridge.expect("leased size", |v| {
        v["Stream"]["snapshots"]
            .as_array()
            .is_some_and(|s| s.iter().any(|s| s["pane"] == pane))
    });
    let snapshot = sized["Stream"]["snapshots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["pane"] == pane)
        .unwrap()
        .clone();
    assert_eq!(snapshot["screen"]["cols"], 60);
    assert_eq!(snapshot["screen"]["rows"], 12);

    let found = bridge.web(
        8,
        "search",
        json!({"pane": pane, "query": "kelp-web-marker"}),
    );
    assert!(
        !found["result"]["PaneSearch"]["hits"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{found}"
    );
    let invalid = bridge.web(9, "search", json!({"pane": pane, "query": "a\nb"}));
    assert!(invalid["error"].is_string());
    let history = bridge.web(
        10,
        "history",
        json!({"pane": pane, "start": -20, "limit": 20}),
    );
    assert!(
        history["result"]["History"]["lines"].is_array(),
        "{history}"
    );

    // Notification acknowledgements reach the server, which knows no
    // notification 1 in this fresh workspace.
    bridge.send(json!({"Stream": {"type": "acknowledge", "notification": 1}}));
    bridge.expect("server reply", |v| {
        v["Stream"]["type"] == "error"
            && v["Stream"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("unknown notification"))
    });

    let unconfirmed = bridge.web(11, "close", json!({"pane": pane}));
    assert!(
        unconfirmed["error"]
            .as_str()
            .unwrap()
            .contains("confirmation")
    );
    let closed = bridge.web(12, "close", json!({"pane": pane, "confirmed": true}));
    assert!(closed["result"].is_object(), "{closed}");
    bridge.expect("pane removal", |v| {
        v["Stream"]["type"] == "update"
            && v["Stream"]["panes"]
                .as_array()
                .is_some_and(|panes| !panes.iter().any(|p| p["id"] == pane))
    });
}

/// The TUI and a web client are independent clients of one server. Each can
/// type into any pane; a pane's PTY size follows whichever client most
/// recently claimed it by focusing or typing (see docs/web.md).
#[test]
fn tui_controller_and_web_client_alternate_pane_input_and_size() {
    use kelpmux::native::{
        protocol::{ClientMessage, ServerMessage, read_message, write_message},
        stream::{Event, Request},
    };
    let server = Server::start();
    // The TUI is a stream client like the browser.
    let mut tui = UnixStream::connect(server.socket()).unwrap();
    tui.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write_message(
        &mut tui,
        &ClientMessage::Stream(Request::Hello { version: 1 }),
    )
    .unwrap();
    match read_message::<ServerMessage>(&mut tui).unwrap().unwrap() {
        ServerMessage::Stream(Event::Welcome { .. }) => {}
        other => panic!("{other:?}"),
    }
    let mut drain = tui.try_clone().unwrap();
    drain.set_read_timeout(None).unwrap();
    thread::spawn(move || while let Ok(Some(_)) = read_message::<ServerMessage>(&mut drain) {});

    let mut bridge = Bridge::start(&server);
    let client = welcome(&mut bridge)["client"].as_u64().unwrap();
    bridge.send(json!({"Stream": {"type": "subscribe", "panes": []}}));
    let update = bridge.expect("metadata", |v| v["Stream"]["type"] == "update");
    let pane = update["Stream"]["panes"][0]["id"].as_u64().unwrap();
    bridge.send(json!({"Stream": {"type": "subscribe", "panes": [pane]}}));
    let size = |bridge: &mut Bridge| {
        bridge.send(json!({"Stream": {"type": "resync", "pane": pane}}));
        let update = bridge.expect("snapshot", |v| {
            v["Stream"]["snapshots"]
                .as_array()
                .is_some_and(|s| s.iter().any(|s| s["pane"] == pane))
        });
        let screen = &update["Stream"]["snapshots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["pane"] == pane)
            .unwrap()["screen"];
        (
            screen["cols"].as_u64().unwrap(),
            screen["rows"].as_u64().unwrap(),
        )
    };
    let base = size(&mut bridge);
    assert_ne!(base, (50, 10));

    // The web client focuses the pane: it claims it and imposes its size.
    bridge.send(json!({"Stream": {"type": "claim", "pane": pane, "takeover": true}}));
    bridge.send(json!({"Stream": {"type": "resize", "pane": pane, "cols": 50, "rows": 10}}));
    bridge.expect("web lease", |v| {
        v["Stream"]["type"] == "lease" && v["Stream"]["owner"] == client
    });
    bridge.send(
        json!({"Stream": {"type": "input", "pane": pane, "bytes": b"echo from-web\n".to_vec()}}),
    );
    assert_eq!(size(&mut bridge), (50, 10));

    // Typing in the TUI takes the pane back at the TUI's own size.
    for request in [
        Request::Claim {
            pane,
            takeover: true,
        },
        Request::Resize {
            pane,
            cols: 100,
            rows: 30,
        },
        Request::Input {
            pane,
            bytes: b"echo from-tui\n".to_vec(),
        },
    ] {
        write_message(&mut tui, &ClientMessage::Stream(request)).unwrap();
    }
    // KelpServer tells only the claiming client about a lease, so the
    // browser sees the takeover in the pane's size.
    let deadline = Instant::now() + Duration::from_secs(10);
    while size(&mut bridge) != (100, 30) {
        assert!(Instant::now() < deadline, "the TUI did not take the pane");
        thread::sleep(Duration::from_millis(50));
    }
    // Focusing it again in the browser hands it back, without any explicit
    // takeover command in the TUI.
    bridge.send(json!({"Stream": {"type": "claim", "pane": pane, "takeover": true}}));
    bridge.send(json!({"Stream": {"type": "resize", "pane": pane, "cols": 50, "rows": 10}}));
    bridge.send(
        json!({"Stream": {"type": "input", "pane": pane, "bytes": b"echo web-again\n".to_vec()}}),
    );
    // Every client's input reached the same shell: the server's search sees
    // each command's output.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut id = 0;
    for word in ["from-web", "from-tui", "web-again"] {
        loop {
            id += 1;
            let found = bridge.web(id, "search", json!({"pane": pane, "query": word}));
            if found["result"]["PaneSearch"]["hits"]
                .as_array()
                .is_some_and(|hits| hits.len() >= 2)
            {
                break;
            }
            assert!(Instant::now() < deadline, "missing {word}: {found}");
            thread::sleep(Duration::from_millis(50));
        }
    }
    assert_eq!(size(&mut bridge), (50, 10));
}

/// Closed terminals are listed with the TUI's restore actions and reopen in
/// a new pane in their folder; the browser chooses only the entry and the
/// action, never a command.
#[test]
fn closed_terminals_are_listed_and_restored() {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut folder = PathBuf::new();
    let server = Server::start_with(|root| {
        folder = root.join("project");
        fs::create_dir(&folder).unwrap();
        let state = root.join("state");
        fs::DirBuilder::new().mode(0o700).create(&state).unwrap();
        let entry = |key: &str, argv: &[&str], closed: u64| {
            json!({"key": key, "workspace": "state", "pane": 7,
                   "cwd": folder.to_str().unwrap(), "name": "program",
                   "kind": null, "session_id": null, "argv": argv,
                   "closed_at": closed, "reason": "Closed"})
        };
        let history = json!({"entries": [
            entry("old:7", &["sleep", "30"], now - 5),
            entry("old:8", &[], now - 10),
        ]});
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(state.join("tab-history.json"))
            .unwrap();
        file.write_all(&serde_json::to_vec(&history).unwrap())
            .unwrap();
    });
    let mut bridge = Bridge::start(&server);
    welcome(&mut bridge);

    let listing = bridge.web(1, "closed", json!({}));
    assert_eq!(listing["result"]["workspace"], "state");
    let entries = listing["result"]["entries"].as_array().unwrap();
    let program = entries
        .iter()
        .find(|e| e["key"] == "old:7")
        .unwrap_or_else(|| panic!("{listing}"));
    assert_eq!(program["choices"], json!(["restart", "folder"]));
    let folder_only = entries.iter().find(|e| e["key"] == "old:8").unwrap();
    assert_eq!(folder_only["choices"], json!(["folder"]));

    let unconfirmed = bridge.web(
        2,
        "restore",
        json!({"key": "old:8", "choice": "folder", "request": "web-restore-1"}),
    );
    assert!(
        unconfirmed["error"]
            .as_str()
            .unwrap()
            .contains("confirmation")
    );
    let wrong = bridge.web(
        3,
        "restore",
        json!({"key": "old:8", "choice": "restart", "request": "web-restore-1", "confirmed": true}),
    );
    assert!(
        wrong["error"].as_str().unwrap().contains("offers"),
        "{wrong}"
    );
    let missing = bridge.web(
        4,
        "restore",
        json!({"key": "nope", "choice": "folder", "request": "web-restore-1", "confirmed": true}),
    );
    assert!(missing["error"].as_str().unwrap().contains("no longer"));
    let restored = bridge.web(
        5,
        "restore",
        json!({"key": "old:8", "choice": "folder", "request": "web-restore-1", "confirmed": true}),
    );
    let reopened = restored["result"]["pane"]
        .as_u64()
        .unwrap_or_else(|| panic!("{restored}"));
    assert_eq!(restored["result"]["existing"], false);
    // Rerunning a program uses the server's recorded command line.
    let rerun = bridge.web(
        6,
        "restore",
        json!({"key": "old:7", "choice": "restart", "request": "web-restore-2", "confirmed": true}),
    );
    let program_pane = rerun["result"]["pane"]
        .as_u64()
        .unwrap_or_else(|| panic!("{rerun}"));
    bridge.send(json!({"Stream": {"type": "subscribe", "panes": [program_pane]}}));
    let canonical = folder.canonicalize().unwrap();
    bridge.expect("restored panes in their folder", |v| {
        v["Stream"]["type"] == "update"
            && v["Stream"]["panes"].as_array().is_some_and(|panes| {
                [reopened, program_pane].iter().all(|id| {
                    panes
                        .iter()
                        .any(|p| p["id"] == *id && p["cwd"] == canonical.to_str().unwrap())
                })
            })
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut id = 6;
    loop {
        id += 1;
        let processes = bridge.web(id, "status", json!({}));
        let sleeping = processes["result"]["processes"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|p| {
                p["command"]
                    .as_str()
                    .is_some_and(|c| c.ends_with("/sleep") || c == "sleep")
            });
        if sleeping {
            break;
        }
        assert!(Instant::now() < deadline, "rerun not started: {processes}");
        thread::sleep(Duration::from_millis(100));
    }
}

/// A stopped workspace directory for start tests; shuts down whatever
/// server a test started in it.
struct Stopped {
    root: PathBuf,
}
impl Stopped {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "kelpmux-web-start-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("state")).unwrap();
        fs::set_permissions(root.join("state"), fs::Permissions::from_mode(0o700)).unwrap();
        Self { root }
    }
    fn socket(&self) -> PathBuf {
        self.root.join("state/control.sock")
    }
}
impl Drop for Stopped {
    fn drop(&mut self) {
        if let Ok(mut socket) = UnixStream::connect(self.socket()) {
            shutdown(&mut socket);
            let until = Instant::now() + Duration::from_secs(3);
            while self.socket().exists() && Instant::now() < until {
                thread::sleep(Duration::from_millis(20));
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn start_is_refused_while_the_workspace_server_runs() {
    let server = Server::start();
    let mut bridge = Bridge::start(&server);
    welcome(&mut bridge);
    let state = bridge.web(1, "state", json!({}));
    assert!(
        state["result"]["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|op| op == "start")
    );
    let refused = bridge.web(2, "start", json!({"confirmed": true}));
    assert_eq!(refused["error"], "the workspace server is already running");
}

#[test]
fn a_stopped_workspace_starts_on_an_explicit_request_then_connects() {
    let workspace = Stopped::new();
    let mut bridge = Bridge::at(&workspace.socket(), &workspace.root);
    bridge.send(json!({"Stream": {"type": "hello", "version": 1}}));
    let notice = bridge.expect("stopped notice", |v| v.get("Workspace").is_some());
    assert_eq!(notice["Workspace"]["running"], false);
    assert_eq!(notice["Workspace"]["workspace"], "state");
    let state = bridge.web(1, "state", json!({}));
    assert_eq!(state["result"]["running"], false);
    assert_eq!(state["result"]["operations"], json!(["state", "start"]));
    let other = bridge.web(2, "new", json!({}));
    assert!(other["error"].as_str().unwrap().contains("not running"));
    let unconfirmed = bridge.web(3, "start", json!({}));
    assert!(
        unconfirmed["error"]
            .as_str()
            .unwrap()
            .contains("confirmation")
    );
    assert!(!workspace.socket().exists(), "nothing starts unconfirmed");
    let started = bridge.web(4, "start", json!({"confirmed": true}));
    assert_eq!(started["result"]["started"], true, "{started}");
    // The bridge then connects as usual: the stream welcome follows.
    let welcome = bridge.expect("welcome", |v| v["Stream"]["type"] == "welcome");
    assert_eq!(welcome["Stream"]["version"], 1);
    // The server started without panes (`--startup-panes none`); the
    // bridge created the one shell the browser opens on.
    assert_eq!(welcome["Stream"]["startup_panes"], "none", "{welcome}");
    bridge.send(json!({"Stream": {"type": "subscribe", "panes": []}}));
    let update = bridge.expect("the first shell", |v| {
        v["Stream"]["type"] == "update"
            && v["Stream"]["panes"]
                .as_array()
                .is_some_and(|p| !p.is_empty())
    });
    assert_eq!(update["Stream"]["panes"].as_array().unwrap().len(), 1);
    let state = bridge.web(5, "state", json!({}));
    assert_eq!(state["result"]["api_version"], 3);
    let again = bridge.web(6, "start", json!({"confirmed": true}));
    assert_eq!(again["error"], "the workspace server is already running");
}

#[test]
fn start_never_takes_a_workspace_binary_or_replacement_from_the_browser() {
    let workspace = Stopped::new();
    let mut bridge = Bridge::at(&workspace.socket(), &workspace.root);
    bridge.send(json!({"Stream": {"type": "hello", "version": 1}}));
    bridge.expect("stopped notice", |v| v.get("Workspace").is_some());
    for (id, params) in [
        json!({"confirmed": true, "workspace": "other"}),
        json!({"confirmed": true, "state_dir": "/tmp/other"}),
        json!({"confirmed": true, "server_binary": "/bin/sh"}),
        json!({"confirmed": true, "replace_running": true}),
    ]
    .into_iter()
    .enumerate()
    {
        let refused = bridge.web(id as u64 + 1, "start", params);
        assert!(
            refused["error"]
                .as_str()
                .unwrap()
                .contains("takes no options"),
            "{refused}"
        );
    }
    assert!(!workspace.socket().exists(), "no server started");
}

#[test]
fn tab_colour_is_browser_state_not_a_bridge_operation() {
    // Tab colours are each client's own UI state (ADR 0002, 144b): the web
    // keeps them per browser, so the bridge offers no `color` operation.
    let server = Server::start();
    let mut bridge = Bridge::start(&server);
    welcome(&mut bridge);
    let state = bridge.web(1, "state", json!({}));
    let operations = state["result"]["operations"].as_array().unwrap();
    assert!(!operations.iter().any(|op| op == "color"), "{state}");
    let refused = bridge.web(2, "color", json!({"pane": 1, "color": 3}));
    assert_eq!(refused["error"], "unsupported web operation");
}

#[test]
fn repos_and_commands_report_read_only_facts_for_the_terminals_table() {
    let server = Server::start_with(|root| {
        fs::create_dir_all(root.join("checkout/.git")).unwrap();
        fs::create_dir_all(root.join("checkout/src")).unwrap();
        fs::write(root.join("checkout/.git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            root.join("checkout/.git/config"),
            "[remote \"origin\"]\n\turl = git@github.com:owner/project.git\n",
        )
        .unwrap();
    });
    let mut bridge = Bridge::start(&server);
    welcome(&mut bridge);
    let state = bridge.web(1, "state", json!({}));
    assert_eq!(state["result"]["api_version"], 3);
    assert_eq!(state["result"]["api_minor"], 0);
    let operations = state["result"]["operations"].as_array().unwrap();
    assert!(operations.iter().any(|op| op == "repos"));
    assert!(operations.iter().any(|op| op == "commands"));

    let checkout = server.root.join("checkout/src");
    let repos = bridge.web(
        2,
        "repos",
        json!({"paths": [checkout.to_str().unwrap(), server.root.to_str().unwrap(), "relative"]}),
    );
    let list = repos["result"]["repos"].as_array().unwrap();
    assert_eq!(
        list[0],
        json!({
            "root": server.root.join("checkout").to_str().unwrap(),
            "branch": "main",
            "detached": null,
            "github": "owner/project",
        })
    );
    assert_eq!(list[1], Value::Null);
    assert_eq!(list[2], Value::Null);
    let many = vec!["/"; 65];
    let refused = bridge.web(3, "repos", json!({ "paths": many }));
    assert!(refused["error"].as_str().unwrap().contains("at most"));
    let wrong = bridge.web(4, "repos", json!({ "paths": [1] }));
    assert!(wrong["error"].as_str().is_some());

    let created = bridge.web(5, "new", json!({"request": "web-cmd-1"}));
    let pane = created["result"]["pane"].as_u64().unwrap();
    let commands = bridge.web(6, "commands", json!({}));
    let map = commands["result"]["commands"].as_object().unwrap();
    // A shell at its prompt has no foreground program of its own.
    assert!(map.contains_key(&pane.to_string()), "{commands}");
}

/// The bridge relays the step 2 session requests unchanged: dismissing the
/// recovery signal and creating a pane with another shell. Welcome carries
/// the startup policy, limits and notification run.
#[test]
fn step_two_session_requests_pass_through_the_bridge() {
    let server = Server::start();
    let mut bridge = Bridge::start(&server);
    let welcome = welcome(&mut bridge);
    assert_eq!(welcome["startup_panes"], "layout", "{welcome}");
    assert_eq!(welcome["limits"]["subscriptions"], 64);
    assert!(welcome["notification_run"].is_string());
    bridge.send(json!({"Stream": {"type": "dismiss_recovery", "request": "r1"}}));
    bridge.expect("dismissed", |v| {
        v["Stream"]["type"] == "done" && v["Stream"]["request"] == "r1"
    });
    bridge.send(json!({"Stream": {"type": "session_events", "enabled": true}}));
    bridge.send(json!({"Stream": {
        "type": "spawn", "request": "r2", "argv": [], "shell": "/bin/sh"
    }}));
    let spawned = bridge.expect("spawned", |v| {
        v["Stream"]["type"] == "spawned" && v["Stream"]["request"] == "r2"
    });
    let pane = spawned["Stream"]["pane"].as_u64().unwrap();
    let client = welcome["client"].as_u64().unwrap();
    bridge.expect("session of the new pane", |v| {
        v["Stream"]["sessions"].as_array().is_some_and(|sessions| {
            sessions
                .iter()
                .any(|s| s["pane"] == pane && s["shell"] == "/bin/sh" && s["spawned_by"] == client)
        })
    });
    bridge.send(json!({"Stream": {
        "type": "spawn", "request": "r3", "argv": [], "shell": "relative/sh"
    }}));
    bridge.expect("refused shell", |v| {
        v["Stream"]["type"] == "error" && v["Stream"]["request"] == "r3"
    });
}
