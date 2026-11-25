use super::*;
use crate::types::{Cell, Color, Cursor, InputModes, PanePatch, PaneSnapshot, Point, Screen};

/// A small deterministic generator (xorshift64*), so failures reproduce.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

fn color(rng: &mut Rng) -> Color {
    match rng.below(4) {
        0 | 1 => Color::Default,
        2 => Color::Indexed(rng.below(256) as u8),
        _ => Color::Rgb(
            rng.below(256) as u8,
            rng.below(256) as u8,
            rng.below(256) as u8,
        ),
    }
}
fn style(rng: &mut Rng) -> Cell {
    Cell {
        text: " ".into(),
        width: 1,
        fg: color(rng),
        bg: color(rng),
        bold: rng.chance(20),
        italic: rng.chance(10),
        underline: rng.chance(10),
        underline_color: rng.chance(10).then(|| color(rng)),
        hyperlink: rng
            .chance(10)
            .then(|| format!("https://example.com/{}", rng.below(5))),
    }
}
const NARROW: &[&str] = &[
    "a",
    "Z",
    "0",
    "~",
    "é",
    "e\u{301}",
    "a\u{300}\u{316}",
    "❤\u{fe0f}",
    "─",
    "λ",
    " ",
];
const WIDE: &[&str] = &["漢", "字", "😀", "ア", "\u{1100}\u{1161}"];

/// A random valid screen: styles, wide characters and their spacers,
/// combining marks, hyperlinks, empty and trailing-blank rows, VS16 cells,
/// empty-text cells, and full-width coloured runs.
fn screen(rng: &mut Rng, cols: u16, rows: u16) -> Screen {
    let cols_n = usize::from(cols);
    let mut cells = Vec::with_capacity(cols_n * usize::from(rows));
    let palette: Vec<Cell> = (0..1 + rng.below(6)).map(|_| style(rng)).collect();
    for _ in 0..rows {
        let mut row: Vec<Cell> = Vec::with_capacity(cols_n);
        match rng.below(10) {
            // Empty row.
            0 => {}
            // A full-width coloured bar.
            1 => {
                let mut bar = rng.pick(&palette).clone();
                bar.text = rng.pick(&[" ", "─", "="]).to_string();
                row.resize(cols_n, bar);
            }
            _ => {
                let length = rng.below(cols_n as u64 + 1) as usize;
                let mut current = rng.pick(&palette).clone();
                while row.len() < length {
                    if rng.chance(15) {
                        current = rng.pick(&palette).clone();
                    }
                    let mut cell = current.clone();
                    match rng.below(20) {
                        0 if row.len() + 2 <= cols_n => {
                            cell.text = rng.pick(WIDE).to_string();
                            cell.width = 2;
                            let mut spacer = cell.clone();
                            spacer.text.clear();
                            spacer.width = 0;
                            if rng.chance(20) {
                                // A spacer whose style differs from its glyph.
                                spacer.bg = Color::Indexed(rng.below(256) as u8);
                            }
                            row.push(cell);
                            row.push(spacer);
                            continue;
                        }
                        1 if row.len() + 2 <= cols_n => {
                            // VS16 emoji: two columns although the base is narrow.
                            cell.text = "❤\u{fe0f}".into();
                            cell.width = 2;
                            let mut spacer = cell.clone();
                            spacer.text.clear();
                            spacer.width = 0;
                            row.push(cell);
                            row.push(spacer);
                            continue;
                        }
                        2 => {
                            cell.text.clear();
                        }
                        3 => {
                            // A lone spacer, as after an overwritten wide glyph.
                            cell.text.clear();
                            cell.width = 0;
                        }
                        4 => cell = Cell::default(),
                        _ => cell.text = rng.pick(NARROW).to_string(),
                    }
                    row.push(cell);
                }
                row.truncate(cols_n);
                if row.last().is_some_and(|cell| cell.width == 2) {
                    row.pop();
                }
                // Trailing default blanks, sometimes after styled blanks.
                row.resize(cols_n, Cell::default());
            }
        }
        row.resize(cols_n, Cell::default());
        cells.extend(row);
    }
    for (index, cell) in cells.iter().enumerate() {
        assert!(
            crate::cell_valid(cell),
            "generator made an invalid cell {cell:?}"
        );
        assert!(!(cell.width == 2 && index % cols_n + 1 == cols_n));
    }
    Screen {
        cols,
        rows,
        cells,
        cursor: rng.chance(80).then(|| Cursor {
            position: Point {
                x: rng.below(u64::from(cols)) as u16,
                y: rng.below(u64::from(rows)) as u16,
            },
            visible: rng.chance(50),
        }),
        modes: InputModes {
            keyboard_flags: rng.below(32) as u8,
            application_cursor: rng.chance(50),
            application_keypad: rng.chance(50),
            bracketed_paste: rng.chance(50),
            mouse_click: rng.chance(50),
            mouse_drag: rng.chance(50),
            mouse_motion: rng.chance(50),
            mouse_sgr: rng.chance(50),
            focus: rng.chance(50),
        },
    }
}
fn geometry(rng: &mut Rng) -> (u16, u16) {
    match rng.below(4) {
        0 => (1 + rng.below(4) as u16, 1 + rng.below(3) as u16),
        1 => (MAX_COLS, MAX_ROWS),
        _ => (2 + rng.below(160) as u16, 1 + rng.below(60) as u16),
    }
}
/// The server's cell diff, for building patches.
fn diff(old: &PaneSnapshot, screen: &Screen) -> PanePatch {
    PanePatch {
        pane: old.pane,
        base: old.revision,
        revision: old.revision + 1,
        cursor: screen.cursor,
        modes: screen.modes,
        changes: old
            .screen
            .cells
            .iter()
            .zip(&screen.cells)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(index, (_, cell))| (index, cell.clone()))
            .collect(),
    }
}
fn only(bytes: &[u8]) -> ScreenMessage {
    let mut messages = decode(bytes, DEFAULT_MAX_CELLS).unwrap();
    assert_eq!(messages.len(), 1);
    messages.remove(0)
}

