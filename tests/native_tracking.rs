use kelpmux::{
    discovery::Discovery,
    journal::{RETENTION, Store},
    model::{Agent, Kind, Status},
    native::{
        hooks,
        protocol::PaneInfo,
        tracking::{filtered, recovery_at},
        ui::AgentFilter,
    },
    process,
    tmux::Pane,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "kelpmux-tracking-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn agent(time: u64) -> Agent {
    Agent {
        pane: "%1".into(),
        window: "@0".into(),
        pid: 101,
        process_start: 10,
        kind: Kind::Codex,
        session_id: Some("session-safe".into()),
        name: "backend".into(),
        cwd: "/project".into(),
        status: Status::Working,
        source: "fixture".into(),
        observation: Default::default(),
        first_seen: time,
        last_seen: time,
        ended_at: None,
    }
}
#[test]
fn filters_only_agent_attention_and_completion_preserving_order() {
    let panes = vec![
        PaneInfo {
            id: 1,
            status: Status::Working,
            ..Default::default()
        },
        PaneInfo {
            id: 2,
            status: Status::InputNeeded,
            kind: Some(Kind::Claude),
            ..Default::default()
        },
        PaneInfo {
            id: 3,
            status: Status::Failed,
            ..Default::default()
        },
        PaneInfo {
            id: 4,
            status: Status::TurnFinished,
            kind: Some(Kind::Codex),
            ..Default::default()
        },
        PaneInfo {
            id: 5,
            status: Status::Failed,
            kind: Some(Kind::Codex),
            ..Default::default()
        },
        PaneInfo {
            id: 6,
            status: Status::Exited,
            kind: Some(Kind::Claude),
            ..Default::default()
        },
        PaneInfo {
            id: 7,
            status: Status::Idle,
            kind: Some(Kind::Codex),
            ..Default::default()
        },
    ];
    assert_eq!(filtered(&panes, AgentFilter::All).len(), 7);
    assert_eq!(
        filtered(&panes, AgentFilter::Attention)
            .iter()
            .map(|p| p.id)
            .collect::<Vec<_>>(),
        vec![2, 5]
    );
    assert_eq!(
        filtered(&panes, AgentFilter::Finished)
            .iter()
            .map(|p| p.id)
            .collect::<Vec<_>>(),
        vec![4, 6]
    );
}
#[test]
fn recovery_retains_known_session_transitions_and_pid_reuse_without_prompt_commands() {
    let root = Temp::new();
    let time = RETENTION + 100;
    let mut store = Store::open(&root.0.join("native")).unwrap();
    let mut first = agent(time - 30);
    first.session_id = None;
    store.append("discovered", &first, time - 30).unwrap();
    first.session_id = Some("session-safe".into());
    store.append("changed", &first, time - 20).unwrap();
    first.session_id = Some("session-other".into());
    first.last_seen = time - 10;
    store.append("changed", &first, time - 10).unwrap();
    let mut reused = agent(time);
    reused.process_start = 11;
    reused.kind = Kind::Claude;
    reused.name = "$(touch /not-run);'".into();
    reused.cwd = "/tmp/'; rm -rf /".into();
    store.registry.agents.insert(reused.key(), reused.clone());
    store.save().unwrap();
    let mut old = agent(1);
    old.pid = 900;
    store.append("exited", &old, 1).unwrap();
    drop(store);
    let entries = recovery_at(&root.0, time).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(
        entries[0].resume_command.as_deref(),
        Some("claude --resume 'session-safe'")
    );
    assert_eq!(entries[0].name, reused.name);
    assert_eq!(entries[0].cwd, reused.cwd);
    assert!(
        entries
            .iter()
            .any(|e| e.session_id.as_deref() == Some("session-other"))
    );
    assert!(
        entries
            .iter()
            .all(|e| !e.resume_command.as_ref().unwrap().contains("touch"))
    );
    assert!(
        Store::open(&root.0.join("native"))
            .unwrap()
            .records()
            .unwrap()
            .iter()
            .all(|r| r.at >= time - RETENTION)
    );
}
#[test]
fn unknown_and_invalid_identifiers_never_offer_resume_and_busy_registry_does_not_wait() {
    let root = Temp::new();
    let time = RETENTION + 1;
    let mut store = Store::open(&root.0.join("native")).unwrap();
    let mut unknown = agent(time);
    unknown.session_id = None;
    store.registry.agents.insert(unknown.key(), unknown);
    let mut invalid = agent(time);
    invalid.pid = 102;
    invalid.session_id = Some("--last; $(bad)".into());
    store.registry.agents.insert(invalid.key(), invalid);
    store.save().unwrap();
    assert!(recovery_at(&root.0, time).is_err());
    drop(store);
    assert!(
        recovery_at(&root.0, time)
            .unwrap()
            .iter()
            .all(|e| e.resume_command.is_none())
    );
}
#[test]
fn hook_validates_current_process_birth_kind_identity_and_payload_privacy() {
    let root = Temp::new();
    let proc = process::read(Path::new("/proc"), std::process::id()).unwrap();
    let mut current = agent(kelpmux::now());
    current.pid = proc.pid;
    current.process_start = proc.start;
    current.session_id = None;
    Store::open(&root.0.join("native"))
        .unwrap()
        .reconcile(vec![current.clone()], kelpmux::now())
        .unwrap();
    let payload = serde_json::json!({"session_id":"session-safe","hook_event_name":"PermissionRequest","prompt":"PRIVATE_PROMPT","tool_input":{"password":"PRIVATE_PROMPT"}});
    hooks::receive(&root.0, "1", Kind::Codex, &payload).unwrap();
    assert!(hooks::receive(&root.0, "%%1", Kind::Codex, &payload).is_err());
    assert!(hooks::receive(&root.0, "1", Kind::Claude, &payload).is_err());
    assert!(
        hooks::receive(
            &root.0,
            "1",
            Kind::Codex,
            &serde_json::json!({"session_id":"different","hook_event_name":"Stop"})
        )
        .is_err()
    );
    {
        let mut store = Store::open(&root.0.join("native")).unwrap();
        let found = store.registry.agents.values_mut().next().unwrap();
        assert_eq!(found.status, Status::InputNeeded);
        found.process_start += 1;
        store.save().unwrap();
    }
    assert!(hooks::receive(&root.0, "1", Kind::Codex, &payload).is_err());
    for entry in fs::read_dir(root.0.join("native")).unwrap() {
        assert!(
            !fs::read_to_string(entry.unwrap().path())
                .unwrap()
                .contains("PRIVATE_PROMPT")
        );
    }
}
fn process_fixture(root: &Path, pid: u32, parent: u32, command: &str, args: &[&str]) {
    let folder = root.join(pid.to_string());
    fs::create_dir_all(folder.join("fd")).unwrap();
    let mut fields = vec!["0".to_owned(); 22];
    fields[0] = "S".into();
    fields[1] = parent.to_string();
    fields[2] = pid.to_string();
    fields[4] = "34816".into();
    fields[5] = pid.to_string();
    fields[19] = "10".into();
    fs::write(
        folder.join("stat"),
        format!("{pid} ({command}) {}", fields.join(" ")),
    )
    .unwrap();
    fs::write(folder.join("cmdline"), args.join("\0")).unwrap();
    fs::write(folder.join("environ"), "").unwrap();
    std::os::unix::fs::symlink("/project", folder.join("cwd")).unwrap();
}
#[test]
fn multiple_open_codex_rollouts_are_ambiguous_without_positive_identity() {
    let root = Temp::new();
    let procroot = root.0.join("proc");
    process_fixture(&procroot, 100, 1, "bash", &["bash"]);
    process_fixture(&procroot, 101, 100, "codex", &["codex"]);
    process_fixture(&procroot, 102, 1, "codex", &["codex", "app-server"]);
    for (fd, id) in [(3, "session-one"), (4, "session-two")] {
        let path = root.0.join(format!("rollout-{id}.jsonl"));
        fs::write(
            &path,
            format!(
                "{}\n",
                serde_json::json!({"type":"session_meta","payload":{"id":id}})
            ),
        )
        .unwrap();
        std::os::unix::fs::symlink(&path, procroot.join(format!("101/fd/{fd}"))).unwrap();
    }
    let discovery = Discovery {
        proc_root: procroot.clone(),
        codex_home: root.0.join("codex"),
        claude_home: root.0.join("claude"),
    };
    let pane = Pane {
        id: "%1".into(),
        window: "@0".into(),
        pid: 100,
        index: 1,
        cwd: "/project".into(),
        dead: false,
        floating: false,
    };
    let first = discovery.scan(std::slice::from_ref(&pane), 20).unwrap();
    assert_eq!(first.len(), 1);
    assert!(first[0].session_id.is_none());
    assert_eq!(first[0].status, Status::Unknown);
    fs::write(
        procroot.join("101/environ"),
        "CODEX_THREAD_ID=session-two\0",
    )
    .unwrap();
    let second = discovery.scan(&[pane], 21).unwrap();
    assert_eq!(second[0].session_id.as_deref(), Some("session-two"));
    assert_eq!(second[0].source, "process-identity");
}

