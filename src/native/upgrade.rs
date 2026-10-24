//! Version-aware attachment. A live workspace is never restarted to upgrade it.
use crate::{Result, journal};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{MetadataExt, OpenOptionsExt},
            net::UnixStream,
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub pid: u32,
    pub birth: u64,
    pub device: u64,
    pub inode: u64,
    pub executable: PathBuf,
    pub version: String,
    pub protocol: u16,
}
impl Identity {
    /// Zero denotes unavailable legacy metadata, never a supported protocol.
    pub fn protocol_label(&self) -> String {
        if self.protocol == 0 {
            "unknown".into()
        } else {
            self.protocol.to_string()
        }
    }
}

fn birth(pid: u32) -> Result<u64> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    text.rsplit_once(") ")
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .ok_or("invalid server process identity")?
        .parse()
        .map_err(Into::into)
}
fn process(pid: u32) -> Result<Identity> {
    let path = PathBuf::from(format!("/proc/{pid}/exe"));
    let owner = fs::metadata(format!("/proc/{pid}"))?;
    if owner.uid() != unsafe { libc::geteuid() } {
        return Err("workspace server belongs to another user".into());
    }
    let meta = fs::metadata(&path)?;
    Ok(Identity {
        pid,
        birth: birth(pid)?,
        device: meta.dev(),
        inode: meta.ino(),
        executable: fs::read_link(path)?,
        version: "unknown (legacy server)".into(),
        protocol: 0,
    })
}
pub fn save(state: &Path) -> Result<()> {
    let mut identity = process(std::process::id())?;
    identity.version = env!("CARGO_PKG_VERSION").into();
    identity.protocol = super::protocol::VERSION;
    journal::atomic(&state.join("server.identity.json"), &identity)
}

/// Authenticate the listener using kernel peer credentials, not a reusable PID file.
/// Historical versions without an identity file remain attachable when their exact
/// running executable is still installed in the trusted immutable versions tree.
pub fn identify(socket: &UnixStream, state: &Path) -> Result<Identity> {
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut size,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    if size as usize != std::mem::size_of::<libc::ucred>()
        || credentials.uid != unsafe { libc::geteuid() }
        || credentials.pid <= 0
    {
        return Err("workspace socket has an invalid server identity".into());
    }
    let actual = process(credentials.pid as u32)?;
    let path = state.join("server.identity.json");
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(actual),
        Err(error) => return Err(error.into()),
    };
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || meta.len() > 4096
    {
        return Err(
            "server identity must be an owned private regular file smaller than 4 KiB".into(),
        );
    }
    let mut text = Vec::new();
    (&mut file).take(4097).read_to_end(&mut text)?;
    let saved: Identity = serde_json::from_slice(&text)?;
    if saved.version.is_empty()
        || saved.version.len() > 64
        || !saved.version.bytes().all(|byte| byte.is_ascii_graphic())
        || saved.protocol == 0
    {
        return Err("invalid server version identity".into());
    }
    if saved.pid != actual.pid
        || saved.birth != actual.birth
        || saved.device != actual.device
        || saved.inode != actual.inode
        || saved.executable != actual.executable
    {
        return Err("workspace server identity is stale or does not match its live socket; no processes were changed".into());
    }
    Ok(saved)
}

/// Resolve only the exact running executable; a directory name never proves a version.
/// The retained file descriptor pins the inode across renames and replacement.
pub fn matching_binary(
    identity: &Identity,
    current: &Path,
    versions: &Path,
) -> Result<Option<File>> {
    let current_meta = fs::metadata(current)?;
    if current_meta.dev() == identity.device && current_meta.ino() == identity.inode {
        return Ok(None);
    }
    if !identity.executable.starts_with(versions)
        || identity
            .executable
            .file_name()
            .is_none_or(|name| name != "kelpmux")
    {
        return Err(format!("workspace runs Kelpmux {} (protocol {}); its matching client is outside the trusted versions directory {}; existing sessions are unchanged", identity.version, identity.protocol_label(), versions.display()).into());
    }
    for path in identity.executable.ancestors() {
        let meta = fs::symlink_metadata(path)?;
        // A root-owned sticky ancestor cannot replace another user's private
        // child directory. This permits safe test/temp installations as well.
        let protected_sticky = meta.is_dir() && meta.uid() == 0 && meta.mode() & 0o1000 != 0;
        if meta.file_type().is_symlink()
            || (meta.mode() & 0o022 != 0 && !protected_sticky)
            || (meta.uid() != unsafe { libc::geteuid() } && meta.uid() != 0)
        {
            return Err(
                "matching client path contains a symlink, foreign owner or writable directory"
                    .into(),
            );
        }
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&identity.executable)
        .map_err(|error| format!("matching installed client is unavailable ({error}); existing sessions are unchanged"))?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o022 != 0
        || meta.mode() & 0o111 == 0
        || meta.dev() != identity.device
        || meta.ino() != identity.inode
    {
        return Err("matching installed client is missing, replaced or unsafe; existing sessions are unchanged".into());
    }
    Ok(Some(file))
}

/// Called before terminal raw mode. The descriptor and process birth prevent a
/// swapped installation or reused PID from selecting another executable.
pub fn attach_matching(socket: &Path, state: &Path, takeover: bool) -> Result<UnixStream> {
    let stream = super::cli::connect(socket)?;
    let identity = identify(&stream, state)?;
    let current = std::env::current_exe()?;
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?);
    let installation = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"))
        .join("kelpmux");
    let builds = installation.join("builds");
    let versions = if identity.executable.starts_with(&builds) {
        builds
    } else {
        installation.join("versions")
    };
    if let Some(binary) = matching_binary(&identity, &current, &versions)? {
        let latest = process(identity.pid)?;
        if latest.birth != identity.birth
            || latest.device != identity.device
            || latest.inode != identity.inode
        {
            return Err(
                "workspace server changed while selecting its matching client; retry".into(),
            );
        }
        eprintln!(
            "Kelpmux workspace: using retained client {} (protocol {}); server remains running",
            identity.version,
            identity.protocol_label()
        );
        // proc/self/fd opens the pinned inode, even if its installation path is replaced.
        // A real ELF executable does not need its fd after exec (CLOEXEC is preserved).
        let mut command = Command::new(format!("/proc/self/fd/{}", binary.as_raw_fd()));
        command
            .arg("attach")
            .arg("--socket")
            .arg(socket)
            .arg("--state-dir")
            .arg(state);
        if takeover {
            command.arg("--takeover");
        }
        return Err(command.exec().into());
    }
    Ok(stream)
}
