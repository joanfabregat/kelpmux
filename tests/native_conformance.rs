//! Hand-authored, content-free terminal compatibility fixtures.
use kelpmux::native::{engine::Terminal, protocol::*, render::terminal_diff};

fn fixtures() -> impl Iterator<Item = String> {
    [
        include_str!("fixtures/native/agent-redraw.json"),
        include_str!("fixtures/native/alternate-tools.json"),
    ]
    .into_iter()
    .map(|json| serde_json::from_str(json).unwrap())
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
fn every_fixture_split_preserves_screen_history_modes_and_ordered_replies() {
    for fixture in fixtures() {
        let bytes = fixture.as_bytes();
        let mut whole = Terminal::new(48, 12);
        let expected_effects = format!("{:?}", whole.feed(bytes));
        let expected = whole.snapshot();
        let history = whole.history(i32::MIN, 1000);
        for split in 0..=bytes.len() {
            let mut fragmented = Terminal::new(48, 12);
            let mut effects = fragmented.feed(&bytes[..split]);
            effects.extend(fragmented.feed(&bytes[split..]));
            let actual = fragmented.snapshot();
            assert!(
                actual == expected,
                "split {split}; first different cell {:?}; cursor {:?}/{:?}; modes {:?}/{:?}",
                actual
                    .cells
                    .iter()
                    .zip(&expected.cells)
                    .enumerate()
                    .find(|(_, (a, b))| a != b),
                actual.cursor,
                expected.cursor,
                actual.modes,
                expected.modes
            );
            assert_eq!(fragmented.history(i32::MIN, 1000), history);
            assert_eq!(format!("{effects:?}"), expected_effects);
        }
        for stride in 1..=17 {
            let mut fragmented = Terminal::new(48, 12);
            let effects: Vec<_> = bytes
                .chunks(stride)
                .flat_map(|b| fragmented.feed(b))
                .collect();
            assert_eq!(fragmented.snapshot(), expected, "stride {stride}");
            assert_eq!(format!("{effects:?}"), expected_effects);
        }
    }
}
#[test]
fn representative_redraws_replay_after_reflow_without_color_loss() {
    for fixture in fixtures() {
        let mut source = Terminal::new(48, 12);
        source.feed(fixture.as_bytes());
        let mut display = Terminal::new(48, 12);
        let mut previous = None;
        for (cols, rows) in [(48, 12), (120, 36), (24, 8), (80, 24)] {
            source.resize(cols, rows);
            display.resize(cols, rows);
            let next = frame(source.snapshot());
            for byte in terminal_diff(previous.as_ref(), &next) {
                display.feed(&[byte]);
            }
            assert_eq!(display.snapshot().cells, next.cells);
            assert_eq!(display.snapshot().cursor, next.cursor);
            previous = Some(next);
        }
    }
}
#[test]
fn fixture_semantics_cover_unicode_color_and_input_modes() {
    let fixture = fixtures().next().unwrap();
    let mut term = Terminal::new(48, 12);
    term.feed(fixture.as_bytes());
    let screen = term.snapshot();
    assert!(screen.cells.iter().any(|c| c.text == "e\u{301}"));
    assert!(screen.cells.iter().any(|c| c.text == "界" && c.width == 2));
    assert!(screen.cells.iter().any(|c| c.bg == Color::Rgb(48, 48, 48)));
    assert!(screen.cells.iter().any(|c| c.fg == Color::Indexed(245)));
    assert!(screen.modes.bracketed_paste && screen.modes.focus);
}
#[test]
fn long_running_output_evicts_old_history_and_caps_requests() {
    let mut term = Terminal::new(40, 10);
    term.feed(b"OLD_SENTINEL\r\n");
    for n in 0..15_000 {
        term.feed(format!("synthetic-{n:05}\r\n").as_bytes());
    }
    assert!(term.search("OLD_SENTINEL", 10).is_empty());
    assert_eq!(term.search("synthetic-14999", 10).len(), 1);
    let oldest = term.history(i32::MIN, usize::MAX);
    assert_eq!(oldest.len(), 1000);
    assert!(oldest[0].0 >= -10_000);
    assert!(term.search("synthetic", usize::MAX).len() <= 256);
    term.feed(b"\x1b[?1049hsynthetic-alt");
    assert_eq!(term.search("synthetic-14999", 10).len(), 1);
    assert_eq!(term.history(i32::MIN, usize::MAX).len(), 1000);
    term.feed(b"\x1b[?1049l");
    assert_eq!(term.search("synthetic-14999", 10).len(), 1);
}

#[test]
fn mixed_utf8_boundaries_and_invalid_interruptions_do_not_swallow_ascii() {
    let cases: &[&[u8]] = &[
        "e\u{301} 界 tail".as_bytes(),
        b"start \xccA B \xe7\x95X end",
        b"start \xe7\x95\x1b[31mred\x1b[0m end",
        b"start \xff\x80tail",
        b"\x1b]2;unicode \xe7\x95\x8c title\x1b\\visible",
        b"\x1bPignored \xe7\x95\x8c\x1b\\visible",
    ];
    for bytes in cases {
        let mut whole = Terminal::new(40, 10);
        let expected_effects = format!("{:?}", whole.feed(bytes));
        let expected = whole.snapshot();
        for split in 0..=bytes.len() {
            let mut parts = Terminal::new(40, 10);
            let mut effects = parts.feed(&bytes[..split]);
            effects.extend(parts.feed(&bytes[split..]));
            assert!(
                parts.snapshot() == expected,
                "case {bytes:?}, split {split}"
            );
            assert_eq!(format!("{effects:?}"), expected_effects);
        }
    }
    let mut term = Terminal::new(40, 10);
    term.feed("e\u{301} 界 tail".as_bytes());
    let cells = term.snapshot().cells;
    assert_eq!(cells[0].text, "e\u{301}");
    assert_eq!(cells[1].text, " ");
    assert_eq!(cells[2].text, "界");
    assert_eq!(cells[5].text, "t");
}

#[test]
fn parser_workloads_report_ascii_unicode_and_fragmentation_costs() {
    // Diagnostic measurement, deliberately no CI wall-clock threshold. Equal
    // final screens ensure batching never trades correctness for throughput.
    for (name, line) in [
        ("ascii", "synthetic status: processing file 123\r\n"),
        ("unicode", "synthetic status: e\u{301} 界 🙂 ⠋\r\n"),
    ] {
        let bytes = line.repeat(2000).into_bytes();
        let mut expected = None;
        for chunk in [4096, 17, 1] {
            let mut term = Terminal::new(80, 24);
            let start = std::time::Instant::now();
            for bytes in bytes.chunks(chunk) {
                term.feed(bytes);
            }
            let elapsed = start.elapsed().as_micros();
            let screen = term.snapshot();
            if let Some(expected) = &expected {
                assert_eq!(&screen, expected);
            } else {
                expected = Some(screen);
            }
            eprintln!(
                "parser workload={name} bytes={} chunk={chunk} micros={elapsed}",
                bytes.len()
            );
        }
    }
}