#[test]
fn recovery_prefers_completed_journal_transition_over_stale_registry() {
    let root = Temp::new();
    let time = RETENTION + 20;
    let mut store = Store::open(&root.0.join("native")).unwrap();
    let old = agent(time - 10);
    store.registry.agents.insert(old.key(), old.clone());
    store.save().unwrap();
    let mut ended = old;
    ended.status = Status::Exited;
    ended.ended_at = Some(time);
    store.append("exited", &ended, time).unwrap();
    drop(store);
    let recovered = recovery_at(&root.0, time).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, Status::Exited);
    assert_eq!(recovered[0].ended_at, Some(time));
}

#[test]
fn native_hook_command_inherits_custom_state_and_explicit_flag_wins() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let root = Temp::new();
    let overridden = Temp::new();
    let current = process::read(Path::new("/proc"), std::process::id()).unwrap();
    for state in [&root.0, &overridden.0] {
        let mut fixture = agent(kelpmux::now());
        fixture.pid = current.pid;
        fixture.process_start = current.start;
        fixture.session_id = None;
        Store::open(&state.join("native"))
            .unwrap()
            .reconcile(vec![fixture], kelpmux::now())
            .unwrap();
    }
    let invoke = |explicit: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kelpmux"));
        command
            .args(["native-event", "codex"])
            .env("KELPMUX_PANE", "1")
            .env("KELPMUX_STATE_DIR", &root.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if explicit {
            command.arg("--state-dir").arg(&overridden.0);
        }
        let mut child = command.spawn().unwrap();
        child.stdin.take().unwrap().write_all(br#"{"session_id":"session-safe","hook_event_name":"Stop","prompt":"PRIVATE_PROMPT"}"#).unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stdout.is_empty());
    };
    invoke(false);
    assert_eq!(
        Store::open(&root.0.join("native"))
            .unwrap()
            .registry
            .agents
            .values()
            .next()
            .unwrap()
            .status,
        Status::TurnFinished
    );
    assert_eq!(
        Store::open(&overridden.0.join("native"))
            .unwrap()
            .registry
            .agents
            .values()
            .next()
            .unwrap()
            .status,
        Status::Working
    );
    invoke(true);
    assert_eq!(
        Store::open(&overridden.0.join("native"))
            .unwrap()
            .registry
            .agents
            .values()
            .next()
            .unwrap()
            .status,
        Status::TurnFinished
    );
}

