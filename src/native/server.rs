//! A foreground, owned PTY server. Client lifetimes never own shell lifetimes.
use super::{
    engine::{Effect, Terminal},
    protocol::*,
    render,
    workspace::Workspace,
};
use crate::{Result, discovery::Discovery, journal, model::Status, process};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            fs::{MetadataExt, PermissionsExt},
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub struct ServerConfig {
    pub initial_weights: [Vec<u16>; 2],
    pub template: Option<super::templates::Template>,
    pub socket: PathBuf,
    pub state_dir: PathBuf,
    pub shell: PathBuf,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
    pub ui: super::ui::UiConfig,
    pub initial_grid: (u8, u8),
    pub initial_tabs: Option<usize>,
}
const MAX_CLIENTS: usize = 16;
const MAX_QUEUED: usize = MAX_MESSAGE + 4;
const MAX_PTY_WRITE: usize = 256 * 1024;

fn private_directory(path: &Path) -> Result<()> {
    if path == Path::new("/") || path == Path::new("/tmp") {
        return Err("native state needs a dedicated private directory".into());
    }
    // Reject symlink ancestors too: checking only the leaf permits redirection.
    for ancestor in path.ancestors() {
        if let Ok(meta) = fs::symlink_metadata(ancestor) {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err("native directory has a symlink or non-directory component".into());
            }
        }
    }
    journal::private_dir(path)
}
fn lock(path: &Path) -> Result<journal::Lock> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return Err("native lock must be a private owned regular file".into());
        }
    }
    journal::Lock::acquire(path, false)
}
struct Endpoint {
    path: PathBuf,
    inode: u64,
    _socket_lock: journal::Lock,
    _state_lock: journal::Lock,
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|m| m.ino() == self.inode) {
            let _ = fs::remove_file(&self.path);
        }
    }
}
fn endpoint(config: &ServerConfig) -> Result<(UnixListener, Endpoint)> {
    private_directory(&config.state_dir)?;
    let parent = config
        .socket
        .parent()
        .ok_or("socket needs a parent directory")?;
    if parent == Path::new("/") || parent == Path::new("/tmp") {
        return Err("socket needs a dedicated private directory".into());
    }
    private_directory(parent)?;
    let state_lock = lock(&config.state_dir.join("native.lock"))?;
    let socket_lock = lock(&config.socket.with_extension("sock.lock"))?;
    if let Ok(meta) = fs::symlink_metadata(&config.socket) {
        use std::os::unix::fs::FileTypeExt;
        if !meta.file_type().is_socket()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o077 != 0
        {
            return Err("refusing foreign or non-private native socket".into());
        }
        match UnixStream::connect(&config.socket) {
            Ok(_) => return Err("native server is already running".into()),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                fs::remove_file(&config.socket)?
            }
            Err(e) => return Err(e.into()),
        }
    }
    let listener = UnixListener::bind(&config.socket)?;
    fs::set_permissions(&config.socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let guard = Endpoint {
        inode: fs::symlink_metadata(&config.socket)?.ino(),
        path: config.socket.clone(),
        _socket_lock: socket_lock,
        _state_lock: state_lock,
    };
    Ok((listener, guard))
}

