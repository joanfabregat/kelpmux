use kelpmux::native::{
    engine::{Effect, Terminal},
    protocol::*,
    render::{compose, terminal_diff},
    workspace::Workspace,
};
use std::collections::BTreeMap;
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
    assert_eq!(frame.cells[23 * 80 + 78].text, "?");
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
