use super::*;
pub(super) fn enrich(
    context: &Discovery,
    daemon: &mut Daemon,
    proc: &Process,
    args: &[String],
    agent: &mut Agent,
) {
    if agent.source != "explicit-link" {
        agent.session_id = process::environment(&context.proc_root, proc.pid, "CODEX_THREAD_ID")
            .filter(|s| session_id(s))
            .or_else(|| resume_id(args, false));
        if agent.session_id.is_some() {
            agent.source = "process-identity".into();
        }
    }
    let mut rollout = None;
    let mut open_rollouts = BTreeMap::new();
    for path in process::links(&context.proc_root, proc.pid) {
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
                        if let Some(id) = value["payload"]["id"].as_str().filter(|s| session_id(s))
                        {
                            open_rollouts.insert(id.to_owned(), path);
                        }
                    }
                }
            }
        }
    }
    // Several open conversations in a process do not identify the active TUI.
    // Never let arbitrary descriptor iteration choose an unrelated session.
    if open_rollouts.len() == 1 && agent.session_id.is_none() {
        let (id, path) = open_rollouts.into_iter().next().unwrap();
        agent.session_id = Some(id);
        agent.source = "open-rollout".into();
        rollout = Some(path);
    } else if let Some(id) = &agent.session_id {
        rollout = open_rollouts.remove(id);
    }
    let Some(id) = agent.session_id.clone() else {
        return;
    };
    if let Some(name) = index_name(&context.codex_home.join("session_index.jsonl"), &id) {
        agent.name = name;
    }
    // Connect only to an already-running daemon; never start or resume an agent.
    if let Ok(value) = daemon.read(context, &id) {
        let thread = &value["thread"];
        if thread["id"].as_str() == Some(&id) {
            if let Some(name) = thread["name"].as_str().filter(|s| !s.is_empty()) {
                agent.name = name.into();
            }
            agent.status = codex_status(&thread["status"]);
            agent.observation.source = crate::model::StatusSource::AppServer;
            return;
        }
    }
    if rollout.is_none() {
        rollout = find_rollout(&context.codex_home.join("sessions"), &id, 0);
    }
    if let Some(path) = rollout {
        agent.status = rollout_status(&path);
        agent.observation.source = crate::model::StatusSource::Transcript;
    }
}

/// One bounded connection and one cached metadata result per thread per cycle.
#[derive(Default)]
pub(super) struct Daemon {
    deadline: Option<Instant>,
    reader: Option<RpcSocket>,
    failed: bool,
    next: u64,
    cache: BTreeMap<String, Value>,
}
impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(socket) = &mut self.reader {
            socket
                .get_ref()
                .deadline
                .set(Instant::now() + Duration::from_millis(25));
            let _ = socket.close(None);
        }
    }
}
impl Daemon {
    fn request(&mut self, context: &Discovery, method: &str, params: Value) -> Result<Value> {
        let deadline = *self
            .deadline
            .get_or_insert_with(|| Instant::now() + Duration::from_millis(1200));
        if Instant::now() >= deadline {
            self.failed = true;
        }
        if self.failed {
            return Err("Codex metadata unavailable for this cycle".into());
        }
        let result = self.exchange(context, method, params);
        if result.is_err() {
            self.failed = true;
            self.reader = None;
        }
        result
    }
    fn exchange(&mut self, context: &Discovery, method: &str, params: Value) -> Result<Value> {
        if self.reader.is_none() {
            let stream = UnixStream::connect(
                context
                    .codex_home
                    .join("app-server-control/app-server-control.sock"),
            )?;
            let mut reader = rpc_socket(stream)?;
            rpc_send(
                &mut reader,
                json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"kelpmux","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}}),
            )?;
            rpc_result(&mut reader, 1)?;
            rpc_send(&mut reader, json!({"method":"initialized"}))?;
            self.next = 2;
            self.reader = Some(reader);
        }
        let id = self.next;
        self.next += 1;
        let reader = self.reader.as_mut().unwrap();
        rpc_send(reader, json!({"id":id,"method":method,"params":params}))?;
        rpc_result(reader, id)
    }
    fn read(&mut self, context: &Discovery, id: &str) -> Result<Value> {
        if let Some(value) = self.cache.get(id) {
            return Ok(value.clone());
        }
        if self.cache.len() >= 128 {
            return Err("Codex metadata cycle capacity reached".into());
        }
        let value = self.request(
            context,
            "thread/read",
            json!({"threadId":id,"includeTurns":false}),
        )?;
        if value["thread"]["id"].as_str() != Some(id) {
            return Err("Codex returned a different thread identity".into());
        }
        let thread = &value["thread"];
        let status_type = thread["status"]["type"]
            .as_str()
            .filter(|value| value.len() <= 64);
        let flags: Vec<_> = thread["status"]["activeFlags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|flag| ["waitingOnApproval", "waitingOnUserInput"].contains(flag))
            .collect();
        let name = thread["name"].as_str().map(|name| {
            name.chars()
                .filter(|c| !c.is_control())
                .take(128)
                .collect::<String>()
        });
        let summary = json!({"thread":{"id":id,"name":name,"status":{"type":status_type,"activeFlags":flags}}});
        self.cache.insert(id.into(), summary.clone());
        Ok(summary)
    }
    pub(super) fn sessions(&mut self, context: &Discovery) -> Result<Vec<polling::Session>> {
        let value = self.request(context, "thread/loaded/list", json!({}))?;
        let ids = value["data"]
            .as_array()
            .ok_or("unsupported Codex loaded-thread schema")?;
        let mut sessions = Vec::new();
        for id in ids
            .iter()
            .take(128)
            .filter_map(Value::as_str)
            .filter(|id| session_id(id))
        {
            let Ok(value) = self.read(context, id) else {
                continue;
            };
            let thread = &value["thread"];
            sessions.push(polling::Session {
                kind: Kind::Codex,
                id: id.into(),
                name: thread["name"].as_str().unwrap_or(id).into(),
                status: codex_status(&thread["status"]),
            });
        }
        Ok(sessions)
    }
}
