use crate::{
    Result,
    model::{Agent, Kind, Status},
    process::{self, Process},
    session_id,
    tmux::Pane,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

pub struct Discovery {
    pub proc_root: PathBuf,
    pub codex_home: PathBuf,
    pub claude_home: PathBuf,
}
impl Discovery {
    pub fn new(home: &Path) -> Self {
        Self {
            proc_root: "/proc".into(),
            codex_home: std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex")),
            claude_home: std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".claude")),
        }
    }
    pub fn scan(&self, panes: &[Pane], time: u64) -> Result<Vec<Agent>> {
        let tree = process::snapshot(&self.proc_root)?;
        let mut result = Vec::new();
        for pane in panes {
            let mut candidates: Vec<_> = process::descendants(&tree, pane.pid)
                .into_iter()
                .filter_map(|pid| {
                    let proc = tree.get(&pid)?;
                    let args = process::args(&self.proc_root, pid);
                    classify(proc, &args).map(|kind| (proc, kind, args))
                })
                .collect();
            // Parent-most interactive agent wins. Nested tool processes are not new panes.
            candidates.sort_by_key(|(p, _, _)| depth(&tree, p.pid, pane.pid));
            // npm's Codex launcher is a node parent; metadata belongs to its native child.
            candidates.retain(|(p, kind, _)| {
                !(p.command == "node"
                    && tree
                        .values()
                        .any(|child| child.parent == p.pid && child.command == kind.as_str()))
            });
            let Some((proc, kind, args)) = candidates.first() else {
                continue;
            };
            let cwd = fs::read_link(self.proc_root.join(proc.pid.to_string()).join("cwd"))
                .unwrap_or_else(|_| pane.cwd.clone().into())
                .to_string_lossy()
                .into_owned();
            let mut agent = Agent {
                pane: pane.id.clone(),
                window: pane.window.clone(),
                pid: proc.pid,
                process_start: proc.start,
                kind: *kind,
                session_id: None,
                name: Path::new(&cwd)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                cwd,
                status: Status::Unknown,
                source: "process".into(),
                first_seen: time,
                last_seen: time,
                ended_at: None,
            };
            match kind {
                Kind::Claude => self.claude(proc, args, &mut agent),
                Kind::Codex => self.codex(proc, args, &mut agent),
            }
            result.push(agent);
        }
        Ok(result)
    }
    fn claude(&self, proc: &Process, args: &[String], agent: &mut Agent) {
        let file = self
            .claude_home
            .join("sessions")
            .join(format!("{}.json", proc.pid));
        if let Some(value) = read_json(&file) {
            let start = value.get("procStart").and_then(|v| {
                v.as_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    .or_else(|| v.as_u64())
            });
            if value.get("pid").and_then(Value::as_u64) == Some(proc.pid as u64)
                && start == Some(proc.start)
            {
                agent.session_id =
                    string(&value, &["sessionId", "session_id"]).filter(|s| session_id(s));
                if let Some(name) = string(&value, &["name"]) {
                    agent.name = name;
                }
                if let Some(status) = string(&value, &["status"]) {
                    agent.status = Status::from_live(&status);
                }
                agent.source = "claude-live-record".into();
            }
        }
        if agent.session_id.is_none() {
            agent.session_id = resume_id(args, true);
            if agent.session_id.is_some() {
                agent.source = "resume-argument".into();
            }
        }
    }
    fn codex(&self, proc: &Process, args: &[String], agent: &mut Agent) {
        agent.session_id = process::environment(&self.proc_root, proc.pid, "CODEX_THREAD_ID")
            .filter(|s| session_id(s))
            .or_else(|| resume_id(args, false));
        if agent.session_id.is_some() {
            agent.source = "process-identity".into();
        }
        let mut rollout = None;
        for path in process::links(&self.proc_root, proc.pid) {
            if path.extension().is_some_and(|e| e == "jsonl")
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("rollout-"))
            {
                if let Ok(file) = File::open(&path) {
                    let mut line = String::new();
                    let _ = BufReader::new(file).take(128 * 1024).read_line(&mut line);
                    if let Ok(value) = serde_json::from_str::<Value>(&line) {
                        if value["type"] == "session_meta" {
                            if let Some(id) =
                                value["payload"]["id"].as_str().filter(|s| session_id(s))
                            {
                                agent.session_id = Some(id.into());
                                agent.source = "open-rollout".into();
                                rollout = Some(path);
                                break;
                            }
                        }
                    }
                }
            }
        }
        let Some(id) = agent.session_id.clone() else {
            return;
        };
        if let Some(name) = index_name(&self.codex_home.join("session_index.jsonl"), &id) {
            agent.name = name;
        }
        // Connect only to an already-running daemon; never start or resume an agent.
        if let Ok(value) = read_codex_thread(
            &self
                .codex_home
                .join("app-server-control/app-server-control.sock"),
            &id,
        ) {
            let thread = &value["thread"];
            if thread["id"].as_str() == Some(&id) {
                if let Some(name) = thread["name"].as_str().filter(|s| !s.is_empty()) {
                    agent.name = name.into();
                }
                agent.status = codex_status(&thread["status"]);
                return;
            }
        }
        if rollout.is_none() {
            rollout = find_rollout(&self.codex_home.join("sessions"), &id, 0);
        }
        if let Some(path) = rollout {
            agent.status = rollout_status(&path);
        }
    }
}

