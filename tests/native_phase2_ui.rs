use kelpmux::{
    model::{Kind, Status},
    native::{
        engine::Terminal,
        input::Router,
        protocol::*,
        render::{compose, terminal_diff},
        tracking::RecoveryEntry,
        ui::DialogKind,
        workspace::{Workspace, config_targets, dialog_bounds_for},
    },
};
use std::collections::BTreeMap;
fn workspace(cols: u16, rows: u16) -> Workspace {
    let mut ws = Workspace::new(cols, rows).unwrap();
    ws.ui.notification_run = "test-run".into();
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
        origin: Some(NotificationOrigin {
            run: "test-run".into(),
            pid: 0,
        }),
    });
    let f = frame(&ws);
    assert_eq!(
        actions(Router::default().route(
            mouse(
                Point {
                    x: f.footer_cols() - 2,
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
fn viewport_reading_preferences_and_automatic_grid_preserve_all_ids() {
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
    assert_eq!(
        ws.layout()[0].body.cols,
        40 - kelpmux::native::workspace::sidebar_width(40)
    );
    ws.apply(&Action::ReadResize { cols: 30, rows: 3 }).unwrap();
    ws.resize(120, 40).unwrap();
    // Legacy resize requests cannot limit tabs after a viewport change.
    assert_eq!(
        ws.layout()[0].body.cols,
        ws.cols - kelpmux::native::workspace::sidebar_width(ws.cols)
    );
    ws.resize(140, 40).unwrap();
    assert_eq!(
        ws.layout()[0].body.cols,
        140 - kelpmux::native::workspace::sidebar_width(140)
    );
    ws.resize(240, 70).unwrap();
    assert_eq!(
        ws.layout()[0].body.cols,
        240 - kelpmux::native::workspace::sidebar_width(240)
    );
    ws.apply(&Action::CloseReading).unwrap();
    let ids = ws.panes.iter().map(|p| p.id).collect::<Vec<_>>();
    ws.apply(&Action::ResizeGrid {
        column: Some((0, 700)),
        row: Some((0, 600)),
    })
    .unwrap();
    let views = ws.layout();
    assert!(views[0].rect.cols.abs_diff(views[1].rect.cols) <= 1);
    assert!(views[0].rect.rows.abs_diff(views[2].rect.rows) <= 1);
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
    assert!(
        router
            .route(mouse(Point { x: 100, y: 2 }, MouseKind::Release), &f)
            .is_empty()
    );
    let separator = Point {
        x: 50,
        y: f.panes[2].rect.y - 1,
    };
    assert!(
        router
            .route(mouse(separator, MouseKind::Press), &f)
            .is_empty()
    );
    assert!(
        router
            .route(mouse(Point { x: 50, y: 20 }, MouseKind::Release), &f)
            .is_empty()
    );
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
    let bounds = dialog_bounds_for(f.cols, f.rows, DialogKind::Config);
    let choice = config_targets(bounds)
        .into_iter()
        .find(|(index, value, _)| *index == 0 && *value == Some(true))
        .unwrap()
        .2;
    assert_eq!(
        actions(Router::default().route(
            mouse(
                Point {
                    x: choice.x,
                    y: choice.y
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
    assert!(text.contains("Header drag"));
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

#[test]
fn dialogs_and_help_take_click_priority_above_scratch_on_compact_terminal() {
    let mut ws = workspace(40, 10);
    ws.panes.push(PaneInfo {
        id: 100,
        ..Default::default()
    });
    ws.ui.scratch = Some(100);
    ws.apply(&Action::Scratch).unwrap();
    ws.apply(&Action::Config).unwrap();
    let bounds = dialog_bounds_for(ws.cols, ws.rows, DialogKind::Config);
    let point = Point {
        x: bounds.x + bounds.cols - 1,
        y: bounds.y,
    };
    assert_eq!(
        actions(Router::default().route(mouse(point, MouseKind::Press), &frame(&ws))),
        vec![Action::DialogClose]
    );
    ws.apply(&Action::Help).unwrap();
    let help = kelpmux::native::input::help_bounds(ws.cols, ws.rows);
    let point = Point {
        x: help.x + help.cols - 1,
        y: help.y,
    };
    assert_eq!(
        actions(Router::default().route(mouse(point, MouseKind::Press), &frame(&ws))),
        vec![Action::Help]
    );
    assert!(ws.ui.scratch_visible);
}
#[test]
fn search_query_limit_is_utf8_safe_for_typing_and_paste() {
    let mut ws = workspace(160, 50);
    ws.apply(&Action::SearchOpen).unwrap();
    for event in [
        InputEvent::Bytes("界".repeat(100).into_bytes()),
        InputEvent::Paste("界".repeat(100).into_bytes()),
    ] {
        let messages = actions(Router::default().route(event, &frame(&ws)));
        assert_eq!(
            messages,
            vec![Action::Search {
                query: "界".repeat(85),
                all: false
            }]
        );
    }
}
#[test]
fn focusing_hidden_scratch_reveals_it_and_retains_underlying_tab() {
    let mut ws = workspace(160, 50);
    ws.apply(&Action::Select(2)).unwrap();
    ws.panes.push(PaneInfo {
        id: 100,
        ..Default::default()
    });
    ws.ui.scratch = Some(100);
    ws.apply(&Action::Focus(100)).unwrap();
    assert!(ws.ui.scratch_visible);
    assert_eq!(ws.active, 100);
    assert_eq!(ws.ui.scratch_return, Some(2));
    assert!(ws.layout().last().unwrap().body.cols > 0);
    ws.apply(&Action::ScratchClose).unwrap();
    assert_eq!(ws.reading, ReadingMode::Tabs(2));
}

#[test]
fn observer_can_take_over_and_detach_while_search_is_open() {
    let mut ws = workspace(160, 50);
    ws.apply(&Action::SearchOpen).unwrap();
    let mut f = frame(&ws);
    f.controller = false;
    let mut router = Router::default();
    assert_eq!(
        actions(router.route(InputEvent::Bytes(vec![2, b't']), &f)),
        vec![Action::Takeover]
    );
    assert_eq!(
        actions(router.route(InputEvent::Bytes(vec![2, b'd']), &f)),
        vec![Action::Detach]
    );
    assert!(
        router
            .route(InputEvent::Bytes(b"must-not-type".to_vec()), &f)
            .is_empty()
    );
    f.controller = true;
    assert_eq!(
        actions(router.route(InputEvent::Bytes(vec![2, b'd']), &f)),
        vec![Action::Detach]
    );
}
