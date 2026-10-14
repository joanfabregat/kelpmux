use super::protocol::*;
use super::ui::{AgentFilter, Dialog, DialogKind, UiState};
use crate::Result;
pub const GRIDS: [(u8, u8); 6] = [(1, 1), (2, 1), (2, 2), (3, 2), (2, 3), (3, 3)];

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
    pub ui: UiState,
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
            ui: UiState {
                grid_shape: (1, 1),
                ..Default::default()
            },
        })
    }
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        validate_size(cols, rows)?;
        self.cols = cols;
        self.rows = rows;
        Ok(())
    }
    pub fn layout(&self) -> Vec<PaneView> {
        if self.reading != ReadingMode::Grid {
            let body = tab_body(
                self.cols,
                self.rows,
                &self.ui.config,
                matches!(self.reading, ReadingMode::Fullscreen(_)),
                self.ui.reading_size,
            );
            return self
                .panes
                .iter()
                .enumerate()
                .map(|(index, pane)| {
                    let mut view = Self::view(pane, index, Rect::default(), false);
                    if pane.id == self.active {
                        view.rect = body;
                        view.body = body;
                    } else {
                        view.body = Rect::default();
                    }
                    view
                })
                .collect();
        }
        self.grid_layout()
    }
    pub fn grid_layout(&self) -> Vec<PaneView> {
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
        let mut views = self.grid_layout();
        if self.reading != ReadingMode::Grid {
            views.extend(
                self.layout()
                    .into_iter()
                    .filter(|view| view.pane.id == self.active),
            );
        }
        for view in views {
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
            Action::Config => {
                self.help = false;
                self.ui.dialog = Some(Dialog {
                    kind: DialogKind::Config,
                    filter: AgentFilter::All,
                    selection: 0,
                    source: None,
                });
            }
            Action::TabPositionSet(position) => self.ui.config.tab_position = position,
            Action::ClosePaneRequest(id) => {
                self.require_pane(id)?;
                self.help = false;
                self.ui.dialog = Some(Dialog {
                    kind: DialogKind::CloseConfirm,
                    filter: AgentFilter::All,
                    selection: 0,
                    source: Some(id),
                });
            }
            Action::ClosePaneConfirm(id) => {
                self.require_pane(id)?;
            }
            Action::TabPosition => {
                use super::ui::TabPosition;
                self.ui.config.tab_position = match self.ui.config.tab_position {
                    TabPosition::Auto => TabPosition::Top,
                    TabPosition::Top => TabPosition::Left,
                    TabPosition::Left => TabPosition::Bottom,
                    TabPosition::Bottom => TabPosition::Auto,
                };
            }
            Action::GridPicker | Action::MovePicker | Action::Agents(_) => {
                let (kind, filter) = match *action {
                    Action::GridPicker => (DialogKind::Grid, AgentFilter::All),
                    Action::MovePicker => (DialogKind::Move, AgentFilter::All),
                    Action::Agents(filter) => (DialogKind::Agents, filter),
                    _ => unreachable!(),
                };
                self.help = false;
                self.ui.dialog = Some(Dialog {
                    kind,
                    filter,
                    selection: 0,
                    source: Some(self.active),
                });
            }
            Action::DialogClose => self.ui.dialog = None,
            Action::DialogNavigate { dx, dy } => {
                let count = self.dialog_entries().len();
                if let Some(dialog) = self.ui.dialog.as_mut() {
                    let columns = match dialog.kind {
                        DialogKind::Grid => 3,
                        DialogKind::Move => isize::from(self.grid_cols),
                        _ => 1,
                    };
                    dialog.selection =
                        (dialog.selection as isize + isize::from(dx) + isize::from(dy) * columns)
                            .clamp(0, count.saturating_sub(1) as isize)
                            as usize;
                }
            }
            Action::DialogPick(index) => {
                if index >= self.dialog_entries().len() {
                    return Err("invalid dialog selection".into());
                }
                if let Some(dialog) = self.ui.dialog.as_mut() {
                    dialog.selection = index;
                }
            }
            Action::FocusDirection { dx, dy } => {
                let index = self.require_pane(self.active)?;
                let columns = usize::from(self.grid_cols);
                let x = index % columns;
                let y = index / columns;
                let nx = (x as isize + isize::from(dx)).clamp(0, columns as isize - 1) as usize;
                let ny = (y as isize + isize::from(dy)).clamp(0, isize::from(self.grid_rows) - 1)
                    as usize;
                if let Some(pane) = self.panes.get(ny * columns + nx) {
                    let id = pane.id;
                    self.apply(&Action::Focus(id))?;
                }
            }
            Action::ReadResize { cols, rows } => {
                if self.reading == ReadingMode::Grid {
                    return Err("open a reading view before resizing".into());
                }
                self.ui.reading_size =
                    Some((cols.clamp(20, self.cols), rows.clamp(3, self.rows - 1)));
            }
            Action::Focus(id) => {
                self.require_pane(id)?;
                self.active = id;
                self.reading = match self.reading {
                    ReadingMode::Grid => ReadingMode::Grid,
                    ReadingMode::Tabs(_) | ReadingMode::Capped(_) => ReadingMode::Tabs(id),
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
                self.reading = if matches!(self.reading, ReadingMode::Fullscreen(_)) {
                    ReadingMode::Fullscreen(id)
                } else {
                    ReadingMode::Tabs(id)
                };
            }
            Action::Read(id) => {
                self.require_pane(id)?;
                self.active = id;
                self.reading = if matches!(self.reading, ReadingMode::Tabs(current) | ReadingMode::Capped(current) | ReadingMode::Fullscreen(current) if current == id)
                {
                    ReadingMode::Grid
                } else {
                    ReadingMode::Tabs(id)
                };
            }
            Action::Fullscreen(id) => {
                self.require_pane(id)?;
                self.active = id;
                self.reading = if self.reading == ReadingMode::Fullscreen(id) {
                    ReadingMode::Tabs(id)
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
                self.ui.grid_shape = (cols, rows);
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
            Action::Help => {
                self.help = !self.help;
                self.ui.dialog = None;
            }
            Action::Detach | Action::Takeover | Action::Shutdown => {}
            _ => return Err("phase-two command handler has not been integrated".into()),
        }
        Ok(())
    }
    pub fn grid_count(cols: u8, rows: u8) -> Result<usize> {
        if !GRIDS.contains(&(cols, rows)) {
            return Err("supported grids are 1x1, 2x1, 2x2, 3x2, 2x3 and 3x3".into());
        }
        Ok(cols as usize * rows as usize)
    }
    pub fn dialog_entries(&self) -> Vec<PaneId> {
        dialog_entries(&self.panes, self.ui.dialog)
    }
}
pub fn dialog_entries(panes: &[PaneInfo], dialog: Option<Dialog>) -> Vec<PaneId> {
    match dialog {
        Some(Dialog {
            kind: DialogKind::Config,
            ..
        }) => (0..4).collect(),
        Some(Dialog {
            kind: DialogKind::CloseConfirm,
            ..
        }) => (0..2).collect(),
        Some(Dialog {
            kind: DialogKind::Grid,
            ..
        }) => (0..6).collect(),
        Some(Dialog {
            kind: DialogKind::Agents,
            filter,
            ..
        }) => panes
            .iter()
            .filter(|p| p.kind.is_some())
            .filter(|p| match filter {
                AgentFilter::All => true,
                AgentFilter::Attention => matches!(
                    p.status,
                    crate::model::Status::InputNeeded | crate::model::Status::Failed
                ),
                AgentFilter::Finished => matches!(
                    p.status,
                    crate::model::Status::TurnFinished | crate::model::Status::Exited
                ),
            })
            .map(|p| p.id)
            .collect(),
        _ => panes.iter().map(|p| p.id).collect(),
    }
}
pub fn dialog_bounds(cols: u16, rows: u16) -> Rect {
    let width = cols.saturating_sub(4).min(96);
    let height = rows.saturating_sub(2).min(18);
    Rect {
        x: (cols - width) / 2,
        y: (rows - 1 - height) / 2,
        cols: width,
        rows: height,
    }
}
pub fn grid_cards(bounds: Rect) -> Vec<Rect> {
    let width = bounds.cols / 3;
    let height = bounds.rows.saturating_sub(2) / 2;
    (0..6)
        .map(|i| Rect {
            x: bounds.x + (i % 3) * width,
            y: bounds.y + 2 + (i / 3) * height,
            cols: width,
            rows: height,
        })
        .collect()
}
pub fn move_cards(bounds: Rect, columns: u8, rows: u8) -> Vec<Rect> {
    let usable = bounds.rows.saturating_sub(2);
    let columns = u16::from(columns.max(1));
    let rows = u16::from(rows.max(1));
    (0..columns * rows)
        .map(|index| {
            let x = index % columns;
            let y = index / columns;
            Rect {
                x: bounds.x + bounds.cols * x / columns,
                y: bounds.y + 2 + usable * y / rows,
                cols: bounds.cols * (x + 1) / columns - bounds.cols * x / columns,
                rows: usable * (y + 1) / rows - usable * y / rows,
            }
        })
        .collect()
}
/// Content is a single terminal with blank margins, never a floating pane.
pub fn tab_body(
    cols: u16,
    rows: u16,
    config: &super::ui::UiConfig,
    full: bool,
    size: Option<(u16, u16)>,
) -> Rect {
    use super::ui::TabPosition;
    let position = config.tab_position.resolve(cols, rows);
    let mut area = Rect {
        x: 0,
        y: 0,
        cols,
        rows: rows.saturating_sub(1),
    };
    match position {
        TabPosition::Top => {
            area.y = 1;
            area.rows = area.rows.saturating_sub(1);
        }
        TabPosition::Left => {
            let sidebar = (cols / 5).clamp(18, 28);
            area.x = sidebar;
            area.cols = cols - sidebar;
        }
        TabPosition::Bottom => area.rows = area.rows.saturating_sub(1),
        TabPosition::Auto => unreachable!(),
    }
    let width = if full {
        area.cols
    } else {
        size.map(|s| s.0)
            .unwrap_or(config.read_width)
            .clamp(20, area.cols)
    };
    Rect {
        x: area.x + (area.cols - width) / 2,
        cols: width,
        ..area
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabTarget {
    Pane(usize),
    Previous,
    Next,
}
pub fn tab_targets(
    cols: u16,
    rows: u16,
    config: &super::ui::UiConfig,
    count: usize,
    active: usize,
) -> Vec<(TabTarget, Rect)> {
    use super::ui::TabPosition;
    if count == 0 {
        return vec![];
    }
    let position = config.tab_position.resolve(cols, rows);
    match position {
        TabPosition::Bottom => vec![
            (
                TabTarget::Previous,
                Rect {
                    x: 0,
                    y: rows - 2,
                    cols: 3,
                    rows: 1,
                },
            ),
            (
                TabTarget::Pane(active),
                Rect {
                    x: 3,
                    y: rows - 2,
                    cols: cols - 6,
                    rows: 1,
                },
            ),
            (
                TabTarget::Next,
                Rect {
                    x: cols - 3,
                    y: rows - 2,
                    cols: 3,
                    rows: 1,
                },
            ),
        ],
        TabPosition::Top => {
            let capacity = usize::from(cols.saturating_sub(6) / 12).max(1).min(count);
            let start = active.saturating_sub(capacity - 1);
            let width = (cols - 6) / capacity as u16;
            let mut tabs: Vec<_> = (start..(start + capacity).min(count))
                .enumerate()
                .map(|(i, index)| {
                    (
                        TabTarget::Pane(index),
                        Rect {
                            x: 3 + i as u16 * width,
                            y: 0,
                            cols: width,
                            rows: 1,
                        },
                    )
                })
                .collect();
            tabs.push((
                TabTarget::Previous,
                Rect {
                    x: 0,
                    y: 0,
                    cols: 3,
                    rows: 1,
                },
            ));
            tabs.push((
                TabTarget::Next,
                Rect {
                    x: cols - 3,
                    y: 0,
                    cols: 3,
                    rows: 1,
                },
            ));
            tabs
        }
        TabPosition::Left => {
            let width = (cols / 5).clamp(18, 28);
            let capacity = usize::from(rows.saturating_sub(3)).max(1);
            let start = active.saturating_sub(capacity - 1);
            let mut tabs: Vec<_> = (start..(start + capacity).min(count))
                .enumerate()
                .map(|(i, index)| {
                    (
                        TabTarget::Pane(index),
                        Rect {
                            x: 0,
                            y: 1 + i as u16,
                            cols: width,
                            rows: 1,
                        },
                    )
                })
                .collect();
            tabs.push((
                TabTarget::Previous,
                Rect {
                    x: 0,
                    y: 0,
                    cols: width,
                    rows: 1,
                },
            ));
            tabs.push((
                TabTarget::Next,
                Rect {
                    x: 0,
                    y: rows - 2,
                    cols: width,
                    rows: 1,
                },
            ));
            tabs
        }
        TabPosition::Auto => unreachable!(),
    }
}
