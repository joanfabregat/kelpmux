use kelpclient_protocol::Cell;
use kelpserver::native::engine::Terminal;
/// Lay out plain search-context rows as cells, mirroring the client-side
/// `history_screen` layout (Unicode widths, combining marks, ZWJ, regional
/// indicator pairs and skin-tone modifiers join the previous glyph; wide
/// glyphs are followed by a zero-width continuation cell). The server reports
/// match coordinates in this cell space, so the test checks them without
/// depending on a client crate. KelpMux keeps the cross-check against the real
/// client implementation.
fn history_cells(lines: &[String], cols: u16, rows: u16) -> Vec<Cell> {
    assert!((1..=400).contains(&cols) && (1..=160).contains(&rows));
    let mut cells = Vec::with_capacity(usize::from(cols) * usize::from(rows));
    for y in 0..usize::from(rows) {
        let mut glyphs: Vec<Cell> = Vec::new();
        for ch in lines.get(y).map(String::as_str).unwrap_or("").chars() {
            if ch.is_control() {
                continue;
            }
            let width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            let join = glyphs.last().is_some_and(|last| {
                last.text.ends_with('\u{200d}')
                    || (('\u{1f1e6}'..='\u{1f1ff}').contains(&ch)
                        && last.text.chars().count() == 1
                        && last
                            .text
                            .chars()
                            .next()
                            .is_some_and(|c| ('\u{1f1e6}'..='\u{1f1ff}').contains(&c)))
            });
            if width == 0 || join || ('\u{1f3fb}'..='\u{1f3ff}').contains(&ch) {
                if let Some(last) = glyphs.last_mut() {
                    if last.text.len() + ch.len_utf8() <= 1024 {
                        last.text.push(ch);
                        last.width = unicode_width::UnicodeWidthStr::width(last.text.as_str())
                            .clamp(1, 2) as u8;
                    }
                }
            } else {
                glyphs.push(Cell {
                    text: ch.to_string(),
                    width: width.min(2) as u8,
                    ..Cell::default()
                });
            }
            if glyphs.len() > usize::from(cols) {
                break;
            }
        }
        let start = cells.len();
        for glyph in glyphs {
            if cells.len() - start + usize::from(glyph.width) > usize::from(cols) {
                break;
            }
            let wide = glyph.width == 2;
            cells.push(glyph);
            if wide {
                cells.push(Cell {
                    text: String::new(),
                    width: 0,
                    ..Cell::default()
                });
            }
        }
        cells.resize(start + usize::from(cols), Cell::default());
    }
    cells
}
fn match_text(hit: &kelpserver::native::protocol::PaneSearchMatch, cols: u16) -> String {
    let cells = history_cells(&hit.lines, cols, 160);
    let start = usize::from(hit.start.y) * usize::from(cols) + usize::from(hit.start.x);
    let end = usize::from(hit.end.y) * usize::from(cols) + usize::from(hit.end.x);
    cells[start..=end]
        .iter()
        .filter(|cell| cell.width > 0)
        .map(|cell| cell.text.as_str())
        .collect()
}
#[test]
fn web_search_has_case_insensitive_occurrences_and_exact_wrapped_unicode_locations() {
    let mut term = Terminal::new(12, 5);
    term.feed("界e\u{301} abcdefghiAlpha ALPHA\r\nété".as_bytes());
    let (hits, truncated) = term.web_search("alpha", 64);
    assert!(!truncated);
    assert_eq!(hits.len(), 2);
    assert_eq!(match_text(&hits[0], 12), "Alpha");
    assert_eq!(match_text(&hits[1], 12), "ALPHA");
    let unicode = term.web_search("ÉTÉ", 64).0;
    assert_eq!(match_text(&unicode[0], 12), "été");
    let saved = hits[0].lines.clone();
    term.feed(&b"\r\nnew output".repeat(50));
    assert_eq!(hits[0].lines, saved);
    assert_eq!(match_text(&hits[0], 12), "Alpha");
    assert!(term.web_search("a", 1).1);
}
#[test]
fn web_search_distinguishes_primary_and_alternate_contexts_with_the_same_line_number() {
    let mut term = Terminal::new(30, 5);
    term.feed(b"PRIMARY NEEDLE\x1b[?1049hALTERNATE needle");
    let (hits, _) = term.web_search("needle", 64);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].source, "primary");
    assert_eq!(hits[1].source, "alternate");
    assert!(hits[0].lines.join("").contains("PRIMARY NEEDLE"));
    assert!(!hits[0].lines.join("").contains("ALTERNATE"));
    assert_eq!(match_text(&hits[1], 30), "needle");
}
#[test]
fn late_pages_in_long_primary_history_remain_correct_in_alternate_screen() {
    let mut term = Terminal::new(30, 4);
    for n in 0..20_000 {
        term.feed(format!("row-{n}\r\n").as_bytes());
    }
    let expected = term.history(-30, 20);
    term.feed(b"\x1b[?1049hALT");
    assert_eq!(term.history(-30, 20), expected);
    assert_eq!(term.history(-30, 0), Vec::new());
    assert!(term.history(100, 20).is_empty());
    assert!(term.history(i32::MIN, 20)[0].0 < -1000);
}
#[test]
fn unicode_wrapped_and_repeated_search_hits_do_not_change_the_viewport() {
    let mut term = Terminal::new(12, 3);
    term.feed(
        "first 界e\u{301}\r\nabcdefGHIJKLMN\r\nrepeat target\r\nrepeat target\r\nlast".as_bytes(),
    );
    term.scroll(2);
    let before = term.snapshot();
    let (hits, truncated) = term.web_search("GHIJKLMN", 20);
    assert!(!truncated);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].lines.join("").contains("abcdefGHIJKLMN"));
    assert_eq!(term.web_search("repeat target", 20).0.len(), 2);
    assert_eq!(term.web_search("界e\u{301}", 20).0.len(), 1);
    assert_eq!(term.snapshot(), before);
    let rows = term.history(i32::MIN, 2);
    assert_eq!(rows.len(), 2);
    assert!(rows[0].0 < 0);
    let next = term.history(rows[1].0 + 1, 2);
    assert!(next[0].0 > rows[1].0);
    assert_eq!(term.snapshot(), before);
    term.scroll_to(rows[0].0).unwrap();
    assert_ne!(term.snapshot(), before);
}
#[test]
fn primary_history_survives_fragmented_alternate_entry_without_disturbing_alt() {
    let mut term = Terminal::new(20, 3);
    term.feed(b"primary-before\r\nprimary target\r\nthird\r\nfourth");
    let primary = |term: &Terminal| {
        let (hits, _) = term.web_search("primary target", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source, "primary");
    };
    primary(&term);
    let line = term
        .history(i32::MIN, 10)
        .into_iter()
        .find(|(_, text)| text == "primary target")
        .unwrap()
        .0;
    for byte in b"\x1b[?1049h" {
        term.feed(&[*byte]);
    }
    assert!(term.is_alternate());
    term.feed(b"ALT-content");
    assert_eq!(term.web_search("ALT-content", 10).0.len(), 1);
    let before = term.snapshot();
    primary(&term);
    assert_eq!(term.snapshot(), before);
    assert!(term.scroll_to(line).is_err());
    assert_eq!(term.snapshot(), before);
    term.resize(25, 4);
    primary(&term);
    term.feed(b"\x1b[?1049l");
    assert!(!term.is_alternate());
    primary(&term);
}
#[test]
fn alternate_entry_in_same_feed_preserves_text_and_exit_allows_new_primary_search() {
    let mut term = Terminal::new(12, 2);
    term.feed(b"new text\x1b[?1049hALT");
    assert!(term.is_alternate());
    assert_eq!(term.web_search("new text", 10).0.len(), 1);
    term.feed(b"\x1b[?1049l\r\nnext");
    assert!(!term.is_alternate());
    assert_eq!(term.web_search("next", 10).0.len(), 1);
    term.resize(8, 2);
    assert_eq!(term.web_search("new text", 10).0.len(), 1);
}
#[test]
fn bounded_queries_cross_wrapped_and_history_rows() {
    let mut term = Terminal::new(8, 2);
    term.feed(b"abcdefghIJK\r\nlast\r\nend");
    assert!(term.web_search("", 10).0.is_empty());
    assert!(term.web_search(&"x".repeat(257), 10).0.is_empty());
    assert!(term.web_search("\x1b", 10).0.is_empty());
    assert!(term.web_search("last", 0).0.is_empty());
    assert!(term.history(100, 10).is_empty());
    assert!(
        term.web_search("ghIJ", 10).0.len() == 1,
        "a hit across a soft wrap"
    );
    term.scroll_to(-2).unwrap();
}
