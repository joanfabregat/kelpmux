use kelpmux::{
    model::{Kind, Status},
    native::{
        engine::Terminal,
        input::Router,
        protocol::*,
        render::{compose, terminal_diff},
        ui::AgentFilter,
        workspace::{GRIDS, Workspace, dialog_bounds, grid_cards},
    },
};
use std::collections::BTreeMap;
fn workspace(size: (u16, u16), shape: (u8, u8)) -> Workspace {
    let mut ws = Workspace::new(size.0, size.1).unwrap();
    ws.panes = (1..=u64::from(shape.0) * u64::from(shape.1))
        .map(|id| PaneInfo {
            id,
            number: id as usize,
            title: format!("agent 界 {id}"),
            palette: id as u8,
            ..Default::default()
        })
        .collect();
    ws.active = 1;
    ws.apply(&Action::Grid {
        cols: shape.0,
        rows: shape.1,
    })
    .unwrap();
    ws
}
fn frame(ws: &Workspace) -> Frame {
    compose(ws, &BTreeMap::new(), true, None, 0)
}
fn actions(messages: Vec<ClientMessage>) -> Vec<Action> {
    messages
        .into_iter()
        .map(|m| match m {
            ClientMessage::Action(action) => action,
            _ => panic!("input leaked into application"),
        })
        .collect()
}
fn click(point: Point) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        position: point,
        kind: MouseKind::Press,
        button: 0,
        shift: false,
        alt: false,
        ctrl: false,
    })
}
#[test]
fn six_grids_fit_device_profiles_and_preserve_other_terminals() {
    for size in [(40, 10), (80, 30), (160, 50), (240, 70)] {
        for shape in GRIDS {
            let mut ws = workspace(size, shape);
            let original = ws.layout();
            let sizes = ws.terminal_sizes();
            for view in &original {
                assert!(view.body.cols > 0 && view.body.rows > 0);
                assert!(view.rect.x + view.rect.cols <= size.0);
                assert!(view.rect.y + view.rect.rows < size.1);
            }
            ws.apply(&Action::Read(1)).unwrap();
            ws.apply(&Action::ReadResize { cols: 85, rows: 21 })
                .unwrap();
            assert_eq!(&ws.layout()[..original.len()], original);
            for item in sizes.iter().skip(1) {
                assert!(ws.terminal_sizes().contains(item));
            }
            let cap = ws.layout().last().unwrap().rect;
            ws.apply(&Action::Fullscreen(1)).unwrap();
            assert_eq!(ws.layout().last().unwrap().rect.cols, size.0);
            ws.apply(&Action::Fullscreen(1)).unwrap();
            assert_eq!(ws.layout().last().unwrap().rect, cap);
        }
    }
}
#[test]
fn picker_cards_disable_occupied_shapes_and_never_leak_input() {
    let mut ws = workspace((160, 50), (3, 3));
    ws.ui.unused = vec![7, 8, 9];
    ws.apply(&Action::GridPicker).unwrap();
    let f = frame(&ws);
    let cards = grid_cards(dialog_bounds(f.cols, f.rows));
    assert_eq!(cards[0].y, cards[2].y);
    assert_eq!(cards[3].y, cards[5].y);
    assert!(cards[3].y > cards[0].y);
    let mut router = Router::default();
    assert!(
        router
            .route(
                click(Point {
                    x: cards[0].x + 1,
                    y: cards[0].y + 1
                }),
                &f
            )
            .is_empty()
    );
    assert_eq!(
        actions(router.route(
            click(Point {
                x: cards[3].x + 1,
                y: cards[3].y + 1
            }),
            &f
        )),
        vec![Action::Grid { cols: 3, rows: 2 }, Action::DialogClose]
    );
    assert!(
        router
            .route(InputEvent::Paste(b"never shell".to_vec()), &f)
            .is_empty()
    );
    assert!(
        router
            .route(InputEvent::Bytes(b"garbage".to_vec()), &f)
            .is_empty()
    );
    let mut router = Router::default();
    let base = frame(&workspace((160, 50), (2, 2)));
    assert_eq!(
        actions(router.route(InputEvent::Bytes(b"\x02ghello".to_vec()), &base)),
        vec![Action::GridPicker]
    );
    assert!(
        router
            .route(
                InputEvent::Paste(b"no leak while waiting for frame".to_vec()),
                &base
            )
            .is_empty()
    );
}
#[test]
fn custom_prefix_preserves_alt_editing_and_observers() {
    let mut ws = workspace((160, 50), (2, 2));
    ws.ui.config.prefix = 1;
    ws.ui.config.bindings.insert("x".into(), "grid".into());
    let f = frame(&ws);
    let mut router = Router::default();
    assert!(
        matches!(&router.route(InputEvent::Bytes(b"\x1bf\x1bb\x1b[1;3D".to_vec()),&f)[..],[ClientMessage::Input(bytes)] if bytes==b"\x1bf\x1bb\x1b[1;3D")
    );
    assert_eq!(
        actions(router.route(InputEvent::Bytes(b"\x01x".to_vec()), &f)),
        vec![Action::GridPicker]
    );
    ws.apply(&Action::FocusDirection { dx: 1, dy: 0 }).unwrap();
    assert_eq!(ws.active, 2);
    ws.apply(&Action::FocusDirection { dx: 0, dy: 1 }).unwrap();
    assert_eq!(ws.active, 4);
    ws.apply(&Action::FocusDirection { dx: 1, dy: 0 }).unwrap();
    assert_eq!(ws.active, 4);
    let mut observer = f;
    observer.controller = false;
    assert!(
        Router::default()
            .route(InputEvent::Bytes(b"\x01x".to_vec()), &observer)
            .is_empty()
    );
}
#[test]
fn agent_filters_and_move_keep_identity() {
    let mut ws = workspace((160, 50), (2, 2));
    for pane in &mut ws.panes {
        pane.kind = Some(Kind::Codex);
    }
    ws.panes[0].status = Status::Working;
    ws.panes[1].status = Status::InputNeeded;
    ws.panes[2].status = Status::TurnFinished;
    ws.panes[3].status = Status::Failed;
    ws.apply(&Action::Agents(AgentFilter::Attention)).unwrap();
    assert_eq!(ws.dialog_entries(), vec![2, 4]);
    ws.apply(&Action::Agents(AgentFilter::Finished)).unwrap();
    assert_eq!(ws.dialog_entries(), vec![3]);
    assert_eq!(
        actions(Router::default().route(InputEvent::Bytes(b"\r".to_vec()), &frame(&ws))),
        vec![Action::Focus(3), Action::DialogClose]
    );
    ws.apply(&Action::MovePicker).unwrap();
    let emitted = actions(Router::default().route(InputEvent::Bytes(b"4".to_vec()), &frame(&ws)));
    assert_eq!(
        emitted,
        vec![
            Action::Swap {
                source: 1,
                target: 4
            },
            Action::DialogClose
        ]
    );
    for action in emitted {
        ws.apply(&action).unwrap();
    }
    assert_eq!(ws.panes[3].id, 1);
    assert_eq!(ws.panes[3].palette, 1);
}
#[test]
fn full_card_highlight_and_unicode_roundtrip() {
    let mut ws = workspace((160, 50), (2, 2));
    ws.ui.unused = vec![1, 2, 3, 4];
    ws.apply(&Action::GridPicker).unwrap();
    ws.apply(&Action::DialogPick(3)).unwrap();
    let f = frame(&ws);
    let card = grid_cards(dialog_bounds(f.cols, f.rows))[3];
    for y in card.y..card.y + card.rows {
        for x in card.x..card.x + card.cols {
            assert_eq!(
                f.cells[usize::from(y) * usize::from(f.cols) + usize::from(x)].bg,
                Color::Indexed(24)
            );
        }
    }
    let mut terminal = Terminal::new(f.cols, f.rows);
    terminal.feed(&terminal_diff(None, &f));
    assert_eq!(terminal.snapshot().cells, f.cells);
}
#[test]
fn reading_grip_consumes_mouse_and_help_dialog_close_do_not_leak() {
    let mut ws = workspace((160, 50), (2, 2));
    ws.apply(&Action::Read(1)).unwrap();
    let mut f = frame(&ws);
    f.panes.last_mut().unwrap().modes.mouse_click = true;
    let rect = f.panes.last().unwrap().rect;
    let mut router = Router::default();
    assert!(
        router
            .route(
                click(Point {
                    x: rect.x + rect.cols - 1,
                    y: rect.y + rect.rows - 1
                }),
                &f
            )
            .is_empty()
    );
    let event = InputEvent::Mouse(MouseEvent {
        position: Point {
            x: rect.x + rect.cols + 4,
            y: rect.y + rect.rows + 1,
        },
        kind: MouseKind::Move,
        button: 0,
        shift: false,
        alt: false,
        ctrl: false,
    });
    assert!(matches!(
        &actions(router.route(event, &f))[..],
        [Action::ReadResize { .. }]
    ));
    ws.apply(&Action::GridPicker).unwrap();
    let f = frame(&ws);
    let bounds = dialog_bounds(f.cols, f.rows);
    assert_eq!(
        actions(Router::default().route(
            click(Point {
                x: bounds.x + bounds.cols - 1,
                y: bounds.y
            }),
            &f
        )),
        vec![Action::DialogClose]
    );
    assert_eq!(
        actions(Router::default().route(click(Point { x: 0, y: 0 }), &f)),
        vec![Action::DialogClose]
    );
}
#[test]
fn selected_number_and_title_have_distinct_colors_and_agent_shortcuts_use_pane_numbers() {
    let mut ws = workspace((160, 50), (2, 2));
    let f = frame(&ws);
    assert_ne!(f.cells[3].fg, f.cells[6].fg);
    for pane in &mut ws.panes {
        pane.kind = Some(Kind::Claude);
    }
    ws.panes[3].status = Status::InputNeeded;
    ws.apply(&Action::Agents(AgentFilter::Attention)).unwrap();
    assert_eq!(
        actions(Router::default().route(InputEvent::Bytes(b"4".to_vec()), &frame(&ws))),
        vec![Action::Focus(4), Action::DialogClose]
    );
}
#[test]
fn agent_picker_scrolls_to_selection_on_small_tablet() {
    let mut ws = workspace((40, 10), (3, 3));
    for pane in &mut ws.panes {
        pane.kind = Some(Kind::Codex);
    }
    ws.apply(&Action::Agents(AgentFilter::All)).unwrap();
    ws.apply(&Action::DialogPick(8)).unwrap();
    let f = frame(&ws);
    let bounds = dialog_bounds(f.cols, f.rows);
    let row = bounds.y + bounds.rows - 1;
    let text = f.cells
        [usize::from(row) * usize::from(f.cols)..usize::from(row + 1) * usize::from(f.cols)]
        .iter()
        .map(|c| c.text.as_str())
        .collect::<String>();
    assert!(text.contains('9'));
    assert_eq!(
        actions(Router::default().route(
            click(Point {
                x: bounds.x + 1,
                y: row
            }),
            &f
        )),
        vec![Action::Focus(9), Action::DialogClose]
    );
}
