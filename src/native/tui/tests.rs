//! Controller tests: tab behaviour, reconciliation with persisted state,
//! leases and input routing, and rendering of the main views. They drive the
//! controller with typed stream events, without a server; the integration
//! tests in `tests/native_stream_tui.rs` use a real KelpServer.
use super::{
    controller::{Controller, Options, REQUIRED, SCRATCH_TITLE},
    state::{Document, EpochState, LayoutTab, PaneState, Store},
};
use crate::{
    model::Status,
    native::{
        input::Router,
        protocol::*,
        stream::{CloseReason, Event, Lease, PaneSession, PaneSnapshot, Request, Update},
        ui::DialogKind,
    },
};

const ME: u64 = 7;
const OTHER: u64 = 9;

fn info(id: PaneId) -> PaneInfo {
    PaneInfo {
        id,
        pid: 1000 + id as u32,
        cwd: format!("/srv/{id}"),
        title: format!("tab {id}"),
        status: Status::Idle,
        ..PaneInfo::default()
    }
}
fn session(id: PaneId, cwd: &str) -> PaneSession {
    PaneSession {
        pane: id,
        created_at: 1_000,
        launch_cwd: cwd.into(),
        shell: "/bin/sh".into(),
        argv: Vec::new(),
        startup: None,
        terminal_title: None,
    }
}
fn update(panes: Vec<PaneInfo>) -> Update {
    let sessions = panes
        .iter()
        .map(|pane| session(pane.id, &pane.cwd))
        .collect();
    Update {
        epoch: "e1".into(),
        subscribed: Vec::new(),
        snapshots: Vec::new(),
        patches: Vec::new(),
        panes,
        leases: Vec::new(),
        notifications: Vec::new(),
        vitals: Default::default(),
        sessions,
        host: None,
    }
}
fn screen(cols: u16, rows: u16, text: &str) -> Screen {
    let mut cells = vec![Cell::default(); usize::from(cols) * usize::from(rows)];
    for (index, ch) in text.chars().enumerate() {
        cells[index].text = ch.to_string();
    }
    Screen {
        cols,
        rows,
        cells,
        cursor: None,
        modes: InputModes::default(),
    }
}
fn controller_with(document: Document) -> Controller {
    controller_from(Store::memory(document))
}
fn controller_from(store: Store) -> Controller {
    let mut controller = Controller::new(
        120,
        40,
        store,
        Options {
            workspace: "main".into(),
            ..Options::default()
        },
    )
    .unwrap();
    let mut capabilities: Vec<String> = REQUIRED.iter().map(|s| (*s).into()).collect();
    capabilities.push("clipboard-write".into());
    controller
        .welcome(ME, "e1".into(), capabilities, Some("test".into()))
        .unwrap();
    controller
}
/// A synced controller whose workspace has already been migrated, with
/// panes `ids` in ID order.
fn attached(ids: &[PaneId]) -> Controller {
    let mut controller = controller_with(Document {
        migrated: true,
        epochs: [("e1".to_owned(), EpochState::default())].into(),
        ..Document::default()
    });
    controller
        .event(Event::Update(update(
            ids.iter().map(|id| info(*id)).collect(),
        )))
        .unwrap();
    controller.take_requests();
    controller
}
fn order(controller: &Controller) -> Vec<PaneId> {
    controller.workspace.panes.iter().map(|p| p.id).collect()
}
fn act(controller: &mut Controller, action: Action) {
    controller.message(ClientMessage::Action(action));
    assert_eq!(controller.notice, None);
}
fn closed(controller: &mut Controller, pane: PaneId) {
    controller
        .event(Event::PaneClosed {
            pane,
            at: 0,
            reason: CloseReason::Closed,
            record: None,
        })
        .unwrap();
}
fn spawned(controller: &mut Controller, pane: PaneId) {
    let request = controller
        .take_requests()
        .into_iter()
        .find_map(|request| match request {
            Request::Spawn { request, .. } => Some(request),
            _ => None,
        })
        .expect("a spawn request");
    controller
        .event(Event::PaneCreated {
            session: session(pane, "/srv/new"),
        })
        .unwrap();
    controller.event(Event::Spawned { request, pane }).unwrap();
}
fn text(frame: &Frame) -> Vec<String> {
    frame
        .cells
        .chunks(usize::from(frame.cols))
        .map(|row| row.iter().map(|c| c.text.as_str()).collect())
        .collect()
}

