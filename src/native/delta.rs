//! Bounded revisioned cell updates. Geometry changes require a full frame.
use super::protocol::{Cell, Frame, MAX_MESSAGE, validate_size};
use crate::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io::{self, Write},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Patch {
    pub base: u64,
    pub revision: u64,
    /// Complete current metadata, with an empty cell vector.
    pub frame: Frame,
    pub changes: Vec<(usize, Cell)>,
}

struct Count {
    bytes: usize,
    limit: usize,
}
impl Write for Count {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| io::Error::other("serialized frame exceeds limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn encoded_size(value: &impl Serialize, limit: usize) -> Option<usize> {
    let mut count = Count { bytes: 0, limit };
    serde_json::to_writer(&mut count, value).ok()?;
    Some(count.bytes)
}
fn shape(frame: &Frame) -> Result<usize> {
    validate_size(frame.cols, frame.rows)?;
    let cells = usize::from(frame.cols) * usize::from(frame.rows);
    if frame.panes.len() > 10
        || frame.cursor.is_some_and(|cursor| {
            cursor.position.x >= frame.cols || cursor.position.y >= frame.rows
        })
    {
        return Err("invalid delta frame metadata".into());
    }
    for view in &frame.panes {
        for rect in [view.rect, view.body] {
            if u32::from(rect.x) + u32::from(rect.cols) > u32::from(frame.cols)
                || u32::from(rect.y) + u32::from(rect.rows) > u32::from(frame.rows)
            {
                return Err("delta pane is outside its frame".into());
            }
        }
    }
    Ok(cells)
}
pub(crate) fn cell_valid(cell: &Cell) -> bool {
    if cell.width > 2
        || cell.text.len() > 1024
        || cell.text.chars().any(char::is_control)
        || cell
            .hyperlink
            .as_ref()
            .is_some_and(|uri| !super::protocol::valid_hyperlink(uri))
    {
        return false;
    }
    let mut chars = cell.text.chars();
    if cell.width == 0 {
        return cell.text.is_empty();
    }
    if let Some(first) = chars.next() {
        unicode_width::UnicodeWidthChar::width(first).unwrap_or(0) == usize::from(cell.width)
            && chars.all(|ch| unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) == 0)
    } else {
        true
    }
}
fn same_geometry(old: &Frame, new: &Frame) -> bool {
    old.cols == new.cols
        && old.rows == new.rows
        && old.panes.len() == new.panes.len()
        && old.panes.iter().zip(&new.panes).all(|(a, b)| {
            a.pane.id == b.pane.id && a.rect == b.rect && a.body == b.body && a.overlay == b.overlay
        })
}
fn metadata(frame: &Frame) -> Frame {
    Frame {
        cols: frame.cols,
        rows: frame.rows,
        cells: Vec::new(),
        cursor: frame.cursor,
        panes: frame.panes.clone(),
        active: frame.active,
        reading: frame.reading,
        controller: frame.controller,
        help: frame.help,
        notice: frame.notice.clone(),
        ui: frame.ui.clone(),
    }
}
/// Returns None when a full frame is required or a patch is not smaller.
/// `base` identifies the client's last fully delivered frame; full frames reset
/// that revision according to the transport's contract.
pub fn diff(old: &Frame, new: &Frame, base: u64) -> Option<Patch> {
    let revision = base.checked_add(1)?;
    let count = shape(new).ok()?;
    if base == 0
        || !same_geometry(old, new)
        || old.cells.len() != count
        || new.cells.len() != count
        || new.cells.iter().any(|cell| !cell_valid(cell))
    {
        return None;
    }
    let changes = old
        .cells
        .iter()
        .zip(&new.cells)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(index, (_, cell))| (index, cell.clone()))
        .collect();
    let patch = Patch {
        base,
        revision,
        frame: metadata(new),
        changes,
    };
    let patch_size = encoded_size(&patch, MAX_MESSAGE - 1024)?;
    // Any valid cell has at least these fields/values (zero-width continuation
    // and true booleans are the shortest encodings). A small patch can therefore
    // avoid reserializing every unchanged cell merely to prove it is smaller.
    const MIN_CELL: usize = br#"{"text":"","width":0,"fg":"Default","bg":"Default","bold":true,"italic":true,"underline":true}"#.len();
    let minimum_full = count * (MIN_CELL + 1);
    if patch_size < minimum_full {
        return Some(patch);
    }
    (patch_size < encoded_size(new, MAX_MESSAGE - 1024)?).then_some(patch)
}
/// Validates bounds and revision before cloning anything supplied by a peer.
/// A stale/dropped update is refused, allowing the transport to reconnect for a
/// fresh full frame instead of displaying a silently corrupted screen.
pub fn apply(old: &Frame, revision: u64, patch: &Patch) -> Result<Frame> {
    let count = shape(old)?;
    if revision == 0
        || patch.base != revision
        || revision.checked_add(1) != Some(patch.revision)
        || !patch.frame.cells.is_empty()
        || shape(&patch.frame)? != count
        || !same_geometry(old, &patch.frame)
        || old.cells.len() != count
        || patch.changes.len() > count
        || old.cells.iter().any(|cell| !cell_valid(cell))
    {
        return Err("invalid or stale native frame patch".into());
    }
    // Counting serialization has a fixed limit and allocates no message buffer.
    // It also bounds new strings and nested metadata vectors before their clone.
    if encoded_size(patch, MAX_MESSAGE).is_none() {
        return Err("native patch exceeds message limit".into());
    }
    let mut seen = BTreeSet::new();
    for (index, cell) in &patch.changes {
        if *index >= count || !seen.insert(*index) || !cell_valid(cell) {
            return Err("invalid native patch cell".into());
        }
    }
    let mut frame = metadata(&patch.frame);
    frame.cells = old.cells.clone();
    for (index, cell) in &patch.changes {
        frame.cells[*index] = cell.clone();
    }
    Ok(frame)
}
