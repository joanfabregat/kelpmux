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
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "kelpmux-web-bridge-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
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
            let shutdown = serde_json::to_vec(&json!({"Command": "Shutdown"})).unwrap();
            let _ = socket.write_all(&(shutdown.len() as u32).to_be_bytes());
            let _ = socket.write_all(&shutdown);
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
    input: ChildStdin,
    events: mpsc::Receiver<Value>,
}
impl Bridge {
    fn start(server: &Server) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
            .arg("web-bridge")
            .arg("--socket")
            .arg(server.socket())
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
            .any(|c| c == "pane-spawn")
    );

    let state = bridge.web(1, "state", json!({}));
    assert_eq!(state["result"]["api_version"], 1);
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

    // Notification read flags belong to the TUI.
    bridge.send(json!({"Stream": {"type": "acknowledge", "notification": 1}}));
    bridge.expect("refusal", |v| {
        v["Stream"]["type"] == "error"
            && v["Stream"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("not available"))
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
    use kelpmux::native::protocol::{
        ClientMessage, ServerMessage, VERSION, read_message, write_message,
    };
    let server = Server::start();
    let mut tui = UnixStream::connect(server.socket()).unwrap();
    tui.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write_message(
        &mut tui,
        &ClientMessage::Attach {
            version: VERSION,
            cols: 100,
            rows: 30,
            takeover: false,
        },
    )
    .unwrap();
    match read_message::<ServerMessage>(&mut tui).unwrap().unwrap() {
        ServerMessage::Welcome { controller, .. } => assert!(controller),
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

    // Typing in the TUI takes the pane back and restores its base geometry.
    write_message(&mut tui, &ClientMessage::Input(b"echo from-tui\n".to_vec())).unwrap();
    bridge.expect("lease returned to the TUI", |v| {
        v["Stream"]["type"] == "lease"
            && v["Stream"]["pane"] == pane
            && v["Stream"]["owner"].is_null()
    });
    assert_eq!(size(&mut bridge), base);

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
