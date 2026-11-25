//! Terminal-only SSH adapter for the browser client. OpenSSH runs it on the
//! authenticated Linux account; it relays the independent pane stream and a
//! small set of request-correlated terminal operations over standard I/O.
//!
//! It never listens on TCP and never restarts or replaces a workspace server.
//! When its own configured workspace is not running it offers one `start`
//! operation, run only on an explicit browser request. It never
//! changes SSH authorization and holds no project or Git management; file
//! operations are KelpServer's `files` stream requests, relayed unchanged
//! like typing into a pane. It only reads, for display, which repository
//! and branch a pane's folder is in and which program a pane runs
//! (`web_repos`), without running `git` or any other process. The `git`
//! operation, for a project's Repos view, runs read-only Git commands
//! through the hardened runner of `web_git` (no hooks, filters, textconv,
//! external diff, fsmonitor or network), never a write. The environment
//! operations (`environments`) start and stop a project's declared
//! environments in panes of their own, only on the browser's explicit
//! request, and run deploy environments' `status` scripts on threads of
//! their own.
//!
//! Its operations use KelpServer's stream session API on a separate,
//! short-lived stream connection each (`session`), never the composed UI's
//! administration.
use super::{
    connection, environments,
    projects::Links,
    protocol::*,
    session::{Session, request_id},
    status,
    stream::{self, ClosedSession, Event, Request},
    user_config, web_git, web_history, web_repos,
};
use crate::Result;
use serde_json::{Value, json};
use std::{
    io,
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

/// Version of the `Web` request API spoken by this bridge.
/// Version 2 added `color`, `closed`, `restore`, `start` (with the
/// `Workspace` notice while the server is stopped) and notification
/// acknowledgements. Version 3 removed `color`: tab colours are each
/// client's own state (ADR 0002), kept per browser.
pub const API_VERSION: u64 = 3;
/// Additive revision within [`API_VERSION`]: clients of the same version
/// keep working and detect newer operations from `state`'s `operations`.
/// Minor 1 added `uploads`: the bridge relays the stream's upload requests
/// (`upload_begin`, `upload_chunk`, `upload_finish`, `upload_cancel`) and
/// their events, which a bridge of minor 0 cannot parse. Minor 2 added
/// `files`: the bridge relays KelpServer's `file_*` stream requests and
/// their replies (and Welcome `file_limits`) unchanged, which a bridge of
/// minor 1 cannot parse. Minor 3 added `downloads`: the bridge relays
/// KelpServer's `file_download_*` requests and events (KelpCore
/// `file-downloads`), which a bridge of minor 2 cannot parse. Minor 4
/// added `config`: the `config` and `config_set` operations on the user
/// configuration (`user_config`), and the `Config` notice the bridge sends
/// when the file changes.
/// Minor 5 added `projects`: the `projects`, `project_move`,
/// `project_create`, `project_update` and `project_delete` operations, a
/// `project` for `new`, the `project` of each `closed` entry, and the
/// `projects` of the `config` result. Minor 6 added `git`: read-only Git
/// facts of a project's checkouts (`web_git`), computed on the bridge's
/// own Git worker threads and answered when ready. Minor 7 added
/// `conversations`: the History listing (closed terminals with the agent
/// conversations they hosted, titled from their first prompt, and
/// conversations started outside KelpMux in a project's folder;
/// `web_history`), answered by the bridge's History thread, and the
/// `resume` parameter of `new` (a conversation by id, or the agent's own
/// picker). Minor 8 added `environments`: the `environments`,
/// `environment_start`, `environment_stop`, `environment_check` and
/// `environment_panes` operations on a project's environments
/// (`environments`, ADR 0005), run on their own threads.
pub const API_MINOR: u64 = 8;
/// Largest accepted `Web` request, in bytes of JSON.
const REQUEST_LIMIT: usize = 65536;
/// Largest request identifier a browser can represent exactly.
const MAX_ID: u64 = 9_007_199_254_740_991;

/// The server process behind the workspace socket. Each management exchange
/// opens a fresh same-user connection and checks that it reaches the same
/// process, so a replaced server cannot receive a request meant for the old one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Peer {
    pid: u32,
    birth: u64,
}
impl Peer {
    fn read(socket: &UnixStream) -> Result<Self> {
        let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut credentials as *mut libc::ucred).cast(),
                &mut length,
            )
        } != 0
            || credentials.pid <= 0
            || credentials.uid != unsafe { libc::geteuid() }
        {
            return Err("terminal peer identity unavailable".into());
        }
        let process = crate::process::read(Path::new("/proc"), credentials.pid as u32)?;
        Ok(Self {
            pid: process.pid,
            birth: process.start,
        })
    }
}

#[derive(Default)]
struct Runtime {
    epoch: String,
    capabilities: Vec<String>,
}

/// Opens checked stream sessions to the relayed server; cloned into the
/// environment threads.
#[derive(Clone)]
struct Sessions {
    peer: Peer,
    runtime: Arc<Mutex<Runtime>>,
    socket: PathBuf,
}
impl Sessions {
    /// A fresh stream connection to the same server process and epoch as
    /// the relayed stream, offering `capability`.
    fn open(&self, capability: &str) -> Result<Session> {
        let socket = connection::connect(&self.socket)?;
        if Peer::read(&socket)? != self.peer {
            return Err("terminal server changed; reconnect before continuing".into());
        }
        let session = Session::open(socket)?;
        if session.epoch != self.runtime()?.0 {
            return Err("terminal server changed; reconnect before continuing".into());
        }
        session
            .require(capability)
            .map_err(|_| "this terminal server cannot do this for web clients")?;
        Ok(session)
    }
    /// The relayed stream's epoch and capabilities.
    fn runtime(&self) -> Result<(String, Vec<String>)> {
        let runtime = self.runtime.lock().map_err(|_| "runtime unavailable")?;
        if runtime.epoch.is_empty() {
            return Err("terminal handshake is incomplete".into());
        }
        Ok((runtime.epoch.clone(), runtime.capabilities.clone()))
    }
}

