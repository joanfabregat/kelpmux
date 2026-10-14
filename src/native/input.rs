use super::protocol::*;

/// Incremental terminal framing. Paste payloads are never interpreted as keys.
#[derive(Default)]
pub struct Decoder {
    pending: Vec<u8>,
    paste: Option<Vec<u8>>,
}
impl Decoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<InputEvent> {
        let mut events = Vec::new();
        // Processing byte by byte bounds retained storage even for an oversized packet.
        for byte in bytes {
            self.pending.push(*byte);
            self.drain(&mut events);
        }
        events
    }
    fn drain(&mut self, events: &mut Vec<InputEvent>) {
        const START: &[u8] = b"\x1b[200~";
        const END: &[u8] = b"\x1b[201~";
        loop {
            if let Some(paste) = self.paste.as_mut() {
                if self.pending == END {
                    events.push(InputEvent::Paste(self.paste.take().unwrap()));
                    self.pending.clear();
                    continue;
                }
                if END.starts_with(&self.pending) {
                    break;
                }
                if !self.pending.is_empty() {
                    if paste.len() < MAX_INPUT {
                        paste.push(self.pending[0]);
                    }
                    self.pending.remove(0);
                    continue;
                }
                break;
            }
            if self.pending.is_empty() {
                break;
            }
            if self.pending[0] != 27 {
                let byte = self.pending.remove(0);
                if let Some(InputEvent::Bytes(previous)) = events.last_mut() {
                    if previous.len() < MAX_INPUT {
                        previous.push(byte);
                        continue;
                    }
                }
                events.push(InputEvent::Bytes(vec![byte]));
                continue;
            }
            if START.starts_with(&self.pending) && self.pending.len() < START.len() {
                break;
            }
            if self.pending == START {
                self.pending.clear();
                self.paste = Some(Vec::new());
                continue;
            }
            if self.pending.len() == 1 {
                break;
            }
            let length = if self.pending[1] == b'[' || self.pending[1] == b'O' {
                self.pending
                    .iter()
                    .enumerate()
                    .skip(2)
                    .find(|(_, b)| (0x40..=0x7e).contains(*b))
                    .map(|(i, _)| i + 1)
            } else {
                // Alt plus a UTF-8 character is one event, preserving word-edit keys.
                let size = match self.pending[1] {
                    0xc2..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf4 => 4,
                    _ => 1,
                };
                (self.pending.len() > size).then_some(size + 1)
            };
            if let Some(length) = length {
                let sequence: Vec<_> = self.pending.drain(..length).collect();
                if let Some(mouse) = decode_mouse(&sequence) {
                    events.push(InputEvent::Mouse(mouse));
                } else {
                    events.push(InputEvent::Bytes(sequence));
                }
            } else if self.pending.len() >= MAX_INPUT {
                events.push(InputEvent::Bytes(std::mem::take(&mut self.pending)));
            } else {
                break;
            }
        }
    }
    /// Called after the frontend's escape timeout; an unfinished paste remains buffered.
    pub fn flush(&mut self) -> Vec<InputEvent> {
        if self.paste.is_some() || self.pending.is_empty() {
            return vec![];
        }
        vec![InputEvent::Bytes(std::mem::take(&mut self.pending))]
    }
}
fn decode_mouse(bytes: &[u8]) -> Option<MouseEvent> {
    if !bytes.starts_with(b"\x1b[<") || !matches!(bytes.last(), Some(b'M' | b'm')) {
        return None;
    }
    let mut parts = std::str::from_utf8(&bytes[3..bytes.len() - 1])
        .ok()?
        .split(';');
    let flags: u16 = parts.next()?.parse().ok()?;
    let x: u16 = parts.next()?.parse().ok()?;
    let y: u16 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || x == 0 || y == 0 || flags > 255 {
        return None;
    }
    let kind = if flags & 64 != 0 {
        if flags & 1 == 0 {
            MouseKind::WheelUp
        } else {
            MouseKind::WheelDown
        }
    } else if bytes.last() == Some(&b'm') {
        MouseKind::Release
    } else if flags & 32 != 0 {
        MouseKind::Move
    } else if flags & 3 == 3 {
        MouseKind::Release
    } else {
        MouseKind::Press
    };
    Some(MouseEvent {
        position: Point { x: x - 1, y: y - 1 },
        kind,
        button: (flags & 3) as u8,
        shift: flags & 4 != 0,
        alt: flags & 8 != 0,
        ctrl: flags & 16 != 0,
    })
}

