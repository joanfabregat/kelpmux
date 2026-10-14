//! Joined PTY/transport/discovery regressions. Fixtures never launch real agents.
use kelpmux::{
    journal::Store,
    model::{Agent, Kind, Status},
    native::{delta, protocol::*, ui::DialogKind},
    process, shell_quote,
};
use std::{
    fs,
    io::Write,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Daemon {
    root: PathBuf,
    state: PathBuf,
    socket: PathBuf,
    child: Child,
}
impl Daemon {
    fn start() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kelpmux-phase2-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let state = root.join("kelpmux/workspaces/current");
        let socket = root.join("native.sock");
        let child = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
            .args(["server", "--socket"])
            .arg(&socket)
            .arg("--state-dir")
            .arg(&state)
            .args(["--shell", "/bin/sh", "--cwd"])
            .arg(&root)
            .args(["--size", "120x40"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut daemon = Self {
            root,
            state,
            socket,
            child,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !daemon.socket.exists() {
            assert!(
                daemon.child.try_wait().unwrap().is_none(),
                "server exited before becoming ready"
            );
            assert!(Instant::now() < deadline, "server startup timed out");
            thread::sleep(Duration::from_millis(10));
        }
        daemon
    }
    fn connect(&self) -> Peer {
        Peer::new(UnixStream::connect(&self.socket).unwrap())
    }
    fn attach(&self) -> Peer {
        let mut peer = self.connect();
        peer.send(ClientMessage::Attach {
            version: VERSION,
            cols: 120,
            rows: 40,
            takeover: false,
        });
        assert!(matches!(peer.read(), ServerMessage::Welcome { .. }));
        peer.wait(|_| true);
        peer
    }
    fn hook(&self, pane: u64, event: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut child = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
                .args(["native-event", "codex", "--state-dir"])
                .arg(&self.state)
                .arg("--pane")
                .arg(pane.to_string())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(serde_json::to_string(&serde_json::json!({"session_id":SESSION,"hook_event_name":event,"prompt":"MUST_NOT_STORE_PRIVATE"})).unwrap().as_bytes()).unwrap();
            let output = child.wait_with_output().unwrap();
            if output.status.success() {
                assert!(output.stdout.is_empty());
                break;
            }
            assert!(
                Instant::now() < deadline,
                "hook failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        if let Ok(mut peer) = UnixStream::connect(&self.socket) {
            let _ = write_message(&mut peer, &ClientMessage::Command(Action::Shutdown));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        if self.child.try_wait().ok().flatten().is_none() {
            // Failure cleanup is confined to this fixture's verified process tree.
            if let Ok(tree) = process::snapshot(std::path::Path::new("/proc")) {
                for pid in process::descendants(&tree, self.child.id()) {
                    if tree.get(&pid).is_some_and(|old| {
                        process::read(std::path::Path::new("/proc"), pid)
                            .is_ok_and(|current| current.start == old.start)
                    }) {
                        unsafe {
                            libc::kill(pid as i32, libc::SIGKILL);
                        }
                    }
                }
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
struct Peer {
    socket: UnixStream,
    frame: Option<Frame>,
    revision: u64,
    full: usize,
    patch: usize,
}
impl Peer {
    fn new(socket: UnixStream) -> Self {
        socket
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        Self {
            socket,
            frame: None,
            revision: 0,
            full: 0,
            patch: 0,
        }
    }
    fn send(&mut self, message: ClientMessage) {
        write_message(&mut self.socket, &message).unwrap();
    }
    fn read(&mut self) -> ServerMessage {
        match read_message::<ServerMessage>(&mut self.socket)
            .unwrap()
            .expect("server disconnected")
        {
            ServerMessage::Frame(frame) => {
                self.revision = 1;
                self.full += 1;
                self.frame = Some(frame.clone());
                ServerMessage::Frame(frame)
            }
            ServerMessage::Patch(patch) => {
                let frame = delta::apply(
                    self.frame.as_ref().expect("patch before full snapshot"),
                    self.revision,
                    &patch,
                )
                .unwrap();
                self.revision = patch.revision;
                self.patch += 1;
                self.frame = Some(frame.clone());
                ServerMessage::Frame(frame)
            }
            other => other,
        }
    }
    fn wait(&mut self, predicate: impl Fn(&Frame) -> bool) -> Frame {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            assert!(Instant::now() < deadline, "expected frame did not arrive");
            match self.read() {
                ServerMessage::Frame(frame) if predicate(&frame) => return frame,
                ServerMessage::Error(error) => panic!("unexpected server error: {error}"),
                _ => (),
            }
        }
    }
    fn action(&mut self, action: Action) {
        self.send(ClientMessage::Action(action));
    }
    fn input(&mut self, text: &str) {
        self.send(ClientMessage::Input(text.as_bytes().to_vec()));
    }
    fn panes(&mut self) -> Vec<PaneInfo> {
        self.send(ClientMessage::Inspect);
        loop {
            if let ServerMessage::State { panes, .. } = self.read() {
                return panes;
            }
        }
    }
    fn history(&mut self, id: u64) -> Vec<(i32, String)> {
        self.send(ClientMessage::History {
            pane: id,
            start: -1000,
            limit: 200,
        });
        loop {
            if let ServerMessage::History { pane, lines } = self.read() {
                assert_eq!(pane, id);
                return lines;
            }
        }
    }
    fn clipboard(&mut self) -> String {
        loop {
            match self.read() {
                ServerMessage::Clipboard(text) => return text,
                ServerMessage::Error(error) => panic!("copy failed: {error}"),
                _ => (),
            }
        }
    }
    fn error(&mut self) -> String {
        loop {
            if let ServerMessage::Error(error) = self.read() {
                return error;
            }
        }
    }
    fn screen(&mut self, id: u64) -> Screen {
        self.send(ClientMessage::InspectPane { pane: id });
        loop {
            if let ServerMessage::PaneState { screen, .. } = self.read() {
                return screen;
            }
        }
    }
}
fn text(frame: &Frame) -> String {
    frame.cells.iter().map(|cell| cell.text.as_str()).collect()
}
fn close(peer: &mut Peer, id: u64) {
    peer.action(Action::ClosePaneRequest(id));
    peer.wait(|frame| {
        frame
            .ui
            .dialog
            .is_some_and(|dialog| dialog.kind == DialogKind::CloseConfirm)
    });
    peer.action(Action::ClosePaneConfirm(id));
    peer.wait(|frame| {
        frame.ui.dialog.is_none() && !frame.panes.iter().any(|view| view.pane.id == id)
    });
}
const SESSION: &str = "019abcde-1234-5678-9012-123456789abc";

#[test]
fn scratch_is_persistent_above_tabs_and_excluded_from_safe_grid_and_last_main_close() {
    let daemon = Daemon::start();
    let mut peer = daemon.attach();
    let main = peer.panes()[0].id;
    peer.action(Action::Read(main));
    peer.wait(|frame| matches!(frame.reading,ReadingMode::Tabs(id)if id==main));
    peer.action(Action::Scratch);
    let opened = peer.wait(|frame| frame.ui.scratch_visible);
    let scratch = opened.ui.scratch.unwrap();
    assert_eq!(opened.active, scratch);
    assert!(
        opened
            .panes
            .iter()
            .any(|view| view.pane.id == scratch && view.overlay)
    );
    let scratch_pid = peer
        .panes()
        .iter()
        .find(|pane| pane.id == scratch)
        .unwrap()
        .pid;
    peer.input("sleep 30 & printf 'SCRATCH_''KEPT\\n'\n");
    peer.wait(|frame| text(frame).contains("SCRATCH_KEPT"));
    peer.action(Action::ScratchClose);
    peer.wait(|frame| !frame.ui.scratch_visible && frame.active == main);
    peer.action(Action::Scratch);
    peer.wait(|frame| frame.ui.scratch_visible && frame.active == scratch);
    assert_eq!(
        peer.panes()
            .iter()
            .find(|pane| pane.id == scratch)
            .unwrap()
            .pid,
        scratch_pid
    );
    assert!(
        peer.history(scratch)
            .iter()
            .any(|(_, line)| line.contains("SCRATCH_KEPT"))
    );
    peer.action(Action::Grid { cols: 2, rows: 1 });
    peer.wait(|frame| frame.ui.grid_shape == (2, 1));
    assert_eq!(peer.panes().len(), 3);
    assert_eq!(
        peer.panes()
            .iter()
            .find(|pane| pane.id == scratch)
            .unwrap()
            .pid,
        scratch_pid
    );
    peer.action(Action::ScratchClose);
    peer.wait(|frame| !frame.ui.scratch_visible);
    let mains: Vec<_> = peer
        .panes()
        .into_iter()
        .filter(|pane| pane.id != scratch)
        .collect();
    peer.wait(|frame| mains.iter().all(|pane| frame.ui.unused.contains(&pane.id)));
    peer.action(Action::Grid { cols: 1, rows: 1 });
    peer.wait(|frame| frame.ui.grid_shape == (1, 1));
    assert_eq!(peer.panes().len(), 2);
    peer.action(Action::Grid { cols: 2, rows: 1 });
    peer.wait(|frame| frame.ui.grid_shape == (2, 1) && !frame.ui.scratch_visible);
    let mains: Vec<_> = peer
        .panes()
        .into_iter()
        .filter(|pane| pane.id != scratch)
        .collect();
    peer.wait(|frame| mains.iter().all(|pane| frame.ui.unused.contains(&pane.id)));
    peer.action(Action::Scratch);
    peer.wait(|frame| frame.ui.scratch_visible);
    peer.action(Action::Grid { cols: 1, rows: 1 });
    peer.wait(|frame| {
        frame.ui.grid_shape == (1, 1) && frame.ui.scratch_visible && frame.active == scratch
    });
    assert_eq!(
        peer.panes()
            .iter()
            .find(|pane| pane.id == scratch)
            .unwrap()
            .pid,
        scratch_pid
    );
    let main = peer
        .panes()
        .into_iter()
        .find(|pane| pane.id != scratch)
        .unwrap()
        .id;
    close(&mut peer, main);
    let panes = peer.panes();
    assert_eq!(panes.len(), 2);
    assert!(
        panes
            .iter()
            .any(|pane| pane.id == scratch && pane.pid == scratch_pid)
    );
    assert!(panes.iter().any(|pane| pane.id != scratch));
    close(&mut peer, scratch);
    let frame = peer.frame.as_ref().unwrap();
    assert_eq!(frame.ui.scratch, None);
    assert!(!frame.ui.scratch_visible);
    assert_eq!(frame.ui.scratch_return, None);
    assert_eq!(peer.panes().len(), 1);
}

#[test]
fn search_unicode_history_clipboard_selection_and_alternate_reveal_use_real_ptys() {
    let daemon = Daemon::start();
    let mut peer = daemon.attach();
    peer.action(Action::Grid { cols: 2, rows: 1 });
    peer.wait(|frame| frame.ui.grid_shape == (2, 1));
    let panes = peer.panes();
    for pane in &panes {
        peer.action(Action::Focus(pane.id));
        peer.input(&format!("printf 'UNICODE_''探査_%s\\n' alpha; i=0; while [ $i -lt 45 ]; do printf 'history %s\\n' $i; i=$((i+1)); done; printf 'OUTPUT_''DONE_{}\\n'\n",pane.id));
        peer.wait(|frame| {
            frame.active == pane.id && text(frame).contains(&format!("OUTPUT_DONE_{}", pane.id))
        });
    }
    peer.action(Action::Search {
        query: "UNICODE_探査".into(),
        all: true,
    });
    let found = peer.wait(|frame| {
        frame
            .ui
            .search_hits
            .iter()
            .map(|hit| hit.pane)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == 2
    });
    assert!(found.ui.search_hits.iter().all(|hit| hit.line < 0));
    let index = found
        .ui
        .search_hits
        .iter()
        .position(|hit| hit.pane == panes[0].id)
        .unwrap();
    peer.action(Action::SearchCopy(index));
    assert!(peer.clipboard().contains("UNICODE_探査_alpha"));
    peer.action(Action::SearchSelect(index));
    let shown = peer.wait(|frame| frame.active == panes[0].id && frame.ui.dialog.is_none());
    assert!(text(&shown).contains("UNICODE_探査_alpha"));
    // History is read-only and bounded; a standalone management connection cannot take control.
    let mut observer = daemon.connect();
    let before = peer.screen(panes[0].id);
    assert!(observer.history(panes[0].id).len() <= 200);
    assert_eq!(peer.screen(panes[0].id), before);
    observer.send(ClientMessage::History {
        pane: panes[0].id,
        start: 0,
        limit: 201,
    });
    assert!(observer.error().contains("200"));
    peer.action(Action::Scroll {
        pane: panes[0].id,
        lines: -10000,
    });
    peer.input("printf '\\033[2J\\033[HSELECT_''ME'\n");
    peer.wait(|frame| text(frame).contains("SELECT_ME"));
    let cursor = peer.screen(panes[0].id).cursor.unwrap();
    peer.action(Action::SelectionStart);
    peer.action(Action::SelectionMove {
        dx: -(cursor.position.x as i16),
        dy: 0,
    });
    peer.action(Action::Copy(panes[0].id));
    let copied = peer.clipboard();
    assert!(
        copied.contains("SELECT_ME"),
        "selection copied {copied:?} from {cursor:?}"
    );
    peer.input("printf '\\033[?1049hALT_''LIVE\\n'\n");
    peer.wait(|frame| text(frame).contains("ALT_LIVE"));
    peer.action(Action::Search {
        query: "UNICODE_探査".into(),
        all: false,
    });
    let alt = peer.wait(|frame| !frame.ui.search_hits.is_empty());
    peer.action(Action::SearchSelect(0));
    assert!(peer.error().contains("alternate screen"));
    peer.action(Action::SearchCopy(0));
    assert!(peer.clipboard().contains("UNICODE_探査"));
    assert!(!alt.ui.search_hits.is_empty());
    peer.input("printf '\\033[?1049l'\n");
}

#[test]
fn fake_codex_lifecycle_notifications_deduplicate_acknowledge_reconnect_and_bell() {
    let daemon = Daemon::start();
    let executable = daemon.root.join("codex");
    fs::copy("/bin/bash", &executable).unwrap();
    let mut peer = daemon.attach();
    let pane = peer.panes()[0].id;
    peer.input(&format!(
        "{} -c {} _ resume {}\n",
        shell_quote(executable.to_str().unwrap()),
        shell_quote("trap 'printf \"\\a\"' USR1; while :; do sleep 0.1; done"),
        SESSION
    ));
    let discovered = peer.wait(|frame| {
        frame.panes.iter().any(|view| {
            view.pane.id == pane
                && view.pane.kind == Some(Kind::Codex)
                && view.pane.session_id.as_deref() == Some(SESSION)
        })
    });
    assert!(discovered.ui.notifications.is_empty());
    let agent = Store::open(&daemon.state.join("native"))
        .unwrap()
        .registry
        .agents
        .values()
        .find(|agent| agent.kind == Kind::Codex && agent.ended_at.is_none())
        .unwrap()
        .clone();
    let identity = process::read(std::path::Path::new("/proc"), agent.pid).unwrap();
    assert_eq!(identity.command, "codex");
    assert_eq!(identity.start, agent.process_start);
    assert_eq!(agent.session_id.as_deref(), Some(SESSION));
    for (event, status) in [
        ("UserPromptSubmit", Status::Working),
        ("PermissionRequest", Status::InputNeeded),
        ("Stop", Status::TurnFinished),
        ("StopFailure", Status::Failed),
    ] {
        daemon.hook(pane, event);
        peer.wait(|frame| {
            frame
                .panes
                .iter()
                .any(|view| view.pane.id == pane && view.pane.status == status)
        });
    }
    let events = peer.frame.as_ref().unwrap().ui.notifications.clone();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.status == Status::InputNeeded)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.status == Status::TurnFinished)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.status == Status::Failed)
            .count(),
        1
    );
    let failed = events
        .iter()
        .find(|event| event.status == Status::Failed)
        .unwrap()
        .id;
    daemon.hook(pane, "StopFailure");
    peer.send(ClientMessage::Ping);
    while !matches!(peer.read(), ServerMessage::Pong) {}
    peer.action(Action::Acknowledge(failed));
    peer.wait(|frame| {
        frame
            .ui
            .notifications
            .iter()
            .any(|event| event.id == failed && event.read)
    });
    peer.action(Action::Detach);
    while !matches!(peer.read(), ServerMessage::Detached) {}
    drop(peer);
    let mut peer = daemon.attach();
    assert!(
        peer.frame
            .as_ref()
            .unwrap()
            .ui
            .notifications
            .iter()
            .any(|event| event.id == failed && event.read)
    );
    assert_eq!(
        peer.frame
            .as_ref()
            .unwrap()
            .ui
            .notifications
            .iter()
            .filter(|event| event.status == Status::Failed)
            .count(),
        1
    );
    // Only the verified, fixture-owned process receives this signal.
    assert_eq!(
        process::read(std::path::Path::new("/proc"), agent.pid)
            .unwrap()
            .start,
        agent.process_start
    );
    assert_eq!(unsafe { libc::kill(agent.pid as i32, libc::SIGUSR1) }, 0);
    peer.wait(|frame| {
        frame
            .ui
            .notifications
            .iter()
            .any(|event| event.status == Status::Unknown && event.title.starts_with("Bell · "))
    });
    assert_eq!(unsafe { libc::kill(agent.pid as i32, libc::SIGTERM) }, 0);
    peer.wait(|frame| {
        frame
            .ui
            .notifications
            .iter()
            .filter(|event| event.status == Status::Exited)
            .count()
            == 1
    });
    for name in ["registry.json", "journal.jsonl"] {
        assert!(
            !fs::read_to_string(daemon.state.join("native").join(name))
                .unwrap()
                .contains("MUST_NOT_STORE_PRIVATE")
        );
    }
}

#[test]
fn incremental_frames_survive_backpressure_takeover_and_metadata_only_diagnostics() {
    let daemon = Daemon::start();
    let mut controller = daemon.attach();
    let pane = controller.panes()[0].id;
    let mut observer = daemon.attach();
    let initial = observer.full;
    controller.input("i=0; while [ $i -lt 80 ]; do printf '\\033[41m\\033[2K\\033[0m\\033[31mCOLOR_%s\\033[0m\\n' $i; i=$((i+1)); sleep 0.04; done; printf 'TRANSPORT_''DONE\\n'\n");
    controller.wait(|frame| text(frame).contains("TRANSPORT_DONE"));
    assert!(controller.patch > 0);
    let received = observer.wait(|frame| text(frame).contains("TRANSPORT_DONE"));
    assert!(
        observer.full > initial,
        "slow observer needed replacement snapshots"
    );
    assert!(
        received
            .cells
            .iter()
            .any(|cell| cell.fg == Color::Indexed(1))
    );
    let patches = observer.patch;
    // Keep reading while the shell waits, draining both command echo and any
    // recovery snapshot before a separate output change arrives.
    controller.input("sleep 1; printf 'PATCH_''AGAIN\\n'\n");
    observer.wait(|frame| text(frame).contains("PATCH_AGAIN"));
    // The first small update can still replace an in-flight recovery frame.
    // Keep consuming updates until the queue drains and revisioned patches
    // resume. The delayed output above separates recovery from new output.
    let deadline = Instant::now() + Duration::from_secs(6);
    while observer.patch == patches && Instant::now() < deadline {
        observer.read();
    }
    assert!(
        observer.patch > patches,
        "transport did not resume patches after recovery snapshot"
    );
    observer.action(Action::Takeover);
    observer.wait(|frame| frame.controller);
    controller.wait(|frame| !frame.controller);
    observer.send(ClientMessage::Resize { cols: 90, rows: 30 });
    observer.wait(|frame| frame.cols == 90 && frame.rows == 30);
    observer.input("printf 'NEW_''CONTROLLER\\n'\n");
    observer.wait(|frame| text(frame).contains("NEW_CONTROLLER"));
    let mut management = daemon.connect();
    management.send(ClientMessage::Diagnostics);
    let report = loop {
        if let ServerMessage::Diagnostics(report) = management.read() {
            break report;
        }
    };
    assert!(report.patch_frames > 0);
    assert!(report.pty_bytes > 0);
    let json = serde_json::to_value(report).unwrap();
    for field in ["cwd", "title", "session_id", "cells", "output"] {
        assert!(json.get(field).is_none());
    }
    assert!(management.history(pane).len() <= 200);
}

#[test]
fn recovery_skips_busy_workspace_without_stalling_pty_and_copies_grouped_resume_metadata() {
    let daemon = Daemon::start();
    let sibling = daemon.state.parent().unwrap().join("other");
    fs::create_dir(&sibling).unwrap();
    fs::set_permissions(&sibling, fs::Permissions::from_mode(0o700)).unwrap();
    let time = kelpmux::now();
    let fixture = Agent {
        pane: "%1".into(),
        window: "@0".into(),
        pid: 999,
        process_start: 1,
        kind: Kind::Claude,
        session_id: Some("known-recovery-id".into()),
        name: "recovery fixture".into(),
        cwd: "/fixture-project".into(),
        status: Status::TurnFinished,
        source: "fixture".into(),
        first_seen: time,
        last_seen: time,
        ended_at: None,
    };
    Store::open(&sibling.join("native"))
        .unwrap()
        .reconcile(vec![fixture], time)
        .unwrap();
    let mut peer = daemon.attach();
    let _lock = Store::open(&daemon.state.join("native")).unwrap();
    let begin = Instant::now();
    peer.action(Action::Recovery);
    peer.input("printf 'RECOVERY_''RESPONSIVE\\n'\n");
    peer.action(Action::DialogClose);
    peer.wait(|frame| text(frame).contains("RECOVERY_RESPONSIVE"));
    assert!(begin.elapsed() < Duration::from_secs(2));
    peer.action(Action::Recovery);
    let found = peer.wait(|frame| {
        !frame.ui.recovery_loading
            && frame
                .ui
                .recovery
                .iter()
                .any(|item| item.workspace == "other")
    });
    let index = found
        .ui
        .recovery
        .iter()
        .position(|item| item.workspace == "other")
        .unwrap();
    assert_eq!(found.ui.recovery[index].entry.cwd, "/fixture-project");
    peer.action(Action::RecoveryCopy(index));
    assert_eq!(peer.clipboard(), "claude --resume 'known-recovery-id'");
}

#[test]
fn search_resets_result_selection_and_reveals_hidden_scratch() {
    let daemon = Daemon::start();
    let mut peer = daemon.attach();
    peer.action(Action::Scratch);
    let scratch = peer
        .wait(|frame| frame.ui.scratch_visible)
        .ui
        .scratch
        .unwrap();
    peer.input("printf 'HIDDEN_''SCRATCH_MATCH\\n'\n");
    peer.wait(|frame| text(frame).contains("HIDDEN_SCRATCH_MATCH"));
    peer.action(Action::ScratchClose);
    peer.wait(|frame| !frame.ui.scratch_visible);
    peer.action(Action::SearchOpen);
    peer.wait(|frame| {
        frame
            .ui
            .dialog
            .is_some_and(|dialog| dialog.kind == DialogKind::Search)
    });
    peer.action(Action::Search {
        query: "HIDDEN_SCRATCH_MATCH".into(),
        all: true,
    });
    let found = peer.wait(|frame| frame.ui.search_hits.iter().any(|hit| hit.pane == scratch));
    let index = found
        .ui
        .search_hits
        .iter()
        .position(|hit| hit.pane == scratch)
        .unwrap();
    peer.action(Action::SearchSelect(index));
    peer.wait(|frame| {
        frame.ui.scratch_visible && frame.active == scratch && frame.ui.dialog.is_none()
    });
    peer.action(Action::ScratchClose);
    peer.wait(|frame| !frame.ui.scratch_visible);
    peer.input("i=0; while [ $i -lt 12 ]; do printf 'MANY_''MATCH_%s\\n' $i; i=$((i+1)); done\n");
    peer.wait(|frame| text(frame).contains("MANY_MATCH_11"));
    peer.action(Action::SearchOpen);
    peer.wait(|frame| {
        frame
            .ui
            .dialog
            .is_some_and(|dialog| dialog.kind == DialogKind::Search)
    });
    peer.action(Action::Search {
        query: "MANY_MATCH".into(),
        all: false,
    });
    peer.wait(|frame| frame.ui.search_hits.len() >= 12);
    peer.action(Action::DialogPick(8));
    peer.wait(|frame| frame.ui.dialog.is_some_and(|dialog| dialog.selection == 8));
    peer.action(Action::Search {
        query: "MANY_MATCH_11".into(),
        all: false,
    });
    let found = peer.wait(|frame| frame.ui.search_query == "MANY_MATCH_11");
    assert_eq!(found.ui.search_hits.len(), 1);
    assert_eq!(found.ui.dialog.unwrap().selection, 0);
}
