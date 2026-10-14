//! Explicit named workspaces and detached process ownership, without a service unit.
use super::{protocol::*, ui::UiConfig};
use crate::{Result, journal};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{MetadataExt, OpenOptionsExt},
            net::UnixStream,
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub ui: UiConfig,
    pub shell: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub grid: (u8, u8),
}
impl Default for Settings {
    fn default() -> Self {
        Self {
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
        let settings: Self = serde_json::from_slice(&fs::read(path)?)?;
        settings.ui.validate()?;
        if ![(1, 1), (2, 1), (2, 2), (3, 2), (2, 3), (3, 3)].contains(&settings.grid) {
            return Err("unsupported startup grid".into());
        }
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
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    write_message(&mut stream, &ClientMessage::Inspect)?;
    read_message(&mut stream)?.ok_or_else(|| "workspace disconnected".into())
}
/// Called before any threads are created. The original process returns false;
/// its session-independent grandchild continues with stdio redirected.
pub fn detach(state: &Path) -> Result<bool> {
    journal::private_dir(state)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
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
    if matches!(inspect(request.socket), Ok(ServerMessage::State { .. })) {
        return Ok(());
    }
    journal::private_dir(request.state)?;
    journal::atomic(&request.state.join("workspace.json"), request.settings)?;
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
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(state.join("server.pid"))?;
    writeln!(file, "{}", std::process::id())?;
    Ok(())
}
