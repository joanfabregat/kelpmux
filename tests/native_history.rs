use kelpmux::native::{engine::Terminal, protocol::Point};
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
