//! Terminal-only SSH adapter for the browser client. OpenSSH runs it on the
//! authenticated Linux account; it relays the independent pane stream and a
//! small set of request-correlated terminal operations over standard I/O.
//!
//! It never listens on TCP, never starts or restarts a workspace server, never
//! changes SSH authorization and holds no project, file or Git management.
use super::{connection, protocol::*, status, stream};
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
pub const API_VERSION: u64 = 1;
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

struct Api {
    peer: Peer,
    runtime: Arc<Mutex<Runtime>>,
    socket: PathBuf,
    state: PathBuf,
    monitor: status::Monitor,
}

fn pane(value: &Value) -> Result<u64> {
    value["pane"]
        .as_u64()
        .filter(|p| *p > 0)
        .ok_or_else(|| "pane required".into())
}

impl Api {
    fn exchange(&self, message: ClientMessage) -> Result<ServerMessage> {
        let mut socket = connection::connect(&self.socket)?;
        if Peer::read(&socket)? != self.peer {
            return Err("terminal server changed; reconnect before continuing".into());
        }
        socket.set_read_timeout(Some(Duration::from_secs(5)))?;
        socket.set_write_timeout(Some(Duration::from_secs(5)))?;
        write_message(&mut socket, &message)?;
        match read_message(&mut socket)?.ok_or("workspace disconnected")? {
            ServerMessage::Error(error) => Err(error.into()),
            result => Ok(result),
        }
    }
    fn command(&self, action: Action) -> Result<ServerMessage> {
        self.exchange(ClientMessage::Command(action))
    }
    fn capabilities(&self) -> Result<Vec<String>> {
        let runtime = self.runtime.lock().map_err(|_| "runtime unavailable")?;
        if runtime.epoch.is_empty() {
            return Err("terminal handshake is incomplete".into());
        }
        Ok(runtime.capabilities.clone())
    }
    fn request(&mut self, method: &str, p: &Value) -> Result<Value> {
        match method {
            "state" => Ok(json!({
                "api_version": API_VERSION,
                "workspace": self.state.file_name().unwrap_or_default().to_string_lossy(),
                "version": env!("CARGO_PKG_VERSION"),
                "server_capabilities": self.capabilities()?,
                "operations": ["state", "new", "close", "search", "history", "status"],
            })),
            "new" => self.spawn(p),
            "close" => {
                let id = pane(p)?;
                if p["confirmed"] != true {
                    return Err("closing a pane requires explicit confirmation".into());
                }
                // This explicitly confirmed same-user action may close a pane
                // another client is using: return its lease to the base
                // geometry first, then close it through the server's own
                // confirmation step.
                self.command(Action::ClaimPane(id))?;
                self.command(Action::ClosePaneRequest(id))?;
                Ok(serde_json::to_value(
                    self.command(Action::ClosePaneConfirm(id))?,
                )?)
            }
            "search" => {
                let query = p["query"].as_str().ok_or("search query required")?;
                if query.is_empty() || query.len() > 256 || query.chars().any(char::is_control) {
                    return Err("search must be 1–256 bytes without control characters".into());
                }
                Ok(serde_json::to_value(self.exchange(
                    ClientMessage::SearchPane {
                        pane: pane(p)?,
                        query: query.to_owned(),
                        limit: 64,
                    },
                )?)?)
            }
            "history" => {
                let start = p["start"].as_i64().unwrap_or(-200).clamp(-100_000, 0) as i32;
                let limit = p["limit"].as_u64().unwrap_or(200).min(200) as usize;
                Ok(serde_json::to_value(self.exchange(
                    ClientMessage::History {
                        pane: pane(p)?,
                        start,
                        limit,
                    },
                )?)?)
            }
            "status" => {
                let mut value = serde_json::to_value(self.monitor.poll(true))?;
                value["refreshing"] = json!(self.monitor.refreshing());
                Ok(value)
            }
            _ => Err("unsupported web operation".into()),
        }
    }
    /// Starts a shell in a new pane through the server's structured
    /// `SpawnPane` request. The browser supplies no command line; only an
    /// optional absolute working directory.
    fn spawn(&self, p: &Value) -> Result<Value> {
        if !self.capabilities()?.iter().any(|c| c == "pane-spawn") {
            return Err("this terminal server cannot create panes for web clients".into());
        }
        let request = p["request"]
            .as_str()
            .ok_or("new pane request identity required")?;
        let cwd = match p.get("cwd") {
            None | Some(Value::Null) => home()?,
            Some(Value::String(path)) => PathBuf::from(path),
            _ => return Err("cwd must be an absolute directory".into()),
        };
        if !cwd.is_absolute() || !cwd.is_dir() {
            return Err("cwd must be an existing absolute directory".into());
        }
        let ServerMessage::SpawnedPane {
            request: echoed,
            pane,
        } = self.exchange(ClientMessage::SpawnPane {
            request: request.to_owned(),
            cwd,
            argv: vec![],
        })?
        else {
            return Err("terminal server did not identify the created pane".into());
        };
        if echoed != request {
            return Err("pane creation identity mismatch".into());
        }
        Ok(json!({ "pane": pane }))
    }
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "HOME is not an absolute directory".into())
}