#[test]
fn shell_names_hide_prompt_identity_but_preserve_application_titles_and_aliases() {
    use kelpmux::native::tracking::shell_tab_name;
    assert_eq!(shell_tab_name(None, None, true), "shell");
    assert_eq!(
        shell_tab_name(Some("joan@jbox: ~/scratch"), None, true),
        "shell"
    );
    assert_eq!(shell_tab_name(Some("htop"), None, false), "htop");
    assert_eq!(shell_tab_name(Some(""), None, false), "shell");
    for idle in [true, false] {
        assert_eq!(shell_tab_name(Some("shell"), None, idle), "shell");
        assert_eq!(
            shell_tab_name(Some("joan@jbox"), Some("Tools"), idle),
            "Tools"
        );
    }
}

#[test]
fn tab_names_never_use_directory_as_conversation_identity_and_preserve_osc_fallback() {
    use kelpmux::native::tracking::tab_name;
    let mut agent = agent(100);
    agent.cwd = "/home/joan/scratch".into();
    agent.name = "scratch".into();
    agent.session_id = None;
    assert_eq!(
        tab_name(&agent, None, None),
        ("codex · session unknown".into(), false)
    );
    assert_eq!(
        tab_name(&agent, Some("scratch"), None).0,
        "codex · session unknown"
    );
    assert_eq!(
        tab_name(&agent, Some("Fix authentication"), None),
        ("Fix authentication".into(), false)
    );
    assert_eq!(
        tab_name(&agent, Some("⠋ Fix authentication | scratch"), None).0,
        "Fix authentication"
    );
    assert_eq!(
        tab_name(&agent, Some("Fix authentication | other"), None).0,
        "Fix authentication | other"
    );
    assert_eq!(
        tab_name(&agent, None, Some("Name | scratch")).0,
        "Name | scratch"
    );
    for title in [
        "⠋ Fix authentication",
        "⠙ Fix authentication",
        "⠇ Fix authentication",
    ] {
        assert_eq!(
            tab_name(&agent, Some(title), None),
            ("Fix authentication".into(), false)
        );
    }
    assert_eq!(
        tab_name(&agent, Some("⠋ scratch"), None).0,
        "codex · session unknown"
    );
    assert_eq!(tab_name(&agent, Some("⠋real title"), None).0, "⠋real title");
    assert_eq!(tab_name(&agent, None, Some("⠋ My title")).0, "⠋ My title");
    agent.name = "Review conversation".into();
    agent.session_id = Some("verified-session".into());
    assert_eq!(
        tab_name(&agent, Some("terminal title"), None),
        ("Review conversation".into(), true)
    );
    assert_eq!(
        tab_name(&agent, Some("terminal title"), Some("Backend")),
        ("Backend".into(), true)
    );
}