/// Most environment operations (`environments`, `environment_start`,
/// `environment_stop`) one bridge runs at once, each on its own thread: a
/// deploy environment's `status` script may take up to ten seconds and a
/// Stop waits for its process, so they never hold up the terminals.
const ENVIRONMENT_THREADS: usize = 3;

struct Api {
    sessions: Sessions,
    environment_threads: Arc<std::sync::atomic::AtomicUsize>,
    state: PathBuf,
    monitor: status::Monitor,
    config: Option<Arc<user_config::Live>>,
    git: web_git::Worker,
    history: web_history::Worker,
}

fn pane(value: &Value) -> Result<u64> {
    value["pane"]
        .as_u64()
        .filter(|p| *p > 0)
        .ok_or_else(|| "pane required".into())
}

impl Api {
    /// A fresh stream connection to the same server process and epoch as
    /// the relayed stream, offering `capability`.
    fn session(&self, capability: &str) -> Result<Session> {
        self.sessions.open(capability)
    }
    /// The relayed stream's epoch and capabilities.
    fn runtime(&self) -> Result<(String, Vec<String>)> {
        self.sessions.runtime()
    }
    /// What an environment operation on `{"project": id}` needs. Scratch
    /// has no environments; the folder comes from the configuration.
    fn environment_context(&self, p: &Value) -> Result<environments::Context> {
        let config = self.user_config()?;
        let project = match p["project"].as_str() {
            Some(user_config::SCRATCH) => {
                return Err("Scratch has no environments".into());
            }
            Some(id) => config
                .project(id)
                .cloned()
                .ok_or("this project no longer exists")?,
            None => return Err("project required".into()),
        };
        Ok(environments::Context {
            project,
            workspace: self.state.clone(),
            ports: config.environment_ports.1,
            base: environments::state_base()?,
            exe: std::env::current_exe()?,
        })
    }
    /// Runs an environment operation on its own thread (at most
    /// [`ENVIRONMENT_THREADS`] at once); `reply` answers when it is done.
    /// `environments {project}` lists the project's environments with their
    /// state, without running any script; `environment_check {project,
    /// name}` runs a deploy environment's `status` script (the only
    /// operation that does); `environment_start` and `environment_stop`
    /// take `{project, name, request}`.
    fn environment(
        &self,
        method: &str,
        p: &Value,
        reply: Box<dyn FnOnce(std::result::Result<Value, String>) + Send>,
    ) -> Result<()> {
        let context = self.environment_context(p)?;
        let name = p["name"].as_str().unwrap_or("").to_owned();
        let request = p["request"].as_str().unwrap_or("").to_owned();
        if method != "environments" && !environments::valid_name(&name) {
            return Err("environment name required".into());
        }
        if matches!(method, "environment_start" | "environment_stop")
            && !stream::valid_request(&request)
        {
            return Err("environment request identity required".into());
        }
        /// Gives the thread's slot back however the thread ends.
        struct Slot(Arc<std::sync::atomic::AtomicUsize>);
        impl Drop for Slot {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::AcqRel);
            }
        }
        let threads = self.environment_threads.clone();
        if threads.fetch_add(1, Ordering::AcqRel) >= ENVIRONMENT_THREADS {
            threads.fetch_sub(1, Ordering::AcqRel);
            return Err("busy with other environment operations; try again".into());
        }
        let slot = Slot(threads);
        let sessions = self.sessions.clone();
        let method = method.to_owned();
        std::thread::spawn(move || {
            let _slot = slot;
            let open = |capability: &str| sessions.open(capability);
            let result = match method.as_str() {
                "environments" => environments::list(&context, &open),
                "environment_start" => environments::start(&context, &name, &request, &open),
                // Runs the `status` script: only on this explicit request.
                "environment_check" => environments::status(&context, &name, &open, true),
                "environment_stop" => environments::stop(&context, &name, &request, &open),
                other => Err(format!("unsupported environment operation {other}").into()),
            };
            reply(result.map_err(|error| error.to_string()));
        });
        Ok(())
    }
    fn request(&mut self, method: &str, p: &Value) -> Result<Value> {
        match method {
            "state" => Ok(json!({
                "api_version": API_VERSION,
                "api_minor": API_MINOR,
                "workspace": self.state.file_name().unwrap_or_default().to_string_lossy(),
                "version": env!("CARGO_PKG_VERSION"),
                "server_capabilities": self.runtime()?.1,
                "operations": OPERATIONS,
            })),
            "new" => self.spawn(p),
            "close" => {
                let id = pane(p)?;
                if p["confirmed"] != true {
                    return Err("closing a pane requires explicit confirmation".into());
                }
                // This explicitly confirmed same-user action may close a pane
                // another client is using: take its lease first (the server
                // refuses to close a pane another connection leases).
                let mut session = self.session("pane-lifecycle")?;
                session.claim(id)?;
                session.call(Request::Close {
                    request: request_id("web-close"),
                    pane: id,
                })?;
                Ok(json!({ "pane": id }))
            }
            "search" => {
                let query = p["query"].as_str().ok_or("search query required")?;
                if query.is_empty() || query.len() > 256 || query.chars().any(char::is_control) {
                    return Err("search must be 1–256 bytes without control characters".into());
                }
                let id = pane(p)?;
                let Event::Search { results, .. } =
                    self.session("pane-history")?.call(Request::Search {
                        request: request_id("web-search"),
                        panes: vec![id],
                        query: query.to_owned(),
                        limit: 64,
                    })?
                else {
                    return Err("terminal server did not answer the search".into());
                };
                let (hits, truncated) = results
                    .into_iter()
                    .find(|result| result.pane == id)
                    .map_or((Vec::new(), false), |result| {
                        (result.hits, result.truncated)
                    });
                Ok(json!({ "PaneSearch": { "pane": id, "hits": hits, "truncated": truncated } }))
            }
            "history" => {
                let start = p["start"].as_i64().unwrap_or(-200).clamp(-100_000, 0) as i32;
                let limit = p["limit"].as_u64().unwrap_or(200).min(200) as usize;
                let Event::History { pane, lines, .. } =
                    self.session("pane-history")?.call(Request::History {
                        request: request_id("web-history"),
                        pane: pane(p)?,
                        start,
                        limit,
                    })?
                else {
                    return Err("terminal server did not return history".into());
                };
                Ok(json!({ "History": { "pane": pane, "lines": lines } }))
            }
            "status" => {
                let mut value = serde_json::to_value(self.monitor.poll(true))?;
                value["refreshing"] = json!(self.monitor.refreshing());
                Ok(value)
            }
            "start" => Err("the workspace server is already running".into()),
            "repos" => {
                // Read-only repository facts for the Terminals table; see
                // web_repos for the bounds. The browser sends pane folders.
                let paths = p["paths"]
                    .as_array()
                    .ok_or("repos takes a list of paths")?
                    .iter()
                    .map(|path| path.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()
                    .ok_or("each path must be a string")?;
                Ok(json!({ "repos": web_repos::lookup(&paths)? }))
            }
            "commands" => self.commands(),
            "closed" => self.closed(),
            "restore" => self.restore(p),
            "config" | "config_set" => config_request(self.config.as_deref(), method, p),
            "projects" => self.project_panes(),
            "project_move" => self.project_move(p),
            "environment_panes" => Ok(environments::panes(&self.state, &self.runtime()?.0)),
            "project_create" | "project_update" | "project_delete" => {
                self.project_change(method, p)
            }
            _ => Err("unsupported web operation".into()),
        }
    }
    /// Queues a project's Git facts (`web_git`): `{"project": id}` (or
    /// `scratch`), with `files: true` to list changed files and `fresh:
    /// true` to bypass the cache. The folder comes from the user
    /// configuration, never from the browser. `reply` answers later.
    fn git(
        &self,
        p: &Value,
        reply: Box<dyn FnOnce(std::result::Result<Value, String>) + Send>,
    ) -> Result<()> {
        let config = self.user_config()?;
        let folder = match p["project"].as_str() {
            Some(user_config::SCRATCH) => config.scratch_folder.path.clone(),
            Some(id) => config
                .project(id)
                .ok_or("this project no longer exists")?
                .folder
                .path
                .clone(),
            None => return Err("project required".into()),
        };
        let request = web_git::Request {
            folder,
            list: p["files"] == true,
            fresh: p["fresh"] == true,
        };
        Ok(self.git.submit(request, reply)?)
    }
    fn links(&self) -> Links {
        Links::at(&self.state)
    }
    fn user_config(&self) -> Result<user_config::Config> {
        self.config
            .as_deref()
            .and_then(user_config::Live::config)
            .ok_or_else(|| "the user configuration is unavailable: HOME is not set".into())
    }
    /// This run's terminals that belong to a project: `{"panes": {"3":
    /// "kelp"}}`; every other terminal belongs to Scratch.
    fn project_panes(&self) -> Result<Value> {
        let epoch = self.runtime()?.0;
        let panes: serde_json::Map<String, Value> = self
            .links()
            .panes(&epoch)
            .into_iter()
            .map(|(pane, project)| (pane.to_string(), json!(project)))
            .collect();
        Ok(json!({ "panes": panes }))
    }
    /// Moves a live terminal to a project (`project`: an id, or null or
    /// `scratch` for Scratch). Only the link changes, never the shell.
    fn project_move(&self, p: &Value) -> Result<Value> {
        let id = pane(p)?;
        let project = match &p["project"] {
            Value::Null => None,
            Value::String(project) if project == user_config::SCRATCH => None,
            Value::String(project) => Some(project.clone()),
            _ => return Err("project must be a project id or null".into()),
        };
        if let Some(project) = &project
            && self.user_config()?.project(project).is_none()
        {
            return Err("this project no longer exists".into());
        }
        let mut session = self.session("pane-lifecycle")?;
        if !session.update()?.panes.iter().any(|pane| pane.id == id) {
            return Err("this terminal is closed".into());
        }
        let epoch = self.runtime()?.0;
        self.links()
            .set(&Links::key(&epoch, id), project.as_deref())?;
        self.project_panes()
    }
    /// Creates, renames or deletes a project in the user configuration.
    /// Creating takes a name, an optional folder (default `<projects.base>/
    /// <name>`, created when missing) and colour; the id is made from the
    /// name, never reusing one that terminals are still linked to.
    fn project_change(&self, method: &str, p: &Value) -> Result<Value> {
        let live = self
            .config
            .as_deref()
            .ok_or("the user configuration is unavailable: HOME is not set")?;
        let config = self.user_config()?;
        let text = |key: &str| -> Result<Option<String>> {
            match &p[key] {
                Value::Null => Ok(None),
                Value::String(text) if text.len() <= 4096 => Ok(Some(text.clone())),
                _ => Err(format!("{key} must be a string").into()),
            }
        };
        let id = || -> Result<String> { text("id")?.ok_or_else(|| "project id required".into()) };
        let color =
            || -> Result<Option<String>> { Ok(text("color")?.filter(|color| !color.is_empty())) };
        let change = match method {
            "project_create" => {
                let name = text("name")?.ok_or("project name required")?;
                let folder = match text("folder")?.filter(|folder| !folder.trim().is_empty()) {
                    Some(folder) => folder,
                    None => format!(
                        "{}/{name}",
                        config.projects_base.written.trim_end_matches('/')
                    ),
                };
                let mut taken: Vec<String> = config
                    .projects
                    .iter()
                    .map(|project| project.id.clone())
                    .collect();
                taken.extend(self.links().projects());
                user_config::ProjectChange::Create {
                    id: user_config::new_project_id(&name, &taken),
                    name,
                    folder,
                    color: color()?,
                }
            }
            "project_update" => user_config::ProjectChange::Update {
                id: id()?,
                name: text("name")?,
                color: if p.get("color").is_some() {
                    Some(color()?)
                } else {
                    None
                },
            },
            _ => {
                if p["confirmed"] != true {
                    return Err("deleting a project requires explicit confirmation".into());
                }
                user_config::ProjectChange::Delete { id: id()? }
            }
        };
        let mut result = live.change_projects(&change)?;
        if let user_config::ProjectChange::Create { id, .. } = &change
            && result["saved"] == true
        {
            result["project"] = json!(id);
            // The folder is created with the project; a failure leaves the
            // project, shown as missing.
            if let Some(project) = live.config().as_ref().and_then(|c| c.project(id))
                && let Err(error) = std::fs::create_dir_all(&project.folder.path)
            {
                result["warning"] = json!(format!(
                    "the project was saved, but its folder {} could not be created: {error}",
                    project.folder.path.display()
                ));
            }
            result["config"] = live.json();
        }
        Ok(result)
    }
    /// Each live pane's foreground program, by pane id, for the Terminals
    /// table. Process identities come from the server, never the browser;
    /// only program names are returned, never arguments.
    fn commands(&self) -> Result<Value> {
        let mut session = self.session("pane-lifecycle")?;
        let panes = session.update()?.panes;
        let proc_root = Path::new("/proc");
        let commands: serde_json::Map<String, Value> = panes
            .iter()
            .filter(|pane| !pane.dead && pane.pid > 0)
            .take(MAX_COMMAND_PANES)
            .map(|pane| {
                (
                    pane.id.to_string(),
                    json!(web_repos::foreground_program(proc_root, pane.pid)),
                )
            })
            .collect();
        Ok(json!({ "commands": commands }))
    }
    /// The server's closed-session records, newest first (at most 256).
    fn records(session: &mut Session) -> Result<Vec<ClosedSession>> {
        match session.call(Request::ClosedSessions {
            request: request_id("web-closed"),
        })? {
            Event::ClosedSessions { records, .. } => Ok(records),
            _ => Err("terminal server did not list closed terminals".into()),
        }
    }
    /// Recently closed terminals and previous runs, the TUI's Recovery list:
    /// this workspace's first, newest first within each group, with the
    /// restore actions each entry offers.
    fn closed(&self) -> Result<Value> {
        let (current, entries) = self.closed_entries()?;
        Ok(json!({ "workspace": current, "entries": entries }))
    }
    /// Queues the History listing (`web_history`): the `closed` entries,
    /// each with the agent `conversations` it hosted (newest first, titled
    /// from their first prompt), and the `external` conversations started
    /// outside KelpMux in a project's or the Scratch folder. The server is
    /// asked here; the transcripts are read on the History thread, which
    /// answers through `reply`.
    fn conversations(
        &self,
        _: &Value,
        reply: Box<dyn FnOnce(std::result::Result<Value, String>) + Send>,
    ) -> Result<()> {
        let (workspace, entries) = self.closed_entries()?;
        // Conversations a live terminal runs are not outside ones.
        let live = self
            .session("pane-lifecycle")
            .and_then(|mut session| session.update())
            .map(|update| {
                update
                    .panes
                    .iter()
                    .filter_map(|pane| {
                        Some((pane.kind?.as_str().to_owned(), pane.session_id.clone()?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let scopes = self
            .user_config()
            .map(|config| {
                config
                    .projects
                    .iter()
                    .map(|project| web_history::Scope {
                        id: project.id.clone(),
                        folder: project.folder.path.clone(),
                    })
                    .chain([web_history::Scope {
                        id: user_config::SCRATCH.into(),
                        folder: config.scratch_folder.path.clone(),
                    }])
                    .collect()
            })
            .unwrap_or_default();
        let request = web_history::Request {
            workspace,
            entries,
            live,
            scopes,
        };
        Ok(self.history.submit(request, reply)?)
    }
    /// The `closed` entries, with their restore choices and project.
    fn closed_entries(&self) -> Result<(String, Vec<Value>)> {
        let current = self.workspace_name();
        let mut records = Self::records(&mut self.session("closed-sessions")?)?;
        records.sort_by_key(|r| {
            (
                r.record.workspace != current,
                r.record.reason != "Closed",
                std::cmp::Reverse(r.record.closed_at),
            )
        });
        let links = self.links().all();
        let entries = records
            .iter()
            .map(|record| {
                let mut value = serde_json::to_value(&record.record)?;
                value["choices"] = json!(choices(record));
                value["project"] = json!(links.get(&record.record.key));
                Ok(value)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((current, entries))
    }
    /// Reopens a closed terminal through the server's `restore`: `resume`
    /// reopens an agent conversation, `restart` reruns a shell's last
    /// program and `folder` opens a shell in its folder. The command line
    /// comes from the server's own record, never from the browser. An agent
    /// conversation that is already open is returned instead of started
    /// twice.
    fn restore(&self, p: &Value) -> Result<Value> {
        if p["confirmed"] != true {
            return Err("restoring a closed terminal requires explicit confirmation".into());
        }
        let key = p["key"].as_str().ok_or("closed entry required")?;
        let choice = p["choice"].as_str().ok_or("restore choice required")?;
        let request = p["request"]
            .as_str()
            .ok_or("restore request identity required")?;
        let mut session = self.session("closed-sessions")?;
        // `restore` addresses a record of the server's latest list.
        let record = Self::records(&mut session)?
            .into_iter()
            .find(|record| record.record.key == key)
            .ok_or("this closed terminal is no longer listed")?;
        if !choices(&record).contains(&choice) {
            return Err("choose a restore action this entry offers".into());
        }
        let Event::Restored { pane, existing, .. } = session.call(Request::Restore {
            request: request.to_owned(),
            record: key.to_owned(),
            rerun: choice == "restart",
        })?
        else {
            return Err("terminal server did not identify the restored pane".into());
        };
        // The reopened terminal stays in the closed one's project.
        if !existing && let Ok((epoch, _)) = self.runtime() {
            let _ = self.links().inherit(key, &Links::key(&epoch, pane));
        }
        Ok(json!({ "pane": pane, "existing": existing }))
    }
    /// Starts a shell in a new pane through the stream's `spawn` request.
    /// The browser supplies no command line; only an optional `project`
    /// (the terminal belongs to it and starts in its folder) and an
    /// optional absolute working directory. Without either it starts in the
    /// Scratch folder (`scratch.folder`, created when missing). A retry with
    /// the same request returns the same pane. With `resume` (`{kind, id}`)
    /// the pane runs that agent conversation's resume command
    /// ([`resume_argv`]), or the terminal already running it is returned
    /// with `existing`; with `{kind, picker: true}` it opens the agent's
    /// own conversation picker.
    fn spawn(&self, p: &Value) -> Result<Value> {
        let request = p["request"]
            .as_str()
            .ok_or("new pane request identity required")?;
        let project = match &p["project"] {
            Value::Null => None,
            Value::String(id) if id == user_config::SCRATCH => None,
            Value::String(id) => Some(
                self.user_config()?
                    .project(id)
                    .cloned()
                    .ok_or("this project no longer exists")?,
            ),
            _ => return Err("project must be a project id".into()),
        };
        let cwd = match (p.get("cwd"), &project) {
            (None | Some(Value::Null), Some(project)) => {
                if !project.folder.path.is_dir() {
                    return Err(format!(
                        "the folder of “{}” ({}) is missing",
                        project.name,
                        project.folder.path.display()
                    )
                    .into());
                }
                project.folder.path.clone()
            }
            (None | Some(Value::Null), None) => match self.user_config() {
                Ok(config) => {
                    let scratch = config.scratch_folder.path;
                    if !scratch.is_dir() {
                        std::fs::create_dir_all(&scratch).map_err(|error| {
                            format!(
                                "the Scratch folder {} could not be created: {error}",
                                scratch.display()
                            )
                        })?;
                    }
                    scratch
                }
                Err(_) => home()?,
            },
            (Some(Value::String(path)), _) => PathBuf::from(path),
            _ => return Err("cwd must be an absolute directory".into()),
        };
        if !cwd.is_absolute() || !cwd.is_dir() {
            return Err("cwd must be an existing absolute directory".into());
        }
        let resume = match &p["resume"] {
            Value::Null => None,
            value => Some(resume_argv(value)?),
        };
        let mut session = self.session("pane-lifecycle")?;
        if let Some((kind, Some(id), _)) = &resume {
            // A conversation that is already open is shown, not started twice.
            let update = session.update()?;
            if let Some(pane) = update.panes.iter().find(|pane| {
                !pane.dead
                    && pane.kind.map(|k| k.as_str()) == Some(*kind)
                    && pane.session_id.as_deref() == Some(id.as_str())
            }) {
                return Ok(json!({ "pane": pane.id, "existing": true }));
            }
        }
        let Event::Spawned { pane, .. } = session.call(Request::Spawn {
            request: request.to_owned(),
            cwd: Some(cwd),
            argv: resume.map(|(_, _, argv)| argv).unwrap_or_default(),
            shell: None,
        })?
        else {
            return Err("terminal server did not identify the created pane".into());
        };
        if let Some(project) = &project {
            let epoch = self.runtime()?.0;
            self.links()
                .set(&Links::key(&epoch, pane), Some(&project.id))?;
        }
        Ok(json!({ "pane": pane }))
    }
    fn workspace_name(&self) -> String {
        self.state
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }
}

/// Bridge operations, reported by `state`. `uploads`, `files` and
/// `downloads` are not `Web` methods: they tell the browser that this
/// bridge relays the stream's upload, file and download requests.
const OPERATIONS: &[&str] = &[
    "state",
    "new",
    "close",
    "search",
    "history",
    "status",
    "closed",
    "restore",
    "start",
    "repos",
    "commands",
    "uploads",
    "files",
    "downloads",
    "config",
    "projects",
    "git",
    "conversations",
    "environments",
];

/// The user configuration operations, while the workspace runs or not:
/// `config` returns the file's state (the configuration in effect, its
/// problems), `config_set` validates and saves `params.changes`
/// (`{"files.root": "~", …}`) and returns `{saved, errors, config}`. The
/// browser chooses values, never the file: it is this user's own.
fn config_request(config: Option<&user_config::Live>, method: &str, p: &Value) -> Result<Value> {
    let config = config.ok_or("the user configuration is unavailable: HOME is not set")?;
    match method {
        "config" => Ok(config.json()),
        _ => config.set(&p["changes"]),
    }
}

/// Watches the user configuration for this connection. Changes reach the
/// browser as `Config` notices once the bridge relays the workspace
/// (`running`); while it is stopped the browser asks with `config`.
fn live_config(
    sender: mpsc::SyncSender<Outbound>,
    running: Arc<AtomicBool>,
) -> Option<Arc<user_config::Live>> {
    let path = user_config::default_path().ok()?;
    let env = user_config::Env::current().ok()?;
    Some(user_config::Live::start(
        path,
        env,
        Box::new(move |value| {
            !running.load(Ordering::Acquire) || sender.send(Outbound::Message(value)).is_ok()
        }),
    ))
}
/// Most panes whose foreground program one `commands` call reports.
const MAX_COMMAND_PANES: usize = 128;

/// The restore actions a closed entry offers, as the TUI's Recovery dialog
/// names them: an agent resumes its conversation (or opens the agent's own
/// picker when the conversation is unknown), a shell whose last program is
/// known can rerun it, and every shell can reopen its folder.
fn choices(record: &ClosedSession) -> Vec<&'static str> {
    if record.record.kind.is_some() {
        vec!["resume"]
    } else if record.rerun.is_some() {
        vec!["restart", "folder"]
    } else {
        vec!["folder"]
    }
}

/// The command line that resumes an agent conversation, from `new`'s
/// `resume`: `{"kind": "claude" | "codex", "id": …}` runs the same
/// commands the server's `restore` runs (`claude --resume ID`,
/// `codex resume ID`); `{"kind": …, "picker": true}` without an id opens
/// the agent's own conversation picker (`claude --resume`,
/// `codex resume`), for a closed terminal whose conversation was never
/// identified. Built here from a fixed table and a validated conversation
/// id, never from a command line the browser sends; an id that is present
/// but invalid is refused, never turned into the picker.
fn resume_argv(value: &Value) -> Result<(&'static str, Option<String>, Vec<String>)> {
    let (kind, program, verb) = match value["kind"].as_str() {
        Some("claude") => ("claude", "claude", "--resume"),
        Some("codex") => ("codex", "codex", "resume"),
        _ => return Err("resume kind must be claude or codex".into()),
    };
    if value["id"].is_null() && value["picker"] == true {
        return Ok((kind, None, vec![program.into(), verb.into()]));
    }
    let id = value["id"]
        .as_str()
        .filter(|id| crate::session_id(id))
        .ok_or("resume needs a valid conversation id")?;
    Ok((
        kind,
        Some(id.to_owned()),
        vec![program.into(), verb.into(), id.into()],
    ))
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "HOME is not an absolute directory".into())
}

/// Parses `web-bridge` options. The workspace name selects the state
/// directory; an explicit state directory or socket overrides it.
pub fn paths(args: &[String]) -> Result<(PathBuf, PathBuf)> {
    let mut workspace = None;
    let mut explicit_state = None;
    let mut socket = None;
    for pair in args.chunks(2) {
        if pair.len() != 2 {
            return Err("option needs a value".into());
        }
        match pair[0].as_str() {
            "--workspace" => workspace = Some(super::lifecycle::workspace_path(&pair[1])?),
            "--socket" => socket = Some(PathBuf::from(&pair[1])),
            "--state-dir" => explicit_state = Some(PathBuf::from(&pair[1])),
            _ => return Err("unknown web-bridge option".into()),
        }
    }
    let state = match explicit_state.or(workspace) {
        Some(state) => state,
        None => match &socket {
            Some(path) => path.parent().ok_or("socket needs a parent")?.to_path_buf(),
            None => super::lifecycle::workspace_path("main")?,
        },
    };
    let absolute = |path: PathBuf| -> Result<PathBuf> {
        Ok(if path.is_absolute() {
            path
        } else {
            std::env::current_dir()?.join(path)
        })
    };
    let state = absolute(state)?;
    let socket = absolute(socket.unwrap_or_else(|| state.join("control.sock")))?;
    Ok((socket, state))
}

/// Whether no workspace server is running: no socket, or a socket nothing
/// listens on. Any other connection failure is reported, not started over.
fn server_absent(socket: &Path) -> bool {
    match UnixStream::connect(socket) {
        Ok(_) => false,
        Err(error) => matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
        ),
    }
}

/// Starts this bridge's own workspace on an explicit browser request. The
/// request chooses nothing: no workspace, server binary or replacement.
fn start(socket: &Path, state: &Path, params: &Value) -> Result<Value> {
    let options = params
        .as_object()
        .ok_or("start parameters must be an object")?;
    if params["confirmed"] != true {
        return Err("starting the workspace requires explicit confirmation".into());
    }
    if options.keys().any(|key| key != "confirmed") {
        return Err("start takes no options; it starts this bridge's own workspace".into());
    }
    if !server_absent(socket) {
        return Err("the workspace server is already running".into());
    }
    super::cli::start_workspace(state, socket)?;
    // The server starts without panes; like the TUI, the browser that
    // started it gets one shell to work in.
    first_shell(socket)?;
    Ok(json!({ "started": true }))
}

/// Creates one shell in a server that still has none (another client may
/// have created panes since it started).
fn first_shell(socket: &Path) -> Result<()> {
    let mut session = Session::open(connection::connect(socket)?)?;
    if !session.has("pane-lifecycle") {
        return Ok(());
    }
    // The first update lists the panes that exist.
    if !session.update()?.panes.is_empty() {
        return Ok(());
    }
    session.call(Request::Spawn {
        request: "web-start".into(),
        cwd: None,
        argv: Vec::new(),
        shell: None,
    })?;
    Ok(())
}

/// Serves the browser while the workspace is not running: answers its hello
/// with a `Workspace` notice, its `state` and, on request, `start`. Returns
/// the connection to the started server.
fn stopped(socket: &Path, state: &Path, config: Option<&user_config::Live>) -> Result<UnixStream> {
    let workspace = state.file_name().unwrap_or_default().to_string_lossy();
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut greeted = false;
    let mut last_id = 0;
    loop {
        let value =
            read_message::<Value>(&mut input)?.ok_or("browser disconnected before start")?;
        let Some(web) = value.get("Web") else {
            match serde_json::from_value::<ClientMessage>(value) {
                Ok(ClientMessage::Stream(stream::Request::Hello { version: 1 })) if !greeted => {
                    greeted = true;
                    write_message(
                        &mut output,
                        &json!({"Workspace": {"running": false, "workspace": workspace}}),
                    )?;
                }
                // Other stream requests have no server to reach yet.
                Ok(ClientMessage::Stream(_)) if greeted => {}
                _ => return Err("unexpected message while the workspace is stopped".into()),
            }
            continue;
        };
        let id = web["id"]
            .as_u64()
            .filter(|id| *id > last_id && *id <= MAX_ID && greeted)
            .ok_or("invalid web request")?;
        last_id = id;
        let method = web["method"].as_str().unwrap_or("");
        let result = if serde_json::to_vec(web).is_ok_and(|b| b.len() > REQUEST_LIMIT) {
            Err("web request too large".into())
        } else {
            match method {
                "state" => Ok(json!({
                    "api_version": API_VERSION,
                    "api_minor": API_MINOR,
                    "workspace": workspace,
                    "version": env!("CARGO_PKG_VERSION"),
                    "running": false,
                    "operations": ["state", "start", "config"],
                })),
                "start" => start(socket, state, &web["params"]),
                "config" | "config_set" => config_request(config, method, &web["params"]),
                _ => Err("the workspace server is not running; start it first".into()),
            }
        };
        let started = method == "start" && result.is_ok();
        let reply = match result {
            Ok(result) => json!({"Web": {"id": id, "result": result}}),
            Err(error) => json!({"Web": {"id": id, "error": error.to_string()}}),
        };
        write_message(&mut output, &reply)?;
        if started {
            return connection::connect(socket);
        }
    }
}

/// Runs the bridge until standard input closes or the workspace disconnects.
/// The command line is fixed by the operator (an SSH forced command or the
/// relay's configured bridge command); the browser supplies no path.
pub fn run(socket: &Path, state: &Path) -> Result<()> {
    if unsafe { libc::isatty(0) } == 1 || unsafe { libc::isatty(1) } == 1 {
        return Err("web bridge requires SSH without a PTY".into());
    }
    let (sender, receiver) = mpsc::sync_channel::<Outbound>(2);
    // `Config` notices wait until this thread writes the relayed stream.
    let relaying = Arc::new(AtomicBool::new(false));
    let config = live_config(sender.clone(), relaying.clone());
    // A workspace that is not running is offered to the browser to start.
    let (mut upstream, greeted) = match connection::connect(socket) {
        Ok(stream) => (stream, false),
        Err(_) if server_absent(socket) => (stopped(socket, state, config.as_deref())?, true),
        Err(error) => return Err(error),
    };
    relaying.store(true, Ordering::Release);
    if greeted {
        // The browser's hello was answered while stopped; greet the server.
        write_message(
            &mut upstream,
            &ClientMessage::Stream(stream::Request::Hello { version: 1 }),
        )?;
    }
    let peer = Peer::read(&upstream)?;
    // The stream stays open while idle; only writes are bounded.
    upstream.set_read_timeout(None)?;
    upstream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let runtime = Arc::new(Mutex::new(Runtime::default()));
    let observations = runtime.clone();
    let reader = upstream.try_clone()?;
    let writer = Arc::new(Mutex::new(upstream));
    let events = sender.clone();
    let ready = Arc::new(AtomicBool::new(false));
    let handshake = ready.clone();
    std::thread::spawn(move || {
        let mut reader = reader;
        // An update announcing `binary_screens`, held until its record.
        let mut announced: Option<Value> = None;
        while let Ok(Some(record)) = read_server_record(&mut reader) {
            let event = match record {
                ServerRecord::Message(event) => event,
                // Binary screen records (`binary-screens`) reach the browser
                // byte for byte; its WASM core decodes them. The record and
                // the update before it go out as one unit, so no `Web` reply
                // from the other thread can come between them.
                ServerRecord::Screens(bytes) => {
                    let Some(update) = announced.take() else {
                        break;
                    };
                    if events.send(Outbound::Screens(update, bytes)).is_err() {
                        break;
                    }
                    continue;
                }
            };
            if announced.is_some() {
                // The server always sends the record right after its update.
                break;
            }
            let binary = matches!(
                &event,
                ServerMessage::Stream(stream::Event::Update(update)) if update.binary_screens
            );
            if let ServerMessage::Stream(stream::Event::Welcome {
                version,
                client,
                ref epoch,
                ref capabilities,
                ..
            }) = event
            {
                if version != stream::VERSION || client == 0 || epoch.is_empty() {
                    break;
                }
                if let Ok(mut runtime) = observations.lock() {
                    runtime.epoch = epoch.clone();
                    runtime.capabilities = capabilities.clone();
                }
                handshake.store(true, Ordering::Release);
            }
            let Ok(value) = serde_json::to_value(event) else {
                break;
            };
            if binary {
                announced = Some(value);
                continue;
            }
            if events.send(Outbound::Message(value)).is_err() {
                break;
            }
        }
        let _ = events.send(Outbound::Closed);
    });
    let socket = socket.to_owned();
    let state = state.to_owned();
    std::thread::spawn(move || {
        let mut monitor = status::Monitor::default();
        // Prime one bounded sample so opening the status view need not wait
        // for a full refresh interval. No sampler runs while it is closed.
        monitor.poll(true);
        let mut api = Api {
            sessions: Sessions {
                peer,
                runtime,
                socket,
            },
            environment_threads: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            state,
            monitor,
            config,
            git: web_git::Worker::start(),
            history: web_history::Worker::start(),
        };
        let mut initialized = greeted;
        let mut last_id = 0;
        let mut input = io::stdin().lock();
        while let Ok(Some(value)) = read_message::<Value>(&mut input) {
            if let Some(web) = value.get("Web") {
                if !initialized {
                    break;
                }
                let Some(id) = web["id"].as_u64().filter(|id| *id > 0 && *id <= MAX_ID) else {
                    break;
                };
                if id <= last_id || !ready.load(Ordering::Acquire) {
                    let reply = json!({"Web": {"id": id, "error": "duplicate request or server handshake incomplete"}});
                    if sender.send(Outbound::Message(reply)).is_err() {
                        break;
                    }
                    continue;
                }
                last_id = id;
                let method = web["method"].as_str().unwrap_or("");
                let result = if !serde_json::to_vec(web).is_ok_and(|b| b.len() <= REQUEST_LIMIT) {
                    Err("web request too large".into())
                } else if matches!(
                    method,
                    "environments" | "environment_start" | "environment_stop" | "environment_check"
                ) {
                    let replies = sender.clone();
                    match api.environment(
                        method,
                        &web["params"],
                        Box::new(move |result| {
                            let reply = match result {
                                Ok(result) => json!({"Web": {"id": id, "result": result}}),
                                Err(error) => json!({"Web": {"id": id, "error": error}}),
                            };
                            let _ = replies.send(Outbound::Message(reply));
                        }),
                    ) {
                        Ok(()) => continue,
                        Err(error) => Err(error),
                    }
                } else if method == "git" || method == "conversations" {
                    // Answered by a Git or the History worker thread when
                    // ready, so a slow repository or a large transcript
                    // folder never holds up the other operations.
                    let replies = sender.clone();
                    let submit = if method == "git" {
                        Api::git
                    } else {
                        Api::conversations
                    };
                    match submit(
                        &api,
                        &web["params"],
                        Box::new(move |result| {
                            let reply = match result {
                                Ok(result) => json!({"Web": {"id": id, "result": result}}),
                                Err(error) => json!({"Web": {"id": id, "error": error}}),
                            };
                            let _ = replies.send(Outbound::Message(reply));
                        }),
                    ) {
                        Ok(()) => continue,
                        Err(error) => Err(error),
                    }
                } else {
                    api.request(method, &web["params"])
                };
                let reply = match result {
                    Ok(result) => json!({"Web": {"id": id, "result": result}}),
                    Err(error) => json!({"Web": {"id": id, "error": error.to_string()}}),
                };
                if sender.send(Outbound::Message(reply)).is_err() {
                    break;
                }
                continue;
            }
            let Ok(ClientMessage::Stream(request)) = serde_json::from_value(value) else {
                break;
            };
            let hello = matches!(request, stream::Request::Hello { .. });
            if !initialized {
                if !matches!(request, stream::Request::Hello { version: 1 }) {
                    break;
                }
                initialized = true;
            } else if hello {
                break;
            }
            // Every stream request is forwarded unchanged, notification
            // acknowledgements included: read flags belong to the workspace,
            // and the TUI marks a terminal's notifications read when it is
            // shown, as the browser does. Upload and file requests too:
            // like typing into a pane, they read and write only what the
            // bridge's own user can, and the server enforces their limits,
            // destination rules and flow-control window on this one
            // relayed connection.
            let sent = writer
                .lock()
                .map_err(|_| "bridge writer unavailable".into())
                .and_then(|mut socket| {
                    write_message(&mut *socket, &ClientMessage::Stream(request))
                });
            if sent.is_err() {
                break;
            }
        }
        let _ = writer
            .lock()
            .map(|socket| socket.shutdown(std::net::Shutdown::Both));
    });
    let mut output = io::stdout().lock();
    for outbound in receiver {
        match outbound {
            Outbound::Message(value) => write_message(&mut output, &value)?,
            Outbound::Screens(update, bytes) => {
                write_message(&mut output, &update)?;
                write_record(&mut output, &bytes)?;
            }
            Outbound::Closed => {
                return Err("workspace disconnected; reconnect for a fresh epoch".into());
            }
        }
    }
    Ok(())
}

/// What the bridge writes to the browser, in order.
enum Outbound {
    /// A JSON record: a server event re-serialized, or a `Web` reply.
    Message(Value),
    /// An update with `binary_screens` and the binary screen record after it,
    /// exactly as the server framed its payload, written back to back.
    Screens(Value, Vec<u8>),
    /// The workspace connection ended.
    Closed,
}

/// Writes one framed payload unchanged (four-byte big-endian length).
fn write_record(writer: &mut impl io::Write, payload: &[u8]) -> Result<()> {
    if payload.is_empty() || payload.len() > MAX_MESSAGE {
        return Err("binary screen record has an invalid length".into());
    }
    writer.write_all(&(payload.len() as u32).to_be_bytes())?;
    writer.write_all(payload)?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Kind;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }
    #[test]
    fn explicit_state_and_workspace_precedence_do_not_depend_on_order() {
        let one = paths(&args(&[
            "--state-dir",
            "/private/state",
            "--workspace",
            "dev",
            "--socket",
            "/private/socket/control.sock",
        ]))
        .unwrap();
        let two = paths(&args(&[
            "--socket",
            "/private/socket/control.sock",
            "--workspace",
            "dev",
            "--state-dir",
            "/private/state",
        ]))
        .unwrap();
        assert_eq!(one, two);
        assert_eq!(one.1, Path::new("/private/state"));
        assert_eq!(one.0, Path::new("/private/socket/control.sock"));
        assert_eq!(
            paths(&args(&["--socket", "/private/custom/control.sock"]))
                .unwrap()
                .1,
            Path::new("/private/custom")
        );
        let (socket, state) = paths(&args(&["--workspace", "dev"])).unwrap();
        assert_eq!(
            state,
            super::super::lifecycle::workspace_path("dev").unwrap()
        );
        assert_eq!(socket, state.join("control.sock"));
    }
    #[test]
    fn rejects_unknown_incomplete_and_invalid_workspace_options() {
        for values in [
            &["--workspace"][..],
            &["--workspace", "../other"],
            &["--unknown", "value"],
        ] {
            assert!(paths(&args(values)).is_err());
        }
    }
    fn record(kind: Option<Kind>, session: Option<&str>, argv: &[&str]) -> ClosedSession {
        super::super::history::Entry {
            key: "k".into(),
            workspace: "main".into(),
            pane: 1,
            cwd: "/tmp".into(),
            name: "n".into(),
            kind,
            session_id: session.map(Into::into),
            sessions: vec![],
            argv: argv.iter().map(|a| (*a).into()).collect(),
            closed_at: Some(1),
            reason: "Closed".into(),
        }
        .into()
    }
    #[test]
    fn restore_choices_follow_the_tui() {
        // An agent resumes, even with an unknown conversation (the agent's
        // own picker); a shell reruns its last program or opens its folder.
        assert_eq!(
            choices(&record(Some(Kind::Codex), Some("abc-123"), &[])),
            ["resume"]
        );
        assert_eq!(choices(&record(Some(Kind::Claude), None, &[])), ["resume"]);
        let shell = record(None, None, &["htop", "-d", "5"]);
        assert_eq!(choices(&shell), ["restart", "folder"]);
        assert_eq!(choices(&record(None, None, &[])), ["folder"]);
    }
    #[test]
    fn resume_commands_come_from_a_fixed_table() {
        let (kind, id, argv) = resume_argv(&json!({"kind": "claude", "id": "a-1"})).unwrap();
        assert_eq!((kind, id.as_deref()), ("claude", Some("a-1")));
        assert_eq!(argv, ["claude", "--resume", "a-1"]);
        // Codex takes the rollout's session id (a UUID).
        let uuid = "01a21031-eea6-73f1-816a-e23ce5e1b0d2";
        let (_, _, argv) = resume_argv(&json!({"kind": "codex", "id": uuid})).unwrap();
        assert_eq!(argv, ["codex", "resume", uuid]);
        // The picker only when asked for, without an id.
        let (kind, id, argv) = resume_argv(&json!({"kind": "codex", "picker": true})).unwrap();
        assert_eq!((kind, id), ("codex", None));
        assert_eq!(argv, ["codex", "resume"]);
        let (_, _, argv) =
            resume_argv(&json!({"kind": "claude", "id": null, "picker": true})).unwrap();
        assert_eq!(argv, ["claude", "--resume"]);
        for bad in [
            json!({"kind": "claude", "id": "x; rm -rf ~"}),
            json!({"kind": "claude", "id": ""}),
            json!({"kind": "claude", "id": "", "picker": true}),
            json!({"kind": "claude", "id": "x y", "picker": true}),
            json!({"kind": "bash", "id": "a"}),
            json!({"kind": "bash", "picker": true}),
            json!({"kind": "codex"}),
            json!({"kind": "codex", "picker": "yes"}),
            json!("claude --resume a"),
        ] {
            assert!(resume_argv(&bad).is_err(), "{bad}");
        }
    }
}
