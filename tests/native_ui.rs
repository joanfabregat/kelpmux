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
fn enhanced_enter_prefix_and_releases_route_without_leaking_mux_commands() {
    let mut ws = workspace((120, 40), (1, 1));
    ws.apply(&Action::Select(1)).unwrap();
    let mut f = frame(&ws);
    for wire in [b"\x1b[13;2u".as_slice(), b"\x1b[27;2;13~".as_slice()] {
        let mut decoder = kelpmux::native::input::Decoder::default();
        let mut router = Router::default();
        let output: Vec<_> = wire
            .iter()
            .flat_map(|b| decoder.feed(&[*b]))
            .flat_map(|e| router.route(e, &f))
            .collect();
        assert!(matches!(&output[..], [ClientMessage::Input(bytes)] if bytes == b"\n"));
    }
    f.panes[0].modes.keyboard_flags = 3;
    let mut router = Router::default();
    assert!(
        matches!(&router.route(InputEvent::Bytes(b"\x1b[13;2u".to_vec()), &f)[..], [ClientMessage::Input(bytes)] if bytes == b"\x1b[13;2u")
    );
    assert_eq!(
        actions(router.route(
            InputEvent::Bytes(b"\x1b[98;5u\x1b[98;5:3u\x1b[102u\x1b[102;1:3u".to_vec()),
            &f
        )),
        vec![Action::Fullscreen(1)]
    );
    assert!(
        matches!(&router.route(InputEvent::Bytes(b"\x1b[13u".to_vec()), &f)[..], [ClientMessage::Input(bytes)] if bytes == b"\x1b[13u")
    );
    let paste = b"\x1b[13;2u\x02f".to_vec();
    assert!(
        matches!(&router.route(InputEvent::Paste(paste.clone()), &f)[..], [ClientMessage::Paste(bytes)] if bytes == &paste)
    );
}

#[test]
fn sidebar_new_follows_tabs_and_parked_stays_at_bottom_with_contextual_headers() {
    use kelpmux::native::{
        input::help_bounds,
        ui::{DialogKind, PALETTE, TabPosition},
        workspace::{TabTarget, close_buttons, dialog_bounds_for, tab_targets},
    };
    let mut ws = workspace((120, 40), (2, 1));
    ws.ui.config.tab_position = TabPosition::Left;
    ws.apply(&Action::Select(1)).unwrap();
    let f = frame(&ws);
    let targets = tab_targets(f.cols, f.rows, &f.ui.config, 2, 0);
    let new = targets
        .iter()
        .find(|(t, _)| *t == TabTarget::New)
        .unwrap()
        .1;
    let parked = targets
        .iter()
        .find(|(t, _)| *t == TabTarget::Parked)
        .unwrap()
        .1;
    assert_eq!((new.rows, parked.rows), (3, 1));
    assert_eq!(parked.y + parked.rows, f.rows);
    let last_tab = targets
        .iter()
        .filter_map(|(target, rect)| {
            matches!(target, TabTarget::Pane(_)).then_some(rect.y + rect.rows)
        })
        .max()
        .unwrap();
    assert_eq!(new.y, last_tab + 1);
    assert!(new.y + new.rows < parked.y);
    assert!(
        !PALETTE
            .iter()
            .any(|(_, muted)| f.cells[usize::from(new.y * f.cols)].bg == Color::Indexed(*muted))
    );
    for row in 0..3 {
        assert_eq!(
            actions(Router::default().route(
                click(Point {
                    x: new.x,
                    y: new.y + row
                }),
                &f
            )),
            vec![Action::NewTab]
        );
        assert_eq!(
            actions(Router::default().route(
                click(Point {
                    x: parked.x,
                    y: parked.y
                }),
                &f
            )),
            vec![Action::Parked]
        );
    }
    let config_bg = f.cells[usize::from((f.rows - 1) * f.cols + 1)].bg;
    ws.apply(&Action::Config).unwrap();
    let config = frame(&ws);
    let bounds = dialog_bounds_for(f.cols, f.rows, DialogKind::Config);
    assert_eq!(
        config.cells[usize::from(bounds.y * f.cols + bounds.x)].bg,
        config_bg
    );
    ws.ui.dialog = None;
    ws.help = true;
    let help = frame(&ws);
    let bounds = help_bounds(f.cols, f.rows);
    assert_eq!(
        help.cells[usize::from(bounds.y * f.cols + bounds.x)].bg,
        f.cells[usize::from((f.rows - 1) * f.cols + 15)].bg
    );
    ws.help = false;
    for action in [Action::ClosePaneRequest(2), Action::ParkRequest(2)] {
        ws.apply(&action).unwrap();
        let kind = ws.ui.dialog.unwrap().kind;
        let bounds = dialog_bounds_for(f.cols, f.rows, kind);
        assert_eq!(close_buttons(bounds)[0].rows, 3);
        let popup = frame(&ws);
        assert_eq!(
            popup.cells[usize::from(bounds.y * f.cols + bounds.x)].bg,
            Color::Indexed(PALETTE[2].1)
        );
    }
}
#[test]
fn help_table_scrolls_to_last_action_and_keeps_columns_aligned() {
    use kelpmux::native::{input::help_bounds, ui::HELP_ROWS};
    for size in [(40, 10), (80, 24), (240, 70)] {
        let mut ws = workspace(size, (1, 1));
        ws.help = true;
        let bounds = help_bounds(ws.cols, ws.rows);
        let f = frame(&ws);
        let text: String = f.cells.iter().map(|c| c.text.as_str()).collect();
        assert!(text.contains("Key"));
        assert!(text.contains("Action"));
        let divider = bounds.x + (bounds.cols / 3).clamp(10, 22) + 1;
        for row in 2..bounds.rows - 1 {
            assert_eq!(
                f.cells[usize::from((bounds.y + row) * f.cols + divider)].text,
                "│"
            );
        }
        ws.apply(&Action::HelpScroll(i16::MAX)).unwrap();
        assert_eq!(ws.ui.help_offset + bounds.rows - 3, HELP_ROWS);
        let f = frame(&ws);
        let text: String = f.cells.iter().map(|c| c.text.as_str()).collect();
        assert!(text.contains("Header drag"));
        let mut terminal = Terminal::new(ws.cols, ws.rows);
        terminal.feed(&terminal_diff(None, &f));
        assert_eq!(terminal.snapshot().cells, f.cells);
    }
}

