use super::protocol::*;
use crate::Result;
#[derive(Debug, Clone)]
pub struct Workspace {
    pub cols: u16,
    pub rows: u16,
    pub panes: Vec<PaneInfo>,
    pub active: PaneId,
    pub reading: ReadingMode,
    pub grid_cols: u8,
    pub grid_rows: u8,
    pub help: bool,
}
impl Workspace {
    pub fn new(cols: u16, rows: u16) -> Result<Self> {
        validate_size(cols, rows)?;
        Ok(Self {
            cols,
            rows,
            panes: vec![],
            active: 0,
            reading: ReadingMode::Grid,
            grid_cols: 1,
            grid_rows: 1,
            help: false,
        })
    }
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        validate_size(cols, rows)?;
        self.cols = cols;
        self.rows = rows;
        Ok(())
    }
    pub fn layout(&self) -> Vec<PaneView> {
        vec![]
    }
    pub fn terminal_sizes(&self) -> Vec<(PaneId, u16, u16)> {
        self.layout()
            .iter()
            .map(|p| (p.pane.id, p.body.cols, p.body.rows))
            .collect()
    }
    pub fn apply(&mut self, _action: &Action) -> Result<()> {
        Err("workspace implementation pending".into())
    }
    pub fn grid_count(cols: u8, rows: u8) -> Result<usize> {
        if ![(1, 1), (2, 1), (2, 2)].contains(&(cols, rows)) {
            return Err("MVP grids are 1x1, 2x1 and 2x2".into());
        }
        Ok(cols as usize * rows as usize)
    }
}
