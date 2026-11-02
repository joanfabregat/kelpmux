use kelpmux::{
    model::{Kind, Status},
    native::{
        history::{Entry, History},
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
    history.close("run-a", 1, kelpmux::now());
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
    history.begin_run(kelpmux::now());
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
        closed_at: Some(kelpmux::now()),
        reason: "Closed".into(),
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