#[test]
fn unparking_puts_the_tab_last() {
    let mut c = attached(&[1, 2, 3]);
    act(&mut c, Action::Park(2));
    assert!(c.workspace.panes.iter().any(|p| p.id == 2 && p.parked));
    act(&mut c, Action::Restore(2));
    assert_eq!(order(&c), vec![1, 3, 2]);
    assert_eq!(c.workspace.active, 2);
    // The new order is client state.
    assert_eq!(c.store.current().unwrap().order, vec![1, 3, 2]);
}

#[test]
fn closing_the_current_tab_focuses_the_previously_selected_one() {
    let mut c = attached(&[1, 2, 3, 4]);
    act(&mut c, Action::Select(1));
    act(&mut c, Action::Focus(3));
    act(&mut c, Action::Focus(2));
    closed(&mut c, 2);
    assert_eq!(c.workspace.active, 3);
    assert_eq!(c.workspace.reading, ReadingMode::Tabs(3));
    // History skips closed tabs: 3 was the last one, then 1.
    closed(&mut c, 3);
    assert_eq!(c.workspace.active, 1);
}

#[test]
fn parking_or_closing_skips_parked_tabs_and_falls_back_to_the_first() {
    let mut c = attached(&[1, 2, 3]);
    act(&mut c, Action::Select(2));
    act(&mut c, Action::Focus(3));
    act(&mut c, Action::Park(2));
    act(&mut c, Action::Focus(3));
    // 2 is parked, so parking 3 cannot return there: first tab.
    act(&mut c, Action::Park(3));
    assert_eq!(c.workspace.active, 1);
    // Without any focus history, closing falls back to the first tab.
    let mut c = attached(&[4, 5, 6]);
    act(&mut c, Action::Select(3));
    closed(&mut c, 6);
    assert_eq!(c.workspace.active, 4);
}

#[test]
fn new_tab_spawns_and_selects_the_new_pane() {
    let mut c = attached(&[1, 2]);
    act(&mut c, Action::Select(1));
    act(&mut c, Action::NewTab);
    spawned(&mut c, 3);
    assert_eq!(order(&c), vec![1, 2, 3]);
    assert_eq!(c.workspace.active, 3);
    assert_eq!(c.workspace.reading, ReadingMode::Tabs(3));
}

#[test]
fn scratch_is_an_ordinary_pane_with_a_client_marker() {
    let mut c = attached(&[1, 2]);
    act(&mut c, Action::Scratch);
    spawned(&mut c, 3);
    assert_eq!(c.workspace.ui.scratch, Some(3));
    assert!(c.workspace.ui.scratch_visible);
    assert_eq!(c.workspace.active, 3);
    let pane = c.workspace.panes.iter().find(|p| p.id == 3).unwrap();
    assert_eq!(pane.title, SCRATCH_TITLE);
    assert!(c.store.pane(3).unwrap().scratch);
    // Scratch is not a tab: it is excluded from the order and the template.
    assert_eq!(c.store.current().unwrap().order, vec![1, 2]);
    act(&mut c, Action::Scratch);
    assert!(!c.workspace.ui.scratch_visible);
    assert_eq!(c.workspace.active, 1);
    // A second toggle reuses the pane.
    act(&mut c, Action::Scratch);
    assert!(
        !c.take_requests()
            .iter()
            .any(|r| matches!(r, Request::Spawn { .. }))
    );
}

#[test]
fn closing_the_last_tab_spawns_its_replacement_first() {
    let mut c = attached(&[1]);
    act(&mut c, Action::ClosePaneRequest(1));
    act(&mut c, Action::ClosePaneConfirm(1));
    let requests = c.take_requests();
    assert!(requests.iter().any(|r| matches!(r, Request::Spawn { .. })));
    assert!(!requests.iter().any(|r| matches!(r, Request::Close { .. })));
    let request = requests
        .iter()
        .find_map(|r| match r {
            Request::Spawn { request, .. } => Some(request.clone()),
            _ => None,
        })
        .unwrap();
    c.event(Event::PaneCreated {
        session: session(2, "/srv"),
    })
    .unwrap();
    c.event(Event::Spawned { request, pane: 2 }).unwrap();
    let requests = c.take_requests();
    assert!(
        requests
            .iter()
            .any(|r| matches!(r, Request::Close { pane: 1, .. }))
    );
    closed(&mut c, 1);
    assert_eq!(c.workspace.active, 2);
}

