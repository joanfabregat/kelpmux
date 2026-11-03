//! Shared UI state and configurable prefix bindings.
use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub const FOOTER_BG: u8 = 239;
pub const TAB_BG: u8 = 236;
/// Label + two-column inset + five meter sections + percentage + divider.
pub const MIN_SIDEBAR_COLUMNS: u16 = 22;
/// Three content columns plus the divider.
pub const FOLDED_SIDEBAR_COLUMNS: u16 = 4;
pub const PARKED_BG: u8 = 60;
pub const RESTORE_BG: u8 = 29;
pub const ALERT_ICON: &str = "!";
pub const WAIT_ICON: &str = "?";
pub const DONE_ICON: &str = "✓";
pub const SPINNER_FRAMES: [&str; 8] = ["◐", "◓", "◑", "◒", "◐", "◓", "◑", "◒"];
pub const SIDEBAR_BALL_FRAMES: [&str; 10] = [
    "●·····",
    "·●····",
    "··●···",
    "···●··",
    "····●·",
    "·····●",
    "····●·",
    "···●··",
    "··●···",
    "·●····",
];
pub const GRID_BALL_FRAMES: [&str; 4] = ["●··", "·●·", "··●", "·●·"];
pub const ACTIVITY_INTERVAL_MS: u64 = 150;
pub const GRID_ACTIVITY_INTERVAL_MS: u64 = 200;
pub const ANIMATION_TICK_MS: u64 = 50;
pub fn activity_phase(tick: usize, columns: u16) -> usize {
    tick.saturating_mul(ANIMATION_TICK_MS as usize)
        / if columns == 3 {
            GRID_ACTIVITY_INTERVAL_MS as usize
        } else {
            ACTIVITY_INTERVAL_MS as usize
        }
}
pub fn status_icon(status: crate::model::Status) -> &'static str {
    match status {
        crate::model::Status::Working => SPINNER_FRAMES[0],
        crate::model::Status::InputNeeded => WAIT_ICON,
        crate::model::Status::TurnFinished => DONE_ICON,
        crate::model::Status::Failed => ALERT_ICON,
        _ => status.marker(),
    }
}
fn notification_priority(status: crate::model::Status) -> u8 {
    match status {
        crate::model::Status::Unknown | crate::model::Status::Failed => 3,
        crate::model::Status::InputNeeded => 2,
        _ => 1,
    }
}
pub fn notification_icon<'a>(
    ui: &'a UiState,
    panes: impl IntoIterator<Item = &'a super::protocol::PaneInfo>,
) -> Option<&'static str> {
    let panes: Vec<_> = panes.into_iter().collect();
    ui.notifications
        .iter()
        .filter(|n| {
            !n.read
                && panes
                    .iter()
                    .any(|pane| super::activity::matches(n, pane, &ui.notification_run))
        })
        .map(|n| notification_state(ui, n))
        .max_by_key(|status| notification_priority(*status))
        .map(|status| {
            if status == crate::model::Status::Unknown {
                ALERT_ICON
            } else {
                status_icon(status)
            }
        })
}
fn notification_state(ui: &UiState, event: &super::protocol::Notification) -> crate::model::Status {
    if event.status != crate::model::Status::Unknown || !event.title.starts_with("Bell · ") {
        return event.status;
    }
    ui.notifications
        .iter()
        .rev()
        .find(|n| {
            n.status != crate::model::Status::Unknown
                && n.pane == event.pane
                && n.origin == event.origin
                && n.workspace == event.workspace
                && n.at.abs_diff(event.at) <= 2
        })
        .map_or(event.status, |n| n.status)
}
pub fn notification_symbol(ui: &UiState, event: &super::protocol::Notification) -> &'static str {
    let status = notification_state(ui, event);
    if status == crate::model::Status::Unknown {
        ALERT_ICON
    } else {
        status_icon(status)
    }
}
pub fn notification_label(icon: &str) -> &'static str {
    match icon {
        DONE_ICON => "Done",
        WAIT_ICON => "Input",
        _ => "Alerts",
    }
}
pub fn pane_indicator(ui: &UiState, pane: &super::protocol::PaneInfo) -> Option<&'static str> {
    use crate::model::Status;
    match pane.status {
        Status::InputNeeded | Status::TurnFinished | Status::Failed => {
            Some(status_icon(pane.status))
        }
        Status::Idle | Status::Unknown | Status::Exited => {
            // Prefer a typed completion/question/error over its accompanying bell.
            let event = ui
                .notifications
                .iter()
                .rev()
                .filter(|n| !n.read && super::activity::matches(n, pane, &ui.notification_run))
                .find(|n| n.status != Status::Unknown);
            event
                .map(|n| status_icon(n.status))
                .or_else(|| notification_icon(ui, [pane]))
        }
        Status::Working => None,
    }
}
pub fn visible_notifications<'a>(
    ui: &'a UiState,
    panes: impl IntoIterator<Item = &'a super::protocol::PaneInfo>,
) -> impl Iterator<Item = &'a super::protocol::Notification> {
    let panes: Vec<_> = panes.into_iter().collect();
    ui.notifications.iter().rev().filter(move |item| {
        ui.notification_history
            || (!item.read
                && panes
                    .iter()
                    .any(|pane| super::activity::matches(item, pane, &ui.notification_run)))
    })
}
pub fn has_unread_notifications<'a>(
    ui: &UiState,
    panes: impl IntoIterator<Item = &'a super::protocol::PaneInfo>,
) -> bool {
    panes.into_iter().any(|pane| {
        ui.notifications
            .iter()
            .any(|item| !item.read && super::activity::matches(item, pane, &ui.notification_run))
    })
}
pub fn notification_buttons(bounds: super::protocol::Rect) -> [super::protocol::Rect; 2] {
    use super::protocol::Rect;
    let width = bounds.cols.saturating_sub(2) / 2;
    [
        Rect {
            x: bounds.x + 1,
            y: bounds.y + 1,
            cols: width,
            rows: 1,
        },
        Rect {
            x: bounds.x + 1 + width,
            y: bounds.y + 1,
            cols: bounds.cols - 2 - width,
            rows: 1,
        },
    ]
}
pub const CONFIG_BUTTON_LABEL: &str = " ⚙︎ Config ";
pub const FOOTER_CONFIG_COLS: u16 = 10;
pub const FOOTER_MODE_COLS: u16 = 8;
pub const HELP_ROWS: u16 = 31;
pub const PALETTE: [(u8, u8); 9] = [
    (39, 24),
    (42, 29),
    (141, 60),
    (214, 94),
    (80, 30),
    (205, 89),
    (185, 58),
    (209, 95),
    (111, 67),
];
pub const COLOR_NAMES: [&str; 9] = [
    "Blue", "Green", "Purple", "Amber", "Teal", "Pink", "Lime", "Coral", "Slate",
];
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabPosition {
    #[default]
    #[serde(alias = "auto", alias = "top", alias = "bottom")]
    Left,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    #[default]
    Blue,
    Sage,
    Glacier,
    Slate,
    Sand,
    Clay,
}
impl Accent {
    pub const ALL: [Self; 6] = [
        Self::Blue,
        Self::Sage,
        Self::Glacier,
        Self::Slate,
        Self::Sand,
        Self::Clay,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Blue => "Blue",
            Self::Sage => "Sage",
            Self::Glacier => "Glacier",
            Self::Slate => "Slate",
            Self::Sand => "Sand",
            Self::Clay => "Clay",
        }
    }
    pub fn rgb(self) -> [u8; 3] {
        match self {
            Self::Blue => [95, 175, 255],
            Self::Sage => [131, 185, 170],
            Self::Glacier => [121, 190, 200],
            Self::Slate => [163, 171, 200],
            Self::Sand => [198, 173, 126],
            Self::Clay => [201, 147, 135],
        }
    }
    pub fn is_default(&self) -> bool {
        *self == Self::Blue
    }
}
impl TabPosition {
    pub fn resolve(self, _cols: u16, _rows: u16) -> Self {
        Self::Left
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentFilter {
    #[default]
    All,
    Attention,
    Finished,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialogKind {
    Status,
    Parked,
    Overview,
    AgentLink,
    Notifications,
    Recovery,
    Search,
    Grid,
    Agents,
    Move,
    Config,
    CloseConfirm,
    ParkConfirm,
    RestoreConfirm,
    ReopenConfirm,
    Color,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dialog {
    pub kind: DialogKind,
    pub selection: usize,
    pub filter: AgentFilter,
    pub source: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    pub view_modes: [Option<bool>; 3],
    pub notification_sound: bool,
    pub prefix: u8,
    // Accept obsolete width preferences from existing workspaces, but never save them.
    #[serde(rename = "read_width", skip_serializing)]
    pub legacy_read_width: Option<u16>,
    #[serde(rename = "wrap_lines", skip_serializing)]
    pub legacy_wrap_lines: Option<bool>,
    pub pane_colors: BTreeMap<u64, u8>,
    pub read_height_percent: u8,
    pub bindings: BTreeMap<String, String>,
    pub tab_position: TabPosition,
    /// Preferred sidebar width in terminal columns; None follows the screen.
    pub sidebar_columns: Option<u16>,
    #[serde(skip_serializing_if = "expanded_sidebar")]
    pub sidebar_folded: bool,
    #[serde(skip_serializing_if = "Accent::is_default")]
    pub accent: Accent,
    /// Legacy bouncing-ball width, accepted for saved-configuration compatibility.
    #[serde(skip_serializing_if = "default_activity_width")]
    pub activity_width: u16,
    #[serde(rename = "remembered_read_widths", skip_serializing)]
    pub legacy_read_widths: Option<[Option<u16>; 3]>,
}
impl Default for UiConfig {
    fn default() -> Self {
        Self {
            view_modes: [None; 3],
            notification_sound: false,
            prefix: 2,
            legacy_read_width: None,
            legacy_wrap_lines: None,
            pane_colors: BTreeMap::new(),
            read_height_percent: 95,
            bindings: BTreeMap::new(),
            tab_position: TabPosition::Left,
            sidebar_columns: None,
            sidebar_folded: false,
            accent: Accent::Blue,
            activity_width: 20,
            legacy_read_widths: None,
        }
    }
}
impl UiConfig {
    pub fn sidebar_width(&self, cols: u16) -> u16 {
        if self.sidebar_folded {
            return FOLDED_SIDEBAR_COLUMNS
                .min(cols.saturating_sub(10).max(cols / 2))
                .max(1);
        }
        // Retain room for the terminal on exceptionally narrow windows.
        let minimum = MIN_SIDEBAR_COLUMNS.min(cols.saturating_sub(10).max(cols / 2));
        self.sidebar_columns
            .unwrap_or_else(|| super::workspace::sidebar_width(cols))
            .clamp(minimum, (cols / 2).max(minimum).max(1))
    }
    pub fn validate(&self) -> Result<()> {
        if !(1..=26).contains(&self.prefix)
            || self
                .pane_colors
                .iter()
                .any(|(&pane, &color)| pane == 0 || usize::from(color) >= PALETTE.len())
            || !(20..=100).contains(&self.read_height_percent)
            || !(1..=4096).contains(&self.activity_width)
            || self
                .sidebar_columns
                .is_some_and(|width| !(18..=4096).contains(&width))
        {
            return Err("invalid prefix or reading dimensions".into());
        }
        const ACTIONS: &[&str] = &[
            "help",
            "grid",
            "read",
            "fullscreen",
            "agents",
            "attention",
            "finished",
            "move",
            "focus-left",
            "focus-right",
            "focus-up",
            "focus-down",
            "detach",
            "copy",
            "scroll",
            "takeover",
            "read-wider",
            "read-narrower",
            "read-taller",
            "read-shorter",
            "unbound",
            "tab-position",
            "config",
            "overview",
            "agent-link",
            "notifications",
            "recovery",
            "reopen",
            "next-attention",
            "next-finished",
            "scratch",
            "search",
            "selection",
            "grid-narrower",
            "grid-wider",
            "grid-shorter",
            "grid-taller",
        ];
        if self.bindings.iter().any(|(key, action)| {
            key.len() != 1 || !key.is_ascii() || !ACTIONS.contains(&action.as_str())
        }) {
            return Err("bindings require one ASCII key and a supported action".into());
        }
        Ok(())
    }
}
fn default_activity_width(width: &u16) -> bool {
    *width == 20
}
fn expanded_sidebar(folded: &bool) -> bool {
    !folded
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiState {
    #[serde(default)]
    pub show_dev_badge: bool,
    #[serde(default)]
    pub tab_scroll_anchor: Option<usize>,
    #[serde(default)]
    pub tab_drop: Option<(super::protocol::PaneId, Option<super::protocol::PaneId>)>,
    #[serde(default)]
    pub grid_drop: Option<(super::protocol::PaneId, super::protocol::PaneId)>,
    #[serde(default)]
    pub status: super::status::Snapshot,
    #[serde(default)]
    pub status_view: super::status::View,
    #[serde(default)]
    pub status_sort: super::status::Sort,
    #[serde(default = "status_descending")]
    pub status_descending: bool,
    #[serde(default)]
    pub status_query: String,
    #[serde(default)]
    pub status_offset: usize,
    /// Presentation colors reported by this client, never persisted as settings.
    #[serde(default)]
    pub terminal_theme: [Option<[u8; 3]>; 2],
    #[serde(default)]
    pub title_ticks: BTreeMap<u64, u16>,
    pub recovery_loading: bool,
    pub notifications: Vec<super::protocol::Notification>,
    #[serde(default)]
    pub notification_run: String,
    #[serde(default)]
    pub notification_history: bool,
    #[serde(default)]
    pub agent_sessions: Vec<crate::discovery::polling::Session>,
    #[serde(default)]
    pub agent_link_loading: bool,
    #[serde(default)]
    pub agent_link_error: Option<String>,
    pub recovery: Vec<super::protocol::RecoveryItem>,
    #[serde(default)]
    pub closed_tabs: Vec<super::history::Entry>,
    pub search_query: String,
    pub search_all: bool,
    pub search_hits: Vec<super::protocol::SearchHit>,
    pub scratch: Option<u64>,
    pub scratch_visible: bool,
    pub scratch_return: Option<u64>,
    pub scratch_reading: Option<super::protocol::ReadingMode>,
    #[serde(default)]
    pub zoom_return: Option<super::protocol::ReadingMode>,
    pub column_weights: Vec<u16>,
    pub row_weights: Vec<u16>,
    pub dialog: Option<Dialog>,
    pub help_offset: u16,
    pub config: UiConfig,
    pub unused: Vec<u64>,
    pub reading_size: Option<(u16, u16)>,
    pub grid_shape: (u8, u8),
    pub vitals: Vec<(String, u8)>,
    #[serde(default)]
    pub disk_readings: Vec<(String, u8)>,
}
fn status_descending() -> bool {
    true
}
pub fn geometry_bucket(cols: u16) -> usize {
    if cols <= 60 {
        0
    } else if cols < 160 {
        1
    } else {
        2
    }
}
