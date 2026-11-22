//! The stream TUI's controller: a local `Workspace` (tabs, order, parking,
//! focus, dialogs, grid geometry, scratch, settings) driven by KelpServer's
//! pane stream and session API.
//!
//! The controller is transport-free: `event` takes stream events, `message`
//! takes the input router's output, and `take_requests` returns the stream
//! requests to send. The attach loop (`attach.rs`) owns the socket and the
//! terminal. This port of the composed server's action handling keeps the
//! legacy behaviour wherever the stream offers the same session operation.
use super::super::{
    history::Entry,
    protocol::*,
    render,
    stream::{
        CloseReason, Event, PanePatch, PaneSession, PaneSnapshot, Request, StopReason, Update,
    },
    ui::{self, DialogKind},
    vitals,
    workspace::Workspace,
};
use super::state::{LayoutTab, PaneState, Store};
use crate::{Result, model::Status};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

/// Session API capabilities the stream TUI needs from a server.
pub const REQUIRED: &[&str] = &[
    "pane-snapshots",
    "pane-patches",
    "pane-leases",
    "metadata",
    "notifications",
    "vitals",
    "pane-lifecycle",
    "closed-sessions",
    "pane-history",
    "pane-scroll",
    "notification-ack-all",
    "server-lifecycle",
];
pub const SCRATCH_TITLE: &str = "Scratch terminal";
const MAX_SUBSCRIPTIONS: usize = 10;
const MAX_SUBSCRIBED_CELLS: usize = 100_000;
const MAX_COPY: usize = 64 * 1024;
/// Panes created within this many seconds of the oldest pane are the
/// server's startup panes.
const STARTUP_WINDOW: u64 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    NewTab {
        grid: bool,
    },
    Scratch,
    /// A replacement shell, then close this pane (the last main tab).
    Replace {
        close: PaneId,
    },
    /// A replacement after the last main tab exited while scratch remains.
    Refill,
    /// Recreate a tab of the layout template after a server restart.
    Respawn {
        slot: usize,
    },
    Close,
    ClosedSessions {
        reopen: bool,
    },
    Restore {
        key: String,
        rerun: bool,
        kind: bool,
        name: String,
    },
    Search,
    Shutdown,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Take every displayed pane on attach, like `attach --takeover`.
    pub takeover: bool,
    /// Name of this workspace, used to prefer its own closed tabs.
    pub workspace: String,
    pub dev_badge: bool,
}

/// A text selection in pane-body coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub pane: PaneId,
    pub start: Point,
    pub end: Point,
}

pub struct Controller {
    pub workspace: Workspace,
    pub screens: BTreeMap<PaneId, Screen>,
    revisions: BTreeMap<PaneId, u64>,
    pub epoch: String,
    pub client: u64,
    pub capabilities: BTreeSet<String>,
    pub server_version: Option<String>,
    infos: BTreeMap<PaneId, PaneInfo>,
    sessions: BTreeMap<PaneId, PaneSession>,
    leases: BTreeMap<PaneId, u64>,
    /// Claims sent and not yet confirmed, with whether they took over.
    claims: BTreeMap<PaneId, bool>,
    /// This client controls input, focus and geometry (legacy controller).
    pub controlling: bool,
    options: Options,
    synced: bool,
    next_request: u64,
    pending: BTreeMap<String, Pending>,
    out: Vec<Request>,
    subscribed: Vec<PaneId>,
    sizes: BTreeMap<PaneId, (u16, u16)>,
    themed: BTreeMap<PaneId, [[u8; 3]; 2]>,
    host: bool,
    server_read: BTreeSet<u64>,
    acked: BTreeSet<u64>,
    seen_notifications: Option<BTreeSet<u64>>,
    /// Template slots of panes during reconciliation after a restart.
    slots: BTreeMap<PaneId, usize>,
    /// Closed-session records of scratch panes, hidden from Closed.
    scratch_records: BTreeSet<String>,
    focus_click: bool,
    title_animation: BTreeMap<PaneId, (String, u16)>,
    animation_at: Instant,
    title_at: Instant,
    pub store: Store,
    pub selection: Option<Selection>,
    pub spinner: usize,
    pub notice: Option<String>,
    /// OSC 52 payloads to write to the outer terminal.
    pub clipboard: Vec<String>,
    pub bell: bool,
    pub detached: bool,
    pub stopped: Option<String>,
    pub dirty: bool,
}

fn default_palette(id: PaneId) -> u8 {
    (id.saturating_sub(1) % 9) as u8
}
fn clamp_pane_size(cols: u16, rows: u16) -> Option<(u16, u16)> {
    (cols >= 2 && rows >= 1).then_some((cols.min(400), rows.min(160)))
}

impl Controller {
    pub fn new(cols: u16, rows: u16, store: Store, options: Options) -> Result<Self> {
        let mut workspace = Workspace::new(cols, rows)?;
        workspace.ui.config = store.document.settings.clone().unwrap_or_default();
        workspace.ui.config.validate()?;
        workspace.ui.show_dev_badge = options.dev_badge;
        let now = Instant::now();
        Ok(Self {
            workspace,
            screens: BTreeMap::new(),
            revisions: BTreeMap::new(),
            epoch: String::new(),
            client: 0,
            capabilities: BTreeSet::new(),
            server_version: None,
            infos: BTreeMap::new(),
            sessions: BTreeMap::new(),
            leases: BTreeMap::new(),
            claims: BTreeMap::new(),
            controlling: false,
            options,
            synced: false,
            next_request: 0,
            pending: BTreeMap::new(),
            out: Vec::new(),
            subscribed: Vec::new(),
            sizes: BTreeMap::new(),
            themed: BTreeMap::new(),
            host: false,
            server_read: BTreeSet::new(),
            acked: BTreeSet::new(),
            seen_notifications: None,
            slots: BTreeMap::new(),
            scratch_records: BTreeSet::new(),
            focus_click: false,
            title_animation: BTreeMap::new(),
            animation_at: now,
            title_at: now,
            store,
            selection: None,
            spinner: 0,
            notice: None,
            clipboard: Vec::new(),
            bell: false,
            detached: false,
            stopped: None,
            dirty: true,
        })
    }

    /// Requests to send now, in order.
    pub fn take_requests(&mut self) -> Vec<Request> {
        self.sync();
        std::mem::take(&mut self.out)
    }
    fn request(&mut self, pending: Pending) -> String {
        self.next_request += 1;
        let id = format!("tui-{}", self.next_request);
        self.pending.insert(id.clone(), pending);
        id
    }
    fn supports(&self, capability: &str) -> bool {
        self.capabilities.contains(capability)
    }
    fn is_scratch(&self, id: PaneId) -> bool {
        self.workspace.ui.scratch == Some(id)
    }
    fn main_panes(&self) -> impl Iterator<Item = &PaneInfo> {
        let scratch = self.workspace.ui.scratch;
        self.workspace
            .panes
            .iter()
            .filter(move |pane| Some(pane.id) != scratch)
    }
    pub fn synced(&self) -> bool {
        self.synced
    }
    pub fn leases(&self) -> &BTreeMap<PaneId, u64> {
        &self.leases
    }
    pub fn subscribed(&self) -> &[PaneId] {
        &self.subscribed
    }

