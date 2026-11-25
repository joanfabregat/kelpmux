//! Restorable tab metadata. A historical entry never represents a live PTY.
use super::protocol::PaneInfo;
use crate::{Result, journal, model::Kind};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io::Read, os::unix::fs::MetadataExt, path::Path};

const LIMIT: usize = 256;
/// Agent conversations kept per entry; the oldest are dropped first.
pub const SESSION_LIMIT: usize = 32;
/// `last_seen` only advances in steps of this many seconds, so an agent
/// that stays open does not rewrite the history file on every discovery.
pub const LAST_SEEN_STEP: u64 = 60;

/// One agent conversation a pane hosted, in the order the server saw them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub kind: Kind,
    /// The provider's conversation ID (validated by `crate::session_id`).
    pub id: String,
    /// The pane's folder when the conversation was first seen; resume runs there.
    pub cwd: String,
    /// Epoch seconds.
    pub first_seen: u64,
    /// Epoch seconds, advanced in `LAST_SEEN_STEP` steps.
    pub last_seen: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub key: String,
    pub workspace: String,
    pub pane: u64,
    pub cwd: String,
    pub name: String,
    pub kind: Option<Kind>,
    /// The latest conversation; kept for compatibility, see `sessions`.
    pub session_id: Option<String>,
    pub argv: Vec<String>,
    pub closed_at: Option<u64>,
    pub reason: String,
    /// Every conversation the pane hosted, oldest first, at most `SESSION_LIMIT`.
    #[serde(default)]
    pub sessions: Vec<Session>,
}
impl Entry {
    /// Record the conversation a pane currently runs, if it has a valid one.
    fn record_session(&mut self, kind: Option<Kind>, id: Option<&str>, cwd: &str, at: u64) {
        let (Some(kind), Some(id)) = (kind, id.filter(|id| crate::session_id(id))) else {
            return;
        };
        match self.sessions.last_mut() {
            Some(last) if last.id == id && last.kind == kind => {
                if at >= last.last_seen.saturating_add(LAST_SEEN_STEP) {
                    last.last_seen = at;
                }
            }
            _ => {
                self.sessions.push(Session {
                    kind,
                    id: id.into(),
                    cwd: cwd.into(),
                    first_seen: at,
                    last_seen: at,
                });
                if self.sessions.len() > SESSION_LIMIT {
                    let excess = self.sessions.len() - SESSION_LIMIT;
                    self.sessions.drain(..excess);
                }
            }
        }
    }
    /// Construct only trusted agent invocations; ordinary commands are opt-in.
    pub fn command(&self) -> Option<String> {
        self.kind.map(|kind| {
            let base = match kind {
                Kind::Codex => "codex resume",
                Kind::Claude => "claude --resume",
            };
            self.session_id
                .as_deref()
                .filter(|id| crate::session_id(id))
                .map_or_else(
                    || match kind {
                        Kind::Codex => "codex resume".into(),
                        Kind::Claude => "claude --resume".into(),
                    },
                    |id| format!("{base} {}", crate::shell_quote(id)),
                )
        })
    }
    pub fn previous_command(&self) -> Option<String> {
        (!self.argv.is_empty()).then(|| {
            self.argv
                .iter()
                .map(|s| crate::shell_quote(s))
                .collect::<Vec<_>>()
                .join(" ")
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub restored: Vec<String>,
}
impl History {
    pub fn load(path: &Path) -> Result<Self> {
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        if !meta.is_file()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o077 != 0
            || meta.len() > 4 * 1024 * 1024
        {
            return Err("tab history must be a bounded private owned file".into());
        }
        let mut data = Vec::new();
        fs::File::open(path)?
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut data)?;
        let mut history: Self = serde_json::from_slice(&data)?;
        history.entries.truncate(LIMIT);
        Ok(history)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        journal::atomic(path, self)
    }
    pub fn begin_run(&mut self, at: u64) {
        for entry in &mut self.entries {
            if entry.closed_at.is_none() {
                entry.closed_at = Some(at);
                entry.reason = "Previous run".into();
            }
        }
        self.prune(at);
    }
    pub fn observe(
        &mut self,
        run: &str,
        workspace: &str,
        panes: &[PaneInfo],
        commands: &BTreeMap<u64, Vec<String>>,
    ) {
        self.observe_at(run, workspace, panes, commands, crate::now());
    }
    /// `observe` at an explicit time (epoch seconds).
    pub fn observe_at(
        &mut self,
        run: &str,
        workspace: &str,
        panes: &[PaneInfo],
        commands: &BTreeMap<u64, Vec<String>>,
        at: u64,
    ) {
        for pane in panes {
            let key = format!("{run}:{}", pane.id);
            let argv = commands.get(&pane.id).cloned().unwrap_or_default();
            // Never add an empty shell; preserve the last useful application after exit.
            if pane.kind.is_none() && argv.is_empty() {
                continue;
            }
            let name = if pane.kind.is_none() {
                argv.first()
                    .and_then(|arg| Path::new(arg).file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| pane.title.clone())
            } else {
                pane.title.clone()
            };
            let mut entry = Entry {
                key: key.clone(),
                workspace: workspace.into(),
                pane: pane.id,
                cwd: pane.cwd.clone(),
                name,
                kind: pane.kind,
                session_id: pane.session_id.clone(),
                argv,
                closed_at: None,
                reason: String::new(),
                sessions: Vec::new(),
            };
            // Everything else is replaced on observation; the conversations accumulate.
            let mut old = self.entries.iter_mut().find(|entry| entry.key == key);
            if let Some(old) = &mut old {
                entry.sessions = std::mem::take(&mut old.sessions);
            }
            entry.record_session(pane.kind, pane.session_id.as_deref(), &pane.cwd, at);
            if let Some(old) = old {
                *old = entry;
            } else {
                self.entries.insert(0, entry);
            }
        }
        self.prune(at);
    }
    pub fn close(&mut self, run: &str, pane: u64, at: u64) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.key == format!("{run}:{pane}"))
        {
            entry.closed_at = Some(at);
            entry.reason = "Closed".into();
        }
        self.prune(at);
    }
    pub fn consume(&mut self, key: &str) {
        self.entries.retain(|entry| entry.key != key);
        self.restored.push(key.into());
        if self.restored.len() > LIMIT {
            self.restored.remove(0);
        }
    }
    fn prune(&mut self, at: u64) {
        self.entries.retain(|entry| {
            entry
                .closed_at
                .is_none_or(|time| at.saturating_sub(time) <= journal::RETENTION)
        });
        self.entries
            .sort_by_key(|entry| std::cmp::Reverse(entry.closed_at.unwrap_or(u64::MAX)));
        self.entries.truncate(LIMIT);
    }
}

/// Same private workspace boundary as the existing recovery journal.
pub fn collect(state: &Path) -> Vec<Entry> {
    let mut states = vec![state.to_path_buf()];
    if let Some(root) = state
        .parent()
        .filter(|root| root.file_name().is_some_and(|n| n == "workspaces"))
    {
        if let Ok(dirs) = fs::read_dir(root) {
            for dir in dirs.take(64).flatten() {
                let path = dir.path();
                if path != state
                    && fs::symlink_metadata(&path).is_ok_and(|m| {
                        m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0
                    })
                {
                    states.push(path);
                }
            }
        }
    }
    let mut entries: Vec<_> = states
        .iter()
        .filter_map(|s| History::load(&s.join("tab-history.json")).ok())
        .flat_map(|h| h.entries)
        .filter(|e| {
            e.closed_at
                .is_some_and(|at| crate::now().saturating_sub(at) <= journal::RETENTION)
        })
        .collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.closed_at));
    entries.truncate(LIMIT);
    entries
}
