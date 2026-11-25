//! Server identity for upgrades. A live workspace is never restarted to
//! upgrade it; clients authenticate the running server with `identify` and
//! choose a compatible client themselves.
use crate::{Result, journal};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{MetadataExt, OpenOptionsExt},
            net::UnixStream,
        },
    },
    path::{Path, PathBuf},
};

/// The local protocol number recorded by servers of the composed protocol
/// (KelpServer 0.2 and KelpMux before the split). Those servers also speak
/// stream protocol 1.
pub const LEGACY_COMPOSED_PROTOCOL: u16 = 58;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub pid: u32,
    pub birth: u64,
    pub device: u64,
    pub inode: u64,
    pub executable: PathBuf,
    pub version: String,
    /// The protocol the server speaks on its socket: the stream protocol
    /// version (1) since KelpServer 0.3, `LEGACY_COMPOSED_PROTOCOL` for
    /// servers of the composed protocol, 0 for unknown legacy metadata.
    pub protocol: u16,
}
impl Identity {
    /// Zero denotes unavailable legacy metadata, never a supported protocol.
    pub fn protocol_label(&self) -> String {
        match self.protocol {
            0 => "protocol unknown".into(),
            LEGACY_COMPOSED_PROTOCOL => format!("local protocol {LEGACY_COMPOSED_PROTOCOL}"),
            version => format!("stream protocol {version}"),
        }
    }
    /// Whether a stream client of this build can use the server.
    pub fn speaks_stream(&self) -> bool {
        self.protocol == super::stream::VERSION || self.protocol == LEGACY_COMPOSED_PROTOCOL
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
    identity.protocol = super::stream::VERSION;
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
