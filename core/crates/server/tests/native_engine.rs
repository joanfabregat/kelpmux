use kelpserver::native::{
    engine::{Effect, Terminal},
    protocol::*,
};
#[test]
fn panes_inherit_terminal_defaults_and_preserve_explicit_composer_background() {
    let mut pane = Terminal::new(40, 8);
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
    assert_eq!(source.cells[0].bg, Color::Default);
    assert_eq!(source.cells[0].fg, Color::Default);
    assert_eq!(source.cells[2 * 40 + 30].bg, Color::Rgb(31, 31, 31));
}

#[test]
fn theme_queries_use_client_colors_without_overriding_app_colors_or_default_cells() {
    let mut terminal = Terminal::new(40, 8);
    terminal.set_theme([Some([208, 216, 224]), Some([31, 32, 33])]);
    let replies = |effects: Vec<Effect>| -> String {
        String::from_utf8(
            effects
                .into_iter()
                .filter_map(|effect| match effect {
                    Effect::WritePty(bytes) => Some(bytes),
                    _ => None,
                })
                .flatten()
                .collect(),
        )
        .unwrap()
    };
    let response = replies(terminal.feed(b"\x1b]10;?\x1b\\\x1b]11;?\x07"));
    assert!(response.contains("rgb:d0d0/d8d8/e0e0"));
    assert!(response.contains("rgb:1f1f/2020/2121"));
    assert_eq!(terminal.snapshot().cells[0].bg, Color::Default);
    assert_eq!(terminal.snapshot().cells[0].fg, Color::Default);
    terminal.feed(b"\x1b[48;2;48;49;50m\x1b[2K input \x1b[0m");
    assert_eq!(terminal.snapshot().cells[30].bg, Color::Rgb(48, 49, 50));
    let response = replies(terminal.feed(b"\x1b]11;rgb:aa/bb/cc\x07\x1b]11;?\x07"));
    assert!(response.contains("rgb:aaaa/bbbb/cccc"));
    let response = replies(terminal.feed(b"\x1b]111\x07\x1b]11;?\x07"));
    assert!(response.contains("rgb:1f1f/2020/2121"));
    terminal.feed(b"\x1b[2;1H\x1b[7mX\x1b[0m");
    assert_eq!(terminal.snapshot().cells[40].fg, Color::Rgb(31, 32, 33));
    assert_eq!(terminal.snapshot().cells[40].bg, Color::Rgb(208, 216, 224));
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
fn alternate_history_and_reflow() {
    let mut terminal = Terminal::new(10, 3);
    terminal.feed(b"hello world");
    terminal.resize(20, 3);
    assert_eq!(line(&terminal.snapshot(), 0), "hello world");
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
fn hyperlinks_survive_fragmentation_and_reject_injected_targets() {
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
    // Targets cannot inject terminal commands, and old cells decode without links.
    assert!(!valid_hyperlink("https://example.com/\x1b]52;c;bad\x07"));
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

#[test]
fn plain_url_targets_exclude_surrounding_parentheses_quotes_and_adjacent_prose() {
    for (text, expected) in [
        ("(http://example.test)", "http://example.test"),
        ("[(https://example.test)]", "https://example.test"),
        ("(https://example.test),next", "https://example.test"),
        (
            "(https://example.test/a(b))next",
            "https://example.test/a(b)",
        ),
        ("[label](https://example.test).", "https://example.test"),
        ("'http://example.test'", "http://example.test"),
        ("`https://example.test`", "https://example.test"),
        ("http://[::1]/a(b)?q=(x)", "http://[::1]/a(b)?q=(x)"),
    ] {
        for cols in [18, 80] {
            let mut source = Terminal::new(cols, 10);
            source.feed(text.as_bytes());
            let screen = source.snapshot();
            let links: Vec<_> = screen
                .cells
                .iter()
                .filter_map(|cell| cell.hyperlink.as_deref())
                .collect();
            assert_eq!(
                links.len(),
                expected.len(),
                "wrong clickable range for {text:?} at {cols} columns: {links:?}"
            );
            assert!(
                links.iter().all(|link| *link == expected),
                "wrong target for {text:?}: {links:?}"
            );
        }
    }
}

#[test]
fn parenthesized_oauth_urls_survive_tui_hard_wraps_and_indent_without_linking_prose() {
    let uri = format!(
        "https://accounts.google.com/o/oauth2/auth?response_type=code&client_id=example.apps.googleusercontent.com&redirect_uri=https%3A%2F%2Fsdk.cloud.google.com%2Fauthcode.html&scope={}&state=example&code_challenge=example&code_challenge_method=S256",
        "openid+cloud-platform+".repeat(16)
    );
    for cols in [40usize, 80, 120] {
        let prefix = "3. Sign in with user@example.test (";
        let mut physical = String::from(prefix);
        let mut column = prefix.len();
        for c in uri.chars() {
            if column >= cols - 2 {
                physical.push_str("\r\n   ");
                column = 3;
            }
            physical.push(c);
            column += 1;
        }
        physical.push_str(") and paste the code shown.\r\nThis prose must not be linked.");
        let mut terminal = Terminal::new(cols as u16, 40);
        terminal.feed(physical.as_bytes());
        let screen = terminal.snapshot();
        let linked: Vec<_> = screen
            .cells
            .iter()
            .filter(|cell| cell.hyperlink.is_some())
            .collect();
        assert_eq!(linked.len(), uri.len(), "at {cols} columns");
        assert!(
            linked
                .iter()
                .all(|cell| cell.hyperlink.as_deref() == Some(uri.as_str()))
        );
        assert_eq!(
            linked
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>(),
            uri
        );
    }
    for text in [
        "(https://a.test/?x=y\r\nnext)",
        "(https://a.test/?x=y\r\nnext",
    ] {
        let mut terminal = Terminal::new(80, 4);
        terminal.feed(text.as_bytes());
        let screen = terminal.snapshot();
        assert!(
            screen.cells[80..160]
                .iter()
                .all(|cell| cell.hyperlink.is_none())
        );
    }
}

#[test]
fn emoji_presentation_warning_reserves_two_cells_across_fragmented_input() {
    let mut terminal = Terminal::new(20, 3);
    for byte in "⚠️✅❓🌑X".as_bytes() {
        terminal.feed(&[*byte]);
    }
    let screen = terminal.snapshot();
    for (index, text) in [(0, "⚠️"), (2, "✅"), (4, "❓"), (6, "🌑")] {
        assert_eq!(screen.cells[index].text, text);
        assert_eq!(screen.cells[index].width, 2);
        assert_eq!(screen.cells[index + 1].width, 0);
    }
    assert_eq!(screen.cells[8].text, "X");
    let mut linked = Terminal::new(20, 3);
    linked.feed("\x1b]8;;https://example.com\x1b\\⚠️\x1b]8;;\x1b\\X".as_bytes());
    let screen = linked.snapshot();
    for index in [0, 1] {
        assert_eq!(
            screen.cells[index].hyperlink.as_deref(),
            Some("https://example.com")
        );
    }
    assert_eq!(screen.cells[2].text, "X");
    assert!(screen.cells[2].hyperlink.is_none());
    let mut overwritten = Terminal::new(20, 3);
    overwritten.feed("A界Z\x1b[1;1H⚠️".as_bytes());
    let screen = overwritten.snapshot();
    assert_eq!(screen.cells[0].text, "⚠️");
    assert_eq!(screen.cells[0].width, 2);
    assert_eq!(screen.cells[1].width, 0);
    assert_eq!(screen.cells[2].text, " ");
    assert_eq!(screen.cells[2].width, 1);
    assert_eq!(screen.cells[3].text, "Z");
}
#[test]
fn alternate_screen_use_is_remembered_until_taken_across_fragments() {
    let mut pane = Terminal::new(40, 8);
    assert!(!pane.take_alternate_used());
    // Entering and leaving within one period is still reported once.
    for byte in b"\x1b[?1049hFULL\x1b[?1049lback" {
        pane.feed(&[*byte]);
    }
    assert!(!pane.is_alternate());
    assert!(pane.take_alternate_used());
    assert!(!pane.take_alternate_used());
    // A screen that stays alternate is reported in each period.
    pane.feed(b"\x1b[?1049h");
    assert!(pane.take_alternate_used());
    assert!(pane.take_alternate_used());
    pane.feed(b"\x1b[?1049l");
    assert!(pane.take_alternate_used());
    assert!(!pane.take_alternate_used());
}
