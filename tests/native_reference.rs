use kelpmux::native::{
    protocol::*,
    reference::State,
    stream::{self, Event, PanePatch, PaneSnapshot, Update},
};
fn hello() -> Event {
    Event::Welcome {
        version: stream::VERSION,
        client: 5,
        epoch: "unique-server-epoch".into(),
        capabilities: vec!["pane-patches".into()],
    }
}
fn snapshot(pane: u64) -> PaneSnapshot {
    PaneSnapshot {
        pane,
        revision: 1,
        screen: Screen {
            cols: 4,
            rows: 2,
            cells: vec![Cell::default(); 8],
            cursor: None,
            modes: InputModes::default(),
        },
    }
}
fn update() -> Update {
    Update {
        epoch: "unique-server-epoch".into(),
        subscribed: vec![1, 2],
        snapshots: vec![snapshot(1), snapshot(2)],
        patches: vec![],
        panes: vec![
            PaneInfo {
                id: 1,
                ..Default::default()
            },
            PaneInfo {
                id: 2,
                ..Default::default()
            },
        ],
        leases: vec![],
        notifications: vec![],
        vitals: Default::default(),
    }
}
fn state() -> State {
    let mut s = State::default();
    s.apply(hello()).unwrap();
    s.apply(Event::Update(update())).unwrap();
    s
}
fn as_json(s: &State) -> serde_json::Value {
    serde_json::to_value(s).unwrap()
}
#[test]
fn epoch_handshake_and_missing_baseline_errors_are_transactional() {
    let mut s = State::default();
    assert!(s.apply(Event::Update(update())).is_err());
    assert!(s.epoch.is_empty());
    s.apply(hello()).unwrap();
    assert!(s.apply(hello()).is_err());
    let before = as_json(&s);
    let mut wrong = update();
    wrong.epoch = "restarted-server".into();
    assert!(s.apply(Event::Update(wrong)).is_err());
    assert_eq!(as_json(&s), before);
    let mut missing = update();
    missing.snapshots.clear();
    assert!(s.apply(Event::Update(missing)).is_err());
    assert_eq!(as_json(&s), before);
    let mut invalid = State::default();
    assert!(
        invalid
            .apply(Event::Welcome {
                version: 999,
                client: 5,
                epoch: "x".into(),
                capabilities: vec![]
            })
            .is_err()
    );
    assert!(invalid.epoch.is_empty());
}
#[test]
fn patches_require_exact_baseline_and_failure_never_partially_commits() {
    let mut s = state();
    let before = as_json(&s);
    let mut patch = PanePatch {
        pane: 1,
        base: 1,
        revision: 2,
        cursor: None,
        modes: InputModes::default(),
        changes: vec![(
            0,
            Cell {
                text: "x".into(),
                ..Default::default()
            },
        )],
    };
    let mut bad = update();
    bad.snapshots.clear();
    bad.patches.push(patch.clone());
    patch.pane = 2;
    patch.base = 99;
    patch.revision = 100;
    bad.patches.push(patch.clone());
    assert!(s.apply(Event::Update(bad)).is_err());
    assert_eq!(as_json(&s), before);
    let mut good = update();
    good.snapshots.clear();
    patch.pane = 1;
    patch.base = 1;
    patch.revision = 2;
    good.patches.push(patch.clone());
    s.apply(Event::Update(good.clone())).unwrap();
    assert_eq!(s.screens[&1].screen.cells[0].text, "x");
    assert!(
        s.apply(Event::Update(good)).is_err(),
        "replay must be rejected"
    );
    assert_eq!(s.screens[&1].revision, 2);
}
#[test]
fn resubscribe_prunes_cached_screens_and_requires_new_members_baselines() {
    let mut s = state();
    let mut one = update();
    one.subscribed = vec![2];
    one.snapshots.clear();
    s.apply(Event::Update(one)).unwrap();
    assert_eq!(s.screens.keys().copied().collect::<Vec<_>>(), vec![2]);
    let mut both = update();
    both.snapshots.clear();
    let before = as_json(&s);
    assert!(s.apply(Event::Update(both)).is_err());
    assert_eq!(as_json(&s), before);
    let mut both = update();
    both.snapshots = vec![snapshot(1)];
    s.apply(Event::Update(both)).unwrap();
    assert_eq!(s.screens.len(), 2);
    let mut empty = update();
    empty.subscribed.clear();
    empty.snapshots.clear();
    s.apply(Event::Update(empty)).unwrap();
    assert!(s.screens.is_empty());
}
#[test]
fn duplicate_metadata_and_unsafe_cells_are_rejected_without_replacing_state() {
    let original = state();
    for which in 0..7 {
        let mut s = original.clone();
        let before = as_json(&s);
        let mut u = update();
        match which {
            0 => u.subscribed.push(1),
            1 => u.snapshots.push(snapshot(1)),
            2 => u.panes.push(u.panes[0].clone()),
            3 => {
                u.snapshots[0].screen.cells[0].text = "\x1b[31m".into();
            }
            4 => {
                u.snapshots[0].screen.cells[3].text = "界".into();
                u.snapshots[0].screen.cells[3].width = 2;
            }
            5 => {
                u.snapshots[0].screen.cells[0].text = "ab".into();
            }
            _ => {
                u.leases.push(stream::Lease { pane: 1, owner: 0 });
            }
        }
        assert!(s.apply(Event::Update(u)).is_err(), "case {which}");
        assert_eq!(as_json(&s), before);
    }
}

#[test]
fn standalone_lease_events_have_bounds_and_validate_known_membership_transactionally() {
    let mut empty = State::default();
    empty.apply(hello()).unwrap();
    for pane in 1..=10 {
        empty
            .apply(Event::Lease {
                pane,
                owner: Some(5),
            })
            .unwrap();
    }
    let before = as_json(&empty);
    assert!(
        empty
            .apply(Event::Lease {
                pane: 11,
                owner: Some(5)
            })
            .is_err()
    );
    assert_eq!(as_json(&empty), before);
    let mut known = state();
    let before = as_json(&known);
    assert!(
        known
            .apply(Event::Lease {
                pane: 99,
                owner: Some(5)
            })
            .is_err()
    );
    assert_eq!(as_json(&known), before);
}
