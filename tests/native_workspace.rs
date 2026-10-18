use kelpmux::{
    model::Status,
    native::{
        input::{Decoder, Router},
        protocol::*,
        workspace::Workspace,
    },
};

fn pane(id: u64) -> PaneInfo {
    PaneInfo {
        id,
        number: id as usize,
        pid: 1,
        cwd: "/".into(),
        title: format!("pane {id}"),
        palette: id as u8,
        status: Status::Unknown,
        session_id: None,
        dead: false,
        ..Default::default()
    }
}
fn workspace(cols: u16, rows: u16, shape: (u8, u8)) -> Workspace {
    let mut workspace = Workspace::new(cols, rows).unwrap();
    workspace.panes = (1..=Workspace::grid_count(shape.0, shape.1).unwrap() as u64)
        .map(pane)
        .collect();
    workspace.active = 1;
    workspace
        .apply(&Action::Grid {
            cols: shape.0,
            rows: shape.1,
        })
        .unwrap();
    workspace
}
fn frame(workspace: &Workspace) -> Frame {
    Frame {
        cols: workspace.cols,
        rows: workspace.rows,
        cells: vec![],
        cursor: None,
        panes: workspace.layout(),
        active: workspace.active,
        reading: workspace.reading,
        controller: true,
        help: workspace.help,
        notice: None,
        ui: workspace.ui.clone(),
    }
}
fn action(messages: &[ClientMessage]) -> Vec<Action> {
    messages
        .iter()
        .filter_map(|m| {
            if let ClientMessage::Action(a) = m {
                Some(a.clone())
            } else {
                None
            }
        })
        .collect()
}
fn mouse(position: Point, kind: MouseKind) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        position,
        kind,
        button: 0,
        shift: false,
        ctrl: false,
        alt: false,
    })
}
#[test]
fn terminal_theme_replies_are_bounded_fragment_safe_and_never_application_input() {
    let wire = b"\x1b]10;rgb:d0d0/d8d8/e0e0\x1b\\\x1b]11;#1f2021\x07";
    let expected = vec![
        InputEvent::ThemeColor {
            background: false,
            rgb: [208, 216, 224],
        },
        InputEvent::ThemeColor {
            background: true,
            rgb: [31, 32, 33],
        },
    ];
    for split in 0..=wire.len() {
        let mut decoder = Decoder::default();
        let mut events = decoder.feed(&wire[..split]);
        events.extend(decoder.feed(&wire[split..]));
        assert_eq!(events, expected);
        assert!(decoder.flush().is_empty());
    }
    for wire in [
        b"\x1b]11;rgb:f/8/0\x07".as_slice(),
        b"\x1b]11;#f80\x1b\\".as_slice(),
    ] {
        assert_eq!(
            Decoder::default().feed(wire),
            vec![InputEvent::ThemeColor {
                background: true,
                rgb: [255, 136, 0]
            }]
        );
    }
    for wire in [
        b"\x1b]11;rgb:00/no/11\x07".as_slice(),
        b"\x1b]11;rgb:fffff/0/0\x07".as_slice(),
        b"\x1b]12;#123456\x07".as_slice(),
    ] {
        assert!(Decoder::default().feed(wire).is_empty());
    }
    let mut decoder = Decoder::default();
    assert!(decoder.feed(b"\x1b]11;rgb:1234/").is_empty());
    assert!(decoder.flush().is_empty());
    assert_eq!(
        decoder.feed(b"ffff/ffff\x07z"),
        vec![InputEvent::Bytes(b"z".to_vec())]
    );
    let mut broken = Decoder::default();
    broken.feed(b"\x1b]11;rgb:1234/");
    broken.flush();
    assert_eq!(
        broken.feed(b"\x02d"),
        vec![InputEvent::Bytes(b"\x02d".to_vec())]
    );
    let oversized = [b"\x1b]11;".as_slice(), &vec![b'x'; 1000], b"\x1b\\ok"].concat();
    assert_eq!(
        decoder.feed(&oversized),
        vec![InputEvent::Bytes(b"ok".to_vec())]
    );
    let paste = [b"\x1b[200~".as_slice(), wire, b"\x1b[201~"].concat();
    assert_eq!(
        Decoder::default().feed(&paste),
        vec![InputEvent::Paste(wire.to_vec())]
    );
    let mut ws = workspace(80, 24, (1, 1));
    for modal in [Action::Help, Action::SearchOpen] {
        ws.apply(&modal).unwrap();
        for event in &expected {
            assert!(matches!(
                &Router::default().route(event.clone(), &frame(&ws))[..],
                [ClientMessage::ThemeColor { .. }]
            ));
        }
    }
}

