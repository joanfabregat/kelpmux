//! Same-user socket validation and bounded handshake shared by clients and bridges.
use super::protocol::*;
use crate::Result;
use std::{
    fs,
    io::Read,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileTypeExt, MetadataExt},
            net::UnixStream,
        },
    },
    path::Path,
    time::{Duration, Instant},
};
const TIMEOUT: Duration = Duration::from_secs(3);
pub fn connect(path: &Path) -> Result<UnixStream> {
    for ancestor in path.ancestors() {
        if fs::symlink_metadata(ancestor).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err("native socket path contains a symlink".into());
        }
    }
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_socket()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
    {
        return Err("native socket must be private and owned by this user".into());
    }
    let parent = fs::symlink_metadata(path.parent().ok_or("socket needs a directory")?)?;
    if !parent.is_dir() || parent.uid() != unsafe { libc::geteuid() } || parent.mode() & 0o077 != 0
    {
        return Err("native socket directory must be private and owned by this user".into());
    }
    let socket = UnixStream::connect(path)?;
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
    {
        return Err(std::io::Error::last_os_error().into());
    }
    if length as usize != std::mem::size_of::<libc::ucred>()
        || credentials.uid != unsafe { libc::geteuid() }
    {
        return Err("native server belongs to another user".into());
    }
    socket.set_read_timeout(Some(TIMEOUT))?;
    socket.set_write_timeout(Some(TIMEOUT))?;
    Ok(socket)
}
pub fn response(socket: &mut UnixStream) -> Result<ServerMessage> {
    let deadline = Instant::now() + TIMEOUT;
    fn exact(socket: &mut UnixStream, mut bytes: &mut [u8], deadline: Instant) -> Result<()> {
        while !bytes.is_empty() {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("native handshake timed out")?;
            socket.set_read_timeout(Some(remaining))?;
            match socket.read(bytes) {
                Ok(0) => return Err("native server disconnected before its reply".into()),
                Ok(n) => {
                    bytes = &mut bytes[n..];
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
    let mut header = [0; 4];
    exact(socket, &mut header, deadline)?;
    let size = u32::from_be_bytes(header) as usize;
    if size == 0 || size > MAX_MESSAGE {
        return Err("invalid native reply size".into());
    }
    let mut bytes = vec![0; size];
    exact(socket, &mut bytes, deadline)?;
    Ok(serde_json::from_slice(&bytes)?)
}