    /// Accepts Welcome; the caller checked the version.
    pub fn welcome(
        &mut self,
        client: u64,
        epoch: String,
        capabilities: Vec<String>,
        server_version: Option<String>,
    ) -> Result<()> {
        self.capabilities = capabilities.into_iter().collect();
        if let Some(missing) = REQUIRED.iter().find(|name| !self.supports(name)) {
            return Err(format!(
                "this workspace's server lacks the stream TUI's session API ({missing}); attach with --legacy or restart the workspace with a current server"
            )
            .into());
        }
        if client == 0 || epoch.is_empty() {
            return Err("invalid stream welcome".into());
        }
        self.client = client;
        self.epoch = epoch.clone();
        self.store.epoch = epoch;
        self.server_version = server_version;
        self.out.push(Request::SessionEvents { enabled: true });
        if self.supports("clipboard-write") {
            self.out.push(Request::Clipboard { enabled: true });
        }
        self.out.push(Request::Subscribe { panes: Vec::new() });
        Ok(())
    }

    pub fn event(&mut self, event: Event) -> Result<()> {
        self.dirty = true;
        match event {
            Event::Welcome { .. } => return Err("unexpected second welcome".into()),
            Event::Update(update) => self.update(update)?,
            Event::Lease { pane, owner } => {
                self.claims.remove(&pane);
                match owner {
                    Some(owner) => {
                        self.leases.insert(pane, owner);
                    }
                    None => {
                        self.leases.remove(&pane);
                    }
                }
                if owner != Some(self.client) {
                    self.sizes.remove(&pane);
                    self.themed.remove(&pane);
                }
            }
            Event::Clipboard { text, .. } => {
                if text.len() <= MAX_COPY {
                    self.clipboard.push(text);
                }
            }
            Event::Error { message, request } => self.error(message, request),
            Event::PaneCreated { session } => self.created(session),
            Event::PaneExited { pane, .. } => {
                for pane in self
                    .workspace
                    .panes
                    .iter_mut()
                    .filter(|info| info.id == pane)
                {
                    pane.dead = true;
                }
                if let Some(info) = self.infos.get_mut(&pane) {
                    info.dead = true;
                }
            }
            Event::PaneClosed {
                pane,
                reason,
                record,
                ..
            } => {
                if self.is_scratch(pane) {
                    self.scratch_records.extend(record);
                }
                self.infos.remove(&pane);
                self.sessions.remove(&pane);
                self.remove(pane, reason == CloseReason::Exited);
            }
            Event::ServerStopping { reason } => {
                self.stopped = Some(match reason {
                    StopReason::Shutdown => "the workspace was stopped".into(),
                    StopReason::LastPaneExited => "the last terminal exited".into(),
                });
            }
            Event::Spawned { request, pane } => self.spawned(&request, pane)?,
            Event::Done { request } => {
                if self.pending.remove(&request) == Some(Pending::Shutdown) {
                    self.stopped = Some("the workspace was stopped".into());
                }
            }
            Event::ClosedSessions { request, records } => {
                if let Some(Pending::ClosedSessions { reopen }) = self.pending.remove(&request) {
                    let mut closed: Vec<Entry> = records
                        .into_iter()
                        .map(|record| record.record)
                        .filter(|entry| !self.scratch_records.contains(&entry.key))
                        .collect();
                    if reopen {
                        let current = self.options.workspace.clone();
                        closed.sort_by_key(|e| {
                            (
                                e.workspace != current,
                                e.reason != "Closed",
                                std::cmp::Reverse(e.closed_at),
                            )
                        });
                    } else {
                        closed.sort_by_key(|e| std::cmp::Reverse(e.closed_at));
                    }
                    closed.truncate(256);
                    self.workspace.ui.closed_tabs = closed;
                    self.workspace.ui.recovery_loading = false;
                    if reopen
                        && self
                            .workspace
                            .ui
                            .dialog
                            .is_some_and(|d| d.kind == DialogKind::Recovery)
                        && !self.workspace.ui.closed_tabs.is_empty()
                    {
                        self.apply(&Action::ReopenRequest(0));
                    }
                }
            }
            Event::Restored {
                request,
                pane,
                existing,
                ..
            } => {
                if let Some(Pending::Restore {
                    key,
                    rerun,
                    kind,
                    name,
                }) = self.pending.remove(&request)
                {
                    self.restored(pane, existing, &key, rerun, kind, &name);
                }
            }
            Event::History { request, .. } => {
                self.pending.remove(&request);
            }
            Event::Search {
                request, results, ..
            } => {
                if self.pending.remove(&request) == Some(Pending::Search) {
                    let mut hits = Vec::new();
                    for result in results {
                        for hit in result.hits {
                            let row = usize::from(hit.start.y);
                            hits.push(SearchHit {
                                pane: result.pane,
                                line: hit.first_line + i32::from(hit.start.y),
                                text: hit.lines.get(row).cloned().unwrap_or_default(),
                            });
                        }
                    }
                    self.workspace.ui.search_hits = hits;
                    if let Some(dialog) = self.workspace.ui.dialog.as_mut() {
                        if dialog.kind == DialogKind::Search {
                            dialog.selection = 0;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn error(&mut self, message: String, request: Option<String>) {
        if let Some(pending) = request.and_then(|request| self.pending.remove(&request)) {
            match pending {
                Pending::ClosedSessions { .. } => {
                    self.workspace.ui.recovery_loading = false;
                    self.notice = Some(format!("Recovery: {message}"));
                }
                Pending::Restore { .. } => self.notice = Some(format!("Restore: {message}")),
                Pending::Respawn { .. } => {}
                _ => self.notice = Some(message),
            }
            return;
        }
        // Lease races are routine: another client claimed first, or a claim
        // for a pane that just exited. Their effect shows in the next update.
        if message.contains("leased") || message.contains("has exited") {
            self.claims.retain(|_, takeover| *takeover);
            return;
        }
        self.notice = Some(message.chars().take(1024).collect());
    }

    fn update(&mut self, update: Update) -> Result<()> {
        if update.epoch != self.epoch {
            return Err("the workspace server restarted; attach again".into());
        }
        // Screens of the current subscription.
        let subscribed: BTreeSet<_> = update.subscribed.iter().copied().collect();
        self.screens.retain(|id, _| subscribed.contains(id));
        self.revisions.retain(|id, _| subscribed.contains(id));
        for snapshot in update.snapshots {
            super::super::stream::validate(&snapshot)?;
            let PaneSnapshot {
                pane,
                revision,
                screen,
            } = snapshot;
            self.revisions.insert(pane, revision);
            self.screens.insert(pane, screen);
        }
        for patch in update.patches {
            let pane = patch.pane;
            let applied = match (self.screens.get_mut(&pane), self.revisions.get_mut(&pane)) {
                (Some(screen), Some(revision)) => apply_patch(screen, revision, &patch).is_ok(),
                _ => false,
            };
            if !applied {
                self.screens.remove(&pane);
                self.revisions.remove(&pane);
                self.out.push(Request::Resync { pane });
            }
        }
        // Observations.
        self.infos = update
            .panes
            .into_iter()
            .map(|pane| (pane.id, pane))
            .collect();
        if !update.sessions.is_empty() || self.infos.is_empty() {
            self.sessions = update
                .sessions
                .into_iter()
                .map(|session| (session.pane, session))
                .collect();
        }
        let mut leases: BTreeMap<_, _> = update
            .leases
            .into_iter()
            .map(|lease| (lease.pane, lease.owner))
            .collect();
        for pane in self.claims.keys() {
            leases.insert(*pane, self.client);
        }
        for (pane, owner) in &leases {
            if *owner != self.client {
                self.sizes.remove(pane);
                self.themed.remove(pane);
            }
        }
        self.leases = leases;
        self.server_read = update
            .notifications
            .iter()
            .filter(|n| n.read)
            .map(|n| n.id)
            .collect();
        let mut notifications = update.notifications;
        for notification in &mut notifications {
            if self.acked.contains(&notification.id) {
                notification.read = true;
            }
        }
        self.acked.retain(|id| !self.server_read.contains(id));
        let ids: BTreeSet<_> = notifications.iter().map(|n| n.id).collect();
        if let Some(previous) = &self.seen_notifications {
            if self.controlling
                && self.workspace.ui.config.notification_sound
                && notifications
                    .iter()
                    .any(|n| !n.read && !previous.contains(&n.id))
            {
                self.bell = true;
            }
        }
        self.seen_notifications = Some(ids);
        self.workspace.ui.notifications = notifications;
        self.vitals(&update.vitals);
        if let Some(host) = update.host {
            if self
                .workspace
                .ui
                .dialog
                .is_some_and(|d| d.kind == DialogKind::Status)
            {
                self.workspace.ui.status = *host;
            }
        }
        if !self.synced {
            self.reconcile();
            self.synced = true;
            let active = self.workspace.active;
            self.controlling = self.options.takeover
                || active == 0
                || self
                    .leases
                    .get(&active)
                    .is_none_or(|owner| *owner == self.client);
            if self.options.takeover {
                self.take_control();
            }
        } else {
            self.merge_panes();
            let active = self.workspace.active;
            if self.controlling
                && !self.claims.contains_key(&active)
                && self
                    .leases
                    .get(&active)
                    .is_some_and(|owner| *owner != self.client)
            {
                // Another client took the active pane: observe, as the
                // composed TUI did when another frontend gained focus.
                self.controlling = false;
            }
        }
        self.workspace.ui.notification_run = self.notification_run();
        self.workspace.ui.unused = self
            .workspace
            .panes
            .iter()
            .filter(|pane| {
                !pane.dead
                    && pane.kind.is_none()
                    && pane.command.is_none()
                    && pane.status == Status::Idle
            })
            .map(|pane| pane.id)
            .collect();
        // Panes announced by pane_created may precede their first observation.
        self.store.live = Some(
            self.infos
                .keys()
                .copied()
                .chain(self.workspace.panes.iter().map(|pane| pane.id))
                .collect(),
        );
        Ok(())
    }

    /// The server's notification run, inferred from notifications that
    /// belong to a live pane process. Stream updates do not carry it.
    fn notification_run(&self) -> String {
        self.workspace
            .ui
            .notifications
            .iter()
            .rev()
            .filter_map(|n| n.origin.as_ref().map(|origin| (n, origin)))
            .find(|(n, origin)| {
                self.infos
                    .get(&n.pane)
                    .is_some_and(|pane| pane.pid == origin.pid && !pane.dead)
            })
            .map(|(_, origin)| origin.run.clone())
            .unwrap_or_else(|| self.workspace.ui.notification_run.clone())
    }

    fn vitals(&mut self, vitals: &vitals::Vitals) {
        self.workspace.ui.disk_readings =
            vitals.disks.iter().map(|p| (String::new(), *p)).collect();
        self.workspace.ui.vitals = if self.workspace.cols <= 60
            && self.workspace.reading == ReadingMode::Grid
            && self.workspace.active != 0
        {
            let mut compact = Vec::new();
            if let Some(cpu) = vitals.cpu {
                compact.push((format!("C{cpu}"), 114));
            }
            if let Some(memory) = vitals.memory {
                compact.push((format!("M{memory}"), 141));
            }
            if let Some(time) = &vitals.time {
                compact.push((time.clone(), 250));
            }
            compact
        } else {
            vitals::render_segments(vitals)
        };
    }

    fn info_for(&self, id: PaneId, base: Option<&PaneInfo>) -> PaneInfo {
        let state = self.store.pane(id).cloned().unwrap_or_default();
        let mut info = base.cloned().unwrap_or_else(|| {
            let session = self.sessions.get(&id);
            PaneInfo {
                id,
                cwd: session.map(|s| s.launch_cwd.clone()).unwrap_or_default(),
                title: "shell".into(),
                ..PaneInfo::default()
            }
        });
        info.parked = state.parked && !state.scratch;
        info.palette = state.color.unwrap_or_else(|| default_palette(id));
        if let Some(name) = state.name {
            info.title = name;
        } else if state.scratch {
            info.title = SCRATCH_TITLE.into();
        }
        info
    }

    /// Builds the local tab list on the first update of a connection.
    fn reconcile(&mut self) {
        let ids: Vec<PaneId> = self.infos.keys().copied().collect();
        let mut order: Vec<PaneId>;
        let mut scratch = None;
        if let Some(state) = self.store.current().cloned() {
            // Reattaching within the same server run.
            order = state
                .order
                .iter()
                .copied()
                .filter(|id| self.infos.contains_key(id))
                .collect();
            for id in &ids {
                if state.panes.get(id).is_some_and(|pane| pane.scratch) {
                    scratch = scratch.or(Some(*id));
                } else if !order.contains(id) {
                    order.push(*id);
                }
            }
        } else if !self.store.document.migrated {
            // First stream attach to this workspace: keep the arrangement the
            // composed TUI left in the server (order, parking, colours).
            let mut panes: Vec<&PaneInfo> = self.infos.values().collect();
            panes.sort_by_key(|pane| (pane.parked, pane.number, pane.id));
            order = panes.iter().map(|pane| pane.id).collect();
            let seeds: Vec<_> = panes
                .iter()
                .map(|pane| {
                    (
                        pane.id,
                        PaneState {
                            parked: pane.parked,
                            color: (pane.palette != default_palette(pane.id)
                                && usize::from(pane.palette) < ui::PALETTE.len())
                            .then_some(pane.palette),
                            ..PaneState::default()
                        },
                    )
                })
                .collect();
            for (id, state) in seeds {
                self.store.set_pane(id, state);
            }
            self.store.mark_migrated();
        } else {
            // A new server run: map the layout template onto its panes.
            order = self.apply_template(&ids);
        }
        self.workspace.panes = order
            .iter()
            .chain(scratch.iter())
            .map(|id| self.info_for(*id, self.infos.get(id)))
            .collect();
        self.workspace.ui.scratch = scratch;
        let active = self
            .main_panes()
            .find(|pane| !pane.parked)
            .map_or(0, |pane| pane.id);
        self.workspace.active = active;
        self.workspace.reflow();
        let (cols, rows) = (self.workspace.cols, self.workspace.rows);
        let _ = self.workspace.resize(cols, rows);
        self.persist();
    }

    /// Matches template tabs to live panes by folder and arguments, then
    /// adopts remaining shells in order. When every live pane is one of the
    /// server's untouched startup panes, missing tabs are spawned.
    fn apply_template(&mut self, ids: &[PaneId]) -> Vec<PaneId> {
        let template = self.store.document.layout.clone();
        let mut free: Vec<PaneId> = ids.to_vec();
        let mut assigned: Vec<(usize, PaneId)> = Vec::new();
        let matches = |session: Option<&PaneSession>, tab: &LayoutTab| {
            session.is_some_and(|s| s.launch_cwd == tab.cwd && s.argv == tab.argv)
        };
        let mut unmatched = Vec::new();
        for (slot, tab) in template.iter().enumerate() {
            if let Some(index) = free
                .iter()
                .position(|id| matches(self.sessions.get(id), tab))
            {
                assigned.push((slot, free.remove(index)));
            } else {
                unmatched.push(slot);
            }
        }
        let earliest = self.sessions.values().map(|s| s.created_at).min();
        let fresh = self.infos.values().all(|pane| {
            pane.kind.is_none()
                && pane.command.is_none()
                && self
                    .sessions
                    .get(&pane.id)
                    .is_some_and(|s| earliest.is_some_and(|t| s.created_at <= t + STARTUP_WINDOW))
        });
        for slot in unmatched {
            let tab = &template[slot];
            let shell = free.iter().position(|id| {
                tab.argv.is_empty() && self.sessions.get(id).is_none_or(|s| s.argv.is_empty())
            });
            if let Some(index) = shell {
                assigned.push((slot, free.remove(index)));
            } else if fresh && self.supports("pane-lifecycle") {
                let request = self.request(Pending::Respawn { slot });
                self.out.push(Request::Spawn {
                    request,
                    cwd: Some(tab.cwd.clone().into()),
                    argv: tab.argv.clone(),
                });
            }
        }
        assigned.sort();
        for (slot, id) in &assigned {
            let tab = &template[*slot];
            self.slots.insert(*id, *slot);
            self.store.set_pane(
                *id,
                PaneState {
                    name: tab.name.clone(),
                    color: tab.color,
                    parked: tab.parked,
                    scratch: false,
                },
            );
        }
        assigned.into_iter().map(|(_, id)| id).chain(free).collect()
    }

    /// Updates the local tab list from the latest observations.
    fn merge_panes(&mut self) {
        let gone: Vec<_> = self
            .workspace
            .panes
            .iter()
            .map(|pane| pane.id)
            .filter(|id| !self.infos.contains_key(id))
            .collect();
        for id in gone {
            self.remove(id, true);
        }
        let known: BTreeSet<_> = self.workspace.panes.iter().map(|p| p.id).collect();
        let fresh: Vec<_> = self
            .infos
            .keys()
            .copied()
            .filter(|id| !known.contains(id))
            .collect();
        for id in fresh {
            self.created(self.sessions.get(&id).cloned().unwrap_or(PaneSession {
                pane: id,
                created_at: crate::now(),
                launch_cwd: String::new(),
                shell: String::new(),
                argv: Vec::new(),
                startup: None,
                terminal_title: None,
            }));
        }
        let panes: Vec<_> = self
            .workspace
            .panes
            .iter()
            .map(|pane| {
                let mut info = self.info_for(pane.id, self.infos.get(&pane.id));
                // Local presentation state wins over the composed fields.
                info.parked = pane.parked;
                info.palette = pane.palette;
                info.number = pane.number;
                info
            })
            .collect();
        self.workspace.panes = panes;
        self.workspace.reflow();
        self.persist();
    }

    fn created(&mut self, session: PaneSession) {
        let id = session.pane;
        self.sessions.insert(id, session);
        if let Some(live) = self.store.live.as_mut() {
            live.insert(id);
        }
        if !self.synced || self.workspace.panes.iter().any(|pane| pane.id == id) {
            return;
        }
        let mut info = self.info_for(id, self.infos.get(&id));
        info.parked = false;
        // New tabs go after the main tabs, before the scratch terminal.
        let index = self
            .workspace
            .ui
            .scratch
            .and_then(|scratch| self.workspace.panes.iter().position(|p| p.id == scratch))
            .unwrap_or(self.workspace.panes.len());
        self.workspace.panes.insert(index, info);
        if self.workspace.active == 0 {
            self.workspace.active = id;
        }
        self.workspace.reflow();
        self.persist();
    }

    /// Removes a closed pane, refocusing like the composed TUI: the
    /// previously selected open, unparked tab, else the first tab.
    fn remove(&mut self, id: PaneId, exited: bool) {
        self.screens.remove(&id);
        self.revisions.remove(&id);
        self.sizes.remove(&id);
        self.themed.remove(&id);
        self.claims.remove(&id);
        self.leases.remove(&id);
        self.slots.remove(&id);
        self.title_animation.remove(&id);
        self.workspace.ui.title_ticks.remove(&id);
        if self.selection.is_some_and(|s| s.pane == id) {
            self.selection = None;
        }
        self.store.remove_pane(id);
        if !self.workspace.panes.iter().any(|pane| pane.id == id) {
            return;
        }
        let is_scratch = self.is_scratch(id);
        if is_scratch && self.workspace.ui.scratch_visible {
            let _ = self.workspace.apply(&Action::ScratchClose);
        }
        self.workspace.track_focus();
        self.workspace.panes.retain(|pane| pane.id != id);
        let fallback = self
            .workspace
            .previous_focus(id)
            .or_else(|| {
                self.main_panes()
                    .find(|pane| !pane.parked && pane.id != id)
                    .map(|pane| pane.id)
            })
            .unwrap_or(0);
        if is_scratch {
            self.workspace.ui.scratch = None;
            self.workspace.ui.scratch_visible = false;
            self.workspace.ui.scratch_return = None;
            self.workspace.ui.scratch_reading = None;
        } else if self.workspace.ui.scratch_return == Some(id) {
            self.workspace.ui.scratch_return = Some(fallback);
        }
        if self.workspace.active == id {
            self.workspace.active = fallback;
        }
        fn replace(reading: ReadingMode, id: PaneId, fallback: PaneId) -> ReadingMode {
            match reading {
                ReadingMode::Tabs(pane) | ReadingMode::Capped(pane) if pane == id => {
                    ReadingMode::Tabs(fallback)
                }
                ReadingMode::Fullscreen(pane) if pane == id => ReadingMode::Fullscreen(fallback),
                other => other,
            }
        }
        self.workspace.reading = replace(self.workspace.reading, id, fallback);
        self.workspace.ui.scratch_reading = self
            .workspace
            .ui
            .scratch_reading
            .map(|reading| replace(reading, id, fallback));
        if self
            .workspace
            .ui
            .dialog
            .is_some_and(|d| d.source == Some(id) && d.kind != DialogKind::ReopenConfirm)
        {
            self.workspace.ui.dialog = None;
        }
        if fallback == 0 && !self.workspace.ui.scratch_visible {
            self.workspace.reading = ReadingMode::Grid;
        }
        self.workspace.reflow();
        // Exiting the last main shell while scratch remains: keep a main tab,
        // as the composed server did.
        if exited
            && !is_scratch
            && self.main_panes().next().is_none()
            && self.workspace.ui.scratch.is_some()
            && !self.pending.values().any(|p| *p == Pending::Refill)
        {
            let request = self.request(Pending::Refill);
            self.out.push(Request::Spawn {
                request,
                cwd: None,
                argv: Vec::new(),
            });
        }
        self.persist();
    }

    fn spawned(&mut self, request: &str, pane: PaneId) -> Result<()> {
        let Some(pending) = self.pending.remove(request) else {
            return Ok(());
        };
        if !self.workspace.panes.iter().any(|p| p.id == pane) {
            // pane_created precedes spawned; tolerate a missing event.
            self.created(PaneSession {
                pane,
                created_at: crate::now(),
                launch_cwd: String::new(),
                shell: String::new(),
                argv: Vec::new(),
                startup: None,
                terminal_title: None,
            });
        }
        match pending {
            Pending::NewTab { grid } => {
                self.workspace.ui.tab_scroll_anchor = None;
                if self.workspace.ui.scratch_visible {
                    self.workspace.apply(&Action::ScratchClose)?;
                }
                self.workspace.reflow();
                self.workspace.active = pane;
                self.workspace.reading = if grid {
                    ReadingMode::Grid
                } else {
                    ReadingMode::Tabs(pane)
                };
                self.workspace.ui.config.view_modes[ui::geometry_bucket(self.workspace.cols)] =
                    Some(!grid);
                self.workspace.track_focus();
            }
            Pending::Scratch => {
                let active = self.workspace.active;
                // Scratch sits after every main tab.
                if let Some(index) = self.workspace.panes.iter().position(|p| p.id == pane) {
                    let mut info = self.workspace.panes.remove(index);
                    info.title = SCRATCH_TITLE.into();
                    info.parked = false;
                    self.workspace.panes.push(info);
                }
                self.workspace.ui.scratch = Some(pane);
                self.store.set_pane(
                    pane,
                    PaneState {
                        scratch: true,
                        ..PaneState::default()
                    },
                );
                self.workspace.active = active;
                self.workspace.reflow();
                self.workspace.apply(&Action::Scratch)?;
            }
            Pending::Replace { close } => {
                self.claim(close, true);
                let request = self.request(Pending::Close);
                self.out.push(Request::Close {
                    request,
                    pane: close,
                });
            }
            Pending::Refill => {
                if self.workspace.active == 0 {
                    self.workspace.active = pane;
                }
            }
            Pending::Respawn { slot } => {
                if let Some(tab) = self.store.document.layout.get(slot).cloned() {
                    self.store.set_pane(
                        pane,
                        PaneState {
                            name: tab.name,
                            color: tab.color,
                            parked: tab.parked,
                            scratch: false,
                        },
                    );
                    self.slots.insert(pane, slot);
                    if let Some(index) = self.workspace.panes.iter().position(|p| p.id == pane) {
                        let info = self.workspace.panes.remove(index);
                        let target = self
                            .workspace
                            .panes
                            .iter()
                            .position(|p| {
                                Some(p.id) == self.workspace.ui.scratch
                                    || self.slots.get(&p.id).is_none_or(|s| *s > slot)
                            })
                            .unwrap_or(self.workspace.panes.len());
                        let info = self.info_for(pane, Some(&info));
                        self.workspace.panes.insert(target, info);
                    }
                }
            }
            _ => {}
        }
        self.workspace.reflow();
        self.persist();
        Ok(())
    }

    fn restored(
        &mut self,
        pane: PaneId,
        existing: bool,
        key: &str,
        rerun: bool,
        kind: bool,
        name: &str,
    ) {
        self.workspace
            .ui
            .closed_tabs
            .retain(|entry| entry.key != key);
        if !self.workspace.panes.iter().any(|p| p.id == pane) {
            return;
        }
        if existing
            && self
                .workspace
                .panes
                .iter()
                .any(|p| p.id == pane && p.parked)
        {
            let _ = self.workspace.apply(&Action::Restore(pane));
        }
        if self.workspace.ui.scratch_visible {
            let _ = self.workspace.apply(&Action::ScratchClose);
        }
        self.workspace.active = pane;
        self.workspace.reading = ReadingMode::Tabs(pane);
        self.workspace.ui.tab_scroll_anchor = None;
        if !existing {
            self.workspace.ui.config.view_modes[ui::geometry_bucket(self.workspace.cols)] =
                Some(true);
            if !kind && !rerun {
                self.notice = Some(format!("Reopened folder. Previous application: {name}"));
            }
        }
        self.workspace.ui.dialog = None;
        self.workspace.track_focus();
        self.persist();
    }

    /// Takes every displayed pane, as focusing a composed TUI did.
    pub fn take_control(&mut self) {
        self.controlling = true;
        self.dirty = true;
        let desired: Vec<_> = self.desired_sizes().into_keys().collect();
        for pane in desired {
            if self.leases.get(&pane) != Some(&self.client) {
                self.claim(pane, true);
            }
        }
    }

    fn claim(&mut self, pane: PaneId, takeover: bool) {
        if self.infos.get(&pane).is_some_and(|info| info.dead) {
            return;
        }
        if self.leases.get(&pane) == Some(&self.client) && !self.claims.contains_key(&pane) {
            return;
        }
        if self.claims.get(&pane) == Some(&true) || (self.claims.contains_key(&pane) && !takeover) {
            return;
        }
        self.out.push(Request::Claim { pane, takeover });
        self.claims.insert(pane, takeover);
        self.leases.insert(pane, self.client);
        self.sizes.remove(&pane);
        self.themed.remove(&pane);
        if let Some(size) = self.desired_sizes().get(&pane).copied() {
            self.out.push(Request::Resize {
                pane,
                cols: size.0,
                rows: size.1,
            });
            self.sizes.insert(pane, size);
        }
    }

    fn owned(&self, pane: PaneId) -> bool {
        self.leases.get(&pane) == Some(&self.client)
    }

    fn desired_sizes(&self) -> BTreeMap<PaneId, (u16, u16)> {
        self.workspace
            .terminal_sizes()
            .into_iter()
            .filter(|(id, _, _)| {
                self.infos.get(id).is_some_and(|info| !info.dead)
                    && self
                        .workspace
                        .panes
                        .iter()
                        .any(|p| p.id == *id && !p.parked)
            })
            .filter_map(|(id, cols, rows)| clamp_pane_size(cols, rows).map(|size| (id, size)))
            .collect()
    }

    /// Brings leases, sizes, themes, subscriptions and opt-ins in line with
    /// the local layout, then persists UI state.
    fn sync(&mut self) {
        if !self.synced {
            return;
        }
        let desired = self.desired_sizes();
        for (pane, size) in &desired {
            if self.owned(*pane) {
                if self.sizes.get(pane) != Some(size) {
                    self.out.push(Request::Resize {
                        pane: *pane,
                        cols: size.0,
                        rows: size.1,
                    });
                    self.sizes.insert(*pane, *size);
                }
            } else if self.controlling && !self.leases.contains_key(pane) {
                self.claim(*pane, false);
            }
        }
        if self.supports("controller-theme") {
            if let [Some(foreground), Some(background)] = self.workspace.ui.terminal_theme {
                let theme = [foreground, background];
                let owned: Vec<_> = self
                    .leases
                    .iter()
                    .filter(|(_, owner)| **owner == self.client)
                    .map(|(pane, _)| *pane)
                    .collect();
                for pane in owned {
                    if self.themed.get(&pane) != Some(&theme) {
                        self.out.push(Request::Theme {
                            pane,
                            foreground,
                            background,
                        });
                        self.themed.insert(pane, theme);
                    }
                }
            }
        }
        let wanted = self.wanted_subscriptions();
        if wanted != self.subscribed {
            self.out.push(Request::Subscribe {
                panes: wanted.clone(),
            });
            self.subscribed = wanted;
        }
        let host = self.supports("host-status")
            && self
                .workspace
                .ui
                .dialog
                .is_some_and(|d| d.kind == DialogKind::Status);
        if host != self.host {
            self.out.push(Request::HostStatus { enabled: host });
            self.host = host;
            if !host {
                self.workspace.ui.status = Default::default();
            }
        }
        self.persist();
    }

    /// Displayed panes, the active one first, within the stream's limits.
    fn wanted_subscriptions(&self) -> Vec<PaneId> {
        let active = self.workspace.active;
        let mut visible: Vec<PaneId> = self
            .workspace
            .layout()
            .iter()
            .filter(|view| view.body.cols > 0 && view.body.rows > 0)
            .map(|view| view.pane.id)
            .collect();
        visible.sort_by_key(|id| *id != active);
        let desired = self.desired_sizes();
        let mut wanted: Vec<PaneId> = Vec::new();
        let mut cells = 0;
        for id in visible {
            if wanted.contains(&id) {
                continue;
            }
            let size = self
                .screens
                .get(&id)
                .map(|s| usize::from(s.cols) * usize::from(s.rows))
                .or_else(|| {
                    desired
                        .get(&id)
                        .map(|(c, r)| usize::from(*c) * usize::from(*r))
                })
                .unwrap_or(0);
            if cells + size > MAX_SUBSCRIBED_CELLS {
                continue;
            }
            cells += size;
            wanted.push(id);
            if wanted.len() == MAX_SUBSCRIPTIONS {
                break;
            }
        }
        // A stable order, so an unchanged set never resubscribes.
        wanted.sort_unstable();
        wanted
    }

    /// Saves order, pane state, settings and the layout template.
    fn persist(&mut self) {
        if !self.synced && self.workspace.panes.is_empty() {
            return;
        }
        let order: Vec<_> = self.main_panes().map(|pane| pane.id).collect();
        self.store.set_order(order);
        let panes: Vec<_> = self
            .workspace
            .panes
            .iter()
            .map(|pane| {
                let scratch = self.is_scratch(pane.id);
                let name = self
                    .store
                    .pane(pane.id)
                    .and_then(|state| state.name.clone());
                (
                    pane.id,
                    PaneState {
                        name,
                        color: (pane.palette != default_palette(pane.id)).then_some(pane.palette),
                        parked: pane.parked && !scratch,
                        scratch,
                    },
                )
            })
            .collect();
        for (id, state) in panes {
            self.store.set_pane(id, state);
        }
        self.store.set_settings(self.workspace.ui.config.clone());
        // The template only changes once every pane's folder is known and no
        // template tab is still being respawned.
        if self.synced
            && !self
                .pending
                .values()
                .any(|p| matches!(p, Pending::Respawn { .. }))
            && self
                .main_panes()
                .all(|pane| self.sessions.contains_key(&pane.id))
        {
            let layout: Vec<_> = self
                .main_panes()
                .map(|pane| {
                    let session = &self.sessions[&pane.id];
                    LayoutTab {
                        cwd: if session.launch_cwd.is_empty() {
                            pane.cwd.clone()
                        } else {
                            session.launch_cwd.clone()
                        },
                        shell: (!session.shell.is_empty()).then(|| session.shell.clone()),
                        argv: session.argv.clone(),
                        name: self
                            .store
                            .pane(pane.id)
                            .and_then(|state| state.name.clone()),
                        color: Some(pane.palette),
                        parked: pane.parked,
                    }
                })
                .filter(|tab| std::path::Path::new(&tab.cwd).is_absolute())
                .collect();
            if !layout.is_empty() {
                self.store.set_layout(layout);
            }
        }
    }

    fn apply(&mut self, action: &Action) {
        if let Err(error) = self.workspace.apply(action) {
            self.notice = Some(error.to_string());
        }
    }

    /// Marks notifications the local workspace read (by focusing their
    /// pane) as acknowledged on the server.
    fn acknowledge_read(&mut self) {
        let read: Vec<_> = self
            .workspace
            .ui
            .notifications
            .iter()
            .filter(|n| n.read && !self.server_read.contains(&n.id) && !self.acked.contains(&n.id))
            .map(|n| n.id)
            .collect();
        for notification in read {
            self.out.push(Request::Acknowledge { notification });
            self.acked.insert(notification);
        }
    }

    /// Handles one routed input message. Errors become the notice.
    pub fn message(&mut self, message: ClientMessage) {
        self.dirty = true;
        let result = match message {
            ClientMessage::Input(bytes) => self.input(bytes, false),
            ClientMessage::Paste(bytes) => self.input(bytes, true),
            ClientMessage::Mouse(mouse) => self.mouse(mouse),
            ClientMessage::ThemeColor { background, rgb } => {
                self.workspace.ui.terminal_theme[usize::from(background)] = Some(rgb);
                Ok(())
            }
            ClientMessage::Action(action) => self.action(action),
            _ => Ok(()),
        };
        if let Err(error) = result {
            self.notice = Some(error.to_string());
        }
        self.acknowledge_read();
        self.persist();
    }

    fn input(&mut self, bytes: Vec<u8>, paste: bool) -> Result<()> {
        if bytes.len() > MAX_INPUT {
            return Err("input too large".into());
        }
        let pane = self.workspace.active;
        if pane == 0 || !self.workspace.panes.iter().any(|p| p.id == pane) {
            return Ok(());
        }
        // A focus-out report never claims a pane another client uses.
        if !paste && bytes == b"\x1b[O" && !self.owned(pane) {
            return Ok(());
        }
        self.selection = None;
        self.controlling = true;
        self.claim(pane, true);
        self.out.push(if paste {
            Request::Paste { pane, bytes }
        } else {
            Request::Input { pane, bytes }
        });
        Ok(())
    }

    fn mouse(&mut self, mouse: MouseEvent) -> Result<()> {
        if self.focus_click && matches!(mouse.kind, MouseKind::Move | MouseKind::Release) {
            if mouse.kind == MouseKind::Release {
                self.focus_click = false;
            }
            return Ok(());
        }
        if mouse.kind == MouseKind::Press {
            self.focus_click = false;
        }
        let Some(view) = self
            .workspace
            .layout()
            .into_iter()
            .rev()
            .find(|v| v.rect.contains(mouse.position))
        else {
            return Ok(());
        };
        let pane = view.pane.id;
        if matches!(
            mouse.kind,
            MouseKind::Press | MouseKind::WheelUp | MouseKind::WheelDown
        ) {
            self.controlling = true;
            self.claim(pane, true);
        } else if !self.owned(pane) {
            return Ok(());
        }
        if mouse.kind == MouseKind::Press {
            let focus_only =
                self.workspace.reading == ReadingMode::Grid && self.workspace.active != pane;
            self.workspace.apply(&Action::Focus(pane))?;
            if focus_only {
                self.focus_click = true;
                return Ok(());
            }
        }
        if !view.body.contains(mouse.position) {
            return Ok(());
        }
        let modes = self.screens.get(&pane).map(|s| s.modes).unwrap_or_default();
        if !mouse.shift && (modes.mouse_click || modes.mouse_drag || modes.mouse_motion) {
            let position = Point {
                x: mouse.position.x - view.body.x,
                y: mouse.position.y - view.body.y,
            };
            if self
                .screens
                .get(&pane)
                .is_some_and(|s| position.x < s.cols && position.y < s.rows)
            {
                self.out.push(Request::Mouse {
                    pane,
                    event: MouseEvent { position, ..mouse },
                });
            }
        } else if matches!(mouse.kind, MouseKind::WheelUp | MouseKind::WheelDown) {
            self.out.push(Request::Scroll {
                pane,
                lines: if mouse.kind == MouseKind::WheelUp {
                    3
                } else {
                    -3
                },
            });
        }
        Ok(())
    }

    fn action(&mut self, action: Action) -> Result<()> {
        match action {
            Action::Detach => self.detached = true,
            Action::Takeover => self.take_control(),
            Action::ClaimPane(pane) => {
                self.controlling = true;
                self.claim(pane, true);
            }
            Action::Shutdown => {
                let request = self.request(Pending::Shutdown);
                self.out.push(Request::Shutdown { request });
            }
            Action::Grid { .. } | Action::GridPicker | Action::ResizeGrid { .. } => {
                return Err("grid layout is automatic; use New Tab, Park or Close".into());
            }
            Action::AgentLinkPicker(_) | Action::AgentLinkChoose(_) => {
                return Err("the conversation picker is not available in the stream TUI".into());
            }
            Action::NewTab => {
                let grid =
                    self.workspace.reading == ReadingMode::Grid && self.workspace.active != 0;
                let request = self.request(Pending::NewTab { grid });
                self.out.push(Request::Spawn {
                    request,
                    cwd: None,
                    argv: Vec::new(),
                });
            }
            Action::Scratch => {
                let live = self.workspace.ui.scratch.filter(|id| {
                    self.workspace
                        .panes
                        .iter()
                        .any(|pane| pane.id == *id && !pane.dead)
                });
                if live.is_some() {
                    self.workspace.apply(&Action::Scratch)?;
                } else if !self.pending.values().any(|p| *p == Pending::Scratch) {
                    if let Some(id) = self.workspace.ui.scratch.take() {
                        // The old scratch exited; forget it locally.
                        self.workspace.ui.scratch_visible = false;
                        self.workspace.panes.retain(|pane| pane.id != id);
                    }
                    let request = self.request(Pending::Scratch);
                    self.out.push(Request::Spawn {
                        request,
                        cwd: None,
                        argv: Vec::new(),
                    });
                }
            }
            Action::ClosePaneConfirm(pane) => {
                let dialog = self
                    .workspace
                    .ui
                    .dialog
                    .ok_or("closing a pane requires confirmation")?;
                if dialog.kind != DialogKind::CloseConfirm || dialog.source != Some(pane) {
                    return Err("closing a pane requires its confirmation dialog".into());
                }
                if !self.workspace.panes.iter().any(|p| p.id == pane) {
                    return Err("unknown pane".into());
                }
                self.workspace.ui.dialog = None;
                self.controlling = true;
                let last = !self.is_scratch(pane) && self.main_panes().count() == 1;
                if last {
                    // Create the replacement first: a failed spawn must not
                    // leave the workspace without a tab.
                    let request = self.request(Pending::Replace { close: pane });
                    self.out.push(Request::Spawn {
                        request,
                        cwd: None,
                        argv: Vec::new(),
                    });
                } else {
                    self.claim(pane, true);
                    let request = self.request(Pending::Close);
                    self.out.push(Request::Close { request, pane });
                }
            }
            Action::Acknowledge(notification) => {
                self.out.push(Request::Acknowledge { notification });
                self.acked.insert(notification);
                self.workspace.apply(&Action::Acknowledge(notification))?;
            }
            Action::AcknowledgeAll => {
                self.out.push(Request::AcknowledgeAll);
                for notification in &self.workspace.ui.notifications {
                    self.acked.insert(notification.id);
                }
                self.workspace.apply(&Action::AcknowledgeAll)?;
            }
            Action::Recovery | Action::ReopenLast => {
                let reopen = action == Action::ReopenLast;
                self.workspace.ui.closed_tabs.clear();
                self.workspace.ui.recovery_loading = true;
                let request = self.request(Pending::ClosedSessions { reopen });
                self.out.push(Request::ClosedSessions { request });
                self.workspace.apply(&Action::Recovery)?;
            }
            Action::RecoveryCopy(index) => {
                let command = self
                    .workspace
                    .ui
                    .closed_tabs
                    .get(index)
                    .and_then(|item| item.command())
                    .ok_or("this session has no known resume command")?;
                self.clipboard.push(command);
            }
            Action::ReopenTab(index) | Action::RerunTab(index) => {
                if !self.workspace.ui.dialog.is_some_and(|d| {
                    d.kind == DialogKind::Recovery
                        || (d.kind == DialogKind::ReopenConfirm && d.source == Some(index as u64))
                }) {
                    return Err("select a Closed entry before restoring a tab".into());
                }
                let entry = self
                    .workspace
                    .ui
                    .closed_tabs
                    .get(index)
                    .cloned()
                    .ok_or("unknown closed tab")?;
                let rerun = matches!(action, Action::RerunTab(_));
                let request = self.request(Pending::Restore {
                    key: entry.key.clone(),
                    rerun,
                    kind: entry.kind.is_some(),
                    name: entry.name.clone(),
                });
                self.out.push(Request::Restore {
                    request,
                    record: entry.key,
                    rerun,
                });
            }
            Action::SearchCopy(index) => {
                let text = self
                    .workspace
                    .ui
                    .search_hits
                    .get(index)
                    .ok_or("unknown search result")?
                    .text
                    .clone();
                self.clipboard.push(text);
            }
            Action::Search { ref query, all } => {
                if query.len() > 256 || query.chars().any(char::is_control) {
                    return Err(
                        "search requires at most 256 bytes without control characters".into(),
                    );
                }
                self.workspace.ui.search_hits.clear();
                if !query.is_empty() {
                    let active = self
                        .workspace
                        .ui
                        .scratch_return
                        .filter(|_| self.workspace.ui.scratch_visible)
                        .unwrap_or(self.workspace.active);
                    let panes = if all || active == 0 {
                        Vec::new()
                    } else {
                        vec![active]
                    };
                    let request = self.request(Pending::Search);
                    self.out.push(Request::Search {
                        request,
                        panes,
                        query: query.clone(),
                        limit: 64,
                    });
                }
                self.workspace.apply(&action)?;
            }
            Action::SearchSelect(index) => {
                let hit = self
                    .workspace
                    .ui
                    .search_hits
                    .get(index)
                    .ok_or("unknown search result")?
                    .clone();
                if !self.workspace.panes.iter().any(|p| p.id == hit.pane) {
                    return Err("search pane has closed".into());
                }
                self.workspace.apply(&Action::Focus(hit.pane))?;
                self.workspace.apply(&Action::DialogClose)?;
                self.controlling = true;
                self.claim(hit.pane, true);
                self.out.push(Request::Reveal {
                    pane: hit.pane,
                    line: hit.line,
                });
            }
            Action::Scroll { pane, lines } => {
                self.workspace.apply(&action)?;
                self.controlling = true;
                self.claim(pane, true);
                self.out.push(Request::Scroll {
                    pane,
                    lines: lines.clamp(-10000, 10000),
                });
            }
            Action::SelectText { pane, start, end } => {
                self.workspace.apply(&action)?;
                self.selection = Some(Selection { pane, start, end });
            }
            Action::SelectionStart => {
                let pane = self.workspace.active;
                self.workspace.apply(&action)?;
                let point = self
                    .screens
                    .get(&pane)
                    .and_then(|s| s.cursor)
                    .map(|c| c.position)
                    .unwrap_or_default();
                self.selection = Some(Selection {
                    pane,
                    start: point,
                    end: point,
                });
            }
            Action::SelectionMove { dx, dy } => {
                let selection = self
                    .selection
                    .filter(|s| s.pane == self.workspace.active)
                    .ok_or("no selection")?;
                let screen = self.screens.get(&selection.pane).ok_or("no active pane")?;
                let end = Point {
                    x: (i32::from(selection.end.x) + i32::from(dx))
                        .clamp(0, i32::from(screen.cols) - 1) as u16,
                    y: (i32::from(selection.end.y) + i32::from(dy))
                        .clamp(0, i32::from(screen.rows) - 1) as u16,
                };
                self.selection = Some(Selection { end, ..selection });
            }
            Action::Copy(pane) => {
                self.workspace.apply(&action)?;
                if let Some(selection) = self.selection.filter(|s| s.pane == pane) {
                    let screen = self.screens.get(&pane).ok_or("pane is not displayed")?;
                    let text = selected_text(screen, selection.start, selection.end)?;
                    if !text.is_empty() {
                        let mut end = text.len().min(MAX_COPY);
                        while !text.is_char_boundary(end) {
                            end -= 1;
                        }
                        self.clipboard.push(text[..end].to_owned());
                    }
                }
            }
            Action::Focus(_) | Action::Read(_) | Action::Select(_) | Action::Fullscreen(_) => {
                self.workspace.apply(&action)?;
                self.selection = None;
            }
            other => self.workspace.apply(&other)?,
        }
        Ok(())
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.workspace.resize(cols, rows)?;
        self.dirty = true;
        Ok(())
    }

    /// Advances UI animation; marks the frame dirty when it changes.
    pub fn tick(&mut self, now: Instant) {
        if now.duration_since(self.animation_at) >= Duration::from_millis(ui::ANIMATION_TICK_MS) {
            let previous = self.spinner;
            self.spinner += 1;
            self.animation_at = now;
            let changed = [1, 3].into_iter().any(|width| {
                ui::activity_phase(previous, width) != ui::activity_phase(self.spinner, width)
            });
            self.dirty |= changed
                && self
                    .workspace
                    .panes
                    .iter()
                    .any(|p| p.status == Status::Working);
        }
        if now.duration_since(self.title_at) >= Duration::from_millis(100) {
            self.title_at = now;
            let ids: BTreeSet<_> = self.workspace.panes.iter().map(|p| p.id).collect();
            self.title_animation.retain(|id, _| ids.contains(id));
            self.workspace
                .ui
                .title_ticks
                .retain(|id, _| ids.contains(id));
            for pane in &self.workspace.panes {
                let entry = self
                    .title_animation
                    .entry(pane.id)
                    .or_insert_with(|| (pane.title.clone(), 0));
                if entry.0 != pane.title {
                    *entry = (pane.title.clone(), 0);
                }
                let stop = (pane.title.chars().count().min(512) as u16) * 3 + 32;
                if entry.1 <= stop {
                    entry.1 += 1;
                    self.workspace.ui.title_ticks.insert(pane.id, entry.1);
                    self.dirty = true;
                }
            }
        }
    }

    /// Composes the screen: the moved composed renderer, plus the local
    /// text selection.
    pub fn frame(&self) -> Frame {
        let mut frame = render::compose(
            &self.workspace,
            &self.screens,
            self.controlling,
            self.notice.as_deref(),
            self.spinner,
        );
        if let Some(selection) = self.selection {
            if let Some(body) = frame
                .panes
                .iter()
                .rev()
                .find(|v| v.pane.id == selection.pane && v.body.cols > 0)
                .map(|v| v.body)
            {
                highlight(&mut frame, body, selection);
            }
        }
        frame
    }
}

fn ordered(start: Point, end: Point) -> (Point, Point) {
    if (start.y, start.x) <= (end.y, end.x) {
        (start, end)
    } else {
        (end, start)
    }
}

fn highlight(frame: &mut Frame, body: Rect, selection: Selection) {
    let (start, end) = ordered(selection.start, selection.end);
    for y in start.y..=end.y.min(body.rows.saturating_sub(1)) {
        let x0 = if y == start.y { start.x } else { 0 };
        let x1 = if y == end.y {
            end.x
        } else {
            body.cols.saturating_sub(1)
        };
        for x in x0..=x1.min(body.cols.saturating_sub(1)) {
            let index = usize::from(body.y + y) * usize::from(frame.cols) + usize::from(body.x + x);
            if let Some(cell) = frame.cells.get_mut(index) {
                cell.bg = Color::Indexed(24);
                cell.fg = Color::Indexed(231);
            }
        }
    }
}

/// The text of a selection on one pane screen, one line per row, trailing
/// blanks trimmed. Wide-glyph continuations contribute no text.
pub fn selected_text(screen: &Screen, start: Point, end: Point) -> Result<String> {
    let (mut start, end) = ordered(start, end);
    if start.x >= screen.cols
        || start.y >= screen.rows
        || end.x >= screen.cols
        || end.y >= screen.rows
    {
        return Err("selection outside screen".into());
    }
    let cols = usize::from(screen.cols);
    if start.x > 0 && screen.cells[usize::from(start.y) * cols + usize::from(start.x)].width == 0 {
        start.x -= 1;
    }
    let mut lines = Vec::new();
    for y in start.y..=end.y {
        let x0 = if y == start.y { start.x } else { 0 };
        let x1 = if y == end.y { end.x } else { screen.cols - 1 };
        let mut line = String::new();
        for x in x0..=x1 {
            let cell = &screen.cells[usize::from(y) * cols + usize::from(x)];
            if cell.width > 0 {
                line.push_str(&cell.text);
            }
        }
        lines.push(line.trim_end().to_owned());
    }
    Ok(lines.join("\n"))
}

/// Applies a stream patch in place, with the checks of `stream::apply`.
fn apply_patch(screen: &mut Screen, revision: &mut u64, patch: &PanePatch) -> Result<()> {
    if patch.base != *revision
        || patch.base.checked_add(1) != Some(patch.revision)
        || patch.changes.len() > screen.cells.len()
        || patch
            .cursor
            .is_some_and(|c| c.position.x >= screen.cols || c.position.y >= screen.rows)
    {
        return Err("invalid pane patch".into());
    }
    let cols = usize::from(screen.cols);
    let mut seen = BTreeSet::new();
    for (index, cell) in &patch.changes {
        if *index >= screen.cells.len()
            || !seen.insert(*index)
            || !kelpclient::cell_valid(cell)
            || (cell.width == 2 && index % cols + 1 == cols)
        {
            return Err("invalid pane patch cell".into());
        }
    }
    for (index, cell) in &patch.changes {
        screen.cells[*index] = cell.clone();
    }
    screen.cursor = patch.cursor;
    screen.modes = patch.modes;
    *revision = patch.revision;
    Ok(())
}
