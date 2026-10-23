//! Explicit named workspaces and detached process ownership, without a service unit.
use super::{protocol::*, ui::UiConfig};
use crate::{Result, journal};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub tab_count: Option<usize>,
    #[serde(skip_serializing)]
    pub grid_weights: [Vec<u16>; 2],
    pub template: Option<super::templates::Template>,
    pub ui: UiConfig,
    pub shell: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    #[serde(skip_serializing)]
    pub grid: (u8, u8),
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_count: None,
            grid_weights: [Vec::new(), Vec::new()],
            template: None,
            ui: UiConfig::default(),
            shell: None,
            cwd: None,
            grid: (1, 1),
        }
    }
}
impl Settings {
    pub fn load(path: &Path) -> Result<Self> {
        let meta = fs::symlink_metadata(path)?;
        if !meta.is_file() || meta.len() > 64 * 1024 {
            return Err("configuration must be a regular file smaller than 64 KiB".into());
        }
        let mut settings: Self = serde_json::from_slice(&fs::read(path)?)?;
        if settings.tab_count.is_some_and(|count| count == 0) {
            return Err("tab_count must be positive".into());
        }
        settings.ui.validate()?;
        for (weights, count) in settings
            .grid_weights
            .iter()
            .zip([settings.grid.0, settings.grid.1])
        {
            if !weights.is_empty()
                && (weights.len() != usize::from(count)
                    || weights.iter().any(|weight| *weight < 50 || *weight > 1000)
                    || weights.iter().map(|weight| u32::from(*weight)).sum::<u32>() > 1000)
            {
                return Err("invalid persistent grid weights".into());
            }
        }
        if let Some(template) = &settings.template {
            template.validate_schema()?;
        }
        if settings.grid.0 == 0 || settings.grid.1 == 0 {
            return Err("unsupported startup grid".into());
        }
        settings
            .tab_count
            .get_or_insert(usize::from(settings.grid.0) * usize::from(settings.grid.1));
        Ok(settings)
    }
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
pub fn inspect(socket: &Path) -> Result<ServerMessage> {
    let mut stream = super::cli::connect(socket)?;
    write_message(&mut stream, &ClientMessage::Inspect)?;
    super::cli::response(&mut stream)
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
    pub cols: u16,
    pub rows: u16,
    pub settings: &'a Settings,
}
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
    if unsafe { libc::flock(startup_lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::WouldBlock {
            return Err(error.into());
        }
        return Err("workspace startup is already in progress; retry when ready".into());
    }
    if matches!(inspect(request.socket), Ok(ServerMessage::State { .. })) {
        return Ok(());
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
    let mut persisted = request.settings.clone();
    persisted.shell = Some(request.shell.to_path_buf());
    persisted.cwd = Some(request.cwd.to_path_buf());
    journal::atomic(&request.state.join("workspace.json"), &persisted)?;
    let size = format!("{}x{}", request.cols, request.rows);
    let mut child = Command::new(std::env::current_exe()?)
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
        .arg(request.state.join("workspace.json"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if matches!(inspect(request.socket), Ok(ServerMessage::State { .. })) {
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
pub fn save_pid(state: &Path) -> Result<()> {
    journal::atomic(&state.join("server.pid"), &std::process::id())?;
    super::upgrade::save(state)
}