fn depth(tree: &BTreeMap<u32, Process>, mut pid: u32, root: u32) -> usize {
    for count in 0..128 {
        if pid == root {
            return count;
        }
        let Some(p) = tree.get(&pid) else {
            break;
        };
        pid = p.parent;
    }
    128
}
fn classify(proc: &Process, args: &[String]) -> Option<Kind> {
    if args
        .iter()
        .any(|s| ["app-server", "mcp-server", "exec", "-p", "--print"].contains(&s.as_str()))
    {
        return None;
    }
    if proc.command == "claude" {
        return Some(Kind::Claude);
    }
    if proc.command == "codex" {
        return Some(Kind::Codex);
    }
    if proc.command == "node"
        && args
            .iter()
            .take(2)
            .any(|s| s.contains("@anthropic-ai/claude-code") || s.ends_with("/claude/cli.js"))
    {
        return Some(Kind::Claude);
    }
    if proc.command == "node" && args.iter().take(2).any(|s| s.contains("@openai/codex")) {
        return Some(Kind::Codex);
    }
    None
}
pub fn resume_id(args: &[String], claude: bool) -> Option<String> {
    for pair in args.windows(2) {
        if ["resume", "--resume", "--session-id"].contains(&pair[0].as_str())
            && pair[1].len() >= 32
            && session_id(&pair[1])
        {
            return Some(pair[1].clone());
        }
    }
    if claude {
        for arg in args {
            if let Some(id) = arg
                .strip_prefix("--resume=")
                .or_else(|| arg.strip_prefix("--session-id="))
            {
                if id.len() >= 32 && session_id(id) {
                    return Some(id.into());
                }
            }
        }
    }
    None
}
fn string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        value
            .get(k)?
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    })
}
pub fn read_json(path: &Path) -> Option<Value> {
    let f = File::open(path).ok()?;
    if f.metadata().ok()?.len() > 1024 * 1024 {
        return None;
    }
    serde_json::from_reader(f).ok()
}
fn tail(path: &Path, bytes: u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut text = String::new();
    file.take(bytes).read_to_string(&mut text).ok()?;
    if start > 0 {
        text = text.split_once('\n')?.1.into();
    }
    Some(text)
}
fn index_name(path: &Path, id: &str) -> Option<String> {
    let text = tail(path, 1024 * 1024)?;
    text.lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|v| {
            if v["id"].as_str() == Some(id) {
                string(&v, &["thread_name", "name"])
            } else {
                None
            }
        })
}
fn find_rollout(dir: &Path, id: &str, depth: usize) -> Option<PathBuf> {
    if depth > 4 {
        return None;
    }
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let typ = entry.file_type().ok()?;
        if typ.is_dir() {
            if let Some(p) = find_rollout(&entry.path(), id, depth + 1) {
                return Some(p);
            }
        } else if typ.is_file()
            && entry
                .file_name()
                .to_string_lossy()
                .ends_with(&format!("{id}.jsonl"))
        {
            return Some(entry.path());
        }
    }
    None
}
pub fn rollout_status(path: &Path) -> Status {
    let Some(text) = tail(path, 256 * 1024) else {
        return Status::Unknown;
    };
    let mut status = Status::Unknown;
    for value in text
        .lines()
        .filter_map(|s| serde_json::from_str::<Value>(s).ok())
    {
        if value["type"] == "event_msg" {
            match value["payload"]["type"].as_str().unwrap_or("") {
                "task_started" | "turn_started" => status = Status::Working,
                "task_complete" | "turn_complete" => status = Status::TurnFinished,
                "turn_aborted" => status = Status::Idle,
                _ => (),
            }
        }
    }
    status
}
pub fn codex_status(v: &Value) -> Status {
    if v["type"] == "active" {
        if v["activeFlags"].as_array().is_some_and(|flags| {
            flags.iter().any(|v| {
                ["waitingOnApproval", "waitingOnUserInput"].contains(&v.as_str().unwrap_or(""))
            })
        }) {
            Status::InputNeeded
        } else {
            Status::Working
        }
    } else {
        Status::from_live(v["type"].as_str().unwrap_or(""))
    }
}
fn read_codex_thread(socket: &Path, id: &str) -> Result<Value> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_millis(300)))?;
    stream.set_write_timeout(Some(Duration::from_millis(300)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    writeln!(
        stream,
        "{}",
        json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"kelpmux","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}})
    )?;
    rpc_result(&mut reader, 1)?;
    writeln!(stream, "{}", json!({"method":"initialized"}))?;
    writeln!(
        stream,
        "{}",
        json!({"id":2,"method":"thread/read","params":{"threadId":id,"includeTurns":false}})
    )?;
    rpc_result(&mut reader, 2)
}
fn rpc_result(reader: &mut impl BufRead, id: u64) -> Result<Value> {
    for _ in 0..32 {
        let mut line = String::new();
        reader.take(1024 * 1024).read_line(&mut line)?;
        if line.is_empty() {
            return Err("Codex closed discovery connection".into());
        }
        let value: Value = serde_json::from_str(&line)?;
        if value["id"].as_u64() == Some(id) {
            if value.get("error").is_some() {
                return Err("Codex rejected metadata request".into());
            }
            return Ok(value["result"].clone());
        }
    }
    Err("too many Codex notifications".into())
}
