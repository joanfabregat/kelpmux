//! OSC 7501 parser, foreground ownership and real owned-PTY regressions.
use kelpmux::{
    model::{Kind, Status, StatusSource},
    native::{
        engine::{Effect, Terminal},
        program_status::{Event, Parser, REPLY, Records},
        protocol::*,
    },
};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn events(bytes: &[u8]) -> Vec<Event> {
    Parser::default().feed(bytes)
}
#[test]
fn fragmented_reports_query_order_and_unrelated_strings_preserve_terminal_output() {
    let input = b"\x1b]7501;?\x07\x1b[c\x1b]7501;state=working:app=codex\x1b\\ok\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\";
    for split in 0..=input.len() {
        let mut terminal = Terminal::new(40, 4);
        let mut output = terminal.feed(&input[..split]);
        output.extend(terminal.feed(&input[split..]));
        let replies: Vec<_> = output
            .iter()
            .filter_map(|e| {
                if let Effect::WritePty(v) = e {
                    Some(v)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(replies[0], REPLY);
        assert!(replies[1].starts_with(b"\x1b[?"));
        assert_eq!(
            output
                .iter()
                .filter(|e| matches!(e, Effect::Program(_)))
                .count(),
            1
        );
        assert_eq!(
            terminal.snapshot().cells[2].hyperlink.as_deref(),
            Some("https://example.com")
        );
    }
    assert!(events(b"\x1bP\x1b]7501;state=working:app=codex\x07\x1b\\").is_empty());
    assert!(
        events(b"\x1b]0;\x1b]7501;state=working:app=codex\x07")
            .iter()
            .any(|e| matches!(e, Event::Report(_)))
    ); // An ESC starts a fresh sequence, as in the VT parser.
}
#[test]
fn bounded_records_inherit_apps_clear_subtrees_and_reject_invalid_metadata() {
    let mut records = Records::default();
    for event in events(b"\x1b]7501;state=idle:app=codex\x07\x1b]7501;state=working:id=worker\x07\x1b]7501;state=blocked:id=worker/approval:kind=permission\x07") { records.apply(&event); }
    assert_eq!(records.status(Kind::Codex), Some(Status::InputNeeded));
    assert_eq!(records.status(Kind::Claude), None);
    for event in events(b"\x1b]7501;state=clear:id=worker\x07") {
        records.apply(&event);
    }
    assert_eq!(records.status(Kind::Codex), Some(Status::Idle));
    for value in [
        "state=working:id=bad//id:app=codex",
        "state=done:app=codex:msg=YmFkCg==",
        "state=unknown:app=codex",
    ] {
        let parsed = events(format!("\x1b]7501;{value}\x07").as_bytes());
        assert!(parsed.is_empty(), "{value}");
    }
    let overflow = format!(
        "\x1b]7501;state=working:app=codex:msg={}\x07safe",
        "A".repeat(5000)
    );
    let mut terminal = Terminal::new(20, 2);
    assert!(
        !terminal
            .feed(overflow.as_bytes())
            .iter()
            .any(|e| matches!(e, Effect::Program(_)))
    );
    assert!(
        terminal
            .snapshot()
            .cells
            .iter()
            .take(4)
            .map(|c| c.text.as_str())
            .collect::<String>()
            .starts_with("safe")
    );
    for event in events(b"\x1b]7501;state=done:app=codex\x07\x1b]133;A\x07") {
        records.apply(&event);
    }
    assert_eq!(records.status(Kind::Codex), Some(Status::TurnFinished));
    records.apply(&Event::Reset);
    assert_eq!(records.status(Kind::Codex), None);
}

struct Fixture {
    root: PathBuf,
    socket: PathBuf,
    child: Child,
}
impl Fixture {
    fn new(app: &str) -> Self {
        let root = std::env::temp_dir().join(format!("km-program-{}-{app}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let executable = root.join(app);
        fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let shell = root.join("shell");
        fs::write(
            &shell,
            format!(
                "#!/bin/sh\nexec '{}' --ignored --exact synthetic_status_agent --nocapture\n",
                executable.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            root.join("report"),
            format!(
                "state=idle:app={}",
                if app == "claude" { "claude-code" } else { app }
            ),
        )
        .unwrap();
        let socket = root.join("control.sock");
        let child = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
            .args(["server", "--shell"])
            .arg(shell)
            .arg("--cwd")
            .arg(&root)
            .arg("--state-dir")
            .arg(root.join("state"))
            .arg("--socket")
            .arg(&socket)
            .args(["--size", "100x30"])
            .env("CODEX_HOME", root.join("codex-home"))
            .env("CLAUDE_CONFIG_DIR", root.join("claude-home"))
            .env_remove("CODEX_THREAD_ID")
            .env_remove("KELPMUX_CODEX_LINK")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self {
            root,
            socket,
            child,
        }
    }
    fn pane(&self) -> Option<PaneInfo> {
        let mut socket = UnixStream::connect(&self.socket).ok()?;
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        write_message(&mut socket, &ClientMessage::Inspect).unwrap();
        match read_message(&mut socket).unwrap()? {
            ServerMessage::State { panes, .. } => panes.into_iter().next(),
            _ => None,
        }
    }
    fn wait(&self, wanted: Status) -> PaneInfo {
        let until = Instant::now() + Duration::from_secs(7);
        loop {
            if let Some(pane) = self.pane() {
                if pane.status == wanted && pane.observation.source == StatusSource::Terminal {
                    return pane;
                }
            }
            assert!(
                Instant::now() < until,
                "missing status {wanted:?}: {:?}",
                self.pane()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(mut socket) = UnixStream::connect(&self.socket) {
            let _ = write_message(&mut socket, &ClientMessage::Command(Action::Shutdown));
        }
        for _ in 0..200 {
            if self.child.try_wait().ok().flatten().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn pane_local_status_survives_polling_without_thread_identity_or_bridge() {
    let codex = Fixture::new("codex");
    let claude = Fixture::new("claude");
    assert!(codex.wait(Status::Idle).session_id.is_none());
    claude.wait(Status::Idle);
    assert!(fs::read_to_string(codex.root.join("query-ok")).is_ok());
    for (state, expected) in [
        ("working", Status::Working),
        ("blocked:kind=question", Status::InputNeeded),
        ("done", Status::TurnFinished),
        ("error", Status::Failed),
        ("idle", Status::Idle),
    ] {
        fs::write(
            codex.root.join("report"),
            format!("state={state}:app=codex"),
        )
        .unwrap();
        codex.wait(expected);
        assert_eq!(claude.wait(Status::Idle).status, Status::Idle);
    }
    // A metadata poll may not overwrite a live terminal report or require a heartbeat.
    fs::write(codex.root.join("report"), "state=working:app=codex").unwrap();
    codex.wait(Status::Working);
    thread::sleep(Duration::from_millis(2300));
    codex.wait(Status::Working);
    let saved = fs::read_to_string(codex.root.join("state/native/registry.json")).unwrap();
    assert!(saved.contains("terminal"));
    fs::write(codex.root.join("report"), "state=clear").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while codex
        .pane()
        .is_none_or(|pane| pane.observation.source == StatusSource::Terminal)
    {
        assert!(
            Instant::now() < deadline,
            "cleared terminal evidence remained active"
        );
        thread::sleep(Duration::from_millis(20));
    }
    thread::sleep(Duration::from_millis(2300));
    assert_ne!(
        codex.pane().unwrap().observation.source,
        StatusSource::Terminal
    );
}
#[test]
#[ignore = "synthetic PTY child, launched only by its fixture"]
fn synthetic_status_agent() {
    let mut term = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::tcgetattr(0, &mut term) }, 0);
    unsafe {
        libc::cfmakeraw(&mut term);
        libc::tcsetattr(0, libc::TCSANOW, &term);
    }
    print!("\x1b]7501;?\x07\x1b[c");
    std::io::stdout().flush().unwrap();
    let mut bytes = [0; 512];
    let mut reply = vec![];
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        let mut fd = libc::pollfd {
            fd: 0,
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut fd, 1, 100) } > 0 {
            let n = std::io::stdin().read(&mut bytes).unwrap();
            reply.extend_from_slice(&bytes[..n]);
            if reply.windows(3).any(|p| p == b"\x1b[?") {
                break;
            }
        }
    }
    assert!(
        reply.starts_with(REPLY),
        "probe reply must precede DA1: {reply:?}"
    );
    fs::write("query-ok", "ok").unwrap();
    let mut previous = String::new();
    loop {
        let report = fs::read_to_string("report").unwrap();
        if report != previous {
            print!("\x1b]7501;{report}\x1b\\");
            std::io::stdout().flush().unwrap();
            previous = report;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn evidence_rejects_reused_jobs_background_processes_and_replacement_agents() {
    use kelpmux::{
        model::{Agent, Observation},
        native::program_status::Snapshot,
        process::Process,
    };
    use std::collections::BTreeMap;
    let process = Process {
        pid: 100,
        parent: 1,
        group: 100,
        foreground: 100,
        tty: 123,
        start: 42,
        state: 'S',
        command: "codex".into(),
    };
    let mut agent = Agent {
        pane: "%1".into(),
        window: "@0".into(),
        pid: 100,
        process_start: 42,
        kind: Kind::Codex,
        session_id: None,
        name: "codex".into(),
        cwd: "/project".into(),
        status: Status::Unknown,
        observation: Observation::default(),
        source: "process".into(),
        first_seen: 1,
        last_seen: 1,
        ended_at: None,
    };
    let mut records = Records::default();
    for event in events(b"\x1b]7501;state=working:app=codex\x07") {
        records.apply(&event);
    }
    let mut snapshot = Snapshot {
        owner: (100, 42),
        agent: Some((100, 42)),
        records,
        event_at: 5,
    };
    let mut tree = BTreeMap::from([(100, process.clone())]);
    assert!(snapshot.apply(&mut agent, &tree, 100));
    assert_eq!(agent.observation.event_at, Some(5)); // No heartbeat or invented new event.
    tree.get_mut(&100).unwrap().start += 1;
    assert!(!snapshot.apply(&mut agent, &tree, 101));
    tree.insert(
        100,
        Process {
            foreground: 200,
            ..process.clone()
        },
    );
    assert!(!snapshot.apply(&mut agent, &tree, 101));
    tree.insert(100, process);
    snapshot.agent = Some((101, 43));
    assert!(!snapshot.apply(&mut agent, &tree, 101));
    snapshot.agent = Some((100, 42));
    agent.kind = Kind::Claude;
    assert!(!snapshot.apply(&mut agent, &tree, 101));
}

#[test]
fn normal_codex_launch_preserves_cwd_and_claude_hyperlinks_are_tty_scoped() {
    use std::os::fd::{AsRawFd, FromRawFd};
    let root = std::env::temp_dir().join(format!("km-shell-capability-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fs::write(
        root.join(".bashrc"),
        "alias c=codex\nexport USER_STARTUP=yes\n",
    )
    .unwrap();
    for name in ["codex", "claude"] {
        let file = root.join(name);
        fs::write(&file, "#!/bin/sh\nprintf '%s|%s|%s|%s|%s\\n' \"$0\" \"$PWD\" \"${FORCE_HYPERLINK-unset}\" \"$USER_STARTUP\" \"$*\"\n").unwrap();
        fs::set_permissions(file, fs::Permissions::from_mode(0o700)).unwrap();
    }
    for (tty, force) in [(false, None), (true, None), (true, Some("0"))] {
        let mut command = Command::new("/bin/bash");
        command
            .env("HOME", &root)
            .env("PATH", format!("{}:/usr/bin:/bin", root.display()))
            .env_remove("FORCE_HYPERLINK")
            .current_dir(&root);
        if let Some(force) = force {
            command.env("FORCE_HYPERLINK", force);
        }
        kelpmux::native::shell::integrate_shell(
            &mut command,
            &root.join("state"),
            std::path::Path::new("/bin/bash"),
        )
        .unwrap();
        command
            .args([
                "-i",
                "-c",
                "c 'argument with spaces'; claude 'argument with spaces'",
            ])
            .stderr(Stdio::null());
        let text = if tty {
            let (mut master, mut slave) = (-1, -1);
            assert_eq!(
                unsafe {
                    libc::openpty(
                        &mut master,
                        &mut slave,
                        std::ptr::null_mut(),
                        std::ptr::null(),
                        std::ptr::null(),
                    )
                },
                0
            );
            let mut master = unsafe { fs::File::from_raw_fd(master) };
            let slave = unsafe { fs::File::from_raw_fd(slave) };
            unsafe {
                libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
            }
            command
                .stdin(Stdio::from(slave.try_clone().unwrap()))
                .stdout(Stdio::from(slave));
            let mut child = command.spawn().unwrap();
            drop(command);
            assert!(child.wait().unwrap().success());
            let mut bytes = vec![];
            let _ = master.read_to_end(&mut bytes); // Linux PTY EOF is EIO.
            String::from_utf8(bytes).unwrap()
        } else {
            let output = command.output().unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap()
        };
        let codex = text
            .lines()
            .find(|line| line.starts_with(root.join("codex").to_str().unwrap()))
            .unwrap();
        let claude = text
            .lines()
            .find(|line| line.starts_with(root.join("claude").to_str().unwrap()))
            .unwrap();
        assert!(
            codex.contains(&format!(
                "|{}|{}|yes|argument with spaces",
                root.display(),
                force.unwrap_or("unset")
            )),
            "{text}"
        );
        assert!(
            claude.contains(&format!(
                "|{}|{}|yes|argument with spaces",
                root.display(),
                force.unwrap_or(if tty { "1" } else { "unset" })
            )),
            "{text}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}