#[test]
fn every_grid_balances_cells_keeps_header_footer_and_all_panes() {
    for (cols, rows) in [(40, 10), (121, 41), (400, 160)] {
        for shape in kelpmux::native::workspace::GRIDS {
            let mut ws = workspace(cols, rows, shape);
            let views = ws.layout();
            assert_eq!(views.len(), (shape.0 * shape.1) as usize);
            for view in &views {
                assert!(view.body.cols > 0 && view.body.rows > 0);
                assert_eq!(view.body.y, view.rect.y + 1);
                assert_eq!(view.body.rows, view.rect.rows - 1);
                assert!(view.rect.x + view.rect.cols <= cols);
                assert!(view.rect.y + view.rect.rows < rows);
            }
            if views.len() > 1 && views[0].rect.y == views[1].rect.y {
                assert_eq!(views[0].rect.x + views[0].rect.cols + 1, views[1].rect.x);
                assert!(views[0].rect.cols.abs_diff(views[1].rect.cols) <= 1);
            }
            ws.resize(40, 10).unwrap();
            assert_eq!(ws.panes.len(), views.len());
            assert!(
                ws.grid_layout()
                    .iter()
                    .all(|v| v.rect.x + v.rect.cols <= 40)
            );
        }
    }
}
#[test]
fn tabs_resize_only_selected_terminal_and_restore_grid() {
    let mut ws = workspace(200, 80, (2, 2));
    let base = ws.terminal_sizes();
    ws.apply(&Action::Read(2)).unwrap();
    let views = ws.layout();
    assert_eq!(views.len(), 4);
    assert_eq!(
        views.iter().find(|v| v.pane.id == 2).unwrap().body.cols,
        120
    );
    assert!(views.iter().all(|view| !view.overlay));
    assert_eq!(views.iter().filter(|view| view.body.cols > 0).count(), 1);
    let capped = ws.terminal_sizes();
    assert_eq!(capped.len(), 4);
    for old in &base {
        if old.0 != 2 {
            assert!(capped.contains(old));
        }
    }
    ws.apply(&Action::Fullscreen(2)).unwrap();
    assert_eq!(
        ws.layout()
            .iter()
            .find(|v| v.pane.id == 2)
            .unwrap()
            .body
            .cols,
        171
    );
    assert_eq!(
        ws.layout()
            .iter()
            .find(|v| v.pane.id == 2)
            .unwrap()
            .body
            .rows,
        79
    );
    ws.apply(&Action::Fullscreen(2)).unwrap();
    assert_eq!(ws.terminal_sizes(), capped);
    ws.apply(&Action::CloseReading).unwrap();
    assert_eq!(ws.terminal_sizes(), base);
}
#[test]
fn selection_focus_swap_and_invalid_actions_preserve_identity() {
    let mut ws = workspace(80, 24, (2, 2));
    ws.apply(&Action::Select(2)).unwrap();
    assert_eq!(ws.reading, ReadingMode::Tabs(2));
    ws.apply(&Action::Focus(3)).unwrap();
    assert_eq!(ws.reading, ReadingMode::Tabs(3));
    ws.apply(&Action::Select(3)).unwrap();
    assert_eq!(ws.reading, ReadingMode::Tabs(3));
    ws.apply(&Action::Swap {
        source: 1,
        target: 4,
    })
    .unwrap();
    assert_eq!(ws.panes[0].id, 4);
    assert_eq!(ws.panes[0].palette, 4);
    assert_eq!(ws.panes[0].number, 1);
    ws.apply(&Action::Select(1)).unwrap();
    assert_eq!(ws.active, 4);
    let before = format!("{ws:?}");
    for invalid in [
        Action::Focus(99),
        Action::Read(99),
        Action::Fullscreen(99),
        Action::Select(0),
        Action::Select(9),
        Action::Swap {
            source: 1,
            target: 99,
        },
        Action::Copy(99),
        Action::Grid { cols: 1, rows: 1 },
        Action::Grid { cols: 3, rows: 1 },
    ] {
        assert!(ws.apply(&invalid).is_err());
        assert_eq!(format!("{ws:?}"), before);
    }
    assert!(ws.resize(39, 10).is_err());
    assert_eq!(format!("{ws:?}"), before);
}

