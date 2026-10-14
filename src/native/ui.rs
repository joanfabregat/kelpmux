//! Shared UI state and configurable prefix bindings.
use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentFilter {
    #[default]
    All,
    Attention,
    Finished,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialogKind {
    Grid,
    Agents,
    Move,
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
    pub prefix: u8,
    pub read_width: u16,
    pub read_height_percent: u8,
    pub bindings: BTreeMap<String, String>,
}
impl Default for UiConfig {
    fn default() -> Self {
        Self {
            prefix: 2,
            read_width: 120,
            read_height_percent: 95,
            bindings: BTreeMap::new(),
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
    pub dialog: Option<Dialog>,
    pub config: UiConfig,
    pub unused: Vec<u64>,
    pub reading_size: Option<(u16, u16)>,
}
