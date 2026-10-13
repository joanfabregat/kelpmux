//! Real tmux tests always use a private socket, empty config, and disposable state.
use kelpmux::{
    model::{Agent, Kind, Status},
    process,
    tmux::Tmux,
    workspace::{self, Grid},
};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Server {
    root: PathBuf,
    tmux: Tmux,
}
impl Server {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "km-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let socket = root.join("s");
        let output = Command::new("tmux")
            .args([
                "-S",
                socket.to_str().unwrap(),
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "test",
                "-x",
                "300",
                "-y",
                "90",
                "exec /bin/bash --noprofile --norc",
            ])
            .env_remove("TMUX")
            .output()
            .expect("tmux is required for integration tests");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let tmux = Tmux {
            socket: Some(socket),
            binary: None,
        };
        tmux.run(&["set-option", "-g", "pane-base-index", "1"])
            .unwrap();
        tmux.run(&["set-option", "-g", "default-shell", "/bin/bash"])
            .unwrap();
        let server = Self { root, tmux };
        server.wait_idle();
        server
    }
    fn state(&self) -> PathBuf {
        self.root.join("state")
    }
    fn window(&self) -> String {
        self.tmux.format("test", "#{window_id}").unwrap()
    }
    fn wait_idle(&self) {
        wait(|| {
            let tree = process::snapshot(std::path::Path::new("/proc")).unwrap();
            self.tmux
                .panes()
                .unwrap()
                .iter()
                .all(|p| process::idle(&tree, p.pid, false))
        });
    }
    fn cli(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_kelpmux"))
            .args([
                "--state-dir",
                self.state().to_str().unwrap(),
                "--tmux-socket",
                self.tmux.socket.as_ref().unwrap().to_str().unwrap(),
            ])
            .args(args)
            .env_remove("TMUX")
            .env("HOME", &self.root)
            .output()
            .unwrap()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.tmux.run(&["kill-server"]);
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[track_caller]
fn wait(mut predicate: impl FnMut() -> bool) {
    let start = Instant::now();
    while !predicate() {
        assert!(start.elapsed() < Duration::from_secs(8), "timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "requires tmux; CI explicitly runs this suite"]
fn grows_and_shrinks_every_grid_without_losing_original_shell() {
    let server = Server::new();
    let window = server.window();
    let original = server.tmux.panes().unwrap()[0].clone();
    for shape in ["2x1", "2x2", "3x2", "3x3", "2x3", "2x2", "2x1", "1x1"] {
        server.wait_idle();
        let grid = Grid::parse(shape).unwrap();
        workspace::resize(&server.tmux, &server.state(), &window, grid, false).unwrap();
        let panes = server.tmux.panes().unwrap();
        assert_eq!(panes.len(), grid.count());
        assert!(
            panes
                .iter()
                .any(|p| p.id == original.id && p.pid == original.pid)
        );
        let dimensions = server
            .tmux
            .run(&[
                "list-panes",
                "-t",
                &window,
                "-F",
                "#{pane_left},#{pane_top}",
            ])
            .unwrap();
        let mut xs = std::collections::BTreeSet::new();
        let mut ys = std::collections::BTreeSet::new();
        for line in dimensions.lines() {
            let (x, y) = line.split_once(',').unwrap();
            xs.insert(x);
            ys.insert(y);
        }
        assert_eq!(xs.len(), grid.cols);
        assert_eq!(ys.len(), grid.rows);
    }
}
#[test]
#[ignore = "requires tmux; CI explicitly runs this suite"]
fn rejects_foreground_background_and_stopped_jobs_without_partial_shrink() {
    let server = Server::new();
    let window = server.window();
    workspace::resize(
        &server.tmux,
        &server.state(),
        &window,
        Grid::parse("2x2").unwrap(),
        false,
    )
    .unwrap();
    server.wait_idle();
    let panes = server.tmux.panes().unwrap();
    for (index, command) in ["sleep 60", "sleep 60 &", "sleep 60 & kill -STOP $!"]
        .iter()
        .enumerate()
    {
        server
            .tmux
            .run(&["send-keys", "-t", &panes[index].id, command, "Enter"])
            .unwrap();
    }
    wait(|| {
        let tree = process::snapshot(std::path::Path::new("/proc")).unwrap();
        panes[..3]
            .iter()
            .all(|p| !process::idle(&tree, p.pid, false))
    });
    let before: Vec<_> = server
        .tmux
        .panes()
        .unwrap()
        .iter()
        .map(|p| (p.id.clone(), p.pid))
        .collect();
    assert!(
        workspace::resize(
            &server.tmux,
            &server.state(),
            &window,
            Grid::parse("2x1").unwrap(),
            false
        )
        .is_err()
    );
    let after: Vec<_> = server
        .tmux
        .panes()
        .unwrap()
        .iter()
        .map(|p| (p.id.clone(), p.pid))
        .collect();
    assert_eq!(before, after);
}
#[test]
#[ignore = "requires tmux; CI explicitly runs this suite"]
fn dry_run_is_read_only_and_generated_config_parses() {
    let server = Server::new();
    let window = server.window();
    let before = server.tmux.format(&window, "#{window_layout}").unwrap();
    assert!(
        server
            .cli(&["layout", "3x3", "--window", &window, "--dry-run"])
            .status
            .success()
    );
    assert_eq!(
        before,
        server.tmux.format(&window, "#{window_layout}").unwrap()
    );
    let config = server.cli(&["tmux-config"]);
    assert!(config.status.success());
    let path = server.root.join("integration.conf");
    fs::write(&path, config.stdout).unwrap();
    server
        .tmux
        .run(&["source-file", path.to_str().unwrap()])
        .unwrap();
    assert!(
        server
            .tmux
            .run(&["list-keys", "-T", "root", "M-1"])
            .unwrap()
            .contains("select 1")
    );
    assert_eq!(
        server
            .tmux
            .run(&["show-options", "-gwv", "window-style"])
            .unwrap(),
        "fg=colour248,bg=default"
    );
    let status = server
        .tmux
        .run(&["show-options", "-gv", "status-left"])
        .unwrap();
    assert!(status.contains("kelpmux-help"));
    assert!(!status.contains("READ/GRID"));
}
struct Client {
    child: std::process::Child,
    master: std::os::fd::OwnedFd,
    output: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}
impl Client {
    fn attach(server: &Server, cols: u16, rows: u16) -> Self {
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::process::CommandExt;
        let mut master = -1;
        let mut slave = -1;
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: openpty writes valid descriptors and reads a valid winsize.
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
        let master = unsafe { std::os::fd::OwnedFd::from_raw_fd(master) };
        let slave = unsafe { std::os::fd::OwnedFd::from_raw_fd(slave) };
        let mut command = Command::new("tmux");
        command
            .args([
                "-u",
                "-S",
                server.tmux.socket.as_ref().unwrap().to_str().unwrap(),
                "attach-session",
                "-t",
                "test",
            ])
            .env_remove("TMUX")
            .env("TERM", "xterm-256color");
        command
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        // Drain terminal output so native redraws never block on a full PTY buffer.
        let drain = master.try_clone().unwrap();
        let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = output.clone();
        std::thread::spawn(move || {
            let mut bytes = [0_u8; 8192];
            loop {
                let count = unsafe {
                    libc::read(drain.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len())
                };
                if count <= 0 {
                    break;
                }
                let mut buffer = captured.lock().unwrap();
                buffer.extend_from_slice(&bytes[..count as usize]);
                if buffer.len() > 1024 * 1024 {
                    buffer.drain(..512 * 1024);
                }
            }
        });
        wait(|| {
            server
                .tmux
                .run(&["list-clients", "-F", "#{client_name}"])
                .is_ok_and(|s| !s.is_empty())
        });
        Self {
            child,
            master,
            output,
        }
    }
    fn output_contains(&self, text: &str) -> bool {
        String::from_utf8_lossy(&self.output.lock().unwrap()).contains(text)
    }
    fn wait_output(&self, text: &str) {
        let start = Instant::now();
        while !self.output_contains(text) {
            assert!(
                start.elapsed() < Duration::from_secs(8),
                "missing {text}: {:?}",
                String::from_utf8_lossy(&self.output.lock().unwrap())
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn resize(&self, cols: u16, rows: u16) {
        use std::os::fd::AsRawFd;
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size) },
            0
        );
    }
    fn keys(&self, bytes: &[u8]) {
        use std::io::Write;
        let mut file = fs::File::from(self.master.try_clone().unwrap());
        file.write_all(bytes).unwrap();
    }
    fn click(&self, x: usize, y: usize) {
        self.keys(format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", x + 1, y + 1, x + 1, y + 1).as_bytes());
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[ignore = "requires tmux; CI explicitly runs this suite"]
fn discovery_updates_headers_and_journals_exit_without_launching_real_agents() {
    let server = Server::new();
    let pane = server.tmux.panes().unwrap()[0].clone();
    // A renamed sleep binary supplies only a synthetic process name; no agent is launched.
    let fake = server.root.join("codex");
    fs::copy("/bin/sleep", &fake).unwrap();
    server
        .tmux
        .run(&[
            "send-keys",
            "-t",
            &pane.id,
            &format!("{} 60", fake.display()),
            "Enter",
        ])
        .unwrap();
    wait(|| {
        process::snapshot(std::path::Path::new("/proc"))
            .unwrap()
            .values()
            .any(|p| p.command == "codex")
    });
    let output = server.cli(&["scan"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        server.tmux.option(&pane.id, "@kelpmux-status").unwrap(),
        "UNKNOWN"
    );
    let agents: serde_json::Value = serde_json::from_slice(&server.cli(&["list"]).stdout).unwrap();
    assert_eq!(agents.as_array().unwrap().len(), 1);
    assert!(agents[0]["session_id"].is_null());
    server
        .tmux
        .run(&["send-keys", "-t", &pane.id, "C-c"])
        .unwrap();
    server.wait_idle();
    assert!(server.cli(&["scan"]).status.success());
    assert_eq!(server.tmux.option(&pane.id, "@kelpmux-status").unwrap(), "");
    assert_eq!(
        server
            .tmux
            .option(&pane.id, "@kelpmux-color-muted")
            .unwrap(),
        ""
    );
    let records: serde_json::Value =
        serde_json::from_slice(&server.cli(&["journal"]).stdout).unwrap();
    assert_eq!(
        records.as_array().unwrap().last().unwrap()["event"],
        "exited"
    );
}

#[test]
#[ignore = "requires tmux 3.8; CI explicitly runs this suite"]
fn status_titles_use_bright_and_muted_colors_with_native_floating_headers() {
    let server = Server::new();
    let window = server.window();
    workspace::resize(
        &server.tmux,
        &server.state(),
        &window,
        Grid::parse("2x1").unwrap(),
        false,
    )
    .unwrap();
    server.wait_idle();
    let panes = server.tmux.panes().unwrap();
    let config = server.cli(&["tmux-config"]);
    assert!(config.status.success());
    let path = server.root.join("colors.conf");
    fs::write(&path, config.stdout).unwrap();
    server
        .tmux
        .run(&["source-file", path.to_str().unwrap()])
        .unwrap();
    let client = Client::attach(&server, 240, 60);
    for status in [
        Status::Working,
        Status::InputNeeded,
        Status::TurnFinished,
        Status::Failed,
        Status::Idle,
        Status::Unknown,
    ] {
        client.output.lock().unwrap().clear();
        for pane in &panes {
            server
                .tmux
                .header(&Agent {
                    pane: pane.id.clone(),
                    window: window.clone(),
                    pid: pane.pid,
                    process_start: 0,
                    kind: Kind::Codex,
                    session_id: None,
                    name: "synthetic-status".into(),
                    cwd: pane.cwd.clone(),
                    status,
                    source: "test".into(),
                    first_seen: 0,
                    last_seen: 0,
                    ended_at: None,
                })
                .unwrap();
        }
        server
            .tmux
            .run(&["select-pane", "-t", &panes[0].id])
            .unwrap();
        server.tmux.run(&["refresh-client"]).unwrap();
        client.wait_output(&format!(
            "48;5;{}",
            status.color().trim_start_matches("colour")
        ));
        client.wait_output(&format!(
            "48;5;{}",
            status.muted_color().trim_start_matches("colour")
        ));
        assert!(!client.output_contains("#{?pane_active"));
        assert!(!client.output_contains("bg=colour88]"));
        let expanded = server
            .tmux
            .format(&panes[0].id, &kelpmux::tmux::header_format())
            .unwrap();
        assert!(expanded.contains(&format!("fill={}", status.color())));
        server
            .tmux
            .run(&["select-pane", "-t", &panes[1].id])
            .unwrap();
        let expanded = server
            .tmux
            .format(&panes[0].id, &kelpmux::tmux::header_format())
            .unwrap();
        assert!(expanded.contains(&format!("fill={}", status.muted_color())));
    }
    client.output.lock().unwrap().clear();
    assert!(
        server
            .cli(&["read", "--pane", &panes[0].id])
            .status
            .success()
    );
    client.wait_output("synthetic-status");
    assert!(!client.output_contains("#{?pane_active"));
    assert_eq!(
        server
            .tmux
            .format(&panes[0].id, "#{pane_floating_flag}")
            .unwrap(),
        "1"
    );
    assert_eq!(
        server
            .tmux
            .run(&["show-options", "-gwv", "pane-border-style"])
            .unwrap(),
        "fg=colour238,bg=colour234"
    );
    assert_eq!(server.tmux.panes().unwrap()[0].pid, panes[0].pid);
}

#[test]
#[ignore = "requires tmux 3.8; CI explicitly runs this suite"]
fn graphical_grid_picker_selects_on_large_and_small_terminals_and_blocks_busy_shrink() {
    let server = Server::new();
    let window = server.window();
    let config = server.cli(&["tmux-config"]);
    assert!(config.status.success());
    let path = server.root.join("picker.conf");
    fs::write(&path, config.stdout).unwrap();
    server
        .tmux
        .run(&["source-file", path.to_str().unwrap()])
        .unwrap();
    server
        .tmux
        .run(&["set-option", "-g", "mouse", "on"])
        .unwrap();
    for (height, key, shape) in [(60, b'4', "3x2"), (28, b'5', "2x3"), (18, b'3', "2x2")] {
        server.wait_idle();
        let client = Client::attach(&server, 100, height);
        client.keys(b"\x02g");
        client.wait_output("Kelpmux grid");
        assert!(client.output_contains("cols"));
        client.keys(&[key]);
        let grid = Grid::parse(shape).unwrap();
        wait(|| {
            workspace::grid_choices(&server.tmux, &window)
                .is_ok_and(|choices| choices.iter().any(|c| c.grid == grid && c.current))
        });
        assert_eq!(server.tmux.format(&window, "#{pane_in_mode}").unwrap(), "0");
        drop(client);
        wait(|| {
            server
                .tmux
                .run(&["list-clients"])
                .is_ok_and(|s| s.is_empty())
        });
    }
    server.wait_idle();
    let pane = server.tmux.panes().unwrap()[0].clone();
    server
        .tmux
        .run(&["respawn-pane", "-k", "-t", &pane.id, "exec sleep 60"])
        .unwrap();
    wait(|| {
        let tree = process::snapshot(std::path::Path::new("/proc")).unwrap();
        let current = server.tmux.panes().unwrap();
        !process::idle(
            &tree,
            current.iter().find(|p| p.id == pane.id).unwrap().pid,
            false,
        )
    });
    let choices = workspace::grid_choices(&server.tmux, &window).unwrap();
    // One busy pane still permits 1x1; occupy another so it becomes unsafe.
    assert!(choices[0].blocked.is_none());
    let other = server.tmux.panes().unwrap()[1].clone();
    server
        .tmux
        .run(&["respawn-pane", "-k", "-t", &other.id, "exec sleep 60"])
        .unwrap();
    wait(|| {
        workspace::grid_choices(&server.tmux, &window).unwrap()[0]
            .blocked
            .is_some()
    });
    let client = Client::attach(&server, 100, 28);
    client.keys(b"\x02g");
    client.wait_output("Kelpmux grid");
    client.keys(b"1");
    client.wait_output("Needs 3 unused; 2 available");
    assert!(client.output_contains("□"));
    client.keys(b"1");
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(server.tmux.panes().unwrap().len(), 5);
    client.keys(b"\x1b");
    // Let the terminal's default escape-time expire before sending a prefix.
    std::thread::sleep(Duration::from_millis(600));
    client.keys(b"\x02?");
    client.wait_output("Kelpmux shortcuts");
    client.keys(b"q");
    // A stale menu cannot bypass the applying command's occupancy checks.
    assert!(
        !server
            .cli(&["layout", "1x1", "--window", &window])
            .status
            .success()
    );
    let busy: Vec<_> = server
        .tmux
        .panes()
        .unwrap()
        .into_iter()
        .filter(|p| p.id == pane.id || p.id == other.id)
        .collect();
    client.output.lock().unwrap().clear();
    client.click(9, 27);
    client.wait_output("Kelpmux grid");
    let dialog = server
        .tmux
        .panes()
        .unwrap()
        .into_iter()
        .find(|p| p.floating)
        .unwrap();
    let position = server
        .tmux
        .format(&dialog.id, "#{pane_left} #{pane_top}")
        .unwrap();
    let position: Vec<usize> = position
        .split_whitespace()
        .map(|s| s.parse().unwrap())
        .collect();
    client.click(position[0] + 4, position[1] + 4);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(server.tmux.panes().unwrap().len(), 5); // Four shells + picker.
    // Second card retains both occupied panes and removes the two idle shells.
    client.keys(format!("\x1b[<35;{};{}M", position[0] + 27, position[1] + 5).as_bytes());
    wait(|| {
        server
            .tmux
            .run(&["capture-pane", "-p", "-e", "-t", &dialog.id])
            .is_ok_and(|s| {
                s.lines()
                    .skip(2)
                    .take(6)
                    .all(|line| line.contains("48;5;39"))
            })
    });
    client.click(position[0] + 26, position[1] + 4);
    wait(|| server.tmux.panes().unwrap().len() == 2);
    let remaining = server.tmux.panes().unwrap();
    for pane in busy {
        assert!(
            remaining
                .iter()
                .any(|p| p.id == pane.id && p.pid == pane.pid)
        );
    }
}

#[test]
#[ignore = "requires tmux 3.8; CI explicitly runs this suite"]
fn native_reading_preserves_panes_indices_and_grid_slots() {
    let server = Server::new();
    let window = server.window();
    workspace::resize(
        &server.tmux,
        &server.state(),
        &window,
        Grid::parse("2x2").unwrap(),
        false,
    )
    .unwrap();
    server.wait_idle();
    let originals = server.tmux.panes().unwrap();
    let positions = server
        .tmux
        .run(&[
            "list-panes",
            "-t",
            &window,
            "-F",
            "#{pane_id} #{pane_index} #{pane_left} #{pane_top} #{pane_width} #{pane_height}",
        ])
        .unwrap();
    let pane = &originals[1];
    let read = server.cli(&["read", "--pane", &pane.id]);
    assert!(
        read.status.success(),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    assert_eq!(
        server
            .tmux
            .format(
                &pane.id,
                "#{pane_floating_flag} #{pane_index} #{pane_width}"
            )
            .unwrap(),
        "1 2 120"
    );
    assert_eq!(
        server
            .tmux
            .run(&["list-sessions", "-F", "#{session_name}"])
            .unwrap(),
        "test"
    );
    assert_eq!(server.tmux.panes().unwrap().len(), 4);
    assert!(
        !server
            .cli(&["layout", "1x1", "--window", &window])
            .status
            .success()
    );
    assert!(
        server
            .cli(&["fullscreen", "--pane", &pane.id])
            .status
            .success()
    );
    assert_eq!(
        server
            .tmux
            .format(&pane.id, "#{window_zoomed_flag} #{pane_width}")
            .unwrap(),
        "1 300"
    );
    assert!(
        server
            .cli(&["restore", "--pane", &pane.id])
            .status
            .success()
    );
    assert_eq!(
        server
            .tmux
            .format(&pane.id, "#{pane_floating_flag} #{pane_width}")
            .unwrap(),
        "1 120"
    );
    // A new short-lived helper invocation returns the same process; no recovery
    // file or long-running reading controller is involved.
    assert!(
        server
            .cli(&["select", "3", "--window", &window])
            .status
            .success()
    );
    assert_eq!(
        server
            .tmux
            .format(&originals[2].id, "#{pane_floating_flag} #{pane_index}")
            .unwrap(),
        "1 3"
    );
    let grid = server.cli(&["grid", "--window", &window]);
    assert!(grid.status.success());
    assert_eq!(
        server
            .tmux
            .run(&[
                "list-panes",
                "-t",
                &window,
                "-F",
                "#{pane_id} #{pane_index} #{pane_left} #{pane_top} #{pane_width} #{pane_height}"
            ])
            .unwrap(),
        positions
    );
    for original in originals {
        assert!(
            server
                .tmux
                .panes()
                .unwrap()
                .iter()
                .any(|p| p.id == original.id
                    && p.pid == original.pid
                    && p.window == window
                    && !p.floating)
        );
    }
}

#[test]
#[ignore = "requires tmux 3.8; CI explicitly runs this suite"]
fn reading_keeps_background_geometry_and_cleans_only_empty_placeholders() {
    for shape in ["1x1", "2x1", "2x2", "3x2", "2x3", "3x3"] {
        let server = Server::new();
        server
            .tmux
            .run(&["set-option", "-g", "pane-border-status", "top"])
            .unwrap();
        let window = server.window();
        workspace::resize(
            &server.tmux,
            &server.state(),
            &window,
            Grid::parse(shape).unwrap(),
            false,
        )
        .unwrap();
        server.wait_idle();
        let original = server.tmux.panes().unwrap();
        let geometry = |id: &str| {
            server
                .tmux
                .format(
                    id,
                    "#{pane_index} #{pane_left} #{pane_top} #{pane_width} #{pane_height}",
                )
                .unwrap()
        };
        let before: Vec<_> = original.iter().map(|p| geometry(&p.id)).collect();
        let source = &original[original.len() / 2];
        workspace::reading(
            &server.tmux,
            &server.state(),
            &source.id,
            120,
            workspace::Reading::Toggle,
        )
        .unwrap();
        for (pane, position) in original.iter().zip(&before) {
            if pane.id != source.id {
                assert_eq!(
                    geometry(&pane.id),
                    *position,
                    "background changed in {shape}"
                );
            }
        }
        let slot = server
            .tmux
            .option(&source.id, "@kelpmux-reading-placeholder")
            .unwrap();
        assert_eq!(server.tmux.format(&slot, "#{pane_pid}").unwrap(), "");
        assert_eq!(
            server.tmux.option(&slot, "window-style").unwrap(),
            "bg=black"
        );
        assert_eq!(
            server
                .tmux
                .format(
                    &slot,
                    "#{pane_left} #{pane_top} #{pane_width} #{pane_height}"
                )
                .unwrap(),
            before[original.len() / 2].split_once(' ').unwrap().1
        );
        assert_eq!(server.tmux.option(&slot, "pane-border-format").unwrap(), "");
        assert_eq!(
            server.tmux.panes().unwrap().len(),
            original.len(),
            "placeholder must not count as an agent"
        );
        workspace::cleanup_reading_slots(&server.tmux, &server.state()).unwrap();
        assert!(server.tmux.format(&slot, "#{pane_id}").is_ok());
        workspace::grid(&server.tmux, &server.state(), &window).unwrap();
        assert_ne!(
            server.tmux.format(&slot, "#{pane_id}").unwrap_or_default(),
            slot
        );
        for (pane, position) in original.iter().zip(&before) {
            assert_eq!(geometry(&pane.id), *position);
            assert!(
                server
                    .tmux
                    .panes()
                    .unwrap()
                    .iter()
                    .any(|p| p.id == pane.id && p.pid == pane.pid)
            );
        }
    }
    let server = Server::new();
    let source = server.tmux.panes().unwrap()[0].clone();
    workspace::reading(
        &server.tmux,
        &server.state(),
        &source.id,
        120,
        workspace::Reading::Toggle,
    )
    .unwrap();
    let slot = server
        .tmux
        .option(&source.id, "@kelpmux-reading-placeholder")
        .unwrap();
    // A user can turn the decoration into a real terminal; never kill that process.
    server
        .tmux
        .run(&["respawn-pane", "-t", &slot, "exec sleep 60"])
        .unwrap();
    let pid = server.tmux.format(&slot, "#{pane_pid}").unwrap();
    workspace::cleanup_reading_slots(&server.tmux, &server.state()).unwrap();
    workspace::close_reading(&server.tmux, &server.state(), &source.id).unwrap();
    assert_eq!(server.tmux.format(&slot, "#{pane_pid}").unwrap(), pid);
    assert_eq!(server.tmux.panes().unwrap().len(), 2);
    // An explicitly killed source leaves only empty decoration for the poller.
    workspace::reading(
        &server.tmux,
        &server.state(),
        &source.id,
        120,
        workspace::Reading::Toggle,
    )
    .unwrap();
    let orphan = server
        .tmux
        .option(&source.id, "@kelpmux-reading-placeholder")
        .unwrap();
    server.tmux.run(&["kill-pane", "-t", &source.id]).unwrap();
    workspace::cleanup_reading_slots(&server.tmux, &server.state()).unwrap();
    assert_ne!(
        server
            .tmux
            .format(&orphan, "#{pane_id}")
            .unwrap_or_default(),
        orphan
    );
    assert_eq!(server.tmux.format(&slot, "#{pane_pid}").unwrap(), pid);
}

#[test]
#[ignore = "requires tmux 3.8; CI explicitly runs this suite"]
fn mouse_controls_sticky_help_and_word_navigation_preserve_agent_panes() {
    let server = Server::new();
    let window = server.window();
    workspace::resize(
        &server.tmux,
        &server.state(),
        &window,
        Grid::parse("2x1").unwrap(),
        false,
    )
    .unwrap();
    server.wait_idle();
    let original = server.tmux.panes().unwrap()[0].clone();
    let config = server.cli(&["tmux-config"]);
    assert!(config.status.success());
    let path = server.root.join("controls.conf");
    fs::write(&path, config.stdout).unwrap();
    server
        .tmux
        .run(&["source-file", path.to_str().unwrap()])
        .unwrap();
    server
        .tmux
        .run(&["set-option", "-g", "mouse", "on"])
        .unwrap();
    server
        .tmux
        .run(&["bind-key", "-n", "F12", "set-option", "-w", "@probe", "hit"])
        .unwrap();
    let client = Client::attach(&server, 240, 60);
    let probe = || {
        server
            .tmux
            .run(&["show-options", "-wqv", "-t", &window, "@probe"])
            .unwrap()
    };
    let clear_probe = || {
        server
            .tmux
            .run(&["set-option", "-wu", "-t", &window, "@probe"])
            .unwrap();
    };
    client.click(2, 59);
    client.wait_output("Kelpmux shortcuts");
    client.keys(b"\x1b[24~");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(probe(), "", "help must stay open after mouse release");
    client.click(2, 59);
    client.keys(b"\x1b[24~");
    wait(|| probe() == "hit");
    clear_probe();
    client.output.lock().unwrap().clear();
    client.click(2, 59);
    client.wait_output("Kelpmux shortcuts");
    client.click(1, 1);
    client.keys(b"\x1b[24~");
    wait(|| probe() == "hit");
    clear_probe();
    client.output.lock().unwrap().clear();
    client.click(2, 59);
    client.wait_output("Kelpmux shortcuts");
    let menu_width = "Fullscreen / reading width: Ctrl+b f or ⛶ / ▣"
        .chars()
        .count()
        + 4;
    let menu_x = (240 - 1) / 2 - menu_width / 2;
    let menu_y = (59 - 1) / 2 + 10 / 2 - 10;
    client.click(menu_x + menu_width - 4, menu_y);
    client.keys(b"\x1b[24~");
    wait(|| probe() == "hit");

    // Option arrows may arrive as either word keys or modified arrow sequences.
    let input = server.root.join("word-keys");
    server
        .tmux
        .run(&[
            "send-keys",
            "-t",
            &original.id,
            &format!(
                "stty -icanon -echo; head -c 16 > {}; stty sane",
                input.display()
            ),
            "Enter",
        ])
        .unwrap();
    wait(|| input.exists());
    client.keys(b"\x1bf\x1bb\x1b[1;3D\x1b[1;3C");
    wait(|| fs::read(&input).is_ok_and(|bytes| bytes.len() == 16));
    let input = fs::read(&input).unwrap();
    assert_eq!(&input[..4], b"\x1bf\x1bb");
    assert_eq!(
        server
            .tmux
            .format(&original.id, "#{pane_floating_flag} #{window_zoomed_flag}")
            .unwrap(),
        "0 0"
    );
    server.wait_idle();
    // Both controls are available on an ordinary tiled pane.
    let coords = server
        .tmux
        .format(&original.id, "#{pane_left} #{pane_top} #{pane_width}")
        .unwrap();
    let coords: Vec<usize> = coords
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    client.click(coords[0] + coords[2] - 5, coords[1] - 1);
    wait(|| {
        server
            .tmux
            .format(&original.id, "#{pane_floating_flag}")
            .unwrap()
            == "1"
    });
    let coords = server
        .tmux
        .format(&original.id, "#{pane_left} #{pane_top} #{pane_width}")
        .unwrap();
    let coords: Vec<usize> = coords
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    client.output.lock().unwrap().clear();
    client.click(coords[0] + coords[2] - 5, coords[1] - 1);
    wait(|| {
        server
            .tmux
            .format(&original.id, "#{window_zoomed_flag}")
            .unwrap()
            == "1"
    });
    // Fullscreen hides the tiled header; equivalent controls remain in the status bar.
    client.wait_output("▣");
    client.output.lock().unwrap().clear();
    let status_label = server
        .tmux
        .format(&original.id, "#{pane_index}: #{b:pane_current_path}")
        .unwrap();
    let full_button_x = " ?  Grid ".chars().count() + 2 + status_label.chars().count() + 1;
    client.click(full_button_x, 59);
    wait(|| {
        server
            .tmux
            .format(&original.id, "#{window_zoomed_flag}")
            .unwrap()
            == "0"
    });
    client.wait_output("⛶");
    let coords = server
        .tmux
        .format(&original.id, "#{pane_left} #{pane_top} #{pane_width}")
        .unwrap();
    let coords: Vec<usize> = coords
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    client.click(coords[0] + coords[2] - 1, coords[1] - 1);
    wait(|| {
        server
            .tmux
            .format(&original.id, "#{pane_floating_flag}")
            .unwrap()
            == "0"
    });
    assert_eq!(server.tmux.panes().unwrap()[0].pid, original.pid);
    assert!(
        server
            .cli(&["close", "--pane", &original.id])
            .status
            .success()
    );
    assert_eq!(
        server
            .tmux
            .format(&original.id, "#{pane_floating_flag}")
            .unwrap(),
        "0"
    );
    // Fullscreen can also start directly from the tiled header.
    let coords = server
        .tmux
        .format(&original.id, "#{pane_left} #{pane_top} #{pane_width}")
        .unwrap();
    let coords: Vec<usize> = coords
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    client.click(coords[0] + coords[2] - 2, coords[1] - 1);
    wait(|| {
        server
            .tmux
            .format(&original.id, "#{window_zoomed_flag}")
            .unwrap()
            == "1"
    });
    let close_button_x = " ?  Grid ".chars().count() + 2 + status_label.chars().count() + 4;
    client.click(close_button_x, 59);
    wait(|| {
        server
            .tmux
            .format(&original.id, "#{pane_floating_flag} #{window_zoomed_flag}")
            .unwrap()
            == "0 0"
    });
    assert_eq!(server.tmux.panes().unwrap()[0].pid, original.pid);
}

#[test]
#[ignore = "requires tmux 3.8; CI explicitly runs this suite"]
fn native_swap_map_captures_digits_and_cancellation_without_losing_processes() {
    let server = Server::new();
    let window = server.window();
    workspace::resize(
        &server.tmux,
        &server.state(),
        &window,
        Grid::parse("2x2").unwrap(),
        false,
    )
    .unwrap();
    server.wait_idle();
    let original = server.tmux.panes().unwrap();
    let source = &original[0];
    let destination = &original[1];
    let position = |pane: &str| {
        server
            .tmux
            .format(pane, "#{pane_left} #{pane_top}")
            .unwrap()
    };
    let config = server.cli(&["tmux-config"]);
    assert!(config.status.success());
    let path = server.root.join("swap.conf");
    fs::write(&path, config.stdout).unwrap();
    server
        .tmux
        .run(&["source-file", path.to_str().unwrap()])
        .unwrap();
    let client = Client::attach(&server, 240, 60);
    let before_source = position(&source.id);
    let before_destination = position(&destination.id);
    server.tmux.run(&["select-pane", "-t", &source.id]).unwrap();
    client.keys(b"\x02s");
    wait(|| server.tmux.format(&source.id, "#{pane_in_mode}").unwrap() == "1");
    client.keys(destination.index.to_string().as_bytes());
    wait(|| {
        position(&source.id) == before_destination && position(&destination.id) == before_source
    });
    assert_eq!(
        server.tmux.format(&window, "#{pane_id}").unwrap(),
        source.id
    );
    client.keys(b"\x02s");
    wait(|| server.tmux.format(&source.id, "#{pane_in_mode}").unwrap() == "1");
    client.keys(b"\x1b");
    wait(|| server.tmux.format(&source.id, "#{pane_in_mode}").unwrap() == "0");
    assert_eq!(position(&source.id), before_destination);
    for pane in original {
        assert!(
            server
                .tmux
                .panes()
                .unwrap()
                .iter()
                .any(|p| p.id == pane.id && p.pid == pane.pid)
        );
    }
}

#[test]
#[ignore = "requires tmux 3.8; CI explicitly runs this suite"]
fn native_reading_keyboard_resize_and_single_pane_work() {
    let server = Server::new();
    let window = server.window();
    workspace::resize(
        &server.tmux,
        &server.state(),
        &window,
        Grid::parse("2x1").unwrap(),
        false,
    )
    .unwrap();
    server.wait_idle();
    let original = server.tmux.panes().unwrap()[0].clone();
    let config = server.cli(&["tmux-config"]);
    assert!(config.status.success());
    let path = server.root.join("keys.conf");
    fs::write(&path, config.stdout).unwrap();
    server
        .tmux
        .run(&["source-file", path.to_str().unwrap()])
        .unwrap();
    let client = Client::attach(&server, 240, 60);
    server
        .tmux
        .run(&["select-pane", "-t", &original.id])
        .unwrap();
    client.keys(b"\x02z");
    wait(|| {
        server
            .tmux
            .format(&original.id, "#{pane_floating_flag} #{pane_width}")
            .is_ok_and(|s| s == "1 120")
    });
    assert_eq!(
        server
            .tmux
            .run(&["list-clients", "-F", "#{client_name}"])
            .unwrap()
            .lines()
            .count(),
        1
    );
    client.keys(b"\x02f");
    wait(|| {
        server
            .tmux
            .format(&original.id, "#{window_zoomed_flag} #{pane_width}")
            .is_ok_and(|s| s == "1 240")
    });
    client.keys(b"\x02R");
    wait(|| {
        server
            .tmux
            .format(&original.id, "#{pane_floating_flag} #{pane_width}")
            .is_ok_and(|s| s == "1 120")
    });
    client.keys(b"\x1b2");
    let second = server
        .tmux
        .panes()
        .unwrap()
        .into_iter()
        .find(|p| p.index == 2)
        .unwrap();
    wait(|| {
        server
            .tmux
            .format(&second.id, "#{pane_floating_flag}")
            .is_ok_and(|s| s == "1")
    });
    client.resize(90, 40);
    wait(|| {
        server
            .tmux
            .format(&second.id, "#{pane_width}")
            .is_ok_and(|s| s == "90")
    });
    client.resize(240, 60);
    wait(|| {
        server
            .tmux
            .format(&second.id, "#{pane_width}")
            .is_ok_and(|s| s == "120")
    });
    client.keys(b"\x02z");
    wait(|| {
        server
            .tmux
            .format(&second.id, "#{pane_floating_flag}")
            .is_ok_and(|s| s == "0")
    });
    assert_eq!(server.tmux.panes().unwrap().len(), 2);
    drop(client);
    server.wait_idle();
    workspace::resize(
        &server.tmux,
        &server.state(),
        &window,
        Grid::parse("1x1").unwrap(),
        false,
    )
    .unwrap();
    let single = server.tmux.panes().unwrap()[0].clone();
    assert!(server.cli(&["read", "--pane", &single.id]).status.success());
    assert!(server.cli(&["grid", "--window", &window]).status.success());
    assert_eq!(server.tmux.panes().unwrap()[0].pid, single.pid);
}

#[test]
#[ignore = "requires tmux 3.8; CI explicitly runs this suite"]
fn bindings_only_preserves_user_appearance_and_manual_floating_panes() {
    let server = Server::new();
    let window = server.window();
    server
        .tmux
        .run(&["set-option", "-g", "status-left", "my status"])
        .unwrap();
    let config = server.cli(&["tmux-config", "--bindings-only"]);
    assert!(config.status.success());
    let path = server.root.join("bindings.conf");
    fs::write(&path, config.stdout).unwrap();
    server
        .tmux
        .run(&["source-file", path.to_str().unwrap()])
        .unwrap();
    assert_eq!(
        server
            .tmux
            .run(&["show-options", "-gv", "status-left"])
            .unwrap(),
        "my status"
    );
    let pane = server.tmux.panes().unwrap()[0].clone();
    server
        .tmux
        .run(&["break-pane", "-W", "-s", &pane.id])
        .unwrap();
    assert!(!server.cli(&["read", "--pane", &pane.id]).status.success());
    assert!(
        !server
            .cli(&["layout", "1x1", "--window", &window])
            .status
            .success()
    );
    assert!(server.cli(&["grid", "--window", &window]).status.success());
    assert_eq!(
        server
            .tmux
            .format(&pane.id, "#{pane_floating_flag}")
            .unwrap(),
        "1"
    );
}

#[test]
#[ignore = "requires tmux 3.8; CI explicitly runs this suite"]
fn returning_reading_preserves_external_new_panes_and_application_colors() {
    let server = Server::new();
    let window = server.window();
    workspace::resize(
        &server.tmux,
        &server.state(),
        &window,
        Grid::parse("2x1").unwrap(),
        false,
    )
    .unwrap();
    server.wait_idle();
    let originals = server.tmux.panes().unwrap();
    let selected = &originals[0];
    let background = &originals[1];
    server
        .tmux
        .run(&[
            "send-keys",
            "-t",
            &background.id,
            "printf '\\033[48;2;40;40;40mgray-bg\\033[0m\\n'",
            "Enter",
        ])
        .unwrap();
    wait(|| {
        server
            .tmux
            .run(&["capture-pane", "-e", "-p", "-t", &background.id])
            .is_ok_and(|s| s.contains("gray-bg") && s.contains("48;2;40;40;40"))
    });
    assert!(
        server
            .cli(&["read", "--pane", &selected.id])
            .status
            .success()
    );
    // A native split outside the helper changes the inventory while reading.
    let added = server
        .tmux
        .run(&[
            "split-window",
            "-h",
            "-d",
            "-t",
            &background.id,
            "-P",
            "-F",
            "#{pane_id}",
        ])
        .unwrap();
    assert!(server.cli(&["grid", "--window", &window]).status.success());
    let after = server.tmux.panes().unwrap();
    assert_eq!(after.len(), 3);
    assert!(after.iter().any(|p| p.id == added));
    for original in originals {
        assert!(
            after
                .iter()
                .any(|p| p.id == original.id && p.pid == original.pid && !p.floating)
        );
    }
}
