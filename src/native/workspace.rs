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
        let old_bucket = super::ui::geometry_bucket(self.cols);
        let prior = self
            .ui
            .scratch_reading
            .filter(|_| self.ui.scratch_visible)
            .unwrap_or(self.reading);
        if old_bucket != super::ui::geometry_bucket(cols)
            && self.ui.config.view_modes[old_bucket].is_none()
        {
            self.ui.config.view_modes[old_bucket] = Some(prior != ReadingMode::Grid);
        }
        self.cols = cols;
        self.rows = rows;
        let bucket = super::ui::geometry_bucket(cols);
        let tabs =
            self.ui.config.view_modes[bucket].unwrap_or(bucket == 0 || prior != ReadingMode::Grid);
        let normal = self
            .ui
            .scratch_return
            .filter(|_| self.ui.scratch_visible)
            .unwrap_or(self.active);
        if self
            .panes
            .iter()
            .any(|pane| pane.id == normal && Some(normal) != self.ui.scratch)
        {
            let previous = self
                .ui
                .scratch_reading
                .filter(|_| self.ui.scratch_visible)
                .unwrap_or(self.reading);
            let reading = if tabs {
                if matches!(previous, ReadingMode::Fullscreen(_)) {
                    ReadingMode::Fullscreen(normal)
                } else {
                    ReadingMode::Tabs(normal)
                }
            } else {
                ReadingMode::Grid
            };
            if self.ui.scratch_visible {
                self.ui.scratch_reading = Some(reading);
                if self.reading != ReadingMode::Fullscreen(self.ui.scratch.unwrap_or(0)) {
                    self.reading = reading;
                }
            } else {
                self.reading = reading;
            }
        }
        self.reflow();
        Ok(())
    }
    pub fn layout(&self) -> Vec<PaneView> {
        if let Some(scratch) = self.ui.scratch {
            let mut base = self.clone();
            base.panes.retain(|pane| pane.id != scratch);
            base.ui.scratch = None;
            if self.ui.scratch_visible {
                base.active = self.ui.scratch_return.unwrap_or(base.active);
                base.reading = self.ui.scratch_reading.unwrap_or(ReadingMode::Grid);
            }
            let mut views = base.layout();
            if let Some(pane) = self.panes.iter().find(|pane| pane.id == scratch) {
                let rect = if self.ui.scratch_visible {
                    if self.reading == ReadingMode::Fullscreen(scratch) {
                        Rect {
                            x: 0,
                            y: 0,
                            cols: self.cols,
                            rows: self.rows - 1,
                        }
                    } else {
                        scratch_rect(self.cols, self.rows)
                    }
                } else {
                    Rect::default()
                };
                let mut view = Self::view(pane, views.len(), rect, self.ui.scratch_visible);
                view.pane.number = 0;
                if rect.rows == 0 {
                    view.body = Rect::default();
                }
                views.push(view);
            }
            return views;
        }
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
                    let number = self
                        .panes
                        .iter()
                        .take(index)
                        .filter(|p| !p.parked && Some(p.id) != self.ui.scratch)
                        .count();
                    let mut view = Self::view(pane, number, Rect::default(), false);
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
        let (_, grid_rows) = self.automatic_shape();
        // Each row's pane header separates it from the preceding terminal.
        let usable_rows = self.rows.saturating_sub(1);
        let mut views = Vec::new();
        let y_boundaries = weighted_boundaries(usable_rows, grid_rows, &[]);
        let count = self
            .panes
            .iter()
            .filter(|p| !p.parked && Some(p.id) != self.ui.scratch)
            .count();
        let mut visible = 0;
        for (index, pane) in self
            .panes
            .iter()
            .filter(|pane| Some(pane.id) != self.ui.scratch)
            .enumerate()
        {
            if pane.parked {
                let mut view = Self::view(pane, index, Rect::default(), false);
                view.body = Rect::default();
                views.push(view);
                continue;
            }
            let index = visible;
            visible += 1;
            let mut row = 0usize;
            let mut offset = 0usize;
            let row_count = loop {
                let length = count / usize::from(grid_rows)
                    + usize::from(row < count % usize::from(grid_rows));
                if index < offset + length {
                    break length;
                }
                offset += length;
                row += 1;
            };
            let col = (index - offset) as u16;
            let row = row as u16;
            let x_boundaries =
                weighted_boundaries(self.cols - (row_count as u16 - 1), row_count as u8, &[]);
            let left = x_boundaries[usize::from(col)];
            let top = y_boundaries[usize::from(row)];
            let rect = Rect {
                x: left + col,
                y: top,
                cols: x_boundaries[usize::from(col + 1)] - left,
                rows: y_boundaries[usize::from(row + 1)] - top,
            };
            views.push(Self::view(pane, index, rect, false));
        }
        views
    }
    pub fn automatic_shape(&self) -> (u8, u8) {
        let count = self
            .panes
            .iter()
            .filter(|p| !p.parked && Some(p.id) != self.ui.scratch)
            .count();
        match count {
            0 | 1 => (1, 1),
            2 if self.cols < 70 => (1, 2),
            2 => (2, 1),
            3 | 4 => (2, 2),
            5 | 6 if self.cols < 100 => (2, 3),
            5 | 6 => (3, 2),
            _ => (3, 3),
        }
    }
    pub fn reflow(&mut self) {
        (self.grid_cols, self.grid_rows) = self.automatic_shape();
        self.ui.grid_shape = (self.grid_cols, self.grid_rows);
        self.ui.column_weights.clear();
        self.ui.row_weights.clear();
        let mut number = 0;
        for pane in &mut self.panes {
            pane.number = if pane.parked || Some(pane.id) == self.ui.scratch {
                0
            } else {
                number += 1;
                number
            };
        }
    }
    fn view(pane: &PaneInfo, index: usize, rect: Rect, overlay: bool) -> PaneView {
        let mut pane = pane.clone();
        pane.number = if pane.parked { 0 } else { index + 1 };
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
        if self.reading != ReadingMode::Grid || self.ui.scratch_visible {
            views.extend(self.layout().into_iter().filter(|view| view.body.cols > 0));
        }
        for view in views {
            if view.body.cols == 0 || view.body.rows == 0 {
                continue;
            }
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
        if let Some(id) = self.ui.scratch {
            if !sizes.iter().any(|entry| entry.0 == id) {
                let rect = scratch_rect(self.cols, self.rows);
                sizes.push((id, rect.cols, rect.rows - 1));
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
            Action::Parked => {
                self.help = false;
                self.ui.dialog = Some(Dialog {
                    kind: DialogKind::Parked,
                    selection: 0,
                    source: None,
                    filter: AgentFilter::All,
                });
            }
            Action::ParkRequest(id) | Action::RestoreRequest(id) => {
                let index = self.require_pane(id)?;
                let restore = matches!(action, Action::RestoreRequest(_));
                if Some(id) == self.ui.scratch || self.panes[index].parked != restore {
                    return Err("tab is not available for this action".into());
                }
                self.help = false;
                self.ui.dialog = Some(Dialog {
                    kind: if restore {
                        DialogKind::RestoreConfirm
                    } else {
                        DialogKind::ParkConfirm
                    },
                    selection: 1,
                    source: Some(id),
                    filter: AgentFilter::All,
                });
            }
            Action::Park(id) => {
                let index = self.require_pane(id)?;
                if Some(id) == self.ui.scratch {
                    return Err("scratch cannot be parked".into());
                }
                if self.ui.scratch_visible {
                    self.apply(&Action::ScratchClose)?;
                }
                self.panes[index].parked = true;
                if self.active == id {
                    self.active = self
                        .panes
                        .iter()
                        .find(|p| !p.parked && Some(p.id) != self.ui.scratch)
                        .map(|p| p.id)
                        .unwrap_or(0);
                    if self.reading != ReadingMode::Grid {
                        self.reading = if self.active == 0 {
                            ReadingMode::Grid
                        } else {
                            ReadingMode::Tabs(self.active)
                        };
                    }
                }
                self.ui.dialog = None;
                self.reflow();
            }
            Action::Restore(id) => {
                let index = self.require_pane(id)?;
                self.panes[index].parked = false;
                self.active = id;
                self.reading = ReadingMode::Tabs(id);
                self.ui.dialog = None;
                self.reflow();
            }
            Action::ClaimPane(_) => {}
            Action::NewTab => return Err("new tabs require the PTY server".into()),
            Action::HelpScroll(amount) => {
                let visible = self.rows.saturating_sub(2).min(30).saturating_sub(3);
                self.ui.help_offset = self
                    .ui
                    .help_offset
                    .saturating_add_signed(amount)
                    .min(super::ui::HELP_ROWS.saturating_sub(visible));
            }
            Action::Overview | Action::Notifications | Action::Recovery | Action::SearchOpen => {
                let kind = match action {
                    Action::Overview => DialogKind::Overview,
                    Action::Notifications => DialogKind::Notifications,
                    Action::Recovery => DialogKind::Recovery,
                    _ => DialogKind::Search,
                };
                self.help = false;
                self.ui.dialog = Some(Dialog {
                    kind,
                    selection: 0,
                    filter: AgentFilter::All,
                    source: Some(if Some(self.active) == self.ui.scratch {
                        self.ui.scratch_return.unwrap_or(self.active)
                    } else {
                        self.active
                    }),
                });
            }
            Action::NextAttention | Action::NextFinished => {
                let panes: Vec<_> = self
                    .panes
                    .iter()
                    .filter(|p| Some(p.id) != self.ui.scratch)
                    .collect();
                let start = panes.iter().position(|p| p.id == self.active).unwrap_or(0);
                let target = (1..=panes.len())
                    .map(|offset| panes[(start + offset) % panes.len()])
                    .find(|p| {
                        if matches!(action, Action::NextAttention) {
                            matches!(
                                p.status,
                                crate::model::Status::InputNeeded | crate::model::Status::Failed
                            )
                        } else {
                            matches!(
                                p.status,
                                crate::model::Status::TurnFinished | crate::model::Status::Exited
                            )
                        }
                    })
                    .map(|p| p.id);
                if let Some(id) = target {
                    self.apply(&Action::Focus(id))?;
                } else {
                    return Err("no matching agent".into());
                }
            }
            Action::Acknowledge(id) => {
                if let Some(notification) =
                    self.ui.notifications.iter_mut().find(|item| item.id == id)
                {
                    notification.read = true;
                }
            }
            Action::NotificationSoundToggle => {
                self.ui.config.notification_sound = !self.ui.config.notification_sound
            }
            Action::RecoveryCopy(index) => {
                self.ui
                    .recovery
                    .get(index)
                    .ok_or("invalid recovery entry")?;
            }
            Action::SearchCopy(index) => {
                self.ui
                    .search_hits
                    .get(index)
                    .ok_or("invalid search result")?;
            }
            Action::Scratch => {
                if self.ui.scratch_visible {
                    self.apply(&Action::ScratchClose)?;
                } else {
                    let id = self
                        .ui
                        .scratch
                        .ok_or("scratch terminal has not been created")?;
                    self.require_pane(id)?;
                    self.ui.scratch_return = Some(self.active);
                    self.ui.scratch_reading = Some(self.reading);
                    self.ui.scratch_visible = true;
                    self.active = id;
                    self.ui.dialog = None;
                }
            }
            Action::ScratchClose => {
                if self.ui.scratch_visible {
                    self.active = self
                        .ui
                        .scratch_return
                        .filter(|id| self.panes.iter().any(|p| p.id == *id && !p.parked))
                        .or_else(|| {
                            self.panes
                                .iter()
                                .find(|p| !p.parked && Some(p.id) != self.ui.scratch)
                                .map(|p| p.id)
                        })
                        .unwrap_or(0);
                    self.reading = self.ui.scratch_reading.take().unwrap_or(ReadingMode::Grid);
                    self.ui.scratch_visible = false;
                    self.ui.scratch_return = None;
                }
            }
            Action::Search { ref query, all } => {
                self.ui.search_query = query
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(1024)
                    .collect();
                self.ui.search_all = all;
            }
            Action::SearchSelect(index) => {
                let pane = self
                    .ui
                    .search_hits
                    .get(index)
                    .ok_or("invalid search result")?
                    .pane;
                self.apply(&Action::Focus(pane))?;
            }
            Action::ResizeGrid { column, row } => {
                if self.reading != ReadingMode::Grid {
                    return Err("grid resizing requires grid view".into());
                }
                let mut columns = self.ui.column_weights.clone();
                let mut rows = self.ui.row_weights.clone();
                if let Some((index, value)) = column {
                    set_boundary(&mut columns, self.grid_cols, index, value)?;
                }
                if let Some((index, value)) = row {
                    set_boundary(&mut rows, self.grid_rows, index, value)?;
                }
                self.ui.column_weights = columns;
                self.ui.row_weights = rows;
            }
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
            Action::TabColor(id) => {
                let index = self.require_pane(id)?;
                self.help = false;
                self.ui.dialog = Some(Dialog {
                    kind: DialogKind::Color,
                    selection: usize::from(self.panes[index].palette),
                    source: Some(id),
                    filter: AgentFilter::All,
                });
            }
            Action::TabColorSet { pane, color } => {
                let index = self.require_pane(pane)?;
                if usize::from(color) >= super::ui::PALETTE.len() {
                    return Err("unknown tab color".into());
                }
                self.panes[index].palette = color;
                self.ui.config.pane_colors.insert(pane, color);
            }
            Action::WrapToggle => {
                self.ui.config.wrap_lines = !(self.ui.config.wrap_lines
                    && !matches!(self.reading, ReadingMode::Fullscreen(_)));
                if self.reading != ReadingMode::Grid {
                    self.reading = ReadingMode::Tabs(self.active);
                }
            }
            Action::ClosePaneRequest(id) => {
                self.require_pane(id)?;
                self.help = false;
                self.ui.dialog = Some(Dialog {
                    kind: DialogKind::CloseConfirm,
                    filter: AgentFilter::All,
                    selection: 1,
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
                    source: Some(if Some(self.active) == self.ui.scratch {
                        self.ui.scratch_return.unwrap_or(self.active)
                    } else {
                        self.active
                    }),
                });
            }
            Action::DialogClose => self.ui.dialog = None,
            Action::DialogNavigate { dx, dy } => {
                let count = self.dialog_entries().len();
                if let Some(dialog) = self.ui.dialog.as_mut() {
                    if dialog.kind == DialogKind::Config {
                        dialog.selection = match (dialog.selection, dx, dy) {
                            (0..=3, _, dy) if dy > 0 => 4,
                            (4, _, dy) if dy > 0 => 5,
                            (5, _, dy) if dy < 0 => 4,
                            (4, _, dy) if dy < 0 => match self.ui.config.tab_position {
                                super::ui::TabPosition::Auto => 0,
                                super::ui::TabPosition::Top => 1,
                                super::ui::TabPosition::Left => 2,
                                super::ui::TabPosition::Bottom => 3,
                            },
                            (index @ 0..=3, dx, _) => {
                                (index as isize + isize::from(dx)).clamp(0, 3) as usize
                            }
                            (index, _, _) => index,
                        };
                        return Ok(());
                    }
                    let columns = match dialog.kind {
                        DialogKind::Grid | DialogKind::Color => 3,
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
                let normal = self
                    .ui
                    .scratch_return
                    .filter(|_| self.ui.scratch_visible)
                    .unwrap_or(self.active);
                self.require_pane(normal)?;
                let views = self.grid_layout();
                let origin = views
                    .iter()
                    .find(|view| view.pane.id == normal)
                    .unwrap()
                    .rect;
                let center = |r: Rect| {
                    (
                        i32::from(r.x) * 2 + i32::from(r.cols),
                        i32::from(r.y) * 2 + i32::from(r.rows),
                    )
                };
                let (x, y) = center(origin);
                if let Some(view) = views
                    .iter()
                    .filter(|v| !v.pane.parked && v.pane.id != normal)
                    .filter(|v| {
                        let (nx, ny) = center(v.rect);
                        if dx != 0 {
                            (nx - x) * i32::from(dx) > 0 && v.rect.y == origin.y
                        } else {
                            (ny - y) * i32::from(dy) > 0
                        }
                    })
                    .min_by_key(|v| {
                        let (nx, ny) = center(v.rect);
                        (nx - x).abs() + (ny - y).abs() * 4
                    })
                {
                    let id = view.pane.id;
                    self.apply(&Action::Focus(id))?;
                }
            }
            Action::ReadResize { cols, rows } => {
                if self.reading == ReadingMode::Grid {
                    return Err("open a reading view before resizing".into());
                }
                self.ui.reading_size =
                    Some((cols.clamp(20, self.cols), rows.clamp(3, self.rows - 1)));
                self.ui.config.remembered_read_widths[super::ui::geometry_bucket(self.cols)] =
                    Some(cols.clamp(20, self.cols));
            }
            Action::Focus(id) => {
                let index = self.require_pane(id)?;
                if self.panes[index].parked {
                    return self.apply(&Action::RestoreRequest(id));
                }
                self.require_pane(id)?;
                if Some(id) == self.ui.scratch && !self.ui.scratch_visible {
                    return self.apply(&Action::Scratch);
                }
                if self.ui.scratch_visible && Some(id) != self.ui.scratch {
                    self.apply(&Action::ScratchClose)?;
                }
                self.active = id;
                self.reading = match self.reading {
                    ReadingMode::Grid => ReadingMode::Grid,
                    ReadingMode::Tabs(_) | ReadingMode::Capped(_) => ReadingMode::Tabs(id),
                    ReadingMode::Fullscreen(_) => ReadingMode::Fullscreen(id),
                };
            }
            Action::Select(number) => {
                if self.ui.scratch_visible {
                    self.apply(&Action::ScratchClose)?;
                }
                let id = self
                    .panes
                    .iter()
                    .filter(|pane| !pane.parked && Some(pane.id) != self.ui.scratch)
                    .nth(number.checked_sub(1).ok_or("invalid pane number")?)
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
                let index = self.require_pane(id)?;
                if self.panes[index].parked {
                    return self.apply(&Action::RestoreRequest(id));
                }
                if Some(id) == self.ui.scratch {
                    return self.apply(&Action::ScratchClose);
                }
                if self.ui.scratch_visible {
                    self.apply(&Action::ScratchClose)?;
                }
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
                let index = self.require_pane(id)?;
                if self.panes[index].parked {
                    return self.apply(&Action::RestoreRequest(id));
                }
                if self.ui.scratch_visible && Some(id) != self.ui.scratch {
                    self.apply(&Action::ScratchClose)?;
                }
                if self.ui.scratch_visible && Some(id) == self.ui.scratch {
                    self.reading = if self.reading == ReadingMode::Fullscreen(id) {
                        self.ui.scratch_reading.unwrap_or(ReadingMode::Grid)
                    } else {
                        ReadingMode::Fullscreen(id)
                    };
                    return Ok(());
                }
                self.require_pane(id)?;
                self.active = id;
                self.reading = if self.reading == ReadingMode::Fullscreen(id) {
                    ReadingMode::Tabs(id)
                } else {
                    ReadingMode::Fullscreen(id)
                };
            }
            Action::CloseReading => {
                if self.ui.scratch_visible {
                    self.apply(&Action::ScratchClose)?;
                }
                self.reading = ReadingMode::Grid;
            }
            Action::Grid { cols, rows } => {
                let count = Self::grid_count(cols, rows)?;
                if self
                    .panes
                    .iter()
                    .filter(|pane| Some(pane.id) != self.ui.scratch)
                    .count()
                    != count
                {
                    return Err("grid requires prepared pane count".into());
                }
                self.grid_cols = cols;
                self.grid_rows = rows;
                self.ui.grid_shape = (cols, rows);
                self.ui.column_weights.clear();
                self.ui.row_weights.clear();
                self.reading = ReadingMode::Grid;
            }
            Action::Swap { source, target } => {
                if Some(source) == self.ui.scratch || Some(target) == self.ui.scratch {
                    return Err("scratch terminal is outside the grid".into());
                }
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
                self.ui.help_offset = 0;
                self.ui.dialog = None;
            }
            Action::Detach | Action::Takeover | Action::Shutdown => {}
            Action::SelectionStart | Action::SelectionMove { .. } => {
                self.require_pane(self.active)?;
            }
        }
        if matches!(
            action,
            Action::Read(_)
                | Action::Fullscreen(_)
                | Action::Select(_)
                | Action::CloseReading
                | Action::Grid { .. }
        ) {
            self.ui.config.view_modes[super::ui::geometry_bucket(self.cols)] =
                Some(self.reading != ReadingMode::Grid);
            if self.ui.scratch_visible {
                self.ui.scratch_reading = Some(self.reading);
            }
        }
        Ok(())
    }
    pub fn grid_count(cols: u8, rows: u8) -> Result<usize> {
        if !GRIDS.contains(&(cols, rows)) && (cols, rows) != (1, 2) {
            return Err("supported grids are 1x1, 2x1, 2x2, 3x2, 2x3 and 3x3".into());
        }
        Ok(cols as usize * rows as usize)
    }
    pub fn dialog_entries(&self) -> Vec<PaneId> {
        match self.ui.dialog.map(|dialog| dialog.kind) {
            Some(DialogKind::Notifications) => {
                return self.ui.notifications.iter().map(|item| item.id).collect();
            }
            Some(DialogKind::Recovery) => return (0..self.ui.recovery.len() as u64).collect(),
            Some(DialogKind::Search) => return (0..self.ui.search_hits.len() as u64).collect(),
            _ => {}
        }
        let panes = self
            .panes
            .iter()
            .filter(|p| Some(p.id) != self.ui.scratch)
            .cloned()
            .collect::<Vec<_>>();
        dialog_entries(&panes, self.ui.dialog)
    }
}
pub fn dialog_entries(panes: &[PaneInfo], dialog: Option<Dialog>) -> Vec<PaneId> {
    match dialog {
        Some(Dialog {
            kind: DialogKind::Parked,
            ..
        }) => panes.iter().filter(|p| p.parked).map(|p| p.id).collect(),
        Some(Dialog {
            kind: DialogKind::Color,
            ..
        }) => (0..9).collect(),
        Some(Dialog {
            kind: DialogKind::Config,
            ..
        }) => (0..6).collect(),
        Some(Dialog {
            kind: DialogKind::CloseConfirm | DialogKind::ParkConfirm | DialogKind::RestoreConfirm,
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
pub fn dialog_bounds_for(cols: u16, rows: u16, kind: super::ui::DialogKind) -> Rect {
    if !matches!(
        kind,
        super::ui::DialogKind::CloseConfirm
            | super::ui::DialogKind::ParkConfirm
            | super::ui::DialogKind::RestoreConfirm
            | super::ui::DialogKind::Config
    ) {
        return dialog_bounds(cols, rows);
    }
    let (width, height) = if kind == super::ui::DialogKind::Config {
        (72, 19)
    } else {
        (52, 9)
    };
    let width = cols.saturating_sub(4).min(width);
    let height = rows.saturating_sub(2).min(height);
    Rect {
        x: (cols - width) / 2,
        y: (rows - 1 - height) / 2,
        cols: width,
        rows: height,
    }
}
pub fn config_geometry(bounds: Rect) -> (u16, u16) {
    if bounds.rows >= 18 {
        (6, 3)
    } else if bounds.rows >= 15 {
        (5, 2)
    } else if bounds.rows >= 12 {
        (4, 1)
    } else {
        (2, 1)
    }
}
/// Logical focus index, optional explicit on/off value, and shared mouse geometry.
pub fn config_targets(bounds: Rect) -> Vec<(usize, Option<bool>, Rect)> {
    let (step, height) = config_geometry(bounds);
    let mut choices = Vec::new();
    let inner = bounds.cols - 4;
    for index in 0..4u16 {
        choices.push((
            usize::from(index),
            None,
            Rect {
                x: bounds.x + 2 + (inner - 3) * index / 4 + index,
                y: bounds.y + 2,
                cols: (inner - 3) * (index + 1) / 4 - (inner - 3) * index / 4,
                rows: height,
            },
        ));
    }
    for group in 0..2u16 {
        for value in 0..2u16 {
            choices.push((
                usize::from(group) + 4,
                Some(value == 0),
                Rect {
                    x: bounds.x + 2 + inner / 2 + (inner - inner / 2 - 1) * value / 2 + value,
                    y: bounds.y + 2 + (group + 1) * step,
                    cols: (inner - inner / 2 - 1) * (value + 1) / 2
                        - (inner - inner / 2 - 1) * value / 2,
                    rows: height,
                },
            ));
        }
    }
    choices
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
/// Terminal content occupies every column except visible UI dividers.
pub fn tab_body(
    cols: u16,
    rows: u16,
    config: &super::ui::UiConfig,
    full: bool,
    _size: Option<(u16, u16)>,
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
            area.y = 3;
            area.rows = area.rows.saturating_sub(3);
        }
        TabPosition::Left => {
            let sidebar = (cols / 5).clamp(18, 28);
            area.x = sidebar;
            area.cols = cols - sidebar;
        }
        TabPosition::Bottom => area.rows = area.rows.saturating_sub(3),
        TabPosition::Auto => unreachable!(),
    }
    let cap = config.remembered_read_widths[super::ui::geometry_bucket(cols)]
        .unwrap_or(config.read_width);
    // A narrow leftover strip is not useful on tablets and small terminals.
    // Count usable terminal columns after the sidebar, before the divider.
    let effective = area.cols.saturating_sub(cap) >= 20;
    if effective {
        area.cols = area.cols.saturating_sub(1);
    }
    let width = if full || !config.wrap_lines || !effective {
        area.cols
    } else {
        config.remembered_read_widths[super::ui::geometry_bucket(cols)]
            .unwrap_or(config.read_width)
            .clamp(20.min(area.cols), area.cols)
    };
    Rect {
        x: area.x,
        cols: width,
        ..area
    }
}
pub fn scratch_rect(cols: u16, rows: u16) -> Rect {
    let width = cols.saturating_sub(4).min(120);
    let height = rows.saturating_sub(4).min(30);
    Rect {
        x: (cols - width) / 2,
        y: (rows - 1 - height) / 2,
        cols: width,
        rows: height,
    }
}
pub fn weighted_boundaries(extent: u16, count: u8, weights: &[u16]) -> Vec<u16> {
    let count = usize::from(count.max(1));
    let min = if usize::from(extent) >= count * 2 {
        2
    } else {
        1
    };
    let spare = usize::from(extent).saturating_sub(count * min);
    let weights: Vec<usize> = if weights.len() == count && weights.iter().all(|w| *w > 0) {
        weights.iter().map(|w| usize::from(*w)).collect()
    } else {
        vec![1; count]
    };
    let total = weights.iter().sum::<usize>();
    let mut sum = 0;
    let mut output = vec![0];
    for (index, weight) in weights.iter().enumerate() {
        sum += weight;
        output.push(((index + 1) * min + spare * sum / total) as u16);
    }
    output
}
fn set_boundary(weights: &mut Vec<u16>, count: u8, index: u8, value: u16) -> Result<()> {
    let count = usize::from(count);
    let index = usize::from(index);
    if index + 1 >= count {
        return Err("invalid grid boundary".into());
    }
    if weights.len() != count {
        *weights = vec![1000 / count as u16; count];
    }
    let before = weights[..index].iter().copied().sum::<u16>();
    let pair = weights[index] + weights[index + 1];
    let target = value.clamp(before + 50, before + pair - 50);
    weights[index] = target - before;
    weights[index + 1] = pair - weights[index];
    Ok(())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabTarget {
    Parked,
    Pane(usize),
    Previous,
    Next,
    New,
}
pub fn tab_number(rect: Rect) -> Rect {
    Rect {
        x: rect.x + 1,
        y: if rect.rows >= 2 {
            rect.y
        } else {
            rect.y + rect.rows / 2
        },
        cols: 3,
        rows: 1,
    }
}
pub fn tab_close(rect: Rect) -> Rect {
    Rect {
        x: rect.x + rect.cols.saturating_sub(3),
        ..tab_number(rect)
    }
}

pub fn close_buttons(bounds: Rect) -> [Rect; 2] {
    let width = bounds.cols.saturating_sub(6) / 2;
    let height = if bounds.rows >= 9 {
        3
    } else if bounds.rows >= 8 {
        2
    } else {
        1
    };
    let y = bounds.y + bounds.rows.saturating_sub(height + 1);
    [
        Rect {
            x: bounds.x + 2,
            y,
            cols: width,
            rows: height,
        },
        Rect {
            x: bounds.x + bounds.cols - width - 2,
            y,
            cols: width,
            rows: height,
        },
    ]
}
pub fn tab_park(rect: Rect) -> Rect {
    let close = tab_close(rect);
    Rect {
        x: close.x.saturating_sub(3),
        ..close
    }
}
pub fn grid_controls(rect: Rect) -> [Rect; 3] {
    let close = tab_close(rect);
    let park = tab_park(rect);
    [
        Rect {
            x: park.x.saturating_sub(3),
            ..park
        },
        park,
        close,
    ]
}
pub fn parked_button(rows: u16) -> Rect {
    Rect {
        x: 19,
        y: rows - 1,
        cols: 12,
        rows: 1,
    }
}
pub fn new_button(rows: u16) -> Rect {
    Rect {
        x: 14,
        y: rows - 1,
        cols: 5,
        rows: 1,
    }
}
pub fn help_button(rows: u16) -> Rect {
    Rect {
        x: 31,
        y: rows - 1,
        cols: 3,
        rows: 1,
    }
}
pub fn color_cards(bounds: Rect) -> Vec<Rect> {
    let height = bounds.rows.saturating_sub(2) / 3;
    (0..9u16)
        .map(|index| {
            let col = index % 3;
            let row = index / 3;
            Rect {
                x: bounds.x + bounds.cols * col / 3,
                y: bounds.y + 2 + row * height,
                cols: bounds.cols * (col + 1) / 3 - bounds.cols * col / 3,
                rows: height,
            }
        })
        .collect()
}
pub fn sidebar_actions(cols: u16, rows: u16) -> [Rect; 2] {
    let height = if rows >= 14 { 2 } else { 1 };
    let new = Rect {
        x: 0,
        y: rows - 1 - height,
        cols: (cols / 5).clamp(18, 28),
        rows: height,
    };
    [new, parked_button(rows)]
}
/// The width switch is useful only when the configured cap is narrower than
/// the available terminal, regardless of the current wrap/full-width mode.
pub fn reading_limit_effective(cols: u16, rows: u16, config: &super::ui::UiConfig) -> bool {
    let full = tab_body(cols, rows, config, true, None);
    full.x + full.cols < cols
}
pub fn tab_targets(
    cols: u16,
    rows: u16,
    config: &super::ui::UiConfig,
    count: usize,
    active: usize,
) -> Vec<(TabTarget, Rect)> {
    use super::ui::TabPosition;
    let position = config.tab_position.resolve(cols, rows);
    if count == 0 {
        if position == TabPosition::Left {
            let [mut new, parked] = sidebar_actions(cols, rows);
            new.y = 1;
            return vec![(TabTarget::New, new), (TabTarget::Parked, parked)];
        }
        let y = if position == TabPosition::Bottom {
            rows - 3
        } else {
            0
        };
        return vec![
            (
                TabTarget::New,
                Rect {
                    x: 0,
                    y,
                    cols: 5,
                    rows: 2,
                },
            ),
            (TabTarget::Parked, parked_button(rows)),
        ];
    }
    match position {
        TabPosition::Top | TabPosition::Bottom => {
            let overflow = count * 18 + 6 > usize::from(cols);
            let reserve = if overflow { 12 } else { 6 };
            let capacity = usize::from(cols.saturating_sub(reserve) / 18)
                .max(1)
                .min(count);
            let start = if overflow {
                active.saturating_sub(capacity - 1)
            } else {
                0
            };
            let width = ((cols - reserve) / capacity as u16).min(40);
            let x = if overflow { 3 } else { 0 };
            let y = if position == TabPosition::Bottom {
                rows - 3
            } else {
                0
            };
            let mut tabs: Vec<_> = (start..(start + capacity).min(count))
                .enumerate()
                .map(|(i, index)| {
                    (
                        TabTarget::Pane(index),
                        Rect {
                            x: x + i as u16 * width,
                            y,
                            cols: width,
                            rows: 2,
                        },
                    )
                })
                .collect();
            tabs.push((
                TabTarget::New,
                Rect {
                    x: x + tabs.len() as u16 * width,
                    y,
                    cols: 5,
                    rows: 2,
                },
            ));
            tabs.push((TabTarget::Parked, parked_button(rows)));
            if overflow {
                tabs.push((
                    TabTarget::Previous,
                    Rect {
                        x: 0,
                        y,
                        cols: 3,
                        rows: 2,
                    },
                ));
                tabs.push((
                    TabTarget::Next,
                    Rect {
                        x: cols - 3,
                        y,
                        cols: 3,
                        rows: 2,
                    },
                ));
            }
            tabs
        }
        TabPosition::Left => {
            let width = (cols / 5).clamp(18, 28);
            let [mut new, parked] = sidebar_actions(cols, rows);
            let area = new.y;
            let overflow = count > usize::from(area);
            let available = area.saturating_sub(if overflow { 2 } else { 0 });
            let height = (available / count as u16).clamp(1, 2);
            let capacity = usize::from(available / height).max(1);
            let start = if overflow {
                active.saturating_sub(capacity - 1)
            } else {
                0
            };
            let top = u16::from(overflow);
            let mut tabs: Vec<_> = (start..(start + capacity).min(count))
                .enumerate()
                .map(|(i, index)| {
                    (
                        TabTarget::Pane(index),
                        Rect {
                            x: 0,
                            y: top + i as u16 * height,
                            cols: width,
                            rows: height,
                        },
                    )
                })
                .collect();
            let end = top + tabs.len() as u16 * height;
            new.y = end + u16::from(overflow);
            if overflow {
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
                        y: end,
                        cols: width,
                        rows: 1,
                    },
                ));
            }
            tabs.push((TabTarget::New, new));
            tabs.push((TabTarget::Parked, parked));
            tabs
        }
        TabPosition::Auto => unreachable!(),
    }
}

/// Restore and close buttons occupy the full height of a parked entry.
pub fn parked_controls(rect: Rect) -> [Rect; 2] {
    let close = Rect {
        x: rect.x + rect.cols.saturating_sub(4),
        cols: 3,
        ..rect
    };
    [
        Rect {
            x: close.x.saturating_sub(3),
            ..close
        },
        close,
    ]
}

/// Shared three-row parked entry bounds for drawing, hover and click routing.
pub fn parked_cards(bounds: Rect, selection: usize, count: usize) -> Vec<(usize, Rect)> {
    let visible = usize::from(bounds.rows.saturating_sub(2) / 3);
    let offset = selection.saturating_sub(visible.saturating_sub(1));
    (offset..count)
        .take(visible)
        .enumerate()
        .map(|(row, index)| {
            (
                index,
                Rect {
                    x: bounds.x,
                    y: bounds.y + 2 + row as u16 * 3,
                    cols: bounds.cols,
                    rows: 3,
                },
            )
        })
        .collect()
}
