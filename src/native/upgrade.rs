//! Version-aware attachment. A live workspace is never restarted to upgrade it.
//!
//! KelpServer keeps the server identity (`Identity`, `save`, `identify`);
//! choosing the client binary that matches a running server is KelpMux's.
//! `matching_binary` and `attach_matching` were copied from KelpServer's
//! `native::upgrade` (kelpserver afc5197, Apache-2.0, same authors), which
//! removed them in 0.3.0-beta.2. They still serve workspaces whose server
//! runs inside a `kelpmux` executable: `kelpmux server`, and servers started
//! by an installed KelpMux before the server split.
pub use kelpserver::native::upgrade::*;

use crate::Result;
use std::{
    fs::{self, File, OpenOptions},
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

/// The start time of `pid` in clock ticks, which a reused PID does not share.
fn birth(pid: u32) -> Result<u64> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    text.rsplit_once(") ")
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .ok_or("invalid server process identity")?
        .parse()
        .map_err(Into::into)
}
/// The live process behind `pid`: birth and executable inode.
fn process(pid: u32) -> Result<(u64, u64, u64)> {
    let path = PathBuf::from(format!("/proc/{pid}/exe"));
    let owner = fs::metadata(format!("/proc/{pid}"))?;
    if owner.uid() != unsafe { libc::geteuid() } {
        return Err("workspace server belongs to another user".into());
    }
    let meta = fs::metadata(&path)?;
    Ok((birth(pid)?, meta.dev(), meta.ino()))
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
        return Err(format!("workspace runs Kelpmux {} ({}); its matching client is outside the trusted versions directory {}; existing sessions are unchanged", identity.version, identity.protocol_label(), versions.display()).into());
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
    let stream = super::connection::connect(socket)?;
    let identity = identify(&stream, state)?;
    // A split server executable is not a retained TUI client. Authenticate its
    // identity and require a compatible wire version before using this client.
    if identity
        .executable
        .file_name()
        .is_some_and(|name| name == "kelpserver")
    {
        if !identity.speaks_stream() {
            return Err(format!(
                "KelpServer {} ({}) is incompatible with this client; install a matching client; sessions are unchanged",
                identity.version,
                identity.protocol_label()
            )
            .into());
        }
        return Ok(stream);
    }
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
        let (birth, device, inode) = process(identity.pid)?;
        if birth != identity.birth || device != identity.device || inode != identity.inode {
            return Err(
                "workspace server changed while selecting its matching client; retry".into(),
            );
        }
        eprintln!(
            "Kelpmux workspace: using retained client {} ({}); server remains running",
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
