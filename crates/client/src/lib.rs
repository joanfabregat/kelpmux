//! Portable terminal types and transactional browser/native client state.
pub mod model;
pub mod types;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(target_arch = "wasm32")]
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use types::*;

pub type Result<T> = std::result::Result<T, String>;
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;
pub const MAX_CELLS: usize = 100_000;
/// Layout plain history using the same Unicode width library as the terminal.
/// Keep this in Rust so browser search/selection do not count wide or combining
/// characters as independent one-column cells.
pub fn history_screen(lines: &[String], cols: u16, rows: u16) -> Result<Screen> {
    if !(1..=400).contains(&cols) || !(1..=160).contains(&rows) {
        return Err("invalid history dimensions".into());
    }
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
    Ok(Screen {
        cols,
        rows,
        cells,
        cursor: None,
        modes: InputModes::default(),
    })
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSnapshot {
    pub pane: PaneId,
    pub revision: u64,
    pub screen: Screen,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanePatch {
    pub pane: PaneId,
    pub base: u64,
    pub revision: u64,
    pub cursor: Option<Cursor>,
    pub modes: InputModes,
    pub changes: Vec<(usize, Cell)>,
}
pub use kelpclient_protocol::cell_valid;
pub fn validate(snapshot: &PaneSnapshot) -> Result<()> {
    let screen = &snapshot.screen;
    if snapshot.pane == 0
        || snapshot.revision == 0
        || !(1..=400).contains(&screen.cols)
        || !(1..=160).contains(&screen.rows)
        || screen.cells.len() != usize::from(screen.cols) * usize::from(screen.rows)
        || screen
            .cursor
            .is_some_and(|c| c.position.x >= screen.cols || c.position.y >= screen.rows)
        || screen.cells.iter().enumerate().any(|(i, c)| {
            !cell_valid(c)
                || c.width == 2 && i % usize::from(screen.cols) + 1 == usize::from(screen.cols)
        })
    {
        return Err("invalid pane snapshot".into());
    }
    Ok(())
}
pub fn apply(snapshot: &PaneSnapshot, patch: &PanePatch) -> Result<PaneSnapshot> {
    validate(snapshot)?;
    if patch.pane != snapshot.pane
        || patch.base != snapshot.revision
        || patch.base.checked_add(1) != Some(patch.revision)
        || patch.changes.len() > snapshot.screen.cells.len()
    {
        return Err("invalid pane patch baseline".into());
    }
    let mut seen = BTreeSet::new();
    for (i, c) in &patch.changes {
        if *i >= snapshot.screen.cells.len() || !seen.insert(*i) || !cell_valid(c) {
            return Err("invalid pane patch cell".into());
        }
    }
    let mut next = snapshot.clone();
    for (i, c) in &patch.changes {
        next.screen.cells[*i] = c.clone();
    }
    next.revision = patch.revision;
    next.screen.cursor = patch.cursor;
    next.screen.modes = patch.modes;
    validate(&next)?;
    Ok(next)
}
#[derive(Default)]
pub struct Client {
    pub epoch: String,
    pub client: u64,
    pub screens: BTreeMap<PaneId, PaneSnapshot>,
    incoming: Vec<u8>,
}
impl Client {
    /// Bytes are framed exactly as they are on the existing SSH stdio bridge.
    pub fn receive(&mut self, bytes: &[u8]) -> Result<Vec<Value>> {
        if self.incoming.len() + bytes.len() > 2 * MAX_MESSAGE {
            return Err("receive buffer limit".into());
        }
        self.incoming.extend_from_slice(bytes);
        let mut events = Vec::new();
        let mut used = 0;
        while self.incoming.len() - used >= 4 {
            let length =
                u32::from_be_bytes(self.incoming[used..used + 4].try_into().unwrap()) as usize;
            if length == 0 || length > MAX_MESSAGE {
                return Err("invalid frame length".into());
            }
            if self.incoming.len() - used < length + 4 {
                break;
            }
            let value: Value = serde_json::from_slice(&self.incoming[used + 4..used + 4 + length])
                .map_err(|e| e.to_string())?;
            self.accept(&value)?;
            events.push(value);
            used += length + 4;
            if events.len() > 256 {
                return Err("event batch limit".into());
            }
        }
        self.incoming.drain(..used);
        Ok(events)
    }
    pub fn accept(&mut self, value: &Value) -> Result<()> {
        let Some(event) = value.get("Stream") else {
            return Ok(());
        };
        match event["type"].as_str() {
            Some("welcome") => {
                let epoch = event["epoch"].as_str().ok_or("missing epoch")?;
                if !self.epoch.is_empty()
                    || epoch.is_empty()
                    || epoch.len() > 128
                    || epoch.chars().any(char::is_control)
                    || event["version"] != kelpclient_protocol::STREAM_PROTOCOL_VERSION
                    || event["client"].as_u64().unwrap_or(0) == 0
                {
                    return Err("invalid stream welcome".into());
                }
                self.epoch = epoch.into();
                self.client = event["client"].as_u64().unwrap();
            }
            Some("update") => {
                if self.epoch.is_empty() || event["epoch"] != self.epoch {
                    return Err("stream epoch changed".into());
                }
                let wanted: Vec<u64> = serde_json::from_value(event["subscribed"].clone())
                    .map_err(|e| e.to_string())?;
                let wanted_set: BTreeSet<_> = wanted.iter().copied().collect();
                let panes: Vec<PaneInfo> =
                    serde_json::from_value(event["panes"].clone()).map_err(|e| e.to_string())?;
                let pane_ids: BTreeSet<_> = panes.iter().map(|p| p.id).collect();
                if panes.len() > 4096
                    || pane_ids.len() != panes.len()
                    || pane_ids.contains(&0)
                    || !wanted_set.is_subset(&pane_ids)
                    || wanted.len() > 10
                    || wanted_set.len() != wanted.len()
                {
                    return Err("invalid subscriptions".into());
                }
                let leases = event["leases"].as_array().ok_or("missing leases")?;
                let mut leased = BTreeSet::new();
                if leases.len() > 4096
                    || leases.iter().any(|lease| {
                        let id = lease["pane"].as_u64().unwrap_or(0);
                        !pane_ids.contains(&id)
                            || !leased.insert(id)
                            || lease["owner"].as_u64().unwrap_or(0) == 0
                    })
                {
                    return Err("invalid lease metadata".into());
                }
                let notifications = event["notifications"]
                    .as_array()
                    .ok_or("missing notifications")?;
                if notifications.len() > 256
                    || notifications.iter().any(|event| {
                        event["title"]
                            .as_str()
                            .is_none_or(|title| title.len() > 4096)
                    })
                {
                    return Err("invalid notifications".into());
                }
                let mut next = self.screens.clone();
                next.retain(|id, _| wanted_set.contains(id));
                let mut changed = BTreeSet::new();
                let snapshots: Vec<PaneSnapshot> =
                    serde_json::from_value(event["snapshots"].clone())
                        .map_err(|e| e.to_string())?;
                let patches: Vec<PanePatch> =
                    serde_json::from_value(event["patches"].clone()).map_err(|e| e.to_string())?;
                for s in snapshots {
                    validate(&s)?;
                    if !wanted_set.contains(&s.pane) || !changed.insert(s.pane) {
                        return Err("invalid snapshot subscription".into());
                    }
                    next.insert(s.pane, s);
                }
                for p in patches {
                    if !wanted_set.contains(&p.pane) || !changed.insert(p.pane) {
                        return Err("invalid patch subscription".into());
                    }
                    let old = next.get(&p.pane).ok_or("patch without snapshot")?;
                    next.insert(p.pane, apply(old, &p)?);
                }
                if next.values().map(|s| s.screen.cells.len()).sum::<usize>() > MAX_CELLS
                    || wanted.iter().any(|p| !next.contains_key(p))
                {
                    return Err("missing baseline or cell budget".into());
                }
                self.screens = next;
            }
            Some("lease") | Some("error") => {}
            _ => return Err("unsupported stream event".into()),
        }
        Ok(())
    }
    pub fn selection(&self, pane: u64, start: Point, end: Point) -> Result<String> {
        let s = &self
            .screens
            .get(&pane)
            .ok_or("unknown selection pane")?
            .screen;
        let (mut start, end) = if (start.y, start.x) <= (end.y, end.x) {
            (start, end)
        } else {
            (end, start)
        };
        if start.x >= s.cols || start.y >= s.rows || end.x >= s.cols || end.y >= s.rows {
            return Err("selection outside screen".into());
        }
        if start.x > 0
            && s.cells[usize::from(start.y) * usize::from(s.cols) + usize::from(start.x)].width == 0
        {
            start.x -= 1;
        }
        let mut lines = Vec::new();
        for y in start.y..=end.y {
            let x0 = if y == start.y { start.x } else { 0 };
            let x1 = if y == end.y { end.x } else { s.cols - 1 };
            let mut line = String::new();
            for x in x0..=x1 {
                let c = &s.cells[usize::from(y) * usize::from(s.cols) + usize::from(x)];
                if c.width > 0 {
                    line.push_str(&c.text);
                }
            }
            lines.push(line.trim_end().to_owned());
        }
        Ok(lines.join("\n"))
    }
}

// Small stable ABI: no DOM or Unix dependencies enter this crate. JS owns the
// caller-provided input allocation; output is valid until the next dispatch.
#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use std::cell::RefCell;
    thread_local! {static CLIENT:RefCell<Client>=RefCell::new(Client::default());static OUTPUT:RefCell<Vec<u8>>=const{RefCell::new(Vec::new())};}
    #[unsafe(no_mangle)]
    pub extern "C" fn alloc(length: usize) -> *mut u8 {
        let v = vec![0u8; length].into_boxed_slice();
        Box::into_raw(v) as *mut u8
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn free(pointer: *mut u8, length: usize) {
        unsafe {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                pointer, length,
            )));
        }
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn receive(pointer: *const u8, length: usize) {
        let bytes = unsafe { std::slice::from_raw_parts(pointer, length) };
        let value = CLIENT.with(|c| match c.borrow_mut().receive(bytes) {
            Ok(events) => json!({"events":events}),
            Err(e) => json!({"error":e}),
        });
        OUTPUT.with(|o| *o.borrow_mut() = serde_json::to_vec(&value).unwrap());
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn reset() {
        CLIENT.with(|c| *c.borrow_mut() = Client::default());
    }
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn layout_history(
        pointer: *const u8,
        length: usize,
        cols: u16,
        rows: u16,
    ) {
        let value = if length > 4 * 1024 * 1024 {
            json!({"error":"history layout too large"})
        } else {
            let bytes = unsafe { std::slice::from_raw_parts(pointer, length) };
            match serde_json::from_slice::<Vec<String>>(bytes)
                .map_err(|e| e.to_string())
                .and_then(|lines| history_screen(&lines, cols, rows))
            {
                Ok(screen) => json!({"screen":screen}),
                Err(error) => json!({"error":error}),
            }
        };
        OUTPUT.with(|o| *o.borrow_mut() = serde_json::to_vec(&value).unwrap());
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn select(pane: u64, x1: u16, y1: u16, x2: u16, y2: u16) {
        let value = CLIENT.with(|c| {
            match c
                .borrow()
                .selection(pane, Point { x: x1, y: y1 }, Point { x: x2, y: y2 })
            {
                Ok(text) => json!({"text":text}),
                Err(e) => json!({"error":e}),
            }
        });
        OUTPUT.with(|o| *o.borrow_mut() = serde_json::to_vec(&value).unwrap());
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn output_ptr() -> *const u8 {
        OUTPUT.with(|o| o.borrow().as_ptr())
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn output_len() -> usize {
        OUTPUT.with(|o| o.borrow().len())
    }
}

#[cfg(test)]
mod tests;
