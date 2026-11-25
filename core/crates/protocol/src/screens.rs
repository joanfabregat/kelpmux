//! Binary screen records (`binary-screens`): a compact encoding of pane
//! snapshots and patches. The format is specified in KelpServer's
//! `docs/binary-screens.md`; this module is its only implementation.
//!
//! A record is one framed payload starting with [`RECORD_MARKER`], which no
//! JSON payload starts with. It carries any number of screen messages, each a
//! full snapshot or a patch of whole rows. Decoding untrusted bytes is bounded:
//! geometry, the aggregate cell count, style tables and cell text are checked
//! before anything proportional to them is allocated, and nothing panics.
use crate::types::{
    Cell, Color, Cursor, InputModes, PaneId, PanePatch, PaneSnapshot, Point, Screen,
};
use std::collections::HashMap;
use unicode_width::UnicodeWidthChar;

/// The Welcome capability and the `binary_screens` opt-in it gates.
pub const CAPABILITY: &str = "binary-screens";
/// The first payload byte of a binary record. JSON payloads start with `{`
/// or `"`, never with a NUL byte.
pub const RECORD_MARKER: u8 = 0x00;
/// The format version, the second payload byte.
pub const FORMAT_VERSION: u8 = 1;
/// Pane geometry accepted by the decoder (the stream's snapshot bounds).
pub const MAX_COLS: u16 = 400;
pub const MAX_ROWS: u16 = 160;
/// Most screen messages in one record.
pub const MAX_MESSAGES: usize = 256;
/// Longest cell text in bytes (as `cell_valid`).
pub const MAX_CELL_TEXT: usize = 1024;
/// Longest hyperlink in bytes (as `valid_hyperlink`).
pub const MAX_HYPERLINK: usize = 2048;
/// The default aggregate cell budget of one record: the stream's advertised
/// `subscribed_cells` limit.
pub const DEFAULT_MAX_CELLS: usize = 100_000;
/// Most bytes of cell text and hyperlinks one record decodes to, so a small
/// record cannot expand beyond what a 16 MiB JSON record could carry.
pub const MAX_DECODED_BYTES: usize = 16 * 1024 * 1024;
/// Identical simple cells at least this long are encoded as a repeat run.
const REPEAT_MIN: usize = 8;

const KIND_SNAPSHOT: u8 = 1;
const KIND_PATCH: u8 = 2;
const RUN_TEXT: u64 = 0;
const RUN_REPEAT: u64 = 1;
const RUN_RAW: u64 = 2;
const STYLE_BOLD: u8 = 1;
const STYLE_ITALIC: u8 = 2;
const STYLE_UNDERLINE: u8 = 4;
const STYLE_UNDERLINE_COLOR: u8 = 8;
const STYLE_HYPERLINK: u8 = 16;
const CURSOR_PRESENT: u8 = 1;
const CURSOR_VISIBLE: u8 = 2;

/// Whether a framed payload is a binary record rather than JSON.
pub fn is_binary_record(payload: &[u8]) -> bool {
    payload.first() == Some(&RECORD_MARKER)
}

/// A decoding failure. Messages are static so they never echo untrusted bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError(pub &'static str);
impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid binary screen record: {}", self.0)
    }
}
impl std::error::Error for DecodeError {}
type Result<T> = std::result::Result<T, DecodeError>;

