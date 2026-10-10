use kelpmux::{
    discovery::{Discovery, codex_status, resume_id, rollout_status},
    journal::{RETENTION, Store, hook_status},
    label,
    model::{Agent, Kind, Status},
    process, shell_quote,
    tmux::Pane,
    workspace::{Grid, checksum, layout, shrink_plan},
};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "kelpmux-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn agent() -> Agent {
    Agent {
        pane: "%1".into(),
        window: "@0".into(),
        pid: 100,
        process_start: 1234,
        kind: Kind::Claude,
        session_id: Some("session-test".into()),
        name: "backend".into(),
        cwd: "/project".into(),
        status: Status::Working,
        source: "fixture".into(),
        first_seen: 10,
        last_seen: 10,
        ended_at: None,
    }
}
fn pane(index: usize, pid: u32) -> Pane {
    Pane {
        id: format!("%{index}"),
        window: "@0".into(),
        pid,
        index,
        cwd: "/project".into(),
        dead: false,
    }
}
fn stat(pid: u32, parent: u32, comm: &str, state: char, start: u64) -> String {
    let mut fields = vec!["0".to_owned(); 22];
    fields[0] = state.to_string();
    fields[1] = parent.to_string();
    fields[2] = pid.to_string();
    fields[4] = "34816".into();
    fields[5] = pid.to_string();
    fields[19] = start.to_string();
    format!("{pid} ({comm}) {}", fields.join(" "))
}
fn fixture_proc(
    root: &std::path::Path,
    pid: u32,
    parent: u32,
    comm: &str,
    args: &[&str],
    start: u64,
) {
    let p = root.join(pid.to_string());
    fs::create_dir_all(p.join("fd")).unwrap();
    fs::write(p.join("stat"), stat(pid, parent, comm, 'S', start)).unwrap();
    fs::write(p.join("cmdline"), args.join("\0")).unwrap();
    fs::write(p.join("environ"), "").unwrap();
    std::os::unix::fs::symlink("/project", p.join("cwd")).unwrap();
}