#[test]
fn closing_needs_the_confirmation_dialog() {
    let mut c = attached(&[1, 2]);
    c.message(ClientMessage::Action(Action::ClosePaneConfirm(1)));
    assert!(c.notice.is_some());
    assert!(
        !c.take_requests()
            .iter()
            .any(|r| matches!(r, Request::Close { .. }))
    );
}

#[test]
fn reattaching_in_the_same_epoch_restores_order_parking_and_colours() {
    let mut epoch = EpochState {
        order: vec![3, 1, 2],
        ..EpochState::default()
    };
    epoch.panes.insert(
        2,
        PaneState {
            parked: true,
            ..PaneState::default()
        },
    );
    epoch.panes.insert(
        1,
        PaneState {
            name: Some("Build".into()),
            color: Some(5),
            ..PaneState::default()
        },
    );
    epoch.panes.insert(
        4,
        PaneState {
            scratch: true,
            ..PaneState::default()
        },
    );
    let mut c = controller_with(Document {
        migrated: true,
        epochs: [("e1".to_owned(), epoch)].into(),
        ..Document::default()
    });
    c.event(Event::Update(update(vec![
        info(1),
        info(2),
        info(3),
        info(4),
    ])))
    .unwrap();
    assert_eq!(order(&c), vec![3, 1, 2, 4]);
    assert_eq!(c.workspace.ui.scratch, Some(4));
    let one = c.workspace.panes.iter().find(|p| p.id == 1).unwrap();
    assert_eq!((one.title.as_str(), one.palette), ("Build", 5));
    assert!(c.workspace.panes.iter().any(|p| p.id == 2 && p.parked));
}

#[test]
fn the_first_stream_attach_adopts_the_composed_arrangement() {
    let mut c = controller_with(Document::default());
    let mut panes = vec![info(1), info(2), info(3)];
    panes[0].number = 2;
    panes[1].parked = true;
    panes[2].number = 1;
    panes[2].palette = 7;
    c.event(Event::Update(update(panes))).unwrap();
    assert_eq!(order(&c), vec![3, 1, 2]);
    assert!(c.workspace.panes.iter().any(|p| p.id == 2 && p.parked));
    assert_eq!(c.store.pane(3).unwrap().color, Some(7));
    assert!(c.store.document.migrated);
}

#[test]
fn a_restarted_server_gets_the_layout_template() {
    let layout = vec![
        LayoutTab {
            cwd: "/srv/a".into(),
            name: Some("A".into()),
            color: Some(4),
            ..LayoutTab::default()
        },
        LayoutTab {
            cwd: "/srv/b".into(),
            name: Some("B".into()),
            parked: true,
            ..LayoutTab::default()
        },
    ];
    let mut c = controller_with(Document {
        migrated: true,
        layout: layout.clone(),
        epochs: [("old".to_owned(), EpochState::default())].into(),
        ..Document::default()
    });
    let mut b = info(5);
    b.cwd = "/srv/b".into();
    let mut a = info(6);
    a.cwd = "/srv/a".into();
    c.event(Event::Update(update(vec![b, a]))).unwrap();
    assert_eq!(order(&c), vec![6, 5]);
    let a = c.workspace.panes.iter().find(|p| p.id == 6).unwrap();
    assert_eq!((a.title.as_str(), a.palette), ("A", 4));
    assert!(c.workspace.panes.iter().any(|p| p.id == 5 && p.parked));

    // A fresh server with fewer startup shells gets the missing tabs.
    let mut c = controller_with(Document {
        migrated: true,
        layout,
        ..Document::default()
    });
    let mut a = info(1);
    a.cwd = "/srv/a".into();
    c.event(Event::Update(update(vec![a]))).unwrap();
    let spawns: Vec<_> = c
        .take_requests()
        .into_iter()
        .filter_map(|r| match r {
            Request::Spawn { cwd, request, .. } => Some((cwd, request)),
            _ => None,
        })
        .collect();
    assert_eq!(spawns.len(), 1);
    assert_eq!(spawns[0].0.as_deref(), Some(std::path::Path::new("/srv/b")));
    c.event(Event::PaneCreated {
        session: session(2, "/srv/b"),
    })
    .unwrap();
    c.event(Event::Spawned {
        request: spawns[0].1.clone(),
        pane: 2,
    })
    .unwrap();
    assert_eq!(order(&c), vec![1, 2]);
    assert!(c.workspace.panes.iter().any(|p| p.id == 2 && p.parked));
}

