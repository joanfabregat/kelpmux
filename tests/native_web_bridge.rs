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
            // The file root is HOME: make it this disposable root, which
            // holds every path the file tests use.
            .env("HOME", &root)
            // Uploads without a destination land in the server's cache
            // directory: keep them inside this disposable root.
            .env("XDG_CACHE_HOME", root.join("cache"))
            // The file operations' trash too.
            .env("XDG_DATA_HOME", root.join("data"))
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
            // This test's own user configuration.
            .env("XDG_CONFIG_HOME", cwd.join("xdg-config"))
            // Environment ports and instance folders under this root.
            .env("XDG_STATE_HOME", cwd.join("xdg-state"))
            // New terminals start in its Scratch folder, `~/scratch`.
            .env("HOME", cwd)
            // Agent transcripts (History) under this HOME only.
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
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
        // A binary screen record (`binary-screens`) arrives as its payload
        // bytes, so tests can decode it.
        if kelpclient_protocol::screens::is_binary_record(&body) {
            if sender.send(json!({ "Binary": body })).is_err() {
                return;
            }
            continue;
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
    // File and Git management are not bridge operations.
    for (id, method) in [(2, "files_list"), (3, "git_status"), (4, "upload_begin")] {
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
    // The server starts without panes; the browser creates the shell.
    let created = bridge.web(1, "new", json!({"request": "web-first"}));
    let pane = created["result"]["pane"].as_u64().unwrap();
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
    // Snapshots of an earlier size may still be queued (the subscription's
    // and each resync's): wait for the latest to show the expected size.
    let until_size = |bridge: &mut Bridge, expected: (u64, u64), what: &str| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while size(bridge) != expected {
            assert!(Instant::now() < deadline, "{what}");
            thread::sleep(Duration::from_millis(50));
        }
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
    until_size(&mut bridge, (50, 10), "the browser did not take the pane");

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
    until_size(&mut bridge, (100, 30), "the TUI did not take the pane");
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
    let mut id = 1;
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
    until_size(
        &mut bridge,
        (50, 10),
        "the browser did not take the pane back",
    );
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
    assert_eq!(
        state["result"]["operations"],
        json!(["state", "start", "config"])
    );
    // The user configuration's file root reaches the server it starts.
    let config = workspace.root.join("xdg-config/kelpmux/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        format!(
            "[files]\nroot = \"{}\"\nextra_roots = [\"/\"]\n[projects]\nbase = \"/p\"\n[scratch]\nfolder = \"/s\"\n",
            workspace.root.display()
        ),
    )
    .unwrap();
    let read = bridge.web(2, "config", json!({}));
    assert_eq!(read["result"]["current"], true, "{read}");
    let other = bridge.web(3, "new", json!({}));
    assert!(other["error"].as_str().unwrap().contains("not running"));
    let unconfirmed = bridge.web(4, "start", json!({}));
    assert!(
        unconfirmed["error"]
            .as_str()
            .unwrap()
            .contains("confirmation")
    );
    assert!(!workspace.socket().exists(), "nothing starts unconfirmed");
    let started = bridge.web(5, "start", json!({"confirmed": true}));
    assert_eq!(started["result"]["started"], true, "{started}");
    let recorded: Value =
        serde_json::from_slice(&fs::read(workspace.root.join("state/workspace.json")).unwrap())
            .unwrap();
    assert_eq!(recorded["files_root"], json!(workspace.root), "{recorded}");
    assert_eq!(recorded["files_extra_roots"], json!(["/"]), "{recorded}");
    // The bridge then connects as usual: the stream welcome follows.
    let welcome = bridge.expect("welcome", |v| v["Stream"]["type"] == "welcome");
    assert_eq!(welcome["Stream"]["version"], 1);
    // The server started without panes (KelpServer 0.3); the
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
    let state = bridge.web(6, "state", json!({}));
    assert_eq!(state["result"]["api_version"], 3);
    let again = bridge.web(7, "start", json!({"confirmed": true}));
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

/// Projects (bridge API minor 5): created in config.toml with their folder,
/// terminals created in them and moved between them, links shared by every
/// bridge through the workspace's `terminal-projects.json`, closed entries
/// keeping their project and restored terminals inheriting it.
#[test]
fn projects_link_terminals_for_every_bridge_and_survive_closing() {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let server = Server::start_with(|root| {
        let state = root.join("state");
        fs::DirBuilder::new().mode(0o700).create(&state).unwrap();
        let history = json!({"entries": [{"key": "old:7", "workspace": "state", "pane": 7,
            "cwd": root.to_str().unwrap(), "name": "program", "kind": null,
            "session_id": null, "argv": [], "closed_at": now - 5, "reason": "Closed"}]});
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(state.join("tab-history.json"))
            .unwrap();
        file.write_all(history.to_string().as_bytes()).unwrap();
    });
    let mut bridge = Bridge::start(&server);
    welcome(&mut bridge);
    let state = bridge.web(1, "state", json!({}));
    assert!(
        state["result"]["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|op| op == "projects")
    );
    // A plain new terminal starts in the Scratch folder, created for it.
    let plain = bridge.web(2, "new", json!({"request": "p-1"}));
    assert!(plain["result"]["pane"].as_u64().is_some(), "{plain}");
    assert!(server.root.join("scratch").is_dir());
    // A project with the default folder <projects.base>/<name>, created.
    let created = bridge.web(
        3,
        "project_create",
        json!({"name": "Kelp Mux", "color": "teal"}),
    );
    assert_eq!(created["result"]["saved"], true, "{created}");
    assert_eq!(created["result"]["project"], "kelp-mux");
    assert!(server.root.join("projects/Kelp Mux").is_dir());
    let projects = &created["result"]["config"]["projects"];
    assert_eq!(projects[0]["folder"], "~/projects/Kelp Mux", "{created}");
    assert_eq!(projects[0]["missing"], false);
    // An overlapping folder is refused with the new project's field.
    let inside = bridge.web(
        4,
        "project_create",
        json!({"name": "Inner", "folder": "~/projects/Kelp Mux/inner"}),
    );
    assert_eq!(inside["result"]["saved"], false, "{inside}");
    assert_eq!(inside["result"]["errors"][0]["key"], "project[1].folder");
    // A terminal created in the project starts in its folder and belongs to it.
    let made = bridge.web(5, "new", json!({"request": "p-2", "project": "kelp-mux"}));
    let pane = made["result"]["pane"]
        .as_u64()
        .unwrap_or_else(|| panic!("{made}"));
    let canonical = server
        .root
        .join("projects/Kelp Mux")
        .canonicalize()
        .unwrap();
    bridge.send(json!({"Stream": {"type": "subscribe", "panes": [pane]}}));
    bridge.expect("the project terminal in its folder", |v| {
        v["Stream"]["type"] == "update"
            && v["Stream"]["panes"].as_array().is_some_and(|panes| {
                panes
                    .iter()
                    .any(|p| p["id"] == pane && p["cwd"] == canonical.to_str().unwrap())
            })
    });
    let listed = bridge.web(6, "projects", json!({}));
    assert_eq!(
        listed["result"]["panes"],
        json!({ pane.to_string(): "kelp-mux" })
    );
    // Moved to Scratch, then back; an unknown project is refused.
    let moved = bridge.web(7, "project_move", json!({"pane": pane, "project": null}));
    assert_eq!(moved["result"]["panes"], json!({}), "{moved}");
    let unknown = bridge.web(8, "project_move", json!({"pane": pane, "project": "nope"}));
    assert!(
        unknown["error"]
            .as_str()
            .unwrap()
            .contains("no longer exists")
    );
    let closed_pane = bridge.web(
        9,
        "project_move",
        json!({"pane": 999, "project": "kelp-mux"}),
    );
    assert!(closed_pane["error"].as_str().unwrap().contains("closed"));
    bridge.web(
        10,
        "project_move",
        json!({"pane": pane, "project": "kelp-mux"}),
    );
    // Another browser's bridge sees the same links.
    let mut other = Bridge::start(&server);
    welcome(&mut other);
    let seen = other.web(1, "projects", json!({}));
    assert_eq!(
        seen["result"]["panes"],
        json!({ pane.to_string(): "kelp-mux" })
    );
    // A closed terminal keeps its project, and reopening it inherits it.
    let links = server.root.join("state/terminal-projects.json");
    let mut stored: Value = serde_json::from_slice(&fs::read(&links).unwrap()).unwrap();
    stored["links"]["old:7"] = json!({"project": "kelp-mux", "at": now});
    fs::write(&links, stored.to_string()).unwrap();
    let closed = other.web(2, "closed", json!({}));
    let entry = closed["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == "old:7")
        .unwrap_or_else(|| panic!("{closed}"))
        .clone();
    assert_eq!(entry["project"], "kelp-mux");
    let restored = other.web(
        3,
        "restore",
        json!({"key": "old:7", "choice": "folder", "request": "p-restore", "confirmed": true}),
    );
    let reopened = restored["result"]["pane"]
        .as_u64()
        .unwrap_or_else(|| panic!("{restored}"));
    let after = other.web(4, "projects", json!({}));
    assert_eq!(
        after["result"]["panes"][reopened.to_string()],
        "kelp-mux",
        "{after}"
    );
    // Renamed (id and folder stay), then deleted: the folder stays and its
    // terminals are Scratch's again (their link names no project).
    let renamed = bridge.web(
        11,
        "project_update",
        json!({"id": "kelp-mux", "name": "Kelp", "color": null}),
    );
    assert_eq!(
        renamed["result"]["config"]["projects"][0]["name"], "Kelp",
        "{renamed}"
    );
    assert_eq!(
        renamed["result"]["config"]["projects"][0]["color"],
        Value::Null
    );
    let unconfirmed = bridge.web(12, "project_delete", json!({"id": "kelp-mux"}));
    assert!(
        unconfirmed["error"]
            .as_str()
            .unwrap()
            .contains("confirmation")
    );
    let deleted = bridge.web(
        13,
        "project_delete",
        json!({"id": "kelp-mux", "confirmed": true}),
    );
    assert_eq!(deleted["result"]["saved"], true, "{deleted}");
    assert_eq!(deleted["result"]["config"]["projects"], json!([]));
    assert!(server.root.join("projects/Kelp Mux").is_dir());
    // Its id is not given to a new project of the same name.
    let again = bridge.web(14, "project_create", json!({"name": "Kelp Mux"}));
    assert_eq!(again["result"]["project"], "kelp-mux-2", "{again}");
}

/// `git` (API 3.6): a project's checkouts, found in its folder and direct
/// subfolders, answered on the bridge's Git threads while other operations
/// keep working; the folder comes from the configuration, never a path.
#[test]
fn git_reports_a_projects_checkouts_read_only() {
    let server = Server::start();
    let mut bridge = Bridge::start(&server);
    welcome(&mut bridge);
    let state = bridge.web(1, "state", json!({}));
    assert!(
        state["result"]["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|op| op == "git")
    );
    let created = bridge.web(2, "project_create", json!({"name": "Work"}));
    assert_eq!(created["result"]["saved"], true, "{created}");
    let folder = server.root.join("projects/Work");
    let git = |dir: &std::path::Path, args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    };
    let api = folder.join("api");
    fs::create_dir_all(&api).unwrap();
    git(&api, &["init", "-q", "-b", "main"]);
    fs::write(api.join("a.txt"), "one\n").unwrap();
    git(&api, &["add", "."]);
    git(&api, &["commit", "-q", "-m", "First"]);
    fs::write(api.join("a.txt"), "one\ntwo\n").unwrap();
    fs::create_dir_all(folder.join("plain")).unwrap();
    let reply = bridge.web(3, "git", json!({"project": "work", "files": true}));
    let repos = reply["result"]["repos"]
        .as_array()
        .unwrap_or_else(|| panic!("{reply}"));
    assert_eq!(repos.len(), 1, "{reply}");
    assert_eq!(repos[0]["name"], "api");
    assert_eq!(repos[0]["branch"], "main");
    assert_eq!(repos[0]["files"], 1);
    assert_eq!(repos[0]["added"], 1);
    assert_eq!(repos[0]["last"]["subject"], "First");
    assert_eq!(repos[0]["changes"][0]["path"], "a.txt");
    // Without `files`, no list; Scratch has none; no path is accepted.
    let summary = bridge.web(4, "git", json!({"project": "work"}));
    assert!(summary["result"]["repos"][0].get("changes").is_none());
    let scratch = bridge.web(5, "git", json!({"project": "scratch"}));
    assert_eq!(scratch["result"]["repos"], json!([]), "{scratch}");
    let unknown = bridge.web(6, "git", json!({"project": "nope"}));
    assert_eq!(unknown["error"], "this project no longer exists");
    let path = bridge.web(7, "git", json!({"folder": "/"}));
    assert_eq!(path["error"], "project required");
    // A github.com origin gives the checkout's `owner/name` (API 3.8),
    // never the URL (which could hold credentials).
    git(
        &api,
        &[
            "remote",
            "add",
            "origin",
            "https://user:secret@github.com/owner/api.git",
        ],
    );
    let reply = bridge.web(8, "git", json!({"project": "work", "fresh": true}));
    assert_eq!(
        reply["result"]["repos"][0]["github"],
        Value::Null,
        "{reply}"
    );
    assert!(!reply.to_string().contains("secret"));
    git(
        &api,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:owner/api.git",
        ],
    );
    let reply = bridge.web(9, "git", json!({"project": "work", "fresh": true}));
    assert_eq!(
        reply["result"]["repos"][0]["github"], "owner/api",
        "{reply}"
    );
}

/// The history operations (API 3.10): a checkout's branches, log, commit
/// details, diffs and working changes, for checkouts `git` reported only,
/// with revisions and paths checked before any Git runs.
#[test]
fn git_history_reads_a_checkouts_history_and_diffs() {
    let server = Server::start();
    let mut bridge = Bridge::start(&server);
    welcome(&mut bridge);
    let state = bridge.web(1, "state", json!({}));
    assert_eq!(state["result"]["api_minor"], 10);
    assert!(
        state["result"]["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|op| op == "git_history")
    );
    let created = bridge.web(2, "project_create", json!({"name": "Hist"}));
    assert_eq!(created["result"]["saved"], true, "{created}");
    let folder = server.root.join("projects/Hist");
    let git = |dir: &std::path::Path, args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    };
    let api = folder.join("api");
    fs::create_dir_all(&api).unwrap();
    git(&api, &["init", "-q", "-b", "main"]);
    fs::write(api.join("a.txt"), "one\n").unwrap();
    git(&api, &["add", "."]);
    git(&api, &["commit", "-q", "-m", "First"]);
    fs::write(api.join("a.txt"), "one\ntwo\n").unwrap();
    git(&api, &["commit", "-q", "-am", "Second"]);
    fs::write(api.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    fs::write(api.join("new.txt"), "n\n").unwrap();
    // The checkout's path as `git` reports it.
    let listed = bridge.web(3, "git", json!({"project": "hist"}));
    let repo = listed["result"]["repos"][0]["path"]
        .as_str()
        .unwrap_or_else(|| panic!("{listed}"))
        .to_owned();
    let at = |extra: Value| {
        let mut params = json!({"project": "hist", "repo": repo});
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        params
    };
    let branches = bridge.web(4, "git_branches", at(json!({})));
    assert_eq!(branches["result"]["current"], "main", "{branches}");
    assert_eq!(branches["result"]["branches"][0]["subject"], "Second");
    let log = bridge.web(5, "git_log", at(json!({"limit": 1})));
    assert_eq!(log["result"]["commits"][0]["subject"], "Second", "{log}");
    assert_eq!(log["result"]["more"], true);
    let sha = log["result"]["commits"][0]["sha"]
        .as_str()
        .unwrap()
        .to_owned();
    let commit = bridge.web(6, "git_commit", at(json!({"rev": &sha[..10]})));
    assert_eq!(commit["result"]["sha"], sha, "{commit}");
    assert_eq!(commit["result"]["files"][0]["path"], "a.txt");
    let diff = bridge.web(
        7,
        "git_diff",
        at(json!({"kind": "commit", "rev": sha, "path": "a.txt"})),
    );
    assert_eq!(
        diff["result"]["files"][0]["hunks"][0]["lines"],
        json!([" one", "+two"]),
        "{diff}"
    );
    let changes = bridge.web(8, "git_changes", at(json!({})));
    assert_eq!(
        changes["result"]["unstaged"][0]["path"], "a.txt",
        "{changes}"
    );
    assert_eq!(changes["result"]["untracked"][0]["path"], "new.txt");
    let untracked = bridge.web(
        9,
        "git_diff",
        at(json!({"kind": "untracked", "path": "new.txt"})),
    );
    assert_eq!(
        untracked["result"]["files"][0]["hunks"][0]["lines"],
        json!(["+n"])
    );
    // Refused before any Git: options, revision syntax, paths outside,
    // checkouts `git` did not report.
    let refused = [
        (
            "git_log",
            at(json!({"rev": "--output=/tmp/x"})),
            "invalid revision",
        ),
        ("git_log", at(json!({"rev": "@{-1}"})), "invalid revision"),
        (
            "git_commit",
            at(json!({"rev": "HEAD:../"})),
            "invalid revision",
        ),
        (
            "git_diff",
            at(json!({"kind": "unstaged", "path": "../../etc/passwd"})),
            "invalid path: a relative path inside the repository",
        ),
        (
            "git_diff",
            at(json!({"kind": "commit", "rev": "HEAD", "path": "-c"})),
            "invalid path: a relative path inside the repository",
        ),
        (
            "git_log",
            json!({"project": "hist", "repo": "/"}),
            "this repository is not one of the project's checkouts",
        ),
        (
            "git_log",
            json!({"project": "hist"}),
            "repo must be the path of one of the project's checkouts",
        ),
        ("git_log", at(json!({"rev": "nope"})), "unknown revision"),
    ];
    for (n, (method, params, error)) in refused.into_iter().enumerate() {
        let reply = bridge.web(10 + n as u64, method, params);
        assert_eq!(reply["error"], error, "{method}: {reply}");
    }
}

/// `github_*` (API 3.8): signed out without `[github]`, explains a token
/// source that fails, and checks every request before any work. Nothing
/// here reaches GitHub.
#[test]
fn github_operations_explain_sign_in_and_check_their_requests() {
    let server = Server::start();
    let mut bridge = Bridge::start(&server);
    welcome(&mut bridge);
    let state = bridge.web(1, "state", json!({}));
    assert!(
        state["result"]["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|op| op == "github")
    );
    let status = bridge.web(2, "github_status", json!({}));
    assert_eq!(status["result"]["enabled"], false, "{status}");
    assert_eq!(status["result"]["signed_in"], false);
    let config = server.root.join("xdg-config/kelpmux/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        "[github]\ntoken_command = \"/nonexistent/github-token\"\n",
    )
    .unwrap();
    let mut id = 3;
    let status = loop {
        let status = bridge.web(id, "github_status", json!({}));
        id += 1;
        if !status["result"]["error"].is_null() || id > 200 {
            break status;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status["result"]["enabled"], true, "{status}");
    assert_eq!(status["result"]["source"], "command");
    assert_eq!(status["result"]["signed_in"], false);
    assert!(
        status["result"]["error"]
            .as_str()
            .unwrap()
            .contains("not an executable"),
        "{status}"
    );
    for (method, params, error) in [
        ("github_state", json!({"repos": ["../x"]}), "owner/name"),
        (
            "github_state",
            json!({"repos": "a/b"}),
            "list of owner/name",
        ),
        (
            "github_clone",
            json!({"project": "scratch", "url": "file:///srv/x.git"}),
            "HTTPS or SSH",
        ),
        (
            "github_clone",
            json!({"project": "scratch", "url": "https://u:p@github.com/o/r.git"}),
            "credentials",
        ),
        (
            "github_clone",
            json!({"project": "scratch"}),
            "url required",
        ),
        (
            "github_clone",
            json!({"project": "nope", "url": "git@github.com:o/r.git"}),
            "no longer exists",
        ),
        ("github_clone_cancel", json!({"clone": 5}), "not known"),
        ("github_nope", json!({}), "unsupported"),
    ] {
        id += 1;
        let reply = bridge.web(id, method, params);
        assert!(
            reply["error"].as_str().is_some_and(|e| e.contains(error)),
            "{method}: {reply}"
        );
    }
}

/// `conversations` (API 3.7): closed terminals with the conversations they
/// hosted, titled from the first prompt of their transcripts, and
/// conversations started outside KelpMux in a project's or the Scratch
/// folder; `new` with `resume` runs the resume command built by the bridge.
#[test]
fn history_lists_conversations_and_new_resumes_them() {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let server = Server::start_with(|root| {
        let work = root.join("projects/Work");
        fs::create_dir_all(work.join("sub")).unwrap();
        fs::create_dir_all(root.join("scratch")).unwrap();
        let state = root.join("state");
        fs::DirBuilder::new().mode(0o700).create(&state).unwrap();
        let history = json!({"entries": [
            {"key": "old:7", "workspace": "state", "pane": 7,
             "cwd": work.to_str().unwrap(), "name": "claude tab",
             "kind": "claude", "session_id": "conv-1", "argv": [],
             "closed_at": now - 5, "reason": "Closed"},
            {"key": "old:8", "workspace": "state", "pane": 8,
             "cwd": work.to_str().unwrap(), "name": "make",
             "kind": null, "session_id": null, "argv": ["make"],
             "closed_at": now - 9, "reason": "Closed"},
            // Codex, whose conversation the server never identified.
            {"key": "old:9", "workspace": "state", "pane": 9,
             "cwd": root.join("scratch").to_str().unwrap(), "name": "codex",
             "kind": "codex", "session_id": null, "argv": [],
             "closed_at": now - 5, "reason": "Closed"},
        ]});
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(state.join("tab-history.json"))
            .unwrap();
        file.write_all(&serde_json::to_vec(&history).unwrap())
            .unwrap();
        let claude = |cwd: &std::path::Path, id: &str, prompt: &str| {
            let dir = root
                .join(".claude/projects")
                .join(cwd.to_str().unwrap().replace(['/', '.'], "-"));
            fs::create_dir_all(&dir).unwrap();
            let lines = [
                json!({"type": "permission-mode", "sessionId": id}),
                json!({"type": "user", "cwd": cwd, "sessionId": id,
                       "timestamp": "2026-11-24T10:00:00Z",
                       "message": {"role": "user", "content": prompt}}),
                json!({"type": "assistant", "message": {"content": "never sent"}}),
            ];
            let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
            fs::write(dir.join(format!("{id}.jsonl")), text).unwrap();
        };
        claude(&work, "conv-1", "Make the History list");
        claude(&work.join("sub"), "ext-1", "Started outside");
        claude(&root.join("elsewhere"), "ext-2", "Not in a project");
        let codex_day = {
            // Today's date folder (UTC), like Codex's local date.
            let day = now / 86_400;
            let output = std::process::Command::new("date")
                .args(["-u", "-d", &format!("@{}", day * 86_400), "+%Y/%m/%d"])
                .output()
                .unwrap();
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        let codex = root.join(".codex/sessions").join(codex_day);
        fs::create_dir_all(&codex).unwrap();
        let lines = [
            json!({"type": "session_meta", "payload": {"id": "cx-9",
                   "cwd": root.join("scratch"), "source": "cli"}}),
            json!({"type": "event_msg", "payload": {"type": "user_message",
                   "message": "A scratch idea"}}),
        ];
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        fs::write(codex.join("rollout-2026-11-24T10-00-00-cx-9.jsonl"), text).unwrap();
    });
    let mut bridge = Bridge::start(&server);
    welcome(&mut bridge);
    let state = bridge.web(1, "state", json!({}));
    assert_eq!(state["result"]["api_minor"], 10);
    assert!(
        state["result"]["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|op| op == "conversations")
    );
    let created = bridge.web(2, "project_create", json!({"name": "Work"}));
    assert_eq!(created["result"]["saved"], true, "{created}");

    let listing = bridge.web(3, "conversations", json!({}));
    let result = &listing["result"];
    assert_eq!(result["workspace"], "state", "{listing}");
    let entries = result["entries"].as_array().unwrap();
    let agent = entries.iter().find(|e| e["key"] == "old:7").unwrap();
    assert_eq!(agent["conversations"][0]["id"], "conv-1", "{listing}");
    assert_eq!(agent["conversations"][0]["title"], "Make the History list");
    let make = entries.iter().find(|e| e["key"] == "old:8").unwrap();
    assert_eq!(make["conversations"], json!([]));
    assert_eq!(make["choices"], json!(["restart", "folder"]));
    // The only Codex transcript of its folder, ended by its close.
    let codex = entries.iter().find(|e| e["key"] == "old:9").unwrap();
    assert_eq!(codex["conversations"][0]["id"], "cx-9", "{listing}");
    assert_eq!(codex["conversations"][0]["matched"], true);
    let mut external: Vec<(String, String, String)> = result["external"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["id"].as_str().unwrap().into(),
                row["project"].as_str().unwrap().into(),
                row["title"].as_str().unwrap().into(),
            )
        })
        .collect();
    external.sort();
    assert_eq!(
        external,
        [("ext-1".into(), "work".into(), "Started outside".into())],
        "{listing}"
    );
    assert!(!listing.to_string().contains("never sent"));

    // Resuming opens a terminal in the project that runs the resume command.
    let folder = server.root.join("projects/Work");
    let bad = bridge.web(
        4,
        "new",
        json!({"request": "r-1", "project": "work", "cwd": folder,
               "resume": {"kind": "claude", "id": "x; touch pwned"}}),
    );
    assert!(
        bad["error"].as_str().unwrap().contains("conversation id"),
        "{bad}"
    );
    let resumed = bridge.web(
        5,
        "new",
        json!({"request": "r-2", "project": "work", "cwd": folder,
               "resume": {"kind": "claude", "id": "conv-1"}}),
    );
    let pane = resumed["result"]["pane"]
        .as_u64()
        .unwrap_or_else(|| panic!("{resumed}"));
    let links = bridge.web(6, "projects", json!({}));
    assert_eq!(
        links["result"]["panes"][pane.to_string()],
        "work",
        "{links}"
    );
    // The shell was asked to run `claude --resume conv-1` (absent here).
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut id = 7;
    loop {
        let history = bridge.web(id, "history", json!({"pane": pane}));
        let text = history["result"]["History"]["lines"].to_string();
        if text.contains("claude") {
            break;
        }
        assert!(Instant::now() < deadline, "{history}");
        id += 1;
        thread::sleep(Duration::from_millis(100));
    }
    // A terminal whose conversation is unknown opens the agent's picker.
    id += 1;
    let picker = bridge.web(
        id,
        "new",
        json!({"request": "r-3", "cwd": server.root.join("scratch"),
               "resume": {"kind": "codex", "picker": true}}),
    );
    let pane = picker["result"]["pane"]
        .as_u64()
        .unwrap_or_else(|| panic!("{picker}"));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        id += 1;
        let history = bridge.web(id, "history", json!({"pane": pane}));
        let text = history["result"]["History"]["lines"].to_string();
        if text.contains("codex") {
            break;
        }
        assert!(Instant::now() < deadline, "{history}");
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn the_user_configuration_is_saved_validated_and_pushed_live() {
    let server = Server::start();
    let mut bridge = Bridge::start(&server);
    welcome(&mut bridge);
    let state = bridge.web(1, "state", json!({}));
    assert!(
        state["result"]["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|op| op == "config")
    );
    let path = server.root.join("xdg-config/kelpmux/config.toml");
    let initial = bridge.web(2, "config", json!({}))["result"].clone();
    assert_eq!(initial["exists"], false, "{initial}");
    assert_eq!(initial["path"], json!(path));
    assert_eq!(initial["values"]["files"]["root"], "~");
    // A save creates the documented file and keeps its comments.
    let saved = bridge.web(
        3,
        "config_set",
        json!({"changes": {"files.extra_roots": ["/"], "scratch.folder": "~/s"}}),
    );
    assert_eq!(saved["result"]["saved"], true, "{saved}");
    assert_eq!(
        saved["result"]["config"]["values"]["scratch"]["folder"],
        "~/s"
    );
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("# KelpMux user configuration."), "{text}");
    assert!(text.contains("\nfolder = \"~/s\"\n"), "{text}");
    // An invalid change is refused with its problems, and nothing changes.
    let refused = bridge.web(
        4,
        "config_set",
        json!({"changes": {"projects.base": "relative/path"}}),
    );
    assert_eq!(refused["result"]["saved"], false, "{refused}");
    assert_eq!(refused["result"]["errors"][0]["key"], "projects.base");
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    let unknown = bridge.web(5, "config_set", json!({"changes": {"files.nope": "x"}}));
    assert!(unknown["error"].as_str().unwrap().contains("files.nope"));
    // A hand edit with a typo reaches the browser as a `Config` notice
    // with its position; the last valid configuration stays in effect.
    fs::write(
        &path,
        text.replace("\nfolder = \"~/s\"\n", "\nfolder = \"~/s\"\nfloder = 1\n"),
    )
    .unwrap();
    let notice = bridge.expect("a Config notice", |v| {
        v["Config"]["errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty())
    });
    assert_eq!(notice["Config"]["current"], false);
    assert_eq!(notice["Config"]["values"]["scratch"]["folder"], "~/s");
    let message = notice["Config"]["errors"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("did you mean `scratch.folder`"),
        "{notice}"
    );
    // Fixed: the notice says so.
    fs::write(&path, "[scratch]\nfolder = \"~/t\"\n").unwrap();
    let fixed = bridge.expect("the fixed configuration", |v| {
        v["Config"]["values"]["scratch"]["folder"] == "~/t"
    });
    assert_eq!(fixed["Config"]["errors"], json!([]));
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
    assert_eq!(state["result"]["api_minor"], 10);
    let operations = state["result"]["operations"].as_array().unwrap();
    assert!(operations.iter().any(|op| op == "repos"));
    assert!(operations.iter().any(|op| op == "commands"));
    assert!(operations.iter().any(|op| op == "uploads"));
    assert!(operations.iter().any(|op| op == "files"));
    assert!(operations.iter().any(|op| op == "downloads"));

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
    assert_eq!(welcome["startup_panes"], "none", "{welcome}");
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

/// Files dropped onto a terminal in the browser reach the server through
/// the relayed stream: the bridge forwards the upload requests and every
/// event of the batch unchanged, keeping the server's flow-control window,
/// and the files land where the shells run.
#[test]
fn uploads_are_relayed_end_to_end_with_flow_control() {
    use base64::Engine as _;
    let server = Server::start();
    let mut bridge = Bridge::start(&server);
    let welcome = welcome(&mut bridge);
    assert!(
        welcome["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "uploads"),
        "{welcome}"
    );
    let limits = &welcome["upload_limits"];
    let chunk = limits["max_chunk_bytes"].as_u64().unwrap() as usize;
    let window = limits["max_chunks_in_flight"].as_u64().unwrap() as usize;
    assert!(chunk > 0 && window > 0, "{welcome}");

    // Into a destination folder: a file of several chunks, one with a name
    // that needs shell quoting and an empty file (which needs no chunk).
    let dest = server.root.join("dropped");
    fs::create_dir(&dest).unwrap();
    let big: Vec<u8> = (0..chunk * 3 + 17).map(|i| (i % 251) as u8).collect();
    let files = [
        ("photo.png", big.clone()),
        ("it's here.txt", b"hello\n".to_vec()),
        ("empty", Vec::new()),
    ];
    bridge.send(json!({"Stream": {
        "type": "upload_begin",
        "request": "upload-1-begin",
        "files": files.iter().map(|(name, data)| json!({"name": name, "size": data.len()})).collect::<Vec<_>>(),
        "dest": dest.to_str().unwrap(),
    }}));
    let started = bridge.expect("upload started", |v| {
        v["Stream"]["type"] == "upload_started" && v["Stream"]["request"] == "upload-1-begin"
    });
    let batch = started["Stream"]["batch"].as_u64().unwrap();
    let dir = PathBuf::from(started["Stream"]["dir"].as_str().unwrap());
    assert_eq!(dir.parent(), Some(dest.as_path()), "{started}");

    let chunks: Vec<(usize, u64, &[u8])> = files
        .iter()
        .enumerate()
        .flat_map(|(index, (_, data))| {
            data.chunks(chunk)
                .enumerate()
                .map(move |(n, part)| (index, (n * chunk) as u64, part))
        })
        .collect();
    // Never more than the window unacknowledged; each progress allows one
    // more chunk.
    let mut sent = 0;
    let mut acknowledged = 0;
    let mut received = vec![0u64; files.len()];
    while acknowledged < chunks.len() {
        while sent < chunks.len() && sent - acknowledged < window {
            let (index, offset, part) = chunks[sent];
            bridge.send(json!({"Stream": {
                "type": "upload_chunk",
                "batch": batch,
                "file_index": index,
                "offset": offset,
                "data": base64::engine::general_purpose::STANDARD.encode(part),
            }}));
            sent += 1;
        }
        let progress = bridge.expect("upload progress", |v| {
            matches!(
                v["Stream"]["type"].as_str(),
                Some("upload_progress" | "upload_failed")
            )
        });
        assert_eq!(progress["Stream"]["type"], "upload_progress", "{progress}");
        assert_eq!(progress["Stream"]["batch"], batch);
        let index = progress["Stream"]["file_index"].as_u64().unwrap() as usize;
        assert_eq!(index, chunks[acknowledged].0);
        received[index] = progress["Stream"]["received"].as_u64().unwrap();
        acknowledged += 1;
    }
    assert_eq!(received[0], big.len() as u64);
    assert_eq!(received[1], 6);

    bridge.send(
        json!({"Stream": {"type": "upload_finish", "request": "upload-1-finish", "batch": batch}}),
    );
    let done = bridge.expect("upload done", |v| {
        v["Stream"]["type"] == "upload_done" && v["Stream"]["request"] == "upload-1-finish"
    });
    let paths: Vec<PathBuf> = done["Stream"]["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| PathBuf::from(p.as_str().unwrap()))
        .collect();
    assert_eq!(
        paths,
        vec![
            dir.join("photo.png"),
            dir.join("it's here.txt"),
            dir.join("empty")
        ]
    );
    for (path, (_, data)) in paths.iter().zip(&files) {
        assert_eq!(&fs::read(path).unwrap(), data, "{}", path.display());
    }

    // Without a destination the batch lands in the server's upload root;
    // a cancel removes it and is acknowledged with `done`.
    bridge.send(json!({"Stream": {
        "type": "upload_begin",
        "request": "upload-2-begin",
        "files": [{"name": "later.bin", "size": 10}],
    }}));
    let started = bridge.expect("second upload started", |v| {
        v["Stream"]["type"] == "upload_started" && v["Stream"]["request"] == "upload-2-begin"
    });
    let second = started["Stream"]["batch"].as_u64().unwrap();
    let root_dir = PathBuf::from(started["Stream"]["dir"].as_str().unwrap());
    assert!(root_dir.starts_with(server.root.join("cache")), "{started}");
    assert!(root_dir.is_dir());
    bridge.send(
        json!({"Stream": {"type": "upload_cancel", "request": "upload-2-cancel", "batch": second}}),
    );
    bridge.expect("cancelled", |v| {
        v["Stream"]["type"] == "done" && v["Stream"]["request"] == "upload-2-cancel"
    });
    assert!(!root_dir.exists());

    // A chunk at the wrong offset fails the batch; refusals of a begin are
    // errors correlated with its request.
    bridge.send(json!({"Stream": {
        "type": "upload_begin",
        "request": "upload-3-begin",
        "files": [{"name": "a", "size": 4}],
    }}));
    let started = bridge.expect("third upload started", |v| {
        v["Stream"]["type"] == "upload_started" && v["Stream"]["request"] == "upload-3-begin"
    });
    let third = started["Stream"]["batch"].as_u64().unwrap();
    bridge.send(json!({"Stream": {
        "type": "upload_chunk", "batch": third, "file_index": 0, "offset": 2, "data": "YWI="
    }}));
    let failed = bridge.expect("upload failed", |v| v["Stream"]["type"] == "upload_failed");
    assert_eq!(failed["Stream"]["batch"], third);
    bridge.send(json!({"Stream": {
        "type": "upload_begin",
        "request": "upload-4-begin",
        "files": [{"name": "a", "size": 1}],
        "dest": "relative/dir",
    }}));
    bridge.expect("refused destination", |v| {
        v["Stream"]["type"] == "error" && v["Stream"]["request"] == "upload-4-begin"
    });
}

/// After the browser opts in to `binary-screens`, the bridge forwards each
/// update's binary screen record right after it, as the server framed it,
/// and keeps relaying JSON (leases, `Web` replies) as before.
#[test]
fn binary_screen_records_are_forwarded_unchanged() {
    use kelpclient_protocol::screens::{self, ScreenMessage};
    let server = Server::start();
    let mut bridge = Bridge::start(&server);
    let welcome = welcome(&mut bridge);
    let client = welcome["client"].as_u64().unwrap();
    assert!(
        welcome["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "binary-screens")
    );
    let created = bridge.web(
        1,
        "new",
        json!({"request": "web-binary-1", "cwd": server.root.to_str().unwrap()}),
    );
    let pane = created["result"]["pane"]
        .as_u64()
        .unwrap_or_else(|| panic!("{created}"));
    bridge.send(json!({"Stream": {"type": "binary_screens", "enabled": true}}));
    bridge.send(json!({"Stream": {"type": "subscribe", "panes": [pane]}}));
    bridge.send(json!({"Stream": {"type": "claim", "pane": pane, "takeover": true}}));
    bridge.expect("lease", |v| {
        v["Stream"]["type"] == "lease"
            && v["Stream"]["pane"] == pane
            && v["Stream"]["owner"] == client
    });
    bridge.send(json!({"Stream": {"type": "resize", "pane": pane, "cols": 60, "rows": 12}}));
    let typed = b"echo kelp-binary-marker\n".to_vec();
    bridge.send(json!({"Stream": {"type": "input", "pane": pane, "bytes": typed}}));
    // `Web` replies come from another bridge thread; none may land
    // between an update and its record.
    for id in 2..=21 {
        bridge.send(json!({"Web": {"id": id, "method": "state", "params": {}}}));
    }
    // Reassemble the pane's screen from snapshots and patches, whichever
    // format carries them, until the echoed marker shows.
    let mut cells: Vec<String> = Vec::new();
    let mut records = 0;
    let mut awaiting = false;
    let mut replies = 0;
    let mut marked = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("timed out waiting for the echoed marker in binary screens");
        let value = bridge.events.recv_timeout(remaining).expect("bridge ended");
        if let Some(bytes) = value.get("Binary") {
            assert!(awaiting, "a binary record follows its update");
            awaiting = false;
            records += 1;
            let bytes: Vec<u8> = serde_json::from_value(bytes.clone()).unwrap();
            for message in screens::decode(&bytes, screens::DEFAULT_MAX_CELLS).unwrap() {
                match message {
                    ScreenMessage::Snapshot(s) if s.pane == pane => {
                        cells = s.screen.cells.iter().map(|c| c.text.clone()).collect();
                    }
                    ScreenMessage::Patch(p) if p.pane == pane => {
                        for (index, cell) in p.into_pane_patch().changes {
                            cells[index] = cell.text;
                        }
                    }
                    _ => {}
                }
            }
            marked |= cells.concat().matches("kelp-binary-marker").count() >= 2;
            if marked && replies == 20 {
                break;
            }
            continue;
        }
        assert!(!awaiting, "nothing comes between an update and its record");
        if value.get("Web").is_some() {
            replies += 1;
            if marked && replies == 20 {
                break;
            }
        }
        if value["Stream"]["type"] == "update" && value["Stream"]["binary_screens"] == true {
            assert_eq!(value["Stream"]["snapshots"], json!([]));
            assert_eq!(value["Stream"]["patches"], json!([]));
            awaiting = true;
        }
    }
    assert!(records > 0);
    // JSON keeps flowing: a `Web` reply after the binary records.
    let state = bridge.web(100, "state", json!({}));
    assert_eq!(state["result"]["api_version"], 3);
}

/// The file browser and editor in the browser reach the server through the
/// relayed stream (`files`, bridge API minor 2): the bridge forwards every
/// `file_*` request and reply unchanged, Welcome's `file_limits` survives
/// its re-serialisation, and the files change where the shells run.
#[test]
fn file_requests_and_replies_are_relayed_unchanged() {
    use base64::Engine as _;
    let server = Server::start();
    let mut bridge = Bridge::start(&server);
    let welcome = welcome(&mut bridge);
    assert!(
        welcome["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "files"),
        "{welcome}"
    );
    // Every limit the server announces reaches the browser.
    let limits = &welcome["file_limits"];
    for key in [
        "max_read_bytes",
        "max_write_bytes",
        "max_list_entries",
        "max_paths",
        "max_in_flight",
        "max_path_bytes",
        "max_depth",
    ] {
        assert!(
            limits[key].as_u64().is_some_and(|n| n > 0),
            "{key}: {welcome}"
        );
    }
    assert_eq!(limits["max_read_bytes"], 8 * 1024 * 1024);
    let state = bridge.web(1, "state", json!({}));
    assert_eq!(state["result"]["api_minor"], 10);
    assert!(
        state["result"]["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|op| op == "files")
    );

    let root = server.root.to_str().unwrap().to_owned();
    let reply = |bridge: &mut Bridge, request: Value| {
        let id = request["request"].as_str().unwrap().to_owned();
        bridge.send(json!({ "Stream": request }));
        bridge.expect(&id.clone(), move |v| v["Stream"]["request"] == id.as_str())["Stream"].clone()
    };
    let docs = format!("{root}/docs");
    let done = reply(
        &mut bridge,
        json!({"type": "file_mkdir", "request": "file-1", "path": docs}),
    );
    assert_eq!(done["type"], "done", "{done}");
    let note = format!("{docs}/it's a note.txt");
    let written = reply(
        &mut bridge,
        json!({"type": "file_write", "request": "file-2", "path": note, "content": "héllo\n", "create": true}),
    );
    assert_eq!(written["type"], "file_written", "{written}");
    assert_eq!(written["created"], true);
    assert_eq!(written["size"], 7);
    let first = written["hash"].as_str().unwrap().to_owned();
    assert_eq!(fs::read_to_string(&note).unwrap(), "héllo\n");

    let listed = reply(
        &mut bridge,
        json!({"type": "file_list", "request": "file-3", "path": docs}),
    );
    assert_eq!(listed["type"], "file_list", "{listed}");
    assert_eq!(listed["path"], docs.as_str());
    assert_eq!(listed["truncated"], false);
    let entry = &listed["entries"][0];
    assert_eq!(entry["name"], "it's a note.txt", "{listed}");
    assert_eq!(entry["kind"], "file");
    assert_eq!(entry["size"], 7);
    assert!(entry["modified"].as_i64().is_some() && entry["mode"].as_u64().is_some());

    let stat = reply(
        &mut bridge,
        json!({"type": "file_stat", "request": "file-4", "path": note}),
    );
    assert_eq!(stat["entry"]["name"], "it's a note.txt", "{stat}");
    let read = reply(
        &mut bridge,
        json!({"type": "file_read", "request": "file-5", "path": note}),
    );
    assert_eq!(read["type"], "file_content", "{read}");
    assert_eq!(read["content"], "héllo\n");
    assert_eq!(read["encoding"], "utf8");
    assert_eq!(read["binary"], false);
    assert_eq!(read["hash"], first.as_str());

    // A change on disk since the read is a conflict carrying the new hash;
    // the browser then overwrites without a base hash.
    fs::write(&note, "changed on disk\n").unwrap();
    let conflict = reply(
        &mut bridge,
        json!({"type": "file_write", "request": "file-6", "path": note, "content": "mine\n", "base_hash": first}),
    );
    assert_eq!(conflict["type"], "file_error", "{conflict}");
    assert_eq!(conflict["error"]["code"], "conflict");
    assert_eq!(
        conflict["error"]["current_hash"].as_str().unwrap().len(),
        64
    );
    assert_eq!(fs::read_to_string(&note).unwrap(), "changed on disk\n");
    let overwritten = reply(
        &mut bridge,
        json!({"type": "file_write", "request": "file-7", "path": note, "content": "mine\n"}),
    );
    assert_eq!(overwritten["type"], "file_written", "{overwritten}");
    assert_eq!(fs::read_to_string(&note).unwrap(), "mine\n");

    // Binary content travels as base64 both ways.
    let blob = format!("{docs}/blob.bin");
    let bytes = [0u8, 159, 146, 150, 255];
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let written = reply(
        &mut bridge,
        json!({"type": "file_write", "request": "file-8", "path": blob, "content": encoded, "encoding": "base64", "create": true}),
    );
    assert_eq!(written["type"], "file_written", "{written}");
    let read = reply(
        &mut bridge,
        json!({"type": "file_read", "request": "file-9", "path": blob}),
    );
    assert_eq!(read["binary"], true, "{read}");
    assert_eq!(read["encoding"], "base64");
    assert_eq!(read["content"], encoded.as_str());

    let moved = format!("{root}/moved.txt");
    let done = reply(
        &mut bridge,
        json!({"type": "file_rename", "request": "file-10", "from": note, "to": moved}),
    );
    assert_eq!(done["type"], "done", "{done}");
    assert_eq!(fs::read_to_string(&moved).unwrap(), "mine\n");
    let refused = reply(
        &mut bridge,
        json!({"type": "file_read", "request": "file-11", "path": docs}),
    );
    assert_eq!(refused["error"]["code"], "is_a_directory", "{refused}");
    assert_eq!(refused["error"]["path"], docs.as_str());

    let trashed = reply(
        &mut bridge,
        json!({"type": "file_trash", "request": "file-12", "paths": [moved]}),
    );
    assert_eq!(trashed["type"], "file_removed", "{trashed}");
    let trash = server.root.join("data/Trash/files/moved.txt");
    assert_eq!(
        trashed["removed"][0]["trashed_as"],
        trash.to_str().unwrap(),
        "{trashed}"
    );
    assert_eq!(fs::read_to_string(&trash).unwrap(), "mine\n");
    let deleted = reply(
        &mut bridge,
        json!({"type": "file_delete", "request": "file-13", "paths": [docs, format!("{root}/missing")]}),
    );
    assert_eq!(deleted["removed"], json!([{ "path": docs }]), "{deleted}");
    assert_eq!(deleted["failed"][0]["error"]["code"], "not_found");
    assert!(!server.root.join("docs").exists());
    // Pane traffic keeps flowing on the same relayed connection.
    let created = bridge.web(2, "new", json!({"request": "web-file-1"}));
    assert!(created["result"]["pane"].as_u64().is_some(), "{created}");
}

/// Downloads (`file-downloads`, bridge API minor 3): the bridge relays the
/// begin, every chunk and its acknowledgement, and the end unchanged, and
/// Welcome's `file_limits.downloads` survives its re-serialisation.
#[test]
fn downloads_are_relayed_with_their_acknowledgements() {
    use base64::Engine as _;
    let server = Server::start();
    let mut bridge = Bridge::start(&server);
    let welcome = welcome(&mut bridge);
    assert!(
        welcome["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "file-downloads"),
        "{welcome}"
    );
    let limits = &welcome["file_limits"]["downloads"];
    for key in [
        "chunk_bytes",
        "max_chunks_in_flight",
        "max_downloads",
        "max_archive_entries",
        "max_archive_bytes",
    ] {
        assert!(
            limits[key].as_u64().is_some_and(|n| n > 0),
            "{key}: {welcome}"
        );
    }
    let state = bridge.web(1, "state", json!({}));
    assert!(
        state["result"]["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|op| op == "downloads")
    );
    // Three chunks' worth, so acknowledgements matter.
    let chunk = limits["chunk_bytes"].as_u64().unwrap() as usize;
    let content: Vec<u8> = (0..chunk * 2 + 1000).map(|i| (i * 7 % 251) as u8).collect();
    let path = server.root.join("data.bin");
    fs::write(&path, &content).unwrap();
    bridge.send(json!({"Stream": {"type": "file_download_begin", "request": "file-d1", "paths": [path.to_str().unwrap()]}}));
    let started =
        bridge.expect("started", |v| v["Stream"]["request"] == "file-d1")["Stream"].clone();
    assert_eq!(started["type"], "file_download_started", "{started}");
    assert_eq!(started["name"], "data.bin");
    assert_eq!(started["size"], content.len());
    let download = started["download"].as_u64().unwrap();
    let mut received = Vec::new();
    loop {
        let event =
            bridge.expect("download event", |v| v["Stream"]["download"] == download)["Stream"]
                .clone();
        match event["type"].as_str().unwrap() {
            "file_download_chunk" => {
                assert_eq!(event["offset"], received.len());
                received.extend(
                    base64::engine::general_purpose::STANDARD
                        .decode(event["data"].as_str().unwrap())
                        .unwrap(),
                );
                bridge.send(json!({"Stream": {"type": "file_download_ack", "download": download, "offset": received.len()}}));
            }
            "file_download_done" => {
                assert_eq!(event["size"], content.len(), "{event}");
                assert_eq!(event["hash"].as_str().unwrap().len(), 64);
                break;
            }
            other => panic!("{other}: {event}"),
        }
    }
    assert_eq!(received, content);
    // A folder comes as a gzip stream; a cancel is acknowledged.
    fs::create_dir(server.root.join("folder")).unwrap();
    fs::write(server.root.join("folder/a.txt"), "a").unwrap();
    bridge.send(json!({"Stream": {"type": "file_download_begin", "request": "file-d2", "paths": [server.root.join("folder").to_str().unwrap()], "archive": "tar.gz"}}));
    let started =
        bridge.expect("archive", |v| v["Stream"]["request"] == "file-d2")["Stream"].clone();
    assert_eq!(started["name"], "folder.tar.gz", "{started}");
    assert_eq!(started["archive"], "tar.gz");
    let download = started["download"].as_u64().unwrap();
    let first = bridge.expect("chunk", |v| v["Stream"]["download"] == download)["Stream"].clone();
    let gzip = base64::engine::general_purpose::STANDARD
        .decode(first["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(&gzip[..2], [0x1f, 0x8b]);
    bridge.send(json!({"Stream": {"type": "file_download_cancel", "request": "file-d3", "download": download}}));
    let done = bridge.expect("cancel", |v| v["Stream"]["request"] == "file-d3")["Stream"].clone();
    assert_eq!(done["type"], "done", "{done}");
}

/// Polls `environments` until `name` reaches `state`.
fn environment_state(bridge: &mut Bridge, id: &mut u64, name: &str, state: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        *id += 1;
        let reply = bridge.web(*id, "environments", json!({"project": "work"}));
        let entry = reply["result"]["environments"]
            .as_array()
            .unwrap_or_else(|| panic!("{reply}"))
            .iter()
            .find(|e| e["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no {name}: {reply}"));
        if entry["state"] == state {
            return entry;
        }
        assert!(Instant::now() < deadline, "{name} never {state}: {entry}");
        thread::sleep(Duration::from_millis(100));
    }
}

fn declare(folder: &std::path::Path, name: &str, manifest: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = folder.join(".kelp/environments").join(name);
    fs::create_dir_all(&dir).unwrap();
    for path in [
        folder.join(".kelp"),
        folder.join(".kelp/environments"),
        dir.clone(),
    ] {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(dir.join("environment.toml"), manifest).unwrap();
    dir
}

/// Environments (API 3.8, ADR 0005): a process environment runs in its own
/// pane of the project with the `KELP_*` variables, is Starting until its
/// port answers, then Running, and Stop ends it with SIGTERM and closes its
/// pane; a deploy environment runs `up` and `down` in panes and is
/// `unknown` until an explicit `environment_check` runs its `status`
/// script, which listing never does. Nothing starts without a request.
#[test]
fn environments_start_report_and_stop() {
    use std::os::unix::fs::PermissionsExt;
    let server = Server::start();
    let mut bridge = Bridge::start(&server);
    welcome(&mut bridge);
    let state = bridge.web(1, "state", json!({}));
    assert_eq!(state["result"]["api_minor"], 10);
    assert!(
        state["result"]["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|op| op == "environments")
    );
    let created = bridge.web(2, "project_create", json!({"name": "Work"}));
    assert_eq!(created["result"]["saved"], true, "{created}");
    let saved = bridge.web(
        3,
        "config_set",
        json!({"changes": {"environments.ports": "45100-45199"}}),
    );
    assert_eq!(saved["result"]["saved"], true, "{saved}");
    let folder = server.root.join("projects/Work");
    // The port the process environment's readiness waits for: this test
    // listens on it once the environment has started.
    let ready = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    declare(
        &folder,
        "web",
        &format!(
            "kind = \"process\"\ndescription = \"Dev server\"\nurl = \"http://localhost:${{KELP_PORT}}/\"\ncommand = \"echo \\\"$KELP_PROJECT $KELP_ENV $KELP_SLUG $KELP_PORT $PWD\\\" > \\\"$KELP_STATE_DIR/vars\\\"; trap 'echo term > \\\"$KELP_STATE_DIR/term\\\"; exit 0' TERM; while :; do sleep 1; done\"\n[ready]\nport = {ready}\n"
        ),
    );
    let deploy = declare(
        &folder,
        "staging",
        "kind = \"deploy\"\n[scripts]\nup = \"up\"\ndown = \"down\"\nstatus = \"status\"\n",
    );
    // The status script leaves a marker each time it runs.
    let marker = server.root.join("status-ran");
    let status = format!(
        "touch '{}'\nif [ -f \"$KELP_STATE_DIR/deployed\" ]; then echo '{{\"state\":\"running\",\"url\":\"https://work-staging.example.test/\"}}'; else echo '{{\"state\":\"stopped\",\"detail\":\"not deployed\"}}'; fi",
        marker.display()
    );
    for (script, body) in [
        ("up", "touch \"$KELP_STATE_DIR/deployed\""),
        ("down", "rm -f \"$KELP_STATE_DIR/deployed\""),
        ("status", status.as_str()),
    ] {
        fs::write(deploy.join(script), format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(deploy.join(script), fs::Permissions::from_mode(0o755)).unwrap();
    }
    // A broken manifest is listed with its problem.
    declare(&folder, "broken", "kind = \"process\"\n");

    let mut id = 10;
    let listed = bridge.web(4, "environments", json!({"project": "work"}));
    let names: Vec<&str> = listed["result"]["environments"]
        .as_array()
        .unwrap_or_else(|| panic!("{listed}"))
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["broken", "staging", "web"], "{listed}");
    let broken = &listed["result"]["environments"][0];
    assert_eq!(broken["state"], "invalid");
    assert!(
        broken["error"].as_str().unwrap().contains("command"),
        "{broken}"
    );
    // Listed, never started: no port yet, stopped.
    let web = environment_state(&mut bridge, &mut id, "web", "stopped");
    assert_eq!(web["kind"], "process");
    assert!(web["port"].is_null());
    // Listing never runs repository code: a deploy environment is unknown
    // until it is checked on request.
    let staging = environment_state(&mut bridge, &mut id, "staging", "unknown");
    assert!(staging["checked"].is_null(), "{staging}");
    for _ in 0..3 {
        environment_state(&mut bridge, &mut id, "staging", "unknown");
    }
    assert!(!marker.exists(), "listing ran the status script");
    let checked = check(&mut bridge, &mut id, "staging");
    assert!(marker.exists(), "the explicit check did not run it");
    assert_eq!(checked["state"], "stopped", "{checked}");
    assert_eq!(checked["detail"], "not deployed");
    assert!(checked["checked"]["at"].as_u64().is_some(), "{checked}");
    // The listing shows the last check, with its time, as unknown.
    fs::remove_file(&marker).unwrap();
    let listed = environment_state(&mut bridge, &mut id, "staging", "unknown");
    assert_eq!(listed["checked"]["state"], "stopped", "{listed}");
    assert!(!marker.exists());
    // Refused: Scratch, an unknown project, an invalid name, no request.
    id += 1;
    let scratch = bridge.web(id, "environments", json!({"project": "scratch"}));
    assert_eq!(scratch["error"], "Scratch has no environments");
    let invalid = bridge.web(
        {
            id += 1;
            id
        },
        "environment_start",
        json!({"project": "work", "name": "../x", "request": "r1"}),
    );
    assert_eq!(invalid["error"], "environment name required");
    let broken = bridge.web(
        {
            id += 1;
            id
        },
        "environment_start",
        json!({"project": "work", "name": "broken", "request": "r2"}),
    );
    assert!(
        broken["error"].as_str().unwrap().contains("cannot run"),
        "{broken}"
    );

    // Start the process environment.
    let started = bridge.web(
        {
            id += 1;
            id
        },
        "environment_start",
        json!({"project": "work", "name": "web", "request": "env-web-1"}),
    );
    let pane = started["result"]["pane"]
        .as_u64()
        .unwrap_or_else(|| panic!("{started}"));
    assert_eq!(started["result"]["port"], 45100);
    assert_eq!(started["result"]["slug"], "work-web");
    let starting = environment_state(&mut bridge, &mut id, "web", "starting");
    assert_eq!(starting["pane"], pane);
    assert_eq!(starting["url"], "http://localhost:45100/");
    let again = bridge.web(
        {
            id += 1;
            id
        },
        "environment_start",
        json!({"project": "work", "name": "web", "request": "env-web-2"}),
    );
    assert!(
        again["error"].as_str().unwrap().contains("already running"),
        "{again}"
    );
    // Its pane belongs to the project and is marked as an environment.
    id += 1;
    let panes = bridge.web(id, "environment_panes", json!({}));
    assert_eq!(
        panes["result"]["panes"][pane.to_string()]["env"],
        "web",
        "{panes}"
    );
    id += 1;
    let links = bridge.web(id, "projects", json!({}));
    assert_eq!(links["result"]["panes"][pane.to_string()], "work");
    let state_dir = server.root.join("xdg-state/kelpmux/environments/work/web");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !state_dir.join("vars").exists() {
        assert!(Instant::now() < deadline, "the command never ran");
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        fs::read_to_string(state_dir.join("vars")).unwrap().trim(),
        format!("work web work-web 45100 {}", folder.display())
    );
    let listener = std::net::TcpListener::bind(("127.0.0.1", ready)).unwrap();
    environment_state(&mut bridge, &mut id, "web", "running");
    drop(listener);
    // Stop: SIGTERM reaches the command, the pane closes.
    id += 1;
    let stopped = bridge.web(
        id,
        "environment_stop",
        json!({"project": "work", "name": "web", "request": "env-web-stop"}),
    );
    assert_eq!(stopped["result"]["state"], "stopped", "{stopped}");
    assert!(
        state_dir.join("term").exists(),
        "the command got no SIGTERM"
    );
    let web = environment_state(&mut bridge, &mut id, "web", "stopped");
    assert!(web["pane"].is_null(), "{web}");
    assert_eq!(web["port"], 45100, "the port stays allocated");
    id += 1;
    let panes = bridge.web(id, "environment_panes", json!({}));
    assert_eq!(panes["result"]["panes"], json!({}));

    // The deploy environment: `up` in a pane; its state is unknown until
    // an explicit check runs `status`.
    id += 1;
    let up = bridge.web(
        id,
        "environment_start",
        json!({"project": "work", "name": "staging", "request": "env-staging-up"}),
    );
    assert_eq!(up["result"]["action"], "up", "{up}");
    assert_eq!(up["result"]["port"], 45101);
    let done = environment_state(&mut bridge, &mut id, "staging", "unknown");
    assert_eq!(done["checked"]["state"], "stopped", "the last check stays");
    let running = check(&mut bridge, &mut id, "staging");
    assert_eq!(running["state"], "running", "{running}");
    assert_eq!(running["url"], "https://work-staging.example.test/");
    assert_eq!(running["checked"]["state"], "running");
    id += 1;
    let down = bridge.web(
        id,
        "environment_stop",
        json!({"project": "work", "name": "staging", "request": "env-staging-down"}),
    );
    assert_eq!(down["result"]["action"], "down", "{down}");
    environment_state(&mut bridge, &mut id, "staging", "unknown");
    assert_eq!(check(&mut bridge, &mut id, "staging")["state"], "stopped");
}

/// Runs a deploy environment's `status` script: `environment_check`.
fn check(bridge: &mut Bridge, id: &mut u64, name: &str) -> Value {
    *id += 1;
    let reply = bridge.web(
        *id,
        "environment_check",
        json!({"project": "work", "name": name}),
    );
    assert!(reply["result"].is_object(), "{reply}");
    reply["result"].clone()
}
