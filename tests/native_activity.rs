use kelpmux::{
    journal::RETENTION,
    model::{Kind, Status},
    native::{
        activity::{Activity, next},
        protocol::PaneInfo,
        ui::AgentFilter,
    },
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kelpmux-activity-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn pane(id: u64, status: Status) -> PaneInfo {
    PaneInfo {
        id,
        pid: id as u32 + 100,
        kind: Some(Kind::Codex),
        title: format!("Agent {id}"),
        session_id: Some(format!("session-{id}")),
        status,
        ..Default::default()
    }
}
#[test]
fn initial_baseline_dedup_explicit_statuses_and_ack_persist_across_restart() {
    let fixture = Fixture::new();
    let time = kelpmux::now();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    let mut agent = pane(1, Status::TurnFinished);
    assert!(
        !activity
            .observe(std::slice::from_ref(&agent), time)
            .unwrap()
    );
    assert!(activity.all().is_empty());
    agent.status = Status::Working;
    assert!(
        !activity
            .observe(std::slice::from_ref(&agent), time + 1)
            .unwrap()
    );
    agent.status = Status::Idle;
    assert!(
        !activity
            .observe(std::slice::from_ref(&agent), time + 2)
            .unwrap()
    );
    assert!(activity.all().is_empty());
    agent.status = Status::InputNeeded;
    assert!(
        activity
            .observe(std::slice::from_ref(&agent), time + 3)
            .unwrap()
    );
    assert!(
        !activity
            .observe(std::slice::from_ref(&agent), time + 4)
            .unwrap()
    );
    assert_eq!(activity.unread(), 1);
    let id = activity.all()[0].id;
    activity.acknowledge(id).unwrap();
    assert!(activity.acknowledge(id + 10).is_err());
    drop(activity);
    let mut restored = Activity::load(&fixture.0, "test").unwrap();
    assert_eq!(restored.unread(), 0);
    assert!(
        !restored
            .observe(std::slice::from_ref(&agent), time + 5)
            .unwrap()
    );
    agent.status = Status::TurnFinished;
    assert!(
        restored
            .observe(std::slice::from_ref(&agent), time + 6)
            .unwrap()
    );
    agent.status = Status::Failed;
    assert!(restored.observe(&[agent], time + 7).unwrap());
    assert_eq!(restored.all().len(), 3);
    assert_eq!(restored.snapshots()[0].status, Status::Failed);
}
#[test]
fn disappearance_and_reused_pane_retain_known_agent_metadata_without_shell_events() {
    let fixture = Fixture::new();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    let time = kelpmux::now();
    let agent = pane(1, Status::Working);
    activity
        .observe(std::slice::from_ref(&agent), time)
        .unwrap();
    let shell = PaneInfo {
        id: 1,
        pid: agent.pid,
        title: "shell".into(),
        status: Status::Working,
        ..Default::default()
    };
    assert!(
        activity
            .observe(std::slice::from_ref(&shell), time + 1)
            .unwrap()
    );
    assert_eq!(activity.all()[0].title, agent.title);
    assert_eq!(activity.all()[0].status, Status::Exited);
    assert!(!activity.observe(&[shell], time + 2).unwrap());
    let mut reused = agent.clone();
    reused.pid += 1;
    reused.status = Status::InputNeeded;
    reused.session_id = Some("new-session".into());
    assert!(activity.observe(&[reused], time + 3).unwrap());
    assert_eq!(activity.all().len(), 2);
    assert!(activity.observe(&[], time + 4).unwrap());
    assert!(!activity.observe(&[], time + 5).unwrap());
    assert_eq!(activity.all().len(), 3);
}
#[test]
fn lost_identity_metadata_is_not_an_exit_and_plain_output_never_writes_history() {
    let fixture = Fixture::new();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    let time = kelpmux::now();
    let mut agent = pane(1, Status::Working);
    activity
        .observe(std::slice::from_ref(&agent), time)
        .unwrap();
    let before = fs::read(fixture.0.join("activity.json")).unwrap();
    agent.title = "renamed without a transition".into();
    agent.last_activity = time + 3;
    agent.session_id = None;
    assert!(
        !activity
            .observe(std::slice::from_ref(&agent), time + 3)
            .unwrap()
    );
    assert_eq!(fs::read(fixture.0.join("activity.json")).unwrap(), before);
    assert!(activity.all().is_empty());
    agent.session_id = Some("different-known-session".into());
    assert!(activity.observe(&[agent], time + 4).unwrap());
    assert_eq!(activity.all()[0].status, Status::Exited);
}
#[test]
fn notification_storage_is_bounded_and_retained_for_seven_days() {
    let fixture = Fixture::new();
    let time = kelpmux::now();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    let mut agent = pane(1, Status::Working);
    activity
        .observe(std::slice::from_ref(&agent), time)
        .unwrap();
    for index in 0..300 {
        agent.status = Status::Working;
        activity
            .observe(std::slice::from_ref(&agent), time + index * 2)
            .unwrap();
        agent.status = Status::TurnFinished;
        activity
            .observe(std::slice::from_ref(&agent), time + index * 2 + 1)
            .unwrap();
    }
    assert_eq!(activity.all().len(), 256);
    assert_eq!(activity.snapshots().len(), 128);
    assert_eq!(activity.unread(), 256);
    let maximum = activity.all().last().unwrap().id;
    assert!(!activity.observe(&[agent], time + RETENTION + 601).unwrap());
    assert!(activity.all().is_empty());
    drop(activity);
    let restored = Activity::load(&fixture.0, "test").unwrap();
    assert!(restored.all().is_empty());
    assert!(maximum >= 300);
}
#[test]
fn bell_requires_agent_identity_is_throttled_and_cycling_uses_logical_agents() {
    let fixture = Fixture::new();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    let time = kelpmux::now();
    let agent = pane(3, Status::Working);
    let shell = PaneInfo {
        id: 1,
        status: Status::InputNeeded,
        ..Default::default()
    };
    assert!(!activity.bell(&shell, time).unwrap());
    assert!(activity.bell(&agent, time).unwrap());
    assert!(!activity.bell(&agent, time + 1).unwrap());
    assert!(activity.bell(&agent, time + 2).unwrap());
    assert_eq!(activity.all()[0].status, Status::Unknown);
    let panes = vec![
        shell,
        pane(8, Status::InputNeeded),
        pane(2, Status::TurnFinished),
        pane(3, Status::Failed),
    ];
    assert_eq!(next(&panes, AgentFilter::Attention, 8), Some(3));
    assert_eq!(next(&panes, AgentFilter::Attention, 3), Some(8));
    assert_eq!(next(&panes, AgentFilter::Finished, 1), Some(2));
    assert_eq!(next(&panes, AgentFilter::All, 2), Some(3));
}
#[test]
fn corrupt_oversized_symlink_and_invalid_observations_fail_clearly() {
    let fixture = Fixture::new();
    let path = fixture.0.join("activity.json");
    fs::write(&path, b"invalid JSON").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(Activity::load(&fixture.0, "test").is_err());
    fs::write(&path, vec![b' '; 512 * 1024 + 1]).unwrap();
    assert!(
        Activity::load(&fixture.0, "test")
            .err()
            .unwrap()
            .to_string()
            .contains("bounded")
    );
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("/dev/null", &path).unwrap();
    assert!(Activity::load(&fixture.0, "test").is_err());
    fs::remove_file(path).unwrap();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    assert!(
        activity
            .observe(&[pane(0, Status::Working)], kelpmux::now())
            .is_err()
    );
    assert!(
        activity
            .observe(
                &[pane(1, Status::Working), pane(1, Status::Working)],
                kelpmux::now()
            )
            .is_err()
    );
}

#[test]
fn delayed_initial_discovery_does_not_create_finished_notification_flood() {
    let fixture = Fixture::new();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    let time = kelpmux::now();
    let shell = PaneInfo {
        id: 1,
        pid: 101,
        ..Default::default()
    };
    activity.observe(&[shell], time).unwrap();
    let mut agent = pane(1, Status::TurnFinished);
    assert!(
        !activity
            .observe(std::slice::from_ref(&agent), time + 1)
            .unwrap()
    );
    assert!(activity.all().is_empty());
    agent.status = Status::Working;
    activity
        .observe(std::slice::from_ref(&agent), time + 2)
        .unwrap();
    agent.status = Status::TurnFinished;
    assert!(activity.observe(&[agent], time + 3).unwrap());
    assert_eq!(activity.all().len(), 1);
}