/// A patch that replaces whole rows. `lines` holds `(row, cells)` in
/// increasing row order, each with exactly `cols` cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowPatch {
    pub pane: PaneId,
    pub base: u64,
    pub revision: u64,
    pub cols: u16,
    pub rows: u16,
    pub cursor: Option<Cursor>,
    pub modes: InputModes,
    pub lines: Vec<(u16, Vec<Cell>)>,
}
impl RowPatch {
    /// Apply to the exact baseline (same pane, `base` revision and geometry).
    pub fn apply(&self, snapshot: &PaneSnapshot) -> Result<PaneSnapshot> {
        let screen = &snapshot.screen;
        if self.pane != snapshot.pane
            || self.base != snapshot.revision
            || screen.cols != self.cols
            || screen.rows != self.rows
            || screen.cells.len() != usize::from(self.cols) * usize::from(self.rows)
            || self.revision != self.base.wrapping_add(1)
            || self
                .lines
                .iter()
                .any(|(row, cells)| *row >= self.rows || cells.len() != usize::from(self.cols))
        {
            return Err(DecodeError("patch does not match its baseline"));
        }
        let mut next = snapshot.clone();
        let cols = usize::from(self.cols);
        for (row, cells) in &self.lines {
            let start = usize::from(*row) * cols;
            next.screen.cells[start..start + cols].clone_from_slice(cells);
        }
        next.revision = self.revision;
        next.screen.cursor = self.cursor;
        next.screen.modes = self.modes;
        Ok(next)
    }
    /// The equivalent cell patch: every cell of every changed row.
    pub fn into_pane_patch(self) -> PanePatch {
        let cols = usize::from(self.cols);
        PanePatch {
            pane: self.pane,
            base: self.base,
            revision: self.revision,
            cursor: self.cursor,
            modes: self.modes,
            changes: self
                .lines
                .into_iter()
                .flat_map(|(row, cells)| {
                    let start = usize::from(row) * cols;
                    cells
                        .into_iter()
                        .enumerate()
                        .map(move |(x, cell)| (start + x, cell))
                })
                .collect(),
        }
    }
}

/// One decoded screen message, in record order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenMessage {
    Snapshot(PaneSnapshot),
    Patch(RowPatch),
}
impl ScreenMessage {
    pub fn pane(&self) -> PaneId {
        match self {
            Self::Snapshot(snapshot) => snapshot.pane,
            Self::Patch(patch) => patch.pane,
        }
    }
}

/// Builds one binary record from snapshots and patches.
#[derive(Debug, Default)]
pub struct RecordBuilder {
    body: Vec<u8>,
    count: usize,
}
impl RecordBuilder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn len(&self) -> usize {
        self.count
    }
    /// Append a full snapshot of `screen`.
    pub fn snapshot(&mut self, pane: PaneId, revision: u64, screen: &Screen) {
        self.count += 1;
        let out = &mut self.body;
        out.push(KIND_SNAPSHOT);
        varint(out, pane);
        varint(out, revision);
        header(out, screen.cols, screen.rows, screen.cursor, screen.modes);
        let mut styles = StyleTable::default();
        let mut rows = Vec::new();
        for row in screen.cells.chunks(usize::from(screen.cols).max(1)) {
            encode_row(&mut rows, row, &mut styles);
        }
        styles.write(out);
        out.extend_from_slice(&rows);
    }
    /// Append `patch`, re-encoding each row it touches whole from `screen`,
    /// the screen that `patch` produces.
    pub fn patch(&mut self, patch: &PanePatch, screen: &Screen) {
        self.count += 1;
        let out = &mut self.body;
        out.push(KIND_PATCH);
        varint(out, patch.pane);
        varint(out, patch.revision);
        varint(out, patch.base);
        header(out, screen.cols, screen.rows, patch.cursor, patch.modes);
        let cols = usize::from(screen.cols).max(1);
        let mut changed: Vec<usize> = patch.changes.iter().map(|(i, _)| i / cols).collect();
        changed.sort_unstable();
        changed.dedup();
        changed.retain(|row| (row + 1) * cols <= screen.cells.len());
        let mut styles = StyleTable::default();
        let mut rows = Vec::new();
        varint(&mut rows, changed.len() as u64);
        for row in changed {
            varint(&mut rows, row as u64);
            encode_row(
                &mut rows,
                &screen.cells[row * cols..(row + 1) * cols],
                &mut styles,
            );
        }
        styles.write(out);
        out.extend_from_slice(&rows);
    }
    /// The record payload (without the four-byte length prefix).
    pub fn finish(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.body.len() + 12);
        out.push(RECORD_MARKER);
        out.push(FORMAT_VERSION);
        varint(&mut out, self.count as u64);
        out.extend_from_slice(&self.body);
        out
    }
}