#[test]
fn random_snapshots_round_trip() {
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    for round in 0..400 {
        let (cols, rows) = geometry(&mut rng);
        let snapshot = PaneSnapshot {
            pane: 1 + rng.below(1000),
            revision: 1 + rng.below(1 << 40),
            screen: screen(&mut rng, cols, rows),
        };
        let bytes = encode_snapshot(&snapshot);
        assert!(is_binary_record(&bytes));
        assert_eq!(
            only(&bytes),
            ScreenMessage::Snapshot(snapshot.clone()),
            "round {round}"
        );
    }
}

#[test]
fn random_patches_round_trip_and_apply() {
    let mut rng = Rng(0x5eed_1234_abcd_0002);
    for round in 0..300 {
        let (cols, rows) = geometry(&mut rng);
        let base = PaneSnapshot {
            pane: 7,
            revision: 1 + rng.below(1000),
            screen: screen(&mut rng, cols, rows),
        };
        // Replace some rows of the base with rows of another random screen.
        let other = screen(&mut rng, cols, rows);
        let mut next = base.screen.clone();
        let width = usize::from(cols);
        for row in 0..usize::from(rows) {
            if rng.chance(20) {
                next.cells[row * width..(row + 1) * width]
                    .clone_from_slice(&other.cells[row * width..(row + 1) * width]);
            }
        }
        next.cursor = other.cursor;
        next.modes = other.modes;
        let patch = diff(&base, &next);
        let bytes = encode_patch(&patch, &next);
        let ScreenMessage::Patch(decoded) = only(&bytes) else {
            panic!("not a patch");
        };
        let changed: std::collections::BTreeSet<_> =
            patch.changes.iter().map(|(i, _)| i / width).collect();
        assert_eq!(
            decoded
                .lines
                .iter()
                .map(|(row, _)| usize::from(*row))
                .collect::<Vec<_>>(),
            changed.into_iter().collect::<Vec<_>>(),
            "round {round}"
        );
        let applied = decoded.apply(&base).unwrap();
        assert_eq!(applied.screen, next, "round {round}");
        assert_eq!(applied.revision, base.revision + 1);
        // The equivalent cell patch gives the same screen.
        let cells = decoded.into_pane_patch();
        let mut by_cells = base.screen.clone();
        for (index, cell) in cells.changes {
            by_cells.cells[index] = cell;
        }
        assert_eq!(by_cells.cells, next.cells);
    }
}

