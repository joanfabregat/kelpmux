//! Versioned, bounded messages on a private local Unix socket.
use crate::{Result, model::Status};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub const VERSION: u16 = 4;
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;
pub const MAX_INPUT: usize = 64 * 1024;
pub type PaneId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub text: String,
    pub width: u8,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}
impl Default for Cell {
    fn default() -> Self {
        Self {
            text: " ".into(),
            width: 1,
            fg: Color::Default,
            bg: Color::Default,
            bold: false,
            italic: false,
            underline: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Point {
    pub x: u16,
    pub y: u16,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
}
impl Rect {
    pub fn contains(self, point: Point) -> bool {
        point.x >= self.x
            && point.y >= self.y
            && point.x < self.x.saturating_add(self.cols)
            && point.y < self.y.saturating_add(self.rows)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub position: Point,
    pub visible: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InputModes {
    pub application_cursor: bool,
    pub application_keypad: bool,
    pub bracketed_paste: bool,
    pub mouse_click: bool,
    pub mouse_drag: bool,
    pub mouse_motion: bool,
    pub mouse_sgr: bool,
    pub focus: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
    pub cursor: Option<Cursor>,
    pub modes: InputModes,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub id: PaneId,
    pub number: usize,
    pub pid: u32,
    pub cwd: String,
    pub title: String,
    pub palette: u8,
    pub status: Status,
    #[serde(default)]
    pub observation: crate::model::Observation,
    pub session_id: Option<String>,
    pub dead: bool,
    #[serde(default)]
    pub kind: Option<crate::model::Kind>,
    #[serde(default)]
    pub last_activity: u64,
    #[serde(default)]
    pub status_since: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ReadingMode {
    #[default]
    Grid,
    Tabs(PaneId),
    Capped(PaneId),
    Fullscreen(PaneId),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneView {
    pub pane: PaneInfo,
    pub rect: Rect,
    pub body: Rect,
    pub overlay: bool,
    pub modes: InputModes,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
    pub cursor: Option<Cursor>,
    pub panes: Vec<PaneView>,
    pub active: PaneId,
    pub reading: ReadingMode,
    pub controller: bool,
    pub help: bool,
    pub notice: Option<String>,
    #[serde(default)]
    pub ui: super::ui::UiState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseKind {
    Press,
    Release,
    Move,
    WheelUp,
    WheelDown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MouseEvent {
    pub position: Point,
    pub kind: MouseKind,
    pub button: u8,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    Bytes(Vec<u8>),
    Paste(Vec<u8>),
    Mouse(MouseEvent),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    /// Explicitly take this pane back from an independent native client.
    ClaimPane(PaneId),
    HelpScroll(i16),
    Overview,
    Notifications,
    Recovery,
    RecoveryCopy(usize),
    SearchCopy(usize),
    NotificationSoundToggle,
    Acknowledge(u64),
    NextAttention,
    NextFinished,
    Scratch,
    ScratchClose,
    Search {
        query: String,
        all: bool,
    },
    SearchOpen,
    SearchSelect(usize),
    ResizeGrid {
        column: Option<(u8, u16)>,
        row: Option<(u8, u16)>,
    },
    SelectionStart,
    SelectionMove {
        dx: i16,
        dy: i16,
    },
    Config,
    TabPositionSet(super::ui::TabPosition),
    ClosePaneRequest(PaneId),
    ClosePaneConfirm(PaneId),
    TabPosition,
    GridPicker,
    Agents(super::ui::AgentFilter),
    MovePicker,
    DialogNavigate {
        dx: i8,
        dy: i8,
    },
    DialogPick(usize),
    DialogClose,
    FocusDirection {
        dx: i8,
        dy: i8,
    },
    ReadResize {
        cols: u16,
        rows: u16,
    },
    Focus(PaneId),
    Select(usize),
    Read(PaneId),
    Fullscreen(PaneId),
    CloseReading,
    Grid {
        cols: u8,
        rows: u8,
    },
    Swap {
        source: PaneId,
        target: PaneId,
    },
    Scroll {
        pane: PaneId,
        lines: i32,
    },
    SelectText {
        pane: PaneId,
        start: Point,
        end: Point,
    },
    Copy(PaneId),
    Help,
    Detach,
    Takeover,
    Shutdown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    Stream(super::stream::Request),
    Diagnostics,
    History {
        pane: PaneId,
        start: i32,
        limit: usize,
    },
    InspectPane {
        pane: PaneId,
    },
    Attach {
        version: u16,
        cols: u16,
        rows: u16,
        takeover: bool,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    Input(Vec<u8>),
    Paste(Vec<u8>),
    Mouse(MouseEvent),
    Action(Action),
    /// Explicit local administration; does not transfer interactive ownership.
    Command(Action),
    Inspect,
    Ping,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    Stream(super::stream::Event),
    Diagnostics(super::diagnostics::Report),
    History {
        pane: PaneId,
        lines: Vec<(i32, String)>,
    },
    Patch(super::delta::Patch),
    PaneState {
        pane: PaneInfo,
        screen: Screen,
    },
    Welcome {
        version: u16,
        client_id: u64,
        controller: bool,
    },
    Frame(Frame),
    State {
        panes: Vec<PaneInfo>,
        active: PaneId,
        reading: ReadingMode,
        controller: Option<u64>,
    },
    Clipboard(String),
    Error(String),
    Detached,
    Pong,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub id: u64,
    pub pane: PaneId,
    pub title: String,
    pub status: Status,
    pub at: u64,
    pub read: bool,
    pub workspace: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryItem {
    pub workspace: String,
    pub entry: super::tracking::RecoveryEntry,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub pane: PaneId,
    pub line: i32,
    pub text: String,
}

pub fn write_message<T: Serialize>(writer: &mut impl Write, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_MESSAGE {
        return Err("native protocol message is too large".into());
    }
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}
pub fn read_message<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<Option<T>> {
    let mut header = [0u8; 4];
    let count = loop {
        match reader.read(&mut header[..1]) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            result => break result?,
        }
    };
    if count == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut header[1..])?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_MESSAGE {
        return Err("invalid native protocol message length".into());
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}

pub fn validate_size(cols: u16, rows: u16) -> Result<()> {
    if !(40..=400).contains(&cols) || !(10..=160).contains(&rows) {
        return Err("terminal size must be 40–400 columns and 10–160 rows".into());
    }
    Ok(())
}