#[test]
fn config_buttons_use_all_three_rows_and_spacing_does_not_change_settings() {
    use kelpmux::native::{
        ui::{DialogKind, TabPosition},
        workspace::{config_targets, dialog_bounds_for},
    };
    for size in [(40, 10), (80, 18), (120, 40), (240, 70)] {
        let mut ws = workspace(size, (1, 1));
        ws.apply(&Action::Config).unwrap();
        let f = frame(&ws);
        let bounds = dialog_bounds_for(ws.cols, ws.rows, DialogKind::Config);
        let targets = config_targets(bounds);
        for (_, _, rect) in &targets {
            assert!(rect.x >= bounds.x && rect.x + rect.cols <= bounds.x + bounds.cols);
            assert!(rect.y > bounds.y && rect.y + rect.rows < bounds.y + bounds.rows);
            if size.1 >= 20 {
                assert_eq!(rect.rows, 3);
            }
        }
        for pair in targets[..4].windows(2) {
            let left = pair[0].2;
            let right = pair[1].2;
            assert_eq!(left.x + left.cols + 1, right.x);
            assert!(
                Router::default()
                    .route(
                        click(Point {
                            x: left.x + left.cols,
                            y: left.y
                        }),
                        &f
                    )
                    .is_empty()
            );
        }
        let target = targets.iter().find(|(i, _, _)| *i == 2).unwrap().2;
        for y in target.y..target.y + target.rows {
            assert_eq!(
                actions(Router::default().route(click(Point { x: target.x, y }), &f)),
                vec![Action::TabPositionSet(TabPosition::Left)]
            );
        }
    }
}
#[test]
fn parking_and_restore_confirm_before_mutation_and_escape_cancels() {
    use kelpmux::native::ui::DialogKind;
    let mut ws = workspace((120, 40), (2, 1));
    ws.apply(&Action::Read(1)).unwrap();
    let original = ws.terminal_sizes();
    let requested =
        actions(Router::default().route(InputEvent::Bytes(b"\x02pX".to_vec()), &frame(&ws)));
    assert_eq!(requested, vec![Action::ParkRequest(1)]);
    ws.apply(&requested[0]).unwrap();
    assert!(!ws.panes[0].parked);
    assert_eq!(ws.terminal_sizes(), original);
    assert_eq!(ws.ui.dialog.unwrap().kind, DialogKind::ParkConfirm);
    let cancel = actions(Router::default().route(InputEvent::Bytes(vec![27]), &frame(&ws)));
    for action in cancel {
        ws.apply(&action).unwrap();
    }
    assert!(!ws.panes[0].parked);
    ws.apply(&Action::ParkRequest(1)).unwrap();
    for action in actions(Router::default().route(InputEvent::Bytes(b"\r".to_vec()), &frame(&ws))) {
        ws.apply(&action).unwrap();
    }
    assert!(ws.panes[0].parked);
    for action in [Action::Focus(1), Action::Read(1), Action::Fullscreen(1)] {
        ws.apply(&action).unwrap();
        assert!(ws.panes[0].parked);
        assert_eq!(ws.ui.dialog.unwrap().kind, DialogKind::RestoreConfirm);
        ws.apply(&Action::DialogClose).unwrap();
    }
    ws.apply(&Action::Parked).unwrap();
    let choose = actions(Router::default().route(InputEvent::Bytes(b"\r".to_vec()), &frame(&ws)));
    assert_eq!(choose, vec![Action::RestoreRequest(1)]);
    ws.apply(&choose[0]).unwrap();
    assert!(ws.panes[0].parked);
    assert_eq!(ws.ui.dialog.unwrap().kind, DialogKind::RestoreConfirm);
    for action in actions(Router::default().route(InputEvent::Bytes(vec![27]), &frame(&ws))) {
        ws.apply(&action).unwrap();
    }
    assert!(ws.panes[0].parked);
    ws.apply(&Action::RestoreRequest(1)).unwrap();
    for action in actions(Router::default().route(InputEvent::Bytes(b"\r".to_vec()), &frame(&ws))) {
        ws.apply(&action).unwrap();
    }
    assert!(!ws.panes[0].parked);
    assert_eq!(ws.active, 1);
}

#[test]
fn parked_tab_control_fits_all_positions_and_remains_reachable_when_all_parked() {
    use kelpmux::native::{
        ui::TabPosition,
        workspace::{TabTarget, tab_targets},
    };
    for size in [(40, 10), (80, 24), (120, 40), (240, 70)] {
        for position in [
            TabPosition::Top,
            TabPosition::Bottom,
            TabPosition::Left,
            TabPosition::Auto,
        ] {
            for count in 0..=9 {
                let mut ws = workspace(size, (3, 3));
                ws.ui.config.tab_position = position;
                for id in count + 1..=9 {
                    ws.apply(&Action::Park(id)).unwrap();
                }
                if count > 0 {
                    ws.apply(&Action::Read(1)).unwrap();
                }
                let f = frame(&ws);
                let targets = tab_targets(f.cols, f.rows, &f.ui.config, count as usize, 0);
                for (i, (_, rect)) in targets.iter().enumerate() {
                    assert!(rect.x + rect.cols <= f.cols && rect.y + rect.rows <= f.rows);
                    for (_, other) in targets.iter().skip(i + 1) {
                        assert!(
                            rect.x + rect.cols <= other.x
                                || other.x + other.cols <= rect.x
                                || rect.y + rect.rows <= other.y
                                || other.y + other.rows <= rect.y
                        );
                    }
                }
                let parked = targets
                    .iter()
                    .find(|(t, _)| *t == TabTarget::Parked)
                    .unwrap()
                    .1;
                assert_eq!(
                    actions(Router::default().route(
                        click(Point {
                            x: parked.x,
                            y: parked.y
                        }),
                        &f
                    )),
                    vec![Action::Parked]
                );
            }
        }
    }
}

