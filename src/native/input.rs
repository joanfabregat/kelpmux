use super::protocol::*;

/// Incremental terminal framing. Paste payloads are never interpreted as keys.
#[derive(Default)]
pub struct Decoder {
    pending: Vec<u8>,
    paste: Option<Vec<u8>>,
    discard_osc: bool,
    osc_escape: bool,
}
impl Decoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<InputEvent> {
        let mut events = Vec::new();
        // Processing byte by byte bounds retained storage even for an oversized packet.
        for byte in bytes {
            if self.discard_osc {
                if *byte == 7 || self.osc_escape && *byte == b'\\' {
                    self.discard_osc = false;
                    self.osc_escape = false;
                    continue;
                }
                if *byte < 32 && !matches!(*byte, 7 | 27) {
                    // A user control key cancels a broken response, preserving
                    // Ctrl-C and the mux prefix as immediate escape routes.
                    self.discard_osc = false;
                    self.osc_escape = false;
                } else {
                    self.osc_escape = *byte == 27;
                    continue;
                }
            }
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
            if self.pending.starts_with(b"\x1b]") {
                if let Some(control) = self
                    .pending
                    .last()
                    .copied()
                    .filter(|byte| *byte < 32 && !matches!(*byte, 7 | 27))
                {
                    self.pending.clear();
                    events.push(InputEvent::Bytes(vec![control]));
                    continue;
                }
                if self.pending.last() == Some(&7) || self.pending.ends_with(b"\x1b\\") {
                    if let Some(event) = theme_reply(&self.pending) {
                        events.push(event);
                    }
                    self.pending.clear();
                    continue;
                }
                if self.pending.len() >= 128 {
                    self.discard_osc = true;
                    self.osc_escape = self.pending.last() == Some(&27);
                    self.pending.clear();
                }
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
        if self.discard_osc {
            return vec![];
        }
        if self.pending.starts_with(b"\x1b]") && self.pending.len() > 2 {
            self.discard_osc = true;
            self.osc_escape = self.pending.last() == Some(&27);
            self.pending.clear();
            return vec![];
        }
        if self.paste.is_some() || self.pending.is_empty() {
            return vec![];
        }
        vec![InputEvent::Bytes(std::mem::take(&mut self.pending))]
    }
}
fn theme_reply(bytes: &[u8]) -> Option<InputEvent> {
    let text = std::str::from_utf8(bytes).ok()?.strip_prefix("\x1b]")?;
    let text = text
        .strip_suffix('\u{7}')
        .or_else(|| text.strip_suffix("\x1b\\"))?;
    let (role, color) = text.split_once(';')?;
    let background = match role {
        "10" => false,
        "11" => true,
        _ => return None,
    };
    let components: Vec<&str> = if let Some(rgb) = color.strip_prefix("rgb:") {
        rgb.split('/').collect()
    } else {
        let hex = color.strip_prefix('#')?;
        if !hex.is_ascii() || hex.is_empty() || hex.len() % 3 != 0 {
            return None;
        }
        let width = hex.len() / 3;
        if width > 4 {
            return None;
        }
        (0..3)
            .map(|index| &hex[index * width..(index + 1) * width])
            .collect()
    };
    if components.len() != 3 {
        return None;
    }
    let mut rgb = [0; 3];
    for (out, component) in rgb.iter_mut().zip(components) {
        if component.is_empty()
            || component.len() > 4
            || !component.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return None;
        }
        let value = u32::from_str_radix(component, 16).ok()?;
        let maximum = (1u32 << (component.len() * 4)) - 1;
        *out = ((value * 255 + maximum / 2) / maximum) as u8;
    }
    Some(InputEvent::ThemeColor { background, rgb })
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
    status_search: bool,
    status_query: Option<String>,
    focus_click: bool,
    search: Option<(String, bool)>,
    search_utf8: Vec<u8>,
    selecting: bool,
    header_drag: Option<PaneId>,
    boundary_drag: Option<(Option<u8>, Option<u8>)>,
    prefix: bool,
    pending_modal: bool,
    scrolling: bool,
    selection: Option<(PaneId, Point, Point)>,
    observed_help: bool,
    help_override: Option<bool>,
    suppressed_releases: Vec<Vec<u8>>,
}
impl Router {
    pub fn route(&mut self, event: InputEvent, frame: &Frame) -> Vec<ClientMessage> {
        if let InputEvent::ThemeColor { background, rgb } = event {
            return vec![ClientMessage::ThemeColor { background, rgb }];
        }
        if frame.ui.dialog.is_some() {
            self.suppressed_releases.clear();
            self.header_drag = None;
            self.boundary_drag = None;
            self.selecting = false;
        }
        if frame
            .ui
            .dialog
            .is_some_and(|d| d.kind == super::ui::DialogKind::Status)
        {
            return self.status_event(event, frame);
        }
        self.status_search = false;
        self.status_query = None;
        if frame
            .ui
            .dialog
            .is_some_and(|d| d.kind == super::ui::DialogKind::Search)
        {
            return self.search_event(event, frame);
        }
        self.search = None;
        self.search_utf8.clear();
        if frame.ui.dialog.is_some() || frame.help {
            self.pending_modal = false;
        }
        if self.observed_help != frame.help {
            self.observed_help = frame.help;
            self.help_override = None;
        }
        let help = self.help_override.unwrap_or(frame.help);
        match event {
            InputEvent::ThemeColor { .. } => vec![],
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
            let wire_key = &bytes[position..position + length];
            position += length;
            let enhanced = enhanced_key(wire_key);
            let key = enhanced
                .as_ref()
                .map_or(wire_key, |(key, _)| key.as_slice());
            if enhanced.as_ref().is_some_and(|(_, event)| *event == 3) {
                if let Some(index) = self
                    .suppressed_releases
                    .iter()
                    .position(|old| old.as_slice() == key)
                {
                    self.suppressed_releases.remove(index);
                    continue;
                }
                if !self.prefix
                    && !self.pending_modal
                    && !self.selecting
                    && !self.scrolling
                    && key != [frame.ui.config.prefix]
                    && frame.controller
                    && !frame.help
                    && frame.ui.dialog.is_none()
                    && frame
                        .panes
                        .iter()
                        .any(|v| v.pane.id == frame.active && v.modes.keyboard_flags & 2 != 0)
                {
                    push_input(&mut output, wire_key);
                }
                continue;
            }
            if self.prefix {
                self.prefix = false;
                if frame
                    .panes
                    .iter()
                    .any(|v| v.pane.id == frame.active && v.modes.keyboard_flags & 2 != 0)
                {
                    if self.suppressed_releases.len() == 32 {
                        self.suppressed_releases.remove(0);
                    }
                    self.suppressed_releases.push(key.to_vec());
                }
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
                        b"g" | b"e" => Some(Action::CloseReading),
                        b"p" => Some(Action::ParkRequest(frame.active)),
                        b"u" => Some(Action::Parked),
                        b"\r" | b"\n" => Some(Action::NewTab),
                        b"\t" => tab_navigation(frame, 1, false),
                        b"\x1b[Z" => tab_navigation(frame, -1, false),
                        b"\x1b[5~" => tab_navigation(frame, -1, true),
                        b"\x1b[6~" => tab_navigation(frame, 1, true),
                        b"v" => Some(Action::TabPosition),
                        b"," => Some(Action::Config),
                        b"o" => Some(Action::Overview),
                        b"b" => Some(Action::Notifications),
                        b"r" => Some(Action::Recovery),
                        b"/" => Some(Action::SearchOpen),
                        b"c" => Some(Action::Scratch),
                        b"N" => Some(Action::NextAttention),
                        b"P" => Some(Action::NextFinished),
                        b" " => {
                            self.selecting = true;
                            Some(Action::SelectionStart)
                        }
                        b"H" => self.grid_resize(frame, -40, 0),
                        b"L" => self.grid_resize(frame, 40, 0),
                        b"K" => self.grid_resize(frame, 0, -40),
                        b"J" => self.grid_resize(frame, 0, 40),
                        b"a" => Some(Action::Agents(super::ui::AgentFilter::All)),
                        b"!" => Some(Action::Agents(super::ui::AgentFilter::Attention)),
                        b"n" => Some(Action::Agents(super::ui::AgentFilter::Finished)),
                        b"h" => Some(Action::FocusDirection { dx: -1, dy: 0 }),
                        b"l" => Some(Action::FocusDirection { dx: 1, dy: 0 }),
                        b"j" => Some(Action::FocusDirection { dx: 0, dy: 1 }),
                        b"k" => Some(Action::FocusDirection { dx: 0, dy: -1 }),
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
                            Action::GridPicker
                                | Action::MovePicker
                                | Action::Agents(_)
                                | Action::Config
                                | Action::ClosePaneRequest(_)
                                | Action::ParkRequest(_)
                                | Action::Parked
                                | Action::Overview
                                | Action::Notifications
                                | Action::Recovery
                                | Action::SearchOpen
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
            if self.selecting {
                let action = match key {
                    b"\x1b[A" => Some(Action::SelectionMove { dx: 0, dy: -1 }),
                    b"\x1b[B" => Some(Action::SelectionMove { dx: 0, dy: 1 }),
                    b"\x1b[C" => Some(Action::SelectionMove { dx: 1, dy: 0 }),
                    b"\x1b[D" => Some(Action::SelectionMove { dx: -1, dy: 0 }),
                    b"y" => {
                        self.selecting = false;
                        Some(Action::Copy(frame.active))
                    }
                    b"\x1b" | b"q" => {
                        self.selecting = false;
                        None
                    }
                    _ => None,
                };
                if let Some(action) = action {
                    emit(&mut output, action, true);
                }
                continue;
            }
            if let Some(dialog) = frame.ui.dialog {
                if key == b"y" && dialog.kind == super::ui::DialogKind::Recovery {
                    if frame
                        .ui
                        .recovery
                        .get(dialog.selection)
                        .is_none_or(|item| item.entry.resume_command.is_none())
                    {
                        continue;
                    }
                    emit(&mut output, Action::RecoveryCopy(dialog.selection), true);
                    continue;
                }
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
                    output.extend(self.pick_dialog(
                        frame,
                        if matches!(
                            dialog.kind,
                            super::ui::DialogKind::CloseConfirm
                                | super::ui::DialogKind::ParkConfirm
                                | super::ui::DialogKind::RestoreConfirm
                        ) {
                            1
                        } else {
                            dialog.selection
                        },
                    ));
                } else if let [digit @ b'1'..=b'9'] = key {
                    let index = if dialog.kind == super::ui::DialogKind::Agents {
                        let panes: Vec<_> = frame
                            .panes
                            .iter()
                            .filter(|v| !v.overlay && Some(v.pane.id) != frame.ui.scratch)
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
                if let Some(amount) = match key {
                    b"\x1b[A" => Some(-1),
                    b"\x1b[B" => Some(1),
                    b"\x1b[5~" => Some(-8),
                    b"\x1b[6~" => Some(8),
                    _ => None,
                } {
                    emit(&mut output, Action::HelpScroll(amount), true);
                    continue;
                }
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
                if frame
                    .panes
                    .iter()
                    .any(|v| v.pane.id == frame.active && v.modes.keyboard_flags & 2 != 0)
                {
                    if self.suppressed_releases.len() == 32 {
                        self.suppressed_releases.remove(0);
                    }
                    self.suppressed_releases.push(key.to_vec());
                }
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
                    push_input(
                        &mut output,
                        if enhanced.is_some() && modes.keyboard_flags != 0 {
                            wire_key
                        } else {
                            key
                        },
                    );
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
        if self.focus_click && matches!(event.kind, MouseKind::Move | MouseKind::Release) {
            if event.kind == MouseKind::Release {
                self.focus_click = false;
            }
            return output;
        }
        if event.kind == MouseKind::Press {
            self.focus_click = false;
        }
        if let Some((column, row)) = self.boundary_drag {
            if matches!(event.kind, MouseKind::Move | MouseKind::Release) {
                emit(
                    &mut output,
                    Action::ResizeGrid {
                        column: column.map(|i| {
                            (
                                i,
                                (u32::from(event.position.x) * 1000 / u32::from(frame.cols)) as u16,
                            )
                        }),
                        row: row.map(|i| {
                            (
                                i,
                                (u32::from(event.position.y) * 1000 / u32::from(frame.rows - 1))
                                    as u16,
                            )
                        }),
                    },
                    true,
                );
                if event.kind == MouseKind::Release {
                    self.boundary_drag = None;
                }
                return output;
            }
        }
        if let Some(source) = self.header_drag {
            if matches!(event.kind, MouseKind::Move | MouseKind::Release) {
                if event.kind == MouseKind::Release {
                    self.header_drag = None;
                    if let Some(view) = frame.panes.iter().find(|v| {
                        v.rect.contains(event.position) && Some(v.pane.id) != frame.ui.scratch
                    }) {
                        if view.pane.id != source {
                            emit(
                                &mut output,
                                Action::Swap {
                                    source,
                                    target: view.pane.id,
                                },
                                true,
                            );
                        }
                    }
                }
                return output;
            }
        }
        if let Some(dialog) = frame.ui.dialog {
            let bounds = super::workspace::dialog_bounds_for(frame.cols, frame.rows, dialog.kind);
            if dialog.kind == super::ui::DialogKind::Parked && event.kind == MouseKind::Press {
                let entries = frame_entries(frame);
                if let Some((index, _)) =
                    super::workspace::parked_cards(bounds, dialog.selection, entries.len())
                        .into_iter()
                        .find(|(_, rect)| {
                            super::workspace::parked_controls(*rect)[1].contains(event.position)
                        })
                {
                    self.pending_modal = true;
                    emit(&mut output, Action::ClosePaneRequest(entries[index]), true);
                    return output;
                }
            }
            if event.kind == MouseKind::Press
                && (!bounds.contains(event.position)
                    || (event.position.y == bounds.y
                        && event.position.x >= bounds.x + bounds.cols - 3))
            {
                emit(&mut output, Action::DialogClose, true);
                return output;
            }
            if dialog.kind == super::ui::DialogKind::Config {
                if let Some((index, value, _)) = super::workspace::config_targets(bounds)
                    .into_iter()
                    .find(|(_, _, rect)| rect.contains(event.position))
                {
                    if event.kind == MouseKind::Press {
                        let current = match index {
                            4 => Some(frame.ui.config.notification_sound),
                            _ => None,
                        };
                        if value.is_none() || value != current {
                            output.extend(self.pick_dialog(frame, index));
                        }
                    } else if event.kind == MouseKind::Move {
                        emit(&mut output, Action::DialogPick(index), true);
                    }
                }
                return output;
            }
            let index = if dialog.kind == super::ui::DialogKind::Grid {
                super::workspace::grid_cards(bounds)
                    .iter()
                    .position(|rect| rect.contains(event.position))
            } else if dialog.kind == super::ui::DialogKind::Color {
                super::workspace::color_cards(bounds)
                    .iter()
                    .position(|rect| rect.contains(event.position))
            } else if dialog.kind == super::ui::DialogKind::Move {
                let (columns, rows) = frame.ui.grid_shape;
                super::workspace::move_cards(bounds, columns, rows)
                    .iter()
                    .position(|rect| rect.contains(event.position))
            } else if matches!(
                dialog.kind,
                super::ui::DialogKind::CloseConfirm
                    | super::ui::DialogKind::ParkConfirm
                    | super::ui::DialogKind::RestoreConfirm
            ) {
                super::workspace::close_buttons(bounds)
                    .iter()
                    .position(|rect| rect.contains(event.position))
            } else if dialog.kind == super::ui::DialogKind::Parked {
                super::workspace::parked_cards(bounds, dialog.selection, frame_entries(frame).len())
                    .into_iter()
                    .find(|(_, rect)| rect.contains(event.position))
                    .map(|(index, _)| index)
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
                if index >= frame_entries(frame).len() {
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
            if matches!(event.kind, MouseKind::WheelUp | MouseKind::WheelDown) {
                emit(
                    &mut output,
                    Action::HelpScroll(if event.kind == MouseKind::WheelUp {
                        -3
                    } else {
                        3
                    }),
                    true,
                );
                return output;
            }
            let bounds = help_bounds(frame.cols, frame.rows);
            let close = event.position.y == bounds.y
                && event.position.x >= bounds.x + bounds.cols.saturating_sub(3);
            let toggle = frame.controls()[3].contains(event.position);
            if event.kind == MouseKind::Press
                && (!bounds.contains(event.position) || close || toggle)
            {
                self.toggle_help(frame);
                emit(&mut output, Action::Help, true);
            }
            return output;
        }
        if frame.ui.scratch_visible {
            if let Some(view) = frame
                .panes
                .iter()
                .find(|v| Some(v.pane.id) == frame.ui.scratch)
            {
                if event.kind == MouseKind::Press
                    && (event.position.y == view.rect.y
                        && event.position.x >= view.rect.x + view.rect.cols - 3
                        || !view.rect.contains(event.position))
                {
                    emit(&mut output, Action::ScratchClose, true);
                    return output;
                }
            }
        }
        let [config, mode, parked, help] = frame.controls();
        if event.kind == MouseKind::Press && frame.ui.dialog.is_none() && !frame.help {
            if let Some(sort) = super::status::metric_target(frame, event.position) {
                self.pending_modal = true;
                emit(&mut output, Action::StatusOpen(sort), true);
                return output;
            }
        }
        if event.kind == MouseKind::Press && config.contains(event.position) {
            self.pending_modal = true;
            emit(&mut output, Action::Config, true);
            return output;
        }
        if event.kind == MouseKind::Press && mode.contains(event.position) {
            if frame.active == 0 {
                emit(&mut output, Action::NewTab, true);
                return output;
            }
            emit(&mut output, Action::Read(frame.active), true);
            return output;
        }
        if event.kind == MouseKind::Press && parked.contains(event.position) {
            self.pending_modal = true;
            emit(&mut output, Action::Parked, true);
            return output;
        }
        if event.kind == MouseKind::Press && help.contains(event.position) {
            if frame.ui.notifications.iter().any(|item| !item.read) {
                self.pending_modal = true;
                emit(&mut output, Action::Notifications, true);
                return output;
            }
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
        if (frame.reading != ReadingMode::Grid || frame.active == 0) && !frame.ui.scratch_visible {
            let tabviews: Vec<_> = frame
                .panes
                .iter()
                .filter(|view| !view.pane.parked && Some(view.pane.id) != frame.ui.scratch)
                .collect();
            let active = tabviews
                .iter()
                .position(|view| view.pane.id == frame.active)
                .unwrap_or(0);
            if let Some((target, _)) = super::workspace::tab_targets(
                frame.cols,
                frame.rows,
                &frame.ui.config,
                tabviews.len(),
                active,
                frame.panes.iter().filter(|view| view.pane.parked).count(),
            )
            .into_iter()
            .find(|(_, rect)| rect.contains(event.position))
            {
                if event.kind == MouseKind::Press && target == super::workspace::TabTarget::Parked {
                    self.pending_modal = true;
                    emit(&mut output, Action::Parked, true);
                    return output;
                }
                if event.kind == MouseKind::Press && target == super::workspace::TabTarget::New {
                    emit(&mut output, Action::NewTab, true);
                    return output;
                }
                if matches!(
                    event.kind,
                    MouseKind::Press | MouseKind::WheelUp | MouseKind::WheelDown
                ) {
                    use super::workspace::TabTarget;
                    let index = match target {
                        _ if event.kind == MouseKind::WheelUp => active.saturating_sub(1),
                        _ if event.kind == MouseKind::WheelDown => {
                            (active + 1).min(tabviews.len().saturating_sub(1))
                        }
                        TabTarget::Pane(index) if event.kind == MouseKind::Press => index,
                        TabTarget::Previous => active.saturating_sub(1),
                        TabTarget::Next => (active + 1).min(tabviews.len().saturating_sub(1)),
                        _ => (active + 1).min(tabviews.len().saturating_sub(1)),
                    };
                    if let Some(view) = tabviews.get(index) {
                        let bounds = super::workspace::tab_targets(
                            frame.cols,
                            frame.rows,
                            &frame.ui.config,
                            tabviews.len(),
                            active,
                            frame.panes.iter().filter(|view| view.pane.parked).count(),
                        )
                        .into_iter()
                        .find(|(item, _)| *item == target)
                        .map(|(_, rect)| rect)
                        .unwrap();
                        let close = matches!(target, TabTarget::Pane(_))
                            && event.kind == MouseKind::Press
                            && super::workspace::tab_close(bounds).contains(event.position);
                        let park = matches!(target, TabTarget::Pane(_))
                            && event.kind == MouseKind::Press
                            && super::workspace::tab_park(bounds).contains(event.position);
                        if park {
                            self.pending_modal = true;
                            emit(&mut output, Action::ParkRequest(view.pane.id), true);
                        } else if close {
                            self.pending_modal = true;
                            emit(&mut output, Action::ClosePaneRequest(view.pane.id), true);
                        } else {
                            emit(&mut output, Action::Focus(view.pane.id), true);
                        }
                    }
                }
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
        if view.overlay && Some(id) == frame.ui.scratch && event.position.y == view.rect.y {
            return output;
        }
        if event.position.y == view.rect.y && frame.reading == ReadingMode::Grid {
            if event.kind != MouseKind::Press {
                return output;
            }
            let [park, close] = super::workspace::grid_controls(Rect {
                rows: 1,
                ..view.rect
            });
            let action = if close.contains(event.position) {
                self.pending_modal = true;
                Action::ClosePaneRequest(id)
            } else if park.contains(event.position) {
                self.pending_modal = true;
                Action::ParkRequest(id)
            } else if super::workspace::tab_number_for(view.rect, view.pane.number)
                .contains(event.position)
            {
                Action::Focus(id)
            } else {
                self.header_drag = Some(id);
                Action::Focus(id)
            };
            emit(&mut output, action, true);
            return output;
        }
        if !view.body.contains(event.position) {
            return output;
        }
        if frame.reading == ReadingMode::Grid
            && frame.active != id
            && event.kind == MouseKind::Press
        {
            self.focus_click = true;
            emit(&mut output, Action::Focus(id), true);
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
    fn binding(&mut self, name: &str, frame: &Frame) -> Option<Action> {
        Some(match name {
            "help" => Action::Help,
            "grid" => Action::CloseReading,
            "read" => Action::Read(frame.active),
            "fullscreen" => Action::Fullscreen(frame.active),
            "tab-position" => Action::TabPosition,
            "config" => Action::Config,
            "overview" => Action::Overview,
            "notifications" => Action::Notifications,
            "recovery" => Action::Recovery,
            "next-attention" => Action::NextAttention,
            "next-finished" => Action::NextFinished,
            "scratch" => Action::Scratch,
            "search" => Action::SearchOpen,
            "selection" => {
                self.selecting = true;
                Action::SelectionStart
            }
            "grid-narrower" => return self.grid_resize(frame, -40, 0),
            "grid-wider" => return self.grid_resize(frame, 40, 0),
            "grid-shorter" => return self.grid_resize(frame, 0, -40),
            "grid-taller" => return self.grid_resize(frame, 0, 40),
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
            "read-wider" | "read-narrower" => return None,
            "read-taller" | "read-shorter" => return None,
            _ => return None,
        })
    }
    fn pick_dialog(&self, frame: &Frame, index: usize) -> Vec<ClientMessage> {
        use super::ui::DialogKind;
        let Some(dialog) = frame.ui.dialog else {
            return vec![];
        };
        let mut actions = vec![];
        if dialog.kind == DialogKind::Parked {
            return frame_entries(frame)
                .get(index)
                .map(|id| vec![ClientMessage::Action(Action::RestoreRequest(*id))])
                .unwrap_or_default();
        }
        if dialog.kind == DialogKind::Color {
            return dialog
                .source
                .filter(|_| index < super::ui::PALETTE.len())
                .map(|pane| {
                    vec![
                        ClientMessage::Action(Action::TabColorSet {
                            pane,
                            color: index as u8,
                        }),
                        ClientMessage::Action(Action::DialogClose),
                    ]
                })
                .unwrap_or_default();
        }
        if dialog.kind == DialogKind::Overview {
            if let Some(&id) = frame_entries(frame).get(index) {
                return dialog_focus(frame, id);
            }
            return vec![];
        }
        if dialog.kind == DialogKind::Notifications {
            let Some(item) = frame.ui.notifications.get(index) else {
                return vec![];
            };
            let mut actions = vec![ClientMessage::Action(Action::Acknowledge(item.id))];
            if frame.panes.iter().any(|view| view.pane.id == item.pane) {
                actions.extend(dialog_focus(frame, item.pane));
            } else {
                actions.push(ClientMessage::Action(Action::DialogClose));
            }
            return actions;
        }
        if dialog.kind == DialogKind::Recovery {
            return frame
                .ui
                .recovery
                .get(index)
                .filter(|item| item.entry.resume_command.is_some())
                .map(|_| vec![ClientMessage::Action(Action::RecoveryCopy(index))])
                .unwrap_or_default();
        }
        if dialog.kind == DialogKind::Search {
            return frame
                .ui
                .search_hits
                .get(index)
                .map(|_| {
                    vec![
                        ClientMessage::Action(Action::SearchSelect(index)),
                        ClientMessage::Action(Action::DialogClose),
                    ]
                })
                .unwrap_or_default();
        }
        if dialog.kind == DialogKind::Config {
            if index == 4 {
                return vec![ClientMessage::Action(Action::NotificationSoundToggle)];
            }
            let positions = [
                super::ui::TabPosition::Auto,
                super::ui::TabPosition::Top,
                super::ui::TabPosition::Left,
                super::ui::TabPosition::Bottom,
            ];
            let Some(&position) = positions.get(index) else {
                return vec![];
            };
            return vec![ClientMessage::Action(Action::TabPositionSet(position))];
        }
        if matches!(
            dialog.kind,
            DialogKind::CloseConfirm | DialogKind::ParkConfirm | DialogKind::RestoreConfirm
        ) {
            return match (index, dialog.source) {
                (0, _) => vec![ClientMessage::Action(Action::DialogClose)],
                (1, Some(id)) => vec![
                    ClientMessage::Action(match dialog.kind {
                        DialogKind::ParkConfirm => Action::Park(id),
                        DialogKind::RestoreConfirm => Action::Restore(id),
                        _ => Action::ClosePaneConfirm(id),
                    }),
                    ClientMessage::Action(Action::DialogClose),
                ],
                _ => vec![],
            };
        }
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
                .filter(|v| !v.overlay && Some(v.pane.id) != frame.ui.scratch)
                .map(|v| v.pane.clone())
                .collect();
            let entries = super::workspace::dialog_entries(&panes, Some(dialog));
            let Some(&id) = entries.get(index) else {
                return vec![];
            };
            if dialog.kind == DialogKind::Move {
                actions.push(Action::Swap {
                    source: dialog.source.unwrap_or(frame.active),
                    target: id,
                });
            } else {
                return dialog_focus(frame, id);
            }
        }
        actions.push(Action::DialogClose);
        actions.into_iter().map(ClientMessage::Action).collect()
    }
}
/// Normalize negotiated CSI-u and xterm modifyOtherKeys for mux controls and
/// legacy applications. Enhanced applications retain the original wire event.
fn enhanced_key(bytes: &[u8]) -> Option<(Vec<u8>, u8)> {
    let payload = bytes.strip_prefix(b"\x1b[")?;
    let (code, modifier, event) = if let Some(payload) = payload.strip_suffix(b"u") {
        let text = std::str::from_utf8(payload).ok()?;
        let mut parts = text.split(';');
        let code = parts.next()?.split(':').next()?.parse::<u32>().ok()?;
        let mut mods = parts.next().unwrap_or("1").split(':');
        (
            code,
            mods.next()?.parse::<u8>().ok()?,
            mods.next().unwrap_or("1").parse::<u8>().ok()?,
        )
    } else {
        let text = std::str::from_utf8(payload.strip_suffix(b"~")?).ok()?;
        let mut parts = text.split(';');
        if parts.next()? != "27" {
            return None;
        }
        let modifier = parts.next()?.parse::<u8>().ok()?;
        let code = parts.next()?.parse::<u32>().ok()?;
        if parts.next().is_some() {
            return None;
        }
        (code, modifier, 1)
    };
    let mods = modifier.checked_sub(1)?;
    if mods & !7 != 0 {
        return None;
    }
    let mut key = if code == 13 && mods == 1 {
        vec![b'\n']
    } else if mods & 4 != 0 && (64..=127).contains(&code) {
        vec![(code as u8) & 31]
    } else if mods & 4 != 0 && code == 32 {
        vec![0]
    } else {
        let mut c = char::from_u32(code)?;
        if mods & 1 != 0 && c.is_ascii_lowercase() {
            c = c.to_ascii_uppercase();
        }
        let mut buf = [0; 4];
        c.encode_utf8(&mut buf).as_bytes().to_vec()
    };
    if mods & 2 != 0 {
        key.insert(0, 27);
    }
    Some((key, event))
}
fn dialog_focus(frame: &Frame, id: PaneId) -> Vec<ClientMessage> {
    if frame.panes.iter().any(|v| v.pane.id == id && v.pane.parked) {
        vec![ClientMessage::Action(Action::RestoreRequest(id))]
    } else {
        vec![
            ClientMessage::Action(Action::Focus(id)),
            ClientMessage::Action(Action::DialogClose),
        ]
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
fn tab_navigation(frame: &Frame, direction: isize, page: bool) -> Option<Action> {
    let panes: Vec<_> = frame
        .panes
        .iter()
        .filter(|view| !view.pane.parked && Some(view.pane.id) != frame.ui.scratch)
        .collect();
    if panes.is_empty() {
        return None;
    }
    let active = panes
        .iter()
        .position(|view| view.pane.id == frame.active)
        .unwrap_or(0);
    let index = if page && frame.reading == ReadingMode::Grid {
        let capacity =
            ((frame.cols + 1) / 13).max(1) as usize * ((frame.rows - 1) / 3).max(1) as usize;
        let page = (active as isize / capacity as isize + direction)
            .rem_euclid(panes.len().div_ceil(capacity) as isize) as usize;
        (page * capacity + active % capacity).min(panes.len() - 1)
    } else {
        (active as isize + direction).rem_euclid(panes.len() as isize) as usize
    };
    Some(Action::Focus(panes[index].pane.id))
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
    let height = rows.saturating_sub(2).min(30);
    Rect {
        x: (cols - width) / 2,
        y: (rows.saturating_sub(1) - height) / 2,
        cols: width,
        rows: height,
    }
}
fn frame_entries(frame: &Frame) -> Vec<u64> {
    use super::ui::DialogKind;
    match frame.ui.dialog.map(|dialog| dialog.kind) {
        Some(DialogKind::Notifications) => frame.ui.notifications.iter().map(|n| n.id).collect(),
        Some(DialogKind::Recovery) => (0..frame.ui.recovery.len() as u64).collect(),
        Some(DialogKind::Search) => (0..frame.ui.search_hits.len() as u64).collect(),
        _ => {
            let panes = frame
                .panes
                .iter()
                .filter(|view| Some(view.pane.id) != frame.ui.scratch)
                .map(|view| view.pane.clone())
                .collect::<Vec<_>>();
            super::workspace::dialog_entries(&panes, frame.ui.dialog)
        }
    }
}
impl Router {
    fn search_event(&mut self, event: InputEvent, frame: &Frame) -> Vec<ClientMessage> {
        if let InputEvent::Bytes(bytes) = &event {
            if !frame.controller || self.prefix || bytes.first() == Some(&frame.ui.config.prefix) {
                return self.keys(bytes, frame);
            }
        }
        if !frame.controller {
            return vec![];
        }
        self.pending_modal = false;
        if self.search.is_none() {
            self.search = Some((frame.ui.search_query.clone(), frame.ui.search_all));
        }
        let mut output = vec![];
        match event {
            InputEvent::ThemeColor { .. } => {}
            InputEvent::Mouse(mouse) => {
                let bounds = super::workspace::dialog_bounds(frame.cols, frame.rows);
                if mouse.kind == MouseKind::Press {
                    if !bounds.contains(mouse.position)
                        || mouse.position.y == bounds.y
                            && mouse.position.x >= bounds.x + bounds.cols - 3
                    {
                        emit(&mut output, Action::DialogClose, true);
                    } else if mouse.position.y >= bounds.y + 4 {
                        let visible = usize::from(bounds.rows.saturating_sub(4));
                        let selection = frame.ui.dialog.unwrap().selection;
                        let index = usize::from(mouse.position.y - bounds.y - 4)
                            + selection.saturating_sub(visible.saturating_sub(1));
                        output.extend(self.pick_dialog(frame, index));
                    }
                }
            }
            InputEvent::Paste(bytes) => {
                let (query, _) = self.search.as_mut().unwrap();
                for c in String::from_utf8_lossy(&bytes)
                    .chars()
                    .filter(|c| !c.is_control())
                {
                    if query.len() + c.len_utf8() <= 256 {
                        query.push(c);
                    }
                }
            }
            InputEvent::Bytes(mut bytes) => {
                if let Some((normalized, event)) = enhanced_key(&bytes) {
                    if event == 3 {
                        return output;
                    }
                    bytes = normalized;
                }
                match bytes.as_slice() {
                    b"\x1b" => {
                        emit(&mut output, Action::DialogClose, true);
                        return output;
                    }
                    b"\r" | b"\n" => {
                        return self.search_submit(frame);
                    }
                    b"\x1b[A" => {
                        emit(&mut output, Action::DialogNavigate { dx: 0, dy: -1 }, true);
                        return output;
                    }
                    b"\x1b[B" => {
                        emit(&mut output, Action::DialogNavigate { dx: 0, dy: 1 }, true);
                        return output;
                    }
                    b"\x19" => {
                        emit(
                            &mut output,
                            Action::SearchCopy(frame.ui.dialog.unwrap().selection),
                            true,
                        );
                        return output;
                    }
                    _ => {}
                }
                for byte in bytes {
                    match byte {
                        13 | 10 => {
                            return self.search_submit(frame);
                        }
                        27 => {
                            emit(&mut output, Action::DialogClose, true);
                            return output;
                        }
                        25 => {
                            emit(
                                &mut output,
                                Action::SearchCopy(frame.ui.dialog.unwrap().selection),
                                true,
                            );
                            return output;
                        }
                        1 | 9 => {
                            let (_, all) = self.search.as_mut().unwrap();
                            *all = !*all;
                        }
                        8 | 127 => {
                            self.search_utf8.clear();
                            self.search.as_mut().unwrap().0.pop();
                        }
                        21 => {
                            self.search_utf8.clear();
                            self.search.as_mut().unwrap().0.clear();
                        }
                        0..=31 => {}
                        _ => {
                            self.search_utf8.push(byte);
                            match std::str::from_utf8(&self.search_utf8) {
                                Ok(text) => {
                                    if self.search.as_ref().unwrap().0.len() + text.len() <= 256 {
                                        self.search.as_mut().unwrap().0.push_str(text);
                                    }
                                    self.search_utf8.clear();
                                }
                                Err(error) if error.error_len().is_some() => {
                                    self.search_utf8.clear()
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        let (query, all) = self.search.as_ref().unwrap();
        if query != &frame.ui.search_query || *all != frame.ui.search_all {
            emit(
                &mut output,
                Action::Search {
                    query: query.clone(),
                    all: *all,
                },
                true,
            );
        }
        output
    }
    fn search_submit(&self, frame: &Frame) -> Vec<ClientMessage> {
        let (query, all) = self.search.as_ref().unwrap();
        if query != &frame.ui.search_query || *all != frame.ui.search_all {
            vec![
                ClientMessage::Action(Action::Search {
                    query: query.clone(),
                    all: *all,
                }),
                ClientMessage::Action(Action::SearchSelect(frame.ui.dialog.unwrap().selection)),
                ClientMessage::Action(Action::DialogClose),
            ]
        } else {
            self.pick_dialog(frame, frame.ui.dialog.unwrap().selection)
        }
    }
    fn grid_resize(&self, _frame: &Frame, _dx: i16, _dy: i16) -> Option<Action> {
        None
    }
}

impl Router {
    fn status_event(&mut self, event: InputEvent, frame: &Frame) -> Vec<ClientMessage> {
        use super::status::{self, Sort, View};
        if !frame.controller {
            return vec![];
        }
        if matches!(
            event,
            InputEvent::Mouse(MouseEvent {
                kind: MouseKind::Press,
                ..
            })
        ) {
            self.focus_click = true;
        }
        self.pending_modal = false;
        let ui = &frame.ui;
        let bounds = status::bounds(frame.cols, frame.rows);
        let mut query = self
            .status_query
            .clone()
            .unwrap_or_else(|| ui.status_query.clone());
        let action = match event {
            InputEvent::Mouse(mouse) => match mouse.kind {
                MouseKind::WheelUp => Some(Action::StatusScroll(-3)),
                MouseKind::WheelDown => Some(Action::StatusScroll(3)),
                MouseKind::Press => {
                    if !bounds.contains(mouse.position)
                        || (mouse.position.y == bounds.y
                            && mouse.position.x >= bounds.x + bounds.cols - 3)
                    {
                        Some(Action::DialogClose)
                    } else if mouse.position.y
                        == bounds.y + 1 + status::summary_height(bounds, &ui.status)
                    {
                        Some(Action::StatusView(if mouse.position.x < bounds.x + 10 {
                            View::Main
                        } else {
                            View::Io
                        }))
                    } else if mouse.position.y == status::table_y(bounds, &ui.status) {
                        let mut x = bounds.x + 1;
                        let mut selected = None;
                        for (_, width, sort) in
                            status::columns(bounds.cols - 2, ui.status_view, &ui.status)
                        {
                            if width > 0 && mouse.position.x >= x && mouse.position.x < x + width {
                                selected = sort.map(Action::StatusSort);
                                break;
                            }
                            x += width;
                        }
                        selected
                    } else {
                        None
                    }
                }
                _ => None,
            },
            InputEvent::Bytes(mut bytes) => {
                if let Some((normalized, event)) = enhanced_key(&bytes) {
                    if event == 3 {
                        return vec![];
                    }
                    bytes = normalized;
                }
                match bytes.as_slice() {
                    b"\x1b" => Some(Action::DialogClose),
                    b"\t" => Some(Action::StatusView(if ui.status_view == View::Main {
                        View::Io
                    } else {
                        View::Main
                    })),
                    b"\x1b[A" => Some(Action::StatusScroll(-1)),
                    b"\x1b[B" => Some(Action::StatusScroll(1)),
                    b"\x1b[5~" => Some(Action::StatusScroll(-10)),
                    b"\x1b[6~" => Some(Action::StatusScroll(10)),
                    b"\x1b[H" => Some(Action::StatusScroll(i16::MIN)),
                    b"\x1b[F" => Some(Action::StatusScroll(i16::MAX)),
                    b"\r" | b"\n" => {
                        self.status_search = false;
                        None
                    }
                    b"\x7f" | b"\x08" => {
                        query.pop();
                        Some(Action::StatusQuery(query))
                    }
                    b"q" if !self.status_search => Some(Action::DialogClose),
                    b"c" if !self.status_search => Some(Action::StatusSort(Sort::Cpu)),
                    b"m" if !self.status_search => Some(Action::StatusSort(Sort::Memory)),
                    b"p" if !self.status_search => Some(Action::StatusSort(Sort::Pid)),
                    b"r" if !self.status_search => Some(Action::StatusSort(Sort::Read)),
                    b"w" if !self.status_search => Some(Action::StatusSort(Sort::Write)),
                    _ => {
                        let input = if bytes.first() == Some(&b'/') && !self.status_search {
                            self.status_search = true;
                            query.clear();
                            &bytes[1..]
                        } else {
                            &bytes[..]
                        };
                        if self.status_search && !input.starts_with(&[27]) {
                            self.search_utf8.extend_from_slice(input);
                            match std::str::from_utf8(&self.search_utf8) {
                                Ok(text) => {
                                    query.extend(
                                        text.chars()
                                            .filter(|c| !c.is_control())
                                            .take(128usize.saturating_sub(query.chars().count())),
                                    );
                                    self.search_utf8.clear();
                                    Some(Action::StatusQuery(query))
                                }
                                Err(error) if error.error_len().is_none() => None,
                                Err(_) => {
                                    self.search_utf8.clear();
                                    None
                                }
                            }
                        } else {
                            None
                        }
                    }
                }
            }
            InputEvent::Paste(bytes) if self.status_search => {
                query.extend(
                    String::from_utf8_lossy(&bytes)
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(128usize.saturating_sub(query.chars().count())),
                );
                Some(Action::StatusQuery(query))
            }
            _ => None,
        };
        if let Some(Action::StatusQuery(query)) = &action {
            self.status_query = Some(query.clone());
        }
        action
            .map(|action| vec![ClientMessage::Action(action)])
            .unwrap_or_default()
    }
}
