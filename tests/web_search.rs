//! Client/server cross-checks: KelpServer's search results rendered by the
//! shared client core (`kelpclient::history_screen`), as the web client does.
use kelpserver::native::engine::Terminal;
fn match_text(hit: &kelpserver::native::protocol::PaneSearchMatch, cols: u16) -> String {
    let screen = kelpclient::history_screen(&hit.lines, cols, 160).unwrap();
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
