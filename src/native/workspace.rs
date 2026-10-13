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
        let columns = u16::from(self.grid_cols.max(1));
        let rows = u16::from(self.grid_rows.max(1));
        let usable_cols = self.cols.saturating_sub(columns - 1);
        let usable_rows = self.rows.saturating_sub(1);
        let mut views = Vec::new();
        for (index, pane) in self.panes.iter().enumerate() {
            let col = index as u16 % columns;
            let row = index as u16 / columns;
            let left = usable_cols * col / columns;
            let top = usable_rows * row / rows;
            let rect = Rect {
                x: left + col,
                y: top,
                cols: usable_cols * (col + 1) / columns - left,
                rows: usable_rows * (row + 1) / rows - top,
            };
            views.push(Self::view(pane, index, rect, false));
        }
        let selected = match self.reading {
            ReadingMode::Grid => None,
            ReadingMode::Capped(id) | ReadingMode::Fullscreen(id) => Some(id),
        };
        if let Some((index, pane)) =
            selected.and_then(|id| self.panes.iter().enumerate().find(|(_, p)| p.id == id))
        {
            let rect = if matches!(self.reading, ReadingMode::Fullscreen(_)) {
                Rect {
                    x: 0,
                    y: 0,
                    cols: self.cols,
                    rows: usable_rows,
                }
            } else {
                let width = self.cols.min(120);
                let height = (usable_rows * 95 / 100).max(2);
                Rect {
                    x: (self.cols - width) / 2,
                    y: (usable_rows - height) / 2,
                    cols: width,
                    rows: height,
                }
            };
            views.push(Self::view(pane, index, rect, true));
        }
        views
    }
    fn view(pane: &PaneInfo, index: usize, rect: Rect, overlay: bool) -> PaneView {
        let mut pane = pane.clone();
        pane.number = index + 1;
        PaneView {
            pane,
            rect,
            body: Rect {
                x: rect.x,
                y: rect.y + 1,
                cols: rect.cols,
                rows: rect.rows.saturating_sub(1),
            },
            overlay,
            modes: InputModes::default(),
        }
    }
    pub fn terminal_sizes(&self) -> Vec<(PaneId, u16, u16)> {
        let mut sizes = Vec::new();
        for view in self.layout() {
            let entry = (view.pane.id, view.body.cols, view.body.rows);
            if let Some(existing) = sizes
                .iter_mut()
                .find(|s: &&mut (PaneId, u16, u16)| s.0 == entry.0)
            {
                *existing = entry;
            } else {
                sizes.push(entry);
            }
        }
        sizes
    }
    fn require_pane(&self, id: PaneId) -> Result<usize> {
        self.panes
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| "unknown pane".into())
    }
    pub fn apply(&mut self, action: &Action) -> Result<()> {
        match *action {
            Action::Focus(id) => {
                self.require_pane(id)?;
                self.active = id;
                self.reading = match self.reading {
                    ReadingMode::Grid => ReadingMode::Grid,
                    ReadingMode::Capped(_) => ReadingMode::Capped(id),
                    ReadingMode::Fullscreen(_) => ReadingMode::Fullscreen(id),
                };
            }
            Action::Select(number) => {
                let id = self
                    .panes
                    .get(number.checked_sub(1).ok_or("invalid pane number")?)
                    .ok_or("invalid pane number")?
                    .id;
                self.active = id;
                self.reading = if matches!(self.reading, ReadingMode::Capped(p) | ReadingMode::Fullscreen(p) if p == id)
                {
                    ReadingMode::Grid
                } else {
                    ReadingMode::Capped(id)
                };
            }
            Action::Read(id) => {
                self.require_pane(id)?;
                self.active = id;
                self.reading = if matches!(self.reading, ReadingMode::Capped(current) | ReadingMode::Fullscreen(current) if current == id) {
                    ReadingMode::Grid
                } else {
                    ReadingMode::Capped(id)
                };
            }
            Action::Fullscreen(id) => {
                self.require_pane(id)?;
                self.active = id;
                self.reading = if self.reading == ReadingMode::Fullscreen(id) {
                    ReadingMode::Capped(id)
                } else {
                    ReadingMode::Fullscreen(id)
                };
            }
            Action::CloseReading => self.reading = ReadingMode::Grid,
            Action::Grid { cols, rows } => {
                let count = Self::grid_count(cols, rows)?;
                if self.panes.len() != count {
                    return Err("grid requires prepared pane count".into());
                }
                self.grid_cols = cols;
                self.grid_rows = rows;
                self.reading = ReadingMode::Grid;
            }
            Action::Swap { source, target } => {
                let source = self.require_pane(source)?;
                let target = self.require_pane(target)?;
                self.panes.swap(source, target);
                for (index, pane) in self.panes.iter_mut().enumerate() {
                    pane.number = index + 1;
                }
            }
            Action::Scroll { pane, .. } | Action::SelectText { pane, .. } | Action::Copy(pane) => {
                self.require_pane(pane)?;
            }
            Action::Help => self.help = !self.help,
            Action::Detach | Action::Takeover | Action::Shutdown => {}
        }
        Ok(())
    }
    pub fn grid_count(cols: u8, rows: u8) -> Result<usize> {
        if ![(1, 1), (2, 1), (2, 2)].contains(&(cols, rows)) {
            return Err("MVP grids are 1x1, 2x1 and 2x2".into());
        }
        Ok(cols as usize * rows as usize)
    }
}
