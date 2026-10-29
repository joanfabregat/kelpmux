//! Stateful Alacritty adapter. This module owns no PTY or event loop.
use super::protocol::*;
use crate::Result;
use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point as TermPoint, Side},
    selection::{Selection, SelectionType},
    term::{
        Config, Term, TermMode, cell::Flags, color::Colors, point_to_viewport, viewport_to_point,
    },
    vte::ansi::{Color as AnsiColor, Processor, Rgb},
};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub enum Effect {
    WritePty(Vec<u8>),
    Title(String),
    Bell,
    Clipboard(String),
    Program(super::program_status::Event),
}

// Callbacks run synchronously inside the parser, never block on consumers, and
// cannot grow a queue without limit when an application floods OSC/DSR requests.
#[derive(Clone, Default)]
struct Listener(Arc<Mutex<Vec<Event>>>);
impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        if matches!(
            event,
            Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange
        ) {
            return;
        }
        if let Ok(mut queue) = self.0.try_lock() {
            if queue.len() < 256 {
                queue.push(event);
            }
        }
    }
}
struct Size {
    cols: usize,
    rows: usize,
}
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Terminal {
    program: super::program_status::Parser,
    theme: [Option<Rgb>; 2],
    term: Term<Listener>,
    parser: Processor,
    listener: Listener,
    escape: Vec<u8>,
    primary_history: Option<Vec<HistoryRow>>,
    selection_anchor: Option<TermPoint>,
    selection_head: Option<TermPoint>,
}
#[derive(Clone)]
struct HistoryRow {
    line: i32,
    text: String,
    wrapped: bool,
}
impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Self {
        let listener = Listener::default();
        let size = Size {
            cols: usize::from(cols.max(2)),
            rows: usize::from(rows.max(1)),
        };
        Self {
            program: Default::default(),
            theme: [None; 2],
            term: Term::new(
                Config {
                    kitty_keyboard: true,
                    ..Config::default()
                },
                &size,
                listener.clone(),
            ),
            parser: Processor::new(),
            listener,
            escape: Vec::new(),
            primary_history: None,
            selection_anchor: None,
            selection_head: None,
        }
    }
    /// Query fallbacks describe the controller's theme without painting default
    /// cells opaque or overriding application OSC palette changes.
    pub fn set_theme(&mut self, theme: [Option<[u8; 3]>; 2]) {
        self.theme = theme.map(|color| color.map(|[r, g, b]| Rgb { r, g, b }));
    }
    fn default_color(&self, index: usize) -> Rgb {
        match index {
            256 | 257 => self.theme[index - 256].unwrap_or_else(|| default_rgb(index)),
            _ => default_rgb(index),
        }
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Effect> {
        let mut effects = Vec::new();
        for chunk in bytes
            .split_inclusive(|byte| *byte == 0x07 || *byte == b'\\')
            .flat_map(|part| part.chunks(4096))
        {
            for event in self.program.feed(chunk) {
                effects.push(match event {
                    super::program_status::Event::Query => {
                        Effect::WritePty(super::program_status::REPLY.to_vec())
                    }
                    event => Effect::Program(event),
                });
            }
            self.advance(chunk);
            let events = self
                .listener
                .0
                .lock()
                .map(|mut q| std::mem::take(&mut *q))
                .unwrap_or_default();
            for event in events {
                let effect = match event {
                    Event::PtyWrite(text) => Some(Effect::WritePty(text.into_bytes())),
                    Event::Title(text) => Some(Effect::Title(safe_text(&text))),
                    Event::ResetTitle => Some(Effect::Title(String::new())),
                    Event::Bell => Some(Effect::Bell),
                    Event::ClipboardStore(_, text) => Some(Effect::Clipboard(text)),
                    Event::ColorRequest(index, format) => {
                        let rgb =
                            self.term.colors()[index].unwrap_or_else(|| self.default_color(index));
                        Some(Effect::WritePty(format(rgb).into_bytes()))
                    }
                    Event::TextAreaSizeRequest(format) => Some(Effect::WritePty(
                        format(WindowSize {
                            num_lines: self.term.screen_lines() as u16,
                            num_cols: self.term.columns() as u16,
                            // Native composition has no pixel geometry to report.
                            cell_width: 0,
                            cell_height: 0,
                        })
                        .into_bytes(),
                    )),
                    // Clipboard reads deliberately remain disabled by Config::osc52.
                    _ => None,
                };
                if let Some(effect) = effect {
                    effects.push(effect);
                }
            }
        }
        if let (Some(anchor), Some(head)) = (self.selection_anchor, self.selection_head) {
            if let Some(range) = self
                .term
                .selection
                .as_ref()
                .and_then(|selection| selection.to_range(&self.term))
            {
                let reversed = (anchor.line.0, anchor.column.0) > (head.line.0, head.column.0);
                self.selection_anchor = Some(if reversed { range.end } else { range.start });
                self.selection_head = Some(if reversed { range.start } else { range.end });
            } else {
                self.selection_anchor = None;
                self.selection_head = None;
            }
        }
        effects
    }
    fn advance(&mut self, bytes: &[u8]) {
        let mut index = 0;
        while index < bytes.len() {
            if self.escape.is_empty() && bytes[index] != 0x1b {
                let end = bytes[index..]
                    .iter()
                    .position(|b| *b == 0x1b)
                    .map_or(bytes.len(), |offset| index + offset);
                self.advance_parser(&bytes[index..end]);
                index = end;
                continue;
            }
            let byte = bytes[index];
            if byte == 0x1b {
                self.escape.clear();
                self.escape.push(byte);
            } else {
                self.escape.push(byte);
                let complete = self.escape.len() > 2 && (0x40..=0x7e).contains(&byte);
                if complete {
                    if !self.term.mode().contains(TermMode::ALT_SCREEN)
                        && byte == b'h'
                        && self.escape.starts_with(b"\x1b[?")
                    {
                        let params = &self.escape[3..self.escape.len() - 1];
                        if params
                            .split(|b| *b == b';')
                            .any(|p| matches!(p, b"47" | b"1047" | b"1049"))
                        {
                            self.primary_history = Some(self.rows());
                        }
                    }
                    self.escape.clear();
                } else if self.escape.len() > 64 || (self.escape.len() == 2 && byte != b'[') {
                    self.escape.clear();
                }
            }
            self.parser
                .advance(&mut self.term, &bytes[index..index + 1]);
            if !self.term.mode().contains(TermMode::ALT_SCREEN) && self.escape.is_empty() {
                self.primary_history = None;
            }
            index += 1;
        }
    }
    fn advance_parser(&mut self, mut bytes: &[u8]) {
        // VTE 0.15's partial UTF-8 fast path can consume intervening ASCII
        // without printing it when a later codepoint is incomplete. Keep ASCII
        // batching, but complete Unicode one byte at a time so arbitrary PTY
        // read boundaries cannot lose text. The parser retains UTF-8 state.
        while !bytes.is_empty() {
            let ascii = bytes
                .iter()
                .position(|b| !b.is_ascii())
                .unwrap_or(bytes.len());
            if ascii != 0 {
                self.parser.advance(&mut self.term, &bytes[..ascii]);
                bytes = &bytes[ascii..];
            }
            if let Some((byte, rest)) = bytes.split_first() {
                self.parser
                    .advance(&mut self.term, std::slice::from_ref(byte));
                bytes = rest;
            }
        }
    }
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.selection_anchor = None;
        self.selection_head = None;
        self.term.resize(Size {
            cols: usize::from(cols.max(2)),
            rows: usize::from(rows.max(1)),
        });
    }
    pub fn snapshot(&self) -> Screen {
        let cols = self.term.columns() as u16;
        let rows = self.term.screen_lines() as u16;
        let content = self.term.renderable_content();
        let mut cells = vec![Cell::default(); usize::from(cols) * usize::from(rows)];
        let mut wraps = vec![false; usize::from(rows)];
        for indexed in content.display_iter {
            let Some(point) = point_to_viewport(content.display_offset, indexed.point) else {
                continue;
            };
            if point.line >= usize::from(rows) || point.column.0 >= usize::from(cols) {
                continue;
            }
            let source = indexed.cell;
            let flags = source.flags;
            if point.column.0 + 1 == usize::from(cols) {
                wraps[point.line] = flags.contains(Flags::WRAPLINE);
            }
            let mut fg = color(source.fg, content.colors);
            let mut bg = color(source.bg, content.colors);
            if flags.contains(Flags::INVERSE) {
                // Defaults are role-dependent in ANSI: resolve before swapping.
                if fg == Color::Default {
                    fg = rgb_color(self.default_color(256));
                }
                if bg == Color::Default {
                    bg = rgb_color(self.default_color(257));
                }
                std::mem::swap(&mut fg, &mut bg);
            }
            if content.selection.as_ref().is_some_and(|selection| {
                selection.contains_cell(&indexed, content.cursor.point, content.cursor.shape)
            }) {
                fg = Color::Rgb(0, 0, 0);
                bg = Color::Rgb(180, 200, 225);
            }
            let spacer = flags.contains(Flags::WIDE_CHAR_SPACER);
            let mut text = if spacer {
                String::new()
            } else if flags.intersects(Flags::HIDDEN | Flags::LEADING_WIDE_CHAR_SPACER) {
                " ".into()
            } else {
                safe_text(&source.c.to_string())
            };
            if !spacer && !flags.contains(Flags::HIDDEN) {
                if let Some(extra) = source.zerowidth() {
                    text.extend(extra.iter().copied().filter(|c| !c.is_control()));
                }
            }
            cells[point.line * usize::from(cols) + point.column.0] = Cell {
                text,
                width: if spacer {
                    0
                } else if flags.contains(Flags::WIDE_CHAR) {
                    2
                } else {
                    1
                },
                fg,
                bg,
                bold: flags.contains(Flags::BOLD),
                italic: flags.contains(Flags::ITALIC),
                underline: flags.intersects(Flags::ALL_UNDERLINES),
                underline_color: source
                    .underline_color()
                    .map(|value| color(value, content.colors)),
                hyperlink: source
                    .hyperlink()
                    .map(|link| link.uri().to_owned())
                    .filter(|uri| valid_hyperlink(uri) && !flags.contains(Flags::HIDDEN)),
            };
        }
        link_plain_urls(&mut cells, usize::from(cols), &wraps);
        let mode = content.mode;
        let cursor = point_to_viewport(content.display_offset, content.cursor.point)
            .filter(|p| p.line < usize::from(rows) && p.column.0 < usize::from(cols))
            .map(|p| Cursor {
                position: Point {
                    x: p.column.0 as u16,
                    y: p.line as u16,
                },
                visible: mode.contains(TermMode::SHOW_CURSOR) && content.display_offset == 0,
            });
        Screen {
            cols,
            rows,
            cells,
            cursor,
            modes: InputModes {
                keyboard_flags: [
                    TermMode::DISAMBIGUATE_ESC_CODES,
                    TermMode::REPORT_EVENT_TYPES,
                    TermMode::REPORT_ALTERNATE_KEYS,
                    TermMode::REPORT_ALL_KEYS_AS_ESC,
                    TermMode::REPORT_ASSOCIATED_TEXT,
                ]
                .iter()
                .enumerate()
                .fold(0, |flags, (i, mode)| {
                    flags
                        | if mode.intersects(content.mode) {
                            1 << i
                        } else {
                            0
                        }
                }),
                application_cursor: mode.contains(TermMode::APP_CURSOR),
                application_keypad: mode.contains(TermMode::APP_KEYPAD),
                bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
                mouse_click: mode.contains(TermMode::MOUSE_REPORT_CLICK),
                mouse_drag: mode.contains(TermMode::MOUSE_DRAG),
                mouse_motion: mode.contains(TermMode::MOUSE_MOTION),
                mouse_sgr: mode.contains(TermMode::SGR_MOUSE),
                focus: mode.contains(TermMode::FOCUS_IN_OUT),
            },
        }
    }
    pub fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }
    fn rows(&self) -> Vec<HistoryRow> {
        self.physical_rows(i32::MIN, usize::MAX)
    }
    fn physical_rows(&self, start: i32, limit: usize) -> Vec<HistoryRow> {
        let grid = self.term.grid();
        let first = start.max(-(grid.history_size() as i32));
        (first..grid.screen_lines() as i32)
            .take(limit)
            .map(|line| {
                let row = &grid[Line(line)];
                let wrapped = row[Column(grid.columns() - 1)]
                    .flags
                    .contains(Flags::WRAPLINE);
                let mut text = String::new();
                for col in 0..grid.columns() {
                    let cell = &row[Column(col)];
                    if cell
                        .flags
                        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                    {
                        continue;
                    }
                    let ch = if cell.flags.contains(Flags::HIDDEN) {
                        ' '
                    } else {
                        cell.c
                    };
                    if !ch.is_control() {
                        text.push(ch);
                    }
                    if !cell.flags.contains(Flags::HIDDEN) {
                        if let Some(extra) = cell.zerowidth() {
                            text.extend(
                                extra.iter().take(32).copied().filter(|ch| !ch.is_control()),
                            );
                        }
                    }
                    if text.len() >= 16 * 1024 {
                        break;
                    }
                }
                if !wrapped {
                    text.truncate(text.trim_end().len());
                }
                HistoryRow {
                    line,
                    text,
                    wrapped,
                }
            })
            .collect()
    }
    /// Physical primary rows without changing display offset. Negative indices
    /// are scrollback; zero is the first live primary row. At most 1,000 rows.
    pub fn history(&self, start: i32, limit: usize) -> Vec<(i32, String)> {
        if let Some(rows) = &self.primary_history {
            rows.iter()
                .filter(|row| row.line >= start)
                .take(limit.min(1000))
                .map(|row| (row.line, row.text.clone()))
                .collect()
        } else {
            self.physical_rows(start, limit.min(1000))
                .into_iter()
                .map(|row| (row.line, row.text))
                .collect()
        }
    }
    /// Literal Unicode search over joined wrapped primary rows. Bounds protect
    /// interactive searches from unbounded queries/results/very long wraps.
    /// While alternate mode is active, its primary snapshot and current active
    /// lines are searchable. Coordinates refer to their respective buffers;
    /// return to primary mode before revealing cached primary results.
    pub fn search(&self, query: &str, limit: usize) -> Vec<(i32, String)> {
        if query.is_empty()
            || query.len() > 256
            || query.chars().any(char::is_control)
            || limit == 0
        {
            return Vec::new();
        }
        let owned;
        let rows = if let Some(rows) = &self.primary_history {
            rows.as_slice()
        } else {
            owned = self.rows();
            owned.as_slice()
        };
        let mut hits = Self::search_rows(rows, query, limit.min(256));
        if self.is_alternate() && self.primary_history.is_some() && hits.len() < limit.min(256) {
            hits.extend(Self::search_rows(
                &self.rows(),
                query,
                limit.min(256) - hits.len(),
            ));
        }
        hits
    }
    fn search_rows(rows: &[HistoryRow], query: &str, limit: usize) -> Vec<(i32, String)> {
        let mut hits = Vec::new();
        let mut text = String::new();
        let mut first = None;
        for row in rows {
            first.get_or_insert(row.line);
            let mut end = row.text.len().min(MAX_INPUT - text.len());
            while !row.text.is_char_boundary(end) {
                end -= 1;
            }
            text.push_str(&row.text[..end]);
            if !row.wrapped {
                if text.contains(query) {
                    hits.push((first.unwrap_or(row.line), std::mem::take(&mut text)));
                    if hits.len() >= limit {
                        break;
                    }
                }
                text.clear();
                first = None;
            }
        }
        if let Some(first) = first {
            if hits.len() < limit && text.contains(query) {
                hits.push((first, text));
            }
        }
        hits
    }
    pub fn is_alternate(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }
    pub fn scroll_to(&mut self, line: i32) -> Result<()> {
        if self.is_alternate() {
            return Err(
                "leave the application's alternate screen to reveal primary history".into(),
            );
        }
        let grid = self.term.grid();
        let offset = (-i64::from(line)).max(0).min(grid.history_size() as i64);
        self.term.scroll_display(Scroll::Delta(
            (offset - grid.display_offset() as i64) as i32,
        ));
        Ok(())
    }
    pub fn selection_start(&mut self, point: Point) -> Result<String> {
        self.select(point, point)
    }
    /// Move the selection head through row/scrollback boundaries, with a
    /// maximum 16,384-cell span. Returned clipboard text is capped at 64 KiB.
    pub fn selection_move(&mut self, dx: i32, dy: i32) -> Result<String> {
        let anchor = self
            .selection_anchor
            .ok_or("keyboard selection has not started")?;
        let head = self.selection_head.unwrap_or(anchor);
        let cols = self.term.columns() as i64;
        let first = -(self.term.grid().history_size() as i64) * cols;
        let last = self.term.screen_lines() as i64 * cols - 1;
        let anchor_index = i64::from(anchor.line.0) * cols + anchor.column.0 as i64;
        let target = (i64::from(head.line.0) * cols
            + head.column.0 as i64
            + i64::from(dx)
            + i64::from(dy) * cols)
            .clamp(first, last)
            .clamp(anchor_index - 16384, anchor_index + 16384);
        let point = TermPoint::new(
            Line(target.div_euclid(cols) as i32),
            Column(target.rem_euclid(cols) as usize),
        );
        self.selection_head = Some(point);
        let backward = point < anchor;
        let mut selection = Selection::new(
            SelectionType::Simple,
            anchor,
            if backward { Side::Right } else { Side::Left },
        );
        selection.update(point, if backward { Side::Left } else { Side::Right });
        self.term.selection = Some(selection);
        if !self.is_alternate() {
            self.scroll_to(point.line.0)?;
        }
        Ok(self.selection().unwrap_or_default())
    }
    pub fn select(&mut self, start: Point, end: Point) -> Result<String> {
        let valid = |p: Point| {
            usize::from(p.x) < self.term.columns() && usize::from(p.y) < self.term.screen_lines()
        };
        if !valid(start) || !valid(end) {
            return Err("selection is outside the terminal viewport".into());
        }
        let point = |p: Point| {
            viewport_to_point(
                self.term.grid().display_offset(),
                TermPoint::new(usize::from(p.y), Column(usize::from(p.x))),
            )
        };
        let (start, end) = if (start.y, start.x) <= (end.y, end.x) {
            (start, end)
        } else {
            (end, start)
        };
        let mut selection = Selection::new(SelectionType::Simple, point(start), Side::Left);
        selection.update(point(end), Side::Right);
        self.selection_anchor = Some(point(start));
        self.selection_head = Some(point(end));
        self.term.selection = Some(selection);
        Ok(self.selection().unwrap_or_default())
    }
    pub fn selection(&self) -> Option<String> {
        self.term.selection_to_string().map(|mut text| {
            let mut end = text.len().min(MAX_INPUT);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text
        })
    }
}

