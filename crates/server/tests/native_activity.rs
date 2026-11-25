use kelpserver::{
    journal::RETENTION,
    model::{Kind, Status},
    native::{
        activity::{Activity, BELL_PAIR_WINDOW},
        protocol::PaneInfo,
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
fn pending_background_work_has_a_static_indicator_and_no_completion_alert() {
    let fixture = Fixture::new();
    let time = kelpserver::now();
    let mut activity = Activity::load(&fixture.0, "background").unwrap();
    let mut agent = pane(1, Status::Working);
    activity
        .observe(std::slice::from_ref(&agent), time)
        .unwrap();
    agent.status = Status::WaitingBackground;
    activity
        .observe(std::slice::from_ref(&agent), time + 1)
        .unwrap();
    assert!(activity.all().is_empty());
}
#[test]
fn notification_baselines_support_more_than_sixty_four_tabs() {
    let fixture = Fixture::new();
    let time = kelpserver::now();
    let mut activity = Activity::load(&fixture.0, "large").unwrap();
    let mut panes: Vec<_> = (1..=100).map(|id| pane(id, Status::Working)).collect();
    activity.observe(&panes, time).unwrap();
    panes[99].status = Status::InputNeeded;
    activity.observe(&panes, time + 1).unwrap();
    drop(activity);
    let activity = Activity::load(&fixture.0, "large").unwrap();
    assert!(activity.all().iter().any(|event| event.pane == 100));
}

#[test]
fn initial_baseline_dedup_explicit_statuses_and_ack_persist_across_restart() {
    let fixture = Fixture::new();
    let time = kelpserver::now();
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
    let time = kelpserver::now();
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
    let time = kelpserver::now();
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
    let time = kelpserver::now();
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
fn bell_requires_agent_identity_and_is_throttled() {
    let fixture = Fixture::new();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    let time = kelpserver::now();
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
            .observe(&[pane(0, Status::Working)], kelpserver::now())
            .is_err()
    );
    assert!(
        activity
            .observe(
                &[pane(1, Status::Working), pane(1, Status::Working)],
                kelpserver::now()
            )
            .is_err()
    );
}

#[test]
fn delayed_initial_discovery_does_not_create_finished_notification_flood() {
    let fixture = Fixture::new();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    let time = kelpserver::now();
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

#[test]
fn fresh_idle_claude_bells_are_silent_but_approvals_and_post_work_bells_still_alert() {
    let fixture = Fixture::new();
    let time = kelpserver::now();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    activity.observe(&[], time).unwrap();
    let mut agent = pane(1, Status::Idle);
    agent.kind = Some(Kind::Claude);
    assert!(
        !activity
            .observe(std::slice::from_ref(&agent), time + 1)
            .unwrap()
    );
    assert!(!activity.bell(&agent, time + 2).unwrap());
    assert!(activity.all().is_empty());
    agent.status = Status::InputNeeded;
    assert!(
        activity
            .observe(std::slice::from_ref(&agent), time + 3)
            .unwrap()
    );
    assert_eq!(activity.all()[0].status, Status::InputNeeded);
    agent.status = Status::Working;
    activity
        .observe(std::slice::from_ref(&agent), time + 4)
        .unwrap();
    agent.status = Status::Idle;
    activity
        .observe(std::slice::from_ref(&agent), time + 5)
        .unwrap();
    assert!(activity.bell(&agent, time + 6).unwrap());
    assert_eq!(activity.all().len(), 2);
    agent.session_id = Some("another-session".into());
    activity
        .observe(std::slice::from_ref(&agent), time + 7)
        .unwrap();
    let count = activity.all().len();
    assert!(!activity.bell(&agent, time + 8).unwrap());
    assert_eq!(activity.all().len(), count);
}

#[test]
fn starting_an_agent_at_a_startup_dialog_does_not_notify() {
    let fixture = Fixture::new();
    let time = kelpserver::now();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    let shell = PaneInfo {
        id: 1,
        pid: 101,
        title: "shell".into(),
        ..Default::default()
    };
    activity.observe(&[shell], time).unwrap();
    // `claude` starts in the shell: first seen waiting at a startup dialog,
    // before it has a session.
    let mut agent = pane(1, Status::InputNeeded);
    agent.kind = Some(Kind::Claude);
    agent.session_id = None;
    agent.title = "claude · session unknown".into();
    assert!(
        !activity
            .observe(std::slice::from_ref(&agent), time + 1)
            .unwrap()
    );
    // The dialog is answered and the session appears: still nothing.
    agent.session_id = Some("session-1".into());
    agent.status = Status::Idle;
    assert!(
        !activity
            .observe(std::slice::from_ref(&agent), time + 2)
            .unwrap()
    );
    assert!(!activity.bell(&agent, time + 3).unwrap());
    assert!(activity.all().is_empty());
    // Real work still alerts: a question, then a completion.
    agent.status = Status::Working;
    activity
        .observe(std::slice::from_ref(&agent), time + 4)
        .unwrap();
    agent.status = Status::InputNeeded;
    assert!(
        activity
            .observe(std::slice::from_ref(&agent), time + 5)
            .unwrap()
    );
    agent.status = Status::TurnFinished;
    assert!(activity.observe(&[agent], time + 6).unwrap());
    let statuses: Vec<_> = activity.all().iter().map(|n| n.status).collect();
    assert_eq!(statuses.len(), 2);
    assert!(statuses.contains(&Status::InputNeeded) && statuses.contains(&Status::TurnFinished));
}

/// An agent known to the activity baseline, then working.
fn working_agent(activity: &mut Activity, id: u64, time: u64) -> PaneInfo {
    activity.observe(&[], time).unwrap();
    let agent = pane(id, Status::Working);
    activity
        .observe(std::slice::from_ref(&agent), time)
        .unwrap();
    agent
}

#[test]
fn a_bell_followed_by_a_typed_notification_becomes_one_marked_record() {
    let fixture = Fixture::new();
    let time = kelpserver::now();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    let mut agent = working_agent(&mut activity, 1, time);
    assert!(activity.bell(&agent, time + 1).unwrap());
    let bell = activity.all()[0].id;
    agent.status = Status::InputNeeded;
    assert!(
        activity
            .observe(std::slice::from_ref(&agent), time + 1 + BELL_PAIR_WINDOW)
            .unwrap()
    );
    let all = activity.all();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].status, Status::InputNeeded);
    assert_eq!(all[0].title, "Agent 1");
    assert!(all[0].bell && !all[0].read);
    assert_ne!(all[0].id, bell, "a merged record is a new event");
    // The merge persists.
    drop(activity);
    let restored = Activity::load(&fixture.0, "test").unwrap();
    assert_eq!(restored.all().len(), 1);
    assert!(restored.all()[0].bell);
}

#[test]
fn a_bell_following_a_typed_notification_is_absorbed() {
    let fixture = Fixture::new();
    let time = kelpserver::now();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    let mut agent = working_agent(&mut activity, 1, time);
    agent.status = Status::TurnFinished;
    assert!(
        activity
            .observe(std::slice::from_ref(&agent), time + 1)
            .unwrap()
    );
    let id = activity.all()[0].id;
    assert!(!activity.all()[0].bell);
    assert!(!activity.bell(&agent, time + 1 + BELL_PAIR_WINDOW).unwrap());
    // A second bell in the window changes nothing.
    assert!(!activity.bell(&agent, time + 2).unwrap());
    let all = activity.all();
    assert_eq!(all.len(), 1);
    assert_eq!((all[0].id, all[0].status), (id, Status::TurnFinished));
    assert!(all[0].bell && !all[0].read);
    drop(activity);
    assert!(Activity::load(&fixture.0, "test").unwrap().all()[0].bell);
}

#[test]
fn a_bell_alone_stays_a_bell_and_typed_notifications_stay_unmarked() {
    let fixture = Fixture::new();
    let time = kelpserver::now();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    let mut first = working_agent(&mut activity, 1, time);
    assert!(activity.bell(&first, time + 1).unwrap());
    let all = activity.all();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].status, Status::Unknown);
    assert_eq!(all[0].title, "Bell · Agent 1");
    assert!(!all[0].bell);
    // A typed notification without a bell is unmarked, and so is its
    // serialization (old clients see exactly the previous wire format).
    first.status = Status::Failed;
    let mut second = pane(2, Status::Working);
    activity
        .observe(&[first.clone(), second.clone()], time + 10)
        .unwrap();
    second.status = Status::InputNeeded;
    activity.observe(&[first, second], time + 20).unwrap();
    let all = activity.all();
    assert_eq!(all.len(), 3);
    assert!(all.iter().all(|event| !event.bell));
    let json = serde_json::to_value(&all[2]).unwrap();
    assert!(json.get("bell").is_none());
}

