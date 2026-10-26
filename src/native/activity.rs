//! Durable metadata notifications. One workspace server owns this file; no journal lock.
use super::{
    protocol::{Notification, PaneId, PaneInfo},
    tracking,
    ui::AgentFilter,
};
use crate::{
    Result, journal,
    model::{Kind, Status},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
const MAX_EVENTS: usize = 256;
const MAX_BYTES: u64 = 512 * 1024;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Seen {
    #[serde(default)]
    run: String,
    pane: PaneId,
    pid: u32,
    kind: Option<Kind>,
    session_id: Option<String>,
    title: String,
    status: Status,
}
impl From<&PaneInfo> for Seen {
    fn from(pane: &PaneInfo) -> Self {
        Self {
            run: String::new(),
            pane: pane.id,
            pid: pane.pid,
            kind: pane.kind,
            session_id: pane.session_id.clone().filter(|id| crate::session_id(id)),
            title: clean(&pane.title),
            status: pane.status,
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Persisted {
    initialized: bool,
    next_id: u64,
    events: Vec<Notification>,
    seen: BTreeMap<PaneId, Seen>,
}
pub struct Activity {
    run: String,
    path: PathBuf,
    workspace: String,
    data: Persisted,
}
fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(128).collect()
}
fn noteworthy(status: Status) -> bool {
    matches!(
        status,
        Status::InputNeeded | Status::TurnFinished | Status::Failed | Status::Exited
    )
}
fn same_identity(old: &Seen, new: &Seen) -> bool {
    old.pane == new.pane
        && old.pid == new.pid
        && old.kind == new.kind
        && (old.session_id == new.session_id
            || old.session_id.is_none()
            || new.session_id.is_none())
}
fn prune(data: &mut Persisted, time: u64) -> bool {
    let before = data.events.len();
    data.events
        .retain(|event| event.at >= time.saturating_sub(journal::RETENTION));
    if data.events.len() > MAX_EVENTS {
        data.events.drain(..data.events.len() - MAX_EVENTS);
    }
    before != data.events.len()
}
fn emit(
    data: &mut Persisted,
    workspace: &str,
    pane: &Seen,
    status: Status,
    time: u64,
    title: Option<String>,
) -> Result<()> {
    let id = data.next_id.max(1);
    data.next_id = id
        .checked_add(1)
        .ok_or("notification identifier exhausted")?;
    data.events.push(Notification {
        origin: (!pane.run.is_empty()).then(|| super::protocol::NotificationOrigin {
            run: pane.run.clone(),
            pid: pane.pid,
        }),
        id,
        pane: pane.pane,
        title: title.unwrap_or_else(|| pane.title.clone()),
        status,
        at: time,
        read: false,
        workspace: workspace.into(),
    });
    Ok(())
}
impl Activity {
    pub fn load(state: &Path, workspace: &str) -> Result<Self> {
        if workspace.is_empty() || workspace.len() > 128 || workspace.chars().any(char::is_control)
        {
            return Err("invalid notification workspace name".into());
        }
        super::lifecycle::private_state(state)?;
        let path = state.join("activity.json");
        let mut data = match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Persisted::default(),
            Err(error) => return Err(error.into()),
            Ok(meta) => {
                if !meta.is_file()
                    || meta.uid() != unsafe { libc::geteuid() }
                    || meta.mode() & 0o077 != 0
                    || meta.len() > MAX_BYTES
                {
                    return Err(
                        "notification history must be a bounded private regular file".into(),
                    );
                }
                let file = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(&path)?;
                let meta = file.metadata()?;
                if !meta.is_file()
                    || meta.uid() != unsafe { libc::geteuid() }
                    || meta.mode() & 0o077 != 0
                {
                    return Err("invalid notification history file".into());
                }
                let mut bytes = Vec::new();
                file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_BYTES {
                    return Err("notification history exceeds size limit".into());
                }
                serde_json::from_slice::<Persisted>(&bytes)?
            }
        };
        if data.events.len() > MAX_EVENTS {
            return Err("notification history exceeds entry limit".into());
        }
        let mut ids = BTreeSet::new();
        for event in &mut data.events {
            if event.id == 0 || !ids.insert(event.id) {
                return Err("invalid notification identifier".into());
            }
            event.title = clean(&event.title);
            event.workspace = clean(&event.workspace);
        }
        for seen in data.seen.values_mut() {
            seen.title = clean(&seen.title);
            seen.session_id = seen.session_id.take().filter(|id| crate::session_id(id));
        }
        data.next_id = data
            .next_id
            .max(
                ids.last()
                    .copied()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or("notification identifier exhausted")?,
            )
            .max(1);
        if prune(&mut data, crate::now()) {
            journal::atomic(&path, &data)?;
        }
        Ok(Self {
            run: fs::read_to_string("/proc/sys/kernel/random/uuid")?
                .trim()
                .to_owned(),
            path,
            workspace: workspace.into(),
            data,
        })
    }
    /// Initial observations establish a baseline; reconnecting loads the existing baseline.
    pub fn observe(&mut self, panes: &[PaneInfo], time: u64) -> Result<bool> {
        let mut current = BTreeMap::new();
        for pane in panes {
            let mut seen = Seen::from(pane);
            seen.run = self.run.clone();
            if pane.id == 0 || current.insert(pane.id, seen).is_some() {
                return Err("invalid or duplicate pane observation".into());
            }
        }
        // Missing metadata is not evidence of a different conversation.
        for seen in current.values_mut() {
            if let Some(old) = self.data.seen.get(&seen.pane) {
                if same_identity(old, seen) && seen.session_id.is_none() {
                    seen.session_id = old.session_id.clone();
                }
            }
        }
        let mut data = self.data.clone();
        let mut sounded = false;
        if data.initialized {
            for old in self
                .data
                .seen
                .values()
                .filter(|old| old.kind.is_some() && old.status != Status::Exited)
            {
                if !current
                    .get(&old.pane)
                    .is_some_and(|new| same_identity(old, new))
                {
                    emit(&mut data, &self.workspace, old, Status::Exited, time, None)?;
                    sounded = true;
                }
            }
            for new in current
                .values()
                .filter(|new| new.kind.is_some() && noteworthy(new.status))
            {
                let known = self
                    .data
                    .seen
                    .get(&new.pane)
                    .is_some_and(|old| old.kind.is_some() && same_identity(old, new));
                if !known && matches!(new.status, Status::TurnFinished | Status::Exited) {
                    // A first discovery of an already finished session is baseline
                    // metadata, not evidence that a new turn just completed.
                    continue;
                }
                let repeated = self
                    .data
                    .seen
                    .get(&new.pane)
                    .is_some_and(|old| same_identity(old, new) && old.status == new.status);
                if !repeated {
                    emit(&mut data, &self.workspace, new, new.status, time, None)?;
                    sounded = true;
                }
            }
        }
        let changed = !data.initialized
            || data.seen.len() != current.len()
            || current.iter().any(|(id, new)| {
                data.seen.get(id).is_none_or(|old| {
                    !same_identity(old, new)
                        || old.session_id != new.session_id
                        || old.status != new.status
                })
            });
        data.initialized = true;
        data.seen = current;
        let pruned = prune(&mut data, time);
        if changed || pruned || sounded {
            journal::atomic(&self.path, &data)?;
        }
        self.data = data;
        Ok(sounded)
    }
    /// Terminal bells are explicit events, distinct from inferred agent questions.
    pub fn bell(&mut self, pane: &PaneInfo, time: u64) -> Result<bool> {
        if pane.kind.is_none() {
            return Ok(false);
        }
        if self.data.events.iter().rev().any(|event| {
            event.pane == pane.id
                && event.status == Status::Unknown
                && event.title.starts_with("Bell · ")
                && time.saturating_sub(event.at) < 2
        }) {
            return Ok(false);
        }
        let mut data = self.data.clone();
        let mut seen = Seen::from(pane);
        seen.run = self.run.clone();
        emit(
            &mut data,
            &self.workspace,
            &seen,
            Status::Unknown,
            time,
            Some(format!("Bell · {}", clean(&pane.title))),
        )?;
        prune(&mut data, time);
        journal::atomic(&self.path, &data)?;
        self.data = data;
        Ok(true)
    }
    pub fn acknowledge(&mut self, id: u64) -> Result<()> {
        let mut data = self.data.clone();
        let event = data
            .events
            .iter_mut()
            .find(|event| event.id == id)
            .ok_or("unknown notification identifier")?;
        if !event.read {
            event.read = true;
            journal::atomic(&self.path, &data)?;
            self.data = data;
        }
        Ok(())
    }
    pub fn snapshots(&self) -> Vec<Notification> {
        self.data.events.iter().rev().take(128).cloned().collect()
    }
    pub fn all(&self) -> &[Notification] {
        &self.data.events
    }
    pub fn run(&self) -> &str {
        &self.run
    }
    pub fn unread(&self) -> usize {
        self.data.events.iter().filter(|event| !event.read).count()
    }
    pub fn next(&self, panes: &[PaneInfo], filter: AgentFilter, active: PaneId) -> Option<PaneId> {
        next(panes, filter, active)
    }
}
/// Cycle logical pane order, independent of view geometry, excluding ordinary shells.
pub fn next(panes: &[PaneInfo], filter: AgentFilter, active: PaneId) -> Option<PaneId> {
    let panes: Vec<_> = tracking::filtered(panes, filter)
        .into_iter()
        .filter(|pane| pane.kind.is_some())
        .collect();
    if panes.is_empty() {
        return None;
    }
    let index = panes
        .iter()
        .position(|pane| pane.id == active)
        .map_or(0, |index| (index + 1) % panes.len());
    Some(panes[index].id)
}

/// A numeric pane ID alone can point at a different shell after a restart.
pub fn matches(event: &Notification, pane: &PaneInfo, run: &str) -> bool {
    event.origin.as_ref().is_some_and(|origin| {
        !run.is_empty()
            && origin.run == run
            && origin.pid == pane.pid
            && event.pane == pane.id
            && !pane.dead
    })
}
