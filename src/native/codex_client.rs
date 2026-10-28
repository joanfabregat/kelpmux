//! Per-terminal Codex transport metadata. Never stores RPC bodies or prompts.
use crate::{Result, journal, process};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command},
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket, protocol::WebSocketConfig};

#[derive(Debug, Serialize, Deserialize)]
pub struct LinkRecord {
    pub pid: u32,
    pub birth: u64,
    pub thread: Option<String>,
}
/// Validate against the actual terminal client (or its native child), not the daemon.
pub fn identity(proc_root: &Path, pid: u32) -> Option<Option<String>> {
    let path = PathBuf::from(process::environment(proc_root, pid, "KELPMUX_CODEX_LINK")?);
    if !path.is_absolute() {
        return None;
    }
    let parent = fs::symlink_metadata(path.parent()?).ok()?;
    if !parent.is_dir() || parent.uid() != unsafe { libc::geteuid() } || parent.mode() & 0o077 != 0
    {
        return None;
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file()
        || meta.uid() != parent.uid()
        || meta.mode() & 0o077 != 0
        || meta.len() > 4096
    {
        return None;
    }
    let record: LinkRecord = serde_json::from_reader(file.take(4097)).ok()?;
    let mut current = pid;
    for _ in 0..8 {
        let proc = process::read(proc_root, current).ok()?;
        if proc.pid == record.pid {
            return (proc.start == record.birth)
                .then_some(record.thread.filter(|id| crate::session_id(id)));
        }
        current = proc.parent;
    }
    None
}

#[derive(Default)]
pub struct Tracker {
    pending: BTreeMap<String, u64>,
    sequence: u64,
    selected: u64,
    pub thread: Option<String>,
}
impl Tracker {
    /// Only client-originated selection operations identify the displayed thread.
    pub fn client(&mut self, value: &Value) -> bool {
        match value["method"].as_str().unwrap_or("") {
            "thread/start" | "thread/resume" | "thread/fork" => {
                self.sequence += 1;
                self.selected = self.sequence;
                self.thread = None;
                if self.pending.len() >= 64 {
                    self.pending.clear();
                }
                if value["id"].is_string() || value["id"].is_number() {
                    self.pending.insert(value["id"].to_string(), self.sequence);
                }
                true
            }
            "turn/start" | "turn/steer" => {
                if let Some(id) = value["params"]["threadId"]
                    .as_str()
                    .filter(|id| crate::session_id(id))
                {
                    self.sequence += 1;
                    self.selected = self.sequence;
                    let changed = self.thread.as_deref() != Some(id);
                    self.thread = Some(id.into());
                    changed
                } else {
                    false
                }
            }
            "thread/unsubscribe"
                if value["params"]["threadId"].as_str() == self.thread.as_deref()
                    && self.thread.is_some() =>
            {
                self.sequence += 1;
                self.selected = self.sequence;
                self.thread = None;
                true
            }
            _ => false,
        }
    }
    pub fn server(&mut self, value: &Value) -> bool {
        let Some(sequence) = self.pending.remove(&value["id"].to_string()) else {
            return false;
        };
        if sequence != self.selected {
            return false;
        }
        let thread = value["result"]["thread"]["id"]
            .as_str()
            .filter(|id| crate::session_id(id));
        self.thread = thread.map(str::to_owned);
        true
    }
}

fn config() -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(16 * 1024)
        .write_buffer_size(0)
        .max_write_buffer_size(64 * 1024 * 1024)
        .max_message_size(Some(32 * 1024 * 1024))
        .max_frame_size(Some(32 * 1024 * 1024))
}
fn blocked(error: &tungstenite::Error) -> bool {
    matches!(error, tungstenite::Error::Io(e) if e.kind() == std::io::ErrorKind::WouldBlock)
}
fn send(socket: &mut WebSocket<UnixStream>, message: Message) -> Result<()> {
    match socket.send(message) {
        Ok(()) => Ok(()),
        Err(e) if blocked(&e) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
fn flush(socket: &mut WebSocket<UnixStream>) -> Result<bool> {
    match socket.flush() {
        Ok(()) => Ok(false),
        Err(e) if blocked(&e) => Ok(true),
        Err(e) => Err(e.into()),
    }
}
/// Forward existing protocol operations unchanged. Observe only selection metadata.
pub fn relay(
    client: UnixStream,
    upstream: &Path,
    record: &Path,
    pid: u32,
    birth: u64,
    child: &mut Child,
) -> Result<()> {
    let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            client.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut peer as *mut libc::ucred).cast(),
            &mut size,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut current = peer.pid as u32;
    let mut verified = false;
    for _ in 0..8 {
        let process = process::read(Path::new("/proc"), current)?;
        if process.pid == pid {
            verified = process.start == birth;
            break;
        }
        current = process.parent;
    }
    if !verified {
        return Err("Codex bridge connection does not belong to its terminal client".into());
    }
    client.set_read_timeout(Some(Duration::from_secs(2)))?;
    client.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut client = tungstenite::accept_with_config(client, Some(config()))?;
    let stream = UnixStream::connect(upstream)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let (mut daemon, _) =
        tungstenite::client::client_with_config("ws://localhost/", stream, Some(config()))?;
    client.get_ref().set_nonblocking(true)?;
    daemon.get_ref().set_nonblocking(true)?;
    let mut tracker = Tracker::default();
    loop {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        let mut backlog = false;
        for from_client in [true, false] {
            for index in 0..64 {
                let message = match if from_client {
                    client.read()
                } else {
                    daemon.read()
                } {
                    Ok(message) => message,
                    Err(e) if blocked(&e) => break,
                    Err(e) => return Err(e.into()),
                };
                backlog |= index == 63;
                if let Message::Text(text) = &message {
                    if let Ok(value) = serde_json::from_str::<Value>(text) {
                        let changed = if from_client {
                            tracker.client(&value)
                        } else {
                            tracker.server(&value)
                        };
                        if changed {
                            journal::atomic(
                                record,
                                &LinkRecord {
                                    pid,
                                    birth,
                                    thread: tracker.thread.clone(),
                                },
                            )?;
                        }
                    }
                }
                // Ping/pong is hop-local; tungstenite queues the corresponding pong.
                if matches!(message, Message::Ping(_) | Message::Pong(_)) {
                    continue;
                }
                let closed = message.is_close();
                send(
                    if from_client {
                        &mut daemon
                    } else {
                        &mut client
                    },
                    message,
                )?;
                if closed {
                    return Ok(());
                }
            }
        }
        let mut fds = [
            (client.get_ref().as_raw_fd(), flush(&mut client)?),
            (daemon.get_ref().as_raw_fd(), flush(&mut daemon)?),
        ]
        .map(|(fd, pending)| libc::pollfd {
            fd,
            events: libc::POLLIN | if pending { libc::POLLOUT } else { 0 },
            revents: 0,
        });
        // Sleep until socket activity. Buffered messages never wait for more bytes.
        unsafe {
            libc::poll(
                fds.as_mut_ptr(),
                fds.len() as libc::nfds_t,
                if backlog { 0 } else { 1000 },
            );
        }
    }
}

fn passthrough(args: &[String]) -> bool {
    let mut skip = false;
    let mut positional = None;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if matches!(
            arg.as_str(),
            "--remote" | "--no-daemon" | "-h" | "--help" | "-V" | "--version"
        ) || arg.starts_with("--remote=")
        {
            return true;
        }
        if matches!(
            arg.as_str(),
            "-c" | "--config"
                | "-m"
                | "--model"
                | "-p"
                | "--profile"
                | "-C"
                | "--cd"
                | "-a"
                | "--ask-for-approval"
                | "-s"
                | "--sandbox"
                | "--enable"
                | "--disable"
                | "-i"
                | "--image"
                | "--add-dir"
                | "--local-provider"
        ) {
            skip = true;
            continue;
        }
        if arg == "--" {
            break;
        }
        if !arg.starts_with('-') && positional.is_none() {
            positional = Some(arg.as_str());
        }
    }
    positional.is_some_and(|arg| {
        matches!(
            arg,
            "exec"
                | "review"
                | "login"
                | "logout"
                | "mcp"
                | "plugin"
                | "app-server"
                | "remote-control"
                | "completion"
                | "update"
                | "doctor"
                | "sandbox"
                | "debug"
                | "apply"
                | "queue"
                | "archive"
                | "delete"
                | "migrate-rollouts"
                | "migrate-nfs-db"
                | "unarchive"
                | "cloud"
                | "exec-server"
                | "features"
                | "help"
        )
    })
}
struct Directory(PathBuf);
struct ClientChild(Child);
impl Drop for ClientChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0.join("client.sock"));
        let _ = fs::remove_file(self.0.join("link.json"));
        let _ = fs::remove_dir(&self.0);
    }
}
pub fn run(args: &[String]) -> Result<()> {
    if args.len() < 3 || args[0] != "--codex" || args[2] != "--" {
        return Err("codex-client requires --codex EXECUTABLE -- [CODEX_ARGUMENTS]".into());
    }
    let executable = Path::new(&args[1]);
    let args = &args[3..];
    if passthrough(args) || std::env::var_os("KELPMUX_PANE").is_none() {
        return Err(Command::new(executable).args(args).exec().into());
    }
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".codex")
        });
    let upstream = home.join("app-server-control/app-server-control.sock");
    if !upstream.exists()
        && !Command::new(executable)
            .args(["app-server", "daemon", "start"])
            .status()?
            .success()
    {
        return Err("could not start Codex's shared daemon".into());
    }
    let dir = std::env::temp_dir().join(format!(
        "kelpmux-codex-{}-{}",
        std::process::id(),
        crate::now()
    ));
    fs::DirBuilder::new().mode(0o700).create(&dir)?;
    let dir = Directory(dir);
    let endpoint = dir.0.join("client.sock");
    let record = dir.0.join("link.json");
    let listener = UnixListener::bind(&endpoint)?;
    fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let mut command = Command::new(executable);
    command
        .arg("--remote")
        .arg(format!("unix://{}", endpoint.display()))
        .args(args)
        .env("KELPMUX_CODEX_LINK", &record);
    unsafe {
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = ClientChild(command.spawn()?);
    // Ctrl-C belongs to the foreground TUI; don't destroy its transport bridge.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGQUIT, libc::SIG_IGN);
    }
    let birth = process::read(Path::new("/proc"), child.0.id())?.start;
    journal::atomic(
        &record,
        &LinkRecord {
            pid: child.0.id(),
            birth,
            thread: None,
        },
    )?;
    let mut waiting = Instant::now();
    let result = loop {
        if let Some(status) = child.0.try_wait()? {
            break status;
        }
        match listener.accept() {
            Ok((client, _)) => {
                let _ = relay(
                    client,
                    &upstream,
                    &record,
                    child.0.id(),
                    birth,
                    &mut child.0,
                );
                journal::atomic(
                    &record,
                    &LinkRecord {
                        pid: child.0.id(),
                        birth,
                        thread: None,
                    },
                )?;
                waiting = Instant::now();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if waiting.elapsed() > Duration::from_secs(30) {
                    let _ = child.0.kill();
                    break child.0.wait()?;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    };
    drop(child);
    drop(dir);
    use std::os::unix::process::ExitStatusExt;
    std::process::exit(
        result
            .code()
            .unwrap_or_else(|| 128 + result.signal().unwrap_or(1)),
    );
}

/// Workspace-local startup files preserve the user's configuration and install
/// an interactive function. No global shell or Codex configuration is modified.
pub fn integrate_shell(command: &mut Command, state: &Path, shell: &Path) -> Result<()> {
    let name = shell.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if !matches!(name, "bash" | "zsh") {
        return Ok(());
    }
    let dir = state.join("shell-integration");
    journal::private_dir(&dir)?;
    command
        .env("KELPMUX_EXECUTABLE", std::env::current_exe()?)
        .env_remove("KELPMUX_CODEX_LINK");
    let write = |name: &str, text: &str| -> Result<()> {
        use std::io::Write;
        let temp = dir.join(format!("{name}-{}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)?;
        file.write_all(text.as_bytes())?;
        fs::rename(temp, dir.join(name))?;
        Ok(())
    };
    if name == "bash" {
        write(
            "bashrc",
            r#"if [ -r "$HOME/.bashrc" ]; then . "$HOME/.bashrc"; fi
codex() { "$KELPMUX_EXECUTABLE" codex-client --codex "$(type -P codex)" -- "$@"; }
"#,
        )?;
        command.arg("--rcfile").arg(dir.join("bashrc"));
    } else {
        let original = std::env::var_os("ZDOTDIR").map(PathBuf::from);
        command
            .env(
                "KELPMUX_USER_ZDOTDIR",
                original.as_deref().unwrap_or_else(|| Path::new("")),
            )
            .env("KELPMUX_INTEGRATION_DIR", &dir)
            .env("ZDOTDIR", &dir);
        write(
            ".zshenv",
            r#"if [[ -n $KELPMUX_USER_ZDOTDIR ]]; then export ZDOTDIR=$KELPMUX_USER_ZDOTDIR; else unset ZDOTDIR; fi
if [[ -r ${ZDOTDIR:-$HOME}/.zshenv ]]; then source "${ZDOTDIR:-$HOME}/.zshenv"; fi
export KELPMUX_USER_ZDOTDIR=${ZDOTDIR-}
export ZDOTDIR=$KELPMUX_INTEGRATION_DIR
"#,
        )?;
        write(
            ".zshrc",
            r#"if [[ -n $KELPMUX_USER_ZDOTDIR ]]; then export ZDOTDIR=$KELPMUX_USER_ZDOTDIR; else unset ZDOTDIR; fi
if [[ -r ${ZDOTDIR:-$HOME}/.zshrc ]]; then source "${ZDOTDIR:-$HOME}/.zshrc"; fi
function codex() { "$KELPMUX_EXECUTABLE" codex-client --codex "$(whence -p codex)" -- "$@"; }
"#,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn selection_replies_follow_client_order_and_ignore_other_threads() {
        let mut tracker = Tracker::default();
        assert!(tracker.client(&json!({"id":1,"method":"thread/start"})));
        assert!(tracker.client(&json!({"id":2,"method":"thread/resume"})));
        assert!(!tracker.server(&json!({"id":1,"result":{"thread":{"id":"wrong-thread"}}})));
        assert!(tracker.server(&json!({"id":2,"result":{"thread":{"id":"selected-thread"}}})));
        assert_eq!(tracker.thread.as_deref(), Some("selected-thread"));
        assert!(
            !tracker
                .server(&json!({"method":"thread/started","params":{"thread":{"id":"subagent"}}}))
        );
        assert!(!tracker.client(
            &json!({"id":3,"method":"thread/read","params":{"threadId":"picker-preview"}})
        ));
        assert_eq!(tracker.thread.as_deref(), Some("selected-thread"));
        assert!(
            tracker.client(&json!({"method":"turn/start","params":{"threadId":"switched-thread"}}))
        );
        assert_eq!(tracker.thread.as_deref(), Some("switched-thread"));
        assert!(tracker.client(
            &json!({"method":"thread/unsubscribe","params":{"threadId":"switched-thread"}})
        ));
        assert!(tracker.thread.is_none());
    }
    #[test]
    fn cli_tools_help_explicit_remote_and_no_daemon_bypass_the_bridge() {
        for args in [
            vec!["exec", "task"],
            vec!["-c", "model=exec", "--help"],
            vec!["--remote", "unix://"],
            vec!["resume", "--no-daemon"],
            vec!["app-server", "daemon", "start"],
        ] {
            assert!(passthrough(
                &args.into_iter().map(str::to_owned).collect::<Vec<_>>()
            ));
        }
        for args in [
            vec![],
            vec!["resume", "some-id"],
            vec!["fork"],
            vec!["-c", "model=exec", "prompt"],
            vec!["agents"],
        ] {
            assert!(!passthrough(
                &args.into_iter().map(str::to_owned).collect::<Vec<_>>()
            ));
        }
    }
}
