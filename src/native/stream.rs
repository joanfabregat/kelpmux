//! Experimental UI-independent pane streams. SSH transports these framed messages unchanged.
//!
//! Send `Hello { version: 1 }` first, then `Subscribe` (replacement set, at most ten
//! unique current panes). Welcome's random server epoch scopes all pane IDs. A new
//! epoch invalidates every saved baseline/lease, even if numerical IDs repeat.
//! Every Update includes current bounded metadata, notifications and structured vitals.
//! A snapshot starts revision 1; patches advance exactly one revision and require
//! the exact delivered baseline. Geometry changes, explicit Resync and backpressure
//! produce replacement snapshots. Clients must never apply a patch to another epoch.
//!
//! Claim/Resize/Input/Paste/Mouse address one pane. Claim is exclusive, explicit
//! takeover may replace an owner, and release/disconnect restores the current base
//! workspace geometry. Leases have connection lifetime (no time-based expiry).
//! The ordinary TUI retains its independent workspace controller; leased panes reject
//! its input and ignore its geometry until explicitly reclaimed with ClaimPane.
//!
//! At most 100,000 cells are subscribed per connection. A later resize exceeding
//! that aggregate budget disconnects that subscriber rather than accumulating state.
//! Server retains one in-flight packet and one replaceable aggregate update; excess
//! reliable responses disconnect a slow peer. Notifications are the latest 128
//! retained records, not an exactly-once event log; acknowledgement is explicit.
//! History paging, tab lifecycle commands and device/MCP capabilities are deferred.
use super::protocol::{
    Cell, Cursor, InputModes, MouseEvent, Notification, PaneId, PaneInfo, Screen,
};
use crate::Result;
use serde::{Deserialize, Serialize};

pub const VERSION: u16 = 1;
pub const MAX_SUBSCRIPTIONS: usize = 10;
/// Pane geometry is independent of the composed TUI's minimum window size.
pub fn validate_size(cols: u16, rows: u16) -> Result<()> {
    if !(2..=400).contains(&cols) || !(1..=160).contains(&rows) {
        return Err("pane size must be 2–400 columns and 1–160 rows".into());
    }
    Ok(())
}
/// Bound both retained screen baselines and aggregate update serialization.
pub const MAX_SUBSCRIBED_CELLS: usize = 100_000;
pub const CAPABILITIES: &[&str] = &[
    "pane-snapshots",
    "pane-patches",
    "pane-leases",
    "metadata",
    "notifications",
    "vitals",
    "hyperlinks",
    "program-status",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Hello { version: u16 },
    Subscribe { panes: Vec<PaneId> },
    Claim { pane: PaneId, takeover: bool },
    Release { pane: PaneId },
    Resize { pane: PaneId, cols: u16, rows: u16 },
    Input { pane: PaneId, bytes: Vec<u8> },
    Paste { pane: PaneId, bytes: Vec<u8> },
    Mouse { pane: PaneId, event: MouseEvent },
    Resync { pane: PaneId },
    Acknowledge { notification: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSnapshot {
    pub pane: PaneId,
    pub revision: u64,
    pub screen: Screen,
}

/// Check a full pane screen before allocating a renderer's backing surface.
pub fn validate(snapshot: &PaneSnapshot) -> Result<()> {
    let screen = &snapshot.screen;
    if snapshot.pane == 0
        || snapshot.revision == 0
        || !(1..=400).contains(&screen.cols)
        || !(1..=160).contains(&screen.rows)
        || screen.cells.len() != usize::from(screen.cols) * usize::from(screen.rows)
        || screen.cursor.is_some_and(|cursor| {
            cursor.position.x >= screen.cols || cursor.position.y >= screen.rows
        })
        || screen.cells.iter().enumerate().any(|(index, cell)| {
            !super::delta::cell_valid(cell)
                || cell.width == 2
                    && index % usize::from(screen.cols) + 1 == usize::from(screen.cols)
        })
    {
        return Err("invalid pane snapshot".into());
    }
    Ok(())
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanePatch {
    pub pane: PaneId,
    pub base: u64,
    pub revision: u64,
    pub cursor: Option<Cursor>,
    pub modes: InputModes,
    pub changes: Vec<(usize, Cell)>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub pane: PaneId,
    pub owner: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Update {
    pub epoch: String,
    pub subscribed: Vec<PaneId>,
    pub snapshots: Vec<PaneSnapshot>,
    pub patches: Vec<PanePatch>,
    pub panes: Vec<PaneInfo>,
    pub leases: Vec<Lease>,
    pub notifications: Vec<Notification>,
    pub vitals: super::vitals::Vitals,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Welcome {
        version: u16,
        client: u64,
        epoch: String,
        capabilities: Vec<String>,
    },
    Update(Update),
    Lease {
        pane: PaneId,
        owner: Option<u64>,
    },
    Error {
        message: String,
    },
}

/// Apply only to the exact baseline. A full snapshot starts a fresh revision chain.
pub fn apply(snapshot: &PaneSnapshot, patch: &PanePatch) -> Result<PaneSnapshot> {
    validate(snapshot)?;
    if patch.pane != snapshot.pane
        || patch.base != snapshot.revision
        || patch.base.checked_add(1) != Some(patch.revision)
        || patch.changes.len() > snapshot.screen.cells.len()
    {
        return Err("invalid pane patch baseline".into());
    }
    if patch.cursor.is_some_and(|cursor| {
        cursor.position.x >= snapshot.screen.cols || cursor.position.y >= snapshot.screen.rows
    }) {
        return Err("invalid pane patch cursor".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for (index, cell) in &patch.changes {
        if *index >= snapshot.screen.cells.len()
            || !seen.insert(*index)
            || !super::delta::cell_valid(cell)
        {
            return Err("invalid pane patch cell".into());
        }
    }
    let mut next = snapshot.clone();
    for (index, cell) in &patch.changes {
        next.screen.cells[*index] = cell.clone();
    }
    next.revision = patch.revision;
    next.screen.cursor = patch.cursor;
    next.screen.modes = patch.modes;
    validate(&next)?;
    Ok(next)
}

pub fn diff(old: &PaneSnapshot, screen: &Screen) -> Option<PanePatch> {
    if old.screen.cols != screen.cols
        || old.screen.rows != screen.rows
        || old.screen.cells.len() != screen.cells.len()
    {
        return None;
    }
    let changes: Vec<_> = old
        .screen
        .cells
        .iter()
        .zip(&screen.cells)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(index, (_, cell))| (index, cell.clone()))
        .collect();
    // A sparse patch is useful; dense changes are cheaper and simpler as snapshots.
    if changes.len() > screen.cells.len() / 4 {
        return None;
    }
    Some(PanePatch {
        pane: old.pane,
        base: old.revision,
        revision: old.revision.checked_add(1)?,
        cursor: screen.cursor,
        modes: screen.modes,
        changes,
    })
}
