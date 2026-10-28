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

// A WebSocket frame can arrive across many reads. Enforce the same absolute
// deadline on every underlying read/write, including fragmented/trickling peers.
struct RpcStream {
    stream: UnixStream,
    deadline: std::cell::Cell<Instant>,
}
impl RpcStream {
    fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            deadline: std::cell::Cell::new(Instant::now() + RPC_TIMEOUT),
        }
    }
    fn remaining(&self) -> std::io::Result<Duration> {
        self.deadline
            .get()
            .checked_duration_since(Instant::now())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Codex metadata request timed out",
                )
            })
    }
}
impl Read for RpcStream {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(bytes)
    }
}
impl Write for RpcStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}
type RpcSocket = tungstenite::WebSocket<RpcStream>;
fn rpc_socket(stream: UnixStream) -> Result<RpcSocket> {
    let config = tungstenite::protocol::WebSocketConfig::default()
        .read_buffer_size(4096)
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_RPC_BYTES)
        .max_message_size(Some(MAX_RPC_LINE))
        .max_frame_size(Some(MAX_RPC_LINE));
    let (socket, _) = tungstenite::client::client_with_config(
        "ws://localhost/",
        RpcStream::new(stream),
        Some(config),
    )
    .map_err(|_| std::io::Error::other("Codex WebSocket handshake failed"))?;
    Ok(socket)
}
fn rpc_send(socket: &mut RpcSocket, value: Value) -> Result<()> {
    socket.get_ref().deadline.set(Instant::now() + RPC_TIMEOUT);
    socket.send(tungstenite::Message::Text(value.to_string().into()))?;
    Ok(())
}
fn rpc_result(socket: &mut RpcSocket, id: u64) -> Result<Value> {
    let deadline = Instant::now() + RPC_TIMEOUT;
    socket.get_ref().deadline.set(deadline);
    let mut received = 0;
    for _ in 0..32 {
        let text = match socket.read()? {
            tungstenite::Message::Text(text) => text,
            tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => continue,
            _ => return Err("Codex closed discovery connection or sent a non-text reply".into()),
        };
        received += text.len();
        if text.len() > MAX_RPC_LINE || received > MAX_RPC_BYTES {
            return Err("Codex metadata reply exceeds its size limit".into());
        }
        if Instant::now() >= deadline {
            return Err("Codex metadata request timed out".into());
        }
        let value: Value = serde_json::from_str(&text)?;
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
    use tungstenite::{Message, WebSocket, protocol::Role};
    fn pair() -> (RpcSocket, WebSocket<UnixStream>) {
        let (client, peer) = UnixStream::pair().unwrap();
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(MAX_RPC_LINE))
            .max_frame_size(Some(MAX_RPC_LINE));
        (
            WebSocket::from_raw_socket(RpcStream::new(client), Role::Client, Some(config)),
            WebSocket::from_raw_socket(peer, Role::Server, None),
        )
    }
    #[test]
    fn independent_websocket_messages_and_pretty_json_are_preserved() {
        let (mut client, mut peer) = pair();
        peer.send(Message::Text(
            "{\n\"id\":1,\n\"result\":{\"ok\":true}\n}".into(),
        ))
        .unwrap();
        peer.send(Message::Text(
            json!({"id":2,"result":"next"}).to_string().into(),
        ))
        .unwrap();
        assert_eq!(rpc_result(&mut client, 1).unwrap(), json!({"ok":true}));
        assert_eq!(rpc_result(&mut client, 2).unwrap(), json!("next"));
    }
    #[test]
    fn trickling_partial_websocket_frame_cannot_extend_deadline() {
        let (client, mut peer) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            peer.write_all(&[0x81, 126, 0, 200]).unwrap();
            for _ in 0..100 {
                if peer.write_all(b" ").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        let mut socket = WebSocket::from_raw_socket(RpcStream::new(client), Role::Client, None);
        let begin = Instant::now();
        assert!(rpc_result(&mut socket, 1).is_err());
        let elapsed = begin.elapsed();
        drop(socket);
        worker.join().unwrap();
        assert!(
            elapsed >= Duration::from_millis(200),
            "timeout too early: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "trickle prolonged timeout: {elapsed:?}"
        );
    }
    #[test]
    fn malformed_error_closed_and_notification_flood_replies_are_rejected() {
        for reply in ["not-json", "{\"id\":1,\"error\":{\"code\":1}}"] {
            let (mut client, mut peer) = pair();
            peer.send(Message::Text(reply.into())).unwrap();
            assert!(rpc_result(&mut client, 1).is_err());
        }
        let (mut client, mut peer) = pair();
        peer.close(None).unwrap();
        assert!(rpc_result(&mut client, 1).is_err());
        let (mut client, mut peer) = pair();
        for _ in 0..32 {
            peer.send(Message::Text("{\"method\":\"notice\"}".into()))
                .unwrap();
        }
        assert!(
            rpc_result(&mut client, 1)
                .unwrap_err()
                .to_string()
                .contains("too many")
        );
    }
    #[test]
    fn message_and_cumulative_reply_bytes_are_bounded() {
        for oversized in [true, false] {
            let (mut client, mut peer) = pair();
            let worker = thread::spawn(move || {
                let text = if oversized {
                    " ".repeat(MAX_RPC_LINE + 1)
                } else {
                    json!({"method":"notice","body":"x".repeat(800_000)}).to_string()
                };
                for _ in 0..3 {
                    if peer.send(Message::Text(text.clone().into())).is_err() {
                        break;
                    }
                }
            });
            assert!(rpc_result(&mut client, 1).is_err());
            drop(client);
            worker.join().unwrap();
        }
    }
}
