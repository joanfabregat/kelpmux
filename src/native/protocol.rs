//! The TUI's own frame, reading modes, input events and actions, plus the
//! local-socket envelope and shared stream types it uses from KelpServer.
//!
//! The stream TUI renders and routes input locally (ADR 0002 step 2), so the
//! composed protocol's presentation types (`Frame`, `PaneView`,
//! `ReadingMode`, `InputEvent`, `Action`) belong to KelpMux. They started as
//! copies of KelpServer's composed-protocol types, which KelpServer deletes in
//! step 3. Only the wire envelope (`ClientMessage::Stream`,
//! `ServerMessage::Stream`), its framing and the stream's own data types come
//! from KelpServer.
use serde::{Deserialize, Serialize};

pub use kelpclient::types::*;
pub use kelpserver::native::protocol::{
    ClientMessage, MAX_INPUT, MAX_MESSAGE, MouseEvent, MouseKind, Notification, NotificationOrigin,
    PaneSearchMatch, ServerMessage, read_message, write_message,
};

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
/// One composed screen of the TUI: cells, cursor, pane geometry and the UI
/// state the input router reads.
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

/// Decoded terminal input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    ThemeColor { background: bool, rgb: [u8; 3] },
    Bytes(Vec<u8>),
    Paste(Vec<u8>),
    Mouse(MouseEvent),
}

/// What the input router produces for the TUI controller: application
/// input for the active pane, or a UI action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiMessage {
    ThemeColor { background: bool, rgb: [u8; 3] },
    Input(Vec<u8>),
    Paste(Vec<u8>),
    Mouse(MouseEvent),
    Action(Action),
}

/// A TUI action: the controller applies it to its local workspace and sends
/// the session operations it needs on the stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    StatusOpen(super::status::Sort),
    StatusView(super::status::View),
    StatusSort(super::status::Sort),
    StatusQuery(String),
    StatusScroll(i16),
    /// Explicitly take this pane back from another client.
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

/// One line of a search result in the Search dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub pane: PaneId,
    pub line: i32,
    pub text: String,
}

/// The TUI's minimum composed size.
pub fn validate_size(cols: u16, rows: u16) -> crate::Result<()> {
    if !(40..=400).contains(&cols) || !(10..=160).contains(&rows) {
        return Err("terminal size must be 40–400 columns and 10–160 rows".into());
    }
    Ok(())
}
