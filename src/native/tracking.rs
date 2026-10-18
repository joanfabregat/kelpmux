//! Metadata-only agent views and recovery; never launches or resumes a process.
use super::{protocol::PaneInfo, ui::AgentFilter};
use crate::{
    Result,
    journal::{RETENTION, Store},
    model::{Agent, Kind, Status},
    now, session_id, shell_quote,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

/// Idle shells use a neutral label; aliases and running application titles win.
pub fn shell_tab_name(terminal: Option<&str>, alias: Option<&str>, idle: bool) -> String {
    alias
        .or_else(|| {
            terminal.filter(|title| {
                !idle && !title.trim().is_empty() && !title.eq_ignore_ascii_case("shell")
            })
        })
        .unwrap_or("Shell")
        .chars()
        .take(1024)
        .collect()
}

/// A directory is useful context but is not evidence of a conversation's name.
/// The boolean means metadata/override owns the title; otherwise OSC may update it.
pub fn tab_name(agent: &Agent, terminal: Option<&str>, alias: Option<&str>) -> (String, bool) {
    if let Some(alias) = alias {
        return (alias.chars().take(1024).collect(), true);
    }
    let folder = Path::new(&agent.cwd)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    if !agent.name.trim().is_empty() && agent.name != folder && agent.name != agent.cwd {
        return (agent.name.chars().take(1024).collect(), true);
    }
    if let Some(title) = terminal
        .map(|title| agent_terminal_title(title, &agent.cwd))
        .filter(|title| {
            !title.trim().is_empty()
                && *title != folder
                && *title != agent.cwd
                && *title != "shell"
                && *title != agent.kind.as_str()
        })
    {
        return (title.chars().take(1024).collect(), false);
    }
    (
        match &agent.session_id {
            Some(id) => format!(
                "{} · {}",
                agent.kind.as_str(),
                id.chars().take(8).collect::<String>()
            ),
            None => format!("{} · session unknown", agent.kind.as_str()),
        },
        false,
    )
}

/// Applications may decorate OSC titles with a spinner independently of agent
/// metadata. Remove only a leading braille glyph followed by whitespace; never
/// infer lifecycle state from it or rewrite explicit names and aliases.
pub fn agent_terminal_title<'a>(title: &'a str, cwd: &str) -> &'a str {
    let title = title.trim();
    let mut chars = title.chars();
    let title = match chars.next() {
        Some(first)
            if ('\u{2800}'..='\u{28ff}').contains(&first)
                && chars.next().is_some_and(char::is_whitespace) =>
        {
            title[first.len_utf8()..].trim_start()
        }
        _ => title,
    };
    let folder = Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    title
        .strip_suffix(folder)
        .filter(|_| !folder.is_empty())
        .and_then(|prefix| prefix.strip_suffix(" | "))
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(title)
}

/// Preserve logical pane order, including ordinary shells in the unfiltered view.
pub fn filtered(panes: &[PaneInfo], filter: AgentFilter) -> Vec<&PaneInfo> {
    panes
        .iter()
        .filter(|pane| match filter {
            AgentFilter::All => true,
            AgentFilter::Attention => {
                pane.kind.is_some() && matches!(pane.status, Status::InputNeeded | Status::Failed)
            }
            AgentFilter::Finished => {
                pane.kind.is_some() && matches!(pane.status, Status::TurnFinished | Status::Exited)
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryEntry {
    pub kind: Kind,
    pub name: String,
    pub cwd: String,
    pub session_id: Option<String>,
    pub pane: String,
    pub pid: u32,
    pub process_start: u64,
    pub source: String,
    pub first_seen: u64,
    pub last_seen: u64,
    pub ended_at: Option<u64>,
    pub status: Status,
    /// Copyable command only; unknown identities must never resume an arbitrary session.
    pub resume_command: Option<String>,
}

impl From<Agent> for RecoveryEntry {
    fn from(agent: Agent) -> Self {
        let resume_command = agent
            .session_id
            .as_deref()
            .filter(|id| session_id(id))
            .map(|id| {
                let argument = shell_quote(id);
                match agent.kind {
                    Kind::Codex => format!("codex resume {argument}"),
                    Kind::Claude => format!("claude --resume {argument}"),
                }
            });
        Self {
            kind: agent.kind,
            name: agent.name,
            cwd: agent.cwd,
            session_id: agent.session_id,
            pane: agent.pane,
            pid: agent.pid,
            process_start: agent.process_start,
            source: agent.source,
            first_seen: agent.first_seen,
            last_seen: agent.last_seen,
            ended_at: agent.ended_at,
            status: agent.status,
            resume_command,
        }
    }
}

/// Read the native registry and bounded journal under its nonblocking metadata lock.
/// A retained running record indicates the last observation, not a live-process promise.
pub fn recovery(state: &Path) -> Result<Vec<RecoveryEntry>> {
    recovery_at(state, now())
}

/// Explicit clock for deterministic retention and recovery tests.
pub fn recovery_at(state: &Path, time: u64) -> Result<Vec<RecoveryEntry>> {
    let store = Store::open_nonblocking(&state.join("native"))?;
    store.prune(time)?;
    let cutoff = time.saturating_sub(RETENTION);
    // Journal records also recover sessions missing from a registry after a crash.
    // Process birth ticks keep PID reuse and different sessions separate.
    let mut latest = BTreeMap::<(String, Option<String>), (u64, Agent)>::new();
    for record in store
        .records()?
        .into_iter()
        .filter(|record| record.at >= cutoff)
    {
        let key = (record.agent.key(), record.agent.session_id.clone());
        if latest.get(&key).is_none_or(|(at, _)| record.at >= *at) {
            latest.insert(key, (record.at, record.agent));
        }
    }
    for agent in store
        .registry
        .agents
        .values()
        .filter(|agent| agent.ended_at.unwrap_or(agent.last_seen) >= cutoff)
    {
        let key = (agent.key(), agent.session_id.clone());
        let at = agent.ended_at.unwrap_or(agent.last_seen);
        if latest.get(&key).is_none_or(|(old_at, _)| at >= *old_at) {
            latest.insert(key, (at, agent.clone()));
        }
    }
    // An initial unidentified observation is superseded once this same process
    // has a positively identified conversation; distinct known IDs remain.
    let identified: std::collections::BTreeSet<_> = latest
        .keys()
        .filter(|(_, id)| id.is_some())
        .map(|(key, _)| key.clone())
        .collect();
    let mut entries: Vec<RecoveryEntry> = latest
        .into_iter()
        .filter(|((key, id), _)| id.is_some() || !identified.contains(key))
        .map(|(_, (_, agent))| RecoveryEntry::from(agent))
        .collect();
    entries.sort_by(|a, b| {
        b.ended_at
            .unwrap_or(b.last_seen)
            .cmp(&a.ended_at.unwrap_or(a.last_seen))
            .then(a.pid.cmp(&b.pid))
            .then(a.process_start.cmp(&b.process_start))
            .then(a.session_id.cmp(&b.session_id))
    });
    Ok(entries)
}
