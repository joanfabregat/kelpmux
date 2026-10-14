use kelpmux::{
    model::{Kind, Status},
    native::{
        engine::Terminal,
        input::Router,
        protocol::*,
        render::{compose, terminal_diff},
        tracking::RecoveryEntry,
        ui::DialogKind,
        workspace::{Workspace, dialog_bounds},
    },
};
use std::collections::BTreeMap;
fn workspace(cols: u16, rows: u16) -> Workspace {
    let mut ws = Workspace::new(cols, rows).unwrap();
    ws.panes = (1..=4)
        .map(|id| PaneInfo {
            id,
            number: id as usize,
            title: format!("task {id}"),
            cwd: "/project".into(),
            kind: Some(Kind::Codex),
            session_id: Some(format!("session-{id}")),
            ..Default::default()
        })
        .collect();
    ws.active = 1;
    ws.apply(&Action::Grid { cols: 2, rows: 2 }).unwrap();
    ws
}
fn frame(ws: &Workspace) -> Frame {
    compose(ws, &BTreeMap::new(), true, None, 0)
}
fn actions(messages: Vec<ClientMessage>) -> Vec<Action> {
    messages
        .into_iter()
        .map(|message| match message {
            ClientMessage::Action(action) => action,
            _ => panic!("application input leaked"),
        })
        .collect()
}
fn mouse(point: Point, kind: MouseKind) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        position: point,
        kind,
        button: 0,
        shift: false,
        alt: false,
        ctrl: false,
    })
}
#[test]
fn scratch_retains_identity_and_restores_tabs_without_becoming_a_grid_pane() {
    for (cols, rows) in [(40, 10), (80, 30), (160, 50), (240, 70)] {
        let mut ws = workspace(cols, rows);
        ws.apply(&Action::Select(2)).unwrap();
        let original = ws.layout();
        ws.panes.push(PaneInfo {
            id: 100,
            title: "scratch".into(),
            ..Default::default()
        });
        ws.ui.scratch = Some(100);
        ws.apply(&Action::Scratch).unwrap();
        assert_eq!(ws.active, 100);
        assert_eq!(ws.ui.scratch_return, Some(2));
        let views = ws.layout();
        assert_eq!(&views[..4], &original);
        assert!(views[4].overlay);
        assert_eq!(views[4].pane.number, 0);
        assert_eq!(ws.terminal_sizes().len(), 5);
        let f = frame(&ws);
        let scratch = &f.panes[4];
        assert_eq!(
            actions(Router::default().route(
                mouse(
                    Point {
                        x: scratch.rect.x + scratch.rect.cols - 1,
                        y: scratch.rect.y
                    },
                    MouseKind::Press
                ),
                &f
            )),
            vec![Action::ScratchClose]
        );
        ws.apply(&Action::ScratchClose).unwrap();
        assert_eq!(ws.active, 2);
        assert_eq!(ws.reading, ReadingMode::Tabs(2));
        assert_eq!(ws.panes.len(), 5);
        assert_eq!(ws.layout()[4].rect.cols, 0);
        ws.apply(&Action::Scratch).unwrap();
        assert_eq!(ws.active, 100);
        assert!(
            ws.apply(&Action::Swap {
                source: 100,
                target: 1
            })
            .is_err()
        );
    }
}
#[test]
fn search_edits_fragmented_unicode_scope_and_results_without_input_leaks() {
    let mut ws = workspace(160, 50);
    ws.apply(&Action::SearchOpen).unwrap();
    let f = frame(&ws);
    let mut router = Router::default();
    let bytes = "界".as_bytes();
    assert!(
        router
            .route(InputEvent::Bytes(bytes[..1].to_vec()), &f)
            .is_empty()
    );
    let emitted = actions(router.route(InputEvent::Bytes(bytes[1..].to_vec()), &f));
    assert_eq!(
        emitted,
        vec![Action::Search {
            query: "界".into(),
            all: false
        }]
    );
    assert_eq!(
        actions(router.route(InputEvent::Bytes(vec![9]), &f)),
        vec![Action::Search {
            query: "界".into(),
            all: true
        }]
    );
    assert_eq!(
        actions(router.route(InputEvent::Bytes(vec![127]), &f)),
        vec![Action::Search {
            query: String::new(),
            all: true
        }]
    );
    ws.ui.search_hits = vec![SearchHit {
        pane: 3,
        line: -8,
        text: "matched line".into(),
    }];
    ws.ui.search_all = true;
    let f = frame(&ws);
    assert_eq!(
        actions(router.route(InputEvent::Bytes(vec![25]), &f)),
        vec![Action::SearchCopy(0)]
    );
    assert_eq!(
        actions(router.route(InputEvent::Bytes(b"\r".to_vec()), &f)),
        vec![Action::SearchSelect(0), Action::DialogClose]
    );
    assert_eq!(
        actions(router.route(InputEvent::Bytes(vec![27]), &f)),
        vec![Action::DialogClose]
    );
}
#[test]
fn notification_history_acknowledges_and_jumps_and_next_agent_navigation_wraps() {
    let mut ws = workspace(160, 50);
    ws.panes[1].status = Status::InputNeeded;
    ws.panes[3].status = Status::TurnFinished;
    ws.apply(&Action::NextAttention).unwrap();
    assert_eq!(ws.active, 2);
    ws.apply(&Action::NextFinished).unwrap();
    assert_eq!(ws.active, 4);
    ws.ui.notifications.push(Notification {
        id: 7,
        pane: 2,
        title: "question".into(),
        status: Status::InputNeeded,
        at: 1,
        read: false,
        workspace: "main".into(),
    });
    let f = frame(&ws);
    assert_eq!(
        actions(Router::default().route(
            mouse(
                Point {
                    x: f.cols - 2,
                    y: f.rows - 1
                },
                MouseKind::Press
            ),
            &f
        )),
        vec![Action::Notifications]
    );
    ws.apply(&Action::Notifications).unwrap();
    let f = frame(&ws);
    assert_eq!(
        actions(Router::default().route(InputEvent::Bytes(b"\r".to_vec()), &f)),
        vec![
            Action::Acknowledge(7),
            Action::Focus(2),
            Action::DialogClose
        ]
    );
    ws.apply(&Action::Acknowledge(7)).unwrap();
    assert!(ws.ui.notifications[0].read);
}
#[test]
fn viewport_reading_preferences_and_grid_drag_preserve_all_ids() {
    let mut ws = workspace(120, 40);
    ws.apply(&Action::Read(1)).unwrap();
    ws.apply(&Action::ReadResize { cols: 85, rows: 20 })
        .unwrap();
    ws.resize(240, 70).unwrap();
    ws.apply(&Action::ReadResize {
        cols: 140,
        rows: 20,
    })
    .unwrap();
    ws.resize(40, 10).unwrap();
    assert_eq!(ws.layout()[0].body.cols, 40);
    ws.apply(&Action::ReadResize { cols: 30, rows: 3 }).unwrap();
    ws.resize(120, 40).unwrap();
    assert_eq!(ws.layout()[0].body.cols, 85);
    ws.resize(240, 70).unwrap();
    assert_eq!(ws.layout()[0].body.cols, 140);
    ws.apply(&Action::CloseReading).unwrap();
    let ids = ws.panes.iter().map(|p| p.id).collect::<Vec<_>>();
    ws.apply(&Action::ResizeGrid {
        column: Some((0, 700)),
        row: Some((0, 600)),
    })
    .unwrap();
    let views = ws.layout();
    assert!(views[0].rect.cols > views[1].rect.cols);
    assert!(views[0].rect.rows > views[2].rect.rows);
    assert_eq!(ids, ws.panes.iter().map(|p| p.id).collect::<Vec<_>>());
    let f = frame(&ws);
    let mut router = Router::default();
    let separator = Point {
        x: f.panes[1].rect.x - 1,
        y: 2,
    };
    assert!(
        router
            .route(mouse(separator, MouseKind::Press), &f)
            .is_empty()
    );
    assert!(matches!(
        actions(router.route(mouse(Point { x: 100, y: 2 }, MouseKind::Release), &f)).as_slice(),
        [Action::ResizeGrid {
            column: Some(_),
            row: None
        }]
    ));
}
#[test]
fn header_drag_swap_and_keyboard_selection_are_owned_by_the_multiplexer() {
    let ws = workspace(160, 50);
    let f = frame(&ws);
    let mut router = Router::default();
    assert_eq!(
        actions(router.route(mouse(Point { x: 8, y: 0 }, MouseKind::Press), &f)),
        vec![Action::Focus(1)]
    );
    let target = f.panes[3].rect;
    assert_eq!(
        actions(router.route(
            mouse(
                Point {
                    x: target.x + 8,
                    y: target.y
                },
                MouseKind::Release
            ),
            &f
        )),
        vec![Action::Swap {
            source: 1,
            target: 4
        }]
    );
    let mut router = Router::default();
    assert_eq!(
        actions(router.route(InputEvent::Bytes(b"\x02 ".to_vec()), &f)),
        vec![Action::SelectionStart]
    );
    assert_eq!(
        actions(router.route(InputEvent::Bytes(b"\x1b[C".to_vec()), &f)),
        vec![Action::SelectionMove { dx: 1, dy: 0 }]
    );
    assert_eq!(
        actions(router.route(InputEvent::Bytes(b"y".to_vec()), &f)),
        vec![Action::Copy(1)]
    );
}
#[test]
fn recovery_and_sound_settings_are_actionable_and_small_screen_help_scrolls() {
    let mut ws = workspace(80, 30);
    ws.ui.recovery.push(RecoveryItem {
        workspace: "main".into(),
        entry: RecoveryEntry {
            kind: Kind::Codex,
            name: "task".into(),
            cwd: "/project".into(),
            session_id: Some("abc".into()),
            pane: "%1".into(),
            pid: 1,
            process_start: 1,
            source: "test".into(),
            first_seen: 1,
            last_seen: 2,
            ended_at: None,
            status: Status::Working,
            resume_command: Some("codex resume abc".into()),
        },
    });
    ws.apply(&Action::Recovery).unwrap();
    assert_eq!(
        actions(Router::default().route(InputEvent::Bytes(b"y".to_vec()), &frame(&ws))),
        vec![Action::RecoveryCopy(0)]
    );
    ws.apply(&Action::Config).unwrap();
    let f = frame(&ws);
    let bounds = dialog_bounds(f.cols, f.rows);
    assert_eq!(
        actions(Router::default().route(
            mouse(
                Point {
                    x: bounds.x + 1,
                    y: bounds.y + 6
                },
                MouseKind::Press
            ),
            &f
        )),
        vec![Action::NotificationSoundToggle]
    );
    ws.apply(&Action::NotificationSoundToggle).unwrap();
    assert!(ws.ui.config.notification_sound);
    ws.resize(40, 10).unwrap();
    ws.apply(&Action::Help).unwrap();
    ws.apply(&Action::HelpScroll(100)).unwrap();
    assert!(ws.ui.help_offset > 0);
    let f = frame(&ws);
    let text = f.cells.iter().map(|c| c.text.as_str()).collect::<String>();
    assert!(text.contains("Resize grid"));
}
#[test]
fn all_dashboards_and_scratch_compose_unicode_colors_roundtrip() {
    for size in [(40, 10), (80, 30), (160, 50), (240, 70)] {
        let mut ws = workspace(size.0, size.1);
        for action in [
            Action::Overview,
            Action::Notifications,
            Action::Recovery,
            Action::SearchOpen,
            Action::Config,
        ] {
            ws.apply(&action).unwrap();
            let f = frame(&ws);
            let mut terminal = Terminal::new(f.cols, f.rows);
            terminal.feed(&terminal_diff(None, &f));
            assert_eq!(terminal.snapshot().cells, f.cells);
            assert!(
                !Router::default()
                    .route(InputEvent::Paste(b"app leak".to_vec()), &f)
                    .iter()
                    .any(|m| matches!(m, ClientMessage::Input(_) | ClientMessage::Paste(_)))
            );
        }
        assert_eq!(ws.ui.dialog.unwrap().kind, DialogKind::Config);
    }
}
#[test]
fn scratch_is_drawn_above_sidebar_and_preserves_truecolor_wide_terminal_cells() {
    for size in [(40, 10), (120, 40), (240, 70)] {
        let mut ws = workspace(size.0, size.1);
        ws.apply(&Action::Select(2)).unwrap();
        ws.panes.push(PaneInfo {
            id: 100,
            title: "scratch".into(),
            ..Default::default()
        });
        ws.ui.scratch = Some(100);
        ws.apply(&Action::Scratch).unwrap();
        let screens = ws
            .terminal_sizes()
            .into_iter()
            .map(|(id, cols, rows)| {
                let mut terminal = Terminal::new(cols, rows);
                terminal.feed(format!("\x1b[38;2;12;34;56;48;5;{id}m界 scratch{id}").as_bytes());
                (id, terminal.snapshot())
            })
            .collect();
        let f = compose(&ws, &screens, true, None, 0);
        let scratch = f.panes.iter().find(|view| view.pane.id == 100).unwrap();
        let cell = &f.cells
            [usize::from(scratch.body.y) * usize::from(f.cols) + usize::from(scratch.body.x)];
        assert_eq!(cell.fg, Color::Rgb(12, 34, 56));
        assert_eq!(cell.bg, Color::Indexed(100));
        let mut outer = Terminal::new(f.cols, f.rows);
        outer.feed(&terminal_diff(None, &f));
        assert_eq!(outer.snapshot().cells, f.cells);
    }
}
#[test]
fn search_submission_in_same_packet_orders_search_before_navigation() {
    let mut ws = workspace(160, 50);
    ws.apply(&Action::SearchOpen).unwrap();
    let f = frame(&ws);
    assert_eq!(
        actions(Router::default().route(InputEvent::Bytes(b"needle\r".to_vec()), &f)),
        vec![
            Action::Search {
                query: "needle".into(),
                all: false
            },
            Action::SearchSelect(0),
            Action::DialogClose
        ]
    );
}
#[test]
fn invalid_grid_resize_is_transactional_and_scratch_does_not_disable_empty_grid_choices() {
    let mut ws = workspace(160, 50);
    ws.apply(&Action::ResizeGrid {
        column: Some((0, 650)),
        row: None,
    })
    .unwrap();
    let before = ws.ui.column_weights.clone();
    assert!(
        ws.apply(&Action::ResizeGrid {
            column: Some((0, 750)),
            row: Some((8, 900))
        })
        .is_err()
    );
    assert_eq!(ws.ui.column_weights, before);
    ws.panes.push(PaneInfo {
        id: 100,
        title: "scratch".into(),
        ..Default::default()
    });
    ws.ui.scratch = Some(100);
    ws.ui.unused = vec![1, 2, 3, 4];
    ws.apply(&Action::GridPicker).unwrap();
    assert_eq!(
        actions(Router::default().route(InputEvent::Bytes(b"1".to_vec()), &frame(&ws))),
        vec![Action::Grid { cols: 1, rows: 1 }, Action::DialogClose]
    );
}
