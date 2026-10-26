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
        self.ui.tab_scroll_anchor = None;
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
            let mut number = 0;
            return self
                .panes
                .iter()
                .map(|pane| {
                    let mut view = Self::view(pane, number, Rect::default(), false);
                    if !pane.parked && Some(pane.id) != self.ui.scratch {
                        number += 1;
                    }
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
        let (page_start, count) = self.grid_page();
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
            let ordinal = visible;
            visible += 1;
            if ordinal < page_start || ordinal >= page_start + count {
                let mut view = Self::view(pane, ordinal, Rect::default(), false);
                view.body = Rect::default();
                views.push(view);
                continue;
            }
            let index = ordinal - page_start;
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
            views.push(Self::view(pane, ordinal, rect, false));
        }
        views
    }
    pub fn automatic_shape(&self) -> (u8, u8) {
        let (_, count) = self.grid_page();
        let count = count.max(1);
        let (max_cols, max_rows) = self.grid_capacity();
        let mut best = (1, 1);
        let mut best_score = f64::INFINITY;
        for rows in 1..=usize::from(max_rows).min(count) {
            let cols = count.div_ceil(rows);
            if cols > usize::from(max_cols) {
                continue;
            }
            // Prefer complete rectangles over a marginal gain in pane aspect.
            // Long single-row/column strips are reserved for extreme windows.
            let aspect =
                (f64::from(self.cols) / cols as f64) / (f64::from(self.rows - 1) / rows as f64);
            let uneven = if count % rows == 0 {
                0.0
            } else {
                2.0 + (cols * rows - count) as f64 / count as f64
            };
            let strip = if count >= 4 && (cols == 1 || rows == 1) {
                2.0
            } else {
                0.0
            };
            let score = (aspect / 2.0).ln().powi(2) + uneven + strip;
            if score < best_score {
                best = (cols as u8, rows as u8);
                best_score = score;
            }
        }
        best
    }
    fn grid_capacity(&self) -> (u8, u8) {
        (
            ((self.cols + 1) / 13).max(1) as u8,
            ((self.rows - 1) / 3).max(1) as u8,
        )
    }
    pub fn grid_page(&self) -> (usize, usize) {
        let (cols, rows) = self.grid_capacity();
        let capacity = usize::from(cols) * usize::from(rows);
        let panes: Vec<_> = self
            .panes
            .iter()
            .filter(|pane| !pane.parked && Some(pane.id) != self.ui.scratch)
            .collect();
        let active = panes
            .iter()
            .position(|pane| pane.id == self.active)
            .unwrap_or(0);
        let start = active / capacity * capacity;
        (start, panes.len().saturating_sub(start).min(capacity))
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
        if matches!(
            action,
            Action::Focus(_)
                | Action::Select(_)
                | Action::Read(_)
                | Action::Fullscreen(_)
                | Action::CloseReading
                | Action::TabPosition
                | Action::TabPositionSet(_)
        ) {
            self.ui.tab_scroll_anchor = None;
        }
        match *action {
            Action::AgentLinkPicker(pane) => {
                self.require_pane(pane)?;
                self.help = false;
                self.ui.agent_sessions.clear();
                self.ui.agent_link_loading = true;
                self.ui.agent_link_error = None;
                self.ui.dialog = Some(Dialog {
                    kind: DialogKind::AgentLink,
                    selection: 0,
                    source: Some(pane),
                    filter: AgentFilter::All,
                });
            }
            Action::AgentLinkChoose(_) => {
                return Err("conversation linking requires a live server".into());
            }
            Action::ScrollTabs(amount) => {
                let panes: Vec<_> = self
                    .panes
                    .iter()
                    .filter(|pane| !pane.parked && Some(pane.id) != self.ui.scratch)
                    .collect();
                let active = panes
                    .iter()
                    .position(|pane| pane.id == self.active)
                    .unwrap_or(0);
                let anchor = self
                    .ui
                    .tab_scroll_anchor
                    .unwrap_or(active)
                    .min(panes.len().saturating_sub(1));
                let visible: Vec<_> = tab_targets(
                    self.cols,
                    self.rows,
                    &self.ui.config,
                    panes.len(),
                    anchor,
                    self.panes.iter().filter(|pane| pane.parked).count(),
                )
                .into_iter()
                .filter_map(|(target, _)| {
                    if let TabTarget::Pane(index) = target {
                        Some(index)
                    } else {
                        None
                    }
                })
                .collect();
                let capacity = visible.len();
                if capacity >= panes.len() || capacity == 0 {
                    self.ui.tab_scroll_anchor = None;
                } else {
                    let start = visible[0]
                        .saturating_add_signed(isize::from(amount))
                        .min(panes.len() - capacity);
                    self.ui.tab_scroll_anchor = Some(start + capacity - 1);
                }
            }
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
                if self.reading != ReadingMode::Grid {
                    self.reading = ReadingMode::Tabs(id);
                }
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
            Action::StatusOpen(sort) => {
                self.help = false;
                self.ui.status_sort = sort;
                self.ui.status_descending = sort != super::status::Sort::Pid;
                self.ui.status_view =
                    if matches!(sort, super::status::Sort::Read | super::status::Sort::Write) {
                        super::status::View::Io
                    } else {
                        super::status::View::Main
                    };
                self.ui.status_offset = 0;
                self.ui.status_query.clear();
                self.ui.dialog = Some(Dialog {
                    kind: DialogKind::Status,
                    selection: 0,
                    filter: AgentFilter::All,
                    source: None,
                });
            }
            Action::StatusView(view) => {
                self.ui.status_view = view;
                self.ui.status_sort = if view == super::status::View::Io {
                    super::status::Sort::Read
                } else {
                    super::status::Sort::Cpu
                };
                self.ui.status_descending = true;
                self.ui.status_offset = 0;
            }
            Action::StatusSort(sort) => {
                match sort {
                    super::status::Sort::Read | super::status::Sort::Write => {
                        self.ui.status_view = super::status::View::Io
                    }
                    super::status::Sort::Cpu | super::status::Sort::Memory => {
                        self.ui.status_view = super::status::View::Main
                    }
                    _ => {}
                }
                self.ui.status_descending = if sort == self.ui.status_sort {
                    !self.ui.status_descending
                } else {
                    sort != super::status::Sort::Pid
                };
                self.ui.status_sort = sort;
                self.ui.status_offset = 0;
            }
            Action::StatusQuery(ref query) => {
                self.ui.status_query = query
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(128)
                    .collect();
                self.ui.status_offset = 0;
            }
            Action::StatusScroll(amount) => {
                let count = super::status::processes(
                    &self.ui.status,
                    &self.ui.status_query,
                    self.ui.status_sort,
                    self.ui.status_descending,
                )
                .len();
                let bounds = super::status::bounds(self.cols, self.rows);
                let capacity = usize::from(
                    (bounds.y + bounds.rows - 1)
                        .saturating_sub(super::status::table_y(bounds, &self.ui.status) + 1),
                )
                .max(1);
                let maximum = count.saturating_sub(capacity);
                self.ui.status_offset = self
                    .ui
                    .status_offset
                    .min(maximum)
                    .saturating_add_signed(isize::from(amount))
                    .min(maximum);
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
            // Legacy clients may still send these; tab bodies always fill their area.
            Action::WrapToggle | Action::ReadResize { .. } => {}
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
                // Retained for old custom bindings; tabs always use the sidebar.
                self.ui.config.tab_position = super::ui::TabPosition::Left;
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
            Action::DialogClose => {
                if let Some(dialog) = self
                    .ui
                    .dialog
                    .filter(|dialog| dialog.kind == DialogKind::RestoreConfirm)
                {
                    let selection = self
                        .panes
                        .iter()
                        .filter(|pane| pane.parked)
                        .position(|pane| Some(pane.id) == dialog.source)
                        .unwrap_or(0);
                    self.apply(&Action::Parked)?;
                    self.ui.dialog.as_mut().unwrap().selection = selection;
                } else {
                    self.ui.dialog = None;
                }
            }
            Action::DialogNavigate { dx, dy } => {
                let count = self.dialog_entries().len();
                if let Some(dialog) = self.ui.dialog.as_mut() {
                    if dialog.kind == DialogKind::Config {
                        dialog.selection = 0;
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
                    .filter(|v| !v.pane.parked && v.pane.id != normal && v.rect.cols > 0)
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
            Action::Focus(_) | Action::Select(_) | Action::Read(_) | Action::Fullscreen(_)
        ) {
            for notification in &mut self.ui.notifications {
                if self
                    .panes
                    .iter()
                    .find(|pane| pane.id == self.active)
                    .is_some_and(|pane| {
                        super::activity::matches(notification, pane, &self.ui.notification_run)
                    })
                {
                    notification.read = true;
                }
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
        if cols == 0 || rows == 0 {
            return Err("grid dimensions must be positive".into());
        }
        Ok(cols as usize * rows as usize)
    }
    pub fn dialog_entries(&self) -> Vec<PaneId> {
        match self.ui.dialog.map(|dialog| dialog.kind) {
            Some(DialogKind::AgentLink) => {
                return (0..=self.ui.agent_sessions.len() as u64).collect();
            }
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
        }) => (0..1).collect(),
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
    if kind == super::ui::DialogKind::Status {
        return super::status::bounds(cols, rows);
    }
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
        (72, 7)
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
    (0, if bounds.rows >= 7 { 3 } else { 1 })
}
/// Logical focus index, optional explicit on/off value, and shared mouse geometry.
pub fn config_targets(bounds: Rect) -> Vec<(usize, Option<bool>, Rect)> {
    let (_, height) = config_geometry(bounds);
    let mut choices = Vec::new();
    let inner = bounds.cols - 4;
    for value in 0..2u16 {
        choices.push((
            0,
            Some(value == 0),
            Rect {
                x: bounds.x + 2 + inner / 2 + (inner - inner / 2 - 1) * value / 2 + value,
                y: bounds.y + 2,
                cols: (inner - inner / 2 - 1) * (value + 1) / 2
                    - (inner - inner / 2 - 1) * value / 2,
                rows: height,
            },
        ));
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
/// Grow with the screen while staying within the original width and +20%.
pub fn sidebar_width(cols: u16) -> u16 {
    let minimum = (cols / 5).clamp(18, 28);
    (cols / 4)
        .clamp(minimum, minimum * 6 / 5)
        .saturating_sub(2)
        .max(minimum)
}
/// Terminal content occupies every column except visible UI dividers.
pub fn tab_body(
    cols: u16,
    rows: u16,
    _config: &super::ui::UiConfig,
    _full: bool,
    _size: Option<(u16, u16)>,
) -> Rect {
    let sidebar = sidebar_width(cols);
    Rect {
        x: sidebar,
        y: 0,
        cols: cols - sidebar,
        rows,
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
pub fn tab_bar_area(cols: u16, rows: u16, _config: &super::ui::UiConfig) -> Rect {
    Rect {
        x: 0,
        y: 0,
        cols: sidebar_width(cols) - 1,
        rows: rows - sidebar_status_rows(rows),
    }
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
pub fn tab_number_for(rect: Rect, number: usize) -> Rect {
    Rect {
        cols: (number.to_string().len() as u16 + 2)
            .max(3)
            .min(rect.cols.saturating_sub(7).max(1)),
        ..tab_number(rect)
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
pub fn grid_controls(rect: Rect) -> [Rect; 2] {
    [tab_park(rect), tab_close(rect)]
}
pub fn parked_label(count: usize) -> String {
    match count {
        0 => " ⊟ Parked ".into(),
        count => format!(" ⊟ Parked ({count}) "),
    }
}
pub fn parked_button(rows: u16, count: usize) -> Rect {
    Rect {
        x: super::ui::FOOTER_CONFIG_COLS + super::ui::FOOTER_MODE_COLS,
        y: rows - 1,
        cols: parked_label(count).chars().count() as u16,
        rows: 1,
    }
}
pub fn help_button(rows: u16, count: usize) -> Rect {
    let parked = parked_button(rows, count);
    Rect {
        x: parked.x + parked.cols,
        y: rows - 1,
        cols: 3,
        rows: 1,
    }
}
pub fn sidebar_status_rows(rows: u16) -> u16 {
    if rows >= 24 {
        8
    } else if rows >= 18 {
        7
    } else {
        3
    }
}
pub fn sidebar_controls(cols: u16, rows: u16, _count: usize) -> [Rect; 4] {
    let width = sidebar_width(cols) - 1;
    let rect = |x, y, cols| Rect {
        x,
        y,
        cols,
        rows: 1,
    };
    let right = width.div_ceil(2);
    [
        rect(0, rows - 2, right),
        rect(right, rows - 2, width - right),
        rect(0, rows - 1, right),
        rect(right, rows - 1, width - right),
    ]
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
pub fn sidebar_actions(cols: u16, rows: u16, parked_count: usize) -> [Rect; 2] {
    let height = 3;
    let new = Rect {
        x: 0,
        y: rows - sidebar_status_rows(rows) - height,
        cols: sidebar_width(cols) - 1,
        rows: height,
    };
    [new, sidebar_controls(cols, rows, parked_count)[2]]
}
pub fn tab_targets(
    cols: u16,
    rows: u16,
    _config: &super::ui::UiConfig,
    count: usize,
    active: usize,
    parked_count: usize,
) -> Vec<(TabTarget, Rect)> {
    if count == 0 {
        let [mut new, parked] = sidebar_actions(cols, rows, parked_count);
        new.y = 1;
        return vec![(TabTarget::New, new), (TabTarget::Parked, parked)];
    }
    let width = sidebar_width(cols) - 1;
    let [mut new, parked] = sidebar_actions(cols, rows, parked_count);
    let area = new.y;
    let height = new.rows;
    let overflow = count > usize::from(area / height);
    let compact = area < height + 2;
    let available = area.saturating_sub(if overflow {
        if compact { 1 } else { 2 }
    } else {
        0
    });
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
    new.y = end + u16::from(overflow && !compact);
    if overflow {
        tabs.push((
            TabTarget::Previous,
            Rect {
                x: 0,
                y: 0,
                cols: if compact { width / 2 } else { width },
                rows: 1,
            },
        ));
        tabs.push((
            TabTarget::Next,
            Rect {
                x: if compact { width / 2 } else { 0 },
                y: if compact { 0 } else { end },
                cols: if compact { width - width / 2 } else { width },
                rows: 1,
            },
        ));
    }
    tabs.push((TabTarget::New, new));
    tabs.push((TabTarget::Parked, parked));
    tabs
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