#[test]
fn a_bell_outside_the_window_stays_a_separate_record() {
    let fixture = Fixture::new();
    let time = kelpserver::now();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    let mut agent = working_agent(&mut activity, 1, time);
    assert!(activity.bell(&agent, time + 1).unwrap());
    agent.status = Status::InputNeeded;
    activity
        .observe(std::slice::from_ref(&agent), time + 2 + BELL_PAIR_WINDOW)
        .unwrap();
    // And the other order: a bell after the typed notification's window.
    assert!(
        activity
            .bell(&agent, time + 3 + 2 * BELL_PAIR_WINDOW)
            .unwrap()
    );
    let statuses: Vec<_> = activity
        .all()
        .iter()
        .map(|event| (event.status, event.bell))
        .collect();
    assert_eq!(
        statuses,
        [
            (Status::Unknown, false),
            (Status::InputNeeded, false),
            (Status::Unknown, false)
        ]
    );
}

#[test]
fn bells_of_another_pane_or_process_do_not_merge() {
    let fixture = Fixture::new();
    let time = kelpserver::now();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    activity.observe(&[], time).unwrap();
    let mut first = pane(1, Status::Working);
    let second = pane(2, Status::Working);
    activity
        .observe(&[first.clone(), second.clone()], time)
        .unwrap();
    // Bell of pane 2, then a question of pane 1.
    assert!(activity.bell(&second, time + 1).unwrap());
    first.status = Status::InputNeeded;
    activity
        .observe(&[first.clone(), second.clone()], time + 1)
        .unwrap();
    // A question of pane 1, then a bell of pane 2 (outside the bell throttle).
    assert!(activity.bell(&second, time + 3).unwrap());
    // A new process in pane 1 rings right after the old one's question.
    let mut replaced = first.clone();
    replaced.pid += 1;
    replaced.status = Status::Working;
    activity
        .observe(&[replaced.clone(), second], time + 3)
        .unwrap();
    let count = activity.all().len();
    assert!(activity.bell(&replaced, time + 3).unwrap());
    assert_eq!(activity.all().len(), count + 1);
    assert!(activity.all().iter().all(|event| !event.bell));
}

