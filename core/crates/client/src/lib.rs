//! Portable terminal types and transactional browser/native client state.
pub mod model;
pub mod types;
use kelpclient_protocol::screens::{self, ScreenMessage};
use serde::Deserialize;
use serde_json::Value;
#[cfg(target_arch = "wasm32")]
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use types::*;
pub use types::{PanePatch, PaneSnapshot};

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
pub use kelpclient_protocol::cell_valid;
/// A client-chosen correlation identifier, as the server accepts them:
/// 1–128 ASCII letters, digits, `-` or `_`.
fn valid_request(value: &Value) -> bool {
    value.as_str().is_some_and(|request| {
        !request.is_empty()
            && request.len() <= 128
            && request
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}
/// An upload directory or file path the client may paste into a shell: an
/// absolute path of at most 4 KiB without control characters (a newline
/// would run a command once pasted).
pub fn valid_upload_path(path: &str) -> bool {
    path.starts_with('/') && path.len() <= 4096 && !path.chars().any(char::is_control)
}
/// Most entries one `file_list` reply may carry. The server sends at most
/// its `file_limits.max_list_entries` (5,000 by default); this bound only
/// keeps a malformed reply from growing without limit.
const MAX_FILE_ENTRIES: usize = 200_000;
/// Longest base64 `file_download_chunk` data. The server sends chunks of at
/// most `file_limits.downloads.chunk_bytes` (256 KiB, about 342 KiB of
/// base64); this bound only keeps a malformed chunk from growing without
/// limit.
const MAX_DOWNLOAD_CHUNK_TEXT: usize = 4 * 1024 * 1024;
/// Most paths one `file_removed` reply may report.
const MAX_FILE_PATHS: usize = 65_536;
/// A path in a `files` reply: absolute, at most 4 KiB, without NUL.
fn valid_file_path(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|path| path.starts_with('/') && path.len() <= 4096 && !path.contains('\0'))
}
/// A `files` entry: a name, a known kind and its numbers.
fn valid_file_entry(entry: &Value) -> bool {
    entry["name"]
        .as_str()
        .is_some_and(|name| !name.is_empty() && name.len() <= 4096 && !name.contains('\0'))
        && matches!(
            entry["kind"].as_str(),
            Some("file" | "dir" | "symlink" | "other")
        )
        && entry["size"].as_u64().is_some()
        && entry["modified"].as_i64().is_some()
        && entry["mode"].as_u64().is_some()
        && (entry["target"].is_null() || entry["target"].as_str().is_some())
}
/// A SHA-256 in hexadecimal, as `file_content` and `file_written` carry it.
fn valid_hash(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
}
/// A `file_error` body: a code and a bounded message.
fn valid_file_error(error: &Value) -> bool {
    error["code"]
        .as_str()
        .is_some_and(|code| !code.is_empty() && code.len() <= 64)
        && error["message"].as_str().is_some_and(|m| m.len() <= 65_536)
        && (error["path"].is_null() || error["path"].as_str().is_some_and(|p| p.len() <= 4096))
        && (error["current_hash"].is_null() || valid_hash(&error["current_hash"]))
        && (error["size"].is_null() || error["size"].as_u64().is_some())
}
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
    /// An update with `binary_screens`, waiting for the binary record that
    /// must follow it (KelpServer `binary-screens`).
    pending: Option<Value>,
    /// Welcome `limits.subscribed_cells`, bounding a binary record's cells.
    subscribed_cells: Option<usize>,
}
/// Whether `value` is an update whose screens follow as a binary record.
fn binary_update(value: &Value) -> bool {
    value["Stream"]["type"] == "update" && value["Stream"]["binary_screens"] == true
}
impl Client {
    /// Bytes are framed exactly as they are on the existing SSH stdio bridge.
    ///
    /// On a connection that enabled `binary-screens`, an update carrying
    /// `binary_screens: true` is held until the binary record after it, whose
    /// screens are then applied exactly as a JSON update's. The update is
    /// returned with those screens as JSON `snapshots` and `patches` (every
    /// cell of each changed row) and without `binary_screens`, so consumers
    /// of the returned events see one format.
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
            let payload = &self.incoming[used + 4..used + 4 + length];
            if screens::is_binary_record(payload) {
                if self.pending.is_none() {
                    return Err("binary screen record without its update".into());
                }
                let messages =
                    screens::decode(payload, self.max_cells()).map_err(|e| e.to_string())?;
                let update = self.pending.take().expect("checked above");
                events.push(self.accept_screens(update, messages)?);
            } else {
                if self.pending.is_some() {
                    return Err("expected a binary screen record after the update".into());
                }
                let value: Value = serde_json::from_slice(payload).map_err(|e| e.to_string())?;
                if binary_update(&value) {
                    let empty = |key| value["Stream"][key].as_array().is_some_and(Vec::is_empty);
                    if !empty("snapshots") || !empty("patches") {
                        return Err("binary screen update carries JSON screens".into());
                    }
                    self.pending = Some(value);
                } else {
                    self.accept(&value)?;
                    events.push(value);
                }
            }
            used += length + 4;
            if events.len() > 256 {
                return Err("event batch limit".into());
            }
        }
        self.incoming.drain(..used);
        Ok(events)
    }
    /// The cell budget of one binary record: the Welcome's advertised
    /// `subscribed_cells`, never above what this client keeps.
    fn max_cells(&self) -> usize {
        self.subscribed_cells
            .unwrap_or(screens::DEFAULT_MAX_CELLS)
            .min(MAX_CELLS)
    }
    /// Applies a held `binary_screens` update with its decoded record and
    /// returns the update as JSON screens would have carried it.
    fn accept_screens(&mut self, mut update: Value, messages: Vec<ScreenMessage>) -> Result<Value> {
        let mut snapshots = Vec::new();
        let mut patches = Vec::new();
        for message in messages {
            match message {
                ScreenMessage::Snapshot(snapshot) => snapshots.push(snapshot),
                ScreenMessage::Patch(patch) => patches.push(patch.into_pane_patch()),
            }
        }
        let event = &mut update["Stream"];
        event["snapshots"] = serde_json::to_value(&snapshots).map_err(|e| e.to_string())?;
        event["patches"] = serde_json::to_value(&patches).map_err(|e| e.to_string())?;
        if let Some(event) = event.as_object_mut() {
            event.remove("binary_screens");
        }
        self.accept_update(&update["Stream"], snapshots, patches)?;
        Ok(update)
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
                self.subscribed_cells = event["limits"]["subscribed_cells"]
                    .as_u64()
                    .filter(|cells| *cells > 0)
                    .map(|cells| usize::try_from(cells).unwrap_or(usize::MAX));
            }
            Some("update") => {
                if event["binary_screens"] == true {
                    return Err("binary screen update without its record".into());
                }
                let snapshots = Vec::<PaneSnapshot>::deserialize(&event["snapshots"])
                    .map_err(|e| e.to_string())?;
                let patches =
                    Vec::<PanePatch>::deserialize(&event["patches"]).map_err(|e| e.to_string())?;
                self.accept_update(event, snapshots, patches)?;
            }
            Some("clipboard") => {
                if event["pane"].as_u64().is_none_or(|pane| pane == 0)
                    || event["text"].as_str().is_none_or(|text| text.len() > 65536)
                {
                    return Err("invalid clipboard event".into());
                }
            }
            Some("upload_started") => {
                if !valid_request(&event["request"])
                    || event["batch"].as_u64().is_none()
                    || event["dir"]
                        .as_str()
                        .is_none_or(|dir| !valid_upload_path(dir))
                {
                    return Err("invalid upload start".into());
                }
            }
            Some("upload_progress") => {
                if event["batch"].as_u64().is_none()
                    || event["file_index"].as_u64().is_none_or(|i| i >= 4096)
                    || event["received"].as_u64().is_none()
                {
                    return Err("invalid upload progress".into());
                }
            }
            Some("upload_done") => {
                let paths = event["paths"].as_array();
                if !valid_request(&event["request"])
                    || event["batch"].as_u64().is_none()
                    || paths.is_none_or(|paths| {
                        paths.len() > 4096
                            || paths
                                .iter()
                                .any(|path| path.as_str().is_none_or(|p| !valid_upload_path(p)))
                    })
                {
                    return Err("invalid upload result".into());
                }
            }
            Some("upload_failed") => {
                if event["batch"].as_u64().is_none()
                    || event["message"].as_str().is_none_or(|m| m.len() > 4096)
                {
                    return Err("invalid upload failure".into());
                }
            }
            Some("done") => {
                if !valid_request(&event["request"]) {
                    return Err("invalid request acknowledgement".into());
                }
            }
            // `files` replies (KelpServer `files`): checked for their shape
            // only. Paths are the server's normalised absolute paths and may
            // hold any character a file name can, so clients quote them
            // before pasting and refuse control characters there.
            Some("file_list") => {
                let entries = event["entries"].as_array();
                if !valid_request(&event["request"])
                    || !valid_file_path(&event["path"])
                    || event["truncated"].as_bool().is_none()
                    || event["skipped"].as_u64().is_none()
                    || entries.is_none_or(|entries| {
                        entries.len() > MAX_FILE_ENTRIES || !entries.iter().all(valid_file_entry)
                    })
                {
                    return Err("invalid file listing".into());
                }
            }
            Some("file_stat") => {
                if !valid_request(&event["request"])
                    || !valid_file_path(&event["path"])
                    || !valid_file_entry(&event["entry"])
                {
                    return Err("invalid file status".into());
                }
            }
            Some("file_content") => {
                if !valid_request(&event["request"])
                    || !valid_file_path(&event["path"])
                    || event["size"].as_u64().is_none()
                    || !valid_hash(&event["hash"])
                    || event["binary"].as_bool().is_none()
                    || !matches!(event["encoding"].as_str(), Some("utf8" | "base64"))
                    || event["content"].as_str().is_none()
                {
                    return Err("invalid file content".into());
                }
            }
            Some("file_written") => {
                if !valid_request(&event["request"])
                    || !valid_file_path(&event["path"])
                    || !valid_hash(&event["hash"])
                    || event["size"].as_u64().is_none()
                    || event["created"].as_bool().is_none()
                {
                    return Err("invalid file write result".into());
                }
            }
            Some("file_removed") => {
                let removed = event["removed"].as_array();
                let failed = event["failed"].as_array();
                if !valid_request(&event["request"])
                    || removed.is_none_or(|removed| {
                        removed.len() > MAX_FILE_PATHS
                            || removed.iter().any(|entry| {
                                !valid_file_path(&entry["path"])
                                    || !(entry["trashed_as"].is_null()
                                        || valid_file_path(&entry["trashed_as"]))
                            })
                    })
                    || failed.is_none_or(|failed| {
                        failed.len() > MAX_FILE_PATHS
                            || failed.iter().any(|entry| {
                                !valid_file_path(&entry["path"])
                                    || !valid_file_error(&entry["error"])
                            })
                    })
                {
                    return Err("invalid file removal result".into());
                }
            }
            Some("file_error") => {
                if !valid_request(&event["request"])
                    || !valid_file_error(&event["error"])
                    || !(event["download"].is_null() || event["download"].as_u64().is_some())
                {
                    return Err("invalid file error".into());
                }
            }
            // `file-downloads`: a download's start, chunks and end.
            Some("file_download_started") => {
                let optional_number =
                    |key: &str| event[key].is_null() || event[key].as_u64().is_some();
                if !valid_request(&event["request"])
                    || event["download"].as_u64().is_none()
                    || event["name"].as_str().is_none_or(|name| {
                        name.is_empty() || name.len() > 4096 || name.contains('\0')
                    })
                    || !optional_number("size")
                    || !optional_number("entries")
                    || !optional_number("input_bytes")
                    || !(event["archive"].is_null() || event["archive"] == "tar.gz")
                {
                    return Err("invalid download start".into());
                }
            }
            Some("file_download_chunk") => {
                if event["download"].as_u64().is_none()
                    || event["offset"].as_u64().is_none()
                    || event["data"]
                        .as_str()
                        .is_none_or(|data| data.is_empty() || data.len() > MAX_DOWNLOAD_CHUNK_TEXT)
                    || !(event["input"].is_null() || event["input"].as_u64().is_some())
                {
                    return Err("invalid download chunk".into());
                }
            }
            Some("file_download_done") => {
                if !valid_request(&event["request"])
                    || event["download"].as_u64().is_none()
                    || event["size"].as_u64().is_none()
                    || !valid_hash(&event["hash"])
                    || event["skipped"].as_u64().is_none()
                    || event["changed"].as_u64().is_none()
                {
                    return Err("invalid download result".into());
                }
            }
            Some("lease") | Some("error") => {}
            _ => return Err("unsupported stream event".into()),
        }
        Ok(())
    }
    /// Checks an update's metadata and applies its screens transactionally,
    /// whichever format carried them.
    fn accept_update(
        &mut self,
        event: &Value,
        snapshots: Vec<PaneSnapshot>,
        patches: Vec<PanePatch>,
    ) -> Result<()> {
        if self.epoch.is_empty() || event["epoch"] != self.epoch {
            return Err("stream epoch changed".into());
        }
        let wanted = Vec::<u64>::deserialize(&event["subscribed"]).map_err(|e| e.to_string())?;
        let wanted_set: BTreeSet<_> = wanted.iter().copied().collect();
        let panes = Vec::<PaneInfo>::deserialize(&event["panes"]).map_err(|e| e.to_string())?;
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
