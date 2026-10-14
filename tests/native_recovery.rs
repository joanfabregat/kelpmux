use kelpmux::{
    journal::{RETENTION, Store},
    model::{Agent, Kind, Status},
    native::recovery::{collect, collect_at},
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kelpmux-recovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        private(&root);
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn private(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn agent(pid: u32, cwd: &str, time: u64) -> Agent {
    Agent {
        pane: "%1".into(),
        window: "@0".into(),
        pid,
        process_start: 1,
        kind: Kind::Codex,
        session_id: Some(format!("session-{pid}")),
        name: "$(touch should-not-run)".into(),
        cwd: cwd.into(),
        status: Status::Working,
        source: "fixture".into(),
        first_seen: time,
        last_seen: time,
        ended_at: None,
    }
}
fn write(state: &Path, agents: Vec<Agent>, time: u64) {
    private(state);
    Store::open(&state.join("native"))
        .unwrap()
        .reconcile(agents, time)
        .unwrap();
}
#[test]
fn recovery_groups_two_workspaces_and_projects_without_interpolating_names() {
    let fixture = Fixture::new();
    let root = fixture.0.join("workspaces");
    private(&root);
    let a = root.join("alpha");
    let b = root.join("beta");
    let time = kelpmux::now();
    write(
        &a,
        vec![agent(1, "/project-b", time), agent(2, "/project-a", time)],
        time,
    );
    write(&b, vec![agent(3, "/project-a", time)], time);
    let found = collect(&a).unwrap();
    assert_eq!(found.len(), 3);
    assert_eq!(
        found
            .iter()
            .map(|item| (&*item.workspace, &*item.entry.cwd))
            .collect::<Vec<_>>(),
        vec![
            ("alpha", "/project-a"),
            ("alpha", "/project-b"),
            ("beta", "/project-a")
        ]
    );
    assert_eq!(
        found[0].entry.resume_command.as_deref(),
        Some("codex resume 'session-2'")
    );
    assert!(
        !found[0]
            .entry
            .resume_command
            .as_ref()
            .unwrap()
            .contains("touch")
    );
}
#[test]
fn custom_state_does_not_scan_siblings_and_unknown_id_has_no_resume_command() {
    let fixture = Fixture::new();
    let a = fixture.0.join("alpha");
    let b = fixture.0.join("beta");
    let time = kelpmux::now();
    let mut unknown = agent(1, "/project", time);
    unknown.session_id = None;
    write(&a, vec![unknown], time);
    write(&b, vec![agent(2, "/project", time)], time);
    let found = collect(&a).unwrap();
    assert_eq!(found.len(), 1);
    assert!(found[0].entry.resume_command.is_none());
}
#[test]
fn held_corrupt_public_symlink_and_oversized_workspace_metadata_are_skipped() {
    let fixture = Fixture::new();
    let root = fixture.0.join("workspaces");
    private(&root);
    let time = kelpmux::now();
    let current = root.join("current");
    write(&current, vec![agent(1, "/project", time)], time);
    let locked = root.join("locked");
    write(&locked, vec![agent(2, "/project", time)], time);
    let _lock = Store::open(&locked.join("native")).unwrap();
    let corrupt = root.join("corrupt");
    write(&corrupt, vec![agent(3, "/project", time)], time);
    fs::write(corrupt.join("native/registry.json"), "bad json").unwrap();
    let public = root.join("public");
    write(&public, vec![agent(4, "/project", time)], time);
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(&current, root.join("link")).unwrap();
    let huge = root.join("huge");
    write(&huge, vec![agent(5, "/project", time)], time);
    fs::write(
        huge.join("native/journal.jsonl"),
        vec![b' '; 8 * 1024 * 1024 + 1],
    )
    .unwrap();
    let found = collect(&current).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].workspace, "current");
}
#[test]
fn old_ended_sessions_are_pruned_but_current_registry_agents_remain() {
    let fixture = Fixture::new();
    let state = fixture.0.join("current");
    let time = RETENTION + 100;
    private(&state);
    let mut store = Store::open(&state.join("native")).unwrap();
    let live = agent(1, "/old-active", 1);
    let mut ended = agent(2, "/old-ended", 1);
    ended.ended_at = Some(1);
    ended.status = Status::Exited;
    store.registry.agents.insert(live.key(), live);
    store.registry.agents.insert(ended.key(), ended.clone());
    store.append("exited", &ended, 1).unwrap();
    store.save().unwrap();
    drop(store);
    let found = collect_at(&state, None, time).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].entry.pid, 1);
}
#[test]
fn maximum_workspace_scan_is_bounded_and_invalid_names_are_ignored() {
    let fixture = Fixture::new();
    let root = fixture.0.join("workspaces");
    private(&root);
    let time = kelpmux::now();
    for index in 0..70 {
        write(
            &root.join(format!("workspace-{index}")),
            vec![agent(index + 1, "/project", time)],
            time,
        );
    }
    let state = root.join("workspace-0");
    let found = collect(&state).unwrap();
    assert!(found.len() <= 64);
    assert!(found.iter().any(|item| item.workspace == "workspace-0"));
    let invalid = root.join("bad;name");
    write(&invalid, vec![agent(900, "/project", time)], time);
    assert!(
        collect(&state)
            .unwrap()
            .iter()
            .all(|item| item.workspace != "bad;name")
    );
}
