//! SSH command-channel transport. Does not attach a TUI or start a workspace.
use super::{protocol::*, stream};
use crate::Result;
use std::{
    collections::VecDeque,
    io,
    os::fd::AsRawFd,
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

const QUEUE_BYTES: usize = 2 * (MAX_MESSAGE + 4);
const QUEUE_RECORDS: usize = 32;
struct Nonblocking {
    fd: i32,
    flags: i32,
}
impl Nonblocking {
    fn new(fd: i32) -> Result<Self> {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self { fd, flags })
    }
}
impl Drop for Nonblocking {
    fn drop(&mut self) {
        unsafe {
            libc::fcntl(self.fd, libc::F_SETFL, self.flags);
        }
    }
}
#[derive(Default)]
struct Queue {
    records: VecDeque<Vec<u8>>,
    offset: usize,
    bytes: usize,
}
impl Queue {
    fn room(&self) -> bool {
        self.bytes < QUEUE_BYTES / 2 && self.records.len() < QUEUE_RECORDS / 2
    }
    fn push(&mut self, bytes: Vec<u8>) -> Result<()> {
        if self.bytes + bytes.len() > QUEUE_BYTES || self.records.len() >= QUEUE_RECORDS {
            return Err("bridge queue limit exceeded".into());
        }
        self.bytes += bytes.len();
        self.records.push_back(bytes);
        Ok(())
    }
    fn flush(&mut self, fd: i32) -> Result<()> {
        for _ in 0..8 {
            let Some(front) = self.records.front() else {
                break;
            };
            let rest = &front[self.offset..];
            let count = unsafe { libc::write(fd, rest.as_ptr().cast(), rest.len()) };
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::WouldBlock {
                    break;
                }
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error.into());
            }
            if count == 0 {
                return Err("bridge output closed".into());
            }
            self.offset += count as usize;
            self.bytes -= count as usize;
            if self.offset == front.len() {
                self.records.pop_front();
                self.offset = 0;
            }
        }
        Ok(())
    }
}
#[derive(Default)]
struct Incoming {
    bytes: Vec<u8>,
    started: Option<Instant>,
    initialized: bool,
}
impl Incoming {
    fn read(&mut self, fd: i32, client: bool, hello: &mut bool, out: &mut Queue) -> Result<bool> {
        let mut chunk = [0u8; 8192];
        let count = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
        if count < 0 {
            let error = io::Error::last_os_error();
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) {
                return Ok(false);
            }
            return Err(error.into());
        }
        if count == 0 {
            if !self.bytes.is_empty() {
                return Err("truncated bridge record".into());
            }
            return Ok(true);
        }
        self.started.get_or_insert_with(Instant::now);
        self.bytes.extend_from_slice(&chunk[..count as usize]);
        self.parse(client, hello, out)?;
        Ok(false)
    }
    fn parse(&mut self, client: bool, hello: &mut bool, out: &mut Queue) -> Result<()> {
        while out.room() {
            if self.bytes.len() < 4 {
                break;
            }
            let length = u32::from_be_bytes(self.bytes[..4].try_into().unwrap()) as usize;
            if length == 0 || length > MAX_MESSAGE {
                return Err("invalid bridge record length".into());
            }
            if self.bytes.len() < length + 4 {
                break;
            }
            let payload = &self.bytes[4..length + 4];
            if client {
                let ClientMessage::Stream(request) = serde_json::from_slice(payload)? else {
                    return Err("SSH bridge accepts only native stream requests".into());
                };
                if !*hello {
                    if !matches!(request, stream::Request::Hello { version } if version == stream::VERSION)
                    {
                        return Err("bridge requires a supported stream hello first".into());
                    }
                    *hello = true;
                } else if matches!(request, stream::Request::Hello { .. }) {
                    return Err("duplicate bridge hello".into());
                }
            } else {
                match serde_json::from_slice::<ServerMessage>(payload)? {
                    ServerMessage::Stream(stream::Event::Welcome {
                        version,
                        client,
                        epoch,
                        ..
                    }) => {
                        if self.initialized
                            || version != stream::VERSION
                            || client == 0
                            || epoch.is_empty()
                            || epoch.len() > 128
                            || epoch.chars().any(char::is_control)
                        {
                            return Err("invalid native stream server welcome".into());
                        }
                        self.initialized = true;
                    }
                    ServerMessage::Stream(stream::Event::Error { .. })
                    | ServerMessage::Error(_) => {}
                    ServerMessage::Stream(_) if self.initialized => {}
                    _ => return Err("workspace did not speak the native stream protocol".into()),
                }
            }
            out.push(self.bytes.drain(..length + 4).collect())?;
            self.started = if self.bytes.is_empty() {
                None
            } else {
                Some(Instant::now())
            };
        }
        Ok(())
    }
    fn timeout(&self) -> Result<()> {
        if self
            .started
            .is_some_and(|at| at.elapsed() > Duration::from_secs(10))
        {
            return Err("bridge record timed out".into());
        }
        Ok(())
    }
}
/// EOF detaches this native connection and releases its leases. Pending unacknowledged
/// requests are never replayed; callers must not treat transport completion as an ack.
pub fn run(socket: UnixStream) -> Result<()> {
    if unsafe { libc::isatty(0) } == 1 || unsafe { libc::isatty(1) } == 1 {
        return Err("bridge requires an SSH command channel without a PTY (ssh -T)".into());
    }
    let _input_flags = Nonblocking::new(0)?;
    let _output_flags = Nonblocking::new(1)?;
    socket.set_nonblocking(true)?;
    let server = socket.as_raw_fd();
    let mut inbound = Incoming::default();
    let mut outbound = Incoming::default();
    let mut to_server = Queue::default();
    let mut to_client = Queue::default();
    let mut hello = false;
    loop {
        to_server.flush(server)?;
        to_client.flush(1)?;
        inbound.parse(true, &mut hello, &mut to_server)?;
        outbound.parse(false, &mut hello, &mut to_client)?;
        inbound.timeout()?;
        outbound.timeout()?;
        let mut poll = [
            libc::pollfd {
                fd: 0,
                events: if to_server.room() { libc::POLLIN } else { 0 },
                revents: 0,
            },
            libc::pollfd {
                fd: server,
                events: if to_client.room() { libc::POLLIN } else { 0 }
                    | if to_server.bytes > 0 {
                        libc::POLLOUT
                    } else {
                        0
                    },
                revents: 0,
            },
            libc::pollfd {
                fd: 1,
                events: if to_client.bytes > 0 {
                    libc::POLLOUT
                } else {
                    0
                },
                revents: 0,
            },
        ];
        let result = unsafe { libc::poll(poll.as_mut_ptr(), poll.len() as _, 50) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if poll[2].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Ok(());
        }
        if poll[0].revents & libc::POLLHUP != 0 && !to_server.room() {
            return Ok(());
        }
        if poll[1].revents & libc::POLLHUP != 0 && !to_client.room() {
            return Err("workspace closed the backpressured native stream".into());
        }
        if poll[0].revents & (libc::POLLIN | libc::POLLHUP) != 0
            && to_server.room()
            && inbound.read(0, true, &mut hello, &mut to_server)?
        {
            return Ok(());
        }
        if poll[1].revents & (libc::POLLIN | libc::POLLHUP) != 0
            && to_client.room()
            && outbound.read(server, false, &mut hello, &mut to_client)?
        {
            return Err("workspace closed the stream; check that it supports the experimental native protocol".into());
        }
        if poll[0].revents & (libc::POLLERR | libc::POLLNVAL) != 0
            || poll[1].revents & (libc::POLLERR | libc::POLLNVAL) != 0
        {
            return Err("bridge transport failed".into());
        }
    }
}