/// A record holding one snapshot.
pub fn encode_snapshot(snapshot: &PaneSnapshot) -> Vec<u8> {
    let mut builder = RecordBuilder::new();
    builder.snapshot(snapshot.pane, snapshot.revision, &snapshot.screen);
    builder.finish()
}
/// A record holding one patch; `screen` is the screen `patch` produces.
pub fn encode_patch(patch: &PanePatch, screen: &Screen) -> Vec<u8> {
    let mut builder = RecordBuilder::new();
    builder.patch(patch, screen);
    builder.finish()
}

/// Decode a record payload. `max_cells` bounds the aggregate cells of its
/// snapshots and changed rows (use the Welcome `limits.subscribed_cells`, or
/// [`DEFAULT_MAX_CELLS`]). Every decoded cell satisfies `cell_valid`.
pub fn decode(bytes: &[u8], max_cells: usize) -> Result<Vec<ScreenMessage>> {
    let mut reader = Reader { bytes, pos: 0 };
    if reader.byte()? != RECORD_MARKER {
        return Err(DecodeError("not a binary record"));
    }
    if reader.byte()? != FORMAT_VERSION {
        return Err(DecodeError("unsupported format version"));
    }
    let count = reader.bounded(MAX_MESSAGES)?;
    let mut budget = Budget {
        cells: max_cells,
        bytes: MAX_DECODED_BYTES,
    };
    let mut messages = Vec::with_capacity(count.min(reader.remaining() / 8));
    let mut panes = std::collections::BTreeSet::new();
    for _ in 0..count {
        let message = decode_message(&mut reader, &mut budget)?;
        if !panes.insert(message.pane()) {
            return Err(DecodeError("duplicate pane"));
        }
        messages.push(message);
    }
    if reader.remaining() != 0 {
        return Err(DecodeError("trailing bytes"));
    }
    Ok(messages)
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}
fn color(out: &mut Vec<u8>, color: Color) {
    match color {
        Color::Default => out.push(0),
        Color::Indexed(index) => out.extend_from_slice(&[1, index]),
        Color::Rgb(r, g, b) => out.extend_from_slice(&[2, r, g, b]),
    }
}
fn header(out: &mut Vec<u8>, cols: u16, rows: u16, cursor: Option<Cursor>, modes: InputModes) {
    varint(out, u64::from(cols));
    varint(out, u64::from(rows));
    match cursor {
        None => out.push(0),
        Some(cursor) => {
            out.push(CURSOR_PRESENT | if cursor.visible { CURSOR_VISIBLE } else { 0 });
            varint(out, u64::from(cursor.position.x));
            varint(out, u64::from(cursor.position.y));
        }
    }
    let flags = [
        modes.application_cursor,
        modes.application_keypad,
        modes.bracketed_paste,
        modes.mouse_click,
        modes.mouse_drag,
        modes.mouse_motion,
        modes.mouse_sgr,
        modes.focus,
    ]
    .iter()
    .enumerate()
    .fold(0u8, |flags, (bit, set)| flags | (u8::from(*set) << bit));
    out.push(flags);
    out.push(modes.keyboard_flags);
}

