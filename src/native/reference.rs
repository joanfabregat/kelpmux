//! Small typed reference client: validates epoch, snapshots and patch baselines.
use super::{
    protocol::*,
    stream::{self, Event, PaneSnapshot},
};
use crate::Result;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

#[derive(Debug, Default, Clone, Serialize)]
pub struct State {
    pub epoch: String,
    pub client: u64,
    pub capabilities: Vec<String>,
    pub subscribed: Vec<PaneId>,
    pub screens: BTreeMap<PaneId, PaneSnapshot>,
    pub panes: Vec<PaneInfo>,
    pub leases: Vec<stream::Lease>,
    pub notifications: Vec<Notification>,
    pub vitals: super::vitals::Vitals,
}
impl State {
    /// Changes are transactional: a bad patch never corrupts a previous baseline.
    pub fn apply(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Welcome {
                version,
                client,
                epoch,
                capabilities,
            } => {
                if !self.epoch.is_empty()
                    || version != stream::VERSION
                    || client == 0
                    || epoch.is_empty()
                    || epoch.len() > 128
                    || epoch.chars().any(char::is_control)
                    || capabilities.len() > 32
                    || capabilities
                        .iter()
                        .any(|c| c.len() > 64 || c.chars().any(char::is_control))
                {
                    return Err("invalid or repeated native stream welcome".into());
                }
                self.epoch = epoch;
                self.client = client;
                self.capabilities = capabilities;
            }
            Event::Update(update) => {
                if self.epoch.is_empty() || update.epoch != self.epoch {
                    return Err("native stream epoch changed; reconnect for a new snapshot".into());
                }
                let wanted: BTreeSet<_> = update.subscribed.iter().copied().collect();
                let panes: BTreeSet<_> = update.panes.iter().map(|p| p.id).collect();
                if wanted.len() != update.subscribed.len()
                    || wanted.len() > stream::MAX_SUBSCRIPTIONS
                    || panes.len() != update.panes.len()
                    || panes.contains(&0)
                    || !wanted.is_subset(&panes)
                    || update.snapshots.len() > 10
                    || update.patches.len() > 10
                    || update.leases.len() > 10
                    || update.notifications.len() > 256
                {
                    return Err("invalid native stream metadata bounds".into());
                }
                let mut next = self.screens.clone();
                next.retain(|id, _| wanted.contains(id));
                let mut changed = BTreeSet::new();
                for snapshot in update.snapshots {
                    stream::validate(&snapshot)?;
                    if !wanted.contains(&snapshot.pane) || !changed.insert(snapshot.pane) {
                        return Err("duplicate or unsubscribed native snapshot".into());
                    }
                    next.insert(snapshot.pane, snapshot);
                }
                for patch in update.patches {
                    if !wanted.contains(&patch.pane) || !changed.insert(patch.pane) {
                        return Err("duplicate or unsubscribed native patch".into());
                    }
                    let old = next
                        .get(&patch.pane)
                        .ok_or("native patch without a snapshot")?;
                    next.insert(patch.pane, stream::apply(old, &patch)?);
                }
                if wanted.iter().any(|id| !next.contains_key(id))
                    || next.values().map(|s| s.screen.cells.len()).sum::<usize>()
                        > stream::MAX_SUBSCRIBED_CELLS
                {
                    return Err("native stream missing baseline or exceeding cell budget".into());
                }
                let mut leases = BTreeSet::new();
                if update
                    .leases
                    .iter()
                    .any(|l| l.owner == 0 || !panes.contains(&l.pane) || !leases.insert(l.pane))
                {
                    return Err("invalid native stream leases".into());
                }
                self.screens = next;
                self.subscribed = update.subscribed;
                self.panes = update.panes;
                self.leases = update.leases;
                self.notifications = update.notifications;
                self.vitals = update.vitals;
            }
            Event::Lease { pane, owner } => {
                if self.epoch.is_empty()
                    || pane == 0
                    || owner == Some(0)
                    || !self.panes.is_empty() && !self.panes.iter().any(|info| info.id == pane)
                    || owner.is_some()
                        && !self.leases.iter().any(|lease| lease.pane == pane)
                        && self.leases.len() >= 10
                {
                    return Err("invalid native lease event".into());
                }
                self.leases.retain(|l| l.pane != pane);
                if let Some(owner) = owner {
                    self.leases.push(stream::Lease { pane, owner });
                }
            }
            Event::Error { message } => return Err(message.into()),
        }
        Ok(())
    }
}
/// Read-only, one-shot reference inspection. Does not claim, resize, attach a TUI,
/// start a server, or persist terminal contents.
pub fn inspect(mut socket: UnixStream, panes: Vec<PaneId>) -> Result<State> {
    if panes.len() > stream::MAX_SUBSCRIPTIONS
        || panes.iter().copied().collect::<BTreeSet<_>>().len() != panes.len()
    {
        return Err("invalid inspection subscriptions".into());
    }
    let deadline = Instant::now() + Duration::from_secs(8);
    socket.set_write_timeout(Some(Duration::from_secs(5)))?;
    write_message(
        &mut socket,
        &ClientMessage::Stream(stream::Request::Hello {
            version: stream::VERSION,
        }),
    )?;
    let mut state = State::default();
    match receive(&mut socket, deadline)? {
        Some(ServerMessage::Stream(event @ Event::Welcome { .. })) => state.apply(event)?,
        Some(ServerMessage::Error(message)) => return Err(message.into()),
        _ => return Err("workspace does not support native streams".into()),
    }
    write_message(
        &mut socket,
        &ClientMessage::Stream(stream::Request::Subscribe {
            panes: panes.clone(),
        }),
    )?;
    loop {
        if Instant::now() >= deadline {
            return Err("native inspection timed out".into());
        }
        match receive(&mut socket, deadline)? {
            Some(ServerMessage::Stream(event)) => state.apply(event)?,
            Some(ServerMessage::Error(message)) => return Err(message.into()),
            _ => return Err("native inspection disconnected".into()),
        }
        if state.subscribed.iter().copied().collect::<BTreeSet<_>>()
            == panes.iter().copied().collect::<BTreeSet<_>>()
            && !state.panes.is_empty()
            && panes.iter().all(|id| state.screens.contains_key(id))
        {
            return Ok(state);
        }
    }
}

fn receive(socket: &mut UnixStream, deadline: Instant) -> Result<Option<ServerMessage>> {
    struct Reader<'a> {
        socket: &'a mut UnixStream,
        deadline: Instant,
    }
    impl std::io::Read for Reader<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "native inspection timed out",
                ));
            }
            self.socket.set_read_timeout(Some(remaining))?;
            std::io::Read::read(self.socket, buffer)
        }
    }
    read_message(&mut Reader { socket, deadline })
}
