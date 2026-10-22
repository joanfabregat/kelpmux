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
        ui::{DialogKind, TabPosition},
        workspace::{TabTarget, close_buttons, dialog_bounds_for, tab_targets},
    };
    let mut ws = workspace((120, 40), (2, 1));
    ws.ui.config.tab_position = TabPosition::Left;
    ws.apply(&Action::Select(1)).unwrap();
    let f = frame(&ws);
    let targets = tab_targets(f.cols, f.rows, &f.ui.config, 2, 0, 0);
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
    assert_eq!(new.y, last_tab);
    assert!(new.y + new.rows < parked.y);
    assert!(f.cells[usize::from(new.y * f.cols)].bg != f.cells[0].bg);
    for row in 0..new.rows {
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
    ws.apply(&Action::Config).unwrap();
    let config = frame(&ws);
    let bounds = dialog_bounds_for(f.cols, f.rows, DialogKind::Config);
    assert_eq!(
        config.cells[usize::from(bounds.y * f.cols + bounds.x)].bg,
        Color::Indexed(24)
    );
    ws.ui.dialog = None;
    ws.help = true;
    let help = frame(&ws);
    let bounds = help_bounds(f.cols, f.rows);
    assert_eq!(
        help.cells[usize::from(bounds.y * f.cols + bounds.x)].bg,
        Color::Indexed(94)
    );
    ws.help = false;
    for (action, kind, icon) in [
        (Action::Config, DialogKind::Config, "⚙︎"),
        (Action::GridPicker, DialogKind::Grid, "▦"),
        (Action::Parked, DialogKind::Parked, "⊟"),
        (
            Action::Notifications,
            DialogKind::Notifications,
            kelpmux::native::ui::ALERT_ICON,
        ),
    ] {
        ws.apply(&action).unwrap();
        let popup = frame(&ws);
        let bounds = dialog_bounds_for(f.cols, f.rows, kind);
        assert_eq!(
            popup.cells[usize::from(bounds.y * f.cols + bounds.x + 1)].text,
            icon
        );
        let mut display = Terminal::new(popup.cols, popup.rows);
        display.feed(&terminal_diff(None, &popup));
        assert_eq!(display.snapshot().cells, popup.cells);
        ws.apply(&Action::DialogClose).unwrap();
    }
    for action in [Action::ClosePaneRequest(2), Action::ParkRequest(2)] {
        ws.apply(&action).unwrap();
        let kind = ws.ui.dialog.unwrap().kind;
        let bounds = dialog_bounds_for(f.cols, f.rows, kind);
        assert_eq!(close_buttons(bounds)[0].rows, 3);
        let popup = frame(&ws);
        assert_eq!(
            popup.cells[usize::from(bounds.y * f.cols + bounds.x)].bg,
            Color::Indexed(if kind == DialogKind::ParkConfirm {
                kelpmux::native::ui::PARKED_BG
            } else {
                88
            })
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
        let popup = frame(&ws);
        let bounds = kelpmux::native::workspace::dialog_bounds_for(
            popup.cols,
            popup.rows,
            DialogKind::RestoreConfirm,
        );
        let button = kelpmux::native::workspace::close_buttons(bounds)[1];
        assert_eq!(
            popup.cells[usize::from(button.y * popup.cols + button.x)].bg,
            Color::Indexed(kelpmux::native::ui::RESTORE_BG)
        );
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
                let targets = tab_targets(f.cols, f.rows, &f.ui.config, count as usize, 0, 0);
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
                for (target, rect) in &targets {
                    if let TabTarget::Pane(index) = target {
                        if position.resolve(f.cols, f.rows) == TabPosition::Left {
                            continue;
                        }
                        let bg = Color::Indexed(if *index == 0 { 250 } else { 234 });
                        assert_eq!(f.cells[usize::from(rect.y * f.cols + rect.x)].bg, bg);
                        for (control, symbol, color) in [
                            (
                                kelpmux::native::workspace::tab_park(*rect),
                                "⊟",
                                if *index == 0 { 91 } else { 141 },
                            ),
                            (
                                kelpmux::native::workspace::tab_close(*rect),
                                "×",
                                if *index == 0 { 160 } else { 203 },
                            ),
                        ] {
                            let cell = &f.cells[usize::from(control.y * f.cols + control.x + 1)];
                            assert_eq!(cell.text, symbol);
                            assert_eq!(cell.fg, Color::Indexed(color));
                            for x in control.x..control.x + control.cols {
                                assert_eq!(f.cells[usize::from(control.y * f.cols + x)].bg, bg);
                            }
                        }
                    }
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
    let b = compose(&ws, &screens, true, None, 2);
    assert_ne!(a.cells, b.cells);
    for (before, after) in a.cells.iter().zip(&b.cells) {
        assert_eq!(before.bg, after.bg);
        if !matches!(before.text.as_str(), "⣤" | "⠶" | "⠛") {
            assert_eq!(before.text, after.text);
            assert_eq!(before.fg, after.fg);
        }
    }
    let accent = usize::from((bounds.y + 2) * ws.cols + bounds.x);
    assert_eq!(a.cells[accent].text, "▌");
    assert_eq!(a.cells[accent].fg, Color::Indexed(250));
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
            assert!(body.x + body.cols <= size.0 && body.y + body.rows <= size.1);
            assert_eq!(
                body.y + body.rows == size.1,
                position.resolve(size.0, size.1) == TabPosition::Left
            );
            assert_eq!(ws.terminal_sizes().len(), 9);
            let f = frame(&ws);
            let targets = tab_targets(f.cols, f.rows, &f.ui.config, 9, 8, 0);
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
fn selecting_same_tab_keeps_tabs_and_legacy_width_actions_do_not_limit_content() {
    let mut ws = workspace((240, 70), (3, 3));
    ws.apply(&Action::Select(5)).unwrap();
    ws.apply(&Action::Select(5)).unwrap();
    assert_eq!(ws.reading, ReadingMode::Tabs(5));
    ws.apply(&Action::ReadResize { cols: 90, rows: 3 }).unwrap();
    let cap = ws.layout()[4].body;
    assert_eq!(cap.cols, 240);
    assert_eq!(cap.rows, 67);
    assert_eq!(cap.y, 2);
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
    assert!(footer.contains("Parked"));
    assert!(footer.starts_with(" ⚙︎ Config"));
    assert!(footer.starts_with(" ⚙︎ Config  ▦ Tabs "));
    assert!(footer.find("Parked").unwrap() < footer.find('?').unwrap());
    assert!(!footer.contains('⛶') && !footer.contains('▣'));
    let start = usize::from((f.rows - 1) * f.cols);
    let config = f.controls()[0];
    let trailing = &f.cells[start + usize::from(config.cols - 1)];
    assert_eq!(trailing.text, " ");
    assert_eq!(trailing.bg, Color::Indexed(24));
    assert!(
        f.cells[start + usize::from(f.footer_cols())..]
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
                x: f.controls()[1].x,
                y: f.rows - 1
            }),
            &f
        )),
        vec![Action::Read(1)]
    );
    assert_eq!(
        actions(Router::default().route(
            click(Point {
                x: 20,
                y: f.rows - 1
            }),
            &f
        )),
        vec![Action::Parked]
    );
    assert_eq!(
        actions(Router::default().route(
            click(Point {
                x: f.footer_cols() - 2,
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
                assert_eq!(f.cells[usize::from(view.body.y * f.cols + x)].text, "│");
                let left = views
                    .iter()
                    .find(|left| left.rect.y == view.rect.y && left.rect.x + left.rect.cols == x)
                    .unwrap();
                assert_eq!(
                    f.cells[usize::from(view.body.y * f.cols + x)].fg,
                    Color::Indexed(if left.pane.id == ws.active { 250 } else { 244 })
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
            let line = &f.cells[usize::from(view.rect.y * f.cols + view.rect.x)];
            assert_eq!(line.text, "─");
            assert_eq!(line.bg, Color::Default);
        }
        let mut display = Terminal::new(f.cols, f.rows);
        display.feed(&terminal_diff(None, &f));
        assert_eq!(display.snapshot().cells, f.cells);
    }
}

#[test]
fn responsive_new_tabs_and_graphic_confirmation_have_disjoint_targets() {
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
            let targets = tab_targets(f.cols, f.rows, &f.ui.config, 2, 0, 0);
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
            let full = ws.layout()[0].body;
            assert_eq!(full.x + full.cols, ws.cols);
            let full_frame = frame(&ws);
            let edge_actions = actions(Router::default().route(
                click(Point {
                    x: ws.cols - 1,
                    y: full.y,
                }),
                &full_frame,
            ));
            assert!(!edge_actions.contains(&Action::WrapToggle));
            assert_eq!(
                full_frame.cells[usize::from(full.y * ws.cols + ws.cols - 1)].text,
                " "
            );

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
                Vec::<Action>::new()
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
fn neutral_tabs_ignore_legacy_palette_settings_and_numbers_focus() {
    use kelpmux::native::{
        ui::TabPosition,
        workspace::{TabTarget, color_cards, dialog_bounds, tab_number, tab_targets},
    };
    for size in [(40, 10), (120, 40), (240, 70)] {
        let mut ws = workspace(size, (2, 1));
        ws.ui.config.tab_position = TabPosition::Left;
        ws.apply(&Action::Select(1)).unwrap();
        let f = frame(&ws);
        let targets = tab_targets(f.cols, f.rows, &f.ui.config, 2, 0, 0);
        for index in 0..2 {
            let Some((_, rect)) = targets
                .iter()
                .find(|(target, _)| *target == TabTarget::Pane(index))
            else {
                continue;
            };
            let rect = *rect;
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
                        250
                    } else {
                        kelpmux::native::ui::TAB_BG
                    })
                );
            }
            assert_eq!(
                f.cells[usize::from(rect.y * f.cols + rect.x)].fg,
                Color::Indexed(if index == 0 { 250 } else { 244 })
            );
            if rect.rows >= 3 {
                let title = &f.cells[usize::from((rect.y + 1) * f.cols + rect.x + 2)];
                assert_eq!(title.text, "a");
                assert_eq!(title.fg, Color::Indexed(if index == 0 { 232 } else { 231 }));
                assert_eq!(
                    title.bg,
                    Color::Indexed(if index == 0 {
                        250
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
                vec![Action::Focus(index as u64 + 1)]
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
            assert_eq!(
                rendered.cells, f.cells,
                "legacy palette settings must not recolor neutral tabs"
            );
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
        assert_eq!(
            body.x,
            if position == TabPosition::Left {
                kelpmux::native::workspace::sidebar_width(f.cols)
            } else {
                0
            }
        );
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
fn tab_bodies_fill_available_width_despite_legacy_width_settings() {
    use kelpmux::native::{ui::TabPosition, workspace::tab_body};
    for position in [TabPosition::Left, TabPosition::Top, TabPosition::Bottom] {
        let mut ws = workspace((120, 40), (1, 1));
        ws.ui.config.tab_position = position;
        let available = if position == TabPosition::Left {
            ws.cols - kelpmux::native::workspace::sidebar_width(ws.cols)
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
            ws.ui.config = serde_json::from_value(serde_json::json!({
                "tab_position": position,
                "read_width": cap,
                "wrap_lines": true,
                "remembered_read_widths": [80, 85, 90]
            }))
            .unwrap();
            let capped = tab_body(ws.cols, ws.rows, &ws.ui.config, false, None);
            let full = tab_body(ws.cols, ws.rows, &ws.ui.config, true, None);
            assert_eq!(full.cols, available);
            assert_eq!(capped.cols, available);
            let saved = serde_json::to_value(&ws.ui.config).unwrap();
            for key in ["read_width", "wrap_lines", "remembered_read_widths"] {
                assert!(saved.get(key).is_none());
            }
        }
    }
}

#[test]
fn working_tabs_animate_only_one_character_and_keep_bells_beside_titles() {
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
            let tab = tab_targets(ws.cols, ws.rows, &ws.ui.config, 2, active - 1, 0)
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
            let b = compose(&ws, &screens, true, None, 2);
            assert_ne!(a.cells, b.cells);
            for (index, (before, after)) in a.cells.iter().zip(&b.cells).enumerate() {
                assert_eq!(before.bg, after.bg);
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
            assert_eq!(b.cells[bell].text, kelpmux::native::ui::ALERT_ICON);
            assert_eq!(b.cells[bell].width, 1);
            assert_eq!(b.cells[bell + 1].text, " ");
            let mut display = Terminal::new(ws.cols, ws.rows);
            display.feed(&terminal_diff(None, &a));
            assert_eq!(display.snapshot().cells, a.cells);
            display.feed(&terminal_diff(Some(&a), &b));
            assert_eq!(display.snapshot().cells, b.cells);
            ws.apply(&Action::Acknowledge(1)).unwrap();
            ws.panes[0].status = Status::Unknown;
            let stopped = compose(&ws, &screens, true, None, 16);
            assert_eq!(stopped.cells, compose(&ws, &screens, true, None, 32).cells);
            assert!(
                !stopped
                    .cells
                    .iter()
                    .any(|cell| cell.text == kelpmux::native::ui::ALERT_ICON)
            );
        }
    }
}

#[test]
fn tab_folder_subtitles_and_horizontal_bars_touch_terminal_content() {
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
        let tab = tab_targets(f.cols, f.rows, &f.ui.config, 2, 0, 0)
            .into_iter()
            .find(|(target, _)| *target == TabTarget::Pane(0))
            .unwrap()
            .1;
        let folder = usize::from(if position == TabPosition::Left {
            (tab.y + 2) * f.cols + tab.x + 2
        } else {
            tab.y * f.cols + tab.x + 7
        });
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
            let body = f.panes[0].body;
            assert_eq!(body.rows, f.rows - 3);
            if position == TabPosition::Top {
                assert_eq!(body.y, tab.y + tab.rows);
            } else {
                assert_eq!(body.y + body.rows, tab.y);
            }
        }
    }
}

#[test]
fn horizontal_tabs_have_two_rows_an_attached_add_button_and_footer_parked() {
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
                let targets = tab_targets(ws.cols, ws.rows, &ws.ui.config, count as usize, 0, 0);
                assert!(
                    targets
                        .iter()
                        .all(|(target, r)| r.rows
                            == if *target == TabTarget::Parked { 1 } else { 2 })
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
                for y in new.y..new.y + new.rows {
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
                        "▌"
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
                    let targets = tab_targets(f.cols, f.rows, &f.ui.config, count, active, 0);
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
                                if direction == TabTarget::Previous || rect.y == 0 {
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
                                .any(|(target, rect)| *target != TabTarget::New
                                    && rect.contains(Point { x: 1, y: new.y - 1 }))
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn neutral_focus_connects_sidebar_divider_regardless_of_legacy_palette() {
    use kelpmux::native::{
        ui::{PALETTE, TabPosition},
        workspace::{TabTarget, tab_number, tab_targets},
    };
    let mut ws = workspace((240, 40), (2, 1));
    ws.ui.config.tab_position = TabPosition::Left;
    ws.apply(&Action::Select(1)).unwrap();
    for color in 0..PALETTE.len() {
        ws.apply(&Action::TabColorSet {
            pane: 1,
            color: color as u8,
        })
        .unwrap();
        {
            let f = frame(&ws);
            let body = f.panes[0].body;
            let tab = tab_targets(f.cols, f.rows, &f.ui.config, 2, 0, 0)
                .into_iter()
                .find(|(t, _)| *t == TabTarget::Pane(0))
                .unwrap()
                .1;
            let number = tab_number(tab);
            assert_eq!(
                f.cells[usize::from(number.y * f.cols + number.x + 4)].bg,
                Color::Indexed(250)
            );
            for y in 0..f.rows - 1 {
                let cell = &f.cells[usize::from(y * f.cols + body.x - 1)];
                assert_eq!(cell.text, "▎");
                assert_eq!(cell.fg, Color::Indexed(250));
                assert_eq!(cell.bg, Color::Default);
            }
            assert_eq!(body.x + body.cols, f.cols);
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
        assert_eq!(ws.reading, ReadingMode::Grid);
        ws.apply(&Action::Read(1)).unwrap();
        let f = frame(&ws);
        let tab = tab_targets(f.cols, f.rows, &f.ui.config, 1, 0, 0)
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
        let help = f.controls()[3];
        let start = usize::from(help.y * f.cols + help.x);
        let at = start
            + f.cells[start..start + usize::from(help.cols)]
                .iter()
                .position(|cell| cell.text == "?")
                .unwrap();
        assert_eq!(f.cells[at].text, "?");
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
        let unread = frame(&ws);
        let alert = start
            + unread.cells[start..start + usize::from(help.cols)]
                .iter()
                .position(|cell| cell.text == kelpmux::native::ui::ALERT_ICON)
                .unwrap();
        assert_eq!(
            frame(&ws).cells[alert].text,
            kelpmux::native::ui::ALERT_ICON
        );
        assert_eq!(frame(&ws).cells[alert].bg, Color::Indexed(160));
        ws.apply(&Action::Acknowledge(1)).unwrap();
        assert_eq!(frame(&ws).cells[at].text, "?");
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
        let configured = frame(&ws);
        for (_, _, rect) in targets.iter().filter(|(index, _, _)| *index < 4) {
            assert_eq!(
                configured.cells[usize::from(rect.y * configured.cols + rect.x)].bg,
                Color::Indexed(238)
            );
        }
        for (_, value, rect) in &targets {
            assert!(rect.x >= bounds.x && rect.x + rect.cols <= bounds.x + bounds.cols);
            assert!(rect.y > bounds.y && rect.y + rect.rows <= bounds.y + bounds.rows);
            if let Some(value) = value {
                let current = ws.ui.config.notification_sound;
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
                    assert_eq!(messages, vec![Action::NotificationSoundToggle]);
                    for action in messages {
                        ws.apply(&action).unwrap();
                    }
                }
                assert_eq!(ws.ui.config.notification_sound, *value);
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
        for (dx, dy, expected) in [(1, 0, 3), (-1, 0, 2), (0, 1, 4), (0, 1, 4), (0, -1, 2)] {
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
        assert_eq!(ws.reading, ReadingMode::Fullscreen(1));
        assert!(ws.ui.config.notification_sound);
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
                    assert!(matches!(color, Color::Rgb(r, g, b) if r != g || g != b));
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
        for (control, symbol, color) in [(park, "⊟", 141), (close, "×", 203)] {
            let cell = &f.cells[usize::from(control.y * f.cols + control.x + 1)];
            assert_eq!(cell.text, symbol);
            assert_eq!(cell.fg, Color::Indexed(color));
            for x in control.x..control.x + control.cols {
                assert_eq!(
                    f.cells[usize::from(control.y * f.cols + x)].bg,
                    Color::Default
                );
            }
        }
        for (control, expected) in [
            (number, Action::Focus(id)),
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
    let parked = f.controls()[2];
    for x in parked.x..parked.x + parked.cols {
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
        let positions: Vec<_> = [0, 2, 4, 6, 8]
            .into_iter()
            .map(|tick| {
                let f = compose(&ws, &BTreeMap::new(), true, None, tick);
                let balls: Vec<_> = f
                    .cells
                    .iter()
                    .enumerate()
                    .filter(|(_, cell)| matches!(cell.text.as_str(), "⣤" | "⠶" | "⠛"))
                    .map(|(index, cell)| (index, cell.text.clone()))
                    .collect();
                assert_eq!(balls.len(), 1);
                balls[0].clone()
            })
            .collect();
        assert!(
            positions
                .iter()
                .all(|position| position.0 == positions[0].0)
        );
        assert_eq!(
            positions
                .iter()
                .map(|position| position.1.as_str())
                .collect::<Vec<_>>(),
            vec!["⣤", "⠶", "⠛", "⠶", "⣤"]
        );
        assert_eq!(positions[0], positions[4]);
    }
}

#[test]
fn tabs_keep_name_separate_from_metadata_and_controls_centered() {
    use kelpmux::native::{
        ui::TabPosition,
        workspace::{TabTarget, tab_close, tab_park, tab_targets},
    };
    for position in [TabPosition::Left, TabPosition::Top, TabPosition::Bottom] {
        let mut ws = workspace((240, 60), (2, 1));
        ws.ui.config.tab_position = position;
        ws.panes[0].title = "Conversation has room".into();
        ws.panes[0].cwd = "/repos/project".into();
        ws.apply(&Action::Read(1)).unwrap();
        let f = frame(&ws);
        let targets = tab_targets(f.cols, f.rows, &f.ui.config, 2, 0, 0);
        let tab = targets
            .iter()
            .find(|(target, _)| *target == TabTarget::Pane(0))
            .unwrap()
            .1;
        assert_eq!(tab.rows, if position == TabPosition::Left { 3 } else { 2 });
        let row: String = f.cells[usize::from((tab.y + 1) * f.cols + tab.x)
            ..usize::from((tab.y + 1) * f.cols + tab.x + tab.cols)]
            .iter()
            .map(|cell| cell.text.as_str())
            .collect();
        assert!(row.contains("Conversation has room"));
        let close = tab_close(tab);
        let park = tab_park(tab);
        assert_eq!(close.x + close.cols, tab.x + tab.cols);
        assert_eq!(park.x + park.cols, close.x);
        assert_eq!(close.y, tab.y);
        assert_eq!(park.y, tab.y);
        if position == TabPosition::Left {
            assert_eq!(close.x + close.cols, ws.layout()[0].body.x - 1);
            let start = usize::from(close.y * f.cols + close.x);
            assert_eq!(f.cells[start].text, " ");
            assert_eq!(f.cells[start + 1].text, "×");
            assert_eq!(f.cells[start + 2].text, " ");
            let end = targets
                .iter()
                .filter_map(|(target, rect)| {
                    matches!(target, TabTarget::Pane(_)).then_some(rect.y + rect.rows)
                })
                .max()
                .unwrap();
            assert_eq!(
                targets
                    .iter()
                    .find(|(target, _)| *target == TabTarget::New)
                    .unwrap()
                    .1
                    .y,
                end
            );
        }
    }
}

#[test]
fn tab_surfaces_follow_dark_and_light_backgrounds_without_repainting_terminal_defaults() {
    use kelpmux::native::{
        ui::TabPosition,
        workspace::{TabTarget, tab_number, tab_targets},
    };
    for position in [TabPosition::Left, TabPosition::Top, TabPosition::Bottom] {
        for (background, expected, light) in [
            ([12, 18, 24], Color::Rgb(26, 32, 38), false),
            ([246, 244, 240], Color::Rgb(232, 230, 226), true),
        ] {
            let mut ws = workspace((240, 60), (2, 1));
            ws.ui.config.tab_position = position;
            ws.ui.terminal_theme[1] = Some(background);
            ws.panes[0].title = "shell".into();
            ws.panes[1].status = Status::Working;
            ws.apply(&Action::Read(1)).unwrap();
            let f = frame(&ws);
            let point = match position {
                TabPosition::Left => Point {
                    x: 0,
                    y: f.rows / 2,
                },
                TabPosition::Bottom => Point {
                    x: f.cols - 1,
                    y: f.rows - 3,
                },
                _ => Point {
                    x: f.cols - 1,
                    y: 0,
                },
            };
            assert_eq!(
                f.cells[usize::from(point.y * f.cols + point.x)].bg,
                if position == TabPosition::Left {
                    expected
                } else {
                    Color::Indexed(234)
                }
            );
            let new = tab_targets(f.cols, f.rows, &f.ui.config, 2, 0, 0)
                .into_iter()
                .find(|(target, _)| *target == TabTarget::New)
                .unwrap()
                .1;
            let offset = if position == TabPosition::Left {
                14
            } else {
                24
            };
            let [r, g, b] = background.map(|value| {
                if light {
                    value.saturating_sub(offset)
                } else {
                    value.saturating_add(offset)
                }
            });
            assert_eq!(
                f.cells[usize::from(new.y * f.cols + new.x)].bg,
                if position == TabPosition::Left {
                    Color::Rgb(r, g, b)
                } else {
                    Color::Indexed(234)
                }
            );
            let inactive = tab_targets(f.cols, f.rows, &f.ui.config, 2, 0, 0)
                .into_iter()
                .find(|(target, _)| *target == TabTarget::Pane(1))
                .unwrap()
                .1;
            let selected = tab_targets(f.cols, f.rows, &f.ui.config, 2, 0, 0)
                .into_iter()
                .find(|(target, _)| *target == TabTarget::Pane(0))
                .unwrap()
                .1;
            let title_start = usize::from((selected.y + 1) * f.cols + selected.x + 2);
            let title = &f.cells[title_start..title_start + 5];
            assert_eq!(
                title
                    .iter()
                    .map(|cell| cell.text.as_str())
                    .collect::<String>(),
                "shell"
            );
            assert!(title.iter().all(|cell| cell.italic));
            let divider = match position {
                TabPosition::Left => Point {
                    x: f.panes[0].body.x - 1,
                    y: 0,
                },
                TabPosition::Top => Point { x: 0, y: 2 },
                TabPosition::Bottom => Point {
                    x: 0,
                    y: f.rows - 4,
                },
                TabPosition::Auto => unreachable!(),
            };
            if position == TabPosition::Left {
                assert_eq!(
                    title[0].bg,
                    f.cells[usize::from(divider.y * f.cols + divider.x)].fg
                );
            } else {
                assert_eq!(title[0].bg, Color::Indexed(250));
                assert_eq!(
                    f.cells[usize::from(inactive.y * f.cols + inactive.x)].bg,
                    Color::Indexed(234)
                );
            }
            let number = tab_number(inactive);
            assert_eq!(
                f.cells[usize::from(number.y * f.cols + number.x + 1)].fg,
                Color::Indexed(if position != TabPosition::Left {
                    250
                } else if light {
                    232
                } else {
                    231
                })
            );
            let ball = f.cells.iter().find(|cell| cell.text == "⣤").unwrap();
            assert_eq!(
                ball.fg,
                Color::Indexed(if position != TabPosition::Left {
                    250
                } else if light {
                    232
                } else {
                    231
                })
            );
            let body = f.panes.iter().find(|view| view.pane.id == 1).unwrap().body;
            assert_eq!(
                f.cells[usize::from(body.y * f.cols + body.x)].bg,
                Color::Default
            );
            ws.apply(&Action::Park(1)).unwrap();
            ws.apply(&Action::Park(2)).unwrap();
            ws.apply(&Action::Parked).unwrap();
            let parked = frame(&ws);
            let bounds = kelpmux::native::workspace::dialog_bounds_for(
                parked.cols,
                parked.rows,
                kelpmux::native::ui::DialogKind::Parked,
            );
            let card = kelpmux::native::workspace::parked_cards(bounds, 0, 2)[1].1;
            let title = &parked.cells[usize::from((card.y + 1) * parked.cols + card.x + 8)];
            assert_eq!(title.fg, Color::Indexed(if light { 232 } else { 231 }));
            assert_eq!(
                parked.cells[usize::from((card.y + 1) * parked.cols + card.x + 6)].fg,
                Color::Indexed(if light { 232 } else { 231 })
            );
        }
    }
}

#[test]
fn sidebar_status_controls_and_metrics_leave_the_last_terminal_row_usable() {
    use kelpmux::native::{
        ui::TabPosition,
        workspace::{TabTarget, sidebar_width, tab_targets},
    };
    for size in [(40, 10), (120, 40), (240, 60)] {
        for position in [TabPosition::Left, TabPosition::Top, TabPosition::Bottom] {
            let mut ws = workspace(size, (3, 3));
            ws.ui.config.tab_position = position;
            ws.ui.vitals = vec![
                ("cpu ▂ 51%".into(), 114),
                ("mem ▄ 100%".into(), 141),
                ("swap ▁ 0%".into(), 73),
                ("disk".into(), 222),
                ("◔ 20%".into(), 222),
                ("● 95%".into(), 196),
                ("2026-10-21".into(), 39),
                ("12:34".into(), 250),
            ];
            ws.panes[8].title = "Shell".into();
            ws.apply(&Action::Read(9)).unwrap();
            let body = ws
                .layout()
                .into_iter()
                .find(|view| view.pane.id == 9)
                .unwrap()
                .body;
            let mut terminal = Terminal::new(body.cols, body.rows);
            terminal.feed(format!("\x1b[{};1Hlast row", body.rows).as_bytes());
            let f = compose(
                &ws,
                &BTreeMap::from([(9, terminal.snapshot())]),
                true,
                None,
                0,
            );
            assert_eq!(body.y + body.rows == f.rows, position == TabPosition::Left);
            assert_eq!(
                f.cells[usize::from((body.y + body.rows - 1) * f.cols + body.x)].text,
                "l"
            );
            for (rect, expected) in f.controls().into_iter().zip([
                Action::Config,
                Action::Read(9),
                Action::Parked,
                Action::Help,
            ]) {
                assert!(!body.contains(Point {
                    x: rect.x,
                    y: rect.y
                }));
                assert_eq!(
                    actions(Router::default().route(
                        click(Point {
                            x: rect.x,
                            y: rect.y
                        }),
                        &f
                    )),
                    vec![expected]
                );
            }
            if position == TabPosition::Left {
                let status_top = f.rows - kelpmux::native::workspace::sidebar_status_rows(f.rows);
                let width = sidebar_width(f.cols) - 1;
                let utility: String = (status_top..f.rows)
                    .flat_map(|y| {
                        f.cells[usize::from(y * f.cols)..usize::from(y * f.cols + width)]
                            .iter()
                            .map(|cell| cell.text.as_str())
                    })
                    .collect();
                assert!(!utility.contains('^'));
                if f.rows >= 24 {
                    for (offset, label, value, color) in [
                        (1, "CPU", "51%", 114),
                        (2, "Memory", "100%", 141),
                        (3, "Swap", "0%", 73),
                        (4, "Disks", "◔ 20% ● 95%", 196),
                        (5, "Clock", "12:34", 250),
                    ] {
                        let start = usize::from((status_top + offset) * f.cols);
                        let row: String = f.cells[start..start + usize::from(width)]
                            .iter()
                            .map(|cell| cell.text.as_str())
                            .collect();
                        assert!(row.trim_start().starts_with(label), "{row:?}");
                        assert!(row.trim_end().ends_with(value), "{row:?}");
                        assert_eq!(
                            f.cells[start + usize::from(width - 2)].fg,
                            Color::Indexed(color)
                        );
                    }
                    for (offset, value) in [(1, "▂  51%"), (2, "▄ 100%"), (3, "▁   0%")] {
                        let start = usize::from((status_top + offset) * f.cols + width - 7);
                        assert_eq!(
                            f.cells[start..start + 6]
                                .iter()
                                .map(|cell| cell.text.as_str())
                                .collect::<String>(),
                            value
                        );
                    }
                    assert_eq!(
                        f.cells[usize::from((status_top + 4) * f.cols + width - 12)].fg,
                        Color::Indexed(222)
                    );
                    assert_eq!(
                        f.cells[usize::from((status_top + 4) * f.cols + width - 6)].fg,
                        Color::Indexed(196)
                    );
                    for (control, color) in f.controls().into_iter().zip([24, 67, 60, 94]) {
                        for x in control.x..control.x + control.cols {
                            assert_eq!(
                                f.cells[usize::from(control.y * f.cols + x)].bg,
                                Color::Indexed(color)
                            );
                        }
                    }
                    let notice = compose(
                        &ws,
                        &BTreeMap::new(),
                        true,
                        Some("Waiting for the workspace to reconnect safely"),
                        0,
                    );
                    for control in f.controls() {
                        let start = usize::from(control.y * f.cols + control.x);
                        assert_eq!(
                            &notice.cells[start..start + usize::from(control.cols)],
                            &f.cells[start..start + usize::from(control.cols)]
                        );
                    }
                    let mut display = Terminal::new(notice.cols, notice.rows);
                    display.feed(&terminal_diff(None, &notice));
                    assert_eq!(display.snapshot().cells, notice.cells);
                }
                let [config, mode, parked, help] = f.controls();
                for (left, right) in [(config, mode), (parked, help)] {
                    assert_eq!(left.x, 0);
                    assert_eq!(left.x + left.cols, right.x);
                    assert_eq!(right.x + right.cols, width);
                }
                let minimum = (size.0 / 5).clamp(18, 28);
                assert!((minimum..=minimum * 6 / 5).contains(&sidebar_width(size.0)));
                assert!(
                    f.cells
                        .iter()
                        .any(|cell| cell.text == "4" && cell.fg == Color::Indexed(250))
                );
                let targets = tab_targets(f.cols, f.rows, &f.ui.config, 9, 8, 0);
                for (target, rect) in &targets {
                    if matches!(target, TabTarget::Pane(_) | TabTarget::New) {
                        assert_eq!(rect.rows, 3);
                    }
                    for control in f.controls() {
                        if *target == TabTarget::Parked {
                            continue;
                        }
                        assert!(
                            !(rect.x < control.x + control.cols
                                && control.x < rect.x + rect.cols
                                && rect.y < control.y + control.rows
                                && control.y < rect.y + rect.rows),
                            "{target:?} overlaps a control"
                        );
                    }
                }
                let new = targets
                    .iter()
                    .find(|(target, _)| *target == TabTarget::New)
                    .unwrap()
                    .1;
                assert_eq!(
                    f.cells[usize::from((new.y + 1) * f.cols + new.x + (new.cols - 11) / 2 + 1)]
                        .text,
                    "+"
                );
                assert_eq!(
                    f.cells[usize::from((new.y + 1) * f.cols + new.x + 1)].text,
                    "─"
                );
                for y in new.y..new.y + new.rows {
                    for x in new.x..new.x + new.cols {
                        let cell = &f.cells[usize::from(y * f.cols + x)];
                        assert_eq!(cell.bg, f.cells[usize::from(status_top * f.cols)].bg);
                        assert!(!cell.bold);
                    }
                }
                let tab = targets
                    .iter()
                    .find(|(target, _)| *target == TabTarget::Pane(8))
                    .unwrap()
                    .1;
                let title_start = usize::from((tab.y + 1) * f.cols + tab.x + 2);
                let title = &f.cells[title_start..title_start + 5];
                assert_eq!(
                    title
                        .iter()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>(),
                    "shell"
                );
                assert!(
                    title
                        .iter()
                        .all(|cell| cell.italic && matches!(cell.fg, Color::Indexed(240 | 245)))
                );
            }
            let mut display = Terminal::new(f.cols, f.rows);
            display.feed(&terminal_diff(None, &f));
            assert_eq!(display.snapshot().cells, f.cells);
        }
    }
}

#[test]
fn footer_parked_help_order_has_no_new_tab_button() {
    for size in [(40, 10), (120, 40)] {
        for count in [0, 1, 9] {
            let mut ws = workspace(size, (3, 3));
            for pane in ws.panes.iter_mut().take(count) {
                pane.parked = true;
            }
            ws.active = if count == 9 { 0 } else { count as u64 + 1 };
            ws.reflow();
            let f = frame(&ws);
            let parked = f.controls()[2];
            let help = f.controls()[3];
            let footer: String = f.cells[usize::from((f.rows - 1) * f.cols)..]
                .iter()
                .map(|cell| cell.text.as_str())
                .collect();
            assert_eq!(footer.contains('+'), ws.active == 0 && !f.sidebar_status());
            assert!(footer.contains("Parked") && !footer.contains("(0)"));
            let label: String = f.cells[usize::from(parked.y * f.cols + parked.x)
                ..usize::from(parked.y * f.cols + parked.x + parked.cols)]
                .iter()
                .map(|cell| cell.text.as_str())
                .collect();
            let expected = if count == 0 {
                " ⊟ Parked ".into()
            } else {
                format!(" ⊟ Parked ({count}) ")
            };
            if f.sidebar_status() {
                assert_eq!(label.trim(), expected.trim());
            } else {
                assert_eq!(label, expected);
            }
            assert_eq!(parked.x, if f.sidebar_status() { 0 } else { 18 });
            if f.sidebar_status() {
                assert_eq!(parked.x + parked.cols, help.x);
                assert_eq!(f.controls()[1].x, help.x);
            } else {
                assert_eq!(parked.x + parked.cols, help.x);
            }
            for x in parked.x..parked.x + parked.cols {
                assert_eq!(
                    f.cells[usize::from(parked.y * f.cols + x)].bg,
                    Color::Indexed(kelpmux::native::ui::PARKED_BG)
                );
                assert_eq!(
                    actions(Router::default().route(click(Point { x, y: parked.y }), &f)),
                    vec![Action::Parked]
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
            assert_eq!(
                actions(Router::default().route(
                    click(Point {
                        x: help.x,
                        y: help.y
                    }),
                    &f
                )),
                vec![Action::Help]
            );
        }
    }
}

#[test]
fn empty_parked_list_has_a_centered_explanation_and_matching_header_without_table_columns() {
    use kelpmux::native::{
        ui::{DialogKind, PARKED_BG},
        workspace::{dialog_bounds_for, parked_button},
    };
    for size in [(40, 10), (120, 40), (240, 70)] {
        let mut ws = workspace(size, (1, 1));
        let original = frame(&ws);
        ws.apply(&Action::Parked).unwrap();
        let f = frame(&ws);
        let bounds = dialog_bounds_for(f.cols, f.rows, DialogKind::Parked);
        let footer = parked_button(f.rows, 0);
        assert_eq!(
            f.cells[usize::from(bounds.y * f.cols + bounds.x)].bg,
            Color::Indexed(PARKED_BG)
        );
        assert_eq!(
            original.cells[usize::from(footer.y * f.cols + footer.x)].bg,
            Color::Indexed(PARKED_BG)
        );
        let text: String = f.cells.iter().map(|cell| cell.text.as_str()).collect();
        assert!(text.contains("No parked tabs"));
        assert!(text.contains("Park tabs to keep jobs running."));
        let row = bounds.y + bounds.rows / 2;
        let x = bounds.x + 2 + (bounds.cols - 4 - 14) / 2;
        assert_eq!(f.cells[usize::from(row * f.cols + x)].text, "N");
        assert_eq!(
            actions(Router::default().route(InputEvent::Bytes(vec![27]), &f)),
            vec![Action::DialogClose]
        );
    }
}

#[test]
fn status_popup_metrics_clicks_views_search_and_small_screen_replay() {
    use kelpmux::native::{
        status::{self, Process, Snapshot, Sort, View},
        ui::{DialogKind, TabPosition},
        workspace::sidebar_status_rows,
    };
    for size in [(40, 10), (80, 24), (160, 50)] {
        for position in [TabPosition::Left, TabPosition::Top, TabPosition::Bottom] {
            let mut ws = workspace(size, (1, 1));
            ws.ui.config.tab_position = position;
            ws.apply(&Action::Select(1)).unwrap();
            ws.ui.vitals = vec![
                ("cpu ▂ 51%".into(), 114),
                ("mem ▄ 70%".into(), 141),
                ("disk".into(), 222),
                ("◔ 20%".into(), 222),
                ("12:34".into(), 250),
            ];
            let f = frame(&ws);
            let point = if f.sidebar_status() {
                let reserved = sidebar_status_rows(f.rows);
                Point {
                    x: 1,
                    y: f.rows - reserved + u16::from(reserved > 3),
                }
            } else {
                Point {
                    x: f.footer_cols() + 1,
                    y: f.rows - 1,
                }
            };
            assert_eq!(
                actions(Router::default().route(click(point), &f)),
                vec![Action::StatusOpen(Sort::Cpu)]
            );
            ws.apply(&Action::StatusOpen(Sort::Cpu)).unwrap();
            ws.ui.status = Snapshot {
                ready: true,
                cpus: vec![51, 100, 0, 25],
                memory_total: 1024 * 1024 * 1024,
                memory_used: 500 * 1024 * 1024,
                load: "0.1 0.2 0.3".into(),
                processes: (1..=100)
                    .map(|pid| Process {
                        pid,
                        cpu: Some(pid * 10),
                        command: format!("/usr/bin/worker{pid}"),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            };
            for view in [View::Main, View::Io] {
                ws.apply(&Action::StatusView(view)).unwrap();
                let f = frame(&ws);
                let bounds = status::bounds(f.cols, f.rows);
                assert_eq!(ws.ui.dialog.unwrap().kind, DialogKind::Status);
                assert_eq!(
                    f.cells[usize::from(bounds.y * f.cols + bounds.x + 1)].text,
                    "S"
                );
                let mut terminal = Terminal::new(f.cols, f.rows);
                terminal.feed(&terminal_diff(None, &f));
                assert_eq!(terminal.snapshot().cells, f.cells);
                let mut router = Router::default();
                ws.apply(&Action::StatusScroll(i16::MAX)).unwrap();
                let last = ws.ui.status_offset;
                assert!(last > 0);
                ws.apply(&Action::StatusScroll(-1)).unwrap();
                assert_eq!(ws.ui.status_offset, last - 1);
                for key in [b"\x1b[A".as_slice(), b"\x1b[B", b"\x1b[5~", b"\t"] {
                    assert!(
                        router
                            .route(InputEvent::Bytes(key.to_vec()), &f)
                            .iter()
                            .all(|message| matches!(message, ClientMessage::Action(_)))
                    );
                }
                let query_actions =
                    actions(router.route(InputEvent::Bytes(b"/worker99".to_vec()), &f));
                assert_eq!(query_actions, vec![Action::StatusQuery("worker99".into())]);
                let mut fragmented = Router::default();
                fragmented.route(InputEvent::Bytes(b"/".to_vec()), &f);
                assert!(
                    fragmented
                        .route(InputEvent::Bytes(vec![0xc3]), &f)
                        .is_empty()
                );
                assert_eq!(
                    actions(fragmented.route(InputEvent::Bytes(vec![0xa9]), &f)),
                    vec![Action::StatusQuery("é".into())]
                );
                assert_eq!(
                    actions(fragmented.route(InputEvent::Bytes(b"x".to_vec()), &f)),
                    vec![Action::StatusQuery("éx".into())]
                );
                assert_eq!(
                    actions(fragmented.route(InputEvent::Paste(b"yz".to_vec()), &f)),
                    vec![Action::StatusQuery("éxyz".into())]
                );
                for action in query_actions {
                    ws.apply(&action).unwrap();
                }
                assert_eq!(
                    status::processes(&ws.ui.status, &ws.ui.status_query, Sort::Cpu, true).len(),
                    1
                );
                assert_eq!(
                    actions(router.route(InputEvent::Bytes(b"\x1b".to_vec()), &f)),
                    vec![Action::DialogClose]
                );
                let mut observer = f.clone();
                observer.controller = false;
                assert!(
                    Router::default()
                        .route(
                            click(Point {
                                x: bounds.x,
                                y: bounds.y
                            }),
                            &observer
                        )
                        .is_empty()
                );
                ws.ui.status_query.clear();
            }
            ws.apply(&Action::DialogClose).unwrap();
            assert_eq!(ws.reading, ReadingMode::Tabs(1));
        }
    }
}

#[test]
fn closing_status_consumes_the_mouse_release_before_the_app_can_receive_it() {
    use kelpmux::native::status::Sort;
    let mut ws = workspace((120, 40), (1, 1));
    ws.apply(&Action::Select(1)).unwrap();
    ws.apply(&Action::StatusOpen(Sort::Cpu)).unwrap();
    let f = frame(&ws);
    let mut router = Router::default();
    let point = Point { x: 0, y: 0 };
    assert_eq!(
        actions(router.route(click(point), &f)),
        vec![Action::DialogClose]
    );
    ws.apply(&Action::DialogClose).unwrap();
    let mut closed = frame(&ws);
    closed.panes[0].modes.mouse_click = true;
    assert!(
        router
            .route(
                InputEvent::Mouse(MouseEvent {
                    position: point,
                    kind: MouseKind::Release,
                    button: 0,
                    shift: false,
                    alt: false,
                    ctrl: false
                }),
                &closed
            )
            .is_empty()
    );
    assert_eq!(
        kelpmux::native::status::metric_target(
            &closed,
            Point {
                x: closed.cols,
                y: closed.rows - 1
            }
        ),
        None
    );
}

#[test]
fn status_summary_is_compact_uses_two_columns_and_draws_horizontal_meters() {
    use kelpmux::native::status::{self, Disk, Snapshot, Sort};
    for size in [(80, 30), (120, 40), (240, 70)] {
        let mut ws = workspace(size, (1, 1));
        ws.apply(&Action::StatusOpen(Sort::Cpu)).unwrap();
        ws.ui.vitals = vec![("2026-10-22".into(), 39), ("12:34".into(), 250)];
        ws.ui.status = Snapshot {
            ready: true,
            cpus: vec![0, 51, 100, 25],
            memory_used: 50,
            memory_total: 100,
            swap_used: 25,
            swap_total: 100,
            disks: vec![
                Disk {
                    mount: "/".into(),
                    used: 20,
                    total: 100,
                    percent: 20,
                },
                Disk {
                    mount: "/data".into(),
                    used: 90,
                    total: 100,
                    percent: 90,
                },
            ],
            ..Default::default()
        };
        let f = frame(&ws);
        let bounds = status::bounds(f.cols, f.rows);
        let height = status::summary_height(bounds, &ws.ui.status);
        assert_eq!(height, if size.0 >= 120 { 7 } else { 12 });
        let row_text = |y: u16| {
            f.cells[usize::from(y * f.cols + bounds.x + 1)
                ..usize::from(y * f.cols + bounds.x + bounds.cols - 1)]
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>()
        };
        for y in bounds.y + 1..bounds.y + 1 + height {
            assert!(!row_text(y).trim().is_empty(), "empty metric row {y}");
        }
        let date_row = (bounds.y + 1..bounds.y + 1 + height)
            .map(row_text)
            .find(|row| row.contains("2026-10-22"))
            .unwrap();
        assert!(date_row.contains("2026-10-22 12:34 · Uptime 0d 00:00"));
        let tabs_y = bounds.y + 1 + height;
        assert!(row_text(tabs_y).starts_with("[ Main ]"));
        assert!(row_text(status::table_y(bounds, &ws.ui.status)).contains("CPU%"));
        let meter_width = if status::summary_columns(bounds) == 2 {
            (bounds.cols - 5) / 2
        } else {
            bounds.cols - 2
        };
        for (index, color, percent) in [
            (0, 114, 0u16),
            (1, 114, 51),
            (2, 114, 100),
            (4, 141, 50),
            (5, 73, 25),
        ] {
            let start = usize::from((bounds.y + 1 + index) * f.cols + bounds.x + 1);
            let cells = &f.cells[start..start + usize::from(meter_width)];
            let text: String = cells.iter().map(|cell| cell.text.as_str()).collect();
            assert!(!text.contains(['█', '░']));
            let left = cells.iter().position(|cell| cell.text == "[").unwrap();
            let right = cells.iter().position(|cell| cell.text == "]").unwrap();
            assert_eq!(right, cells.len() - 1);
            let reading = format!("{percent:>3}%");
            assert!(text.contains(&reading));
            let value_width = if index == 4 {
                " 50% 50B/100B".len()
            } else if index == 5 {
                " 25% 25B/100B".len()
            } else {
                reading.len()
            };
            let visible_width = right - left - 1 - value_width;
            let filled = cells.iter().filter(|cell| cell.text == "|").count();
            assert!(visible_width >= 8);
            assert_eq!(
                filled,
                (((right - left - 1) * usize::from(percent) + 50) / 100).min(visible_width)
            );
            assert!(
                cells
                    .iter()
                    .filter(|cell| cell.text == "|")
                    .all(|cell| cell.fg == Color::Indexed(color))
            );
        }
        if size.0 >= 120 {
            let right = bounds.x + 1 + meter_width + 3;
            let start = usize::from((bounds.y + 1) * f.cols + right);
            let text = f.cells[start..start + 5]
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>();
            assert_eq!(text, "Tasks");
            assert!(height < 12);
        }
        let mut terminal = Terminal::new(f.cols, f.rows);
        terminal.feed(&terminal_diff(None, &f));
        assert_eq!(terminal.snapshot().cells, f.cells);
        let mut router = Router::default();
        assert_eq!(
            actions(router.route(
                click(Point {
                    x: bounds.x + 2,
                    y: status::table_y(bounds, &ws.ui.status)
                }),
                &f
            ))
            .len(),
            1
        );
    }
}