#[test]
fn a_controlling_client_claims_and_sizes_free_panes_only() {
    let mut c = controller_with(Document {
        migrated: true,
        ..Document::default()
    });
    let mut first = update(vec![info(1), info(2)]);
    first.leases = vec![Lease {
        pane: 2,
        owner: OTHER,
    }];
    c.event(Event::Update(first)).unwrap();
    assert!(c.controlling);
    let requests = c.take_requests();
    assert!(requests.iter().any(|r| matches!(
        r,
        Request::Claim {
            pane: 1,
            takeover: false
        }
    )));
    assert!(
        !requests
            .iter()
            .any(|r| matches!(r, Request::Claim { pane: 2, .. }))
    );
    assert!(
        requests
            .iter()
            .any(|r| matches!(r, Request::Resize { pane: 1, .. }))
    );
    // Focus takes every displayed pane, including the other client's.
    c.take_control();
    assert!(c.take_requests().iter().any(|r| matches!(
        r,
        Request::Claim {
            pane: 2,
            takeover: true
        }
    )));
}

#[test]
fn another_client_taking_the_active_pane_makes_this_one_observe() {
    let mut c = attached(&[1, 2]);
    c.event(Event::Lease {
        pane: 1,
        owner: Some(ME),
    })
    .unwrap();
    let mut next = update(vec![info(1), info(2)]);
    next.leases = vec![Lease {
        pane: c.workspace.active,
        owner: OTHER,
    }];
    c.event(Event::Update(next)).unwrap();
    assert!(!c.controlling);
    assert!(!c.frame().controller);
    // Typing takes it back.
    c.message(ClientMessage::Input(b"x".to_vec()));
    let requests = c.take_requests();
    let claim = requests
        .iter()
        .position(|r| matches!(r, Request::Claim { takeover: true, .. }))
        .unwrap();
    let input = requests
        .iter()
        .position(|r| matches!(r, Request::Input { .. }))
        .unwrap();
    assert!(claim < input);
    assert!(c.controlling);
}

#[test]
fn prefix_keys_drive_the_session_api() {
    let mut c = attached(&[1, 2]);
    let mut router = Router::default();
    let mut press = |c: &mut Controller, bytes: &[u8]| {
        let frame = c.frame();
        for message in router.route(InputEvent::Bytes(bytes.to_vec()), &frame) {
            c.message(message);
        }
    };
    press(&mut c, &[2]);
    press(&mut c, b"\r");
    assert!(
        c.take_requests()
            .iter()
            .any(|r| matches!(r, Request::Spawn { argv, .. } if argv.is_empty()))
    );
    press(&mut c, &[2]);
    press(&mut c, b"p");
    assert_eq!(
        c.workspace.ui.dialog.map(|d| d.kind),
        Some(DialogKind::ParkConfirm)
    );
    press(&mut c, b"\x1b");
    press(&mut c, &[2]);
    press(&mut c, b"r");
    assert!(
        c.take_requests()
            .iter()
            .any(|r| matches!(r, Request::ClosedSessions { .. }))
    );
    assert!(c.workspace.ui.recovery_loading);
    press(&mut c, b"\x1b");
    // Ordinary keys go to the active pane.
    press(&mut c, b"ls");
    assert!(
        c.take_requests()
            .iter()
            .any(|r| matches!(r, Request::Input { bytes, .. } if bytes == b"ls"))
    );
    press(&mut c, &[2]);
    press(&mut c, b"d");
    assert!(c.detached);
}

