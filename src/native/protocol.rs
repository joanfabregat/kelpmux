//! Versioned, bounded messages on a private local Unix socket.
use crate::{Result, model::Status};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub const VERSION: u16 = 57;
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;
pub const MAX_INPUT: usize = 64 * 1024;
pub use kelpmux_client_core::types::*;
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
impl Frame {
    pub fn sidebar_width(&self) -> u16 {
        self.ui.config.sidebar_width(self.cols)
    }
    pub fn sidebar_status(&self) -> bool {
        !matches!(self.reading, ReadingMode::Fullscreen(_))
            && (self.reading != ReadingMode::Grid || self.active == 0)
            && self.ui.config.tab_position.resolve(self.cols, self.rows)
                == super::ui::TabPosition::Left
    }
    pub fn controls(&self) -> [Rect; 4] {
        let count = self.panes.iter().filter(|view| view.pane.parked).count();
        if self.sidebar_status() {
            super::workspace::sidebar_controls(self.cols, self.rows, count, &self.ui.config)
        } else {
            [
                Rect {
                    x: super::ui::FOOTER_MODE_COLS,
                    y: self.rows - 1,
                    cols: super::ui::FOOTER_CONFIG_COLS,
                    rows: 1,
                },
                Rect {
                    x: 0,
                    y: self.rows - 1,
                    cols: super::ui::FOOTER_MODE_COLS,
                    rows: 1,
                },
                super::workspace::parked_button(self.rows, count),
                super::workspace::help_button(self.rows, count),
            ]
        }
    }
    pub fn footer_cols(&self) -> u16 {
        let agents = self.history_controls()[1];
        (agents.x
            + agents.cols
            + if self.ui.show_dev_badge {
                super::ui::GRID_DEV_COLS
            } else {
                0
            })
        .min(self.cols)
    }
    pub fn history_controls(&self) -> [Rect; 2] {
        if self.sidebar_status() {
            if self.ui.config.sidebar_folded {
                return [Rect::default(); 2];
            }
            let width = self.sidebar_width().saturating_sub(1);
            let half = width.div_ceil(2);
            [
                Rect {
                    x: 0,
                    y: self.rows - 1,
                    cols: half,
                    rows: 1,
                },
                Rect {
                    x: half,
                    y: self.rows - 1,
                    cols: width - half,
                    rows: 1,
                },
            ]
        } else {
            let help = self.controls()[3];
            let x = help.x + help.cols;
            let available = self.cols.saturating_sub(
                x + if self.ui.show_dev_badge {
                    super::ui::GRID_DEV_COLS
                } else {
                    0
                },
            );
            let closed = 10.min(available.div_ceil(2));
            let agents = 10.min(available.saturating_sub(closed));
            [
                Rect {
                    x,
                    y: self.rows - 1,
                    cols: closed,
                    rows: 1,
                },
                Rect {
                    x: x + closed,
                    y: self.rows - 1,
                    cols: agents,
                    rows: 1,
                },
            ]
        }
    }
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
    ThemeColor { background: bool, rgb: [u8; 3] },
    Bytes(Vec<u8>),
    Paste(Vec<u8>),
    Mouse(MouseEvent),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    StatusOpen(super::status::Sort),
    StatusView(super::status::View),
    StatusSort(super::status::Sort),
    StatusQuery(String),
    StatusScroll(i16),
    /// Explicitly take this pane back from an independent native client.
    ClaimPane(PaneId),
    HelpScroll(i16),
    Overview,
    Notifications,
    Recovery,
    RecoveryCopy(usize),
    ReopenLast,
    ReopenRequest(usize),
    ReopenTab(usize),
    RerunTab(usize),
    SearchCopy(usize),
    NotificationSoundToggle,
    AccentSet(super::ui::Accent),
    Acknowledge(u64),
    AcknowledgeAll,
    NotificationHistory,
    ResizeSidebar(u16),
    ToggleSidebarFold,
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
    AgentLinkPicker(PaneId),
    AgentLinkChoose(usize),
    WrapToggle,
    NewTab,
    ScrollTabs(i16),
    TabDrag(Option<(PaneId, Option<PaneId>)>),
    GridDrag(Option<(PaneId, PaneId)>),
    Reorder {
        source: PaneId,
        before: Option<PaneId>,
    },
    ParkRequest(PaneId),
    RestoreRequest(PaneId),
    Park(PaneId),
    Restore(PaneId),
    Parked,
    TabColor(PaneId),
    TabColorSet {
        pane: PaneId,
        color: u8,
    },
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
    ThemeColor {
        background: bool,
        rgb: [u8; 3],
    },
    Stream(super::stream::Request),
    Diagnostics,
    History {
        pane: PaneId,
        start: i32,
        limit: usize,
    },
    SearchPane {
        pane: PaneId,
        query: String,
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
    PaneSearch {
        pane: PaneId,
        hits: Vec<(i32, String)>,
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
    #[serde(default)]
    pub origin: Option<NotificationOrigin>,
    pub id: u64,
    pub pane: PaneId,
    pub title: String,
    pub status: Status,
    pub at: u64,
    pub read: bool,
    pub workspace: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationOrigin {
    pub run: String,
    pub pid: u32,
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
