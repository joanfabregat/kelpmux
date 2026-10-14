use kelpmux::{
    journal::Store,
    model::{Agent, Kind, Status},
    native::hooks::receive,
    process,
};
use std::{fs, path::Path};

#[test]
fn native_hooks_require_live_identity_and_store_only_session_metadata() {
    let root = std::env::temp_dir().join(format!("kelpmux-native-hook-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let journal = root.join("native");
    let current = process::read(Path::new("/proc"), std::process::id()).unwrap();
    let agent = Agent {
        pane: "%1".into(),
        window: "@0".into(),
        pid: current.pid,
        process_start: current.start,
        kind: Kind::Codex,
        session_id: None,
        name: "fixture".into(),
        cwd: "/".into(),
        status: Status::Unknown,
        source: "fixture".into(),
        observation: Default::default(),
        first_seen: kelpmux::now(),
        last_seen: kelpmux::now(),
        ended_at: None,
    };
    Store::open(&journal)
        .unwrap()
        .reconcile(vec![agent.clone()], kelpmux::now())
        .unwrap();
    let input = serde_json::json!({"session_id":"thread-123", "hook_event_name":"Stop", "prompt":"MUST_NOT_STORE", "tool_input":{"password":"MUST_NOT_STORE"}});
    receive(&root, "%1", Kind::Codex, &input).unwrap();
    {
        let store = Store::open(&journal).unwrap();
        let agent = store.registry.agents.values().next().unwrap();
        assert_eq!(agent.session_id.as_deref(), Some("thread-123"));
        assert_eq!(agent.status, Status::TurnFinished);
    }
    assert!(
        receive(
            &root,
            "1",
            Kind::Codex,
            &serde_json::json!({"session_id":"different", "hook_event_name":"Stop"})
        )
        .is_err()
    );
    assert!(receive(&root, "1", Kind::Claude, &input).is_err());
    assert!(receive(&root, "2", Kind::Codex, &input).is_err());
    {
        let mut store = Store::open(&journal).unwrap();
        store
            .registry
            .agents
            .values_mut()
            .next()
            .unwrap()
            .process_start += 1;
        store.save().unwrap();
    }
    assert!(receive(&root, "1", Kind::Codex, &input).is_err());
    for path in fs::read_dir(&journal).unwrap() {
        let path = path.unwrap().path();
        if path.is_file() {
            assert!(!fs::read_to_string(path).unwrap().contains("MUST_NOT_STORE"));
        }
    }
    fs::remove_dir_all(root).unwrap();
}