#[derive(Default)]
pub struct Router {
    prefix: bool,
    resizing: bool,
    pending_modal: bool,
    scrolling: bool,
    selection: Option<(PaneId, Point, Point)>,
    observed_help: bool,
    help_override: Option<bool>,
}
impl Router {
    pub fn route(&mut self, event: InputEvent, frame: &Frame) -> Vec<ClientMessage> {
        if frame.ui.dialog.is_some() || frame.help {
            self.pending_modal = false;
        }
        if self.observed_help != frame.help {
            self.observed_help = frame.help;
            self.help_override = None;
        }
        let help = self.help_override.unwrap_or(frame.help);
        match event {
            InputEvent::Paste(bytes)
                if frame.controller
                    && !help
                    && frame.ui.dialog.is_none()
                    && !self.pending_modal =>
            {
                vec![ClientMessage::Paste(bytes)]
            }
            InputEvent::Paste(_) => vec![],
            InputEvent::Mouse(mouse) => self.mouse(mouse, frame),
            InputEvent::Bytes(bytes) => self.keys(&bytes, frame),
        }
    }
    fn keys(&mut self, bytes: &[u8], frame: &Frame) -> Vec<ClientMessage> {
        let mut output = Vec::new();
        let mut position = 0;
        while position < bytes.len() {
            let byte = bytes[position];
            // Terminal sequences remain indivisible: their digits and Ctrl-B cannot be bindings.
            let length = if byte == 27 && position + 1 < bytes.len() {
                if matches!(bytes[position + 1], b'[' | b'O') {
                    bytes[position + 2..]
                        .iter()
                        .position(|b| (0x40..=0x7e).contains(b))
                        .map_or(bytes.len() - position, |i| i + 3)
                } else {
                    (bytes.len() - position).min(2)
                }
            } else {
                1
            };
            let key = &bytes[position..position + length];
            position += length;
            if self.prefix {
                self.prefix = false;
                let configured = std::str::from_utf8(key)
                    .ok()
                    .and_then(|key| frame.ui.config.bindings.get(key));
                let action = if let Some(binding) = configured {
                    self.binding(binding, frame)
                } else {
                    match key {
                        b"d" => Some(Action::Detach),
                        b"t" | b"T" => Some(Action::Takeover),
                        b"z" => Some(Action::Read(frame.active)),
                        b"f" => Some(Action::Fullscreen(frame.active)),
                        b"?" => Some(Action::Help),
                        b"g" => Some(Action::GridPicker),
                        b"a" => Some(Action::Agents(super::ui::AgentFilter::All)),
                        b"!" => Some(Action::Agents(super::ui::AgentFilter::Attention)),
                        b"n" => Some(Action::Agents(super::ui::AgentFilter::Finished)),
                        b"h" => Some(Action::FocusDirection { dx: -1, dy: 0 }),
                        b"l" => Some(Action::FocusDirection { dx: 1, dy: 0 }),
                        b"j" => Some(Action::FocusDirection { dx: 0, dy: 1 }),
                        b"k" => Some(Action::FocusDirection { dx: 0, dy: -1 }),
                        b"+" => self.resize_action(frame, 10, 0),
                        b"-" => self.resize_action(frame, -10, 0),
                        b">" => self.resize_action(frame, 0, 2),
                        b"<" => self.resize_action(frame, 0, -2),
                        b"[" => {
                            self.scrolling = true;
                            None
                        }
                        b"y" => Some(Action::Copy(frame.active)),
                        b"s" => Some(Action::MovePicker),
                        [digit @ b'1'..=b'9'] => Some(Action::Select((digit - b'0') as usize)),
                        [byte] if *byte == frame.ui.config.prefix && frame.controller => {
                            output.push(ClientMessage::Input(vec![*byte]));
                            None
                        }
                        _ => None,
                    }
                };
                if let Some(action) = action {
                    if frame.controller
                        && matches!(
                            action,
                            Action::GridPicker | Action::MovePicker | Action::Agents(_)
                        )
                    {
                        self.pending_modal = true;
                    }
                    if action == Action::Help && frame.controller {
                        self.toggle_help(frame);
                    }
                    emit(&mut output, action, frame.controller);
                }
                continue;
            }
            if key == [frame.ui.config.prefix] {
                self.prefix = true;
                continue;
            }
            if !frame.controller {
                continue;
            }
            if self.pending_modal {
                continue;
            }
            if let Some(dialog) = frame.ui.dialog {
                let action = match key {
                    b"\x1b" | b"q" => Some(Action::DialogClose),
                    b"\x1b[A" => Some(Action::DialogNavigate { dx: 0, dy: -1 }),
                    b"\x1b[B" => Some(Action::DialogNavigate { dx: 0, dy: 1 }),
                    b"\x1b[C" => Some(Action::DialogNavigate { dx: 1, dy: 0 }),
                    b"\x1b[D" => Some(Action::DialogNavigate { dx: -1, dy: 0 }),
                    _ => None,
                };
                if let Some(action) = action {
                    emit(&mut output, action, true);
                } else if matches!(key, b"\r" | b"\n") {
                    output.extend(self.pick_dialog(frame, dialog.selection));
                } else if let [digit @ b'1'..=b'9'] = key {
                    let index = if dialog.kind == super::ui::DialogKind::Agents {
                        let panes: Vec<_> = frame
                            .panes
                            .iter()
                            .filter(|v| !v.overlay)
                            .map(|v| v.pane.clone())
                            .collect();
                        super::workspace::dialog_entries(&panes, Some(dialog))
                            .iter()
                            .position(|id| {
                                panes
                                    .iter()
                                    .any(|p| p.id == *id && p.number == usize::from(digit - b'0'))
                            })
                    } else {
                        Some(usize::from(digit - b'1'))
                    };
                    if let Some(index) = index {
                        output.extend(self.pick_dialog(frame, index));
                    }
                }
                continue;
            }
            if self.help_override.unwrap_or(frame.help) {
                if matches!(key, b"q" | b"\x1b" | b"?") {
                    self.toggle_help(frame);
                    emit(&mut output, Action::Help, true);
                }
                continue;
            }
            if self.scrolling {
                let lines = match key {
                    b"\x1b[A" => 1,
                    b"\x1b[B" => -1,
                    b"\x1b[5~" => 10,
                    b"\x1b[6~" => -10,
                    _ => 0,
                };
                if lines != 0 {
                    emit(
                        &mut output,
                        Action::Scroll {
                            pane: frame.active,
                            lines,
                        },
                        true,
                    );
                } else if key == b"y" {
                    emit(&mut output, Action::Copy(frame.active), true);
                    self.scrolling = false;
                } else if matches!(key, b"q" | b"\x1b") {
                    self.scrolling = false;
                }
                continue;
            }
            let action = match key {
                b"\x1b\t" | b"\x1b0" => Some(Action::Read(frame.active)),
                [27, digit @ b'1'..=b'9'] => Some(Action::Select((digit - b'0') as usize)),
                _ => None,
            };
            if let Some(action) = action {
                emit(&mut output, action, true);
            } else {
                let modes = frame
                    .panes
                    .iter()
                    .rev()
                    .find(|view| view.pane.id == frame.active)
                    .map(|view| view.modes)
                    .unwrap_or_default();
                if matches!(key, b"\x1b[I" | b"\x1b[O") && !modes.focus {
                    continue;
                }
                if let [
                    27,
                    b'[',
                    final_byte @ (b'A' | b'B' | b'C' | b'D' | b'H' | b'F'),
                ] = key
                {
                    if modes.application_cursor {
                        push_input(&mut output, &[27, b'O', *final_byte]);
                    } else {
                        push_input(&mut output, key);
                    }
                } else {
                    push_input(&mut output, key);
                }
            }
        }
        output
    }
    fn mouse(&mut self, event: MouseEvent, frame: &Frame) -> Vec<ClientMessage> {
        let mut output = Vec::new();
        if !frame.controller {
            return output;
        }
        if self.pending_modal {
            return output;
        }
        if self.resizing && matches!(event.kind, MouseKind::Move | MouseKind::Release) {
            if let Some(view) = frame.panes.iter().rev().find(|v| v.overlay) {
                let cols = (event.position.x.saturating_sub(view.rect.x) + 1)
                    .saturating_mul(2)
                    .saturating_sub(view.rect.cols);
                let rows = (event.position.y.saturating_sub(view.rect.y) + 1)
                    .saturating_mul(2)
                    .saturating_sub(view.rect.rows);
                emit(
                    &mut output,
                    Action::ReadResize {
                        cols: cols.clamp(20, frame.cols),
                        rows: rows.clamp(3, frame.rows - 1),
                    },
                    true,
                );
            }
            if event.kind == MouseKind::Release {
                self.resizing = false;
            }
            return output;
        }
        if let Some(dialog) = frame.ui.dialog {
            let bounds = super::workspace::dialog_bounds(frame.cols, frame.rows);
            if event.kind == MouseKind::Press
                && (!bounds.contains(event.position)
                    || (event.position.y == bounds.y
                        && event.position.x >= bounds.x + bounds.cols - 3))
            {
                emit(&mut output, Action::DialogClose, true);
                return output;
            }
            let index = if dialog.kind == super::ui::DialogKind::Grid {
                super::workspace::grid_cards(bounds)
                    .iter()
                    .position(|rect| rect.contains(event.position))
            } else if dialog.kind == super::ui::DialogKind::Move {
                let views: Vec<_> = frame.panes.iter().filter(|v| !v.overlay).collect();
                let columns = views.iter().filter(|v| v.rect.y == 0).count().max(1) as u8;
                let rows = views.len().div_ceil(usize::from(columns)) as u8;
                super::workspace::move_cards(bounds, columns, rows)
                    .iter()
                    .position(|rect| rect.contains(event.position))
            } else if event.position.y > bounds.y + 1 {
                let visible = usize::from(bounds.rows.saturating_sub(2));
                Some(
                    usize::from(event.position.y - bounds.y - 2)
                        + dialog.selection.saturating_sub(visible.saturating_sub(1)),
                )
            } else {
                None
            };
            if let Some(index) = index {
                let panes: Vec<_> = frame
                    .panes
                    .iter()
                    .filter(|v| !v.overlay)
                    .map(|v| v.pane.clone())
                    .collect();
                if index >= super::workspace::dialog_entries(&panes, Some(dialog)).len() {
                    return output;
                }
                if event.kind == MouseKind::Press {
                    output.extend(self.pick_dialog(frame, index));
                } else if event.kind == MouseKind::Move {
                    emit(&mut output, Action::DialogPick(index), true);
                }
            }
            return output;
        }
        if self.help_override.unwrap_or(frame.help) {
            let bounds = help_bounds(frame.cols, frame.rows);
            let close = event.position.y == bounds.y
                && event.position.x >= bounds.x + bounds.cols.saturating_sub(3);
            let toggle = event.position.y == frame.rows.saturating_sub(1)
                && event.position.x >= frame.cols.saturating_sub(3);
            if event.kind == MouseKind::Press
                && (!bounds.contains(event.position) || close || toggle)
            {
                self.toggle_help(frame);
                emit(&mut output, Action::Help, true);
            }
            return output;
        }
        if event.kind == MouseKind::Press
            && event.position.y == frame.rows.saturating_sub(1)
            && event.position.x >= frame.cols.saturating_sub(10)
            && event.position.x < frame.cols.saturating_sub(3)
        {
            self.pending_modal = true;
            emit(&mut output, Action::GridPicker, true);
            return output;
        }
        if event.kind == MouseKind::Press
            && event.position.y == frame.rows.saturating_sub(1)
            && event.position.x >= frame.cols.saturating_sub(3)
        {
            self.toggle_help(frame);
            emit(&mut output, Action::Help, true);
            return output;
        }
        if let Some((pane, start, previous)) = self.selection {
            if matches!(event.kind, MouseKind::Move | MouseKind::Release) {
                if let Some(view) = frame.panes.iter().rev().find(|v| v.pane.id == pane) {
                    let end = local_point(event.position, view.body);
                    emit(&mut output, Action::SelectText { pane, start, end }, true);
                    if event.kind == MouseKind::Release {
                        self.selection = None;
                        emit(&mut output, Action::Copy(pane), true);
                    } else {
                        self.selection = Some((pane, start, end));
                    }
                } else {
                    self.selection = None;
                }
                let _ = previous;
                return output;
            }
        }
        let Some(view) = frame
            .panes
            .iter()
            .rev()
            .find(|v| v.rect.contains(event.position))
        else {
            return output;
        };
        let id = view.pane.id;
        if event.kind == MouseKind::Press
            && view.overlay
            && matches!(frame.reading, ReadingMode::Capped(_))
            && event.position.x == view.rect.x + view.rect.cols - 1
            && event.position.y == view.rect.y + view.rect.rows - 1
        {
            self.resizing = true;
            return output;
        }
        if event.kind == MouseKind::Press
            && !view.overlay
            && frame.reading != ReadingMode::Grid
            && id != frame.active
        {
            // Focusing a background tile moves the overlay. Its old body coordinates
            // must not also become a click or selection in the newly moved terminal.
            emit(&mut output, Action::Focus(id), true);
            return output;
        }
        if event.position.y == view.rect.y {
            if event.kind != MouseKind::Press {
                return output;
            }
            let buttons = if view.overlay && matches!(frame.reading, ReadingMode::Capped(_)) {
                3
            } else {
                2
            };
            let right = view.rect.x + view.rect.cols;
            let first = right.saturating_sub(buttons * 3);
            let action = if event.position.x >= first {
                let slot = (event.position.x - first) / 3;
                if view.overlay {
                    match (buttons, slot) {
                        (3, 0) => Action::CloseReading,
                        (3, 1) | (2, 0) => Action::Fullscreen(id),
                        _ => Action::CloseReading,
                    }
                } else if slot == 0 {
                    Action::Read(id)
                } else {
                    Action::Fullscreen(id)
                }
            } else {
                Action::Focus(id)
            };
            emit(&mut output, action, true);
            return output;
        }
        let app_mouse = view.modes.mouse_click || view.modes.mouse_drag || view.modes.mouse_motion;
        if matches!(event.kind, MouseKind::WheelUp | MouseKind::WheelDown)
            && (event.shift || !app_mouse)
        {
            let lines = if event.kind == MouseKind::WheelUp {
                3
            } else {
                -3
            };
            emit(&mut output, Action::Scroll { pane: id, lines }, true);
        } else if event.kind == MouseKind::Press && event.button == 0 && (event.shift || !app_mouse)
        {
            emit(&mut output, Action::Focus(id), true);
            let start = local_point(event.position, view.body);
            self.selection = Some((id, start, start));
            emit(
                &mut output,
                Action::SelectText {
                    pane: id,
                    start,
                    end: start,
                },
                true,
            );
        } else {
            if event.kind == MouseKind::Press {
                emit(&mut output, Action::Focus(id), true);
            }
            if app_mouse && !event.shift {
                output.push(ClientMessage::Mouse(event));
            }
        }
        output
    }
    fn toggle_help(&mut self, frame: &Frame) {
        self.help_override = Some(!self.help_override.unwrap_or(frame.help));
    }
    fn resize_action(&self, frame: &Frame, dx: i16, dy: i16) -> Option<Action> {
        let view = frame.panes.iter().rev().find(|v| v.overlay)?;
        Some(Action::ReadResize {
            cols: view
                .rect
                .cols
                .saturating_add_signed(dx)
                .clamp(20, frame.cols),
            rows: view
                .rect
                .rows
                .saturating_add_signed(dy)
                .clamp(3, frame.rows - 1),
        })
    }
    fn binding(&mut self, name: &str, frame: &Frame) -> Option<Action> {
        Some(match name {
            "help" => Action::Help,
            "grid" => Action::GridPicker,
            "read" => Action::Read(frame.active),
            "fullscreen" => Action::Fullscreen(frame.active),
            "agents" => Action::Agents(super::ui::AgentFilter::All),
            "attention" => Action::Agents(super::ui::AgentFilter::Attention),
            "finished" => Action::Agents(super::ui::AgentFilter::Finished),
            "move" => Action::MovePicker,
            "focus-left" => Action::FocusDirection { dx: -1, dy: 0 },
            "focus-right" => Action::FocusDirection { dx: 1, dy: 0 },
            "focus-up" => Action::FocusDirection { dx: 0, dy: -1 },
            "focus-down" => Action::FocusDirection { dx: 0, dy: 1 },
            "detach" => Action::Detach,
            "copy" => Action::Copy(frame.active),
            "takeover" => Action::Takeover,
            "scroll" => {
                self.scrolling = true;
                return None;
            }
            "read-wider" => return self.resize_action(frame, 10, 0),
            "read-narrower" => return self.resize_action(frame, -10, 0),
            "read-taller" => return self.resize_action(frame, 0, 2),
            "read-shorter" => return self.resize_action(frame, 0, -2),
            _ => return None,
        })
    }
    fn pick_dialog(&self, frame: &Frame, index: usize) -> Vec<ClientMessage> {
        use super::ui::DialogKind;
        let Some(dialog) = frame.ui.dialog else {
            return vec![];
        };
        let mut actions = vec![];
        if dialog.kind == DialogKind::Grid {
            if let Some(&(cols, rows)) = super::workspace::GRIDS.get(index) {
                let occupied = frame
                    .panes
                    .iter()
                    .filter(|v| !v.overlay && !frame.ui.unused.contains(&v.pane.id))
                    .count();
                if usize::from(cols) * usize::from(rows) < occupied {
                    return vec![];
                }
                actions.push(Action::Grid { cols, rows });
            } else {
                return vec![];
            }
        } else {
            let panes: Vec<_> = frame
                .panes
                .iter()
                .filter(|v| !v.overlay)
                .map(|v| v.pane.clone())
                .collect();
            let entries = super::workspace::dialog_entries(&panes, Some(dialog));
            let Some(&id) = entries.get(index) else {
                return vec![];
            };
            actions.push(if dialog.kind == DialogKind::Move {
                Action::Swap {
                    source: dialog.source.unwrap_or(frame.active),
                    target: id,
                }
            } else {
                Action::Focus(id)
            });
        }
        actions.push(Action::DialogClose);
        actions.into_iter().map(ClientMessage::Action).collect()
    }
}
fn local_point(point: Point, body: Rect) -> Point {
    Point {
        x: point
            .x
            .saturating_sub(body.x)
            .min(body.cols.saturating_sub(1)),
        y: point
            .y
            .saturating_sub(body.y)
            .min(body.rows.saturating_sub(1)),
    }
}
fn emit(output: &mut Vec<ClientMessage>, action: Action, controller: bool) {
    if controller || matches!(action, Action::Detach | Action::Takeover) {
        output.push(ClientMessage::Action(action));
    }
}
fn push_input(output: &mut Vec<ClientMessage>, bytes: &[u8]) {
    if let Some(ClientMessage::Input(previous)) = output.last_mut() {
        if previous.len() + bytes.len() <= MAX_INPUT {
            previous.extend_from_slice(bytes);
            return;
        }
    }
    output.push(ClientMessage::Input(bytes.to_vec()));
}
/// Matches the renderer's centered help panel in the work area above the footer.
pub fn help_bounds(cols: u16, rows: u16) -> Rect {
    let width = cols.saturating_sub(4).min(76);
    let height = rows.saturating_sub(2).min(18);
    Rect {
        x: (cols - width) / 2,
        y: (rows.saturating_sub(1) - height) / 2,
        cols: width,
        rows: height,
    }
}
