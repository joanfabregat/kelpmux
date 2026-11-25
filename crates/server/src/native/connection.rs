//! Same-user socket validation, bounded replies and one-shot stream sessions
//! shared by clients, bridges and the lifecycle commands.
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

/// A stream connection that completed Hello, for one-shot administration
/// (`start`, `stop`, `status`, `diagnostics`). Updates and other unsolicited
/// events are skipped while waiting for a reply.
pub struct Session {
    pub socket: UnixStream,
    pub client: u64,
    pub epoch: String,
    pub capabilities: Vec<String>,
    pub server_version: Option<String>,
    next_request: u64,
}
impl Session {
    /// Connect and negotiate the stream protocol. A server that is not
    /// running, or does not speak the stream protocol, is an error.
    pub fn open(path: &Path) -> Result<Self> {
        use super::stream::{Event, Request, VERSION};
        let mut socket = connect(path)?;
        write_message(
            &mut socket,
            &ClientMessage::Stream(Request::Hello { version: VERSION }),
        )?;
        match response(&mut socket)? {
            ServerMessage::Stream(Event::Welcome {
                version,
                client,
                epoch,
                capabilities,
                server_version,
                ..
            }) if version == VERSION => Ok(Self {
                socket,
                client,
                epoch,
                capabilities,
                server_version,
                next_request: 1,
            }),
            ServerMessage::Stream(Event::Error { message, .. }) | ServerMessage::Error(message) => {
                Err(format!("server refused the stream protocol: {message}").into())
            }
            _ => Err("server did not answer the stream hello".into()),
        }
    }
    pub fn has(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }
    /// A fresh correlation identifier for this connection.
    pub fn request_id(&mut self) -> String {
        let id = format!("admin-{}", self.next_request);
        self.next_request += 1;
        id
    }
    /// The next stream event before `deadline`.
    pub fn next(&mut self, deadline: Instant) -> Result<super::stream::Event> {
        if Instant::now() >= deadline {
            return Err("native server did not reply in time".into());
        }
        match response(&mut self.socket)? {
            ServerMessage::Stream(event) => Ok(event),
            ServerMessage::Error(message) => Err(message.into()),
            ServerMessage::Diagnostics(_) => Err("unexpected diagnostics reply".into()),
        }
    }
    /// Send a correlated request and wait for its reply. A correlated
    /// `error` reply is returned as an error.
    pub fn request(&mut self, request: super::stream::Request) -> Result<super::stream::Event> {
        let id = request
            .correlation()
            .ok_or("request has no correlation identifier")?
            .to_owned();
        write_message(&mut self.socket, &ClientMessage::Stream(request))?;
        let deadline = Instant::now() + TIMEOUT + TIMEOUT;
        loop {
            let event = self.next(deadline)?;
            if event.correlation() == Some(id.as_str()) {
                if let super::stream::Event::Error { message, .. } = event {
                    return Err(message.into());
                }
                return Ok(event);
            }
        }
    }
    /// The first metadata update (pane list, leases, notifications).
    pub fn update(&mut self) -> Result<super::stream::Update> {
        let deadline = Instant::now() + TIMEOUT + TIMEOUT;
        loop {
            match self.next(deadline)? {
                super::stream::Event::Update(update) => return Ok(update),
                super::stream::Event::Error { message, .. } => return Err(message.into()),
                _ => {}
            }
        }
    }
}