/// A cell's style: everything but its text and width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct StyleKey<'a> {
    fg: Color,
    bg: Color,
    bold: bool,
    italic: bool,
    underline: bool,
    underline_color: Option<Color>,
    hyperlink: Option<&'a str>,
}
impl<'a> StyleKey<'a> {
    fn of(cell: &'a Cell) -> Self {
        Self {
            fg: cell.fg,
            bg: cell.bg,
            bold: cell.bold,
            italic: cell.italic,
            underline: cell.underline,
            underline_color: cell.underline_color,
            hyperlink: cell.hyperlink.as_deref(),
        }
    }
}
#[derive(Default)]
struct StyleTable<'a> {
    index: HashMap<StyleKey<'a>, u64>,
    order: Vec<StyleKey<'a>>,
    last: Option<(StyleKey<'a>, u64)>,
}
impl<'a> StyleTable<'a> {
    fn get(&mut self, cell: &'a Cell) -> u64 {
        let key = StyleKey::of(cell);
        if let Some((last, index)) = self.last {
            if last == key {
                return index;
            }
        }
        let next = self.order.len() as u64;
        let index = *self.index.entry(key).or_insert(next);
        if index == next {
            self.order.push(key);
        }
        self.last = Some((key, index));
        index
    }
    fn write(&self, out: &mut Vec<u8>) {
        varint(out, self.order.len() as u64);
        for style in &self.order {
            let mut flags = 0;
            for (set, bit) in [
                (style.bold, STYLE_BOLD),
                (style.italic, STYLE_ITALIC),
                (style.underline, STYLE_UNDERLINE),
                (style.underline_color.is_some(), STYLE_UNDERLINE_COLOR),
                (style.hyperlink.is_some(), STYLE_HYPERLINK),
            ] {
                if set {
                    flags |= bit;
                }
            }
            out.push(flags);
            color(out, style.fg);
            color(out, style.bg);
            if let Some(underline) = style.underline_color {
                color(out, underline);
            }
            if let Some(uri) = style.hyperlink {
                varint(out, uri.len() as u64);
                out.extend_from_slice(uri.as_bytes());
            }
        }
    }
}