#[test]
fn stat_handles_spaces_and_parentheses() {
    let p = process::parse_stat(100, &stat(100, 1, "a (b) c", 'S', 12345)).unwrap();
    assert_eq!(p.command, "a (b) c");
    assert_eq!(p.start, 12345);
    assert_eq!(p.foreground, 100);
    assert!(process::parse_stat(1, "bad").is_err());
}
#[test]
fn idle_shell_excludes_foreground_background_and_stopped_jobs() {
    let mut tree = std::collections::BTreeMap::new();
    tree.insert(
        100,
        process::parse_stat(100, &stat(100, 1, "zsh", 'S', 12)).unwrap(),
    );
    assert!(process::idle(&tree, 100, false));
    for state in ['S', 'T', 'Z'] {
        tree.insert(
            101,
            process::parse_stat(101, &stat(101, 100, "ping", state, 13)).unwrap(),
        );
        assert!(!process::idle(&tree, 100, false));
    }
    tree.remove(&101);
    tree.get_mut(&100).unwrap().foreground = 101;
    assert!(!process::idle(&tree, 100, false));
    tree.get_mut(&100).unwrap().foreground = 100;
    tree.get_mut(&100).unwrap().state = 'R';
    assert!(!process::idle(&tree, 100, false));
    tree.get_mut(&100).unwrap().state = 'T';
    assert!(!process::idle(&tree, 100, false));
    assert!(process::idle(&tree, 100, true));
}
#[test]
fn shrink_is_all_or_nothing_and_prefers_high_indices() {
    let panes: Vec<_> = (1..=9).map(|n| pane(n, n as u32)).collect();
    assert!(shrink_plan(&panes, |pid| pid >= 8, 6).is_err());
    let victims = shrink_plan(&panes, |pid| pid >= 6, 6).unwrap();
    assert_eq!(
        victims.iter().map(|p| p.index).collect::<Vec<_>>(),
        [9, 8, 7]
    );
    assert!(shrink_plan(&panes, |_| false, 9).unwrap().is_empty());
}
#[test]
fn layouts_cover_all_presets_and_reject_small_terminals() {
    for shape in ["1x1", "2x1", "2x2", "3x2", "2x3", "3x3"] {
        let grid = Grid::parse(shape).unwrap();
        let panes: Vec<_> = (1..=grid.count()).map(|n| format!("%{n}")).collect();
        let value = layout(grid, 301, 91, &panes).unwrap();
        let (sum, body) = value.split_once(',').unwrap();
        assert_eq!(u16::from_str_radix(sum, 16).unwrap(), checksum(body));
        assert!(body.starts_with("301x91,0,0"));
        assert!(layout(grid, 1, 1, &panes).is_err());
    }
    assert!(Grid::parse("0x3").is_err());
    assert!(Grid::parse("99x99").is_err());
    assert!(
        layout(
            Grid::parse("1x1").unwrap(),
            80,
            24,
            &["%1;kill-server".into()]
        )
        .is_err()
    );
}
#[test]
fn safe_labels_and_shell_arguments() {
    assert_eq!(label("x#{exec}y\n\u{1b}"), "x##{exec}y");
    assert_eq!(shell_quote("a'b"), "'a'\\''b'");
}
#[test]
fn claude_discovery_validates_pid_birth_and_deduplicates_nested_agents() {
    let temp = Temp::new();
    let proc_root = temp.0.join("proc");
    fs::create_dir(&proc_root).unwrap();
    fixture_proc(&proc_root, 100, 1, "zsh", &["zsh"], 10);
    fixture_proc(&proc_root, 101, 100, "claude", &["claude"], 11);
    fixture_proc(&proc_root, 102, 101, "claude", &["claude"], 12);
    let claude = temp.0.join("claude");
    fs::create_dir_all(claude.join("sessions")).unwrap();
    let metadata = claude.join("sessions/101.json");
    fs::write(&metadata, json!({"pid":101,"procStart":"11","sessionId":"session-test","name":"Auth","status":"running"}).to_string()).unwrap();
    let d = Discovery {
        proc_root,
        claude_home: claude,
        codex_home: temp.0.join("codex"),
    };
    let found = d.scan(&[pane(1, 100)], 20).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].session_id.as_deref(), Some("session-test"));
    assert_eq!(found[0].name, "Auth");
    assert_eq!(found[0].status, Status::Working);
    fs::write(
        metadata,
        json!({"pid":101,"procStart":"999","sessionId":"wrong-session"}).to_string(),
    )
    .unwrap();
    assert!(d.scan(&[pane(1, 100)], 20).unwrap()[0].session_id.is_none());
}
#[test]
fn codex_identity_is_never_guessed_from_directory() {
    let temp = Temp::new();
    let root = temp.0.join("proc");
    fs::create_dir(&root).unwrap();
    fixture_proc(&root, 100, 1, "zsh", &["zsh"], 10);
    fixture_proc(&root, 101, 100, "codex", &["codex"], 11);
    let d = Discovery {
        proc_root: root.clone(),
        claude_home: temp.0.join("claude"),
        codex_home: temp.0.join("codex"),
    };
    let first = d.scan(&[pane(1, 100)], 20).unwrap();
    assert!(first[0].session_id.is_none());
    assert_eq!(first[0].status, Status::Unknown);
    fs::write(
        root.join("101/environ"),
        "CODEX_THREAD_ID=thread-test\0API_KEY=do-not-persist\0",
    )
    .unwrap();
    let found = d.scan(&[pane(1, 100)], 20).unwrap();
    assert_eq!(found[0].session_id.as_deref(), Some("thread-test"));
    assert!(
        !serde_json::to_string(&found)
            .unwrap()
            .contains("do-not-persist")
    );
}
#[test]
fn excludes_headless_and_daemon_processes() {
    let temp = Temp::new();
    let root = temp.0.join("proc");
    fs::create_dir(&root).unwrap();
    fixture_proc(&root, 100, 1, "codex", &["codex", "app-server"], 10);
    let d = Discovery {
        proc_root: root,
        claude_home: temp.0.join("claude"),
        codex_home: temp.0.join("codex"),
    };
    assert!(d.scan(&[pane(1, 100)], 20).unwrap().is_empty());
}
#[test]
fn explicit_resume_and_live_flags() {
    assert!(resume_id(&["codex".into(), "resume".into(), "--last".into()], false).is_none());
    assert_eq!(
        resume_id(
            &[
                "codex".into(),
                "resume".into(),
                "01a12332-b71d-7400-967f-627f1e6a29e9".into()
            ],
            false
        )
        .as_deref(),
        Some("01a12332-b71d-7400-967f-627f1e6a29e9")
    );
    assert_eq!(
        codex_status(&json!({"type":"active","activeFlags":["waitingOnUserInput"]})),
        Status::InputNeeded
    );
    assert_eq!(
        codex_status(&json!({"type":"active","activeFlags":[]})),
        Status::Working
    );
}
#[test]
fn transcript_tail_does_not_confuse_assistant_text_with_status() {
    let temp = Temp::new();
    let path = temp.0.join("rollout.jsonl");
    fs::write(
        &path,
        [
            json!({"type":"event_msg","payload":{"type":"task_started"}}),
            json!({"type":"response_item","payload":{"text":"task_complete"}}),
        ]
        .iter()
        .map(|v| format!("{v}\n"))
        .collect::<String>(),
    )
    .unwrap();
    assert_eq!(rollout_status(&path), Status::Working);
    use std::io::Write;
    writeln!(
        fs::OpenOptions::new().append(true).open(&path).unwrap(),
        "{}",
        json!({"type":"event_msg","payload":{"type":"task_complete"}})
    )
    .unwrap();
    assert_eq!(rollout_status(&path), Status::TurnFinished);
}
#[test]
fn journal_tracks_moves_exits_retention_and_long_running_sessions() {
    let temp = Temp::new();
    let mut store = Store::open(&temp.0).unwrap();
    let a = agent();
    store.reconcile(vec![a.clone()], 10).unwrap();
    store.reconcile(vec![a.clone()], 11).unwrap();
    assert_eq!(store.records().unwrap().len(), 1);
    let mut moved = a.clone();
    moved.window = "@99".into();
    store.reconcile(vec![moved], 12).unwrap();
    assert_eq!(store.records().unwrap().len(), 2);
    store.reconcile(vec![a.clone()], RETENTION + 20).unwrap();
    assert_eq!(store.registry.agents.len(), 1);
    assert!(store.records().unwrap().iter().all(|r| r.at >= 20));
    store.reconcile(vec![], RETENTION + 21).unwrap();
    assert_eq!(
        store.registry.agents.values().next().unwrap().status,
        Status::Exited
    );
    store.reconcile(vec![], RETENTION * 2 + 22).unwrap();
    assert!(store.registry.agents.is_empty());
    assert!(store.records().unwrap().is_empty());
}
#[test]
fn hooks_keep_only_metadata_and_reject_identity_conflicts() {
    let temp = Temp::new();
    let mut store = Store::open(&temp.0).unwrap();
    store.reconcile(vec![agent()], 10).unwrap();
    store.hook("%1",Kind::Claude,&json!({"session_id":"session-test","hook_event_name":"PermissionRequest","prompt":"secret-prompt"})).unwrap();
    assert_eq!(
        store.registry.agents.values().next().unwrap().status,
        Status::InputNeeded
    );
    assert!(
        !fs::read_to_string(temp.0.join("journal.jsonl"))
            .unwrap()
            .contains("secret-prompt")
    );
    assert!(
        store
            .hook(
                "%1",
                Kind::Claude,
                &json!({"session_id":"another","hook_event_name":"Stop"})
            )
            .is_err()
    );
    assert_eq!(hook_status("Stop", &json!({"stop_hook_active":true})), None);
}
#[test]
fn name_override_survives_discovery_refresh() {
    let temp = Temp::new();
    let mut store = Store::open(&temp.0).unwrap();
    store.reconcile(vec![agent()], 10).unwrap();
    store.rename("%1", "Friendly name").unwrap();
    store.reconcile(vec![agent()], 11).unwrap();
    assert_eq!(
        store.registry.agents.values().next().unwrap().name,
        "Friendly name"
    );
}
#[test]
fn state_rejects_symlink_directory_and_corruption() {
    let temp = Temp::new();
    let link = temp.0.join("link");
    std::os::unix::fs::symlink(&temp.0, &link).unwrap();
    assert!(Store::open(&link).is_err());
    fs::write(temp.0.join("registry.json"), "broken").unwrap();
    assert!(Store::open(&temp.0).is_err());
}

