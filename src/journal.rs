use crate::{
    Result,
    model::{Agent, Kind, Status},
    now, session_id,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub const RETENTION: u64 = 7 * 24 * 60 * 60;
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Registry {
    pub agents: BTreeMap<String, Agent>,
    pub names: BTreeMap<String, String>,
    pub hooks: BTreeMap<String, Hook>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    pub id: String,
    pub kind: Kind,
    pub status: Status,
    pub at: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub at: u64,
    pub event: String,
    pub agent: Agent,
}

pub struct Lock(File);
impl Lock {
    pub fn acquire(path: &Path, wait: bool) -> Result<Self> {
        use std::os::fd::AsRawFd;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        // SAFETY: flock operates on the valid, owned descriptor.
        if unsafe {
            libc::flock(
                file.as_raw_fd(),
                libc::LOCK_EX | if wait { 0 } else { libc::LOCK_NB },
            )
        } != 0
        {
            return Err("another Kelpmux operation is already running".into());
        }
        Ok(Self(file))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: unlock the still-owned descriptor.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

pub fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    // SAFETY: geteuid has no arguments or memory effects.
    if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err("state directory must be an owned directory, not a symlink".into());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
pub fn atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temp = path.with_extension(format!(
        "{}-{}-{}.tmp",
        std::process::id(),
        now(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp)?;
    let result = (|| -> Result<()> {
        serde_json::to_writer(&mut file, value)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        File::open(path.parent().ok_or("missing parent")?)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

pub struct Store {
    pub root: PathBuf,
    pub registry: Registry,
    _lock: Lock,
}
impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        private_dir(root)?;
        let lock = Lock::acquire(&root.join("registry.lock"), true)?;
        repair_tail(&root.join("journal.jsonl"))?;
        let file = root.join("registry.json");
        let registry = if file.exists() {
            let f = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(file)?;
            serde_json::from_reader(f)?
        } else {
            Registry::default()
        };
        Ok(Self {
            root: root.into(),
            registry,
            _lock: lock,
        })
    }
    pub fn save(&self) -> Result<()> {
        atomic(&self.root.join("registry.json"), &self.registry)
    }
    pub fn append(&self, event: &str, agent: &Agent, time: u64) -> Result<()> {
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.root.join("journal.jsonl"))?;
        serde_json::to_writer(
            &mut file,
            &Record {
                at: time,
                event: event.into(),
                agent: agent.clone(),
            },
        )?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        Ok(())
    }
    pub fn reconcile(&mut self, agents: Vec<Agent>, time: u64) -> Result<()> {
        let mut current = BTreeMap::new();
        for mut agent in agents {
            let key = agent.key();
            if let Some(old) = self.registry.agents.get(&key) {
                agent.first_seen = old.first_seen;
                if agent.status == Status::Idle
                    && matches!(old.status, Status::Working | Status::TurnFinished)
                    && old.session_id == agent.session_id
                {
                    agent.status = Status::TurnFinished;
                }
            }
            if let Some(hook) = self.registry.hooks.get(&key) {
                if hook.kind == agent.kind && hook.at >= time.saturating_sub(RETENTION) {
                    if agent.session_id.is_none() {
                        agent.session_id = Some(hook.id.clone());
                        agent.source = "hook".into();
                    }
                    if agent.session_id.as_deref() == Some(&hook.id)
                        && (agent.status == Status::Unknown
                            || agent.source == "hook"
                            || hook.at >= time.saturating_sub(5))
                    {
                        agent.status = hook.status;
                    }
                }
            }
            let name_key = name_key(&agent);
            if let Some(name) = self.registry.names.get(&name_key) {
                agent.name = name.clone();
            }
            match self.registry.agents.get(&key) {
                None => self.append("discovered", &agent, time)?,
                Some(old) if agent.changed(old) => self.append("changed", &agent, time)?,
                _ => (),
            }
            current.insert(key, agent);
        }
        for (key, old) in &self.registry.agents {
            if current.contains_key(key) {
                continue;
            }
            let mut ended = old.clone();
            if ended.ended_at.is_none() {
                ended.ended_at = Some(time);
                ended.status = Status::Exited;
                self.append("exited", &ended, time)?;
            }
            if ended
                .ended_at
                .is_some_and(|t| t >= time.saturating_sub(RETENTION))
            {
                current.insert(key.clone(), ended);
            }
        }
        self.registry.agents = current;
        self.registry
            .hooks
            .retain(|_, h| h.at >= time.saturating_sub(RETENTION));
        self.prune(time)?;
        self.save()
    }
    pub fn records(&self) -> Result<Vec<Record>> {
        let path = self.root.join("journal.jsonl");
        if !path.exists() {
            return Ok(vec![]);
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let mut records = vec![];
        let mut reader = BufReader::new(file);
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            // A crash can interrupt the final append. Completed records remain recoverable.
            if !line.ends_with('\n') {
                break;
            }
            if !line.trim().is_empty() {
                records.push(serde_json::from_str(&line)?);
            }
        }
        Ok(records)
    }
    pub fn prune(&self, time: u64) -> Result<()> {
        let records = self.records()?;
        if !records
            .iter()
            .any(|r| r.at < time.saturating_sub(RETENTION))
        {
            return Ok(());
        }
        let path = self.root.join("journal.jsonl");
        let temp = self
            .root
            .join(format!("journal.{}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temp)?;
        for record in records
            .iter()
            .filter(|r| r.at >= time.saturating_sub(RETENTION))
        {
            serde_json::to_writer(&mut file, record)?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
        fs::rename(temp, path)?;
        Ok(())
    }
    pub fn rename(&mut self, pane: &str, name: &str) -> Result<()> {
        let agent = self
            .registry
            .agents
            .values_mut()
            .find(|a| a.pane == pane && a.ended_at.is_none())
            .ok_or("no discovered agent in pane")?;
        let key = name_key(agent);
        agent.name = name.into();
        let agent = agent.clone();
        self.registry.names.insert(key, name.into());
        self.append("renamed", &agent, now())?;
        self.save()
    }
    pub fn hook(&mut self, pane: &str, kind: Kind, input: &Value) -> Result<()> {
        let id = input["session_id"]
            .as_str()
            .or_else(|| input["thread-id"].as_str())
            .filter(|s| session_id(s))
            .ok_or("hook has no valid session identifier")?;
        let event = input["hook_event_name"]
            .as_str()
            .or_else(|| input["type"].as_str())
            .unwrap_or("");
        let Some(status) = hook_status(event, input) else {
            return Ok(());
        };
        let agent = self
            .registry
            .agents
            .values_mut()
            .find(|a| a.pane == pane && a.kind == kind && a.ended_at.is_none())
            .ok_or("pane is not registered; run scan or daemon first")?;
        // Never overwrite a positively identified different conversation.
        if agent.session_id.as_deref().is_some_and(|s| s != id) {
            return Err("hook session does not match discovered agent".into());
        }
        agent.session_id = Some(id.into());
        agent.status = status;
        agent.source = "hook".into();
        let agent = agent.clone();
        self.registry.hooks.insert(
            agent.key(),
            Hook {
                id: id.into(),
                kind,
                status,
                at: now(),
            },
        );
        self.append("hook", &agent, now())?;
        self.save()
    }
}
fn repair_tail(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0];
    file.read_exact(&mut last)?;
    if last[0] == b'\n' {
        return Ok(());
    }
    let mut end = len;
    loop {
        let start = end.saturating_sub(8192);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = vec![0; (end - start) as usize];
        file.read_exact(&mut bytes)?;
        if let Some(i) = bytes.iter().rposition(|b| *b == b'\n') {
            file.set_len(start + i as u64 + 1)?;
            break;
        }
        if start == 0 {
            file.set_len(0)?;
            break;
        }
        end = start;
    }
    file.sync_all()?;
    Ok(())
}
pub fn name_key(agent: &Agent) -> String {
    format!(
        "{}:{}",
        agent.kind.as_str(),
        agent.session_id.as_deref().unwrap_or(&agent.key())
    )
}
pub fn hook_status(event: &str, value: &Value) -> Option<Status> {
    match event {
        "SessionStart" => Some(Status::Idle),
        "UserPromptSubmit" | "PostToolUse" => Some(Status::Working),
        "PreToolUse"
            if ["AskUserQuestion", "request_user_input"]
                .contains(&value["tool_name"].as_str().unwrap_or("")) =>
        {
            Some(Status::InputNeeded)
        }
        "PreToolUse" => Some(Status::Working),
        "PermissionRequest" => Some(Status::InputNeeded),
        "Notification"
            if ["permission_prompt", "idle_prompt"]
                .contains(&value["notification_type"].as_str().unwrap_or("")) =>
        {
            Some(Status::InputNeeded)
        }
        "Stop" | "agent-turn-complete" if !value["stop_hook_active"].as_bool().unwrap_or(false) => {
            Some(Status::TurnFinished)
        }
        "StopFailure" => Some(Status::Failed),
        "SessionEnd" => Some(Status::Exited),
        _ => None,
    }
}
