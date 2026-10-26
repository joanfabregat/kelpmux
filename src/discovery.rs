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
    time::{Duration, Instant},
};

mod claude;
mod codex;
pub mod polling;

#[derive(Clone)]
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
        self.scan_tree(panes, time, &tree, &mut polling::builtins(), &[])
    }
    pub fn scan_tree(
        &self,
        panes: &[Pane],
        time: u64,
        tree: &BTreeMap<u32, Process>,
        adapters: &mut [Box<dyn polling::Adapter>],
        links: &[polling::Link],
    ) -> Result<Vec<Agent>> {
        let mut result = Vec::new();
        for pane in panes {
            let mut candidates: Vec<_> = process::descendants(tree, pane.pid)
                .into_iter()
                .filter_map(|pid| {
                    let proc = tree.get(&pid)?;
                    let args = process::args(&self.proc_root, pid);
                    adapters
                        .iter()
                        .position(|adapter| adapter.matches(proc, &args))
                        .map(|index| (proc, index, args))
                })
                .collect();
            // Parent-most interactive agent wins. Nested tool processes are not new panes.
            candidates.sort_by_key(|(p, _, _)| depth(tree, p.pid, pane.pid));
            // npm's Codex launcher is a node parent; metadata belongs to its native child.
            candidates.retain(|(p, index, _)| {
                !(p.command == "node"
                    && tree.values().any(|child| {
                        child.parent == p.pid && child.command == adapters[*index].kind().as_str()
                    }))
            });
            let Some((proc, index, args)) = candidates.first() else {
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
                kind: adapters[*index].kind(),
                session_id: None,
                name: Path::new(&cwd)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                cwd,
                status: Status::Unknown,
                observation: crate::model::Observation::checked(
                    crate::model::StatusSource::Process,
                    time,
                ),
                source: "process".into(),
                first_seen: time,
                last_seen: time,
                ended_at: None,
            };
            if let Some(link) = links.iter().find(|link| {
                link.pane == pane.id
                    && link.pid == proc.pid
                    && link.birth == proc.start
                    && link.kind == agent.kind
                    && session_id(&link.session_id)
            }) {
                agent.session_id = Some(link.session_id.clone());
                agent.source = "explicit-link".into();
            }
            let mut enriched = agent.clone();
            if adapters[*index]
                .poll(self, proc, args, &mut enriched)
                .is_ok()
                && enriched.pid == agent.pid
                && enriched.process_start == agent.process_start
                && enriched.pane == agent.pane
                && enriched.kind == agent.kind
                && (agent.source != "explicit-link" || enriched.session_id == agent.session_id)
            {
                agent = enriched;
            }
            result.push(agent);
        }
        Ok(result)
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
const RPC_TIMEOUT: Duration = Duration::from_millis(300);
const MAX_RPC_LINE: usize = 1024 * 1024;
const MAX_RPC_BYTES: usize = 2 * 1024 * 1024;

fn rpc_result(reader: &mut BufReader<UnixStream>, id: u64) -> Result<Value> {
    // A per-read timeout alone lets a continuously trickling peer hold this
    // worker forever. Notifications and partial lines share one fixed deadline.
    let deadline = Instant::now() + RPC_TIMEOUT;
    let mut received = 0;
    for _ in 0..32 {
        let mut line = Vec::new();
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("Codex metadata request timed out")?;
            reader.get_ref().set_read_timeout(Some(remaining))?;
            let bytes = match reader.fill_buf() {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if bytes.is_empty() {
                return Err("Codex closed discovery connection".into());
            }
            let newline = bytes.iter().position(|byte| *byte == b'\n');
            let count = newline.map_or(bytes.len(), |position| position + 1);
            received += count;
            if line.len() + count > MAX_RPC_LINE || received > MAX_RPC_BYTES {
                return Err("Codex metadata reply exceeds its size limit".into());
            }
            line.extend_from_slice(&bytes[..count]);
            reader.consume(count);
            if newline.is_some() {
                break;
            }
        }
        if Instant::now() >= deadline {
            return Err("Codex metadata request timed out".into());
        }
        let value: Value = serde_json::from_slice(&line)?;
        if value["id"].as_u64() == Some(id) {
            if value.get("error").is_some() {
                return Err("Codex rejected metadata request".into());
            }
            return Ok(value["result"].clone());
        }
    }
    Err("too many Codex notifications".into())
}

#[cfg(test)]
mod rpc_tests {
    use super::*;
    use std::thread;

    #[test]
    fn fragmented_reply_and_buffered_following_reply_are_preserved() {
        let (client, mut peer) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            for bytes in [
                b"{\"id\":1,".as_slice(),
                b"\"result\":{\"ok\":true}}\n{\"id\":2,\"result\":\"next\"}\n",
            ] {
                peer.write_all(bytes).unwrap();
                thread::sleep(Duration::from_millis(5));
            }
        });
        let mut reader = BufReader::new(client);
        assert_eq!(rpc_result(&mut reader, 1).unwrap(), json!({"ok": true}));
        assert_eq!(rpc_result(&mut reader, 2).unwrap(), json!("next"));
        worker.join().unwrap();
    }

    #[test]
    fn trickling_unterminated_line_cannot_extend_absolute_deadline() {
        let (client, mut peer) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            let finish = Instant::now() + Duration::from_secs(2);
            while Instant::now() < finish {
                if peer.write_all(b" ").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        let begin = Instant::now();
        let mut reader = BufReader::new(client);
        assert!(rpc_result(&mut reader, 1).is_err());
        let elapsed = begin.elapsed();
        drop(reader);
        worker.join().unwrap();
        assert!(
            elapsed >= Duration::from_millis(200),
            "timeout occurred unexpectedly early: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "trickling reply prolonged timeout: {elapsed:?}"
        );
    }

    #[test]
    fn irrelevant_notifications_cannot_reset_deadline() {
        let (client, mut peer) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            for _ in 0..20 {
                if peer.write_all(b"{\"method\":\"notice\"}\n").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
        });
        let begin = Instant::now();
        let mut reader = BufReader::new(client);
        assert!(rpc_result(&mut reader, 1).is_err());
        let elapsed = begin.elapsed();
        drop(reader);
        worker.join().unwrap();
        assert!(elapsed >= Duration::from_millis(200));
        assert!(elapsed < Duration::from_millis(800));
    }

    #[test]
    fn malformed_error_closed_and_notification_flood_replies_are_rejected() {
        for reply in [
            b"not-json\n".as_slice(),
            b"{\"id\":1,\"error\":{\"code\":1}}\n",
            b"{\"id\":1",
        ] {
            let (client, mut peer) = UnixStream::pair().unwrap();
            peer.write_all(reply).unwrap();
            drop(peer);
            assert!(rpc_result(&mut BufReader::new(client), 1).is_err());
        }
        let (client, mut peer) = UnixStream::pair().unwrap();
        peer.write_all(b"{\"method\":\"notice\"}\n".repeat(32).as_slice())
            .unwrap();
        assert!(
            rpc_result(&mut BufReader::new(client), 1)
                .unwrap_err()
                .to_string()
                .contains("too many")
        );
    }

    #[test]
    fn oversized_line_and_cumulative_reply_bytes_are_bounded() {
        for reply in [vec![b' '; MAX_RPC_LINE + 1], {
            // Valid individual JSON notifications together exceed the RPC budget.
            let notification = format!("{{\"method\":\"{}\"}}\n", "x".repeat(800_000));
            notification.repeat(3).into_bytes()
        }] {
            let (client, mut peer) = UnixStream::pair().unwrap();
            let worker = thread::spawn(move || {
                let _ = peer.write_all(&reply);
            });
            let mut reader = BufReader::new(client);
            assert!(
                rpc_result(&mut reader, 1)
                    .unwrap_err()
                    .to_string()
                    .contains("size limit")
            );
            drop(reader);
            worker.join().unwrap();
        }
    }
}