fn char_width(ch: char) -> usize {
    ch.width().unwrap_or(0)
}
/// How many cells a text run covers starting at `cells[i]`: 1 for a simple
/// narrow cell, 2 for a simple wide cell and its matching spacer, 0 when the
/// cell needs a raw run. A simple cell's text is one character of exactly
/// the cell's width followed only by zero-width characters, so the decoder
/// can split run text at every character of non-zero width.
fn simple(cells: &[Cell], i: usize) -> usize {
    let cell = &cells[i];
    let mut chars = cell.text.chars();
    let Some(first) = chars.next() else {
        return 0;
    };
    let width = char_width(first);
    if width == 0 || width != usize::from(cell.width) || chars.any(|ch| char_width(ch) != 0) {
        return 0;
    }
    match width {
        1 => 1,
        2 => match cells.get(i + 1) {
            Some(spacer)
                if spacer.width == 0
                    && spacer.text.is_empty()
                    && StyleKey::of(spacer) == StyleKey::of(cell) =>
            {
                2
            }
            _ => 0,
        },
        _ => 0,
    }
}
/// Identical simple narrow cells starting at `cells[i]`.
fn repeated(cells: &[Cell], i: usize) -> usize {
    if cells[i].width != 1 || simple(cells, i) != 1 {
        return 0;
    }
    cells[i..]
        .iter()
        .take_while(|cell| **cell == cells[i])
        .count()
}
fn run_header(out: &mut Vec<u8>, length: usize, kind: u64, style: u64) {
    varint(out, ((length as u64) << 2) | kind);
    varint(out, style);
}
fn encode_row<'a>(out: &mut Vec<u8>, row: &'a [Cell], styles: &mut StyleTable<'a>) {
    let default = Cell::default();
    let end = row
        .iter()
        .rposition(|cell| *cell != default)
        .map_or(0, |last| last + 1);
    let row = &row[..end];
    let mut runs = Vec::new();
    let mut count = 0u64;
    let mut i = 0;
    while i < row.len() {
        let style = styles.get(&row[i]);
        count += 1;
        let repeat = repeated(row, i);
        if repeat >= REPEAT_MIN {
            run_header(&mut runs, repeat, RUN_REPEAT, style);
            varint(&mut runs, row[i].text.len() as u64);
            runs.extend_from_slice(row[i].text.as_bytes());
            i += repeat;
        } else if simple(row, i) > 0 {
            let mut text = String::new();
            while i < row.len() && styles.get(&row[i]) == style {
                let step = simple(row, i);
                if step == 0 || repeated(row, i) >= REPEAT_MIN {
                    break;
                }
                text.push_str(&row[i].text);
                i += step;
            }
            run_header(&mut runs, text.len(), RUN_TEXT, style);
            runs.extend_from_slice(text.as_bytes());
        } else {
            let start = i;
            let mut raw = Vec::new();
            while i < row.len()
                && (i == start || simple(row, i) == 0 && styles.get(&row[i]) == style)
            {
                raw.push(row[i].width);
                varint(&mut raw, row[i].text.len() as u64);
                raw.extend_from_slice(row[i].text.as_bytes());
                i += 1;
            }
            run_header(&mut runs, i - start, RUN_RAW, style);
            runs.extend_from_slice(&raw);
        }
    }
    varint(out, count);
    out.extend_from_slice(&runs);
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }
    fn byte(&mut self) -> Result<u8> {
        let byte = *self
            .bytes
            .get(self.pos)
            .ok_or(DecodeError("truncated record"))?;
        self.pos += 1;
        Ok(byte)
    }
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        if length > self.remaining() {
            return Err(DecodeError("truncated record"));
        }
        let bytes = &self.bytes[self.pos..self.pos + length];
        self.pos += length;
        Ok(bytes)
    }
    fn varint(&mut self) -> Result<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            let bits = u64::from(byte & 0x7f);
            if shift == 63 && bits > 1 {
                return Err(DecodeError("integer overflow"));
            }
            value |= bits << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(DecodeError("integer overflow"))
    }
    fn bounded(&mut self, max: usize) -> Result<usize> {
        let value = self.varint()?;
        if value > max as u64 {
            return Err(DecodeError("value out of range"));
        }
        Ok(value as usize)
    }
    fn text(&mut self, length: usize) -> Result<&'a str> {
        std::str::from_utf8(self.take(length)?).map_err(|_| DecodeError("invalid UTF-8"))
    }
    fn color(&mut self) -> Result<Color> {
        Ok(match self.byte()? {
            0 => Color::Default,
            1 => Color::Indexed(self.byte()?),
            2 => Color::Rgb(self.byte()?, self.byte()?, self.byte()?),
            _ => return Err(DecodeError("unknown colour tag")),
        })
    }
}

#[derive(Debug, Clone)]
struct Style {
    fg: Color,
    bg: Color,
    bold: bool,
    italic: bool,
    underline: bool,
    underline_color: Option<Color>,
    hyperlink: Option<String>,
    /// The hyperlink's length, charged to the budget for every cell.
    link: usize,
}
impl Style {
    fn cell(&self, text: String, width: u8) -> Cell {
        Cell {
            text,
            width,
            fg: self.fg,
            bg: self.bg,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            underline_color: self.underline_color,
            hyperlink: self.hyperlink.clone(),
        }
    }
}

