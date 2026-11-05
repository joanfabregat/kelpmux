use kelpmux::native::{engine::Terminal, protocol::Point};
fn match_text(hit: &kelpmux::native::protocol::PaneSearchMatch, cols: u16) -> String {
    let screen = kelpmux_client_core::history_screen(&hit.lines, cols, 160).unwrap();
    let start = usize::from(hit.start.y) * usize::from(cols) + usize::from(hit.start.x);
    let end = usize::from(hit.end.y) * usize::from(cols) + usize::from(hit.end.x);
    screen.cells[start..=end]
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
    let hits = term.search("GHIJKLMN", 20);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].1.contains("abcdefGHIJKLMN"));
    assert_eq!(term.search("repeat target", 20).len(), 2);
    assert_eq!(term.search("界e\u{301}", 20).len(), 1);
    assert_eq!(term.snapshot(), before);
    let rows = term.history(i32::MIN, 2);
    assert_eq!(rows.len(), 2);
    assert!(rows[0].0 < 0);
    let next = term.history(rows[1].0 + 1, 2);
    assert!(next[0].0 > rows[1].0);
    assert_eq!(term.snapshot(), before);
    term.scroll_to(hits[0].0).unwrap();
    assert_ne!(term.snapshot(), before);
}
#[test]
fn primary_history_survives_fragmented_alternate_entry_without_disturbing_alt() {
    let mut term = Terminal::new(20, 3);
    term.feed(b"primary-before\r\nprimary target\r\nthird\r\nfourth");
    let original = term.search("primary target", 10);
    assert_eq!(original.len(), 1);
    for byte in b"\x1b[?1049h" {
        term.feed(&[*byte]);
    }
    assert!(term.is_alternate());
    term.feed(b"ALT-content");
    assert_eq!(term.search("ALT-content", 10).len(), 1);
    let before = term.snapshot();
    assert_eq!(term.search("primary target", 10), original);
    assert_eq!(term.snapshot(), before);
    assert!(term.scroll_to(original[0].0).is_err());
    assert_eq!(term.snapshot(), before);
    term.resize(25, 4);
    assert_eq!(term.search("primary target", 10), original);
    term.feed(b"\x1b[?1049l");
    assert!(!term.is_alternate());
    assert_eq!(term.search("primary target", 10).len(), 1);
}
#[test]
fn alternate_entry_in_same_feed_preserves_text_and_exit_allows_new_primary_search() {
    let mut term = Terminal::new(12, 2);
    term.feed(b"new text\x1b[?1049hALT");
    assert!(term.is_alternate());
    assert_eq!(term.search("new text", 10).len(), 1);
    term.feed(b"\x1b[?1049l\r\nnext");
    assert!(!term.is_alternate());
    assert_eq!(term.search("next", 10).len(), 1);
    term.resize(8, 2);
    assert_eq!(term.search("new text", 10).len(), 1);
}
#[test]
fn bounded_queries_and_keyboard_selection_cross_wrapped_and_history_rows() {
    let mut term = Terminal::new(8, 2);
    term.feed(b"abcdefghIJK\r\nlast\r\nend");
    assert!(term.search("", 10).is_empty());
    assert!(term.search(&"x".repeat(257), 10).is_empty());
    assert!(term.search("\x1b", 10).is_empty());
    assert!(term.search("last", 0).is_empty());
    assert!(term.history(100, 10).is_empty());
    assert!(term.selection_move(1, 0).is_err());
    term.scroll_to(-2).unwrap();
    term.selection_start(Point { x: 0, y: 0 }).unwrap();
    let selected = term.selection_move(10, 0).unwrap();
    assert!(selected.contains("abcdefghIJK"), "{selected:?}");
    assert_eq!(term.selection().as_deref(), Some(selected.as_str()));
    assert!(term.selection_move(i32::MIN, i32::MIN).unwrap().len() <= 65536);
    term.resize(10, 3);
    assert!(term.selection_move(1, 0).is_err());
}
#[test]
fn selection_tracks_new_output_and_alternate_swap_requires_a_new_anchor() {
    let mut term = Terminal::new(10, 2);
    term.feed(b"abcdefgh\r\nlast");
    term.selection_start(Point { x: 0, y: 0 }).unwrap();
    assert_eq!(term.selection_move(2, 0).unwrap(), "abc");
    term.feed(b"\r\nmore");
    assert_eq!(term.selection_move(1, 0).unwrap(), "abcd");
    term.feed(b"\x1b[?1049h");
    assert!(term.selection_move(1, 0).is_err());
}
#[test]
fn backward_selection_includes_both_edges_and_can_reverse_direction() {
    let mut term = Terminal::new(20, 2);
    term.feed(b"SELECT_ME++");
    term.selection_start(Point { x: 8, y: 0 }).unwrap();
    assert_eq!(term.selection_move(-8, 0).unwrap(), "SELECT_ME");
    assert_eq!(term.selection_move(10, 0).unwrap(), "E++");
}
