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
        observation: Default::default(),
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
        floating: false,
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
fn native_reading_requires_supported_server_version() {
    for version in ["3.8", "3.8a", "3.10", "4.0"] {
        assert!(kelpmux::workspace::native_floating_supported(version));
    }
    for version in ["3.3a", "3.6a", "3.7", "bad", "next-3.8"] {
        assert!(!kelpmux::workspace::native_floating_supported(version));
    }
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
        let (stream, _) = listener.accept().unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        let message = socket.read().unwrap().into_text().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&message).unwrap()["method"],
            "initialize"
        );
        socket
            .send(tungstenite::Message::Text(
                json!({"id":1,"result":{}}).to_string().into(),
            ))
            .unwrap();
        let message = socket.read().unwrap().into_text().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&message).unwrap()["method"],
            "initialized"
        );
        let message = socket.read().unwrap().into_text().unwrap();
        let request: serde_json::Value = serde_json::from_str(&message).unwrap();
        assert_eq!(request["method"], "thread/read");
        assert_eq!(request["params"]["includeTurns"], false);
        assert_eq!(request["params"]["threadId"], "thread-test");
        socket.send(tungstenite::Message::Text(json!({"id":2,"result":{"thread":{"id":"thread-test","name":"Review","status":{"type":"active","activeFlags":["waitingOnApproval"]}}}}).to_string().into())).unwrap();
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

#[test]
fn idle_is_not_completion_and_status_evidence_preserves_hook_event_time() {
    use kelpmux::model::{Observation, StatusSource};
    let dir = Temp::new();
    let mut store = Store::open(&dir.0).unwrap();
    let mut observed = agent();
    observed.observation = Observation::checked(StatusSource::LiveRecord, 100);
    store.reconcile(vec![observed.clone()], 100).unwrap();
    observed.status = Status::Idle;
    observed.observation.checked_at = 102;
    store.reconcile(vec![observed.clone()], 102).unwrap();
    assert_eq!(store.registry.agents[&observed.key()].status, Status::Idle);
    assert!(
        store.registry.agents[&observed.key()]
            .observation
            .event_at
            .is_none()
    );
    assert!(!observed.observation.stale(110));
    assert!(observed.observation.stale(113));
    store.registry.hooks.insert(
        observed.key(),
        kelpmux::journal::Hook {
            id: observed.session_id.clone().unwrap(),
            kind: observed.kind,
            status: Status::InputNeeded,
            at: 105,
        },
    );
    observed.status = Status::Unknown;
    observed.observation.source = StatusSource::Unknown;
    store.reconcile(vec![observed.clone()], 110).unwrap();
    let evidence = &store.registry.agents[&observed.key()].observation;
    assert_eq!(evidence.source, StatusSource::Hook);
    assert_eq!(evidence.checked_at, 110);
    assert_eq!(evidence.event_at, Some(105));
    let mut old_json = serde_json::to_value(observed).unwrap();
    old_json.as_object_mut().unwrap().remove("observation");
    let old: Agent = serde_json::from_value(old_json).unwrap();
    assert_eq!(old.observation.source, StatusSource::Unknown);
}

