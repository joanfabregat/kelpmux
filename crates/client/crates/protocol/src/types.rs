use crate::model::Status;
use serde::{Deserialize, Serialize};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underline_color: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hyperlink: Option<String>,
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
            underline_color: None,
            hyperlink: None,
        }
    }
}

/// OSC 8 parameters must never contain terminal controls or unbounded payloads.
pub fn valid_hyperlink(uri: &str) -> bool {
    if uri.len() > 2048 || uri.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    uri.split_once(':').is_some_and(|(scheme, rest)| {
        !rest.is_empty()
            && scheme
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b))
    })
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
    #[serde(default)]
    pub keyboard_flags: u8,
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
    #[serde(default)]
    pub parked: bool,
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
