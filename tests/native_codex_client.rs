//! Automatic ownership fixtures use synthetic clients/daemon, never real agents.
use kelpmux::{
    discovery::{Discovery, polling::Poller},
    model::{Status, StatusSource},
    process,
    tmux::Pane,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, Write},
    os::unix::{
        fs::{PermissionsExt, symlink},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tungstenite::Message;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    children: Vec<Child>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "km-codex-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir_all(root.join("codex/app-server-control")).unwrap();
        fs::create_dir(root.join("native")).unwrap();
        fs::create_dir(root.join("bin")).unwrap();
        symlink(std::env::current_exe().unwrap(), root.join("native/codex")).unwrap();
        fs::write(root.join("bin/codex"), format!("#!/bin/sh\nexport FIXTURE_ENDPOINT=\"${{2#unix://}}\"\nexec '{}' --ignored --exact synthetic_codex_tui --nocapture\n", root.join("native/codex").display())).unwrap();
        fs::set_permissions(root.join("bin/codex"), fs::Permissions::from_mode(0o700)).unwrap();
        let listener =
            UnixListener::bind(root.join("codex/app-server-control/app-server-control.sock"))
                .unwrap();
        listener.set_nonblocking(true).unwrap();
        fs::write(root.join("status"), "active").unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let state = root.clone();
        let worker = thread::spawn(move || {
            let mut workers = vec![];
            while !done.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let state = state.clone();
                        let done = done.clone();
                        workers.push(thread::spawn(move || {
                        stream.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
                        let Ok(mut socket) = tungstenite::accept(stream) else { return; };
                        while !done.load(Ordering::Relaxed) {
                            let message = match socket.read() {
                                Ok(Message::Text(text)) => text,
                                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                                _ => break,
                            };
                            let request: Value = serde_json::from_str(&message).unwrap();
                            let result = match request["method"].as_str().unwrap() {
                                "initialize" => json!({"userAgent":"fixture"}),
                                "initialized" => continue,
                                "thread/start" | "thread/resume" => json!({"thread":{"id":request["params"]["threadId"]}}),
                                "thread/read" => {
                                    assert_eq!(request["params"]["includeTurns"], false);
                                    let status = fs::read_to_string(state.join("status")).unwrap();
                                    json!({"thread":{"id":request["params"]["threadId"],"name":"Synthetic","status":{"type":if status=="waiting" {"active"} else {&status},"activeFlags":if status=="waiting" {vec!["waitingOnApproval"]} else {vec![]}}}})
                                }
                                other => panic!("unexpected method {other}"),
                            };
                            if socket.send(Message::Text(json!({"id":request["id"],"result":result}).to_string().into())).is_err() { break; }
                        }
                    }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("{e}"),
                }
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        Self {
            root,
            stop,
            worker: Some(worker),
            children: vec![],
        }
    }
    fn launch(&mut self, id: &str) -> Pane {
        let child = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
            .args(["codex-client", "--codex"])
            .arg(self.root.join("bin/codex"))
            .arg("--")
            .env("KELPMUX_PANE", format!("%{}", self.children.len() + 1))
            .env("CODEX_HOME", self.root.join("codex"))
            .env("FIXTURE_ROOT", &self.root)
            .env("FIXTURE_THREAD", id)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pane = Pane {
            id: format!("%{}", self.children.len() + 1),
            window: "@0".into(),
            pid: child.id(),
            index: self.children.len() + 1,
            cwd: self.root.to_string_lossy().into(),
            dead: false,
            floating: false,
        };
        self.children.push(child);
        pane
    }
    fn poller(&self) -> Poller {
        Poller::new(Discovery {
            proc_root: "/proc".into(),
            codex_home: self.root.join("codex"),
            claude_home: self.root.join("claude"),
        })
    }
}
#[test]
#[ignore = "synthetic child entry point; invoked only by fixture"]
fn synthetic_codex_tui() {
    let root = PathBuf::from(std::env::var_os("FIXTURE_ROOT").unwrap());
    let mut id = std::env::var("FIXTURE_THREAD").unwrap();
    let stream = UnixStream::connect(std::env::var("FIXTURE_ENDPOINT").unwrap()).unwrap();
    let (mut socket, _) = tungstenite::client("ws://localhost/", stream).unwrap();
    for (seq, method, params) in [
        (1, "initialize", json!({})),
        (
            2,
            "thread/start",
            json!({"threadId":id,"prompt":"PRIVATE_MUST_NOT_PERSIST"}),
        ),
    ] {
        socket
            .send(Message::Text(
                json!({"id":seq,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        socket.read().unwrap();
    }
    fs::write(
        root.join(format!("ready-{id}")),
        std::process::id().to_string(),
    )
    .unwrap();
    for (seq, line) in (3..).zip(std::io::stdin().lock().lines()) {
        let line = line.unwrap();
        if line == "exit" {
            break;
        }
        if line == "reconnect" {
            let _ = socket.close(None);
            let stream = UnixStream::connect(std::env::var("FIXTURE_ENDPOINT").unwrap()).unwrap();
            socket = tungstenite::client("ws://localhost/", stream).unwrap().0;
            socket
                .send(Message::Text(
                    json!({"id":0,"method":"initialize","params":{}})
                        .to_string()
                        .into(),
                ))
                .unwrap();
            socket.read().unwrap();
        } else {
            id = line;
        }
        socket
            .send(Message::Text(
                json!({"id":seq,"method":"thread/resume","params":{"threadId":id}})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        socket.read().unwrap();
        fs::write(root.join(format!("ready-{id}")), seq.to_string()).unwrap();
    }
    let _ = socket.close(None);
}
fn wait(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !check() {
        assert!(Instant::now() < deadline, "fixture timed out");
        thread::sleep(Duration::from_millis(20));
    }
}
#[test]
fn two_clients_in_one_directory_link_and_switch_automatically_with_live_status() {
    let mut fixture = Fixture::new();
    let panes = vec![
        fixture.launch("thread-first"),
        fixture.launch("thread-second"),
    ];
    wait(|| {
        fixture.root.join("ready-thread-first").exists()
            && fixture.root.join("ready-thread-second").exists()
    });
    let mut poller = fixture.poller();
    let check = |poller: &mut Poller, expected: &str, status: Status| {
        let cycle = poller.poll(&panes, &[], kelpmux::now()).unwrap();
        assert_eq!(cycle.agents.len(), 2);
        assert_eq!(cycle.agents[0].session_id.as_deref(), Some(expected));
        assert_eq!(cycle.agents[1].session_id.as_deref(), Some("thread-second"));
        assert_eq!(cycle.agents[0].status, status);
        assert_eq!(cycle.agents[0].source, "client-bridge");
        assert_eq!(cycle.agents[0].observation.source, StatusSource::AppServer);
        cycle
    };
    let cycle = check(&mut poller, "thread-first", Status::Working);
    let first_pid = cycle.agents[0].pid;
    let link = process::environment(Path::new("/proc"), first_pid, "KELPMUX_CODEX_LINK").unwrap();
    assert!(
        !fs::read_to_string(&link)
            .unwrap()
            .contains("PRIVATE_MUST_NOT_PERSIST")
    );
    fs::write(fixture.root.join("status"), "waiting").unwrap();
    check(&mut poller, "thread-first", Status::InputNeeded);
    fs::write(fixture.root.join("status"), "active").unwrap();
    fixture.children[0]
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"thread-switched\n")
        .unwrap();
    wait(|| fixture.root.join("ready-thread-switched").exists());
    check(&mut poller, "thread-switched", Status::Working);
    fs::remove_file(fixture.root.join("ready-thread-switched")).unwrap();
    fixture.children[0]
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"reconnect\n")
        .unwrap();
    wait(|| fixture.root.join("ready-thread-switched").exists());
    check(&mut poller, "thread-switched", Status::Working);
    fs::write(fixture.root.join("status"), "idle").unwrap();
    check(&mut poller, "thread-switched", Status::Idle);
    fixture.children[0]
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"exit\n")
        .unwrap();
    wait(|| fixture.children[0].try_wait().unwrap().is_some());
    assert!(!Path::new(&link).exists());
    assert!(
        poller
            .poll(&panes, &[], kelpmux::now())
            .unwrap()
            .agents
            .iter()
            .all(|a| a.pid != first_pid)
    );
}

#[test]
fn bash_startup_preserves_user_rc_and_wraps_plain_codex_without_changing_cli_tools() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join(".bashrc"),
        "export USER_RC_PRESERVED=yes\n",
    )
    .unwrap();
    fs::write(
        fixture.root.join("bin/codex"),
        "#!/bin/sh\nprintf '%s|%s|%s' \"$USER_RC_PRESERVED\" \"$1\" \"$2\"\n",
    )
    .unwrap();
    let mut command = Command::new("/bin/bash");
    kelpmux::native::codex_client::integrate_shell(
        &mut command,
        &fixture.root.join("state"),
        Path::new("/bin/bash"),
    )
    .unwrap();
    let output = command
        .env("HOME", &fixture.root)
        .env("KELPMUX_PANE", "%1")
        .env("KELPMUX_EXECUTABLE", env!("CARGO_BIN_EXE_kelpmux"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", fixture.root.join("bin").display()),
        )
        .args(["-i", "-c", "codex exec 'two words'"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"yes|exec|two words");
    assert_eq!(
        fs::read_to_string(fixture.root.join(".bashrc")).unwrap(),
        "export USER_RC_PRESERVED=yes\n"
    );
}

#[test]
fn bridge_identity_rejects_reused_pids_foreign_records_and_unknown_selection() {
    let fixture = Fixture::new();
    let root = fixture.root.join("proc");
    fs::create_dir_all(root.join("42")).unwrap();
    let record = fixture.root.join("link.json");
    fs::write(
        root.join("42/environ"),
        format!("KELPMUX_CODEX_LINK={}\0", record.display()),
    )
    .unwrap();
    // state, parent, group, session, tty, tpgid, flags ... process start ticks.
    let stat = "42 (codex) S 1 42 42 1 42 0 0 0 0 0 0 0 0 0 0 0 0 0 100\n";
    fs::write(root.join("42/stat"), stat).unwrap();
    let write = |pid, birth, thread| {
        kelpmux::journal::atomic(
            &record,
            &kelpmux::native::codex_client::LinkRecord { pid, birth, thread },
        )
        .unwrap()
    };
    write(42, 100, Some("correct-thread".into()));
    assert_eq!(
        kelpmux::native::codex_client::identity(&root, 42),
        Some(Some("correct-thread".into()))
    );
    write(42, 101, Some("stale-thread".into()));
    assert_eq!(kelpmux::native::codex_client::identity(&root, 42), None);
    write(99, 100, Some("foreign-thread".into()));
    assert_eq!(kelpmux::native::codex_client::identity(&root, 42), None);
    write(42, 100, None);
    assert_eq!(
        kelpmux::native::codex_client::identity(&root, 42),
        Some(None)
    );
    fs::set_permissions(&record, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(kelpmux::native::codex_client::identity(&root, 42), None);
}

#[test]
fn normal_codex_command_in_owned_pty_links_and_drives_the_rendered_spinner() {
    use kelpmux::native::{delta, protocol::*};
    let fixture = Fixture::new();
    let endpoint = fixture.root.join("native.sock");
    struct Native {
        child: Child,
        endpoint: PathBuf,
    }
    impl Drop for Native {
        fn drop(&mut self) {
            let _ = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
                .args(["ctl", "--socket"])
                .arg(&self.endpoint)
                .arg("shutdown")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    let child = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
        .args(["server", "--socket"])
        .arg(&endpoint)
        .arg("--state-dir")
        .arg(fixture.root.join("state"))
        .args(["--shell", "/bin/bash", "--cwd"])
        .arg(&fixture.root)
        .args(["--size", "120x40"])
        .env("HOME", &fixture.root)
        .env("CODEX_HOME", fixture.root.join("codex"))
        .env("CLAUDE_CONFIG_DIR", fixture.root.join("claude"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", fixture.root.join("bin").display()),
        )
        .env("FIXTURE_ROOT", &fixture.root)
        .env("FIXTURE_THREAD", "thread-pty")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _native = Native {
        child,
        endpoint: endpoint.clone(),
    };
    wait(|| endpoint.exists());
    let mut socket = UnixStream::connect(&endpoint).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(6)))
        .unwrap();
    write_message(
        &mut socket,
        &ClientMessage::Attach {
            version: VERSION,
            cols: 120,
            rows: 40,
            takeover: false,
        },
    )
    .unwrap();
    let mut previous = None;
    let mut revision = 0;
    let mut frame_until = |socket: &mut UnixStream, predicate: &dyn Fn(&Frame) -> bool| -> Frame {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "expected automatic status frame");
            let frame = match read_message::<ServerMessage>(socket).unwrap().unwrap() {
                ServerMessage::Frame(frame) => {
                    revision = 1;
                    frame
                }
                ServerMessage::Patch(patch) => {
                    let frame = delta::apply(previous.as_ref().unwrap(), revision, &patch).unwrap();
                    revision = patch.revision;
                    frame
                }
                ServerMessage::Error(e) => panic!("{e}"),
                _ => continue,
            };
            previous = Some(frame.clone());
            if predicate(&frame) {
                return frame;
            }
        }
    };
    frame_until(&mut socket, &|_| true);
    write_message(&mut socket, &ClientMessage::Input(b"codex\r".to_vec())).unwrap();
    let working = frame_until(&mut socket, &|f| {
        f.panes.iter().any(|p| {
            p.pane.session_id.as_deref() == Some("thread-pty") && p.pane.status == Status::Working
        })
    });
    assert!(
        working
            .cells
            .iter()
            .any(|c| kelpmux::model::WORKING_FRAMES.contains(&c.text.as_str()))
    );
    assert_eq!(
        working.panes[0].pane.observation.source,
        StatusSource::AppServer
    );
    let spinner = working
        .cells
        .iter()
        .position(|c| kelpmux::model::WORKING_FRAMES.contains(&c.text.as_str()))
        .unwrap();
    frame_until(&mut socket, &|f| {
        f.cells[spinner].text != working.cells[spinner].text
    });
    write_message(
        &mut socket,
        &ClientMessage::Input(b"thread-pty-switched\r".to_vec()),
    )
    .unwrap();
    frame_until(&mut socket, &|f| {
        f.panes.iter().any(|p| {
            p.pane.session_id.as_deref() == Some("thread-pty-switched")
                && p.pane.status == Status::Working
        })
    });
    fs::write(fixture.root.join("status"), "idle").unwrap();
    frame_until(&mut socket, &|f| {
        f.panes.iter().any(|p| {
            p.pane.session_id.as_deref() == Some("thread-pty-switched")
                && p.pane.status == Status::Idle
        })
    });
    write_message(&mut socket, &ClientMessage::Action(Action::Shutdown)).unwrap();
}
