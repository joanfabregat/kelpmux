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
    pub socket: PathBuf,
    pub state_dir: PathBuf,
    pub shell: PathBuf,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
    pub ui: super::ui::UiConfig,
    pub initial_grid: (u8, u8),
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
    child: Child,
    master: File,
    start: u64,
    terminal: Terminal,
    pending: VecDeque<u8>,
    dead: bool,
    last_input: Option<Instant>,
    idle_observations: u8,
    agent_named: bool,
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
            child,
            master,
            start: 0,
            terminal: Terminal::new(cols, rows),
            pending: VecDeque::new(),
            dead: false,
            last_input: None,
            idle_observations: 0,
            agent_named: false,
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
        for _ in 0..8 {
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
                    return Err("native message flood".into());
                }
            }
            if self.input.len() > MAX_MESSAGE + 4 {
                return Err("native message buffer full".into());
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
        let packet = wire(&ServerMessage::Frame(frame.clone()))?;
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

struct Server {
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
    fn add_pane(&mut self) -> Result<()> {
        let id = self.next_pane;
        let pane = NativePane::spawn(
            &self.config,
            self.workspace.cols,
            self.workspace.rows.saturating_sub(2),
            id,
        )?;
        self.workspace.panes.push(PaneInfo {
            id,
            number: self.workspace.panes.len() + 1,
            pid: pane.child.id(),
            cwd: self.config.cwd.to_string_lossy().into_owned(),
            title: "shell".into(),
            palette: ((id - 1) % 9) as u8,
            status: Status::Unknown,
            session_id: None,
            dead: false,
            kind: None,
            last_activity: crate::now(),
            status_since: crate::now(),
        });
        self.panes.insert(id, pane);
        self.next_pane += 1;
        if self.workspace.active == 0 {
            self.workspace.active = id;
        }
        Ok(())
    }
    fn resize(&mut self) {
        for (id, cols, rows) in self.workspace.terminal_sizes() {
            if let Some(pane) = self.panes.get_mut(&id) {
                pane.resize(cols.max(1), rows.max(1));
            }
        }
        self.dirty = true;
    }
    fn grid(&mut self, cols: u8, rows: u8) -> Result<()> {
        let count = Workspace::grid_count(cols, rows)?;
        let old_len = self.workspace.panes.len();
        if count < old_len {
            let tree = process::snapshot(Path::new("/proc"))?;
            let victims: Vec<_> = self
                .workspace
                .panes
                .iter()
                .rev()
                .filter(|info| {
                    self.panes
                        .get(&info.id)
                        .is_some_and(|pane| pane.unused(&tree))
                })
                .take(old_len - count)
                .map(|info| info.id)
                .collect();
            if victims.len() != old_len - count {
                return Err("grid shrink refused: insufficient unused panes; remaining panes are occupied or cannot be proven idle".into());
            }
            // Validate every victim from one fresh snapshot, then recheck all immediately before mutation.
            for _ in 0..2 {
                let tree = process::snapshot(Path::new("/proc"))?;
                if victims
                    .iter()
                    .any(|id| !self.panes.get(id).is_some_and(|p| p.unused(&tree)))
                {
                    return Err(
                        "grid shrink refused: a removed pane is occupied or cannot be proven idle"
                            .into(),
                    );
                }
            }
            let mut prepared = self.workspace.clone();
            prepared.panes.retain(|pane| !victims.contains(&pane.id));
            if victims.contains(&prepared.active) {
                prepared.active = prepared.panes[0].id;
            }
            if matches!(prepared.reading, ReadingMode::Capped(id) | ReadingMode::Fullscreen(id) if victims.contains(&id))
            {
                prepared.reading = ReadingMode::Grid;
            }
            prepared.apply(&Action::Grid { cols, rows })?;
            self.workspace = prepared;
            for id in victims {
                self.panes.remove(&id);
            }
        } else {
            while self.workspace.panes.len() < count {
                if let Err(e) = self.add_pane() {
                    for p in self.workspace.panes.drain(old_len..) {
                        self.panes.remove(&p.id);
                    }
                    return Err(e);
                }
            }
            let mut prepared = self.workspace.clone();
            if let Err(e) = prepared.apply(&Action::Grid { cols, rows }) {
                for p in self.workspace.panes.drain(old_len..) {
                    self.panes.remove(&p.id);
                }
                return Err(e);
            }
            self.workspace = prepared;
        }
        for (number, pane) in self.workspace.panes.iter_mut().enumerate() {
            pane.number = number + 1;
        }
        self.resize();
        Ok(())
    }
    fn message(&mut self, id: u64, message: ClientMessage) -> Result<()> {
        match message {
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
                match action {
                    Action::Shutdown => self.shutdown = true,
                    Action::Grid { cols, rows } => self.grid(cols, rows)?,
                    Action::Focus(_)
                    | Action::Read(_)
                    | Action::Fullscreen(_)
                    | Action::CloseReading
                    | Action::Swap { .. }
                    | Action::Help => {
                        self.workspace.apply(&action)?;
                        self.resize();
                    }
                    Action::ReadResize { .. }
                    | Action::FocusDirection { .. }
                    | Action::GridPicker
                    | Action::Agents(_)
                    | Action::MovePicker
                    | Action::DialogNavigate { .. }
                    | Action::DialogPick(_)
                    | Action::DialogClose => {
                        self.workspace.apply(&action)?;
                        self.resize();
                    }
                    _ => return Err("unsupported management command".into()),
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
                    }
                    ClientMessage::Paste(bytes) => {
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
                    }
                    ClientMessage::Mouse(mouse) => self.mouse(mouse)?,
                    ClientMessage::Action(action) => {
                        match action {
                            Action::Shutdown => self.shutdown = true,
                            Action::Grid { cols, rows } => self.grid(cols, rows)?,
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
                        self.resize();
                    }
                    _ => return Err("invalid native client message".into()),
                }
            }
        }
        Ok(())
    }
    fn mouse(&mut self, mouse: MouseEvent) -> Result<()> {
        let Some(view) = self
            .workspace
            .layout()
            .into_iter()
            .rev()
            .find(|v| v.rect.contains(mouse.position))
        else {
            return Ok(());
        };
        if mouse.kind == MouseKind::Press {
            self.workspace.apply(&Action::Focus(view.pane.id))?;
            self.resize();
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
        for (id, pane) in &mut self.panes {
            let mut buf = [0; 8192];
            for _ in 0..16 {
                match pane.master.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        self.dirty = true;
                        for effect in pane.terminal.feed(&buf[..n]) {
                            match effect {
                                Effect::WritePty(bytes) => {
                                    let _ = pane.queue(&bytes);
                                }
                                Effect::Title(title) => {
                                    if let Some(info) =
                                        self.workspace.panes.iter_mut().find(|p| p.id == *id)
                                    {
                                        info.title = title.chars().take(1024).collect();
                                    }
                                }
                                Effect::Clipboard(text) if text.len() <= MAX_INPUT => {
                                    clipboard.push(text)
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
                pane.dead = true;
                pane.pending.clear();
                if let Some(info) = self.workspace.panes.iter_mut().find(|p| p.id == *id) {
                    info.dead = true;
                    info.status = Status::Exited;
                }
                self.dirty = true;
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
                info.kind = Some(agent.kind);
                info.session_id = agent.session_id.clone();
                info.title = agent.name.chars().take(1024).collect();
                if let Some(pane) = self.panes.get_mut(&info.id) {
                    pane.agent_named = true;
                }
            } else {
                if let Some(pane) = self.panes.get_mut(&info.id) {
                    if std::mem::take(&mut pane.agent_named) {
                        info.title = "shell".into();
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
            }
            if before.status != info.status {
                info.status_since = crate::now();
            }
            self.dirty |= before != *info;
        }
    }
    fn frames(&mut self, spinner: usize) {
        let screens = self
            .panes
            .iter()
            .map(|(id, pane)| (*id, pane.terminal.snapshot()))
            .collect();
        let observer_frame = render::compose(
            &self.workspace,
            &screens,
            false,
            self.notice.as_deref(),
            spinner,
        );
        let controller_frame = render::compose(
            &self.workspace,
            &screens,
            true,
            self.notice.as_deref(),
            spinner,
        );
        let mut removed = vec![];
        for (id, client) in &mut self.clients {
            if client.attached {
                let frame = if self.controller == Some(*id) {
                    &controller_frame
                } else {
                    &observer_frame
                };
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
    validate_size(config.cols, config.rows)?;
    let mut workspace = Workspace::new(config.cols, config.rows)?;
    workspace.ui.config = config.ui.clone();
    let (listener, _endpoint) = endpoint(&config)?;
    super::lifecycle::save_pid(&config.state_dir)?;
    let mut server = Server {
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
    server.add_pane()?;
    server.grid(server.config.initial_grid.0, server.config.initial_grid.1)?;
    server.resize();
    let (requests, results, worker) = discovery_worker(server.config.state_dir.clone());
    let mut discovery = Instant::now() - Duration::from_secs(2);
    let mut render_time = Instant::now() - Duration::from_secs(1);
    let mut animation = Instant::now();
    let mut spinner = 0;
    let mut idle_poll = Instant::now() - Duration::from_millis(100);
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
        if idle_poll.elapsed() >= Duration::from_millis(100) {
            for pane in server.panes.values_mut() {
                if !pane.dead {
                    pane.observe_idle();
                }
            }
            if let Ok(tree) = process::snapshot(Path::new("/proc")) {
                let unused: Vec<_> = server
                    .workspace
                    .panes
                    .iter()
                    .filter(|info| {
                        server
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
            idle_poll = Instant::now();
        }
        if discovery.elapsed() >= Duration::from_secs(2) {
            let _ = requests.try_send(server.discovery_snapshot());
            discovery = Instant::now();
        }
        if let Ok(result) = results.try_recv() {
            server.discovery_result(result);
        }
        if animation.elapsed() >= Duration::from_millis(125) {
            spinner += 1;
            animation = Instant::now();
            server.dirty |= server
                .workspace
                .panes
                .iter()
                .any(|p| p.status == Status::Working);
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
            libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 10);
        }
    }
    drop(requests);
    drop(results);
    let _ = worker.join();
    Ok(())
}