#[test]
fn tabs_toggle_exits_fullscreen_and_navigation_never_sends_application_clicks() {
    let mut ws = workspace(240, 40, (2, 1));
    ws.apply(&Action::Fullscreen(1)).unwrap();
    ws.apply(&Action::Read(1)).unwrap();
    assert_eq!(ws.reading, ReadingMode::Grid);
    ws.apply(&Action::Read(1)).unwrap();
    let f = frame(&ws);
    let (_, rect) =
        kelpmux::native::workspace::tab_targets(f.cols, f.rows, &f.ui.config, f.panes.len(), 0)
            .into_iter()
            .find(|(target, _)| *target == kelpmux::native::workspace::TabTarget::Pane(1))
            .unwrap();
    let point = Point {
        x: rect.x + 7,
        y: rect.y,
    };
    let mut router = Router::default();
    assert_eq!(
        action(&router.route(mouse(point, MouseKind::Press), &f)),
        vec![Action::Focus(2)]
    );
}

#[test]
fn help_close_button_and_footer_toggle_close_without_application_input() {
    let mut ws = workspace(100, 30, (1, 1));
    ws.help = true;
    let f = frame(&ws);
    let mut router = Router::default();
    let bounds = kelpmux::native::input::help_bounds(f.cols, f.rows);
    for point in [
        Point {
            x: bounds.x + bounds.cols - 2,
            y: bounds.y,
        },
        Point { x: 15, y: 29 },
    ] {
        let messages = router.route(mouse(point, MouseKind::Press), &f);
        assert_eq!(action(&messages), vec![Action::Help]);
        assert_eq!(messages.len(), 1);
    }
}

#[test]
fn application_cursor_mode_translates_arrows_but_preserves_word_shortcuts() {
    let ws = workspace(80, 24, (1, 1));
    let mut f = frame(&ws);
    f.panes[0].modes.application_cursor = true;
    let mut router = Router::default();
    let messages = router.route(InputEvent::Bytes(b"\x1b[A\x1bf\x1b[1;3C".to_vec()), &f);
    assert!(
        matches!(&messages[..], [ClientMessage::Input(bytes)] if bytes == b"\x1bOA\x1bf\x1b[1;3C")
    );
}

#[test]
fn focus_reports_only_reach_applications_that_requested_them() {
    let ws = workspace(80, 24, (1, 1));
    let mut f = frame(&ws);
    let mut router = Router::default();
    assert!(
        router
            .route(InputEvent::Bytes(b"\x1b[I\x1b[O".to_vec()), &f)
            .is_empty()
    );
    f.panes[0].modes.focus = true;
    assert!(
        matches!(&router.route(InputEvent::Bytes(b"\x1b[I\x1b[O".to_vec()), &f)[..], [ClientMessage::Input(bytes)] if bytes == b"\x1b[I\x1b[O")
    );
}