#[test]
fn interrupted_journal_append_preserves_completed_records_and_accepts_new_events() {
    use std::io::Write;
    let temp = Temp::new();
    {
        let mut store = Store::open(&temp.0).unwrap();
        store.reconcile(vec![agent()], 10).unwrap();
    }
    let path = temp.0.join("journal.jsonl");
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"interrupted\":")
        .unwrap();
    let mut store = Store::open(&temp.0).unwrap();
    assert_eq!(store.records().unwrap().len(), 1);
    store.reconcile(vec![], 11).unwrap();
    assert_eq!(store.records().unwrap().len(), 2);
    assert!(!fs::read_to_string(path).unwrap().contains("interrupted"));
}

#[test]
fn codex_native_child_reads_existing_daemon_metadata_without_starting_a_thread() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    let temp = Temp::new();
    let root = temp.0.join("proc");
    fs::create_dir(&root).unwrap();
    fixture_proc(&root, 100, 1, "zsh", &["zsh"], 10);
    fixture_proc(
        &root,
        101,
        100,
        "node",
        &["node", "/npm/@openai/codex/bin/codex.js"],
        11,
    );
    fixture_proc(&root, 102, 101, "codex", &["codex"], 12);
    fs::write(root.join("102/environ"), "CODEX_THREAD_ID=thread-test\0").unwrap();
    let codex = temp.0.join("codex");
    let socket_dir = codex.join("app-server-control");
    fs::create_dir_all(&socket_dir).unwrap();
    let listener = UnixListener::bind(socket_dir.join("app-server-control.sock")).unwrap();
    let responder = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).unwrap()["method"],
            "initialize"
        );
        writeln!(stream, "{}", json!({"id":1,"result":{}})).unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).unwrap()["method"],
            "initialized"
        );
        line.clear();
        reader.read_line(&mut line).unwrap();
        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["method"], "thread/read");
        assert_eq!(request["params"]["includeTurns"], false);
        assert_eq!(request["params"]["threadId"], "thread-test");
        writeln!(stream, "{}", json!({"id":2,"result":{"thread":{"id":"thread-test","name":"Review","status":{"type":"active","activeFlags":["waitingOnApproval"]}}}})).unwrap();
    });
    let found = Discovery {
        proc_root: root,
        claude_home: temp.0.join("claude"),
        codex_home: codex,
    }
    .scan(&[pane(1, 100)], 20)
    .unwrap();
    responder.join().unwrap();
    assert_eq!(found[0].pid, 102);
    assert_eq!(found[0].name, "Review");
    assert_eq!(found[0].status, Status::InputNeeded);
}
