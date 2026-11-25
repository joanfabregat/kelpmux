use kelpserver::{
    model::{Kind, Status},
    native::{
        history::{Entry, History, LAST_SEEN_STEP, SESSION_LIMIT, Session},
        protocol::PaneInfo,
    },
};
use std::collections::BTreeMap;

#[test]
fn excludes_empty_shells_and_preserves_last_application_for_closed_tabs() {
    let mut history = History::default();
    let mut pane = PaneInfo {
        id: 1,
        cwd: "/project".into(),
        title: "shell".into(),
        ..Default::default()
    };
    history.observe(
        "run-a",
        "dev",
        std::slice::from_ref(&pane),
        &BTreeMap::new(),
    );
    assert!(history.entries.is_empty());
    history.observe(
        "run-a",
        "dev",
        std::slice::from_ref(&pane),
        &BTreeMap::from([(1, vec!["htop".into()])]),
    );
    assert_eq!(history.entries[0].name, "htop");
    history.observe(
        "run-a",
        "dev",
        std::slice::from_ref(&pane),
        &BTreeMap::new(),
    );
    assert_eq!(history.entries[0].argv, ["htop"]);
    pane.kind = Some(Kind::Codex);
    pane.title = "Conversation A".into();
    pane.session_id = Some("session-a".into());
    pane.status = Status::Working;
    history.observe(
        "run-a",
        "dev",
        std::slice::from_ref(&pane),
        &BTreeMap::new(),
    );
    history.close("run-a", 1, kelpserver::now());
    assert_eq!(history.entries.len(), 1);
    assert_eq!(
        history.entries[0].command().unwrap(),
        "codex resume 'session-a'"
    );
    assert_eq!(history.entries[0].reason, "Closed");
    history.consume("run-a:1");
    assert!(history.entries.is_empty());
    assert!(history.restored.contains(&"run-a:1".into()));
}

#[test]
fn previous_runs_preserve_conversations_and_reused_pane_ids_stay_separate() {
    let mut history = History::default();
    let pane = PaneInfo {
        id: 1,
        kind: Some(Kind::Claude),
        session_id: Some("claude-a".into()),
        cwd: "/project".into(),
        title: "Review".into(),
        ..Default::default()
    };
    history.observe("a", "dev", std::slice::from_ref(&pane), &BTreeMap::new());
    history.begin_run(kelpserver::now());
    assert_eq!(history.entries[0].reason, "Previous run");
    assert!(history.entries[0].closed_at.is_some());
    history.observe("b", "dev", &[pane], &BTreeMap::new());
    assert_eq!(history.entries.len(), 2);
    assert_eq!(
        history.entries[1].command().unwrap(),
        "claude --resume 'claude-a'"
    );
}

#[test]
fn unknown_or_invalid_agent_id_uses_picker_and_application_arguments_are_quoted() {
    let mut entry = Entry {
        key: "a:1".into(),
        workspace: "dev".into(),
        pane: 1,
        cwd: "/project".into(),
        name: "Review".into(),
        kind: Some(Kind::Codex),
        session_id: Some("bad\nID".into()),
        argv: vec!["printf".into(), "$(touch /tmp/should-not-exist)".into()],
        closed_at: Some(kelpserver::now()),
        reason: "Closed".into(),
        sessions: vec![],
    };
    assert_eq!(entry.command().unwrap(), "codex resume");
    entry.kind = Some(Kind::Claude);
    assert_eq!(entry.command().unwrap(), "claude --resume");
    entry.kind = None;
    assert!(entry.command().is_none());
    assert_eq!(
        entry.previous_command().unwrap(),
        "'printf' '$(touch /tmp/should-not-exist)'"
    );
}

fn agent(id: &str, cwd: &str) -> PaneInfo {
    PaneInfo {
        id: 1,
        kind: Some(Kind::Claude),
        session_id: Some(id.into()),
        cwd: cwd.into(),
        title: "Agent".into(),
        ..Default::default()
    }
}

fn observe(history: &mut History, pane: &PaneInfo, at: u64) {
    history.observe_at(
        "run",
        "dev",
        std::slice::from_ref(pane),
        &BTreeMap::new(),
        at,
    );
}