#[test]
fn registered_polling_adapters_share_process_discovery_and_isolate_failed_observations() {
    use kelpmux::discovery::polling::{Adapter, Poller};
    use std::sync::{Arc, atomic::AtomicUsize};
    struct Custom(Arc<AtomicUsize>);
    impl Adapter for Custom {
        fn kind(&self) -> Kind {
            Kind::Claude
        }
        fn matches(&self, process: &process::Process, _: &[String]) -> bool {
            process.command == "custom-agent"
        }
        fn begin_cycle(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        fn poll(
            &mut self,
            _: &Discovery,
            process: &process::Process,
            _: &[String],
            agent: &mut Agent,
        ) -> kelpmux::Result<()> {
            agent.status = Status::Working;
            if process.pid == 100 {
                return Err("unsupported metadata".into());
            }
            agent.session_id = Some("custom-session".into());
            Ok(())
        }
    }
    let temp = Temp::new();
    fixture_proc(&temp.0, 100, 1, "custom-agent", &["custom-agent"], 10);
    fixture_proc(&temp.0, 200, 1, "custom-agent", &["custom-agent"], 20);
    let cycles = Arc::new(AtomicUsize::new(0));
    let context = Discovery {
        proc_root: temp.0.clone(),
        codex_home: temp.0.join("codex"),
        claude_home: temp.0.join("claude"),
    };
    let mut poller = Poller::with_adapters(context, vec![Box::new(Custom(cycles.clone()))]);
    let result = poller.poll(&[pane(1, 100), pane(2, 200)], &[], 42).unwrap();
    assert_eq!(cycles.load(Ordering::SeqCst), 1);
    assert_eq!(result.tree.len(), 2);
    assert_eq!(result.agents[0].status, Status::Unknown);
    assert!(result.agents[0].session_id.is_none());
    assert_eq!(result.agents[1].status, Status::Working);
    assert_eq!(
        result.agents[1].session_id.as_deref(),
        Some("custom-session")
    );
}

#[test]
fn codex_shared_daemon_polling_caches_metadata_and_links_only_the_exact_client_identity() {
    use kelpmux::discovery::polling::{Link, Poller};
    use std::{
        os::unix::net::UnixListener,
        sync::{Arc, Mutex, atomic::AtomicBool},
        thread,
        time::{Duration, Instant},
    };
    let temp = Temp::new();
    let proc_root = temp.0.join("proc");
    fs::create_dir(&proc_root).unwrap();
    fixture_proc(
        &proc_root,
        100,
        1,
        "codex",
        &["codex", "resume", "alpha"],
        10,
    );
    fixture_proc(
        &proc_root,
        200,
        1,
        "codex",
        &["codex", "resume", "alpha"],
        20,
    );
    fixture_proc(&proc_root, 300, 1, "codex", &["codex"], 30);
    for pid in [100, 200] {
        fs::write(
            proc_root.join(pid.to_string()).join("environ"),
            "CODEX_THREAD_ID=alpha\0",
        )
        .unwrap();
    }
    let codex_home = temp.0.join("codex");
    fs::create_dir_all(codex_home.join("app-server-control")).unwrap();
    let listener =
        UnixListener::bind(codex_home.join("app-server-control/app-server-control.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let requests = Arc::new(Mutex::new(Vec::<(usize, String, String)>::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let (record, cancel) = (requests.clone(), stop.clone());
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut connection = 0;
        while !cancel.load(Ordering::SeqCst) && Instant::now() < deadline {
            let (stream, _) = match listener.accept() {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            connection += 1;
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            loop {
                let Ok(tungstenite::Message::Text(text)) = socket.read() else {
                    break;
                };
                let message: serde_json::Value = serde_json::from_str(&text).unwrap();
                let method = message["method"].as_str().unwrap();
                let id = message["params"]["threadId"].as_str().unwrap_or("");
                record
                    .lock()
                    .unwrap()
                    .push((connection, method.into(), id.into()));
                if method == "initialized" {
                    continue;
                }
                let result = match method {
                    "initialize" => json!({}),
                    "thread/loaded/list" => json!({"data":["alpha","beta","bad/id"]}),
                    "thread/read" => {
                        assert_eq!(message["params"]["includeTurns"], false);
                        json!({"thread":{"id":id,"name":format!("Conversation {id}"),"status":{"type":if id=="alpha" {"active"} else {"idle"}},"turns":[{"content":"never retained"}]}})
                    }
                    other => panic!("poller attempted side-effectful method {other}"),
                };
                socket
                    .send(tungstenite::Message::Text(
                        json!({"id":message["id"],"result":result})
                            .to_string()
                            .into(),
                    ))
                    .unwrap();
            }
        }
    });
    let context = Discovery {
        proc_root,
        codex_home,
        claude_home: temp.0.join("claude"),
    };
    let mut poller = Poller::new(context);
    let panes = [pane(1, 100), pane(2, 200), pane(3, 300)];
    let first = poller.poll(&panes, &[], 1).unwrap();
    assert_eq!(first.agents[0].status, Status::Working);
    assert_eq!(first.agents[1].status, Status::Working);
    assert!(
        first.agents[2].session_id.is_none(),
        "shared daemon alone must not identify an unbound TUI"
    );
    let sessions = poller.sessions(Kind::Codex).unwrap();
    assert_eq!(sessions.len(), 2);
    assert!(
        sessions
            .iter()
            .any(|session| session.id == "beta" && session.status == Status::Idle)
    );
    let link = Link {
        pane: "%3".into(),
        pid: 300,
        birth: 30,
        kind: Kind::Codex,
        session_id: "beta".into(),
    };
    let linked = poller.poll(&panes, std::slice::from_ref(&link), 2).unwrap();
    assert_eq!(linked.agents[2].session_id.as_deref(), Some("beta"));
    assert_eq!(linked.agents[2].status, Status::Idle);
    assert_eq!(linked.agents[2].source, "explicit-link");
    let stale = Link { birth: 31, ..link };
    let unlinked = poller.poll(&panes, &[stale], 3).unwrap();
    assert!(
        unlinked.agents[2].session_id.is_none(),
        "reused PID must not inherit a link"
    );
    drop(poller);
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|(connection, method, _)| *connection == 1 && method == "initialize")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|(connection, method, id)| *connection == 1
                && method == "thread/read"
                && id == "alpha")
            .count(),
        1
    );
}

