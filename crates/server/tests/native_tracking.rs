use kelpserver::{
    discovery::Discovery,
    discovery::Pane,
    journal::{RETENTION, Store},
    model::{Agent, Kind, Status},
    native::{
        hooks,
        protocol::{PaneInfo, ShellCommand},
        tracking::{recovery_at, track_command},
    },
    process,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
#[test]
fn terminal_title_matches_deleted_cwd_without_truncating_conversation_names() {
    use kelpserver::native::tracking::{agent_terminal_title, tab_name};
    for (title, cwd, expected) in [
        (
            "VPN new device | vpn-config",
            "/repos/vpn-config (deleted)",
            "VPN new device",
        ),
        (
            "⠋ VPN new device | vpn-config",
            "/repos/vpn-config (deleted)",
            "VPN new device",
        ),
        (
            "VPN | new device | vpn-config",
            "/repos/vpn-config (deleted)",
            "VPN | new device",
        ),
        (
            "VPN | other-repo",
            "/repos/vpn-config (deleted)",
            "VPN | other-repo",
        ),
        (
            "VPN | vpn-config (deleted)",
            "/repos/vpn-config (deleted)",
            "VPN",
        ),
        (
            " | vpn-config",
            "/repos/vpn-config (deleted)",
            "| vpn-config",
        ),
    ] {
        assert_eq!(agent_terminal_title(title, cwd), expected);
    }
    let mut entry = agent(0);
    entry.cwd = "/repos/vpn-config (deleted)".into();
    entry.name = "vpn-config (deleted)".into();
    assert_eq!(
        tab_name(&entry, Some("VPN new device | vpn-config")).0,
        "VPN new device"
    );
    entry.name = "Explicit | vpn-config".into();
    assert_eq!(tab_name(&entry, None).0, "Explicit | vpn-config");
}
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
    let mut current = agent(kelpserver::now());
    current.pid = proc.pid;
    current.process_start = proc.start;
    current.session_id = None;
    Store::open(&root.0.join("native"))
        .unwrap()
        .reconcile(vec![current.clone()], kelpserver::now())
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
        let mut fixture = agent(kelpserver::now());
        fixture.pid = current.pid;
        fixture.process_start = current.start;
        fixture.session_id = None;
        Store::open(&state.join("native"))
            .unwrap()
            .reconcile(vec![fixture], kelpserver::now())
            .unwrap();
    }
    let invoke = |explicit: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kelpserver"));
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
fn shell_names_hide_prompt_identity_but_preserve_application_titles() {
    use kelpserver::native::tracking::shell_tab_name;
    assert_eq!(shell_tab_name(None, true), "shell");
    assert_eq!(shell_tab_name(Some("joan@jbox: ~/scratch"), true), "shell");
    assert_eq!(shell_tab_name(Some("htop"), false), "htop");
    assert_eq!(shell_tab_name(Some(""), false), "shell");
    for idle in [true, false] {
        assert_eq!(shell_tab_name(Some("shell"), idle), "shell");
    }
}

