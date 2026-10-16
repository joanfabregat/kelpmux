use kelpmux::native::{
    engine::{Effect, Terminal},
    protocol::*,
    render::{compose, terminal_diff},
    workspace::Workspace,
};
use std::collections::BTreeMap;
#[test]
fn pane_palette_replies_match_opaque_defaults_and_faint_composer_background() {
    let mut pane = Terminal::for_pane(40, 8);
    let effects = pane.feed(b"\x1b[6n\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b[?u\x1b[c");
    let replies: Vec<u8> = effects
        .into_iter()
        .filter_map(|e| match e {
            Effect::WritePty(bytes) => Some(bytes),
            _ => None,
        })
        .flatten()
        .collect();
    let replies = String::from_utf8(replies).unwrap();
    assert!(replies.contains("rgb:e5e5/e5e5/e5e5"));
    assert!(replies.contains("rgb:0000/0000/0000"));
    assert!(replies.contains("\x1b[?0u"));
    // Codex-style low-contrast shading, including erased/blank cells.
    for byte in b"\x1b[3;1H\x1b[48;2;31;31;31m\x1b[2K Input \x1b[0m" {
        pane.feed(&[*byte]);
    }
    let source = pane.snapshot();
    assert_eq!(source.cells[0].bg, Color::Rgb(0, 0, 0));
    assert_eq!(source.cells[2 * 40 + 30].bg, Color::Rgb(31, 31, 31));
    let mut display = Terminal::new(40, 8);
    display.feed(b"\x1b]10;rgb:00/00/00\x07\x1b]11;rgb:1f/1f/1f\x07");
    display.feed(&terminal_diff(None, &frame(source.clone())));
    assert_eq!(display.snapshot().cells, source.cells);
    assert_ne!(
        display.snapshot().cells[0].bg,
        display.snapshot().cells[2 * 40].bg
    );
}

