use super::protocol::*;
use crate::Result;
#[derive(Debug, Clone)]
pub enum Effect {
    WritePty(Vec<u8>),
    Title(String),
    Bell,
    Clipboard(String),
}
pub struct Terminal;
impl Terminal {
    pub fn new(_cols: u16, _rows: u16) -> Self {
        Self
    }
    pub fn feed(&mut self, _bytes: &[u8]) -> Vec<Effect> {
        vec![]
    }
    pub fn resize(&mut self, _cols: u16, _rows: u16) {}
    pub fn snapshot(&self) -> Screen {
        Screen {
            cols: 0,
            rows: 0,
            cells: vec![],
            cursor: None,
            modes: InputModes::default(),
        }
    }
    pub fn scroll(&mut self, _lines: i32) {}
    pub fn select(&mut self, _start: Point, _end: Point) -> Result<String> {
        Err("terminal implementation pending".into())
    }
    pub fn selection(&self) -> Option<String> {
        None
    }
}