#[test]
fn focusing_a_pane_acknowledges_its_notifications() {
    let mut c = attached(&[1, 2]);
    let mut next = update(vec![info(1), info(2)]);
    next.notifications = vec![Notification {
        origin: Some(NotificationOrigin {
            run: "run".into(),
            pid: 1002,
        }),
        id: 11,
        pane: 2,
        title: "Done".into(),
        status: Status::TurnFinished,
        at: 1,
        read: false,
        workspace: "main".into(),
    }];
    c.event(Event::Update(next)).unwrap();
    assert_eq!(c.workspace.ui.notification_run, "run");
    act(&mut c, Action::Focus(2));
    assert!(
        c.take_requests()
            .iter()
            .any(|r| matches!(r, Request::Acknowledge { notification: 11 }))
    );
}

#[test]
fn selections_copy_from_the_local_screen() {
    let mut c = attached(&[1]);
    let mut next = update(vec![info(1)]);
    next.subscribed = vec![1];
    next.snapshots = vec![PaneSnapshot {
        pane: 1,
        revision: 1,
        screen: screen(20, 3, "hello world"),
    }];
    c.event(Event::Update(next)).unwrap();
    act(
        &mut c,
        Action::SelectText {
            pane: 1,
            start: Point { x: 6, y: 0 },
            end: Point { x: 10, y: 0 },
        },
    );
    act(&mut c, Action::Copy(1));
    assert_eq!(c.clipboard, vec!["world".to_owned()]);
}

#[test]
fn bad_patches_resynchronize_the_pane() {
    let mut c = attached(&[1]);
    let mut next = update(vec![info(1)]);
    next.subscribed = vec![1];
    next.snapshots = vec![PaneSnapshot {
        pane: 1,
        revision: 1,
        screen: screen(10, 2, "abc"),
    }];
    c.event(Event::Update(next)).unwrap();
    let mut patched = update(vec![info(1)]);
    patched.subscribed = vec![1];
    patched.patches = vec![crate::native::stream::PanePatch {
        pane: 1,
        base: 1,
        revision: 2,
        cursor: None,
        modes: InputModes::default(),
        changes: vec![(
            0,
            Cell {
                text: "X".into(),
                ..Cell::default()
            },
        )],
    }];
    c.event(Event::Update(patched.clone())).unwrap();
    assert_eq!(c.screens[&1].cells[0].text, "X");
    // The same patch again no longer matches the baseline.
    c.event(Event::Update(patched)).unwrap();
    assert!(!c.screens.contains_key(&1));
    assert!(
        c.take_requests()
            .iter()
            .any(|r| matches!(r, Request::Resync { pane: 1 }))
    );
}

#[test]
fn a_different_epoch_ends_the_session() {
    let mut c = attached(&[1]);
    let mut next = update(vec![info(1)]);
    next.epoch = "e2".into();
    assert!(c.event(Event::Update(next)).is_err());
}

#[test]
fn servers_without_the_session_api_are_refused() {
    let mut c = Controller::new(
        120,
        40,
        Store::memory(Document::default()),
        Options::default(),
    )
    .unwrap();
    let error = c
        .welcome(1, "e".into(), vec!["pane-snapshots".into()], None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("--legacy"), "{error}");
}

#[test]
fn renders_the_sidebar_tabs_and_the_active_terminal() {
    let mut c = attached(&[1, 2]);
    act(&mut c, Action::Select(1));
    let size = c.take_requests().into_iter().find_map(|r| match r {
        Request::Resize {
            pane: 1,
            cols,
            rows,
        } => Some((cols, rows)),
        _ => None,
    });
    let (cols, rows) = size.unwrap_or((80, 30));
    let mut next = update(vec![info(1), info(2)]);
    next.subscribed = vec![1];
    next.snapshots = vec![PaneSnapshot {
        pane: 1,
        revision: 1,
        screen: screen(cols, rows, "$ echo stream-tui"),
    }];
    c.event(Event::Update(next)).unwrap();
    let frame = c.frame();
    let lines = text(&frame);
    assert!(lines.iter().any(|l| l.contains("$ echo stream-tui")));
    assert!(lines.iter().any(|l| l.contains("tab 1")));
    assert!(lines.iter().any(|l| l.contains("tab 2")));
    assert!(frame.controller);
    assert!(frame.sidebar_status());
}