/// Stream requests a browser may forward. Hello and clipboard opt-in are
/// connection setup; notification acknowledgements stay with the TUI, whose
/// read flags they change.
fn permitted(request: &stream::Request) -> bool {
    !matches!(request, stream::Request::Acknowledge { .. })
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

/// Runs the bridge until standard input closes or the workspace disconnects.
/// The command line is fixed by the operator (an SSH forced command or the
/// relay's configured bridge command); the browser supplies no path.
pub fn run(socket: &Path, state: &Path) -> Result<()> {
    if unsafe { libc::isatty(0) } == 1 || unsafe { libc::isatty(1) } == 1 {
        return Err("web bridge requires SSH without a PTY".into());
    }
    let upstream = connection::connect(socket)?;
    let peer = Peer::read(&upstream)?;
    // The stream stays open while idle; only writes are bounded.
    upstream.set_read_timeout(None)?;
    upstream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let runtime = Arc::new(Mutex::new(Runtime::default()));
    let observations = runtime.clone();
    let reader = upstream.try_clone()?;
    let writer = Arc::new(Mutex::new(upstream));
    let (sender, receiver) = mpsc::sync_channel::<Value>(2);
    let events = sender.clone();
    let ready = Arc::new(AtomicBool::new(false));
    let handshake = ready.clone();
    std::thread::spawn(move || {
        let mut reader = reader;
        while let Ok(Some(event)) = read_message::<ServerMessage>(&mut reader) {
            if let ServerMessage::Stream(stream::Event::Welcome {
                version,
                client,
                ref epoch,
                ref capabilities,
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
            if events.send(value).is_err() {
                break;
            }
        }
        let _ = events.send(json!({"__workspace_closed": true}));
    });
    let socket = socket.to_owned();
    let state = state.to_owned();
    std::thread::spawn(move || {
        let mut monitor = status::Monitor::default();
        // Prime one bounded sample so opening the status view need not wait
        // for a full refresh interval. No sampler runs while it is closed.
        monitor.poll(true);
        let mut api = Api {
            peer,
            runtime,
            socket,
            state,
            monitor,
        };
        let mut initialized = false;
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
                    if sender.send(reply).is_err() {
                        break;
                    }
                    continue;
                }
                last_id = id;
                let result = if serde_json::to_vec(web).is_ok_and(|b| b.len() <= REQUEST_LIMIT) {
                    api.request(web["method"].as_str().unwrap_or(""), &web["params"])
                } else {
                    Err("web request too large".into())
                };
                let reply = match result {
                    Ok(result) => json!({"Web": {"id": id, "result": result}}),
                    Err(error) => json!({"Web": {"id": id, "error": error.to_string()}}),
                };
                if sender.send(reply).is_err() {
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
            if !permitted(&request) {
                let refused = json!({"Stream": {"type": "error", "message": "this operation is not available to web clients"}});
                if sender.send(refused).is_err() {
                    break;
                }
                continue;
            }
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
    for value in receiver {
        if value.get("__workspace_closed").is_some() {
            return Err("workspace disconnected; reconnect for a fresh epoch".into());
        }
        write_message(&mut output, &value)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
    #[test]
    fn notification_acknowledgements_stay_with_the_tui() {
        assert!(!permitted(&stream::Request::Acknowledge {
            notification: 1
        }));
        assert!(permitted(&stream::Request::Claim {
            pane: 1,
            takeover: true
        }));
        assert!(permitted(&stream::Request::Input {
            pane: 1,
            bytes: vec![b'a']
        }));
    }
}