#[test]
fn a_record_carries_snapshots_and_patches_in_order() {
    let mut rng = Rng(9);
    let first = PaneSnapshot {
        pane: 1,
        revision: 1,
        screen: screen(&mut rng, 80, 24),
    };
    let base = PaneSnapshot {
        pane: 2,
        revision: 4,
        screen: screen(&mut rng, 40, 10),
    };
    let mut next = base.screen.clone();
    next.cells[45] = Cell {
        text: "x".into(),
        ..Cell::default()
    };
    let patch = diff(&base, &next);
    let mut builder = RecordBuilder::new();
    assert!(builder.is_empty());
    builder.patch(&patch, &next);
    builder.snapshot(first.pane, first.revision, &first.screen);
    assert_eq!(builder.len(), 2);
    let messages = decode(&builder.finish(), DEFAULT_MAX_CELLS).unwrap();
    assert!(
        matches!(&messages[0], ScreenMessage::Patch(p) if p.lines.len() == 1 && p.lines[0].0 == 1)
    );
    assert_eq!(messages[1], ScreenMessage::Snapshot(first));
    // An empty record is valid: an update without screen changes.
    assert_eq!(decode(&RecordBuilder::new().finish(), 0).unwrap(), vec![]);
}

#[test]
fn malformed_records_are_rejected_without_panicking() {
    let mut rng = Rng(0x5eed_1234_abcd_0003);
    let snapshot = PaneSnapshot {
        pane: 3,
        revision: 1,
        screen: screen(&mut rng, 120, 40),
    };
    let bytes = encode_snapshot(&snapshot);
    // Every truncation fails cleanly.
    for length in 0..bytes.len() {
        assert!(decode(&bytes[..length], DEFAULT_MAX_CELLS).is_err());
    }
    // Trailing bytes are refused.
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(decode(&longer, DEFAULT_MAX_CELLS).is_err());
    // Random corruption never panics.
    for _ in 0..5000 {
        let mut corrupt = bytes.clone();
        for _ in 0..1 + rng.below(4) {
            let at = rng.below(corrupt.len() as u64) as usize;
            corrupt[at] = rng.below(256) as u8;
        }
        if let Ok(messages) = decode(&corrupt, DEFAULT_MAX_CELLS) {
            for message in messages {
                if let ScreenMessage::Snapshot(s) = message {
                    assert!(s.screen.cells.iter().all(crate::cell_valid));
                }
            }
        }
    }
    // Random bytes behind a valid header never panic either.
    for _ in 0..2000 {
        let mut random = vec![RECORD_MARKER, FORMAT_VERSION];
        random.extend((0..rng.below(200)).map(|_| rng.below(256) as u8));
        let _ = decode(&random, DEFAULT_MAX_CELLS);
    }
    assert!(decode(b"{\"Stream\":{}}", DEFAULT_MAX_CELLS).is_err());
    assert!(decode(&[RECORD_MARKER, 2, 0], DEFAULT_MAX_CELLS).is_err());
}

/// Bytes of a snapshot header for hand-built records.
fn header_bytes(cols: u64, rows: u64) -> Vec<u8> {
    let mut out = vec![RECORD_MARKER, FORMAT_VERSION, 1, KIND_SNAPSHOT, 1, 1];
    varint(&mut out, cols);
    varint(&mut out, rows);
    out.extend_from_slice(&[0, 0, 0]);
    out
}

