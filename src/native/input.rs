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
                (self.pending.len() >= size + 1).then_some(size + 1)
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
    swap: Option<Option<PaneId>>,
    scrolling: bool,
    selection: Option<(PaneId, Point, Point)>,
}
impl Router {
    pub fn route(&mut self, event: InputEvent, frame: &Frame) -> Vec<ClientMessage> {
        match event {
            InputEvent::Paste(bytes) if frame.controller && !frame.help => {
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
                let action = match key {
                    b"d" => Some(Action::Detach),
                    b"t" | b"T" => Some(Action::Takeover),
                    b"z" => Some(Action::Read(frame.active)),
                    b"f" => Some(Action::Fullscreen(frame.active)),
                    b"?" => Some(Action::Help),
                    b"g" => {
                        let count = frame.panes.iter().filter(|p| !p.overlay).count();
                        let (cols, rows) = match count {
                            1 => (2, 1),
                            2 => (2, 2),
                            _ => (1, 1),
                        };
                        Some(Action::Grid { cols, rows })
                    }
                    b"[" => {
                        self.scrolling = true;
                        None
                    }
                    b"y" => Some(Action::Copy(frame.active)),
                    b"s" => {
                        self.swap = Some(None);
                        None
                    }
                    [digit @ b'1'..=b'9'] => Some(Action::Select((digit - b'0') as usize)),
                    b"\x02" if frame.controller => {
                        output.push(ClientMessage::Input(vec![2]));
                        None
                    }
                    _ => None,
                };
                if let Some(action) = action {
                    emit(&mut output, action, frame.controller);
                }
                continue;
            }
            if key == b"\x02" {
                self.prefix = true;
                continue;
            }
            if !frame.controller {
                continue;
            }
            if frame.help {
                if matches!(key, b"q" | b"\x1b" | b"?") {
                    emit(&mut output, Action::Help, true);
                }
                continue;
            }
            if let Some(source) = self.swap.take() {
                if let [digit @ b'1'..=b'9'] = key {
                    if let Some(pane) = frame
                        .panes
                        .iter()
                        .find(|p| p.pane.number == (digit - b'0') as usize)
                    {
                        if let Some(source) = source {
                            emit(
                                &mut output,
                                Action::Swap {
                                    source,
                                    target: pane.pane.id,
                                },
                                true,
                            );
                        } else {
                            self.swap = Some(Some(pane.pane.id));
                        }
                    }
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
                push_input(&mut output, key);
            }
        }
        output
    }
    fn mouse(&mut self, event: MouseEvent, frame: &Frame) -> Vec<ClientMessage> {
        let mut output = Vec::new();
        if !frame.controller {
            return output;
        }
        if frame.help {
            let bounds = help_bounds(frame.cols, frame.rows);
            if event.kind == MouseKind::Press && !bounds.contains(event.position) {
                emit(&mut output, Action::Help, true);
            }
            return output;
        }
        if event.kind == MouseKind::Press
            && event.position.y == frame.rows.saturating_sub(1)
            && event.position.x >= frame.cols.saturating_sub(3)
        {
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