#[test]
fn tab_names_never_use_directory_as_conversation_identity_and_preserve_osc_fallback() {
    use kelpserver::native::tracking::tab_name;
    let mut agent = agent(100);
    agent.cwd = "/home/joan/scratch".into();
    agent.name = "scratch".into();
    agent.session_id = None;
    assert_eq!(
        tab_name(&agent, None),
        ("codex · session unknown".into(), false)
    );
    assert_eq!(
        tab_name(&agent, Some("scratch")).0,
        "codex · session unknown"
    );
    assert_eq!(
        tab_name(&agent, Some("Fix authentication")),
        ("Fix authentication".into(), false)
    );
    assert_eq!(
        tab_name(&agent, Some("⠋ Fix authentication | scratch")).0,
        "Fix authentication"
    );
    assert_eq!(
        tab_name(&agent, Some("Fix authentication | other")).0,
        "Fix authentication | other"
    );
    for title in [
        "⠋ Fix authentication",
        "⠙ Fix authentication",
        "⠇ Fix authentication",
    ] {
        assert_eq!(
            tab_name(&agent, Some(title)),
            ("Fix authentication".into(), false)
        );
    }
    assert_eq!(
        tab_name(&agent, Some("⠋ scratch")).0,
        "codex · session unknown"
    );
    assert_eq!(tab_name(&agent, Some("⠋real title")).0, "⠋real title");
    agent.name = "Review conversation".into();
    agent.session_id = Some("verified-session".into());
    assert_eq!(
        tab_name(&agent, Some("terminal title")),
        ("Review conversation".into(), true)
    );
}
#[test]
fn shell_commands_track_start_return_and_alternate_screen() {
    let none = || None;
    // Idle without a running command records nothing.
    assert_eq!(track_command(None, Status::Idle, false, none, 5, 10), None);
    // A new command starts at its first process, never in the future or
    // before the previous command returned.
    let running = track_command(None, Status::Working, false, || Some(98), 90, 100).unwrap();
    assert_eq!(
        running,
        ShellCommand {
            started_at: 98,
            ended_at: None,
            alternate_screen: false
        }
    );
    assert_eq!(
        track_command(None, Status::Working, false, || Some(200), 0, 100)
            .unwrap()
            .started_at,
        100
    );
    assert_eq!(
        track_command(None, Status::Working, false, none, 0, 100)
            .unwrap()
            .started_at,
        100
    );
    let earlier = ShellCommand {
        started_at: 50,
        ended_at: Some(60),
        alternate_screen: true,
    };
    assert_eq!(
        track_command(
            Some(earlier.clone()),
            Status::Working,
            false,
            || Some(55),
            0,
            100
        ),
        Some(ShellCommand {
            started_at: 60,
            ended_at: None,
            alternate_screen: false
        })
    );
    // The alternate screen is sticky for the command's lifetime.
    let full = track_command(Some(running.clone()), Status::Working, true, none, 0, 102).unwrap();
    let full = track_command(Some(full), Status::Working, false, none, 0, 104).unwrap();
    assert!(full.alternate_screen && full.started_at == 98);
    // Unknown status keeps the record; the return uses the prompt's output.
    assert_eq!(
        track_command(Some(full.clone()), Status::Unknown, false, none, 0, 105),
        Some(full.clone())
    );
    let returned = track_command(Some(full), Status::Idle, false, none, 109, 110).unwrap();
    assert_eq!(returned.ended_at, Some(109));
    assert!(returned.alternate_screen);
    // Without output since the start, the return is the observation time.
    let silent = track_command(Some(running.clone()), Status::Idle, false, none, 90, 111).unwrap();
    assert_eq!(silent.ended_at, Some(111));
    assert!(!silent.alternate_screen);
    // A returned command stays as it was until the next one starts.
    assert_eq!(
        track_command(Some(silent.clone()), Status::Idle, true, none, 130, 140),
        Some(silent)
    );
}
#[test]
fn process_start_times_keep_sub_second_uptime_precision() {
    // Observed at 2000.0 s with 1000.1 s of uptime, the host booted at
    // 999.9 s, which btime truncates to 999. A process started 900.5 s after
    // boot began at 1900.4 s; btime plus whole seconds reported 1899, before
    // the process existed.
    let at = |uptime: &str, start| process::started_at_from(uptime, start, 100, 2_000_000);
    assert_eq!(at("1000.10 4.00\n", 90_050), Some(1900));
    assert_eq!(at("1000.1", 90_050), Some(1900));
    assert_eq!(at("1000.105", 90_050), Some(1900));
    assert_eq!(at("1000.10", 89_950), Some(1899));
    // A start recorded after the truncated uptime is now, never the future.
    assert_eq!(at("1000.10", 100_020), Some(2000));
    for malformed in ["", "1000", "1000.", "1000.x8", "x.10", "-1.00"] {
        assert_eq!(at(malformed, 90_050), None, "{malformed:?}");
    }
    assert_eq!(process::started_at_from("3000.00", 0, 100, 2_000_000), None);
}
#[test]
fn pane_info_without_a_command_stays_compatible() {
    let pane = PaneInfo {
        id: 1,
        ..Default::default()
    };
    let json = serde_json::to_value(&pane).unwrap();
    assert!(json.get("command").is_none());
    let decoded: PaneInfo = serde_json::from_value(json).unwrap();
    assert_eq!(decoded, pane);
    let with = PaneInfo {
        command: Some(ShellCommand {
            started_at: 1,
            ended_at: None,
            alternate_screen: true,
        }),
        ..pane
    };
    let json = serde_json::to_value(&with).unwrap();
    assert_eq!(
        json["command"],
        serde_json::json!({"started_at":1,"ended_at":null,"alternate_screen":true})
    );
    assert_eq!(serde_json::from_value::<PaneInfo>(json).unwrap(), with);
    let minimal: ShellCommand = serde_json::from_str(r#"{"started_at":4}"#).unwrap();
    assert_eq!(minimal.ended_at, None);
    assert!(!minimal.alternate_screen);
}