#[test]
fn renders_the_grid_with_a_header_per_pane() {
    let mut c = attached(&[1, 2, 3]);
    act(&mut c, Action::CloseReading);
    assert_eq!(c.workspace.reading, ReadingMode::Grid);
    let frame = c.frame();
    let views: Vec<_> = frame.panes.iter().filter(|v| v.rect.cols > 0).collect();
    assert_eq!(views.len(), 3);
    let lines = text(&frame);
    for id in 1..=3 {
        assert!(lines.iter().any(|l| l.contains(&format!("tab {id}"))));
    }
    // Every grid pane is subscribed for local rendering.
    c.take_requests();
    assert_eq!(c.subscribed(), &[1, 2, 3]);
}

#[test]
fn renders_dialogs_from_local_state() {
    let mut c = attached(&[1, 2]);
    act(&mut c, Action::Help);
    assert!(text(&c.frame()).iter().any(|l| l.contains("Detach")));
    act(&mut c, Action::Help);
    act(&mut c, Action::ParkRequest(1));
    act(&mut c, Action::Park(1));
    act(&mut c, Action::Parked);
    let lines = text(&c.frame());
    assert!(lines.iter().any(|l| l.contains("tab 1")));
    assert_eq!(
        c.workspace.ui.dialog.map(|d| d.kind),
        Some(DialogKind::Parked)
    );
    act(&mut c, Action::DialogClose);
    act(&mut c, Action::Config);
    assert!(text(&c.frame()).iter().any(|l| l.contains("Notifications")));
}

/// A private state directory shared by the TUIs of one test.
struct StateDir(std::path::PathBuf);
impl StateDir {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kelpmux-tui-sync-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        crate::journal::private_dir(&path).unwrap();
        Self(path)
    }
    /// A synced TUI on this directory's state with panes `ids`, its own
    /// changes written.
    fn attach(&self, ids: &[PaneId]) -> Controller {
        let mut store = Store::open(&self.0).unwrap();
        store.mark_migrated();
        let mut c = controller_from(store);
        c.event(Event::Update(update(
            ids.iter().map(|id| info(*id)).collect(),
        )))
        .unwrap();
        c.take_requests();
        c.exchange_state().unwrap();
        c
    }
}
impl Drop for StateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
/// Applies pending actions and lets `c` exchange state.
fn exchange(c: &mut Controller) {
    c.take_requests();
    c.exchange_state().unwrap();
}
fn pane(c: &Controller, id: PaneId) -> &PaneInfo {
    c.workspace.panes.iter().find(|p| p.id == id).unwrap()
}

#[test]
fn another_tuis_order_names_colours_and_parking_apply_live() {
    let dir = StateDir::new();
    let mut a = dir.attach(&[1, 2, 3, 4]);
    let mut b = dir.attach(&[1, 2, 3, 4]);
    exchange(&mut a);
    // B's transient state: focus history 1, 3, 2 and an open dialog.
    act(&mut b, Action::Select(1));
    act(&mut b, Action::Focus(3));
    act(&mut b, Action::Focus(2));
    act(&mut b, Action::Config);
    exchange(&mut b);
    b.dirty = false;

    act(
        &mut a,
        Action::Reorder {
            source: 4,
            before: Some(1),
        },
    );
    act(&mut a, Action::TabColorSet { pane: 1, color: 5 });
    let mut one = a.store.pane(1).cloned().unwrap_or_default();
    one.name = Some("Build".into());
    a.store.set_pane(1, one);
    act(&mut a, Action::Park(2));
    exchange(&mut a);

    exchange(&mut b);
    assert!(b.dirty, "a reload repaints");
    assert_eq!(order(&b), vec![4, 1, 2, 3]);
    assert_eq!(pane(&b, 1).title, "Build");
    assert_eq!(pane(&b, 1).palette, 5);
    assert!(pane(&b, 2).parked);
    // The parked tab was focused here: focus returns to the previous one.
    assert_eq!(b.workspace.active, 3);
    assert_eq!(b.workspace.reading, ReadingMode::Tabs(3));
    assert_eq!(
        b.workspace.ui.dialog.map(|d| d.kind),
        Some(DialogKind::Config)
    );
    // Applying it leaves nothing to write back.
    b.take_requests();
    assert!(b.store.changes.is_empty(), "{:?}", b.store.changes);
    // Restoring elsewhere puts the tab back where that TUI put it, without
    // moving focus here.
    act(&mut a, Action::Restore(2));
    exchange(&mut a);
    exchange(&mut b);
    assert_eq!(order(&b), vec![4, 1, 3, 2]);
    assert!(!pane(&b, 2).parked);
    assert_eq!(b.workspace.active, 3);
}