#[test]
fn acknowledging_a_merged_record_clears_the_bell_too() {
    let fixture = Fixture::new();
    let time = kelpserver::now();
    let mut activity = Activity::load(&fixture.0, "test").unwrap();
    let mut agent = working_agent(&mut activity, 1, time);
    assert!(activity.bell(&agent, time + 1).unwrap());
    let bell = activity.all()[0].id;
    agent.status = Status::InputNeeded;
    activity
        .observe(std::slice::from_ref(&agent), time + 2)
        .unwrap();
    assert_eq!(activity.unread(), 1);
    let merged = activity.all()[0].id;
    activity.acknowledge(merged).unwrap();
    assert_eq!(activity.unread(), 0);
    // A client that saw only the bell acknowledges it after the merge: the
    // merged record is the one it reaches.
    agent.status = Status::Working;
    activity
        .observe(std::slice::from_ref(&agent), time + 10)
        .unwrap();
    assert!(activity.bell(&agent, time + 11).unwrap());
    let bell2 = activity.all().last().unwrap().id;
    agent.status = Status::TurnFinished;
    activity
        .observe(std::slice::from_ref(&agent), time + 12)
        .unwrap();
    assert_eq!(activity.unread(), 1);
    activity.acknowledge(bell2).unwrap();
    assert_eq!(activity.unread(), 0);
    assert!(activity.all().iter().all(|event| event.bell));
    assert!(activity.all().iter().all(|event| event.id != bell));
    // Unknown identifiers still fail.
    assert!(activity.acknowledge(u64::MAX).is_err());
}
