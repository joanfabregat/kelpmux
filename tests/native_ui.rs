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
            assert_eq!(ws.grid_layout(), original);
            for item in sizes.iter().skip(1) {
                assert!(ws.terminal_sizes().contains(item));
            }
            let cap = ws.layout()[0].rect;
            ws.apply(&Action::Fullscreen(1)).unwrap();
            assert!(ws.layout()[0].rect.cols >= cap.cols);
            ws.apply(&Action::Fullscreen(1)).unwrap();
            assert_eq!(ws.layout()[0].rect, cap);
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
fn tab_body_mouse_and_dialog_close_have_separate_hit_targets() {
    let mut ws = workspace((160, 50), (2, 2));
    ws.apply(&Action::Read(1)).unwrap();
    let mut f = frame(&ws);
    f.panes[0].modes.mouse_click = true;
    let rect = f.panes[0].rect;
    let mut router = Router::default();
    assert!(matches!(
        router
            .route(
                click(Point {
                    x: rect.x,
                    y: rect.y
                }),
                &f
            )
            .last(),
        Some(ClientMessage::Mouse(_))
    ));
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
    assert!(router.route(event, &f).is_empty());
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
#[test]
fn responsive_tabs_keep_all_panes_without_overlays_and_mouse_selects_tabs() {
    use kelpmux::native::{
        ui::TabPosition,
        workspace::{TabTarget, tab_targets},
    };
    for size in [(40, 10), (80, 30), (160, 50), (240, 70)] {
        for position in [
            TabPosition::Auto,
            TabPosition::Top,
            TabPosition::Left,
            TabPosition::Bottom,
        ] {
            let mut ws = workspace(size, (3, 3));
            ws.ui.config.tab_position = position;
            ws.apply(&Action::Select(9)).unwrap();
            let views = ws.layout();
            assert_eq!(views.len(), 9);
            assert!(views.iter().all(|v| !v.overlay));
            assert_eq!(views.iter().filter(|v| v.body.cols > 0).count(), 1);
            let body = views[8].body;
            assert!(body.cols >= 12 && body.rows > 0);
            assert!(body.x + body.cols <= size.0 && body.y + body.rows < size.1);
            assert_eq!(ws.terminal_sizes().len(), 9);
            let f = frame(&ws);
            let targets = tab_targets(f.cols, f.rows, &f.ui.config, 9, 8);
            let (_, rect) = targets
                .iter()
                .find(|(target, _)| *target == TabTarget::Pane(8))
                .unwrap();
            let mut router = Router::default();
            assert_eq!(
                actions(router.route(
                    click(Point {
                        x: rect.x,
                        y: rect.y
                    }),
                    &f
                )),
                vec![Action::Focus(9)]
            );
            let messages = router.route(
                click(Point {
                    x: rect.x + rect.cols - 1,
                    y: rect.y,
                }),
                &f,
            );
            assert_eq!(actions(messages), vec![Action::ClosePaneRequest(9)]);
            let mut term = Terminal::new(f.cols, f.rows);
            term.feed(&terminal_diff(None, &f));
            assert_eq!(term.snapshot().cells, f.cells);
        }
    }
    assert_eq!(TabPosition::Auto.resolve(40, 10), TabPosition::Bottom);
    assert_eq!(TabPosition::Auto.resolve(120, 40), TabPosition::Left);
    assert_eq!(TabPosition::Auto.resolve(240, 70), TabPosition::Top);
}
#[test]
fn config_position_choices_and_close_confirmation_require_explicit_consent() {
    use kelpmux::native::{ui::TabPosition, workspace::dialog_bounds};
    let mut ws = workspace((120, 40), (2, 2));
    ws.apply(&Action::Config).unwrap();
    let f = frame(&ws);
    let bounds = dialog_bounds(f.cols, f.rows);
    assert_eq!(
        actions(Router::default().route(
            click(Point {
                x: bounds.x + 1,
                y: bounds.y + 4
            }),
            &f
        )),
        vec![Action::TabPositionSet(TabPosition::Left)]
    );
    ws.apply(&Action::ClosePaneRequest(2)).unwrap();
    assert_eq!(ws.ui.dialog.unwrap().selection, 0);
    let f = frame(&ws);
    assert_eq!(
        actions(Router::default().route(InputEvent::Bytes(b"\r".to_vec()), &f)),
        vec![Action::DialogClose]
    );
    assert_eq!(
        actions(Router::default().route(InputEvent::Bytes(vec![27]), &f)),
        vec![Action::DialogClose]
    );
    ws.apply(&Action::DialogNavigate { dx: 0, dy: 1 }).unwrap();
    let f = frame(&ws);
    assert_eq!(
        actions(Router::default().route(InputEvent::Bytes(b"\r".to_vec()), &f)),
        vec![Action::ClosePaneConfirm(2), Action::DialogClose]
    );
}
#[test]
fn selecting_same_tab_does_not_toggle_back_to_grid_and_width_is_inline() {
    let mut ws = workspace((240, 70), (3, 3));
    ws.apply(&Action::Select(5)).unwrap();
    ws.apply(&Action::Select(5)).unwrap();
    assert_eq!(ws.reading, ReadingMode::Tabs(5));
    ws.apply(&Action::ReadResize { cols: 90, rows: 3 }).unwrap();
    let cap = ws.layout()[4].body;
    assert_eq!(cap.cols, 90);
    assert_eq!(cap.rows, 68);
    assert_eq!(cap.y, 1);
    assert_eq!(cap.x, 0);
    ws.apply(&Action::Fullscreen(5)).unwrap();
    assert_eq!(ws.layout()[4].body.cols, 240);
    ws.apply(&Action::Fullscreen(5)).unwrap();
    assert_eq!(ws.layout()[4].body, cap);
    ws.apply(&Action::Read(5)).unwrap();
    assert_eq!(ws.reading, ReadingMode::Grid);
}
#[test]
fn footer_buttons_and_compact_vitals_have_disjoint_hit_targets() {
    let mut ws = workspace((40, 10), (3, 3));
    ws.ui.vitals = vec![
        ("C0".into(), 114),
        ("M52".into(), 141),
        ("14:49".into(), 250),
    ];
    let f = frame(&ws);
    let footer = f.cells[usize::from(f.rows - 1) * usize::from(f.cols)..]
        .iter()
        .map(|c| c.text.as_str())
        .collect::<String>();
    assert!(footer.contains("14:49"));
    assert!(footer.contains("Layout"));
    assert_eq!(
        actions(Router::default().route(
            click(Point {
                x: f.cols - 26,
                y: f.rows - 1
            }),
            &f
        )),
        vec![Action::Config]
    );
    assert_eq!(
        actions(Router::default().route(
            click(Point {
                x: f.cols - 19,
                y: f.rows - 1
            }),
            &f
        )),
        vec![Action::Read(1)]
    );
    assert_eq!(
        actions(Router::default().route(
            click(Point {
                x: f.cols - 11,
                y: f.rows - 1
            }),
            &f
        )),
        vec![Action::Fullscreen(1)]
    );
    assert_eq!(
        actions(Router::default().route(
            click(Point {
                x: f.cols - 9,
                y: f.rows - 1
            }),
            &f
        )),
        vec![Action::GridPicker]
    );
}

#[test]
fn grid_separators_are_visible_and_outside_application_bodies() {
    for shape in GRIDS {
        let ws = workspace((160, 50), shape);
        let f = frame(&ws);
        let views = ws.grid_layout();
        for view in &views {
            if view.rect.x > 0 {
                let x = view.rect.x - 1;
                assert_eq!(f.cells[usize::from(view.body.y * f.cols + x)].text, "│");
                assert!(
                    views
                        .iter()
                        .all(|other| !other.body.contains(Point { x, y: view.body.y }))
                );
            }
            if view.rect.y > 0 {
                let y = view.rect.y - 1;
                assert_eq!(f.cells[usize::from(y * f.cols)].text, "─");
                assert!(
                    views
                        .iter()
                        .all(|other| !other.body.contains(Point { x: 0, y }))
                );
            }
        }
        if shape.0 > 1 && shape.1 > 1 {
            let x = views[1].rect.x - 1;
            let y = views[usize::from(shape.0)].rect.y - 1;
            assert_eq!(f.cells[usize::from(y * f.cols + x)].text, "┼");
        }
        let mut display = Terminal::new(f.cols, f.rows);
        display.feed(&terminal_diff(None, &f));
        assert_eq!(display.snapshot().cells, f.cells);
    }
}

#[test]
fn responsive_new_tabs_wrap_controls_and_graphic_confirmation_have_disjoint_targets() {
    use kelpmux::native::{
        ui::TabPosition,
        workspace::{TabTarget, close_buttons, dialog_bounds, tab_targets},
    };
    for size in [(40, 10), (100, 30), (240, 70)] {
        for position in [TabPosition::Left, TabPosition::Top, TabPosition::Bottom] {
            let mut ws = workspace(size, (2, 1));
            ws.ui.config.tab_position = position;
            ws.apply(&Action::Select(1)).unwrap();
            let f = frame(&ws);
            let targets = tab_targets(f.cols, f.rows, &f.ui.config, 2, 0);
            for (index, (_, rect)) in targets.iter().enumerate() {
                assert!(rect.x + rect.cols <= f.cols && rect.y + rect.rows < f.rows);
                for (_, other) in targets.iter().skip(index + 1) {
                    assert!(!rect.contains(Point {
                        x: other.x,
                        y: other.y
                    }));
                    assert!(!other.contains(Point {
                        x: rect.x,
                        y: rect.y
                    }));
                }
            }
            let new = targets
                .iter()
                .find(|(target, _)| *target == TabTarget::New)
                .unwrap()
                .1;
            assert_eq!(
                actions(Router::default().route(click(Point { x: new.x, y: new.y }), &f)),
                vec![Action::NewTab]
            );
            if position == TabPosition::Left {
                let wrap = targets
                    .iter()
                    .find(|(target, _)| *target == TabTarget::Wrap)
                    .unwrap()
                    .1;
                assert_eq!(
                    actions(Router::default().route(
                        click(Point {
                            x: wrap.x,
                            y: wrap.y
                        }),
                        &f
                    )),
                    vec![Action::WrapToggle]
                );
                if size.1 >= 30 {
                    assert_eq!(
                        targets
                            .iter()
                            .find(|(t, _)| *t == TabTarget::Pane(0))
                            .unwrap()
                            .1
                            .rows,
                        3
                    );
                }
            }
            let capped = ws.layout()[0].body;
            if capped.x + capped.cols < ws.cols {
                assert_eq!(
                    actions(Router::default().route(
                        click(Point {
                            x: capped.x + capped.cols,
                            y: capped.y
                        }),
                        &f
                    )),
                    vec![Action::WrapToggle]
                );
            }
            ws.apply(&Action::WrapToggle).unwrap();
            assert!(!ws.ui.config.wrap_lines);
            let full = ws.layout()[0].body;
            assert_eq!(full.x + full.cols, ws.cols);
            ws.apply(&Action::WrapToggle).unwrap();
            assert_eq!(ws.layout()[0].body, capped);

            ws.apply(&Action::Config).unwrap();
            let f = frame(&ws);
            let bounds = dialog_bounds(f.cols, f.rows);
            assert_eq!(
                actions(Router::default().route(
                    click(Point {
                        x: bounds.x + 1,
                        y: bounds.y + 7
                    }),
                    &f
                )),
                vec![Action::WrapToggle]
            );
            ws.apply(&Action::ClosePaneRequest(2)).unwrap();
            let f = frame(&ws);
            let buttons = close_buttons(bounds);
            for (index, button) in buttons.into_iter().enumerate() {
                let point = Point {
                    x: button.x,
                    y: button.y + button.rows - 1,
                };
                let actual = actions(Router::default().route(click(point), &f));
                let expected = if index == 0 {
                    vec![Action::DialogClose]
                } else {
                    vec![Action::ClosePaneConfirm(2), Action::DialogClose]
                };
                assert_eq!(actual, expected);
                assert_eq!(
                    f.cells[usize::from(point.y * f.cols + point.x)].bg,
                    Color::Indexed(if index == 0 { 27 } else { 88 })
                );
            }
        }
    }
}

#[test]
fn long_tab_titles_scroll_once_then_stop_without_moving_terminal_content() {
    use kelpmux::native::ui::TabPosition;
    let mut ws = workspace((100, 30), (2, 1));
    ws.ui.config.tab_position = TabPosition::Left;
    ws.panes[0].title = "long title 界 with its useful ending".into();
    ws.apply(&Action::Select(1)).unwrap();
    let initial = frame(&ws);
    ws.ui.title_ticks.insert(1, 48);
    let middle = frame(&ws);
    assert_ne!(initial.cells, middle.cells);
    assert_eq!(initial.panes, middle.panes);
    ws.ui.title_ticks.insert(1, 1000);
    let final_frame = frame(&ws);
    assert_eq!(initial.cells, final_frame.cells);
    let mut display = Terminal::new(100, 30);
    display.feed(&terminal_diff(None, &middle));
    assert_eq!(display.snapshot().cells, middle.cells);
}