#[test]
fn parked_table_animates_status_and_modals_dim_without_altering_terminal_contents() {
    use kelpmux::native::{ui::DialogKind, workspace::dialog_bounds_for};
    let mut ws = workspace((120, 40), (2, 1));
    ws.apply(&Action::Read(1)).unwrap();
    let body = ws.layout()[0].body;
    let mut terminal = Terminal::new(body.cols, body.rows);
    terminal.feed(b"\x1b[38;2;120;200;240m\x1b[48;2;100;80;60mcontent");
    let screen = terminal.snapshot();
    let screens = BTreeMap::from([(1, screen.clone())]);
    let original = compose(&ws, &screens, true, None, 0);
    let at = usize::from(body.y * ws.cols + body.x);
    for action in [
        Action::Config,
        Action::Help,
        Action::ClosePaneRequest(1),
        Action::ParkRequest(1),
    ] {
        ws.help = false;
        ws.ui.dialog = None;
        ws.apply(&action).unwrap();
        let modal = compose(&ws, &screens, true, None, 0);
        assert_eq!(modal.cells[at].text, original.cells[at].text);
        assert_eq!(modal.cells[at].fg, Color::Rgb(50, 84, 100));
        assert_eq!(modal.cells[at].bg, Color::Rgb(42, 33, 25));
        assert_eq!(screens[&1], screen);
        let mut display = Terminal::new(ws.cols, ws.rows);
        display.feed(&terminal_diff(None, &modal));
        assert_eq!(display.snapshot().cells, modal.cells);
    }
    ws.ui.dialog = None;
    ws.panes[1].status = Status::Working;
    ws.panes[1].title = "A parked agent".into();
    ws.apply(&Action::Park(2)).unwrap();
    ws.apply(&Action::Parked).unwrap();
    let bounds = dialog_bounds_for(ws.cols, ws.rows, DialogKind::Parked);
    let a = compose(&ws, &screens, true, None, 0);
    let b = compose(&ws, &screens, true, None, 1);
    assert_ne!(a.cells, b.cells);
    for (before, after) in a.cells.iter().zip(&b.cells) {
        if before.text != "●" && after.text != "●" {
            assert_eq!(before.text, after.text);
            assert_eq!(before.fg, after.fg);
        }
    }
    let accent = usize::from((bounds.y + 2) * ws.cols + bounds.x);
    assert_eq!(a.cells[accent].text, "▎");
    assert_eq!(
        a.cells[accent].fg,
        Color::Indexed(kelpmux::native::ui::PALETTE[2].1)
    );
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
        vec![Action::CloseReading]
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
                    x: kelpmux::native::workspace::tab_close(*rect).x,
                    y: kelpmux::native::workspace::tab_close(*rect).y,
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
    use kelpmux::native::{
        ui::TabPosition,
        workspace::{config_targets, dialog_bounds_for},
    };
    let mut ws = workspace((120, 40), (2, 2));
    ws.apply(&Action::Config).unwrap();
    let f = frame(&ws);
    let bounds = dialog_bounds_for(f.cols, f.rows, f.ui.dialog.unwrap().kind);
    let choice = config_targets(bounds)
        .into_iter()
        .find(|(index, _, _)| *index == 2)
        .unwrap()
        .2;
    assert_eq!(
        actions(Router::default().route(
            click(Point {
                x: choice.x,
                y: choice.y
            }),
            &f
        )),
        vec![Action::TabPositionSet(TabPosition::Left)]
    );
    ws.apply(&Action::ClosePaneRequest(2)).unwrap();
    assert_eq!(ws.ui.dialog.unwrap().selection, 1);
    let f = frame(&ws);
    assert_eq!(
        actions(Router::default().route(InputEvent::Bytes(b"\r".to_vec()), &f)),
        vec![Action::ClosePaneConfirm(2), Action::DialogClose]
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
    assert_eq!(cap.rows, 65);
    assert_eq!(cap.y, 4);
    assert_eq!(cap.x, 0);
    ws.apply(&Action::Fullscreen(5)).unwrap();
    assert_eq!(ws.layout()[4].body.cols, 239);
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
    assert!(footer.contains("Parked"));
    assert!(footer.starts_with(" Config "));
    assert_eq!(&footer[..17], " Config  Tabs  ? ");
    assert!(!footer.contains('⛶') && !footer.contains('▣'));
    let start = usize::from((f.rows - 1) * f.cols);
    assert!(
        f.cells[start + usize::from(kelpmux::native::ui::FOOTER_COLS)..]
            .iter()
            .all(|c| c.bg == Color::Indexed(kelpmux::native::ui::FOOTER_BG))
    );
    assert_eq!(
        actions(Router::default().route(InputEvent::Bytes(b"\x02f".to_vec()), &f)),
        vec![Action::Fullscreen(1)]
    );
    assert_eq!(
        actions(Router::default().route(
            click(Point {
                x: 1,
                y: f.rows - 1
            }),
            &f
        )),
        vec![Action::Config]
    );
    assert_eq!(
        actions(Router::default().route(
            click(Point {
                x: 9,
                y: f.rows - 1
            }),
            &f
        )),
        vec![Action::Read(1)]
    );
    assert_eq!(
        actions(Router::default().route(
            click(Point {
                x: 17,
                y: f.rows - 1
            }),
            &f
        )),
        vec![Action::Parked]
    );
    assert_eq!(
        actions(Router::default().route(
            click(Point {
                x: 15,
                y: f.rows - 1
            }),
            &f
        )),
        vec![Action::Help]
    );
}

#[test]
fn grid_headers_separate_rows_without_gaps_and_vertical_lines_keep_the_left_pane_color() {
    for shape in GRIDS {
        let ws = workspace((160, 50), shape);
        let f = frame(&ws);
        let views = ws.grid_layout();
        for view in &views {
            if view.rect.x > 0 {
                let x = view.rect.x - 1;
                assert_eq!(f.cells[usize::from(view.body.y * f.cols + x)].text, "▎");
                let left = views
                    .iter()
                    .find(|left| left.rect.y == view.rect.y && left.rect.x + left.rect.cols == x)
                    .unwrap();
                assert_eq!(
                    f.cells[usize::from(view.body.y * f.cols + x)].fg,
                    Color::Indexed(kelpmux::native::ui::PALETTE[usize::from(left.pane.palette)].1)
                );
                assert!(
                    views
                        .iter()
                        .all(|other| !other.body.contains(Point { x, y: view.body.y }))
                );
            }
            if view.rect.y > 0 {
                assert!(
                    views
                        .iter()
                        .any(|upper| upper.rect.y + upper.rect.rows == view.rect.y)
                );
                assert_eq!(view.body.y, view.rect.y + 1);
            }
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
        workspace::{TabTarget, close_buttons, dialog_bounds_for, tab_targets},
    };
    for size in [(40, 10), (100, 30), (240, 70)] {
        for position in [TabPosition::Left, TabPosition::Top, TabPosition::Bottom] {
            let mut ws = workspace(size, (2, 1));
            ws.ui.config.tab_position = position;
            ws.apply(&Action::Select(1)).unwrap();
            let f = frame(&ws);
            let targets = tab_targets(f.cols, f.rows, &f.ui.config, 2, 0);
            for (index, (_, rect)) in targets.iter().enumerate() {
                assert!(rect.x + rect.cols <= f.cols && rect.y + rect.rows <= f.rows);
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
            if position == TabPosition::Left && size.1 >= 30 {
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
            let capped = ws.layout()[0].body;
            let effective = kelpmux::native::workspace::reading_limit_effective(
                ws.cols,
                ws.rows,
                &ws.ui.config,
            );
            if capped.x + capped.cols < ws.cols && effective {
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
            assert_eq!(full.x + full.cols, ws.cols - u16::from(effective));
            let full_frame = frame(&ws);
            let edge_actions = actions(Router::default().route(
                click(Point {
                    x: ws.cols - 1,
                    y: full.y,
                }),
                &full_frame,
            ));
            if effective {
                assert_eq!(edge_actions, vec![Action::WrapToggle]);
            } else {
                assert!(!edge_actions.contains(&Action::WrapToggle));
            }
            assert_eq!(
                full_frame.cells[usize::from(full.y * ws.cols + ws.cols - 1)].text,
                if effective { "−" } else { " " }
            );
            ws.apply(&Action::WrapToggle).unwrap();
            assert_eq!(ws.layout()[0].body, capped);

            ws.apply(&Action::Config).unwrap();
            let f = frame(&ws);
            let bounds = dialog_bounds_for(f.cols, f.rows, f.ui.dialog.unwrap().kind);
            let choice = kelpmux::native::workspace::config_targets(bounds)
                .into_iter()
                .find(|(index, value, _)| *index == 4 && *value == Some(false))
                .unwrap()
                .2;
            assert_eq!(
                actions(Router::default().route(
                    click(Point {
                        x: choice.x,
                        y: choice.y
                    }),
                    &f
                )),
                vec![Action::WrapToggle]
            );
            ws.apply(&Action::ClosePaneRequest(2)).unwrap();
            let f = frame(&ws);
            let bounds = dialog_bounds_for(f.cols, f.rows, f.ui.dialog.unwrap().kind);
            assert!(bounds.cols <= 52 && bounds.rows <= 9);
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
                    Color::Indexed(if index == 0 { 24 } else { 196 })
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

#[test]
fn tab_colors_keep_bright_titles_selected_outline_and_disjoint_picker_controls() {
    use kelpmux::native::{
        ui::{PALETTE, TabPosition},
        workspace::{TabTarget, color_cards, dialog_bounds, tab_number, tab_targets},
    };
    for size in [(40, 10), (120, 40), (240, 70)] {
        let mut ws = workspace(size, (2, 1));
        ws.ui.config.tab_position = TabPosition::Left;
        ws.apply(&Action::Select(1)).unwrap();
        let f = frame(&ws);
        let targets = tab_targets(f.cols, f.rows, &f.ui.config, 2, 0);
        for index in 0..2 {
            let rect = targets
                .iter()
                .find(|(target, _)| *target == TabTarget::Pane(index))
                .unwrap()
                .1;
            let number = tab_number(rect);
            let number_cell = &f.cells[usize::from(number.y * f.cols + number.x + 1)];
            assert_eq!(
                number_cell.fg,
                Color::Indexed(if index == 0 { 226 } else { 231 })
            );
            for x in number.x + 3..number.x + 5 {
                assert_eq!(f.cells[usize::from(number.y * f.cols + x)].text, " ");
                assert_eq!(
                    f.cells[usize::from(number.y * f.cols + x)].bg,
                    Color::Indexed(if index == 0 {
                        PALETTE[usize::from(f.panes[index].pane.palette)].1
                    } else {
                        kelpmux::native::ui::TAB_BG
                    })
                );
            }
            assert_eq!(
                f.cells[usize::from(rect.y * f.cols + rect.x)].fg,
                Color::Indexed(PALETTE[usize::from(f.panes[index].pane.palette)].1)
            );
            if rect.rows >= 3 {
                let title = &f.cells[usize::from((rect.y + 1) * f.cols + rect.x + 2)];
                assert_eq!(title.text, "a");
                assert_eq!(title.fg, Color::Indexed(231));
                assert_eq!(
                    title.bg,
                    Color::Indexed(if index == 0 {
                        PALETTE[usize::from(f.panes[index].pane.palette)].1
                    } else {
                        kelpmux::native::ui::TAB_BG
                    })
                );
            }
            let point = Point {
                x: number.x + 1,
                y: number.y,
            };
            assert_eq!(
                actions(Router::default().route(click(point), &f)),
                vec![Action::TabColor(index as u64 + 1)]
            );
        }
        let ids: Vec<_> = ws.panes.iter().map(|pane| pane.id).collect();
        ws.apply(&Action::TabColor(2)).unwrap();
        for color in 0..9 {
            let dialog = frame(&ws);
            let card = color_cards(dialog_bounds(dialog.cols, dialog.rows))[color];
            let messages = actions(Router::default().route(
                click(Point {
                    x: card.x + 1,
                    y: card.y,
                }),
                &dialog,
            ));
            assert_eq!(
                messages,
                vec![
                    Action::TabColorSet {
                        pane: 2,
                        color: color as u8
                    },
                    Action::DialogClose
                ]
            );
            for action in messages {
                ws.apply(&action).unwrap();
            }
            assert_eq!(ws.active, 1);
            assert_eq!(ws.panes[1].palette, color as u8);
            assert_eq!(ws.ui.config.pane_colors.get(&2), Some(&(color as u8)));
            assert_eq!(ws.panes.iter().map(|pane| pane.id).collect::<Vec<_>>(), ids);
            let rendered = frame(&ws);
            let mut display = Terminal::new(rendered.cols, rendered.rows);
            display.feed(&terminal_diff(None, &rendered));
            assert_eq!(display.snapshot().cells, rendered.cells);
            ws.apply(&Action::TabColor(2)).unwrap();
        }
        assert!(
            ws.apply(&Action::TabColorSet { pane: 2, color: 9 })
                .is_err()
        );
    }
}

#[test]
fn terminal_content_has_no_blank_side_margins_in_tabs_or_grid() {
    use kelpmux::native::ui::TabPosition;
    for position in [TabPosition::Top, TabPosition::Left, TabPosition::Bottom] {
        let mut ws = workspace((120, 40), (2, 1));
        ws.ui.config.tab_position = position;
        ws.apply(&Action::Fullscreen(1)).unwrap();
        let mut f = frame(&ws);
        let body = f.panes[0].body;
        assert_eq!(body.x + body.cols, f.cols);
        assert_eq!(body.x, if position == TabPosition::Left { 24 } else { 0 });
        f.panes[0].modes.mouse_click = true;
        for x in [body.x, body.x + body.cols - 1] {
            let point = Point { x, y: body.y + 2 };
            assert!(
                Router::default()
                    .route(click(point), &f)
                    .iter()
                    .any(|message| matches!(message, ClientMessage::Mouse { .. }))
            );
        }
        ws.apply(&Action::CloseReading).unwrap();
        for view in ws.layout() {
            assert_eq!(view.body.x, view.rect.x);
            assert_eq!(view.body.x + view.body.cols, view.rect.x + view.rect.cols);
        }
    }
}

#[test]
fn width_switch_reserves_only_a_useful_divider_at_the_cap_boundary() {
    use kelpmux::native::{
        ui::TabPosition,
        workspace::{reading_limit_effective, tab_body},
    };
    for position in [TabPosition::Left, TabPosition::Top, TabPosition::Bottom] {
        let mut ws = workspace((120, 40), (1, 1));
        ws.ui.config.tab_position = position;
        let available = if position == TabPosition::Left {
            96
        } else {
            120
        };
        for cap in [
            available - 21,
            available - 20,
            available - 19,
            available,
            available + 1,
        ] {
            ws.ui.config.read_width = cap;
            let effective = available.saturating_sub(cap) >= 20;
            assert_eq!(
                reading_limit_effective(ws.cols, ws.rows, &ws.ui.config),
                effective
            );
            let capped = tab_body(ws.cols, ws.rows, &ws.ui.config, false, None);
            let full = tab_body(ws.cols, ws.rows, &ws.ui.config, true, None);
            assert_eq!(full.cols, available - u16::from(effective));
            assert_eq!(capped.cols, if effective { cap } else { available });
        }
    }
}

#[test]
fn working_tabs_animate_only_backgrounds_and_keep_bells_beside_titles() {
    use kelpmux::native::{
        ui::TabPosition,
        workspace::{TabTarget, tab_close, tab_number, tab_park, tab_targets},
    };
    for (position, size) in [
        (TabPosition::Left, (120, 40)),
        (TabPosition::Top, (240, 70)),
        (TabPosition::Bottom, (40, 10)),
    ] {
        for active in [1, 2] {
            let mut ws = workspace(size, (2, 1));
            ws.ui.config.tab_position = position;
            ws.panes[0].status = Status::Working;
            ws.panes[0].title = "Wide 界 title".into();
            ws.panes[1].status = Status::Unknown;
            ws.apply(&Action::Select(active)).unwrap();
            // Keep the working tab visible on a one-tab horizontal viewport.
            if size.0 == 40 && active == 2 {
                continue;
            }
            ws.ui.notifications.push(Notification {
                id: 1,
                pane: 1,
                title: "Question".into(),
                status: Status::InputNeeded,
                at: 1,
                read: false,
                workspace: "test".into(),
            });
            let tab = tab_targets(ws.cols, ws.rows, &ws.ui.config, 2, active - 1)
                .into_iter()
                .find(|(target, _)| *target == TabTarget::Pane(0))
                .unwrap()
                .1;
            let body = ws
                .layout()
                .into_iter()
                .find(|view| view.pane.id == active as u64)
                .unwrap()
                .body;
            let mut terminal = Terminal::new(body.cols, body.rows);
            terminal.feed(b"\x1b[48;2;31;31;31mcontent\x1b[0m");
            let screens = BTreeMap::from([(active as u64, terminal.snapshot())]);
            let a = compose(&ws, &screens, true, None, 0);
            let b = compose(&ws, &screens, true, None, 16);
            assert_ne!(a.cells, b.cells);
            for (index, (before, after)) in a.cells.iter().zip(&b.cells).enumerate() {
                assert_eq!(before.text, after.text);
                assert_eq!(before.fg, after.fg);
                if before != after {
                    assert!(tab.contains(Point {
                        x: (index % usize::from(ws.cols)) as u16,
                        y: (index / usize::from(ws.cols)) as u16
                    }));
                }
            }
            for control in [tab_number(tab), tab_close(tab), tab_park(tab)] {
                for x in control.x..control.x + control.cols {
                    let index = usize::from(control.y * ws.cols + x);
                    assert_eq!(a.cells[index], b.cells[index]);
                }
            }
            let bell = usize::from((tab.y + 1) * ws.cols + tab.x + 2);
            assert_eq!(b.cells[bell].text, "🔔");
            assert_eq!(b.cells[bell].width, 2);
            let mut display = Terminal::new(ws.cols, ws.rows);
            display.feed(&terminal_diff(None, &a));
            assert_eq!(display.snapshot().cells, a.cells);
            display.feed(&terminal_diff(Some(&a), &b));
            assert_eq!(display.snapshot().cells, b.cells);
            ws.apply(&Action::Acknowledge(1)).unwrap();
            ws.panes[0].status = Status::Unknown;
            let stopped = compose(&ws, &screens, true, None, 16);
            assert_eq!(stopped.cells, compose(&ws, &screens, true, None, 32).cells);
            assert!(!stopped.cells.iter().any(|cell| cell.text == "🔔"));
        }
    }
}

#[test]
fn tab_folder_subtitles_and_horizontal_rules_stay_outside_terminal_hit_targets() {
    use kelpmux::native::{
        ui::TabPosition,
        workspace::{TabTarget, tab_targets},
    };
    for position in [TabPosition::Top, TabPosition::Bottom, TabPosition::Left] {
        let mut ws = workspace((120, 40), (2, 1));
        ws.ui.config.tab_position = position;
        ws.panes[0].title = "Conversation name".into();
        ws.panes[0].cwd = "/home/joan/repos/project".into();
        ws.apply(&Action::Select(1)).unwrap();
        let f = frame(&ws);
        let tab = tab_targets(f.cols, f.rows, &f.ui.config, 2, 0)
            .into_iter()
            .find(|(target, _)| *target == TabTarget::Pane(0))
            .unwrap()
            .1;
        let folder = usize::from((tab.y + 2) * f.cols + tab.x + 2);
        let name = usize::from((tab.y + 1) * f.cols + tab.x + 2);
        assert_eq!(
            f.cells[folder..folder + 7]
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>(),
            "project"
        );
        assert_ne!(f.cells[folder].fg, f.cells[name].fg);
        if position != TabPosition::Left {
            let y = if position == TabPosition::Top {
                3
            } else {
                f.rows - 5
            };
            for x in 0..f.cols {
                assert_eq!(
                    f.cells[usize::from(y * f.cols + x)].text,
                    if position == TabPosition::Top {
                        "▔"
                    } else {
                        "▁"
                    }
                );
                assert!(
                    Router::default()
                        .route(click(Point { x, y }), &f)
                        .is_empty()
                );
                assert!(!f.panes[0].body.contains(Point { x, y }));
            }
        }
    }
}

#[test]
fn horizontal_tabs_have_three_rows_an_attached_wider_add_button_and_right_anchored_parked() {
    use kelpmux::native::{
        ui::TabPosition,
        workspace::{TabTarget, tab_targets},
    };
    for position in [TabPosition::Top, TabPosition::Bottom] {
        for size in [(40, 10), (120, 40), (240, 70)] {
            for count in [0, 1, 4, 9] {
                let mut ws = workspace(
                    size,
                    match count {
                        4 => (2, 2),
                        9 => (3, 3),
                        _ => (1, 1),
                    },
                );
                ws.ui.config.tab_position = position;
                if count == 0 {
                    ws.apply(&Action::Park(1)).unwrap();
                } else {
                    ws.apply(&Action::Select(1)).unwrap();
                }
                let f = frame(&ws);
                let targets = tab_targets(ws.cols, ws.rows, &ws.ui.config, count as usize, 0);
                assert!(
                    targets
                        .iter()
                        .all(|(target, r)| r.rows
                            == if *target == TabTarget::Parked { 1 } else { 3 })
                );
                let new = targets
                    .iter()
                    .find(|(t, _)| *t == TabTarget::New)
                    .unwrap()
                    .1;
                let parked = targets
                    .iter()
                    .find(|(t, _)| *t == TabTarget::Parked)
                    .unwrap()
                    .1;
                assert_eq!(parked.y, ws.rows - 1);
                assert_eq!(new.cols, 5);
                assert_eq!(
                    f.cells[usize::from((new.y + 1) * f.cols + new.x + 2)].text,
                    "+"
                );
                for y in new.y..new.y + 3 {
                    for x in new.x..new.x + new.cols {
                        let actual = actions(Router::default().route(click(Point { x, y }), &f));
                        assert_eq!(
                            actual,
                            if count < 9 {
                                vec![Action::NewTab]
                            } else {
                                vec![]
                            }
                        );
                    }
                    assert_eq!(
                        actions(Router::default().route(
                            click(Point {
                                x: parked.x,
                                y: parked.y
                            }),
                            &f
                        )),
                        vec![Action::Parked]
                    );
                }
                if count > 0 {
                    let tab = targets
                        .iter()
                        .find(|(t, _)| *t == TabTarget::Pane(0))
                        .unwrap()
                        .1;
                    let last_tab = targets
                        .iter()
                        .filter_map(|(target, rect)| {
                            matches!(target, TabTarget::Pane(_)).then_some(rect.x + rect.cols)
                        })
                        .max()
                        .unwrap();
                    assert_eq!(new.x, last_tab);
                    assert!(
                        !f.cells[usize::from(tab.y * f.cols + tab.x)
                            ..usize::from(tab.y * f.cols + tab.x + tab.cols)]
                            .iter()
                            .any(|cell| cell.text == "?")
                    );
                    assert!(
                        f.cells
                            [usize::from((tab.y + 1) * f.cols)..usize::from((tab.y + 2) * f.cols)]
                            .iter()
                            .any(|c| c.text == "a")
                    );
                }
            }
        }
    }
}

#[test]
fn parked_three_row_entries_scroll_and_every_row_restores_the_correct_tab() {
    use kelpmux::native::{
        ui::DialogKind,
        workspace::{dialog_bounds_for, parked_cards, parked_controls},
    };
    for size in [(40, 10), (120, 40)] {
        let mut ws = workspace(size, (3, 3));
        for id in 1..=9 {
            ws.apply(&Action::Park(id)).unwrap();
        }
        ws.apply(&Action::Parked).unwrap();
        for selection in [0, 4, 8] {
            ws.apply(&Action::DialogPick(selection)).unwrap();
            let f = frame(&ws);
            let bounds = dialog_bounds_for(f.cols, f.rows, DialogKind::Parked);
            let cards = parked_cards(bounds, selection, 9);
            assert!(cards.iter().any(|(index, _)| *index == selection));
            for (index, rect) in cards {
                assert_eq!(rect.rows, 3);
                assert!(rect.y + rect.rows <= bounds.y + bounds.rows);
                for row in 0..3 {
                    assert_eq!(
                        f.cells[usize::from((rect.y + row) * f.cols + rect.x)].text,
                        "▎"
                    );
                    assert_eq!(
                        actions(Router::default().route(
                            click(Point {
                                x: rect.x + 1,
                                y: rect.y + row
                            }),
                            &f
                        )),
                        vec![Action::RestoreRequest(index as u64 + 1)]
                    );
                    let [restore, close] = parked_controls(rect);
                    assert_eq!(
                        actions(Router::default().route(
                            click(Point {
                                x: restore.x,
                                y: rect.y + row
                            }),
                            &f
                        )),
                        vec![Action::RestoreRequest(index as u64 + 1)]
                    );
                    assert_eq!(
                        actions(Router::default().route(
                            click(Point {
                                x: close.x,
                                y: rect.y + row
                            }),
                            &f
                        )),
                        vec![Action::ClosePaneRequest(index as u64 + 1)]
                    );
                }
            }
        }
    }
}

#[test]
fn tab_overflow_arrows_only_appear_when_needed_and_bracket_the_list() {
    use kelpmux::native::{
        ui::TabPosition,
        workspace::{TabTarget, tab_targets},
    };
    for size in [(40, 10), (80, 20), (120, 40), (240, 70)] {
        for position in [TabPosition::Left, TabPosition::Top, TabPosition::Bottom] {
            for count in 1..=9 {
                for active in 0..count {
                    let mut ws = workspace(size, (3, 3));
                    ws.panes.truncate(count);
                    ws.ui.config.tab_position = position;
                    ws.apply(&Action::Select(active + 1)).unwrap();
                    let f = frame(&ws);
                    let targets = tab_targets(f.cols, f.rows, &f.ui.config, count, active);
                    let tabs: Vec<_> = targets
                        .iter()
                        .filter(|(t, _)| matches!(t, TabTarget::Pane(_)))
                        .collect();
                    let overflow = tabs.len() < count;
                    assert!(tabs.iter().any(|(t, _)| *t == TabTarget::Pane(active)));
                    for (index, (_, rect)) in targets.iter().enumerate() {
                        assert!(rect.x + rect.cols <= f.cols && rect.y + rect.rows <= f.rows);
                        for (_, other) in targets.iter().skip(index + 1) {
                            assert!(
                                rect.x + rect.cols <= other.x
                                    || other.x + other.cols <= rect.x
                                    || rect.y + rect.rows <= other.y
                                    || other.y + other.rows <= rect.y
                            );
                        }
                    }
                    for direction in [TabTarget::Previous, TabTarget::Next] {
                        let arrow = targets.iter().find(|(t, _)| *t == direction);
                        assert_eq!(arrow.is_some(), overflow);
                        if let Some((_, rect)) = arrow {
                            if position == TabPosition::Left {
                                if direction == TabTarget::Previous {
                                    assert!(rect.y < tabs[0].1.y);
                                } else {
                                    assert!(
                                        rect.y
                                            >= tabs.last().unwrap().1.y
                                                + tabs.last().unwrap().1.rows
                                    );
                                }
                            } else if direction == TabTarget::Previous {
                                assert!(rect.x < tabs[0].1.x);
                            } else {
                                assert!(
                                    rect.x
                                        >= tabs.last().unwrap().1.x + tabs.last().unwrap().1.cols
                                );
                            }
                            let selected = if direction == TabTarget::Previous {
                                active.saturating_sub(1)
                            } else {
                                (active + 1).min(count - 1)
                            };
                            assert_eq!(
                                actions(Router::default().route(
                                    click(Point {
                                        x: rect.x,
                                        y: rect.y
                                    }),
                                    &f
                                )),
                                vec![Action::Focus(selected as u64 + 1)]
                            );
                        }
                    }
                    let new = targets
                        .iter()
                        .find(|(t, _)| *t == TabTarget::New)
                        .unwrap()
                        .1;
                    if position == TabPosition::Left {
                        assert!(
                            targets
                                .iter()
                                .all(|(_, rect)| !rect.contains(Point { x: 1, y: new.y - 1 }))
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn active_palette_connects_sidebar_and_width_dividers_in_capped_and_full_modes() {
    use kelpmux::native::{
        ui::{PALETTE, TabPosition},
        workspace::{TabTarget, tab_number, tab_targets},
    };
    let mut ws = workspace((240, 40), (2, 1));
    ws.ui.config.tab_position = TabPosition::Left;
    ws.apply(&Action::Select(1)).unwrap();
    for (color, &(_, muted)) in PALETTE.iter().enumerate() {
        ws.apply(&Action::TabColorSet {
            pane: 1,
            color: color as u8,
        })
        .unwrap();
        for wrapping in [true, false] {
            ws.ui.config.wrap_lines = wrapping;
            let f = frame(&ws);
            let body = f.panes[0].body;
            let tab = tab_targets(f.cols, f.rows, &f.ui.config, 2, 0)
                .into_iter()
                .find(|(t, _)| *t == TabTarget::Pane(0))
                .unwrap()
                .1;
            let number = tab_number(tab);
            assert_eq!(
                f.cells[usize::from(number.y * f.cols + number.x + 4)].bg,
                Color::Indexed(muted)
            );
            for y in 0..f.rows - 1 {
                let cell = &f.cells[usize::from(y * f.cols + body.x - 1)];
                assert_eq!(cell.text, "▎");
                assert_eq!(cell.fg, Color::Indexed(muted));
                assert_eq!(cell.bg, Color::Default);
            }
            let right = body.x + body.cols;
            if !wrapping {
                assert_eq!(right, f.cols - 1);
            }
            for y in body.y..body.y + body.rows {
                let cell = &f.cells[usize::from(y * f.cols + right)];
                assert_eq!(
                    cell.text,
                    if y == body.y {
                        if wrapping { "+" } else { "−" }
                    } else {
                        "┃"
                    }
                );
                assert_eq!(cell.fg, Color::Indexed(muted));
            }
        }
    }
}

#[test]
fn automatic_grid_parking_controls_alert_colors_and_three_row_tabs() {
    use kelpmux::native::{
        ui::TabPosition,
        workspace::{TabTarget, tab_park, tab_targets},
    };
    for size in [(40, 10), (100, 30), (240, 70)] {
        let mut ws = workspace(size, (3, 3));
        ws.ui.config.tab_position = TabPosition::Left;
        let ids: Vec<_> = ws.panes.iter().map(|p| p.id).collect();
        ws.reflow();
        for count in (1..=9).rev() {
            let views = ws.grid_layout();
            assert_eq!(views.iter().filter(|v| v.body.cols > 0).count(), count);
            for view in views.iter().filter(|v| v.body.cols > 0) {
                let end = views
                    .iter()
                    .filter(|other| other.rect.y == view.rect.y && other.body.cols > 0)
                    .map(|other| other.rect.x + other.rect.cols)
                    .max()
                    .unwrap();
                assert_eq!(end, ws.cols);
                assert!(view.rect.y + view.rect.rows < ws.rows);
            }
            ws.apply(&Action::Park(count as u64)).unwrap();
        }
        assert_eq!(ws.active, 0);
        assert!(ws.terminal_sizes().is_empty());
        ws.apply(&Action::Parked).unwrap();
        let f = frame(&ws);
        assert_eq!(
            actions(Router::default().route(InputEvent::Bytes(b"\r".to_vec()), &f)),
            vec![Action::RestoreRequest(1)]
        );
        ws.apply(&Action::Restore(1)).unwrap();
        assert_eq!(ws.panes.iter().map(|p| p.id).collect::<Vec<_>>(), ids);
        let f = frame(&ws);
        let tab = tab_targets(f.cols, f.rows, &f.ui.config, 1, 0)
            .into_iter()
            .find(|(t, _)| *t == TabTarget::Pane(0))
            .unwrap()
            .1;
        assert_eq!(tab.rows, 3);
        let park = tab_park(tab);
        assert_eq!(
            actions(Router::default().route(
                click(Point {
                    x: park.x,
                    y: park.y
                }),
                &f
            )),
            vec![Action::ParkRequest(1)]
        );
        let at = usize::from((f.rows - 1) * f.cols + 15);
        assert_eq!(f.cells[at].bg, Color::Indexed(94));
        ws.ui.notifications.push(Notification {
            id: 1,
            pane: 1,
            title: "Question".into(),
            status: Status::InputNeeded,
            at: 1,
            read: false,
            workspace: "test".into(),
        });
        assert_eq!(frame(&ws).cells[at].bg, Color::Indexed(160));
        ws.apply(&Action::Acknowledge(1)).unwrap();
        assert_eq!(frame(&ws).cells[at].bg, Color::Indexed(94));
    }
}

#[test]
fn settings_zones_radio_values_and_keyboard_focus_remain_usable_on_small_screens() {
    use kelpmux::native::{
        ui::{DialogKind, TabPosition},
        workspace::{config_targets, dialog_bounds_for},
    };
    for size in [(40, 10), (80, 20), (120, 40), (240, 70)] {
        let mut ws = workspace(size, (2, 1));
        ws.ui.config.tab_position = TabPosition::Left;
        ws.apply(&Action::Select(1)).unwrap();
        ws.apply(&Action::Config).unwrap();
        let bounds = dialog_bounds_for(ws.cols, ws.rows, DialogKind::Config);
        assert!(bounds.cols <= 72 && bounds.rows <= 19);
        let targets = config_targets(bounds);
        for (index, value, rect) in &targets {
            assert!(rect.x >= bounds.x && rect.x + rect.cols <= bounds.x + bounds.cols);
            assert!(rect.y > bounds.y && rect.y + rect.rows <= bounds.y + bounds.rows);
            if let Some(value) = value {
                let current = if *index == 4 {
                    ws.ui.config.wrap_lines
                } else {
                    ws.ui.config.notification_sound
                };
                let f = frame(&ws);
                assert_eq!(
                    f.cells[usize::from((rect.y + rect.rows / 2) * f.cols + rect.x + 1)].text,
                    if current == *value { "●" } else { "○" }
                );
                let messages = actions(Router::default().route(
                    click(Point {
                        x: rect.x,
                        y: rect.y,
                    }),
                    &f,
                ));
                if current == *value {
                    assert!(messages.is_empty());
                } else {
                    assert_eq!(
                        messages,
                        vec![if *index == 4 {
                            Action::WrapToggle
                        } else {
                            Action::NotificationSoundToggle
                        }]
                    );
                    for action in messages {
                        ws.apply(&action).unwrap();
                    }
                }
                assert_eq!(
                    if *index == 4 {
                        ws.ui.config.wrap_lines
                    } else {
                        ws.ui.config.notification_sound
                    },
                    *value
                );
                let selected = frame(&ws);
                assert_eq!(
                    selected.cells
                        [usize::from((rect.y + rect.rows / 2) * selected.cols + rect.x + 1)]
                    .text,
                    "●"
                );
                assert!(
                    Router::default()
                        .route(
                            click(Point {
                                x: rect.x,
                                y: rect.y
                            }),
                            &selected
                        )
                        .is_empty()
                );
            }
        }
        ws.apply(&Action::DialogPick(2)).unwrap();
        for (dx, dy, expected) in [
            (1, 0, 3),
            (-1, 0, 2),
            (0, 1, 4),
            (0, 1, 5),
            (0, -1, 4),
            (0, -1, 2),
        ] {
            ws.apply(&Action::DialogNavigate { dx, dy }).unwrap();
            assert_eq!(ws.ui.dialog.unwrap().selection, expected);
        }
        // Section headings and padding must not toggle a value or reach the PTY.
        assert!(
            Router::default()
                .route(
                    click(Point {
                        x: bounds.x + 2,
                        y: bounds.y + 1
                    }),
                    &frame(&ws)
                )
                .is_empty()
        );
        let f = frame(&ws);
        let mut display = Terminal::new(f.cols, f.rows);
        display.feed(&terminal_diff(None, &f));
        assert_eq!(display.snapshot().cells, f.cells);
        ws.apply(&Action::Fullscreen(1)).unwrap();
        ws.apply(&Action::Config).unwrap();
        let f = frame(&ws);
        let on = targets
            .iter()
            .find(|(index, value, _)| *index == 4 && *value == Some(true))
            .unwrap()
            .2;
        for action in actions(Router::default().route(click(Point { x: on.x, y: on.y }), &f)) {
            ws.apply(&action).unwrap();
        }
        assert_eq!(ws.reading, ReadingMode::Tabs(1));
        assert!(ws.ui.config.wrap_lines);
    }
}

#[test]
fn grid_dimming_is_presentation_only_and_focus_restores_original_application_colors() {
    let mut ws = workspace((180, 50), (2, 1));
    let mut terminal = Terminal::new(20, 3);
    terminal.feed(b"\x1b[38;2;80;200;120m\x1b[48;2;20;40;90mcolored\x1b[0m");
    let screen = terminal.snapshot();
    let screens = BTreeMap::from([(1, screen.clone()), (2, screen.clone())]);
    let original = screens.clone();
    for selected in [1, 2] {
        ws.apply(&Action::Focus(selected)).unwrap();
        let f = compose(&ws, &screens, true, None, 0);
        for view in &f.panes {
            let cell = &f.cells[usize::from(view.body.y * f.cols + view.body.x)];
            if view.pane.id == selected {
                assert_eq!(cell, &screen.cells[0]);
            } else {
                assert_eq!(cell.text, screen.cells[0].text);
                for color in [cell.fg, cell.bg] {
                    assert!(matches!(color, Color::Rgb(r, g, b) if r == g && g == b));
                }
            }
        }
    }
    assert_eq!(screens, original);
}

#[test]
fn grid_header_numbers_zoom_park_close_and_footer_have_distinct_targets() {
    use kelpmux::native::workspace::{grid_controls, tab_number};
    let ws = workspace((180, 50), (2, 1));
    let f = frame(&ws);
    for view in &f.panes {
        let id = view.pane.id;
        let header = Rect {
            rows: 1,
            ..view.rect
        };
        let number = tab_number(header);
        let number_cell = &f.cells[usize::from(number.y * f.cols + number.x + 1)];
        assert_eq!(
            number_cell.fg,
            Color::Indexed(if id == ws.active { 226 } else { 231 })
        );
        let [zoom, park, close] = grid_controls(header);
        for (control, expected) in [
            (number, Action::TabColor(id)),
            (zoom, Action::Fullscreen(id)),
            (park, Action::ParkRequest(id)),
            (close, Action::ClosePaneRequest(id)),
        ] {
            assert_eq!(
                actions(Router::default().route(
                    click(Point {
                        x: control.x + 1,
                        y: control.y
                    }),
                    &f
                )),
                vec![expected]
            );
        }
    }
    for x in 17..29 {
        assert_eq!(
            actions(Router::default().route(click(Point { x, y: f.rows - 1 }), &f)),
            vec![Action::Parked]
        );
    }
}

#[test]
fn working_ball_bounces_in_tabs_grid_and_parked_cards_without_animating_idle_panes() {
    for mode in 0..3 {
        let mut ws = workspace((180, 50), (2, 1));
        ws.panes[0].status = Status::Working;
        ws.panes[1].status = Status::Idle;
        if mode == 1 {
            ws.apply(&Action::Read(1)).unwrap();
        }
        if mode == 2 {
            ws.apply(&Action::Park(1)).unwrap();
            ws.apply(&Action::Parked).unwrap();
        }
        let positions: Vec<_> = [0, 1, 4, 5, 8]
            .into_iter()
            .map(|tick| {
                let f = compose(&ws, &BTreeMap::new(), true, None, tick);
                let balls: Vec<_> = f
                    .cells
                    .iter()
                    .enumerate()
                    .filter(|(_, cell)| cell.text == "●")
                    .map(|(index, _)| index % usize::from(f.cols))
                    .collect();
                assert_eq!(balls.len(), 1);
                balls[0]
            })
            .collect();
        assert!(positions[0] < positions[1] && positions[1] < positions[2]);
        assert!(positions[3] < positions[2]);
        assert_eq!(positions[0], positions[4]);
    }
}