fn decode_message(reader: &mut Reader<'_>, budget: &mut Budget) -> Result<ScreenMessage> {
    let kind = reader.byte()?;
    if kind != KIND_SNAPSHOT && kind != KIND_PATCH {
        return Err(DecodeError("unknown message kind"));
    }
    let pane = reader.varint()?;
    let revision = reader.varint()?;
    if pane == 0 || revision == 0 {
        return Err(DecodeError("zero pane or revision"));
    }
    let base = if kind == KIND_PATCH {
        let base = reader.varint()?;
        if base.checked_add(1) != Some(revision) {
            return Err(DecodeError("patch revision is not base + 1"));
        }
        base
    } else {
        0
    };
    let cols = reader.bounded(usize::from(MAX_COLS))?;
    let rows = reader.bounded(usize::from(MAX_ROWS))?;
    if cols == 0 || rows == 0 {
        return Err(DecodeError("empty geometry"));
    }
    let cursor = match reader.byte()? {
        0 => None,
        flags @ (CURSOR_PRESENT | 3) => {
            let x = reader.bounded(cols - 1)? as u16;
            let y = reader.bounded(rows - 1)? as u16;
            Some(Cursor {
                position: Point { x, y },
                visible: flags & CURSOR_VISIBLE != 0,
            })
        }
        _ => return Err(DecodeError("invalid cursor flags")),
    };
    let flags = reader.byte()?;
    let bit = |n: u8| flags & (1 << n) != 0;
    let modes = InputModes {
        application_cursor: bit(0),
        application_keypad: bit(1),
        bracketed_paste: bit(2),
        mouse_click: bit(3),
        mouse_drag: bit(4),
        mouse_motion: bit(5),
        mouse_sgr: bit(6),
        focus: bit(7),
        keyboard_flags: reader.byte()?,
    };
    let styles = decode_styles(reader, cols * rows)?;
    let (cols16, rows16) = (cols as u16, rows as u16);
    if kind == KIND_SNAPSHOT {
        budget.cells(cols * rows)?;
        let mut cells = Vec::with_capacity(cols * rows);
        for _ in 0..rows {
            decode_row(reader, &styles, cols, &mut cells, budget)?;
        }
        return Ok(ScreenMessage::Snapshot(PaneSnapshot {
            pane,
            revision,
            screen: Screen {
                cols: cols16,
                rows: rows16,
                cells,
                cursor,
                modes,
            },
        }));
    }
    let changed = reader.bounded(rows)?;
    budget.cells(changed * cols)?;
    let mut lines = Vec::with_capacity(changed);
    let mut previous = None;
    for _ in 0..changed {
        let row = reader.bounded(rows - 1)?;
        if previous.is_some_and(|previous| row <= previous) {
            return Err(DecodeError("patch rows out of order"));
        }
        previous = Some(row);
        let mut cells = Vec::with_capacity(cols);
        decode_row(reader, &styles, cols, &mut cells, budget)?;
        lines.push((row as u16, cells));
    }
    Ok(ScreenMessage::Patch(RowPatch {
        pane,
        base,
        revision,
        cols: cols16,
        rows: rows16,
        cursor,
        modes,
        lines,
    }))
}
/// What one record may still produce: cells, and bytes of cell text and
/// hyperlinks (a hyperlink is copied into every cell of its style).
struct Budget {
    cells: usize,
    bytes: usize,
}
impl Budget {
    fn cells(&mut self, cells: usize) -> Result<()> {
        self.cells = self
            .cells
            .checked_sub(cells)
            .ok_or(DecodeError("record exceeds the cell budget"))?;
        Ok(())
    }
    fn bytes(&mut self, bytes: usize) -> Result<()> {
        self.bytes = self
            .bytes
            .checked_sub(bytes)
            .ok_or(DecodeError("record exceeds the decoded size budget"))?;
        Ok(())
    }
}
fn decode_styles(reader: &mut Reader<'_>, max: usize) -> Result<Vec<Style>> {
    let count = reader.bounded(max)?;
    // Every style takes at least three bytes.
    if count > reader.remaining() / 3 {
        return Err(DecodeError("truncated style table"));
    }
    let mut styles = Vec::with_capacity(count);
    for _ in 0..count {
        let flags = reader.byte()?;
        if flags & !0x1f != 0 {
            return Err(DecodeError("unknown style flags"));
        }
        let fg = reader.color()?;
        let bg = reader.color()?;
        let underline_color = if flags & STYLE_UNDERLINE_COLOR != 0 {
            Some(reader.color()?)
        } else {
            None
        };
        let hyperlink = if flags & STYLE_HYPERLINK != 0 {
            let length = reader.bounded(MAX_HYPERLINK)?;
            let uri = reader.text(length)?;
            if !crate::types::valid_hyperlink(uri) {
                return Err(DecodeError("invalid hyperlink"));
            }
            Some(uri.to_owned())
        } else {
            None
        };
        styles.push(Style {
            fg,
            bg,
            bold: flags & STYLE_BOLD != 0,
            italic: flags & STYLE_ITALIC != 0,
            underline: flags & STYLE_UNDERLINE != 0,
            underline_color,
            link: hyperlink.as_ref().map_or(0, String::len),
            hyperlink,
        });
    }
    Ok(styles)
}
/// Decode one row and append exactly `cols` cells to `cells`.
fn decode_row(
    reader: &mut Reader<'_>,
    styles: &[Style],
    cols: usize,
    cells: &mut Vec<Cell>,
    budget: &mut Budget,
) -> Result<()> {
    let start = cells.len();
    let runs = reader.bounded(cols)?;
    let full = |cells: &Vec<Cell>| cells.len() - start >= cols;
    for _ in 0..runs {
        let header = reader.varint()?;
        let (length, kind) = (header >> 2, header & 3);
        let style = styles
            .get(reader.bounded(styles.len().saturating_sub(1))?)
            .ok_or(DecodeError("style index out of range"))?;
        if length == 0 {
            return Err(DecodeError("empty run"));
        }
        match kind {
            RUN_TEXT => {
                let length = usize::try_from(length).map_err(|_| DecodeError("run too long"))?;
                let text = reader.text(length)?;
                budget.bytes(text.len())?;
                let mut current = None;
                for ch in text.chars() {
                    let width = char_width(ch);
                    if width == 0 {
                        let index = current.ok_or(DecodeError("text run starts with a mark"))?;
                        let cell: &mut Cell = &mut cells[index];
                        if cell.text.len() + ch.len_utf8() > MAX_CELL_TEXT {
                            return Err(DecodeError("cell text too long"));
                        }
                        cell.text.push(ch);
                        continue;
                    }
                    if full(cells) || width == 2 && cells.len() - start + 2 > cols {
                        return Err(DecodeError("row too long"));
                    }
                    budget.bytes(style.link * width)?;
                    current = Some(cells.len());
                    cells.push(style.cell(ch.to_string(), width as u8));
                    if width == 2 {
                        cells.push(style.cell(String::new(), 0));
                    }
                }
            }
            RUN_REPEAT => {
                let count = length as usize;
                if length > cols as u64 || cells.len() - start + count > cols {
                    return Err(DecodeError("row too long"));
                }
                let size = reader.bounded(MAX_CELL_TEXT)?;
                let text = reader.text(size)?;
                let mut chars = text.chars();
                if !chars.next().is_some_and(|ch| char_width(ch) == 1)
                    || chars.any(|ch| char_width(ch) != 0)
                {
                    return Err(DecodeError("invalid repeated cell"));
                }
                budget.bytes(count * (text.len() + style.link))?;
                let cell = style.cell(text.to_owned(), 1);
                cells.extend(std::iter::repeat_n(cell, count));
            }
            RUN_RAW => {
                if length > cols as u64 || cells.len() - start + length as usize > cols {
                    return Err(DecodeError("row too long"));
                }
                for _ in 0..length {
                    let width = reader.byte()?;
                    let size = reader.bounded(MAX_CELL_TEXT)?;
                    let text = reader.text(size)?;
                    budget.bytes(text.len() + style.link)?;
                    cells.push(style.cell(text.to_owned(), width));
                }
            }
            _ => return Err(DecodeError("unknown run kind")),
        }
    }
    let row = cells.len() - start;
    if row > cols {
        return Err(DecodeError("row too long"));
    }
    cells.resize(start + cols, Cell::default());
    for (x, cell) in cells[start..].iter().enumerate() {
        if !crate::cell_valid(cell) || cell.width == 2 && x + 1 == cols {
            return Err(DecodeError("invalid cell"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
