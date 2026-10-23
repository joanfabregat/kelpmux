//! Real isolated startup transactions, named workspaces and SSH PTY attachment.
use kelpmux::native::{
    lifecycle::{Settings, workspace_path},
    protocol::*,
};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            fs::{OpenOptionsExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
static NEXT: AtomicU64 = AtomicU64::new(0);

// Drain both pipes concurrently, but never wait indefinitely for a starter or
// an inherited output pipe. A regression must fail a test, not hang the suite.
fn bounded_output(command: &mut Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, rx) = mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    for (is_stdout, pipe) in [
        (true, Box::new(stdout) as Box<dyn Read + Send>),
        (false, Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let tx = tx.clone();
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = pipe.take(1024 * 1024 + 1).read_to_end(&mut bytes);
            let _ = tx.send((is_stdout, result, bytes));
        });
    }
    drop(tx);
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("workspace command exceeded ten seconds");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    for _ in 0..2 {
        let (is_stdout, result, bytes) = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("workspace command retained an output pipe");
        result.unwrap();
        assert!(
            bytes.len() <= 1024 * 1024,
            "workspace output exceeded limit"
        );
        if is_stdout {
            stdout = bytes;
        } else {
            stderr = bytes;
        }
    }
    Output {
        status,
        stdout,
        stderr,
    }
}
fn panes(out: &Output) -> Vec<PaneInfo> {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    match serde_json::from_slice(&out.stdout).unwrap() {
        ServerMessage::State { panes, .. } => panes,
        _ => panic!("missing status"),
    }
}
struct Fixture {
    root: PathBuf,
    state: PathBuf,
    socket: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kelpmux-lifecycle-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let state = root.join("state");
        let socket = state.join("control.sock");
        Self {
            root,
            state,
            socket,
        }
    }
    fn command(&self, verb: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kelpmux"));
        cmd.arg(verb)
            .arg("--state-dir")
            .arg(&self.state)
            .arg("--socket")
            .arg(&self.socket)
            .current_dir(&self.root);
        cmd
    }
    fn cmd(&self, verb: &str, extra: &[&str]) -> Output {
        bounded_output(self.command(verb).args(extra))
    }
    fn start(&self, extra: &[&str]) -> Vec<PaneInfo> {
        let mut args = vec!["--shell", "/bin/sh", "--cwd"];
        args.push(self.root.to_str().unwrap());
        args.extend(extra);
        panes(&self.cmd("start", &args))
    }
    fn status(&self) -> Vec<PaneInfo> {
        panes(&self.cmd("status", &[]))
    }
    fn stop(&self) {
        let out = self.cmd("stop", &[]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        wait_absent(&self.socket);
    }
}
fn wait_absent(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while path.exists() {
        assert!(
            Instant::now() < deadline,
            "{} did not disappear",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.socket.exists() {
            let _ = self.cmd("stop", &[]);
            let deadline = Instant::now() + Duration::from_secs(3);
            while self.socket.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
        }
        // Named-workspace assertions must also clean up after a panic.
        for name in ["first", "second"] {
            let state = self.root.join("kelpmux/workspaces").join(name);
            let socket = state.join("control.sock");
            if socket.exists() {
                let _ = bounded_output(
                    Command::new(env!("CARGO_BIN_EXE_kelpmux"))
                        .arg("stop")
                        .arg("--state-dir")
                        .arg(&state)
                        .arg("--socket")
                        .arg(&socket),
                );
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn background_workspace_survives_starter_exit_is_idempotent_and_stops_cleanly() {
    let fixture = Fixture::new();
    let before = fixture.start(&[]);
    assert_eq!(before.len(), 1);
    let pid: u32 = fs::read_to_string(fixture.state.join("server.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(fs::metadata(format!("/proc/{pid}")).is_ok());
    assert_eq!(fixture.start(&[])[0].pid, before[0].pid);
    assert_eq!(fixture.status()[0].pid, before[0].pid);
    fixture.stop();
    wait_absent(Path::new(&format!("/proc/{}", before[0].pid)));
    assert!(!fixture.cmd("status", &[]).status.success());
}
#[test]
fn named_workspaces_reject_path_traversal_and_preserve_effective_restart_settings() {
    for name in ["", "../other", "a/b", ".", "a;exit"] {
        assert!(workspace_path(name).is_err());
    }
    assert!(
        workspace_path("project-123")
            .unwrap()
            .ends_with("project-123")
    );
    let fixture = Fixture::new();
    fixture.start(&[]);
    let settings = Settings::load(&fixture.state.join("workspace.json")).unwrap();
    assert_eq!(settings.cwd.as_ref(), Some(&fixture.root));
    assert_eq!(settings.shell.as_deref(), Some(Path::new("/bin/sh")));
    fixture.stop();
    let out = bounded_output(
        fixture
            .command("start")
            .current_dir("/")
            .env("SHELL", "/does/not/exist"),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fixture.status()[0].cwd, fixture.root.to_string_lossy());
    fixture.stop();
}
#[test]
fn dev_example_starts_exactly_one_shell() {
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/native-tabs.json");
    let settings = kelpmux::native::lifecycle::Settings::load(&config).unwrap();
    assert_eq!(settings.tab_count, Some(1));
    let fixture = Fixture::new();
    assert_eq!(
        fixture.start(&["--config", config.to_str().unwrap()]).len(),
        1
    );
    fixture.stop();
}

#[test]
fn configured_large_startup_and_invalid_config_are_checked() {
    let fixture = Fixture::new();
    let config = fixture.root.join("config.json");
    fs::write(&config,r#"{"grid":[3,3],"ui":{"prefix":1,"read_width":100,"read_height_percent":80,"bindings":{"r":"read"}}}"#).unwrap();
    let panes = fixture.start(&["--config", config.to_str().unwrap()]);
    assert_eq!(panes.len(), 9);
    assert_eq!(
        panes
            .iter()
            .map(|p| p.id)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        9
    );
    fixture.stop();
    fs::write(&config, r#"{"tab_count":12}"#).unwrap();
    assert_eq!(
        fixture.start(&["--config", config.to_str().unwrap()]).len(),
        12
    );
    fixture.stop();
    fs::write(&config, r#"{"tab_count":1000}"#).unwrap();
    assert_eq!(
        kelpmux::native::lifecycle::Settings::load(&config)
            .unwrap()
            .tab_count,
        Some(1000)
    );
    fs::write(&config, r#"{"grid":[0,4]}"#).unwrap();
    assert!(
        !fixture
            .cmd("start", &["--config", config.to_str().unwrap()])
            .status
            .success()
    );
    assert!(!fixture.socket.exists());
}
#[test]
fn pid_fifo_cannot_block_background_start_and_symlink_state_is_rejected() {
    let fixture = Fixture::new();
    fs::create_dir(&fixture.state).unwrap();
    let pid = fixture.state.join("server.pid");
    let path = std::ffi::CString::new(pid.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let start = Instant::now();
    fixture.start(&[]);
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(fs::symlink_metadata(pid).unwrap().is_file());
    fixture.stop();
    let foreign = fixture.root.join("foreign");
    fs::create_dir(&foreign).unwrap();
    let link = fixture.root.join("symlink-state");
    std::os::unix::fs::symlink(&foreign, &link).unwrap();
    let out = bounded_output(
        Command::new(env!("CARGO_BIN_EXE_kelpmux"))
            .arg("start")
            .arg("--state-dir")
            .arg(&link)
            .arg("--shell")
            .arg("/bin/sh"),
    );
    assert!(!out.status.success());
    assert!(!foreign.join("workspace.json").exists());
}

#[test]
fn startup_lock_rejects_fifo_symlink_and_held_lock_without_mutating_settings() {
    for kind in ["fifo", "symlink", "held", "public"] {
        let fixture = Fixture::new();
        fs::create_dir(&fixture.state).unwrap();
        let path = fixture.state.join("startup.lock");
        let mut held = None;
        match kind {
            "fifo" => {
                let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            }
            "symlink" => std::os::unix::fs::symlink(fixture.root.join("outside"), &path).unwrap(),
            "held" => {
                let lock = OpenOptions::new()
                    .create_new(true)
                    .read(true)
                    .write(true)
                    .mode(0o600)
                    .open(&path)
                    .unwrap();
                assert_eq!(
                    unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                    0
                );
                held = Some(lock);
            }
            "public" => {
                let _ = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o644)
                    .open(&path)
                    .unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            }
            _ => unreachable!(),
        }
        let started = Instant::now();
        let out = fixture.cmd("start", &["--shell", "/bin/sh"]);
        assert!(!out.status.success(), "{kind} lock accepted");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(!fixture.state.join("workspace.json").exists());
        assert!(!fixture.socket.exists());
        drop(held);
    }
}
#[test]
fn concurrent_starters_keep_winning_configuration_consistent_with_live_shells() {
    let fixture = Fixture::new();
    let barrier = Arc::new(Barrier::new(2));
    let mut workers = Vec::new();
    for (index, grid, shell) in [(0, (2, 1), "/bin/sh"), (1, (2, 2), "/bin/bash")] {
        let cwd = fixture.root.join(format!("candidate-{index}"));
        fs::create_dir(&cwd).unwrap();
        let config = fixture.root.join(format!("config-{index}.json"));
        let settings = Settings {
            grid,
            shell: Some(PathBuf::from(shell)),
            cwd: Some(cwd),
            ..Settings::default()
        };
        fs::write(&config, serde_json::to_vec(&settings).unwrap()).unwrap();
        let mut command = fixture.command("start");
        command.arg("--config").arg(config);
        let barrier = barrier.clone();
        workers.push(thread::spawn(move || {
            barrier.wait();
            bounded_output(&mut command)
        }));
    }
    let outcomes: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert!(outcomes.iter().any(|out| out.status.success()));
    for out in &outcomes {
        if !out.status.success() {
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("startup is already in progress"),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
    let settings = Settings::load(&fixture.state.join("workspace.json")).unwrap();
    let actual = fixture.status();
    assert_eq!(
        actual.len(),
        usize::from(settings.grid.0) * usize::from(settings.grid.1)
    );
    for pane in actual {
        assert_eq!(pane.cwd, settings.cwd.as_ref().unwrap().to_string_lossy());
        let argv = fs::read(format!("/proc/{}/cmdline", pane.pid)).unwrap();
        assert_eq!(
            argv.split(|b| *b == 0).next().unwrap(),
            settings
                .shell
                .as_ref()
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
        );
    }
    fixture.stop();
}
#[test]
fn invalid_shell_and_working_directory_fail_bounded_without_live_server() {
    for (shell, cwd) in [
        ("/does/not/exist", None),
        ("/bin/sh", Some("/does/not/exist")),
    ] {
        let fixture = Fixture::new();
        let mut args = vec!["--shell", shell];
        if let Some(cwd) = cwd {
            args.extend(["--cwd", cwd]);
        }
        let started = Instant::now();
        let out = fixture.cmd("start", &args);
        assert!(!out.status.success());
        assert!(started.elapsed() < Duration::from_secs(8));
        assert!(!fixture.socket.exists());
        if let Ok(pid) = fs::read_to_string(fixture.state.join("server.pid")) {
            // Container PID 1 need not reap orphaned daemon zombies. A zombie
            // has exited and owns no PTYs; reject any genuinely live server.
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let stat = fs::read_to_string(format!("/proc/{}/stat", pid.trim()));
                let dead = match stat {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                    Ok(stat) => stat
                        .rsplit_once(')')
                        .and_then(|(_, tail)| tail.split_whitespace().next())
                        .is_some_and(|state| state == "Z" || state == "X"),
                    _ => false,
                };
                if dead {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "failed startup retained a live server"
                );
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}
#[test]
fn stopping_one_named_workspace_preserves_the_other_process() {
    let fixture = Fixture::new();
    let named = |verb: &str, name: &str| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kelpmux"));
        command
            .arg(verb)
            .args(["--workspace", name])
            .env("XDG_STATE_HOME", &fixture.root)
            .current_dir(&fixture.root);
        if verb == "start" {
            command.args(["--shell", "/bin/sh"]);
        }
        bounded_output(&mut command)
    };
    let first = panes(&named("start", "first"));
    let second = panes(&named("start", "second"));
    assert_ne!(first[0].pid, second[0].pid);
    assert!(named("stop", "first").status.success());
    wait_absent(&fixture.root.join("kelpmux/workspaces/first/control.sock"));
    assert_eq!(panes(&named("status", "second"))[0].pid, second[0].pid);
    assert!(Path::new(&format!("/proc/{}", second[0].pid)).exists());
    assert!(named("stop", "second").status.success());
    wait_absent(&fixture.root.join("kelpmux/workspaces/second/control.sock"));
}

struct SshPty {
    master: File,
    _slave: File,
    child: Child,
    output: Vec<u8>,
}
impl SshPty {
    fn spawn(fixture: &Fixture) -> Self {
        let (mut master, mut slave) = (-1, -1);
        let size = libc::winsize {
            ws_col: 80,
            ws_row: 24,
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
        for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
                0
            );
        }
        let mut command = fixture.command("ssh-entry");
        command
            .args(["--shell", "/bin/sh"])
            .env("TERM", "xterm-kelpmux")
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
            _slave: slave,
            child,
            output: Vec::new(),
        }
    }
    fn drain(&mut self) {
        let mut bytes = [0; 8192];
        while let Ok(n) = self.master.read(&mut bytes) {
            if n == 0 {
                break;
            }
            self.output.extend_from_slice(&bytes[..n]);
            assert!(self.output.len() < 1024 * 1024);
        }
    }
    fn ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            self.drain();
            if self.output.windows(4).any(|bytes| bytes == b"\x1b[2J") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "SSH entry failed to render: {}",
                String::from_utf8_lossy(&self.output)
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn input(&mut self, mut bytes: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !bytes.is_empty() {
            match self.master.write(bytes) {
                Ok(0) => panic!("PTY closed"),
                Ok(n) => bytes = &bytes[n..],
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    self.drain();
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("{error}"),
            }
        }
    }
    fn detach(&mut self) {
        self.input(b"\x02d");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.drain();
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "{}",
                    String::from_utf8_lossy(&self.output)
                );
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for SshPty {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[test]
fn ssh_entry_starts_attaches_and_reconnects_with_nonrecursive_shell_environment() {
    let fixture = Fixture::new();
    let mut first = SshPty::spawn(&fixture);
    first.ready();
    let before = fixture.status();
    assert_eq!(before.len(), 1);
    let result = fixture.root.join("shell-environment");
    let command = format!(
        "printf '%s|%s|%s' \"$TERM\" \"$KELPMUX\" \"$KELPMUX_PANE\" > '{}'\r",
        result.display()
    );
    first.input(command.as_bytes());
    let deadline = Instant::now() + Duration::from_secs(5);
    let expected = format!("xterm-256color|1|%{}", before[0].id);
    while fs::read_to_string(&result).ok().as_deref() != Some(expected.as_str()) {
        first.drain();
        assert!(
            Instant::now() < deadline,
            "shell environment was not reset: {}",
            String::from_utf8_lossy(&first.output)
        );
        thread::sleep(Duration::from_millis(10));
    }
    first.detach();
    assert_eq!(fixture.status()[0].pid, before[0].pid);
    let mut second = SshPty::spawn(&fixture);
    second.ready();
    assert_eq!(fixture.status()[0].pid, before[0].pid);
    second.detach();
    fixture.stop();
}

#[test]
fn template_start_preserves_names_directories_restart_and_control_after_directory_removal() {
    let fixture = Fixture::new();
    let first = fixture.root.join("first-project");
    let second = fixture.root.join("second-project");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    let template = fixture.root.join("template.json");
    fs::write(
        &template,
        serde_json::to_vec(&serde_json::json!({
            "grid": [2,1], "panes": [
                {"name":"Backend","cwd":first,"shell":"/bin/sh"},
                {"name":"Review","cwd":second}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let initial = fixture.start(&["--template", template.to_str().unwrap()]);
    assert_eq!(initial.len(), 2);
    assert_eq!(initial[0].title, "Backend");
    assert_eq!(initial[1].title, "Review");
    assert_eq!(Path::new(&initial[0].cwd), first);
    assert_eq!(Path::new(&initial[1].cwd), second);
    assert_eq!(fixture.start(&[])[0].pid, initial[0].pid);
    fixture.stop();
    let restarted = fixture.start(&[]);
    assert_eq!(restarted.len(), 2);
    assert_eq!(restarted[1].title, "Review");
    assert_eq!(Path::new(&restarted[1].cwd), second);
    fs::remove_dir(&second).unwrap();
    assert_eq!(fixture.status().len(), 2);
    assert!(fixture.cmd("diagnostics", &[]).status.success());
    fixture.stop();
    assert!(!fixture.cmd("start", &[]).status.success());
}

#[test]
fn explicit_template_override_rebalances_saved_grid_weights() {
    let fixture = Fixture::new();
    fixture.start(&[]);
    fixture.stop();
    let path = fixture.state.join("workspace.json");
    let mut settings = Settings::load(&path).unwrap();
    settings.grid = (2, 1);
    settings.grid_weights = [vec![600, 400], vec![1000]];
    fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
    let template = fixture.root.join("template.json");
    fs::write(&template, b"{\"grid\":[3,2],\"panes\":[]}").unwrap();
    assert_eq!(
        fixture
            .start(&["--template", template.to_str().unwrap()])
            .len(),
        6
    );
    let saved = Settings::load(&path).unwrap();
    assert_eq!(saved.tab_count, Some(6));
    assert!(saved.grid_weights.iter().all(Vec::is_empty));
    fixture.stop();
}
