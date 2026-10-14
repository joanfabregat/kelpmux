//! Shared UI state and configurable prefix bindings.
use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub const FOOTER_COLS: u16 = 27;
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabPosition {
    #[default]
    Auto,
    Top,
    Left,
    Bottom,
}
impl TabPosition {
    pub fn resolve(self, cols: u16, rows: u16) -> Self {
        match self {
            Self::Auto if cols <= 60 => Self::Bottom,
            Self::Auto if cols >= 160 && cols >= rows.saturating_mul(3) => Self::Top,
            Self::Auto if cols >= 90 => Self::Left,
            Self::Auto => Self::Top,
            _ => self,
        }
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
    Overview,
    Notifications,
    Recovery,
    Search,
    Grid,
    Agents,
    Move,
    Config,
    CloseConfirm,
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
    pub notification_sound:bool,
    pub prefix: u8,
    pub read_width: u16,
    pub read_height_percent: u8,
    pub bindings: BTreeMap<String, String>,
    pub tab_position: TabPosition,
}
impl Default for UiConfig {
    fn default() -> Self {
        Self {
            notification_sound:false,
            prefix: 2,
            read_width: 120,
            read_height_percent: 95,
            bindings: BTreeMap::new(),
            tab_position: TabPosition::Auto,
        }
    }
}
impl UiConfig {
    pub fn validate(&self) -> Result<()> {
        if !(1..=26).contains(&self.prefix)
            || !(20..=400).contains(&self.read_width)
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
            "overview", "notifications", "recovery", "next-attention", "next-finished", "scratch", "search",
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
    pub notifications:Vec<super::protocol::Notification>,
    pub recovery:Vec<super::protocol::RecoveryItem>,
    pub search_query:String,
    pub search_all:bool,
    pub search_hits:Vec<super::protocol::SearchHit>,
    pub scratch:Option<u64>,
    pub scratch_visible:bool,
    pub scratch_return:Option<u64>,
    pub column_weights:Vec<u16>,
    pub row_weights:Vec<u16>,
    pub dialog: Option<Dialog>,
    pub config: UiConfig,
    pub unused: Vec<u64>,
    pub reading_size: Option<(u16, u16)>,
    pub grid_shape: (u8, u8),
    pub vitals: Vec<(String, u8)>,
}
