//! Shared UI state and configurable prefix bindings.
use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub const FOOTER_BG: u8 = 239;
pub const TAB_BG: u8 = 236;
pub const PARKED_BG: u8 = 60;
pub const RESTORE_BG: u8 = 29;
pub const ALERT_ICON: &str = "⚠︎";
pub const CONFIG_BUTTON_LABEL: &str = " ⚙︎ Config ";
pub const FOOTER_CONFIG_COLS: u16 = 10;
pub const FOOTER_MODE_COLS: u16 = 8;
pub const HELP_ROWS: u16 = 30;
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
            legacy_read_widths: None,
        }
    }
}
impl UiConfig {
    pub fn validate(&self) -> Result<()> {
        if !(1..=26).contains(&self.prefix)
            || self
                .pane_colors
                .iter()
                .any(|(&pane, &color)| pane == 0 || usize::from(color) >= PALETTE.len())
            || !(20..=100).contains(&self.read_height_percent)
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiState {
    #[serde(default)]
    pub show_dev_badge: bool,
    #[serde(default)]
    pub tab_scroll_anchor: Option<usize>,
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
    pub agent_sessions: Vec<crate::discovery::polling::Session>,
    #[serde(default)]
    pub agent_link_loading: bool,
    #[serde(default)]
    pub agent_link_error: Option<String>,
    pub recovery: Vec<super::protocol::RecoveryItem>,
    pub search_query: String,
    pub search_all: bool,
    pub search_hits: Vec<super::protocol::SearchHit>,
    pub scratch: Option<u64>,
    pub scratch_visible: bool,
    pub scratch_return: Option<u64>,
    pub scratch_reading: Option<super::protocol::ReadingMode>,
    pub column_weights: Vec<u16>,
    pub row_weights: Vec<u16>,
    pub dialog: Option<Dialog>,
    pub help_offset: u16,
    pub config: UiConfig,
    pub unused: Vec<u64>,
    pub reading_size: Option<(u16, u16)>,
    pub grid_shape: (u8, u8),
    pub vitals: Vec<(String, u8)>,
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