struct NativePane {
    size: (u16, u16),
    child: Child,
    master: File,
    start: u64,
    terminal: Terminal,
    pending: VecDeque<u8>,
    dead: bool,
    last_input: Option<Instant>,
    idle_observations: u8,
    agent_named: bool,
    terminal_title: Option<String>,
}
impl NativePane {
    fn spawn(config: &ServerConfig, cols: u16, rows: u16, id: PaneId) -> Result<Self> {
        let mut master = -1;
        let mut slave = -1;
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: openpty initializes both descriptors and only reads the winsize.
        if unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                &size,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
            if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        let mut command = Command::new(&config.shell);
        command
            .arg("-i")
            .current_dir(&config.cwd)
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env("TERM_PROGRAM", "kelpmux")
            .env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"))
            .env_remove("NO_COLOR")
            .env("KELPMUX", "1")
            .env("KELPMUX_PANE", format!("%{id}"))
            .env("KELPMUX_STATE_DIR", &config.state_dir)
            .env_remove("SSH_CONNECTION")
            .env_remove("SSH_CLIENT")
            .env_remove("SSH_TTY")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("STY")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("CLAUDECODE")
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        // Only async-signal-safe libc operations occur between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Mark all nonstandard descriptors close-on-exec, including future inherited fds.
                if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 4u32) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        let mut pane = Self {
            size: (cols, rows),
            child,
            master,
            start: 0,
            terminal: Terminal::new(cols, rows),
            pending: VecDeque::new(),
            dead: false,
            last_input: None,
            idle_observations: 0,
            agent_named: false,
            terminal_title: None,
        };
        pane.start = process::read(Path::new("/proc"), pane.child.id())?.start;
        let fd = pane.master.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(pane)
    }
    fn queue(&mut self, bytes: &[u8]) -> Result<()> {
        if self.dead {
            return Err("pane process has exited".into());
        }
        if self.pending.len() + bytes.len() > MAX_PTY_WRITE {
            return Err("pane input queue is full".into());
        }
        self.pending.extend(bytes);
        if !bytes.is_empty() {
            self.last_input = Some(Instant::now());
            self.idle_observations = 0;
        }
        Ok(())
    }
    fn resize(&mut self, cols: u16, rows: u16) {
        if self.size == (cols, rows) {
            return;
        }
        self.size = (cols, rows);
        self.terminal.resize(cols, rows);
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe {
            libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size);
        }
    }
    fn unused(&self, tree: &BTreeMap<u32, process::Process>) -> bool {
        if !self.pending.is_empty()
            || self.idle_observations < 2
            || self
                .last_input
                .is_some_and(|at| at.elapsed() < Duration::from_millis(200))
        {
            return false;
        }
        let Some(root) = tree.get(&self.child.id()) else {
            return false;
        };
        root.start == self.start
            && process::idle(tree, self.child.id(), false)
            && !tree
                .values()
                .any(|p| p.pid != root.pid && p.tty == root.tty)
    }
    fn observe_idle(&mut self) {
        // Two separate event-loop observations plus a cooldown narrow the race
        // between writing terminal input and the shell consuming it. This is
        // conservative evidence, not an atomic guarantee against external input.
        let root = process::read(Path::new("/proc"), self.child.id()).ok();
        let idle = self.pending.is_empty()
            && self
                .last_input
                .is_none_or(|at| at.elapsed() >= Duration::from_millis(200))
            && root.is_some_and(|root| {
                root.start == self.start
                    && process::idle(&BTreeMap::from([(root.pid, root.clone())]), root.pid, false)
            });
        self.idle_observations = if idle {
            self.idle_observations.saturating_add(1).min(2)
        } else {
            0
        };
    }
}
impl Drop for NativePane {
    fn drop(&mut self) {
        // Explicit daemon shutdown owns termination; detach never drops a pane.
        if !self.dead
            && (self.start == 0
                || process::read(Path::new("/proc"), self.child.id())
                    .is_ok_and(|p| p.start == self.start))
        {
            if let Ok(tree) = process::snapshot(Path::new("/proc")) {
                for pid in process::descendants(&tree, self.child.id()) {
                    if pid != self.child.id()
                        && tree.get(&pid).is_some_and(|old| {
                            process::read(Path::new("/proc"), pid)
                                .is_ok_and(|new| old.start == new.start)
                        })
                    {
                        unsafe {
                            libc::kill(pid as i32, libc::SIGKILL);
                        }
                    }
                }
            }
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGHUP);
            }
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

struct Client {
    theme: [Option<[u8; 3]>; 2],
    stream: Option<StreamClient>,
    previous_frame: Option<(u64, Frame)>,
    full_frames: u64,
    patch_frames: u64,
    full_bytes: u64,
    patch_bytes: u64,
    socket: UnixStream,
    input: Vec<u8>,
    started: Option<Instant>,
    attached: bool,
    geometry: Option<(u16, u16)>,
    output: VecDeque<Vec<u8>>,
    writing: Option<(Vec<u8>, usize)>,
    frame: Option<Vec<u8>>,
    queued: usize,
}
struct StreamClient {
    panes: Vec<PaneId>,
    previous: BTreeMap<PaneId, super::stream::PaneSnapshot>,
    metadata: Option<super::stream::Update>,
}
struct PaneLease {
    owner: u64,
    size: (u16, u16),
}
fn wire(message: &ServerMessage) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(message)?;
    if bytes.len() > MAX_MESSAGE {
        return Err("native response is too large".into());
    }
    let mut packet = Vec::with_capacity(bytes.len() + 4);
    packet.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    packet.extend(bytes);
    Ok(packet)
}
impl Client {
    fn new(socket: UnixStream) -> Result<Self> {
        let mut creds: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut creds as *mut _ as *mut _,
                &mut len,
            )
        } < 0
            || creds.uid != unsafe { libc::geteuid() }
        {
            return Err("native peer uid mismatch".into());
        }
        socket.set_nonblocking(true)?;
        Ok(Self {
            theme: [None; 2],
            stream: None,
            previous_frame: None,
            full_frames: 0,
            patch_frames: 0,
            full_bytes: 0,
            patch_bytes: 0,
            socket,
            input: vec![],
            started: None,
            attached: false,
            geometry: None,
            output: VecDeque::new(),
            writing: None,
            frame: None,
            queued: 0,
        })
    }
    fn send(&mut self, message: ServerMessage) -> Result<()> {
        let bytes = wire(&message)?;
        if self.queued + bytes.len() > MAX_QUEUED || self.output.len() >= 32 {
            return Err("slow native client".into());
        }
        self.queued += bytes.len();
        self.output.push_back(bytes);
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        // Work per client is bounded so even a permanently writable observer cannot monopolize PTYs.
        for _ in 0..8 {
            if self.writing.is_none() {
                self.writing = self
                    .output
                    .pop_front()
                    .or_else(|| self.frame.take())
                    .map(|v| (v, 0));
            }
            let Some((bytes, offset)) = &mut self.writing else {
                break;
            };
            match self.socket.write(&bytes[*offset..]) {
                Ok(0) => return Err("native client closed".into()),
                Ok(n) => {
                    *offset += n;
                    if *offset == bytes.len() {
                        self.queued = self.queued.saturating_sub(bytes.len());
                        self.writing = None;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
    fn receive(&mut self) -> Result<Vec<ClientMessage>> {
        let mut messages = vec![];
        let mut buf = [0; 8192];
        for pass in 0..=8 {
            loop {
                if self.input.len() < 4 {
                    break;
                }
                let n = u32::from_be_bytes(self.input[..4].try_into().unwrap()) as usize;
                if n == 0 || n > MAX_MESSAGE {
                    return Err("invalid native message length".into());
                }
                if self.input.len() < n + 4 {
                    break;
                }
                messages.push(serde_json::from_slice(&self.input[4..n + 4])?);
                self.input.drain(..n + 4);
                self.started = if self.input.is_empty() {
                    None
                } else {
                    Some(Instant::now())
                };
                if messages.len() >= 64 {
                    // Yield to PTYs and other clients; buffered complete records
                    // are decoded before the next socket read, even without new bytes.
                    return Ok(messages);
                }
            }
            if self.input.len() > MAX_MESSAGE + 4 {
                return Err("native message buffer full".into());
            }
            if pass == 8 {
                break;
            }
            match self.socket.read(&mut buf) {
                Ok(0) if !messages.is_empty() => break,
                Ok(0) => return Err("native client closed".into()),
                Ok(n) => {
                    self.started.get_or_insert_with(Instant::now);
                    self.input.extend_from_slice(&buf[..n]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }
        if self
            .started
            .is_some_and(|t| t.elapsed() > Duration::from_secs(5))
        {
            return Err("native message timeout".into());
        }
        Ok(messages)
    }
    fn latest(&mut self, frame: &Frame) -> Result<()> {
        let idle = self.frame.is_none() && self.writing.is_none() && self.output.is_empty();
        if idle
            && self
                .previous_frame
                .as_ref()
                .is_some_and(|(_, old)| old == frame)
        {
            return Ok(());
        }
        let patch = if idle {
            self.previous_frame
                .as_ref()
                .and_then(|(revision, old)| super::delta::diff(old, frame, *revision))
        } else {
            None
        };
        let packet = if let Some(patch) = patch {
            let revision = patch.revision;
            let bytes = wire(&ServerMessage::Patch(patch))?;
            self.previous_frame = Some((revision, frame.clone()));
            self.patch_frames += 1;
            self.patch_bytes += bytes.len() as u64;
            bytes
        } else {
            let bytes = wire(&ServerMessage::Frame(frame.clone()))?;
            self.previous_frame = Some((1, frame.clone()));
            self.full_frames += 1;
            self.full_bytes += bytes.len() as u64;
            bytes
        };
        if let Some(old) = self.frame.take() {
            self.queued = self.queued.saturating_sub(old.len());
        }
        // An in-flight frame and one replacement are the only retained frames.
        if self.queued + packet.len() > 2 * MAX_QUEUED {
            return Err("slow native client".into());
        }
        self.queued += packet.len();
        self.frame = Some(packet);
        Ok(())
    }
}

struct RecoveryJob {
    receiver: std::sync::mpsc::Receiver<Result<Vec<RecoveryItem>>>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for RecoveryJob {
    fn drop(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
struct Server {
    focus_click: bool,
    title_animation: BTreeMap<PaneId, (String, u16)>,
    epoch: String,
    leases: BTreeMap<PaneId, PaneLease>,
    initializing: bool,
    recovery_job: Option<RecoveryJob>,
    aliases: BTreeMap<PaneId, String>,
    activity: super::activity::Activity,
    started: Instant,
    pty_bytes: u64,
    vitals: super::vitals::Sampler,
    status: super::status::Monitor,
    config: ServerConfig,
    workspace: Workspace,
    panes: BTreeMap<PaneId, NativePane>,
    clients: BTreeMap<u64, Client>,
    controller: Option<u64>,
    next_pane: u64,
    next_client: u64,
    dirty: bool,
    notice: Option<String>,
    shutdown: bool,
}
impl Server {
    fn terminal_theme(&self) -> [Option<[u8; 3]>; 2] {
        self.controller
            .and_then(|id| self.clients.get(&id))
            .map_or([None; 2], |client| client.theme)
    }
    fn apply_theme(&mut self) {
        let theme = self.terminal_theme();
        for pane in self.panes.values_mut() {
            pane.terminal.set_theme(theme);
        }
        self.dirty = true;
    }
    fn tui_write_allowed(&self, pane: PaneId) -> Result<()> {
        if self.leases.contains_key(&pane) {
            return Err("pane belongs to a native client; claim it explicitly first".into());
        }
        Ok(())
    }
    fn stream_owned(&self, client: u64, pane: PaneId) -> Result<()> {
        if !self.panes.contains_key(&pane) {
            return Err("unknown pane".into());
        }
        if self
            .leases
            .get(&pane)
            .is_none_or(|lease| lease.owner != client)
        {
            return Err("pane lease required".into());
        }
        Ok(())
    }
    fn stream_request(&mut self, id: u64, request: super::stream::Request) -> Result<()> {
        use super::stream::{Event, Request};
        if let Request::Hello { version } = request {
            if version != super::stream::VERSION {
                return Err("unsupported stream protocol version".into());
            }
            let client = self.clients.get_mut(&id).unwrap();
            if client.attached || client.stream.is_some() {
                return Err("connection already initialized".into());
            }
            client.stream = Some(StreamClient {
                panes: Vec::new(),
                previous: BTreeMap::new(),
                metadata: None,
            });
            client.send(ServerMessage::Stream(Event::Welcome {
                version,
                client: id,
                epoch: self.epoch.clone(),
                capabilities: super::stream::CAPABILITIES
                    .iter()
                    .map(|s| (*s).into())
                    .collect(),
            }))?;
            self.dirty = true;
            return Ok(());
        }
        if self.clients[&id].stream.is_none() {
            return Err("stream hello required".into());
        }
        match request {
            Request::Subscribe { panes } => {
                let unique: std::collections::BTreeSet<_> = panes.iter().copied().collect();
                if panes.len() > super::stream::MAX_SUBSCRIPTIONS
                    || unique.len() != panes.len()
                    || panes.iter().any(|pane| !self.panes.contains_key(pane))
                {
                    return Err("invalid pane subscriptions".into());
                }
                if panes
                    .iter()
                    .map(|pane| {
                        usize::from(self.panes[pane].size.0) * usize::from(self.panes[pane].size.1)
                    })
                    .sum::<usize>()
                    > super::stream::MAX_SUBSCRIBED_CELLS
                {
                    return Err("pane subscription cell budget exceeded".into());
                }
                let stream = self.clients.get_mut(&id).unwrap().stream.as_mut().unwrap();
                stream.panes = panes;
                stream.previous.clear();
                stream.metadata = None;
            }
            Request::Claim { pane, takeover } => {
                let pty = self.panes.get(&pane).ok_or("unknown pane")?;
                if pty.dead {
                    return Err("pane process has exited".into());
                }
                if self
                    .leases
                    .get(&pane)
                    .is_some_and(|lease| lease.owner != id)
                    && !takeover
                {
                    return Err("pane already leased".into());
                }
                self.leases.insert(
                    pane,
                    PaneLease {
                        owner: id,
                        size: pty.size,
                    },
                );
                self.clients
                    .get_mut(&id)
                    .unwrap()
                    .send(ServerMessage::Stream(Event::Lease {
                        pane,
                        owner: Some(id),
                    }))?;
            }
            Request::Release { pane } => {
                self.stream_owned(id, pane)?;
                self.leases.remove(&pane);
                self.resize();
                self.clients
                    .get_mut(&id)
                    .unwrap()
                    .send(ServerMessage::Stream(Event::Lease { pane, owner: None }))?;
            }
            Request::Resize { pane, cols, rows } => {
                self.stream_owned(id, pane)?;
                super::stream::validate_size(cols, rows)?;
                self.leases.get_mut(&pane).unwrap().size = (cols, rows);
                self.panes.get_mut(&pane).unwrap().resize(cols, rows);
            }
            Request::Input { pane, bytes } => {
                self.stream_owned(id, pane)?;
                if bytes.len() > MAX_INPUT {
                    return Err("pane input too large".into());
                }
                self.panes.get_mut(&pane).unwrap().queue(&bytes)?;
                if let Some(info) = self.workspace.panes.iter_mut().find(|info| info.id == pane) {
                    info.last_activity = crate::now();
                }
            }
            Request::Paste { pane, bytes } => {
                self.stream_owned(id, pane)?;
                if bytes.len() > MAX_INPUT {
                    return Err("pane paste too large".into());
                }
                let pty = self.panes.get_mut(&pane).unwrap();
                if pty.terminal.snapshot().modes.bracketed_paste {
                    pty.queue(
                        &[
                            b"\x1b[200~".as_slice(),
                            bytes.as_slice(),
                            b"\x1b[201~".as_slice(),
                        ]
                        .concat(),
                    )?;
                } else {
                    pty.queue(&bytes)?;
                }
                if let Some(info) = self.workspace.panes.iter_mut().find(|info| info.id == pane) {
                    info.last_activity = crate::now();
                }
            }
            Request::Mouse { pane, event } => {
                self.stream_owned(id, pane)?;
                let pty = self.panes.get_mut(&pane).unwrap();
                if event.position.x >= pty.size.0 || event.position.y >= pty.size.1 {
                    return Err("mouse outside pane".into());
                }
                let modes = pty.terminal.snapshot().modes;
                if modes.mouse_click || modes.mouse_drag || modes.mouse_motion {
                    if let Some(info) = self.workspace.panes.iter_mut().find(|info| info.id == pane)
                    {
                        info.last_activity = crate::now();
                    }
                    if event.kind == MouseKind::Move
                        && !modes.mouse_motion
                        && !(modes.mouse_drag && event.button < 3)
                    {
                        return Ok(());
                    }
                    let button = match event.kind {
                        MouseKind::WheelUp => 64,
                        MouseKind::WheelDown => 65,
                        _ => u16::from(event.button.min(3)),
                    } + if event.kind == MouseKind::Move { 32 } else { 0 }
                        + if event.shift { 4 } else { 0 }
                        + if event.alt { 8 } else { 0 }
                        + if event.ctrl { 16 } else { 0 };
                    if modes.mouse_sgr {
                        pty.queue(
                            format!(
                                "\x1b[<{button};{};{}{}",
                                event.position.x + 1,
                                event.position.y + 1,
                                if event.kind == MouseKind::Release {
                                    'm'
                                } else {
                                    'M'
                                }
                            )
                            .as_bytes(),
                        )?;
                    } else {
                        return Err("legacy mouse encoding unsupported by stream client".into());
                    }
                }
            }
            Request::Resync { pane } => {
                let stream = self.clients.get_mut(&id).unwrap().stream.as_mut().unwrap();
                if !stream.panes.contains(&pane) {
                    return Err("pane is not subscribed".into());
                }
                stream.previous.remove(&pane);
            }
            Request::Acknowledge { notification } => {
                self.activity.acknowledge(notification)?;
            }
            Request::Hello { .. } => unreachable!(),
        }
        self.dirty = true;
        Ok(())
    }
    fn stream_frames(&mut self, vitals: super::vitals::Vitals) {
        use super::stream::{Event, Lease, PaneSnapshot, Update};
        self.leases
            .retain(|pane, _| self.panes.get(pane).is_some_and(|pty| !pty.dead));
        let mut removed = Vec::new();
        for (id, client) in &mut self.clients {
            let idle =
                client.frame.is_none() && client.writing.is_none() && client.output.is_empty();
            let Some(stream) = client.stream.as_mut() else {
                continue;
            };
            if stream
                .panes
                .iter()
                .filter_map(|pane| self.panes.get(pane))
                .map(|pane| usize::from(pane.size.0) * usize::from(pane.size.1))
                .sum::<usize>()
                > super::stream::MAX_SUBSCRIBED_CELLS
            {
                removed.push(*id);
                continue;
            }
            stream.panes.retain(|pane| self.panes.contains_key(pane));
            let mut update = Update {
                epoch: self.epoch.clone(),
                subscribed: stream.panes.clone(),
                snapshots: Vec::new(),
                patches: Vec::new(),
                panes: self.workspace.panes.clone(),
                leases: self
                    .leases
                    .iter()
                    .map(|(pane, lease)| Lease {
                        pane: *pane,
                        owner: lease.owner,
                    })
                    .collect(),
                notifications: self.activity.snapshots(),
                vitals: vitals.clone(),
            };
            stream.panes.retain(|pane| self.panes.contains_key(pane));
            stream
                .previous
                .retain(|pane, _| stream.panes.contains(pane));
            for pane in &stream.panes {
                let screen = self.panes[pane].terminal.snapshot();
                if idle
                    && stream
                        .previous
                        .get(pane)
                        .is_some_and(|old| old.screen == screen)
                {
                    continue;
                }
                let patch = if idle {
                    stream
                        .previous
                        .get(pane)
                        .and_then(|old| super::stream::diff(old, &screen))
                } else {
                    None
                };
                let revision = if let Some(patch) = patch {
                    let revision = patch.revision;
                    update.patches.push(patch);
                    revision
                } else {
                    update.snapshots.push(PaneSnapshot {
                        pane: *pane,
                        revision: 1,
                        screen: screen.clone(),
                    });
                    1
                };
                stream.previous.insert(
                    *pane,
                    PaneSnapshot {
                        pane: *pane,
                        revision,
                        screen,
                    },
                );
            }
            let metadata = Update {
                epoch: update.epoch.clone(),
                subscribed: update.subscribed.clone(),
                snapshots: Vec::new(),
                patches: Vec::new(),
                panes: update.panes.clone(),
                leases: update.leases.clone(),
                notifications: update.notifications.clone(),
                vitals: update.vitals.clone(),
            };
            if update.snapshots.is_empty()
                && update.patches.is_empty()
                && stream.metadata.as_ref() == Some(&metadata)
            {
                continue;
            }
            stream.metadata = Some(metadata);
            let packet = match wire(&ServerMessage::Stream(Event::Update(update))) {
                Ok(packet) => packet,
                Err(_) => {
                    removed.push(*id);
                    continue;
                }
            };
            if let Some(old) = client.frame.take() {
                client.queued = client.queued.saturating_sub(old.len());
            }
            if client.queued + packet.len() > 2 * MAX_QUEUED {
                removed.push(*id);
                continue;
            }
            client.queued += packet.len();
            client.frame = Some(packet);
        }
        for id in removed {
            self.remove_client(id);
        }
    }
    fn persist_geometry(&mut self) -> Result<()> {
        let path = self.config.state_dir.join("workspace.json");
        let mut settings = if path.exists() {
            super::lifecycle::Settings::load(&path)?
        } else {
            super::lifecycle::Settings::default()
        };
        settings.ui = self.workspace.ui.config.clone();
        settings.shell = Some(self.config.shell.clone());
        settings.cwd = Some(self.config.cwd.clone());
        settings.grid = (self.workspace.grid_cols, self.workspace.grid_rows);
        settings.grid_weights = [
            self.workspace.ui.column_weights.clone(),
            self.workspace.ui.row_weights.clone(),
        ];
        settings.template = self.config.template.clone();
        settings.tab_count = Some(
            self.workspace
                .panes
                .iter()
                .filter(|p| Some(p.id) != self.workspace.ui.scratch)
                .count()
                .max(1),
        );
        journal::atomic(&path, &settings)
    }
    fn poll_recovery(&mut self) {
        let result = self
            .recovery_job
            .as_ref()
            .and_then(|job| match job.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("recovery worker stopped".into()))
                }
            });
        if let Some(result) = result {
            match result {
                Ok(mut entries) => {
                    entries.truncate(256);
                    self.workspace.ui.recovery = entries;
                }
                Err(error) => self.notice = Some(format!("Recovery: {error}")),
            }
            self.workspace.ui.recovery_loading = false;
            self.recovery_job = None;
            self.dirty = true;
        }
    }
    fn feature_action(&mut self, client: u64, action: &Action) -> Result<bool> {
        match action {
            Action::StatusOpen(_)
            | Action::StatusView(_)
            | Action::StatusSort(_)
            | Action::StatusQuery(_)
            | Action::StatusScroll(_)
            | Action::ScrollTabs(_) => {
                self.workspace.apply(action)?;
                self.dirty = true;
                return Ok(true);
            }
            Action::Grid { .. } | Action::GridPicker | Action::ResizeGrid { .. } => {
                return Err("grid layout is automatic; use New Tab, Park or Close".into());
            }
            Action::NewTab => {
                // Spawn first; failure must leave every existing pane and view intact.
                self.spawn_pane(None)?;
                self.workspace.ui.tab_scroll_anchor = None;
                let id = self.workspace.panes.last().unwrap().id;
                if self.workspace.ui.scratch_visible {
                    self.workspace.apply(&Action::ScratchClose)?;
                }
                self.workspace.reflow();
                self.workspace.active = id;
                self.workspace.reading = ReadingMode::Tabs(id);
                self.workspace.ui.config.view_modes
                    [super::ui::geometry_bucket(self.workspace.cols)] = Some(true);
                let mut number = 0;
                for pane in &mut self.workspace.panes {
                    if Some(pane.id) == self.workspace.ui.scratch {
                        pane.number = 0;
                    } else {
                        number += 1;
                        pane.number = number;
                    }
                }
                self.persist_geometry()?;
            }
            Action::Scratch => {
                let dead = self
                    .workspace
                    .ui
                    .scratch
                    .is_some_and(|id| self.panes.get(&id).is_none_or(|pane| pane.dead));
                if dead {
                    if let Some(id) = self.workspace.ui.scratch.take() {
                        self.panes.remove(&id);
                        self.workspace.panes.retain(|pane| pane.id != id);
                    }
                    self.workspace.ui.scratch_visible = false;
                }
                if self.workspace.ui.scratch.is_none() {
                    let active = self.workspace.active;
                    self.spawn_pane(None)?;
                    let id = self
                        .workspace
                        .panes
                        .last()
                        .ok_or("scratch creation failed")?
                        .id;
                    self.workspace.ui.scratch = Some(id);
                    if let Some(pane) = self.workspace.panes.last_mut() {
                        pane.title = "Scratch terminal".into();
                    }
                    self.workspace.active = active;
                }
                self.workspace.apply(action)?;
            }
            Action::Acknowledge(id) => {
                self.activity.acknowledge(*id)?;
                self.workspace.ui.notifications = self.activity.all().to_vec();
            }
            Action::Recovery => {
                if self.recovery_job.is_none() {
                    let state = self.config.state_dir.clone();
                    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
                    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let worker_cancel = cancel.clone();
                    let worker = std::thread::spawn(move || {
                        let result = super::recovery::collect_cancellable(&state, &worker_cancel);
                        let _ = sender.send(result);
                    });
                    self.recovery_job = Some(RecoveryJob {
                        receiver,
                        cancel,
                        worker: Some(worker),
                    });
                    self.workspace.ui.recovery_loading = true;
                }
                self.workspace.apply(action)?;
            }
            Action::RecoveryCopy(index) => {
                let command = self
                    .workspace
                    .ui
                    .recovery
                    .get(*index)
                    .and_then(|item| item.entry.resume_command.clone())
                    .ok_or("this session has no known resume command")?;
                self.clients
                    .get_mut(&client)
                    .ok_or("unknown client")?
                    .send(ServerMessage::Clipboard(command))?;
            }
            Action::SearchCopy(index) => {
                let text = self
                    .workspace
                    .ui
                    .search_hits
                    .get(*index)
                    .ok_or("unknown search result")?
                    .text
                    .clone();
                self.clients
                    .get_mut(&client)
                    .ok_or("unknown client")?
                    .send(ServerMessage::Clipboard(text))?;
            }
            Action::Search { query, all } => {
                if query.len() > 256 || query.chars().any(char::is_control) {
                    return Err(
                        "search requires at most 256 bytes without control characters".into(),
                    );
                }
                let mut hits = Vec::new();
                if !query.is_empty() {
                    for info in &self.workspace.panes {
                        if !*all && info.id != self.workspace.active {
                            continue;
                        }
                        if let Some(pane) = self.panes.get(&info.id) {
                            for (line, text) in pane.terminal.search(query, 100 - hits.len()) {
                                hits.push(SearchHit {
                                    pane: info.id,
                                    line,
                                    text,
                                });
                            }
                        }
                        if hits.len() >= 100 {
                            break;
                        }
                    }
                }
                self.workspace.ui.search_query = query.clone();
                self.workspace.ui.search_all = *all;
                self.workspace.ui.search_hits = hits;
                if let Some(dialog) = self.workspace.ui.dialog.as_mut() {
                    if dialog.kind == super::ui::DialogKind::Search {
                        dialog.selection = 0;
                    }
                }
                self.workspace.apply(action)?;
            }
            Action::SearchSelect(index) => {
                let hit = self
                    .workspace
                    .ui
                    .search_hits
                    .get(*index)
                    .ok_or("unknown search result")?
                    .clone();
                self.panes
                    .get_mut(&hit.pane)
                    .ok_or("search pane has closed")?
                    .terminal
                    .scroll_to(hit.line)?;
                self.workspace.apply(&Action::Focus(hit.pane))?;
                self.workspace.apply(&Action::DialogClose)?;
            }
            Action::SelectionStart => {
                let pane = self
                    .panes
                    .get_mut(&self.workspace.active)
                    .ok_or("no active pane")?;
                let point = pane
                    .terminal
                    .snapshot()
                    .cursor
                    .map(|cursor| cursor.position)
                    .unwrap_or_default();
                pane.terminal.selection_start(point)?;
            }
            Action::SelectionMove { dx, dy } => {
                self.panes
                    .get_mut(&self.workspace.active)
                    .ok_or("no active pane")?
                    .terminal
                    .selection_move(i32::from(*dx), i32::from(*dy))?;
            }
            _ => return Ok(false),
        }
        self.persist_ui()?;
        self.resize();
        Ok(true)
    }
    fn close_pane(&mut self, id: PaneId) -> Result<()> {
        self.tui_write_allowed(id)?;
        let dialog = self
            .workspace
            .ui
            .dialog
            .ok_or("closing a pane requires confirmation")?;
        if dialog.kind != super::ui::DialogKind::CloseConfirm || dialog.source != Some(id) {
            return Err("closing a pane requires its confirmation dialog".into());
        }
        self.remove_pane(id, true)
    }
    fn remove_pane(&mut self, id: PaneId, replace_last: bool) -> Result<()> {
        if !self.panes.contains_key(&id) {
            return Err("unknown pane".into());
        }
        // Preserve an interactive terminal when the last tab is closed. Create
        // its replacement first so spawn failure cannot leave an empty workspace.
        let is_scratch = Some(id) == self.workspace.ui.scratch;
        let main_count = self
            .workspace
            .panes
            .iter()
            .filter(|pane| Some(pane.id) != self.workspace.ui.scratch)
            .count();
        if !is_scratch && main_count == 1 && (replace_last || self.workspace.ui.scratch.is_some()) {
            self.spawn_pane(None)?;
        }
        if is_scratch && self.workspace.ui.scratch_visible {
            self.workspace.apply(&Action::ScratchClose)?;
        }
        self.panes.remove(&id);
        self.workspace.panes.retain(|pane| pane.id != id);
        self.aliases.remove(&id);
        self.workspace.ui.config.pane_colors.remove(&id);
        self.workspace.ui.unused.retain(|pane| *pane != id);
        let fallback = self
            .workspace
            .panes
            .iter()
            .find(|pane| !pane.parked && Some(pane.id) != self.workspace.ui.scratch)
            .map(|pane| pane.id)
            .unwrap_or(0);
        if is_scratch {
            self.workspace.ui.scratch = None;
            self.workspace.ui.scratch_visible = false;
            self.workspace.ui.scratch_return = None;
            self.workspace.ui.scratch_reading = None;
        } else if self.workspace.ui.scratch_return == Some(id) {
            self.workspace.ui.scratch_return = Some(fallback);
        }
        if self.workspace.active == id {
            self.workspace.active = fallback;
        }
        fn replace(reading: ReadingMode, id: PaneId, fallback: PaneId) -> ReadingMode {
            match reading {
                ReadingMode::Tabs(pane) | ReadingMode::Capped(pane) if pane == id => {
                    ReadingMode::Tabs(fallback)
                }
                ReadingMode::Fullscreen(pane) if pane == id => ReadingMode::Fullscreen(fallback),
                other => other,
            }
        }
        self.workspace.reading = replace(self.workspace.reading, id, fallback);
        self.workspace.ui.scratch_reading = self
            .workspace
            .ui
            .scratch_reading
            .map(|reading| replace(reading, id, fallback));
        self.workspace.ui.dialog = None;
        if fallback == 0 {
            self.workspace.reading = ReadingMode::Grid;
        }
        if !replace_last && !is_scratch && main_count == 1 && self.workspace.ui.scratch.is_none() {
            self.shutdown = true;
            for client in self.clients.values_mut().filter(|client| client.attached) {
                let _ = client.send(ServerMessage::Detached);
            }
        }
        for (number, pane) in self.workspace.panes.iter_mut().enumerate() {
            pane.number = number + 1;
        }
        self.persist_ui()?;
        self.resize();
        self.persist_geometry()?;
        Ok(())
    }
    fn persist_ui(&mut self) -> Result<()> {
        if self.config.ui != self.workspace.ui.config {
            self.persist_geometry()?;
            self.config.ui = self.workspace.ui.config.clone();
        }
        Ok(())
    }
    fn add_pane(&mut self) -> Result<()> {
        let index = self
            .workspace
            .panes
            .iter()
            .filter(|pane| Some(pane.id) != self.workspace.ui.scratch)
            .count();
        let specification = self
            .config
            .template
            .as_ref()
            .and_then(|template| template.panes.get(index))
            .filter(|_| self.initializing)
            .cloned();
        self.spawn_pane(specification)
    }
    fn spawn_pane(&mut self, specification: Option<super::templates::TemplatePane>) -> Result<()> {
        let id = self.next_pane;
        let original_cwd = self.config.cwd.clone();
        let original_shell = self.config.shell.clone();
        if let Some(specification) = &specification {
            self.config.cwd = specification.cwd.clone();
            if let Some(shell) = &specification.shell {
                self.config.shell = shell.clone();
            }
        }
        let pane = NativePane::spawn(
            &self.config,
            self.workspace.cols,
            self.workspace.rows.saturating_sub(2),
            id,
        );
        self.config.cwd = original_cwd;
        self.config.shell = original_shell;
        let mut pane = pane?;
        pane.terminal.set_theme(self.terminal_theme());
        let cwd = specification
            .as_ref()
            .map(|specification| specification.cwd.clone())
            .unwrap_or_else(|| self.config.cwd.clone());
        let title = specification
            .as_ref()
            .map(|specification| specification.name.clone())
            .unwrap_or_else(|| "shell".into());
        if specification.is_some() {
            self.aliases.insert(id, title.clone());
        }
        self.workspace.panes.push(PaneInfo {
            parked: false,
            id,
            number: self.workspace.panes.len() + 1,
            pid: pane.child.id(),
            cwd: cwd.to_string_lossy().into_owned(),
            title,
            palette: self
                .workspace
                .ui
                .config
                .pane_colors
                .get(&id)
                .copied()
                .unwrap_or(((id - 1) % 9) as u8),
            status: Status::Unknown,
            session_id: None,
            dead: false,
            kind: None,
            last_activity: crate::now(),
            status_since: crate::now(),
            observation: crate::model::Observation::default(),
        });
        self.panes.insert(id, pane);
        self.next_pane += 1;
        if self.workspace.active == 0 {
            self.workspace.active = id;
        }
        Ok(())
    }
    fn resize(&mut self) {
        self.workspace.reflow();
        for (id, cols, rows) in self.workspace.terminal_sizes() {
            if let Some(pane) = self.panes.get_mut(&id) {
                let (cols, rows) = self
                    .leases
                    .get(&id)
                    .map(|lease| lease.size)
                    .unwrap_or((cols, rows));
                pane.resize(cols.max(1), rows.max(1));
            }
        }
        self.dirty = true;
    }
    fn message(&mut self, id: u64, message: ClientMessage) -> Result<()> {
        if let ClientMessage::Stream(request) = message {
            let result = self.stream_request(id, request);
            if let Err(error) = result {
                self.clients
                    .get_mut(&id)
                    .unwrap()
                    .send(ServerMessage::Stream(super::stream::Event::Error {
                        message: error.to_string(),
                    }))?;
            }
            return Ok(());
        }
        if self.clients[&id].stream.is_some() {
            return Err("stream peers may only send stream requests".into());
        }
        match message {
            ClientMessage::Diagnostics => {
                let report = super::diagnostics::Report {
                    version: env!("CARGO_PKG_VERSION").into(),
                    protocol: VERSION,
                    cols: self.workspace.cols,
                    rows: self.workspace.rows,
                    grid: (self.workspace.grid_cols, self.workspace.grid_rows),
                    panes: self.panes.len(),
                    clients: self.clients.len(),
                    uptime_seconds: self.started.elapsed().as_secs(),
                    pty_bytes: self.pty_bytes,
                    full_frames: self.clients.values().map(|client| client.full_frames).sum(),
                    patch_frames: self
                        .clients
                        .values()
                        .map(|client| client.patch_frames)
                        .sum(),
                    full_bytes: self.clients.values().map(|client| client.full_bytes).sum(),
                    patch_bytes: self.clients.values().map(|client| client.patch_bytes).sum(),
                };
                self.clients
                    .get_mut(&id)
                    .unwrap()
                    .send(ServerMessage::Diagnostics(report))?;
            }
            ClientMessage::History { pane, start, limit } => {
                if limit > 200 {
                    return Err("history page exceeds 200 lines".into());
                }
                let lines = self
                    .panes
                    .get(&pane)
                    .ok_or("unknown pane")?
                    .terminal
                    .history(start, limit);
                self.clients
                    .get_mut(&id)
                    .unwrap()
                    .send(ServerMessage::History { pane, lines })?;
            }
            ClientMessage::InspectPane { pane } => {
                let info = self
                    .workspace
                    .panes
                    .iter()
                    .find(|info| info.id == pane)
                    .ok_or("unknown pane")?
                    .clone();
                let screen = self
                    .panes
                    .get(&pane)
                    .ok_or("unknown pane")?
                    .terminal
                    .snapshot();
                self.clients
                    .get_mut(&id)
                    .unwrap()
                    .send(ServerMessage::PaneState { pane: info, screen })?;
            }
            ClientMessage::Inspect => {
                self.clients
                    .get_mut(&id)
                    .unwrap()
                    .send(ServerMessage::State {
                        panes: self.workspace.panes.clone(),
                        active: self.workspace.active,
                        reading: self.workspace.reading,
                        controller: self.controller,
                    })?
            }
            ClientMessage::Ping => self
                .clients
                .get_mut(&id)
                .unwrap()
                .send(ServerMessage::Pong)?,
            ClientMessage::Command(action) => {
                if self.clients[&id].attached && self.controller != Some(id) {
                    return Err(
                        "observers cannot issue management commands; take control explicitly"
                            .into(),
                    );
                }
                match &action {
                    Action::Scroll { pane, .. } | Action::SelectText { pane, .. } => {
                        self.tui_write_allowed(*pane)?
                    }
                    Action::SelectionStart | Action::SelectionMove { .. } => {
                        self.tui_write_allowed(self.workspace.active)?
                    }
                    Action::SearchSelect(index) => {
                        if let Some(hit) = self.workspace.ui.search_hits.get(*index) {
                            self.tui_write_allowed(hit.pane)?;
                        }
                    }
                    _ => {}
                }
                if !self.feature_action(id, &action)? {
                    match action {
                        Action::ClaimPane(pane) => {
                            if !self.panes.contains_key(&pane) {
                                return Err("unknown pane".into());
                            }
                            self.leases.remove(&pane);
                            self.resize();
                        }
                        Action::Shutdown => self.shutdown = true,
                        Action::Grid { .. } => return Err("grid layout is automatic".into()),
                        Action::ClosePaneConfirm(pane) => self.close_pane(pane)?,
                        Action::Focus(_)
                        | Action::Read(_)
                        | Action::Fullscreen(_)
                        | Action::CloseReading
                        | Action::Swap { .. }
                        | Action::Help => {
                            self.workspace.apply(&action)?;
                            self.persist_ui()?;
                            self.resize();
                        }
                        Action::ReadResize { .. }
                        | Action::Config
                        | Action::TabPosition
                        | Action::TabPositionSet(_)
                        | Action::ClosePaneRequest(_)
                        | Action::FocusDirection { .. }
                        | Action::GridPicker
                        | Action::Agents(_)
                        | Action::MovePicker
                        | Action::DialogNavigate { .. }
                        | Action::DialogPick(_)
                        | Action::DialogClose => {
                            self.workspace.apply(&action)?;
                            self.persist_ui()?;
                            self.resize();
                        }
                        other => {
                            self.workspace.apply(&other)?;
                            if matches!(other, Action::ResizeGrid { .. }) {
                                self.persist_geometry()?;
                            }
                            self.persist_ui()?;
                            self.resize();
                        }
                    }
                }
                self.clients
                    .get_mut(&id)
                    .unwrap()
                    .send(ServerMessage::State {
                        panes: self.workspace.panes.clone(),
                        active: self.workspace.active,
                        reading: self.workspace.reading,
                        controller: self.controller,
                    })?;
            }
            ClientMessage::ThemeColor { background, rgb } if self.clients[&id].attached => {
                self.clients.get_mut(&id).unwrap().theme[usize::from(background)] = Some(rgb);
                self.dirty = true;
                if self.controller == Some(id) {
                    self.apply_theme();
                }
            }
            ClientMessage::Attach {
                version,
                cols,
                rows,
                takeover,
            } => {
                if version != VERSION {
                    return Err("unsupported native protocol version".into());
                }
                validate_size(cols, rows)?;
                if self.clients[&id].attached {
                    return Err("client already attached".into());
                }
                self.clients.get_mut(&id).unwrap().attached = true;
                self.clients.get_mut(&id).unwrap().geometry = Some((cols, rows));
                if self.controller.is_none() || takeover {
                    self.controller = Some(id);
                    self.apply_theme();
                    self.workspace.resize(cols, rows)?;
                    self.resize();
                }
                self.clients
                    .get_mut(&id)
                    .unwrap()
                    .send(ServerMessage::Welcome {
                        version: VERSION,
                        client_id: id,
                        controller: self.controller == Some(id),
                    })?;
                self.dirty = true;
            }
            ClientMessage::Action(Action::Detach) => {
                self.clients.get_mut(&id).unwrap().attached = false;
                if self.controller == Some(id) {
                    self.controller = None;
                }
                self.clients
                    .get_mut(&id)
                    .unwrap()
                    .send(ServerMessage::Detached)?;
                self.dirty = true;
            }
            ClientMessage::Action(Action::Takeover) if self.clients[&id].attached => {
                let (cols, rows) = self.clients[&id]
                    .geometry
                    .ok_or("attached client has no geometry")?;
                self.controller = Some(id);
                self.apply_theme();
                self.workspace.resize(cols, rows)?;
                self.resize();
            }
            ClientMessage::Resize { cols, rows } if self.clients[&id].attached => {
                validate_size(cols, rows)?;
                self.clients.get_mut(&id).unwrap().geometry = Some((cols, rows));
                if self.controller == Some(id) {
                    self.workspace.resize(cols, rows)?;
                    self.resize();
                }
            }
            message => {
                if self.controller != Some(id) || !self.clients[&id].attached {
                    return Err("observer input refused; take control explicitly".into());
                }
                match message {
                    ClientMessage::Resize { cols, rows } => {
                        self.workspace.resize(cols, rows)?;
                        self.resize();
                    }
                    ClientMessage::Input(bytes) => {
                        self.tui_write_allowed(self.workspace.active)?;
                        if bytes.len() > MAX_INPUT {
                            return Err("input too large".into());
                        }
                        self.panes
                            .get_mut(&self.workspace.active)
                            .ok_or("no active pane")?
                            .queue(&bytes)?;
                        if let Some(info) = self
                            .workspace
                            .panes
                            .iter_mut()
                            .find(|info| info.id == self.workspace.active)
                        {
                            info.last_activity = crate::now();
                        }
                        self.dirty = true;
                    }
                    ClientMessage::Paste(bytes) => {
                        self.tui_write_allowed(self.workspace.active)?;
                        if bytes.len() > MAX_INPUT {
                            return Err("paste too large".into());
                        }
                        let pane = self
                            .panes
                            .get_mut(&self.workspace.active)
                            .ok_or("no active pane")?;
                        let data = if pane.terminal.snapshot().modes.bracketed_paste {
                            [
                                b"\x1b[200~".as_slice(),
                                bytes.as_slice(),
                                b"\x1b[201~".as_slice(),
                            ]
                            .concat()
                        } else {
                            bytes
                        };
                        pane.queue(&data)?;
                        if let Some(info) = self
                            .workspace
                            .panes
                            .iter_mut()
                            .find(|info| info.id == self.workspace.active)
                        {
                            info.last_activity = crate::now();
                        }
                        self.dirty = true;
                    }
                    ClientMessage::Mouse(mouse) => self.mouse(mouse)?,
                    ClientMessage::Action(action) => {
                        if let Action::ClaimPane(pane) = action {
                            if !self.panes.contains_key(&pane) {
                                return Err("unknown pane".into());
                            }
                            self.leases.remove(&pane);
                            self.resize();
                            return Ok(());
                        }
                        match &action {
                            Action::Scroll { pane, .. } | Action::SelectText { pane, .. } => {
                                self.tui_write_allowed(*pane)?
                            }
                            Action::SelectionStart | Action::SelectionMove { .. } => {
                                self.tui_write_allowed(self.workspace.active)?
                            }
                            Action::SearchSelect(index) => {
                                if let Some(hit) = self.workspace.ui.search_hits.get(*index) {
                                    self.tui_write_allowed(hit.pane)?;
                                }
                            }
                            _ => {}
                        }
                        let geometry = matches!(action, Action::ResizeGrid { .. });
                        if !self.feature_action(id, &action)? {
                            match action {
                                Action::Shutdown => self.shutdown = true,
                                Action::Grid { .. } => {
                                    return Err("grid layout is automatic".into());
                                }
                                Action::ClosePaneConfirm(pane) => self.close_pane(pane)?,
                                Action::Scroll { pane, lines } => self
                                    .panes
                                    .get_mut(&pane)
                                    .ok_or("unknown pane")?
                                    .terminal
                                    .scroll(lines.clamp(-10000, 10000)),
                                Action::SelectText { pane, start, end } => {
                                    self.panes
                                        .get_mut(&pane)
                                        .ok_or("unknown pane")?
                                        .terminal
                                        .select(start, end)?;
                                }
                                Action::Copy(pane) => {
                                    let text = self
                                        .panes
                                        .get(&pane)
                                        .ok_or("unknown pane")?
                                        .terminal
                                        .selection()
                                        .unwrap_or_default();
                                    self.clients
                                        .get_mut(&id)
                                        .unwrap()
                                        .send(ServerMessage::Clipboard(text))?;
                                }
                                other => self.workspace.apply(&other)?,
                            }
                        }
                        self.persist_ui()?;
                        if geometry {
                            self.persist_geometry()?;
                        }
                        self.resize();
                    }
                    _ => return Err("invalid native client message".into()),
                }
            }
        }
        self.persist_notification_reads()?;
        Ok(())
    }
    fn persist_notification_reads(&mut self) -> Result<()> {
        let ids: Vec<_> = self
            .workspace
            .ui
            .notifications
            .iter()
            .filter(|event| {
                event.read
                    && self
                        .activity
                        .all()
                        .iter()
                        .any(|saved| saved.id == event.id && !saved.read)
            })
            .map(|event| event.id)
            .collect();
        for id in ids {
            self.activity.acknowledge(id)?;
        }
        Ok(())
    }
    fn mouse(&mut self, mouse: MouseEvent) -> Result<()> {
        if self.focus_click && matches!(mouse.kind, MouseKind::Move | MouseKind::Release) {
            if mouse.kind == MouseKind::Release {
                self.focus_click = false;
            }
            return Ok(());
        }
        if mouse.kind == MouseKind::Press {
            self.focus_click = false;
        }
        let Some(view) = self
            .workspace
            .layout()
            .into_iter()
            .rev()
            .find(|v| v.rect.contains(mouse.position))
        else {
            return Ok(());
        };
        self.tui_write_allowed(view.pane.id)?;
        if mouse.kind == MouseKind::Press {
            let focus_only = self.workspace.reading == ReadingMode::Grid
                && self.workspace.active != view.pane.id;
            self.workspace.apply(&Action::Focus(view.pane.id))?;
            self.resize();
            if focus_only {
                self.focus_click = true;
                return Ok(());
            }
        }
        if !view.body.contains(mouse.position) {
            return Ok(());
        }
        let pane = self
            .panes
            .get_mut(&view.pane.id)
            .ok_or("unknown mouse pane")?;
        let modes = pane.terminal.snapshot().modes;
        if !mouse.shift && (modes.mouse_click || modes.mouse_drag || modes.mouse_motion) {
            if let Some(info) = self
                .workspace
                .panes
                .iter_mut()
                .find(|info| info.id == view.pane.id)
            {
                info.last_activity = crate::now();
            }
            self.dirty = true;
            if mouse.kind == MouseKind::Move
                && !modes.mouse_motion
                && !(modes.mouse_drag && mouse.button < 3)
            {
                return Ok(());
            }
            let mut button = match mouse.kind {
                MouseKind::WheelUp => 64,
                MouseKind::WheelDown => 65,
                _ => mouse.button.min(3) as u16,
            };
            if mouse.kind == MouseKind::Move {
                button += 32;
            }
            button += if mouse.shift { 4 } else { 0 }
                + if mouse.alt { 8 } else { 0 }
                + if mouse.ctrl { 16 } else { 0 };
            let x = mouse.position.x - view.body.x + 1;
            let y = mouse.position.y - view.body.y + 1;
            if modes.mouse_sgr {
                pane.queue(
                    format!(
                        "\x1b[<{button};{x};{y}{}",
                        if mouse.kind == MouseKind::Release {
                            'm'
                        } else {
                            'M'
                        }
                    )
                    .as_bytes(),
                )?;
            } else if x <= 223 && y <= 223 {
                if mouse.kind == MouseKind::Release {
                    button = 3;
                }
                pane.queue(&[
                    27,
                    b'[',
                    b'M',
                    button as u8 + 32,
                    x as u8 + 32,
                    y as u8 + 32,
                ])?;
            }
        } else if matches!(mouse.kind, MouseKind::WheelUp | MouseKind::WheelDown) {
            pane.terminal.scroll(if mouse.kind == MouseKind::WheelUp {
                3
            } else {
                -3
            });
            self.dirty = true;
        }
        Ok(())
    }
    fn poll_ptys(&mut self) {
        let mut clipboard = vec![];
        let mut exited = vec![];
        for (id, pane) in &mut self.panes {
            let mut buf = [0; 8192];
            for _ in 0..16 {
                match pane.master.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        self.pty_bytes = self.pty_bytes.saturating_add(n as u64);
                        self.dirty = true;
                        if let Some(info) =
                            self.workspace.panes.iter_mut().find(|info| info.id == *id)
                        {
                            info.last_activity = crate::now();
                        }
                        for effect in pane.terminal.feed(&buf[..n]) {
                            match effect {
                                Effect::WritePty(bytes) => {
                                    let _ = pane.queue(&bytes);
                                }
                                Effect::Title(title) => {
                                    pane.terminal_title = (!title.is_empty())
                                        .then(|| title.chars().take(1024).collect());
                                    // Discovery owns an agent tab's name; OSC cannot race it.
                                    if pane.agent_named {
                                        continue;
                                    }
                                    if let Some(info) =
                                        self.workspace.panes.iter_mut().find(|p| p.id == *id)
                                    {
                                        let title = if info.kind.is_some() {
                                            super::tracking::agent_terminal_title(&title, &info.cwd)
                                        } else {
                                            title.as_str()
                                        };
                                        if let Some(kind) = info.kind {
                                            let folder = Path::new(&info.cwd)
                                                .file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy();
                                            if title.trim().is_empty()
                                                || title == folder.as_ref()
                                                || title == info.cwd
                                                || title == "shell"
                                                || title == kind.as_str()
                                            {
                                                continue;
                                            }
                                        }
                                        info.title = if info.kind.is_none() {
                                            super::tracking::shell_tab_name(
                                                Some(title),
                                                self.aliases.get(id).map(String::as_str),
                                                info.status == Status::Idle,
                                            )
                                        } else {
                                            self.aliases.get(id).cloned().unwrap_or_else(|| {
                                                title.chars().take(1024).collect()
                                            })
                                        };
                                    }
                                }
                                Effect::Clipboard(text) if text.len() <= MAX_INPUT => {
                                    clipboard.push(text)
                                }
                                Effect::Bell => {
                                    if let Some(info) =
                                        self.workspace.panes.iter().find(|info| info.id == *id)
                                    {
                                        if let Err(error) = self.activity.bell(info, crate::now()) {
                                            self.notice =
                                                Some(format!("Notification history: {error}"));
                                        }
                                    }
                                }
                                _ => (),
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            if !pane.pending.is_empty() {
                let (front, _) = pane.pending.as_slices();
                match pane.master.write(front) {
                    Ok(n) => {
                        pane.pending.drain(..n);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => (),
                    Err(_) => {
                        pane.pending.clear();
                    }
                }
            }
            if !pane.dead && pane.child.try_wait().ok().flatten().is_some() {
                exited.push(*id);
                pane.dead = true;
                pane.pending.clear();
                if let Some(info) = self.workspace.panes.iter_mut().find(|p| p.id == *id) {
                    info.dead = true;
                    info.status = Status::Exited;
                    info.status_since = crate::now();
                    info.observation = crate::model::Observation {
                        source: crate::model::StatusSource::Process,
                        checked_at: crate::now(),
                        event_at: Some(crate::now()),
                    };
                }
                self.dirty = true;
            }
        }
        for id in exited {
            if let Err(error) = self.remove_pane(id, false) {
                self.notice = Some(format!("Close exited tab: {error}"));
            }
        }
        if let Some(client) = self.controller.and_then(|id| self.clients.get_mut(&id)) {
            for text in clipboard {
                let _ = client.send(ServerMessage::Clipboard(text));
            }
        }
    }
    fn discovery_snapshot(&self) -> DiscoveryRequest {
        let bridge: Vec<_> = self
            .workspace
            .panes
            .iter()
            .map(|p| crate::tmux::Pane {
                id: format!("%{}", p.id),
                window: "@0".into(),
                pid: p.pid,
                index: p.number,
                cwd: p.cwd.clone(),
                dead: p.dead,
                floating: false,
            })
            .collect();
        DiscoveryRequest {
            bridge,
            identities: self
                .panes
                .iter()
                .map(|(id, p)| (*id, (p.child.id(), p.start)))
                .collect(),
        }
    }
    fn discovery_result(&mut self, result: DiscoveryResult) {
        let DiscoveryResult {
            identities,
            agents: reconciled,
            tree,
        } = result;
        for info in &mut self.workspace.panes {
            if !self
                .panes
                .get(&info.id)
                .is_some_and(|p| identities.get(&info.id) == Some(&(p.child.id(), p.start)))
            {
                continue;
            }
            if !info.dead
                && !process::read(Path::new("/proc"), info.pid).is_ok_and(|p| {
                    identities
                        .get(&info.id)
                        .is_some_and(|(_, start)| *start == p.start)
                })
            {
                continue;
            }
            let before = info.clone();
            if let Ok(cwd) = fs::read_link(format!("/proc/{}/cwd", info.pid)) {
                info.cwd = cwd.to_string_lossy().into_owned();
            }
            if info.dead {
                info.status = Status::Exited;
            } else if let Some(agent) = reconciled.iter().find(|a| {
                a.pane == format!("%{}", info.id)
                    && process::read(Path::new("/proc"), a.pid)
                        .is_ok_and(|p| p.start == a.process_start)
            }) {
                info.status = agent.status;
                info.observation = agent.observation.clone();
                info.kind = Some(agent.kind);
                info.session_id = agent.session_id.clone();
                if let Some(pane) = self.panes.get_mut(&info.id) {
                    let (name, authoritative) = super::tracking::tab_name(
                        agent,
                        pane.terminal_title.as_deref(),
                        self.aliases.get(&info.id).map(String::as_str),
                    );
                    info.title = name;
                    pane.agent_named = authoritative;
                }
            } else {
                if let Some(pane) = self.panes.get_mut(&info.id) {
                    if std::mem::take(&mut pane.agent_named) || before.kind.is_some() {
                        pane.terminal_title = None;
                    }
                }
                info.session_id = None;
                info.kind = None;
                info.status = if process::idle(&tree, info.pid, false) {
                    Status::Idle
                } else if tree.contains_key(&info.pid) {
                    Status::Working
                } else {
                    Status::Unknown
                };
                info.title = super::tracking::shell_tab_name(
                    self.panes
                        .get(&info.id)
                        .and_then(|pane| pane.terminal_title.as_deref()),
                    self.aliases.get(&info.id).map(String::as_str),
                    info.status == Status::Idle,
                );
                info.observation = crate::model::Observation::checked(
                    crate::model::StatusSource::Process,
                    crate::now(),
                );
            }
            if before.status != info.status {
                info.status_since = crate::now();
            }
            self.dirty |= before != *info;
        }
    }
    fn frames(&mut self, spinner: usize) {
        if let Err(error) = self.activity.observe(&self.workspace.panes, crate::now()) {
            self.notice = Some(format!("Notification history: {error}"));
        }
        self.workspace.ui.notifications = self.activity.all().to_vec();
        let attached = self.clients.values().any(|client| client.attached);
        let streaming = self.clients.values().any(|client| client.stream.is_some());
        // Detached shells still drain PTYs and record notifications, but have
        // no display or metrics consumer. Do not snapshot their screens.
        if !attached && !streaming {
            self.status.poll(false);
            self.dirty = false;
            return;
        }
        let vitals = self.vitals.sample();
        let status_open = self
            .workspace
            .ui
            .dialog
            .is_some_and(|dialog| dialog.kind == super::ui::DialogKind::Status)
            && self.clients.values().any(|client| client.attached);
        self.workspace.ui.status = if status_open {
            self.status.poll(true).clone()
        } else {
            self.status.poll(false);
            super::status::Snapshot::default()
        };
        self.stream_frames(vitals.clone());
        if !attached {
            self.dirty = false;
            return;
        }
        self.workspace.ui.vitals = if self.workspace.cols <= 60 {
            let mut compact = Vec::new();
            if let Some(cpu) = vitals.cpu {
                compact.push((format!("C{cpu}"), 114));
            }
            if let Some(memory) = vitals.memory {
                compact.push((format!("M{memory}"), 141));
            }
            if let Some(time) = vitals.time {
                compact.push((time, 250));
            }
            compact
        } else {
            super::vitals::render_segments(&vitals)
        };
        let screens = self
            .panes
            .iter()
            .map(|(id, pane)| (*id, pane.terminal.snapshot()))
            .collect();
        let mut presentations = BTreeMap::new();
        let mut removed = vec![];
        for (id, client) in &mut self.clients {
            if client.attached {
                let controller = self.controller == Some(*id);
                let frame = presentations
                    .entry((client.theme, controller))
                    .or_insert_with(|| {
                        let mut workspace = self.workspace.clone();
                        workspace.ui.terminal_theme = client.theme;
                        render::compose(
                            &workspace,
                            &screens,
                            controller,
                            self.notice.as_deref(),
                            spinner,
                        )
                    });
                if client.latest(frame).is_err() {
                    removed.push(*id);
                }
            }
        }
        for id in removed {
            self.remove_client(id);
        }
        self.dirty = false;
    }
    fn remove_client(&mut self, id: u64) {
        self.clients.remove(&id);
        let before = self.leases.len();
        self.leases.retain(|_, lease| lease.owner != id);
        if before != self.leases.len() {
            self.resize();
        }
        if self.controller == Some(id) {
            self.controller = None;
            self.dirty = true;
        }
    }
}

struct DiscoveryRequest {
    bridge: Vec<crate::tmux::Pane>,
    identities: BTreeMap<PaneId, (u32, u64)>,
}
struct DiscoveryResult {
    identities: BTreeMap<PaneId, (u32, u64)>,
    agents: Vec<crate::model::Agent>,
    tree: BTreeMap<u32, process::Process>,
}
fn discovery_worker(
    state: PathBuf,
) -> (
    std::sync::mpsc::SyncSender<DiscoveryRequest>,
    std::sync::mpsc::Receiver<DiscoveryResult>,
    std::thread::JoinHandle<()>,
) {
    let (requests, receiver) = std::sync::mpsc::sync_channel::<DiscoveryRequest>(1);
    let (sender, results) = std::sync::mpsc::sync_channel(1);
    let handle = std::thread::spawn(move || {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let discovery = Discovery::new(&home);
        while let Ok(request) = receiver.recv() {
            let Ok(agents) = discovery.scan(&request.bridge, crate::now()) else {
                continue;
            };
            // A registry client may hold the lock. Skip that journal cycle rather than wait forever.
            let root = state.join("native");
            let agents = if journal::private_dir(&root).is_ok() {
                if let Ok(mut store) = journal::Store::open_nonblocking(&root) {
                    if store.reconcile(agents.clone(), crate::now()).is_ok() {
                        store
                            .registry
                            .agents
                            .values()
                            .filter(|a| a.ended_at.is_none())
                            .cloned()
                            .collect()
                    } else {
                        agents
                    }
                } else {
                    agents
                }
            } else {
                agents
            };
            let result = DiscoveryResult {
                identities: request.identities,
                agents,
                tree: process::snapshot(Path::new("/proc")).unwrap_or_default(),
            };
            if sender.send(result).is_err() {
                break;
            }
        }
    });
    (requests, results, handle)
}

pub fn serve(config: ServerConfig) -> Result<()> {
    if let Some(template) = &config.template {
        template.validate()?;
    }
    validate_size(config.cols, config.rows)?;
    config.ui.validate()?;
    Workspace::grid_count(config.initial_grid.0, config.initial_grid.1)?;
    let mut workspace = Workspace::new(config.cols, config.rows)?;
    workspace.ui.config = config.ui.clone();
    let (listener, _endpoint) = endpoint(&config)?;
    super::lifecycle::save_pid(&config.state_dir)?;
    let mut server = Server {
        focus_click: false,
        title_animation: BTreeMap::new(),
        epoch: fs::read_to_string("/proc/sys/kernel/random/uuid")?
            .trim()
            .to_owned(),
        leases: BTreeMap::new(),
        initializing: true,
        recovery_job: None,
        aliases: BTreeMap::new(),
        activity: super::activity::Activity::load(
            &config.state_dir,
            config
                .state_dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("workspace"),
        )?,
        started: Instant::now(),
        pty_bytes: 0,
        vitals: super::vitals::Sampler::default(),
        status: super::status::Monitor::default(),
        config,
        workspace,
        panes: BTreeMap::new(),
        clients: BTreeMap::new(),
        controller: None,
        next_pane: 1,
        next_client: 1,
        dirty: true,
        notice: None,
        shutdown: false,
    };
    let count = server.config.initial_tabs.unwrap_or(
        usize::from(server.config.initial_grid.0) * usize::from(server.config.initial_grid.1),
    );
    for _ in 0..count {
        server.add_pane()?;
    }
    server.initializing = false;
    server.workspace.reflow();
    server.persist_geometry()?;
    server.resize();
    let (requests, results, worker) = discovery_worker(server.config.state_dir.clone());
    let mut discovery = Instant::now() - Duration::from_secs(2);
    let mut render_time = Instant::now() - Duration::from_secs(1);
    let mut metrics_time = Instant::now();
    let mut animation = Instant::now();
    let mut spinner = 0;
    let mut idle_poll = Instant::now() - Duration::from_millis(100);
    let mut unused_poll = Instant::now() - Duration::from_secs(1);
    while !server.shutdown {
        for _ in 0..MAX_CLIENTS {
            match listener.accept() {
                Ok((socket, _)) if server.clients.len() < MAX_CLIENTS => {
                    if let Ok(client) = Client::new(socket) {
                        let id = server.next_client;
                        server.next_client += 1;
                        server.clients.insert(id, client);
                    }
                }
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }
        let ids: Vec<_> = server.clients.keys().copied().collect();
        for id in ids {
            let messages = match server.clients.get_mut(&id).unwrap().receive() {
                Ok(m) => m,
                Err(_) => {
                    server.remove_client(id);
                    continue;
                }
            };
            for message in messages {
                if let Err(e) = server.message(id, message) {
                    if server
                        .clients
                        .get_mut(&id)
                        .unwrap()
                        .send(ServerMessage::Error(e.to_string()))
                        .is_err()
                    {
                        server.remove_client(id);
                        break;
                    }
                }
            }
        }
        server.poll_ptys();
        server.poll_recovery();
        let attached = server.clients.values().any(|client| client.attached);
        let streaming = server
            .clients
            .values()
            .any(|client| client.stream.is_some());
        let idle_interval = if attached {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(1)
        };
        if idle_poll.elapsed() >= idle_interval {
            for pane in server.panes.values_mut() {
                if !pane.dead {
                    pane.observe_idle();
                }
            }
            idle_poll = Instant::now();
        }
        // This process-table scan only supplies the attached UI's unused-shell
        // hints. Never scan the whole host ten times a second per workspace.
        if attached && unused_poll.elapsed() >= Duration::from_millis(500) {
            if let Ok(tree) = process::snapshot(Path::new("/proc")) {
                let unused: Vec<_> = server
                    .workspace
                    .panes
                    .iter()
                    .filter(|info| {
                        !server.leases.contains_key(&info.id)
                            && server
                                .panes
                                .get(&info.id)
                                .is_some_and(|pane| pane.unused(&tree))
                    })
                    .map(|info| info.id)
                    .collect();
                if unused != server.workspace.ui.unused {
                    server.workspace.ui.unused = unused;
                    server.dirty = true;
                }
            }
            unused_poll = Instant::now();
        }
        if discovery.elapsed() >= Duration::from_secs(2) {
            let _ = requests.try_send(server.discovery_snapshot());
            discovery = Instant::now();
        }
        if let Ok(result) = results.try_recv() {
            server.discovery_result(result);
        }
        if attached && animation.elapsed() >= Duration::from_millis(125) {
            spinner += 1;
            animation = Instant::now();
            server
                .title_animation
                .retain(|id, _| server.panes.contains_key(id));
            server
                .workspace
                .ui
                .title_ticks
                .retain(|id, _| server.panes.contains_key(id));
            for pane in &server.workspace.panes {
                let entry = server
                    .title_animation
                    .entry(pane.id)
                    .or_insert_with(|| (pane.title.clone(), 0));
                if entry.0 != pane.title {
                    *entry = (pane.title.clone(), 0);
                }
                let stop = (pane.title.chars().count().min(512) as u16) * 3 + 32;
                if entry.1 <= stop {
                    entry.1 += 1;
                    server.workspace.ui.title_ticks.insert(pane.id, entry.1);
                    server.dirty = true;
                }
            }
            server.dirty |= server
                .workspace
                .panes
                .iter()
                .any(|p| p.status == Status::Working);
        }
        if (attached || streaming) && metrics_time.elapsed() >= Duration::from_secs(1) {
            server.dirty = true;
            metrics_time = Instant::now();
        }
        if server.dirty && render_time.elapsed() >= Duration::from_millis(33) {
            server.frames(spinner);
            render_time = Instant::now();
        }
        let ids: Vec<_> = server.clients.keys().copied().collect();
        for id in ids {
            if server.clients.get_mut(&id).unwrap().flush().is_err() {
                server.remove_client(id);
            }
        }
        // poll yields without blocking a disconnected observer or PTY output pump.
        let mut fds = vec![libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        for pane in server.panes.values() {
            if !pane.dead {
                fds.push(libc::pollfd {
                    fd: pane.master.as_raw_fd(),
                    events: libc::POLLIN
                        | if pane.pending.is_empty() {
                            0
                        } else {
                            libc::POLLOUT
                        },
                    revents: 0,
                });
            }
        }
        for client in server.clients.values() {
            fds.push(libc::pollfd {
                fd: client.socket.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
        }
        unsafe {
            libc::poll(
                fds.as_mut_ptr(),
                fds.len() as libc::nfds_t,
                if attached || streaming { 10 } else { 100 },
            );
        }
    }
    drop(requests);
    drop(results);
    let _ = worker.join();
    server.panes.clear();
    let _ = server.activity.observe(&[], crate::now());
    server.recovery_job = None;
    if let Ok(mut store) = journal::Store::open_nonblocking(&server.config.state_dir.join("native"))
    {
        let _ = store.reconcile(vec![], crate::now());
    }
    Ok(())
}
