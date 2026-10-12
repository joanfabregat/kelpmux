//! Real tmux tests always use a private socket, empty config, and disposable state.
use kelpmux::{
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
        std::thread::spawn(move || {
            let mut bytes = [0_u8; 8192];
            loop {
                if unsafe { libc::read(drain.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) }
                    <= 0
                {
                    break;
                }
            }
        });
        wait(|| {
            server
                .tmux
                .run(&["list-clients", "-F", "#{client_name}"])
                .is_ok_and(|s| !s.is_empty())
        });
        Self { child, master }
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
    let records: serde_json::Value =
        serde_json::from_slice(&server.cli(&["journal"]).stdout).unwrap();
    assert_eq!(
        records.as_array().unwrap().last().unwrap()["event"],
        "exited"
    );
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
    client.keys(b"\x02p");
    std::thread::sleep(Duration::from_millis(100));
    client.keys(b"2\r");
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
