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
}
impl Router {
    pub fn route(&mut self, event: InputEvent, frame: &Frame) -> Vec<ClientMessage> {
        if frame.ui.dialog.is_some() {
            self.header_drag = None;
            self.boundary_drag = None;
            self.selecting = false;
        }
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
                            Action::GridPicker
                                | Action::MovePicker
                                | Action::Agents(_)
                                | Action::Config
                                | Action::ClosePaneRequest(_)
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
                    output.extend(self.pick_dialog(frame, dialog.selection));
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
            } else if dialog.kind == super::ui::DialogKind::Color {
                super::workspace::color_cards(bounds)
                    .iter()
                    .position(|rect| rect.contains(event.position))
            } else if dialog.kind == super::ui::DialogKind::Move {
                let (columns, rows) = frame.ui.grid_shape;
                super::workspace::move_cards(bounds, columns, rows)
                    .iter()
                    .position(|rect| rect.contains(event.position))
            } else if dialog.kind == super::ui::DialogKind::CloseConfirm {
                super::workspace::close_buttons(bounds)
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
        if event.kind == MouseKind::Press
            && event.position.y == frame.rows - 1
            && event.position.x >= frame.cols.saturating_sub(27)
            && event.position.x < frame.cols.saturating_sub(20)
        {
            self.pending_modal = true;
            emit(&mut output, Action::Config, true);
            return output;
        }
        if event.kind == MouseKind::Press
            && event.position.y == frame.rows - 1
            && event.position.x >= frame.cols.saturating_sub(20)
            && event.position.x < frame.cols.saturating_sub(10)
        {
            let action = if event.position.x < frame.cols - 13 {
                Action::Read(frame.active)
            } else {
                Action::Fullscreen(frame.active)
            };
            emit(&mut output, action, true);
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
        if frame.reading == ReadingMode::Grid
            && !frame.ui.scratch_visible
            && event.kind == MouseKind::Press
        {
            for view in frame
                .panes
                .iter()
                .filter(|v| Some(v.pane.id) != frame.ui.scratch)
            {
                if view.rect.y == 0 && view.rect.x > 0 && event.position.x == view.rect.x - 1 {
                    self.boundary_drag = Some((Some((view.pane.number - 1) as u8 - 1), None));
                    return output;
                }
                if view.rect.y > 0 && event.position.y == view.rect.y - 1 {
                    let rows = frame.ui.grid_shape.0.max(1);
                    self.boundary_drag = Some((
                        None,
                        Some(((view.pane.number - 1) / usize::from(rows)) as u8 - 1),
                    ));
                    return output;
                }
            }
        }
        if frame.reading != ReadingMode::Grid && !frame.ui.scratch_visible {
            if event.kind == MouseKind::Press
                && frame.panes.iter().any(|view| {
                    view.pane.id == frame.active
                        && view.body.cols > 0
                        && view.body.x + view.body.cols < frame.cols
                        && event.position.x == view.body.x + view.body.cols + 1
                        && event.position.y == view.body.y
                })
            {
                emit(&mut output, Action::WrapToggle, true);
                return output;
            }
            let tabviews: Vec<_> = frame
                .panes
                .iter()
                .filter(|view| Some(view.pane.id) != frame.ui.scratch)
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
            )
            .into_iter()
            .find(|(_, rect)| rect.contains(event.position))
            {
                if event.kind == MouseKind::Press && target == super::workspace::TabTarget::New {
                    if tabviews.len() < 9 {
                        emit(&mut output, Action::NewTab, true);
                    }
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
                        )
                        .into_iter()
                        .find(|(item, _)| *item == target)
                        .map(|(_, rect)| rect)
                        .unwrap();
                        let close = matches!(target, TabTarget::Pane(_))
                            && event.kind == MouseKind::Press
                            && super::workspace::tab_close(bounds).contains(event.position);
                        let color = matches!(target, TabTarget::Pane(_))
                            && event.kind == MouseKind::Press
                            && super::workspace::tab_number(bounds).contains(event.position);
                        if color {
                            self.pending_modal = true;
                            emit(&mut output, Action::TabColor(view.pane.id), true);
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
            let buttons = 2;
            let right = view.rect.x + view.rect.cols;
            let first = right.saturating_sub(buttons * 3);
            let action = if event.position.x >= first {
                let slot = (event.position.x - first) / 3;
                if slot == 0 {
                    Action::Read(id)
                } else {
                    Action::Fullscreen(id)
                }
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
        if frame.reading == ReadingMode::Grid {
            return None;
        }
        let view = frame.panes.iter().find(|v| v.pane.id == frame.active)?;
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
                return vec![
                    ClientMessage::Action(Action::Focus(id)),
                    ClientMessage::Action(Action::DialogClose),
                ];
            }
            return vec![];
        }
        if dialog.kind == DialogKind::Notifications {
            let Some(item) = frame.ui.notifications.get(index) else {
                return vec![];
            };
            let mut actions = vec![ClientMessage::Action(Action::Acknowledge(item.id))];
            if frame.panes.iter().any(|view| view.pane.id == item.pane) {
                actions.push(ClientMessage::Action(Action::Focus(item.pane)));
            }
            actions.push(ClientMessage::Action(Action::DialogClose));
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
            if index == 5 {
                return vec![ClientMessage::Action(Action::WrapToggle)];
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
        if dialog.kind == DialogKind::CloseConfirm {
            return match (index, dialog.source) {
                (0, _) => vec![ClientMessage::Action(Action::DialogClose)],
                (1, Some(id)) => vec![
                    ClientMessage::Action(Action::ClosePaneConfirm(id)),
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
            InputEvent::Bytes(bytes) => {
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
    fn grid_resize(&self, frame: &Frame, dx: i16, dy: i16) -> Option<Action> {
        if frame.reading != ReadingMode::Grid {
            return None;
        }
        let (cols, rows) = frame.ui.grid_shape;
        let index = frame
            .panes
            .iter()
            .filter(|view| Some(view.pane.id) != frame.ui.scratch)
            .position(|view| view.pane.id == frame.active)?;
        let col = index % usize::from(cols.max(1));
        let row = index / usize::from(cols.max(1));
        let column = if dx != 0 && cols > 1 {
            let boundary = col.min(usize::from(cols) - 2);
            let current = if frame.ui.column_weights.len() == usize::from(cols) {
                frame.ui.column_weights[..=boundary].iter().sum()
            } else {
                1000 * (boundary as u16 + 1) / u16::from(cols)
            };
            Some((boundary as u8, current.saturating_add_signed(dx)))
        } else {
            None
        };
        let row = if dy != 0 && rows > 1 {
            let boundary = row.min(usize::from(rows) - 2);
            let current = if frame.ui.row_weights.len() == usize::from(rows) {
                frame.ui.row_weights[..=boundary].iter().sum()
            } else {
                1000 * (boundary as u16 + 1) / u16::from(rows)
            };
            Some((boundary as u8, current.saturating_add_signed(dy)))
        } else {
            None
        };
        Some(Action::ResizeGrid { column, row })
    }
}