/// Ordinary URLs need explicit links once separate pane streams are composed:
/// an outer terminal cannot reconstruct wrapping inside one grid column.
fn link_plain_urls(cells: &mut [Cell], cols: usize, wraps: &[bool]) {
    let mut start = 0;
    for (row, wrapped) in wraps.iter().enumerate() {
        if *wrapped && row + 1 < wraps.len() {
            continue;
        }
        let end = (row + 1) * cols;
        let mut text = String::new();
        let mut spans = Vec::new();
        for (offset, cell) in cells[start..end].iter().enumerate() {
            let from = text.len();
            text.push_str(&cell.text);
            spans.push((start + offset, from, text.len()));
        }
        for (begin, _) in text.match_indices("http") {
            let tail = &text[begin..];
            if !tail.starts_with("http://") && !tail.starts_with("https://") {
                continue;
            }
            let quoted = text[..begin].ends_with('\'');
            let mut brackets = Vec::new();
            let length = tail
                .char_indices()
                .find_map(|(offset, c)| {
                    if c.is_control()
                        || c.is_whitespace()
                        || matches!(c, '<' | '>' | '"' | '`')
                        || c == '\'' && quoted
                    {
                        return Some(offset);
                    }
                    match c {
                        '(' | '[' | '{' => brackets.push(c),
                        ')' | ']' | '}' => {
                            let opening = match c {
                                ')' => '(',
                                ']' => '[',
                                _ => '{',
                            };
                            if brackets.pop() != Some(opening) {
                                return Some(offset);
                            }
                        }
                        _ => (),
                    }
                    None
                })
                .unwrap_or(tail.len());
            let uri = tail[..length].trim_end_matches(['.', ',', ';', ':', '!', '?']);
            if !valid_hyperlink(uri) {
                continue;
            }
            let end = begin + uri.len();
            for &(index, from, to) in &spans {
                if from < end && to > begin && cells[index].hyperlink.is_none() {
                    cells[index].hyperlink = Some(uri.to_owned());
                }
            }
        }
        start = end;
    }
}

