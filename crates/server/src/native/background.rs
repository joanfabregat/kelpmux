//! Minimal Claude lifecycle metadata; safe alongside older workspace servers.
//! No prompt, tool arguments, task descriptions or commands are persisted.
use crate::{
    Result, journal,
    model::{Agent, Kind, Observation, Status, StatusSource},
    process,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs::File, io::Read, os::unix::ffi::OsStrExt, path::Path};

#[derive(Serialize, Deserialize)]
struct Pending {
    pid: u32,
    birth: u64,
    session: String,
    tasks: usize,
    wakeups: usize,
    at: u64,
}

pub fn counts(value: &Value) -> Option<(usize, usize)> {
    match value["hook_event_name"].as_str()? {
        "UserPromptSubmit" | "PreToolUse" | "SessionEnd" => Some((0, 0)),
        "Stop" | "SubagentStop"
            if value["background_tasks"].is_array() && value["session_crons"].is_array() =>
        {
            let tasks = value["background_tasks"]
                .as_array()?
                .iter()
                .take(128)
                .filter(|task| matches!(task["status"].as_str(), Some("running" | "pending")))
                .count();
            let wakeups = value["session_crons"]
                .as_array()?
                .iter()
                .take(128)
                .filter(|cron| cron["id"].is_string() && cron["schedule"].is_string())
                .count();
            Some((tasks, wakeups))
        }
        _ => None,
    }
}
fn json(path: &Path) -> Option<Value> {
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 4 * 1024 * 1024 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
pub fn receive(state: &Path, pane: &str, input: &Value) -> Result<()> {
    let Some((tasks, wakeups)) = counts(input) else {
        return Ok(());
    };
    if !state.is_absolute() {
        return Err("background state path must be absolute".into());
    }
    let id = pane.trim_start_matches('%').parse::<u64>()?;
    if id == 0 {
        return Err("invalid pane".into());
    }
    let session = input["session_id"]
        .as_str()
        .filter(|id| crate::session_id(id))
        .ok_or("missing session ID")?;
    // Atomic registry reads avoid contending with the polling worker's lock.
    let registry: journal::Registry = serde_json::from_value(
        json(&state.join("native/registry.json")).ok_or("agent registry unavailable")?,
    )?;
    let agent = registry
        .agents
        .values()
        .find(|agent| {
            agent.pane == format!("%{id}") && agent.kind == Kind::Claude && agent.ended_at.is_none()
        })
        .ok_or("Claude is not registered in this pane")?;
    if agent.session_id.as_deref().is_some_and(|id| id != session)
        || !process::read(Path::new("/proc"), agent.pid)
            .is_ok_and(|proc| proc.start == agent.process_start)
    {
        return Err("Claude process/session identity changed".into());
    }
    let root = state.join("native/background");
    journal::private_dir(&root)?;
    journal::atomic(
        &root.join(format!("{id}.json")),
        &Pending {
            pid: agent.pid,
            birth: agent.process_start,
            session: session.into(),
            tasks,
            wakeups,
            at: crate::now(),
        },
    )
}
pub fn enrich(proc_root: &Path, agent: &mut Agent) {
    if agent.kind != Kind::Claude
        || !matches!(
            agent.status,
            Status::Idle | Status::TurnFinished | Status::Unknown
        )
    {
        return;
    }
    let Ok(file) = File::open(proc_root.join(format!("{}/environ", agent.pid))) else {
        return;
    };
    let mut bytes = Vec::new();
    if file.take(1024 * 1024).read_to_end(&mut bytes).is_err() {
        return;
    }
    let Some(state) = bytes
        .split(|&b| b == 0)
        .find_map(|entry| entry.strip_prefix(b"KELPMUX_STATE_DIR="))
    else {
        return;
    };
    let state = Path::new(std::ffi::OsStr::from_bytes(state));
    if !state.is_absolute() {
        return;
    }
    let Some(id) = agent
        .pane
        .strip_prefix('%')
        .and_then(|s| s.parse::<u64>().ok())
    else {
        return;
    };
    let Some(value) = json(&state.join(format!("native/background/{id}.json"))) else {
        return;
    };
    let Ok(pending) = serde_json::from_value::<Pending>(value) else {
        return;
    };
    if pending.pid != agent.pid
        || pending.birth != agent.process_start
        || agent.session_id.as_deref() != Some(&pending.session)
        || pending.tasks > 128
        || pending.wakeups > 128
        || pending.tasks.saturating_add(pending.wakeups) == 0
        || pending.at < crate::now().saturating_sub(journal::RETENTION)
    {
        return;
    }
    agent.status = Status::WaitingBackground;
    agent.observation = Observation {
        source: StatusSource::Hook,
        checked_at: crate::now(),
        event_at: Some(pending.at),
    };
}
