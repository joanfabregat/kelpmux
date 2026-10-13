//! Optional lifecycle metadata for an already discovered native agent.
use crate::{Result, journal::Store, model::Kind, process};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub fn run(args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let kind = match args.first().map(String::as_str) {
        Some("codex") => Kind::Codex,
        Some("claude") => Kind::Claude,
        _ => return Err("native-event requires codex or claude".into()),
    };
    args.remove(0);
    let option = |args: &mut Vec<String>, name: &str| -> Result<Option<String>> {
        if let Some(i) = args.iter().position(|arg| arg == name) {
            args.remove(i);
            if i == args.len() || args[i].starts_with("--") {
                return Err(format!("{name} requires a value").into());
            }
            Ok(Some(args.remove(i)))
        } else {
            Ok(None)
        }
    };
    let pane = option(&mut args, "--pane")?
        .or_else(|| std::env::var("KELPMUX_PANE").ok())
        .ok_or("native-event requires --pane or KELPMUX_PANE")?;
    let state = option(&mut args, "--state-dir")?
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .unwrap_or_else(|| {
                    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
                })
                .join("kelpmux/native")
        });
    if !args.is_empty() {
        return Err("unexpected native-event arguments".into());
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 {
        return Err("native hook payload exceeds 64 KiB".into());
    }
    receive(&state, &pane, kind, &serde_json::from_slice(&bytes)?)
}

pub fn receive(state: &Path, pane: &str, kind: Kind, input: &serde_json::Value) -> Result<()> {
    let id: u64 = pane.trim_start_matches('%').parse()?;
    if id == 0 {
        return Err("invalid native pane ID".into());
    }
    let pane = format!("%{id}");
    let mut store = Store::open_nonblocking(&state.join("native"))?;
    let agent = store
        .registry
        .agents
        .values()
        .find(|agent| agent.pane == pane && agent.kind == kind && agent.ended_at.is_none())
        .ok_or("native agent has not been discovered yet")?;
    if !process::read(Path::new("/proc"), agent.pid)
        .is_ok_and(|current| current.start == agent.process_start)
    {
        return Err("native agent process identity is stale".into());
    }
    store.hook(&pane, kind, input)
}