#[test]
fn idle_prompt_is_idle_while_permission_notifications_request_attention() {
    assert_eq!(
        kelpmux::journal::hook_status(
            "Notification",
            &serde_json::json!({"notification_type":"idle_prompt"})
        ),
        Some(Status::Idle)
    );
    assert_eq!(
        kelpmux::journal::hook_status(
            "Notification",
            &serde_json::json!({"notification_type":"permission_prompt"})
        ),
        Some(Status::InputNeeded)
    );
    assert_eq!(
        kelpmux::journal::hook_status("PermissionRequest", &serde_json::json!({})),
        Some(Status::InputNeeded)
    );
}

#[test]
fn pending_automatic_client_selection_does_not_reuse_an_old_hook_identity() {
    let temp = Temp::new();
    let mut store = Store::open(&temp.0).unwrap();
    let mut observed = agent();
    observed.kind = Kind::Codex;
    store.reconcile(vec![observed.clone()], 10).unwrap();
    store
        .hook(
            "%1",
            Kind::Codex,
            &json!({"session_id":"session-test", "hook_event_name":"Stop"}),
        )
        .unwrap();
    observed.source = "client-bridge".into();
    observed.session_id = None;
    observed.status = Status::Unknown;
    store.reconcile(vec![observed], 11).unwrap();
    let current = store.registry.agents.values().next().unwrap();
    assert!(current.session_id.is_none());
    assert_eq!(current.status, Status::Unknown);
    assert!(
        store
            .hook(
                "%1",
                Kind::Codex,
                &json!({"session_id":"session-test", "hook_event_name":"Stop"})
            )
            .is_err()
    );
}

#[test]
fn codex_profiles_are_interactive_while_claude_print_mode_is_excluded() {
    let temp = Temp::new();
    fixture_proc(&temp.0, 100, 1, "zsh", &["zsh"], 10);
    fixture_proc(&temp.0, 101, 100, "codex", &["codex", "-p", "work"], 11);
    let discovery = Discovery {
        proc_root: temp.0.clone(),
        codex_home: temp.0.join("codex-home"),
        claude_home: temp.0.join("claude-home"),
    };
    assert_eq!(discovery.scan(&[pane(1, 100)], 20).unwrap().len(), 1);
    fs::write(temp.0.join("101/stat"), stat(101, 100, "claude", 'S', 11)).unwrap();
    fs::write(temp.0.join("101/cmdline"), b"claude\0-p\0prompt").unwrap();
    assert!(discovery.scan(&[pane(1, 100)], 21).unwrap().is_empty());
}

#[test]
fn terminal_program_evidence_wins_over_late_hooks_and_metadata_reconciliation() {
    use kelpmux::model::{Observation, StatusSource};
    let dir = Temp::new();
    let mut store = Store::open(&dir.0).unwrap();
    let mut observed = agent();
    observed.observation = Observation {
        source: StatusSource::Terminal,
        checked_at: kelpmux::now(),
        event_at: Some(kelpmux::now()),
    };
    store
        .reconcile(vec![observed.clone()], kelpmux::now())
        .unwrap();
    store
        .hook(
            "%1",
            Kind::Claude,
            &json!({"session_id":"session-test", "hook_event_name":"Stop"}),
        )
        .unwrap();
    assert_eq!(
        store.registry.agents[&observed.key()].status,
        Status::Working
    );
    assert_eq!(
        store.registry.agents[&observed.key()].observation.source,
        StatusSource::Terminal
    );
    observed.status = Status::Idle;
    store
        .reconcile(vec![observed.clone()], kelpmux::now())
        .unwrap();
    assert_eq!(store.registry.agents[&observed.key()].status, Status::Idle);
}