pub(crate) fn safe_text(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}
fn rgb_color(rgb: Rgb) -> Color {
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}
fn color(value: AnsiColor, colors: &Colors) -> Color {
    let index = match value {
        AnsiColor::Spec(rgb) => return rgb_color(rgb),
        AnsiColor::Indexed(i) => usize::from(i),
        AnsiColor::Named(n) => n as usize,
    };
    if let Some(rgb) = colors[index] {
        return rgb_color(rgb);
    }
    if index < 256 {
        Color::Indexed(index as u8)
    } else if index == 256 || index == 257 {
        Color::Default
    } else {
        rgb_color(default_rgb(index))
    }
}
fn default_rgb(index: usize) -> Rgb {
    const ANSI: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    let (r, g, b) = match index {
        0..=15 => ANSI[index],
        16..=231 => {
            let i = index - 16;
            let c = |v| if v == 0 { 0 } else { (55 + v * 40) as u8 };
            (c(i / 36), c(i / 6 % 6), c(i % 6))
        }
        232..=255 => {
            let v = (8 + (index - 232) * 10) as u8;
            (v, v, v)
        }
        257 => (0, 0, 0),
        259..=266 => {
            let (r, g, b) = ANSI[index - 259];
            (r / 2, g / 2, b / 2)
        }
        _ => (229, 229, 229),
    };
    Rgb { r, g, b }
}