#[test]
fn decoding_is_bounded() {
    // Geometry beyond the protocol's maximum.
    for (cols, rows) in [(401, 1), (1, 161), (0, 1), (1, 0), (u64::MAX, 1)] {
        let mut bytes = header_bytes(cols, rows);
        bytes.push(0);
        assert!(decode(&bytes, DEFAULT_MAX_CELLS).is_err(), "{cols}x{rows}");
    }
    // The aggregate cell budget.
    let screen = Screen {
        cols: 400,
        rows: 160,
        cells: vec![Cell::default(); 64_000],
        cursor: None,
        modes: InputModes::default(),
    };
    let mut builder = RecordBuilder::new();
    builder.snapshot(1, 1, &screen);
    builder.snapshot(2, 1, &screen);
    let bytes = builder.finish();
    assert!(decode(&bytes, DEFAULT_MAX_CELLS).is_err());
    assert_eq!(decode(&bytes, 128_000).unwrap().len(), 2);
    // A style table larger than the screen.
    let mut bytes = header_bytes(2, 1);
    varint(&mut bytes, 3);
    bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert!(decode(&bytes, DEFAULT_MAX_CELLS).is_err());
    // A style count beyond the remaining bytes allocates nothing.
    let mut bytes = header_bytes(400, 160);
    varint(&mut bytes, 60_000);
    assert!(decode(&bytes, DEFAULT_MAX_CELLS).is_err());
    // Duplicate panes.
    let one = PaneSnapshot {
        pane: 1,
        revision: 1,
        screen: Screen {
            cols: 2,
            rows: 1,
            cells: vec![Cell::default(); 2],
            cursor: None,
            modes: InputModes::default(),
        },
    };
    let mut builder = RecordBuilder::new();
    builder.snapshot(1, 1, &one.screen);
    builder.snapshot(1, 1, &one.screen);
    assert!(decode(&builder.finish(), DEFAULT_MAX_CELLS).is_err());
}

#[test]
fn repeated_hyperlinks_cannot_amplify_a_small_record() {
    // A 2 KiB link on every cell of 64,000 cells would decode to 128 MiB.
    let link = format!("https://example.com/{}", "a".repeat(2000));
    let cell = Cell {
        hyperlink: Some(link),
        ..Cell::default()
    };
    let screen = Screen {
        cols: 400,
        rows: 160,
        cells: vec![cell; 64_000],
        cursor: None,
        modes: InputModes::default(),
    };
    let bytes = encode_snapshot(&PaneSnapshot {
        pane: 1,
        revision: 1,
        screen,
    });
    assert!(bytes.len() < 4096, "{} bytes", bytes.len());
    assert!(decode(&bytes, DEFAULT_MAX_CELLS).is_err());
}

#[test]
fn invalid_cells_are_rejected() {
    // A wide cell in the last column.
    let mut cells = vec![Cell::default(); 2];
    cells[1] = Cell {
        text: "漢".into(),
        width: 2,
        ..Cell::default()
    };
    let bytes = encode_snapshot(&PaneSnapshot {
        pane: 1,
        revision: 1,
        screen: Screen {
            cols: 2,
            rows: 1,
            cells,
            cursor: None,
            modes: InputModes::default(),
        },
    });
    assert!(decode(&bytes, DEFAULT_MAX_CELLS).is_err());
    // Control characters.
    let bytes = encode_snapshot(&PaneSnapshot {
        pane: 1,
        revision: 1,
        screen: Screen {
            cols: 2,
            rows: 1,
            cells: vec![
                Cell {
                    text: "\u{1b}".into(),
                    ..Cell::default()
                },
                Cell::default(),
            ],
            cursor: None,
            modes: InputModes::default(),
        },
    });
    assert!(decode(&bytes, DEFAULT_MAX_CELLS).is_err());
}