#[test]
fn help_keys_do_not_leak_into_the_application_before_a_new_frame_arrives() {
    let f = frame(&workspace(80, 24, (1, 1)));
    let mut router = Router::default();
    let messages = router.route(InputEvent::Bytes(b"\x02?qhello".to_vec()), &f);
    assert_eq!(action(&messages), vec![Action::Help, Action::Help]);
    assert!(matches!(messages.last(), Some(ClientMessage::Input(bytes)) if bytes == b"hello"));
    assert_eq!(
        action(&router.route(InputEvent::Bytes(b"\x02?".to_vec()), &f)),
        vec![Action::Help]
    );
    assert!(
        router
            .route(InputEvent::Paste(b"no leak".to_vec()), &f)
            .is_empty()
    );
    assert_eq!(
        action(&router.route(InputEvent::Bytes(b"q".to_vec()), &f)),
        vec![Action::Help]
    );
}
#[test]
fn decoder_handles_fragmented_mouse_paste_and_escape_timeout() {
    let mut decoder = Decoder::default();
    assert!(decoder.feed(b"\x1b").is_empty());
    assert!(decoder.feed(b"[<0;12;").is_empty());
    let event = decoder.feed(b"7M");
    assert!(matches!(
        &event[..],
        [InputEvent::Mouse(MouseEvent {
            position: Point { x: 11, y: 6 },
            kind: MouseKind::Press,
            ..
        })]
    ));
    let payload = b"hello\x02z\x1bf\x1b[<0;1;1M";
    let mut stream = b"\x1b[200~".to_vec();
    stream.extend_from_slice(payload);
    stream.extend_from_slice(b"\x1b[201~");
    let mut events = vec![];
    for byte in stream {
        events.extend(decoder.feed(&[byte]));
    }
    assert_eq!(events, vec![InputEvent::Paste(payload.to_vec())]);
    assert!(decoder.feed(b"\x1b").is_empty());
    assert_eq!(decoder.flush(), vec![InputEvent::Bytes(vec![27])]);
    assert!(decoder.feed(b"\x1b[200~unterminated").is_empty());
    assert!(decoder.flush().is_empty());
}
#[test]
fn paste_is_bounded_without_leaking_excess_as_shortcuts() {
    let mut decoder = Decoder::default();
    let mut input = b"\x1b[200~".to_vec();
    input.extend(vec![b'x'; MAX_INPUT + 100]);
    input.extend_from_slice(b"\x02z\x1b[201~");
    let events = decoder.feed(&input);
    assert!(matches!(&events[..], [InputEvent::Paste(p)] if p.len() == MAX_INPUT));
}
#[test]
fn prefix_sequences_and_application_editing_keys_are_separate() {
    let ws = workspace(80, 24, (2, 2));
    let frame = frame(&ws);
    let mut router = Router::default();
    assert!(router.route(InputEvent::Bytes(vec![2]), &frame).is_empty());
    assert_eq!(
        action(&router.route(InputEvent::Bytes(b"2".to_vec()), &frame)),
        vec![Action::Select(2)]
    );
    let keys = b"\x1bf\x1bb\x1b[1;3D\x1b[1;5C\x1b[13;2u\x0a";
    let mut decoder = Decoder::default();
    let messages: Vec<_> = decoder
        .feed(keys)
        .into_iter()
        .flat_map(|event| router.route(event, &frame))
        .collect();
    let passthrough: Vec<_> = messages
        .iter()
        .flat_map(|m| {
            if let ClientMessage::Input(b) = m {
                b.clone()
            } else {
                panic!("application keys became action")
            }
        })
        .collect();
    assert_eq!(passthrough, b"\x1bf\x1bb\x1b[1;3D\x1b[1;5C\n\n");
    assert_eq!(
        action(&router.route(InputEvent::Bytes(b"\x02s14".to_vec()), &frame)),
        vec![Action::MovePicker]
    );
    assert!(
        router
            .route(InputEvent::Paste(b"\x02z".to_vec()), &frame)
            .is_empty()
    );
}
#[test]
fn grid_headers_and_tab_footer_have_distinct_hit_targets() {
    let mut ws = workspace(160, 40, (2, 1));
    let mut router = Router::default();
    let f = frame(&ws);
    let second = &f.panes[1];
    assert_eq!(
        action(&router.route(
            mouse(
                Point {
                    x: second.rect.x + second.rect.cols - 5,
                    y: 0
                },
                MouseKind::Press
            ),
            &f
        )),
        vec![Action::Focus(2)]
    );
    ws.apply(&Action::Read(1)).unwrap();
    let f = frame(&ws);
    assert_eq!(
        action(&router.route(
            mouse(
                Point {
                    x: 15,
                    y: f.rows - 1
                },
                MouseKind::Press
            ),
            &f
        )),
        vec![Action::Help]
    );
}
#[test]
fn selection_is_local_and_observers_cannot_inject() {
    let ws = workspace(80, 24, (2, 1));
    let mut f = frame(&ws);
    let mut router = Router::default();
    let body = f.panes[1].body;
    assert_eq!(
        action(&router.route(
            mouse(
                Point {
                    x: body.x + 2,
                    y: body.y + 1
                },
                MouseKind::Press
            ),
            &f
        )),
        vec![
            Action::Focus(2),
            Action::SelectText {
                pane: 2,
                start: Point { x: 2, y: 1 },
                end: Point { x: 2, y: 1 }
            }
        ]
    );
    let released = action(&router.route(
        mouse(
            Point {
                x: body.x + 5,
                y: body.y + 1,
            },
            MouseKind::Release,
        ),
        &f,
    ));
    assert_eq!(released.last(), Some(&Action::Copy(2)));
    f.controller = false;
    assert!(
        router
            .route(InputEvent::Bytes(b"a\x02z\x1b1".to_vec()), &f)
            .is_empty()
    );
    assert!(
        router
            .route(InputEvent::Paste(b"a".to_vec()), &f)
            .is_empty()
    );
    assert!(
        router
            .route(mouse(Point { x: 2, y: 2 }, MouseKind::Press), &f)
            .is_empty()
    );
    assert_eq!(
        action(&router.route(InputEvent::Bytes(b"\x02t\x02d".to_vec()), &f)),
        vec![Action::Takeover, Action::Detach]
    );
}

