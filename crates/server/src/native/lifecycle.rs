//! Explicit named workspaces and detached process ownership, without a service unit.
use super::{connection::Session, protocol::*};
use crate::{Result, journal};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub fn private_state(state: &Path) -> Result<()> {
    if !state.is_absolute() || state == Path::new("/") || state == Path::new("/tmp") {
        return Err("workspace state requires a dedicated absolute directory".into());
    }
    for path in state.ancestors() {
        if let Ok(meta) = fs::symlink_metadata(path) {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err("workspace state contains a symlink or non-directory".into());
            }
        }
    }
    if let Some(parent) = state.parent() {
        if parent.file_name().is_some_and(|name| name == "workspaces") {
            if let Some(root) = parent
                .parent()
                .filter(|root| root.file_name().is_some_and(|name| name == "kelpmux"))
            {
                journal::private_dir(root)?;
                journal::private_dir(parent)?;
            }
        }
    }
    journal::private_dir(state)
}

/// The server's part of `workspace.json`: the shell and launch directory of
/// panes created without their own. Clients may keep their own keys in the
/// same file; the server ignores them and `start` preserves them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub shell: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    /// The upload root (`--upload-dir`).
    pub upload_dir: Option<PathBuf>,
    /// Days completed uploads are kept (`--upload-retention-days`, 0 keeps them).
    pub upload_retention_days: Option<u32>,
    /// The folder file operations are confined to (`--files-root`; the
    /// server user's home directory when absent).
    pub files_root: Option<PathBuf>,
    /// Further folders outside the file root that file operations may
    /// reach (`--files-extra-root`, repeatable).
    pub files_extra_roots: Option<Vec<PathBuf>>,
}
const MAX_SETTINGS: u64 = 64 * 1024;
fn settings_bytes(path: &Path) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.len() > MAX_SETTINGS {
        return Err("configuration must be a regular file smaller than 64 KiB".into());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_SETTINGS + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SETTINGS {
        return Err("configuration must be a regular file smaller than 64 KiB".into());
    }
    Ok(bytes)
}
impl Settings {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(serde_json::from_slice(&settings_bytes(path)?)?)
    }
}
/// Record the shell and launch directory in `workspace.json`, keeping every
/// other key (a client's own settings) unchanged.
fn persist_settings(path: &Path, shell: &Path, cwd: &Path) -> Result<()> {
    persist_keys(
        path,
        [
            ("shell", serde_json::to_value(shell)?),
            ("cwd", serde_json::to_value(cwd)?),
        ],
    )
}
/// Record the upload settings of `kelpserver start --upload-dir` or
/// `--upload-retention-days` in `workspace.json`, keeping every other key.
/// They take effect when the server next starts.
pub fn persist_upload_settings(path: &Path, uploads: &super::uploads::Settings) -> Result<()> {
    let mut keys = Vec::new();
    if let Some(dir) = &uploads.dir {
        keys.push(("upload_dir", serde_json::to_value(dir)?));
    }
    if let Some(days) = uploads.retention_days {
        keys.push(("upload_retention_days", serde_json::to_value(days)?));
    }
    if keys.is_empty() {
        return Ok(());
    }
    persist_keys(path, keys)
}
/// Record the file root of `kelpserver start --files-root` in
/// `workspace.json`, keeping every other key. It takes effect when the
/// server next starts.
pub fn persist_files_root(path: &Path, root: &Path) -> Result<()> {
    persist_keys(path, [("files_root", serde_json::to_value(root)?)])
}
/// Record the extra file roots of `kelpserver start --files-extra-root` in
/// `workspace.json`, replacing the previous list and keeping every other
/// key. They take effect when the server next starts.
pub fn persist_files_extra_roots(path: &Path, roots: &[PathBuf]) -> Result<()> {
    persist_keys(path, [("files_extra_roots", serde_json::to_value(roots)?)])
}
fn persist_keys<'a>(
    path: &Path,
    keys: impl IntoIterator<Item = (&'a str, serde_json::Value)>,
) -> Result<()> {
    let mut object = match settings_bytes(path) {
        Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes)? {
            serde_json::Value::Object(object) => object,
            _ => return Err("workspace.json must contain a JSON object".into()),
        },
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            serde_json::Map::new()
        }
        Err(error) => return Err(error),
    };
    for (key, value) in keys {
        object.insert(key.into(), value);
    }
    journal::atomic(path, &serde_json::Value::Object(object))
}
pub fn workspace_path(name: &str) -> Result<PathBuf> {
    if name.is_empty()
        || name.len() > 48
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(
            "workspace name must contain 1–48 letters, numbers, hyphens or underscores".into(),
        );
    }
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?);
    Ok(std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".local/state"))
        .join("kelpmux/workspaces")
        .join(name))
}
/// Called before any threads are created. The original process returns false;
/// its session-independent grandchild continues with stdio redirected.
pub fn detach(state: &Path) -> Result<bool> {
    private_state(state)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(state.join("server.log"))?;
    if !log.metadata()?.is_file() || log.metadata()?.uid() != unsafe { libc::geteuid() } {
        return Err("server log must be an owned regular file".into());
    }
    let null = OpenOptions::new().read(true).open("/dev/null")?;
    let first = unsafe { libc::fork() };
    if first < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if first > 0 {
        return Ok(false);
    }
    // No allocator or Rust destructors run in the intermediate child.
    if unsafe { libc::setsid() } < 0 {
        unsafe { libc::_exit(1) };
    }
    let second = unsafe { libc::fork() };
    if second < 0 {
        unsafe { libc::_exit(1) };
    }
    if second > 0 {
        unsafe { libc::_exit(0) };
    }
    for (from, to) in [
        (null.as_raw_fd(), 0),
        (log.as_raw_fd(), 1),
        (log.as_raw_fd(), 2),
    ] {
        if unsafe { libc::dup2(from, to) } < 0 {
            unsafe { libc::_exit(1) };
        }
    }
    Ok(true)
}
pub struct StartRequest<'a> {
    pub socket: &'a Path,
    pub state: &'a Path,
    pub shell: &'a Path,
    pub cwd: &'a Path,
    /// The base pane size.
    pub cols: u16,
    pub rows: u16,
    pub replace_running: bool,
    pub executable: Option<&'a Path>,
}
/// Start the workspace server in the background unless it is already running.
/// With `replace_running`, a running server of another executable is stopped
/// first (its panes end). The new server starts without panes.
pub fn start(request: StartRequest<'_>) -> Result<()> {
    private_state(request.state)?;
    // Serialize the entire readiness/configuration transaction. Locking only
    // the eventual server endpoint is too late: competing starters could
    // overwrite workspace.json while the winning server reads it.
    let startup_lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(request.state.join("startup.lock"))?;
    let meta = startup_lock.metadata()?;
    if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err("workspace startup lock must be an owned private regular file".into());
    }
    let lock_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if unsafe { libc::flock(startup_lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::WouldBlock {
            return Err(error.into());
        }
        if !request.replace_running || Instant::now() >= lock_deadline {
            return Err("workspace startup is already in progress; retry when ready".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    let executable = request
        .executable
        .map(Path::to_path_buf)
        .unwrap_or(std::env::current_exe()?);
    let target = fs::metadata(&executable)?;
    if !target.is_file() || target.mode() & 0o111 == 0 {
        return Err("selected server binary must be an executable regular file".into());
    }
    let mut previous = None;
    if request.socket.try_exists()? {
        if let Ok(session) = Session::open(request.socket) {
            if !request.replace_running {
                return Ok(());
            }
            let identity = super::upgrade::identify(&session.socket, request.state)?;
            if identity.device == target.dev() && identity.inode == target.ino() {
                return Ok(());
            }
            previous = Some(session);
        }
    }
    // Fail obvious configuration mistakes before persisting or forking. The
    // server still handles races and relative shell names resolved through PATH.
    if !request.cwd.is_dir() {
        return Err("workspace working directory does not exist or is not a directory".into());
    }
    if request.shell.is_absolute() {
        let shell = fs::metadata(request.shell)?;
        if !shell.is_file() || shell.mode() & 0o111 == 0 {
            return Err("workspace shell must be an executable file".into());
        }
    }
    super::stream::validate_size(request.cols, request.rows)?;
    if let Some(session) = previous {
        eprintln!("Updating workspace: ending the previous build's shells and jobs.");
        shutdown(session, request.socket)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if !request.socket.try_exists()? {
                // Socket removal precedes release of the endpoint's locks.
                if let Ok(guard) = journal::Lock::acquire(&request.state.join("native.lock"), false)
                {
                    drop(guard);
                    break;
                }
            }
            if Instant::now() >= deadline {
                return Err("previous server is still shutting down; reconnect shortly".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
    let config = request.state.join("workspace.json");
    persist_settings(&config, request.shell, request.cwd)?;
    let size = format!("{}x{}", request.cols, request.rows);
    let mut command = Command::new(&executable);
    // The server and its panes start clean of the starter's agent and
    // multiplexer session markers (a start from inside Claude Code or tmux).
    super::shell::remove_session_markers(&mut command);
    command
        .arg("server")
        .arg("--background")
        .arg("--socket")
        .arg(request.socket)
        .arg("--state-dir")
        .arg(request.state)
        .arg("--shell")
        .arg(request.shell)
        .arg("--cwd")
        .arg(request.cwd)
        .arg("--size")
        .arg(size)
        .arg("--config")
        .arg(&config);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if Session::open(request.socket).is_ok() {
            let _ = child.wait();
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err(format!(
                    "workspace startup failed ({status}); inspect {}/server.log",
                    request.state.display()
                )
                .into());
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "workspace did not become ready; inspect {}/server.log",
                request.state.display()
            )
            .into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}
/// Ask a running server to stop. The server replies, then ends its panes and
/// exits. Servers without `server-lifecycle` (before the stream session API)
/// get the legacy shutdown command of the composed protocol.
pub fn stop(socket: &Path) -> Result<()> {
    shutdown(Session::open(socket)?, socket)
}
fn shutdown(mut session: Session, socket: &Path) -> Result<()> {
    if session.has("server-lifecycle") {
        let request = session.request_id();
        return match session.request(super::stream::Request::Shutdown { request })? {
            super::stream::Event::Done { .. } => Ok(()),
            _ => Err("server did not acknowledge shutdown".into()),
        };
    }
    drop(session);
    // A stream peer may send only stream requests: use a new connection.
    let mut legacy = super::connection::connect(socket)?;
    write_message(&mut legacy, &serde_json::json!({"Command": "Shutdown"}))?;
    let reply: serde_json::Value = {
        let mut header = [0u8; 4];
        legacy.read_exact(&mut header)?;
        let length = u32::from_be_bytes(header) as usize;
        if length == 0 || length > MAX_MESSAGE {
            return Err("invalid legacy server reply".into());
        }
        let mut bytes = vec![0; length];
        legacy.read_exact(&mut bytes)?;
        serde_json::from_slice(&bytes)?
    };
    match reply.get("Error").and_then(|error| error.as_str()) {
        Some(error) => Err(error.to_owned().into()),
        None if reply.get("State").is_some() => Ok(()),
        None => Err("previous server did not acknowledge shutdown".into()),
    }
}
pub fn save_pid(state: &Path) -> Result<()> {
    journal::atomic(&state.join("server.pid"), &std::process::id())?;
    super::upgrade::save(state)
}

/// Call under the exclusive workspace lock, only after startup succeeds.
/// A marker is cleared solely on a completed orderly shutdown, never by Drop.
pub fn begin_run(state: &Path) -> Result<bool> {
    let path = state.join("server.running.json");
    let unclean = match fs::symlink_metadata(&path) {
        Ok(meta) => {
            if !meta.is_file()
                || meta.uid() != unsafe { libc::geteuid() }
                || meta.mode() & 0o077 != 0
                || meta.len() > 32
            {
                return Err("server run marker must be an owned private regular file".into());
            }
            if !serde_json::from_slice::<bool>(&fs::read(&path)?)? {
                return Err("invalid server run marker".into());
            }
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    journal::atomic(&path, &true)?;
    Ok(unclean)
}
pub fn finish_run(state: &Path) -> Result<()> {
    fs::remove_file(state.join("server.running.json"))?;
    fs::File::open(state)?.sync_all()?;
    Ok(())
}