fn line(row: &mut Vec<Cell>, text: &str, fg: Color, bold: bool) {
    for ch in text.chars() {
        row.push(Cell {
            text: ch.to_string(),
            fg,
            bold,
            ..Cell::default()
        });
    }
}
/// Text-heavy shell output: prompts, `ls --color`, a coloured diff and
/// compiler messages, most lines shorter than the terminal.
fn realistic(cols: u16, rows: u16, blank_after: usize) -> Screen {
    let mut rng = Rng(42);
    let words = [
        "src",
        "target",
        "Cargo.toml",
        "README.md",
        "let",
        "mut",
        "fn",
        "pub",
        "struct",
        "impl",
        "return",
        "error[E0308]:",
        "warning:",
        "expected",
        "found",
        "-->",
        "screens.rs:120:9",
        "Compiling",
        "kelpserver",
        "v0.3.0",
        "Finished",
        "test",
        "ok",
        "drwxr-xr-x",
        "joan",
        "4096",
        "Nov",
        "22",
        "13:05",
    ];
    let colors = [
        Color::Default,
        Color::Default,
        Color::Default,
        Color::Indexed(1),
        Color::Indexed(2),
        Color::Indexed(4),
        Color::Indexed(6),
        Color::Rgb(0xd7, 0x87, 0x00),
    ];
    let mut cells = Vec::new();
    for y in 0..usize::from(rows) {
        let mut row = Vec::new();
        if y < blank_after {
            if y % 12 == 0 {
                line(&mut row, "joan@jbox", Color::Indexed(2), true);
                line(&mut row, ":", Color::Default, false);
                line(&mut row, "~/repos/kelp/core", Color::Indexed(4), true);
                line(&mut row, "$ cargo test --workspace", Color::Default, false);
            } else {
                let target = rng.below(u64::from(cols) * 9 / 10) as usize + 10;
                while row.len() < target {
                    let fg = *rng.pick(&colors);
                    line(
                        &mut row,
                        words[rng.below(words.len() as u64) as usize],
                        fg,
                        fg == Color::Indexed(1),
                    );
                    line(&mut row, " ", Color::Default, false);
                }
            }
        }
        row.truncate(usize::from(cols));
        row.resize(usize::from(cols), Cell::default());
        cells.extend(row);
    }
    Screen {
        cols,
        rows,
        cells,
        cursor: Some(Cursor {
            position: Point { x: 24, y: 0 },
            visible: true,
        }),
        modes: InputModes::default(),
    }
}

#[test]
fn size_report_json_versus_binary() {
    let mut rng = Rng(7);
    let tui = {
        // A full-screen program: coloured background bars and panels.
        let mut screen = screen(&mut rng, 300, 80);
        for y in 0..80 {
            let bar = Cell {
                bg: Color::Indexed(if y % 2 == 0 { 236 } else { 234 }),
                ..Cell::default()
            };
            for x in 0..300 {
                let cell = &mut screen.cells[y * 300 + x];
                if *cell == Cell::default() {
                    *cell = bar.clone();
                }
            }
        }
        screen
    };
    for (name, screen) in [
        ("text-heavy 300x80", realistic(300, 80, 80)),
        ("mostly blank 300x80", realistic(300, 80, 3)),
        ("random styled TUI 300x80", tui),
    ] {
        let snapshot = PaneSnapshot {
            pane: 1,
            revision: 1,
            screen,
        };
        let json = serde_json::to_vec(&snapshot).unwrap().len();
        let binary = encode_snapshot(&snapshot);
        assert_eq!(only(&binary), ScreenMessage::Snapshot(snapshot.clone()));
        println!(
            "{name}: JSON {json} bytes, binary {} bytes ({:.1}x smaller)",
            binary.len(),
            json as f64 / binary.len() as f64
        );
        assert!(binary.len() * 10 < json);
    }
    // A one-row change: JSON cell patch versus binary row patch.
    let base = PaneSnapshot {
        pane: 1,
        revision: 1,
        screen: realistic(300, 80, 80),
    };
    let mut next = base.screen.clone();
    for x in 0..20 {
        next.cells[40 * 300 + x] = Cell {
            text: "#".into(),
            ..Cell::default()
        };
    }
    let patch = diff(&base, &next);
    let json = serde_json::to_vec(&patch).unwrap().len();
    let binary = encode_patch(&patch, &next).len();
    println!("one-row patch 300x80: JSON {json} bytes, binary {binary} bytes");
}