#[test]
fn app_mouse_forwarding_shift_selection_wheel_and_help() {
    let ws = workspace(80, 24, (1, 1));
    let mut f = frame(&ws);
    f.panes[0].modes.mouse_click = true;
    let mut router = Router::default();
    let press = MouseEvent {
        position: Point { x: 5, y: 5 },
        kind: MouseKind::Press,
        button: 0,
        shift: false,
        alt: false,
        ctrl: false,
    };
    let messages = router.route(InputEvent::Mouse(press), &f);
    assert_eq!(action(&messages), vec![Action::Focus(1)]);
    assert!(matches!(messages.last(), Some(ClientMessage::Mouse(m)) if *m == press));
    let wheel = MouseEvent {
        kind: MouseKind::WheelUp,
        ..press
    };
    assert!(matches!(
        &router.route(InputEvent::Mouse(wheel), &f)[..],
        [ClientMessage::Mouse(_)]
    ));
    assert_eq!(
        action(&router.route(
            InputEvent::Mouse(MouseEvent {
                shift: true,
                ..wheel
            }),
            &f
        )),
        vec![Action::Scroll { pane: 1, lines: 3 }]
    );
    assert!(matches!(
        action(&router.route(
            InputEvent::Mouse(MouseEvent {
                shift: true,
                ..press
            }),
            &f
        ))
        .last(),
        Some(Action::SelectText { pane: 1, .. })
    ));
    f.help = true;
    assert!(
        router
            .route(
                InputEvent::Bytes(b"ordinary application bytes".to_vec()),
                &f
            )
            .is_empty()
    );
    assert_eq!(
        action(&router.route(InputEvent::Bytes(vec![27]), &f)),
        vec![Action::Help]
    );
    // A separate open-help interaction; the preceding Escape already closed it.
    let mut router = Router::default();
    assert_eq!(
        action(&router.route(mouse(Point { x: 0, y: 0 }, MouseKind::Press), &f)),
        vec![Action::Help]
    );
}

#[test]
fn decoder_preserves_utf8_and_fragmented_word_navigation() {
    let mut decoder = Decoder::default();
    let ws = workspace(80, 24, (1, 1));
    let f = frame(&ws);
    let mut router = Router::default();
    let original = "héllo\u{1b}f\u{1b}[1;3D".as_bytes();
    let mut result = Vec::new();
    for byte in original {
        for event in decoder.feed(&[*byte]) {
            for message in router.route(event, &f) {
                if let ClientMessage::Input(bytes) = message {
                    result.extend(bytes);
                } else {
                    panic!("word navigation routed to a workspace action");
                }
            }
        }
    }
    assert_eq!(result, original);
}