#[test]
fn successive_conversations_in_one_pane_are_all_kept_in_order() {
    let mut history = History::default();
    let start = kelpserver::now();
    observe(&mut history, &agent("first", "/project/a"), start);
    observe(
        &mut history,
        &agent("first", "/project/b"),
        start + LAST_SEEN_STEP,
    );
    // `/clear` or a second `claude` run: a new conversation in the same pane.
    observe(&mut history, &agent("second", "/project/b"), start + 100);
    history.close("run", 1, start + 200);
    assert_eq!(history.entries.len(), 1);
    let entry = &history.entries[0];
    assert_eq!(entry.session_id.as_deref(), Some("second"));
    assert_eq!(entry.cwd, "/project/b");
    assert_eq!(
        entry.sessions,
        [
            Session {
                kind: Kind::Claude,
                id: "first".into(),
                cwd: "/project/a".into(),
                first_seen: start,
                last_seen: start + LAST_SEEN_STEP,
            },
            Session {
                kind: Kind::Claude,
                id: "second".into(),
                cwd: "/project/b".into(),
                first_seen: start + 100,
                last_seen: start + 100,
            },
        ]
    );
}

#[test]
fn reobserving_a_conversation_only_bumps_last_seen() {
    let mut history = History::default();
    let start = kelpserver::now();
    let pane = agent("same", "/project");
    observe(&mut history, &pane, start);
    let first = history.clone();
    // Below the step nothing changes, so the file is not rewritten.
    observe(&mut history, &pane, start + LAST_SEEN_STEP - 1);
    assert_eq!(history, first);
    observe(&mut history, &agent("same", "/elsewhere"), start + 300);
    let sessions = &history.entries[0].sessions;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].first_seen, start);
    assert_eq!(sessions[0].last_seen, start + 300);
    assert_eq!(sessions[0].cwd, "/project");
    // A pane without a conversation, or with an unusable ID, adds nothing.
    let mut plain = agent("same", "/project");
    plain.session_id = None;
    observe(&mut history, &plain, start + 400);
    observe(&mut history, &agent("bad id;", "/project"), start + 500);
    assert_eq!(history.entries[0].sessions.len(), 1);
    assert_eq!(history.entries[0].session_id.as_deref(), Some("bad id;"));
}

#[test]
fn sessions_are_capped_dropping_the_oldest() {
    let mut history = History::default();
    let start = kelpserver::now();
    for i in 0..SESSION_LIMIT + 5 {
        observe(
            &mut history,
            &agent(&format!("s{i}"), "/project"),
            start + i as u64,
        );
    }
    let sessions = &history.entries[0].sessions;
    assert_eq!(sessions.len(), SESSION_LIMIT);
    assert_eq!(sessions[0].id, "s5");
    assert_eq!(
        sessions.last().unwrap().id,
        format!("s{}", SESSION_LIMIT + 4)
    );
}

#[test]
fn old_history_files_load_and_sessions_round_trip() {
    let old = serde_json::json!({
        "entries": [{
            "key": "a:1", "workspace": "dev", "pane": 1, "cwd": "/project",
            "name": "Review", "kind": "codex", "session_id": "s-1", "argv": [],
            "closed_at": 1, "reason": "Closed"
        }]
    });
    let history: History = serde_json::from_value(old).unwrap();
    assert!(history.entries[0].sessions.is_empty());
    assert_eq!(history.entries[0].session_id.as_deref(), Some("s-1"));

    let mut history = History::default();
    let start = kelpserver::now();
    observe(&mut history, &agent("one", "/a"), start);
    observe(&mut history, &agent("two", "/b"), start + 1);
    let json = serde_json::to_value(&history).unwrap();
    assert_eq!(
        json["entries"][0]["sessions"][1],
        serde_json::json!({"kind": "claude", "id": "two", "cwd": "/b",
            "first_seen": start + 1, "last_seen": start + 1})
    );
    let back: History = serde_json::from_value(json).unwrap();
    assert_eq!(back, history);

    // Closed-session records expose the list beside the flattened entry.
    let record = kelpserver::native::stream::ClosedSession::from(history.entries[0].clone());
    let json = serde_json::to_value(&record).unwrap();
    assert_eq!(json["sessions"][0]["id"], "one");
    assert_eq!(json["session_id"], "two");
    assert_eq!(json["resume"], "claude --resume 'two'");
}
