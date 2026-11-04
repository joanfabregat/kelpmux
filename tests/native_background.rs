use kelpmux::{
    journal::Store,
    model::{Agent, Kind, Status},
    native::background,
    process,
};
use serde_json::json;
use std::{fs, path::Path};

#[test]
fn background_events_are_identity_bound_private_and_compatible_with_old_registries() {
    let root = std::env::temp_dir().join(format!("kelpmux-background-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let current = process::read(Path::new("/proc"), std::process::id()).unwrap();
    let mut agent = Agent {
        pane: "%1".into(),
        window: "@0".into(),
        pid: current.pid,
        process_start: current.start,
        kind: Kind::Claude,
        session_id: Some("session-background".into()),
        name: "fixture".into(),
        cwd: "/".into(),
        status: Status::Idle,
        observation: Default::default(),
        source: "claude-live-record".into(),
        first_seen: kelpmux::now(),
        last_seen: kelpmux::now(),
        ended_at: None,
    };
    Store::open(&root.join("native"))
        .unwrap()
        .reconcile(vec![agent.clone()], kelpmux::now())
        .unwrap();
    let proc = root.join(format!("proc/{}", current.pid));
    fs::create_dir_all(&proc).unwrap();
    fs::write(
        proc.join("environ"),
        format!("KELPMUX_STATE_DIR={}\0", root.display()),
    )
    .unwrap();
    let payload = json!({"session_id":"session-background", "hook_event_name":"Stop", "background_tasks":[{"id":"task1","status":"running","description":"PRIVATE_TASK","command":"PRIVATE_COMMAND"}], "session_crons":[], "last_assistant_message":"PRIVATE_REPLY"});
    background::receive(&root, "1", &payload).unwrap();
    let file = root.join("native/background/1.json");
    assert!(!fs::read_to_string(&file).unwrap().contains("PRIVATE"));
    assert_eq!(
        Store::open(&root.join("native"))
            .unwrap()
            .registry
            .agents
            .values()
            .next()
            .unwrap()
            .status,
        Status::Idle
    );
    background::enrich(&root.join("proc"), &mut agent);
    assert_eq!(agent.status, Status::WaitingBackground);
    agent.status = Status::InputNeeded;
    background::enrich(&root.join("proc"), &mut agent);
    assert_eq!(agent.status, Status::InputNeeded);
    agent.status = Status::Idle;
    agent.session_id = Some("another-session".into());
    background::enrich(&root.join("proc"), &mut agent);
    assert_eq!(agent.status, Status::Idle);
    let mut mismatched = payload.clone();
    mismatched["session_id"] = json!("another-session");
    assert!(background::receive(&root, "1", &mismatched).is_err());
    background::receive(
        &root,
        "1",
        &json!({"session_id":"session-background", "hook_event_name":"PreToolUse"}),
    )
    .unwrap();
    agent.session_id = Some("session-background".into());
    background::enrich(&root.join("proc"), &mut agent);
    assert_eq!(agent.status, Status::Idle);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn only_pending_tasks_and_scheduled_wakeups_count() {
    assert_eq!(
        background::counts(
            &json!({"hook_event_name":"Stop", "background_tasks":[{"status":"completed"},{"status":"failed"},{"status":"pending"}], "session_crons":[{"id":"wake","schedule":"*/5 * * * *","prompt":"PRIVATE"}]})
        ),
        Some((1, 1))
    );
    assert_eq!(background::counts(&json!({"hook_event_name":"Stop"})), None);
    assert_eq!(
        background::counts(&json!({"hook_event_name":"SessionEnd"})),
        Some((0, 0))
    );
}