#[test]
fn enhanced_keyboard_modes_report_push_and_pop_across_fragmented_output() {
    let mut terminal = Terminal::new(40, 8);
    let mut replies = Vec::new();
    for byte in b"\x1b[>3u\x1b[?u" {
        for effect in terminal.feed(&[*byte]) {
            if let Effect::WritePty(bytes) = effect {
                replies.extend(bytes);
            }
        }
    }
    assert_eq!(terminal.snapshot().modes.keyboard_flags, 3);
    assert_eq!(replies, b"\x1b[?3u");
    terminal.feed(b"\x1b[<1u");
    assert_eq!(terminal.snapshot().modes.keyboard_flags, 0);
}
fn line(screen: &Screen, y: usize) -> String {
    screen.cells[y * usize::from(screen.cols)..(y + 1) * usize::from(screen.cols)]
        .iter()
        .map(|c| c.text.as_str())
        .collect::<String>()
        .trim_end()
        .to_owned()
}
fn frame(screen: Screen) -> Frame {
    Frame {
        cols: screen.cols,
        rows: screen.rows,
        cells: screen.cells,
        cursor: screen.cursor,
        panes: vec![],
        active: 0,
        reading: ReadingMode::Grid,
        controller: true,
        help: false,
        notice: None,
        ui: Default::default(),
    }
}
#[test]
fn fragmented_unicode_ansi_colors_and_modes() {
    let mut terminal = Terminal::new(20, 4);
    let input = "\x1b[38;2;12;34;56m界e\u{301}\x1b[48;5;42mX\x1b[?1h\x1b=\x1b[?2004h\x1b[?1002h\x1b[?1006h\x1b[?1004h\x1b[?25l";
    for byte in input.bytes() {
        terminal.feed(&[byte]);
    }
    let screen = terminal.snapshot();
    assert_eq!(screen.cells.len(), 80);
    assert_eq!(screen.cells[0].text, "界");
    assert_eq!(screen.cells[0].width, 2);
    assert_eq!(screen.cells[1].width, 0);
    assert_eq!(screen.cells[2].text, "e\u{301}");
    assert_eq!(screen.cells[2].fg, Color::Rgb(12, 34, 56));
    assert_eq!(screen.cells[3].bg, Color::Indexed(42));
    assert!(
        screen.modes.application_cursor
            && screen.modes.application_keypad
            && screen.modes.bracketed_paste
            && screen.modes.mouse_drag
            && screen.modes.mouse_sgr
            && screen.modes.focus
    );
    assert!(!screen.cursor.unwrap().visible);
}
#[test]
fn alternate_history_reflow_and_selection() {
    let mut terminal = Terminal::new(10, 3);
    terminal.feed(b"hello world");
    terminal.resize(20, 3);
    assert_eq!(line(&terminal.snapshot(), 0), "hello world");
    assert_eq!(
        terminal
            .select(Point { x: 0, y: 0 }, Point { x: 4, y: 0 })
            .unwrap(),
        "hello"
    );
    assert!(
        terminal
            .select(Point { x: 30, y: 0 }, Point { x: 4, y: 0 })
            .is_err()
    );
    terminal.feed(b"\x1b[?1049hALT\x1b[?1049l");
    assert_eq!(line(&terminal.snapshot(), 0), "hello world");
    terminal.feed(b"\r\nsecond\r\nthird\r\nfourth\r\nfifth");
    terminal.scroll(2);
    assert_eq!(line(&terminal.snapshot(), 0), "hello world");
    terminal.scroll(-999);
    assert_eq!(line(&terminal.snapshot(), 2), "fifth");
}
#[test]
fn callbacks_query_osc_colors_title_clipboard_bell() {
    let mut terminal = Terminal::new(20, 4);
    let effects=terminal.feed(b"\x1b]2;title\x07\x07\x1b]52;c;aGVsbG8=\x07\x1b[6n\x1b[18t\x1b]10;rgb:12/34/56\x07\x1b]10;?\x07");
    assert!(
        effects
            .iter()
            .any(|e| matches!(e,Effect::Title(s) if s=="title"))
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::Bell)));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e,Effect::Clipboard(s) if s=="hello"))
    );
    let replies = effects
        .iter()
        .filter_map(|e| {
            if let Effect::WritePty(b) = e {
                Some(String::from_utf8_lossy(b))
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("");
    assert!(replies.contains("\x1b[1;1R"));
    assert!(replies.contains("\x1b[8;4;20t"));
    assert!(replies.contains("rgb:1212/3434/5656"));
    terminal.feed(b"A");
    assert_eq!(terminal.snapshot().cells[0].fg, Color::Rgb(18, 52, 86));
    assert!(terminal.feed(b"\x1b]52;c;?\x07").is_empty());
}
#[test]
fn diff_replays_styles_wide_shrink_resize_and_unchanged() {
    let mut source = Terminal::new(20, 4);
    source.feed("\x1b[1;3;4;38;2;40;50;60;48;5;17m界e\u{301} hello\x1b[2;1Hsecond".as_bytes());
    let first = frame(source.snapshot());
    let mut display = Terminal::new(20, 4);
    display.feed(&terminal_diff(None, &first));
    assert_eq!(display.snapshot().cells, first.cells);
    assert_eq!(display.snapshot().cursor, first.cursor);
    assert!(terminal_diff(Some(&first), &first).is_empty());
    source.feed(b"\x1b[1;1H\x1b[0mab");
    let second = frame(source.snapshot());
    let diff = terminal_diff(Some(&first), &second);
    assert!(diff.len() < 200);
    display.feed(&diff);
    assert_eq!(display.snapshot().cells, second.cells);
    source.resize(30, 5);
    display.resize(30, 5);
    let third = frame(source.snapshot());
    let diff = terminal_diff(Some(&second), &third);
    assert!(diff.windows(4).any(|b| b == b"\x1b[2J"));
    display.feed(&diff);
    assert_eq!(display.snapshot().cells, third.cells);
}
#[test]
fn compositor_footer_help_and_control_sanitization() {
    let mut workspace = Workspace::new(80, 24).unwrap();
    let frame = compose(&workspace, &BTreeMap::new(), false, Some("safe\x1b[2J"), 0);
    assert_eq!(frame.cells.len(), 80 * 24);
    assert!(
        frame
            .cells
            .iter()
            .all(|c| !c.text.chars().any(char::is_control))
    );
    assert_eq!(frame.cells[23 * 80 + 15].text, "?");
    workspace.help = true;
    let frame = compose(&workspace, &BTreeMap::new(), true, None, 0);
    assert!(frame.cursor.is_none());
    assert!(frame.cells.iter().any(|c| c.text == "×"));
    let mut display = Terminal::new(80, 24);
    display.feed(&terminal_diff(None, &frame));
    assert_eq!(display.snapshot().cells, frame.cells);
}

#[test]
fn queries_observe_palette_at_each_osc_terminator() {
    let mut terminal = Terminal::new(20, 4);
    let effects = terminal
        .feed(b"\x1b]10;rgb:ff/00/00\x07\x1b]10;?\x07\x1b]10;rgb:00/00/ff\x1b\\\x1b]10;?\x1b\\");
    let replies: Vec<_> = effects
        .into_iter()
        .filter_map(|e| {
            if let Effect::WritePty(bytes) = e {
                Some(String::from_utf8(bytes).unwrap())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(replies.len(), 2);
    assert!(replies[0].contains("rgb:ffff/0000/0000"));
    assert!(replies[1].contains("rgb:0000/0000/ffff"));
}

#[test]
fn composed_reading_views_replay_colors_unicode_and_background_geometry() {
    use kelpmux::model::Status;
    for cols in [40, 120, 240] {
        let mut ws = Workspace::new(cols, 24).unwrap();
        ws.panes = (1..=4)
            .map(|id| PaneInfo {
                id,
                number: id as usize,
                pid: id as u32,
                cwd: "/".into(),
                title: format!("agent {id}"),
                palette: id as u8,
                status: Status::Working,
                session_id: Some(format!("session-{id}")),
                dead: false,
                ..Default::default()
            })
            .collect();
        ws.active = 2;
        ws.apply(&Action::Grid { cols: 2, rows: 2 }).unwrap();
        let original = ws.layout();
        let mut display = Terminal::new(cols, 24);
        let mut previous = None;
        for action in [
            Action::CloseReading,
            Action::Read(2),
            Action::Fullscreen(2),
            Action::Read(2),
        ] {
            ws.apply(&action).unwrap();
            let mut screens = BTreeMap::new();
            for (id, width, height) in ws.terminal_sizes() {
                let mut terminal = Terminal::new(width, height);
                terminal
                    .feed(format!("\x1b[38;2;12;34;56;48;5;{id}m界e\u{301} pane{id}").as_bytes());
                screens.insert(id, terminal.snapshot());
            }
            let frame = compose(&ws, &screens, true, None, 3);
            assert_eq!(ws.grid_layout(), original);
            display.feed(&terminal_diff(previous.as_ref(), &frame));
            assert_eq!(display.snapshot().cells, frame.cells);
            assert_eq!(display.snapshot().cursor, frame.cursor);
            let tab = frame.panes.iter().find(|v| v.pane.id == 2);
            if ws.reading != ReadingMode::Grid {
                let overlay = tab.unwrap();
                let body =
                    usize::from(overlay.body.y) * usize::from(cols) + usize::from(overlay.body.x);
                assert_eq!(frame.cells[body].fg, Color::Rgb(12, 34, 56));
                assert_eq!(frame.cells[body].bg, Color::Indexed(2));
            }
            previous = Some(frame);
        }
        assert_eq!(ws.reading, ReadingMode::Grid);
    }
}

#[test]
fn hyperlinks_survive_fragmentation_composition_patches_and_erasure() {
    let uri = "https://example.com/path?q=one&v=two";
    let input = format!("\x1b]8;id=source;{uri}\x1b\\界e\u{301}\x1b]8;;\x1b\\ plain");
    let mut source = Terminal::new(40, 4);
    for byte in input.bytes() {
        source.feed(&[byte]);
    }
    let screen = source.snapshot();
    assert_eq!(screen.cells[0].hyperlink.as_deref(), Some(uri));
    assert_eq!(screen.cells[2].hyperlink.as_deref(), Some(uri));
    assert_eq!(screen.cells[3].hyperlink, None);
    let mut ws = Workspace::new(40, 10).unwrap();
    ws.panes = vec![PaneInfo {
        id: 1,
        ..Default::default()
    }];
    ws.active = 1;
    let first = compose(&ws, &BTreeMap::from([(1, screen)]), true, None, 0);
    let body = first.panes[0].body;
    let base = usize::from(body.y * first.cols + body.x);
    let bytes = terminal_diff(None, &first);
    let mut display = Terminal::new(40, 10);
    display.feed(&bytes);
    assert_eq!(display.snapshot().cells, first.cells);
    assert!(
        first.cells[..40]
            .iter()
            .all(|cell| cell.hyperlink.is_none())
    );

    // Changing only the target must redraw the link and travel in a delta.
    let mut next = first.clone();
    next.cells[base].hyperlink = Some("https://example.org/new".into());
    next.cells[base + 1].hyperlink = next.cells[base].hyperlink.clone();
    let patch = kelpmux::native::delta::diff(&first, &next, 1).unwrap();
    assert_eq!(
        kelpmux::native::delta::apply(&first, 1, &patch).unwrap(),
        next
    );
    display.feed(&terminal_diff(Some(&first), &next));
    assert_eq!(display.snapshot().cells, next.cells);
    let mut cleared = next.clone();
    cleared.cells[base] = Cell::default();
    cleared.cells[base + 1] = Cell::default();
    display.feed(&terminal_diff(Some(&next), &cleared));
    assert_eq!(display.snapshot().cells, cleared.cells);

    // Targets cannot inject terminal commands, and old cells decode without links.
    assert!(!valid_hyperlink("https://example.com/\x1b]52;c;bad\x07"));
    let mut malicious = first.clone();
    malicious.cells[base].hyperlink = Some("https://example.com/\x1b]52;c;bad\x07".into());
    assert!(kelpmux::native::delta::diff(&first, &malicious, 1).is_none());
    let output = String::from_utf8(terminal_diff(None, &malicious)).unwrap();
    assert!(!output.contains("52;c;bad"));
    let mut legacy = serde_json::to_value(Cell::default()).unwrap();
    legacy.as_object_mut().unwrap().remove("hyperlink");
    assert_eq!(
        serde_json::from_value::<Cell>(legacy).unwrap().hyperlink,
        None
    );
}

#[test]
fn plain_urls_span_soft_wraps_but_not_separate_lines_and_keep_explicit_targets() {
    let mut source = Terminal::new(20, 6);
    source.feed(b"(https://example.com/a(b)).\r\nhttps://a.test\r\nnext");
    let screen = source.snapshot();
    assert_eq!(
        screen.cells[1].hyperlink.as_deref(),
        Some("https://example.com/a(b)")
    );
    assert_eq!(screen.cells[20].hyperlink, screen.cells[1].hyperlink);
    assert_eq!(
        screen.cells[40].hyperlink.as_deref(),
        Some("https://a.test")
    );
    assert_eq!(screen.cells[60].hyperlink, None);
    let mut source = Terminal::new(60, 2);
    source.feed(b"\x1b]8;;https://target.test\x1b\\https://label.test\x1b]8;;\x1b\\");
    assert_eq!(
        source.snapshot().cells[0].hyperlink.as_deref(),
        Some("https://target.test")
    );
}