#[test]
fn a_tuis_own_writes_are_not_reapplied() {
    let dir = StateDir::new();
    let mut a = dir.attach(&[1, 2]);
    act(&mut a, Action::Park(2));
    exchange(&mut a);
    a.dirty = false;
    // Its own write: no reload, no repaint, nothing to write.
    exchange(&mut a);
    assert!(!a.dirty);
    assert!(!a.store.poll().unwrap());
    assert!(!a.store.take_reloaded());
    // Local transient state set after the write survives the exchange.
    act(&mut a, Action::Help);
    a.dirty = false;
    exchange(&mut a);
    assert!(a.workspace.help);
    assert!(!a.dirty);
}

#[test]
fn pending_local_changes_survive_another_tuis_write() {
    let dir = StateDir::new();
    let mut a = dir.attach(&[1, 2, 3]);
    let mut b = dir.attach(&[1, 2, 3]);
    exchange(&mut a);
    // B parks 3 and recolours 1; not written yet.
    act(&mut b, Action::Park(3));
    act(&mut b, Action::TabColorSet { pane: 1, color: 6 });
    b.take_requests();
    assert!(!b.store.changes.is_empty());
    // A reorders and recolours 1 and 2 meanwhile.
    act(
        &mut a,
        Action::Reorder {
            source: 2,
            before: Some(1),
        },
    );
    act(&mut a, Action::TabColorSet { pane: 1, color: 2 });
    act(&mut a, Action::TabColorSet { pane: 2, color: 4 });
    exchange(&mut a);
    // B's exchange writes its changes over A's for the same fields and
    // adopts the rest.
    exchange(&mut b);
    assert!(pane(&b, 3).parked);
    assert_eq!(pane(&b, 1).palette, 6);
    assert_eq!(pane(&b, 2).palette, 4);
    // Parking moves nothing, so B left the order alone: it is A's.
    assert_eq!(order(&b), vec![2, 1, 3]);
    let disk = super::state::load(&dir.0.join(super::state::FILE)).unwrap();
    let epoch = &disk.epochs["e1"];
    assert!(epoch.panes[&3].parked);
    assert_eq!(epoch.panes[&1].color, Some(6));
    assert_eq!(epoch.panes[&2].color, Some(4));
    assert_eq!(epoch.order, order(&b));
    // A converges on the merged state.
    exchange(&mut a);
    assert!(pane(&a, 3).parked);
    assert_eq!(pane(&a, 1).palette, 6);
    assert_eq!(order(&a), order(&b));
}

#[test]
fn settings_from_another_tui_wait_for_open_dialogs() {
    let dir = StateDir::new();
    let mut a = dir.attach(&[1]);
    let mut b = dir.attach(&[1]);
    exchange(&mut a);
    act(&mut b, Action::Config);
    exchange(&mut b);
    act(&mut a, Action::NotificationSoundToggle);
    exchange(&mut a);
    exchange(&mut b);
    // Not applied under the open dialog, and not written back either.
    assert!(!b.workspace.ui.config.notification_sound);
    exchange(&mut b);
    let path = dir.0.join(super::state::FILE);
    assert!(
        super::state::load(&path)
            .unwrap()
            .settings
            .unwrap()
            .notification_sound
    );
    act(&mut b, Action::DialogClose);
    exchange(&mut b);
    assert!(b.workspace.ui.config.notification_sound);
    // A local change made under the dialog wins over a deferred one.
    act(&mut b, Action::Config);
    act(&mut a, Action::NotificationSoundToggle);
    exchange(&mut a);
    exchange(&mut b);
    act(&mut b, Action::ToggleSidebarFold);
    act(&mut b, Action::DialogClose);
    exchange(&mut b);
    assert!(b.workspace.ui.config.notification_sound);
    assert!(b.workspace.ui.config.sidebar_folded);
}

#[test]
fn settings_changes_are_client_state() {
    let mut c = attached(&[1]);
    act(&mut c, Action::NotificationSoundToggle);
    assert!(
        c.store
            .document
            .settings
            .as_ref()
            .unwrap()
            .notification_sound
    );
    assert!(c.store.changes.settings);
}
