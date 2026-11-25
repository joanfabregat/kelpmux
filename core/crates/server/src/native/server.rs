//! A foreground, owned PTY server. Client lifetimes never own shell lifetimes.
//!
//! The server holds sessions only: PTYs, terminal emulation and history, pane
//! observations, closed-session records and host sampling. Every client is a
//! stream client (`super::stream`); presentation and UI state belong to the
//! clients (ADR 0002).
use super::{
    engine::{Effect, Terminal},
    protocol::*,
};
use crate::{Result, discovery::Discovery, journal, model::Status, process};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
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
    /// The shell of panes created without `spawn {shell}`.
    pub shell: PathBuf,
    /// The launch directory of panes created without `spawn {cwd}`.
    pub cwd: PathBuf,
    /// The base pane size: panes without a lease have this size.
    pub cols: u16,
    pub rows: u16,
    /// Upload directory and retention (`uploads`).
    pub uploads: super::uploads::Settings,
    /// File operations (`files`): the trash location and the file root.
    pub files: super::files::Settings,
}
/// A finished file operation: the connection, whether it was a deletion, and
/// its reply.
type FileResult = (u64, bool, super::stream::Event);
/// File operations, counted per connection from their start until their
/// reply enters the connection's output queue (or is dropped after a
/// disconnect). A reply held for a connection that has not read earlier
/// output keeps its slot, so held replies are bounded like running ones.
struct FileJobs {
    running: BTreeMap<u64, usize>,
    /// `file_delete` operations still running on a worker thread.
    deletes: usize,
    sender: std::sync::mpsc::Sender<FileResult>,
    receiver: std::sync::mpsc::Receiver<FileResult>,
    /// The folder file operations, downloads and upload destinations are
    /// confined to, resolved at startup; `None` when it could not be.
    root: Option<super::files::Root>,
}
impl FileJobs {
    fn new(root: Option<super::files::Root>) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        Self {
            running: BTreeMap::new(),
            deletes: 0,
            sender,
            receiver,
            root,
        }
    }
    fn release(&mut self, id: u64, count: usize) {
        if count == 0 {
            return;
        }
        if let Some(running) = self.running.get_mut(&id) {
            *running = running.saturating_sub(count);
            if *running == 0 {
                self.running.remove(&id);
            }
        }
    }
}
/// One download as the event loop sees it: the worker's pieces are taken
/// only while the client has window credits, and each framed message waits
/// in `next` until the client's output queue has room.
struct Download {
    client: u64,
    request: String,
    job: super::downloads::Job,
    /// Chunks the client may still receive before acknowledging one.
    credits: usize,
    /// End offsets of the chunks sent and not yet acknowledged, in order.
    in_flight: VecDeque<u64>,
    /// Bytes sent so far.
    offset: u64,
    started: bool,
    /// A framed event waiting for room in the output queue.
    next: Option<Vec<u8>>,
    /// `next` is the last event (done or error).
    finished: bool,
    /// The last acknowledgement, or the last event taken into the output
    /// queue (`downloads::STALL_TIMEOUT`).
    progress: Instant,
}
#[derive(Default)]
struct Downloads {
    active: BTreeMap<u64, Download>,
    next: u64,
    /// Download threads still running, cancelled ones included.
    threads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// Threads of downloads that ended early (cancelled, failed by the
    /// server or stalled), by connection, until they return: a connection
    /// cannot cancel and restart downloads to run more threads than its
    /// allowance.
    stopping: Vec<(u64, super::downloads::Thread)>,
}
impl Downloads {
    /// End a download: dropping it cancels its thread, which still counts
    /// against its connection until it returns.
    fn remove(&mut self, download: u64) {
        if let Some(state) = self.active.remove(&download) {
            let (client, thread) = (state.client, state.job.thread());
            drop(state);
            if thread.running() {
                self.stopping.push((client, thread));
            }
        }
    }
    /// Downloads a connection holds: unfinished ones, and the threads of
    /// those it ended that have not returned yet.
    fn held_by(&mut self, client: u64) -> usize {
        self.stopping.retain(|(_, thread)| thread.running());
        self.active
            .values()
            .filter(|state| state.client == client)
            .count()
            + self.stopping.iter().filter(|(c, _)| *c == client).count()
    }
}
/// Whether a download waiting for its client (a chunk unacknowledged or an
/// event waiting for room) has made no progress for
/// `downloads::STALL_TIMEOUT`.
fn download_stalled(waiting: bool, progress: Instant, now: Instant) -> bool {
    waiting && now.saturating_duration_since(progress) >= super::downloads::STALL_TIMEOUT
}
/// Download bytes a connection's output queue may hold: two framed chunks
/// (base64 with room for the JSON fields). A screen update queued after
/// them waits for at most that much, however many downloads run.
const MAX_QUEUED_DOWNLOAD_BYTES: usize = 2 * (super::downloads::CHUNK_BYTES.div_ceil(3) * 4 + 1024);
const MAX_CLIENTS: usize = 16;
const MAX_QUEUED: usize = MAX_MESSAGE + 4;
const MAX_PTY_WRITE: usize = 256 * 1024;
/// Output-queue room a file reply leaves for other reliable messages: a
/// reply enters the queue only while the queue stays this far below
/// `MAX_QUEUED` and below half its message count. The largest file reply
/// (15 MiB, `files::REPLY_BUDGET`) still fits an otherwise empty queue.
const FILE_REPLY_HEADROOM: usize = 512 * 1024;
const MAX_OUTPUT_MESSAGES: usize = 32;
/// File replies held per connection, beyond which it is a slow peer: its own
/// running operations (at most `files::MAX_IN_FLIGHT`) plus immediate
/// refusals such as `busy`.
const MAX_HELD_FILE_REPLIES: usize = 256;
/// Pause after `accept` fails for lack of descriptors or memory.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(250);
/// The open-file limit the server started with: the server raises its soft
/// limit and restores this one for its shells.
static OPEN_FILES: std::sync::OnceLock<libc::rlimit> = std::sync::OnceLock::new();
/// Raise the soft open-file limit to the hard limit (at most 65,536): the
/// server holds descriptors for PTYs, connections and file operations, and
/// polls them with `poll`, which has no `select` ceiling.
fn raise_open_file_limit() {
    let mut limit: libc::rlimit = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return;
    }
    OPEN_FILES.get_or_init(|| limit);
    let target = limit.rlim_max.min(65_536);
    if limit.rlim_cur < target {
        limit.rlim_cur = target;
        unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) };
    }
}

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

/// Remembered folders may have been moved or deleted since they were saved.
/// Fall back to the workspace folder, then `$HOME`, then `/`, so one stale
/// folder cannot prevent a shell from starting.
fn launch_folder(requested: &Path, workspace: &Path) -> PathBuf {
    [requested, workspace]
        .into_iter()
        .map(Path::to_path_buf)
        .chain(std::env::var_os("HOME").map(PathBuf::from))
        .find(|folder| folder.is_absolute() && folder.is_dir())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// A client-selected shell passes the same checks as the workspace shell at
/// `start`: an absolute path to an existing executable regular file.
fn valid_shell(shell: &Path) -> Result<()> {
    if !shell.is_absolute() || shell.as_os_str().len() > 4096 {
        return Err("shell must be an absolute path".into());
    }
    let meta = fs::metadata(shell).map_err(|e| format!("shell {}: {e}", shell.display()))?;
    if !meta.is_file() || meta.mode() & 0o111 == 0 {
        return Err("shell must be an executable file".into());
    }
    Ok(())
}

/// What a new pane runs and where.
struct Launch<'a> {
    state_dir: &'a Path,
    shell: &'a Path,
    cwd: &'a Path,
    size: (u16, u16),
    id: PaneId,
    startup: Option<&'a str>,
}

struct NativePane {
    launch_cwd: PathBuf,
    launch_shell: PathBuf,
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
    program: Option<super::program_status::Snapshot>,
    created_at: u64,
    startup: Option<String>,
    argv: Vec<String>,
    exit: Option<(Option<i32>, Option<i32>)>,
    /// The stream connection whose request created this pane.
    spawned_by: Option<u64>,
}
impl NativePane {
    fn spawn(launch: &Launch<'_>) -> Result<Self> {
        let (cols, rows) = launch.size;
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
        let mut command = Command::new(launch.shell);
        super::shell::integrate_shell(&mut command, launch.state_dir, launch.shell)?;
        // Agent and multiplexer session markers of whoever started the server.
        super::shell::remove_session_markers(&mut command);
        command.arg("-i");
        if let Some(startup) = launch.startup {
            command.arg("-c").arg(format!(
                "{startup}; exec {} -i",
                crate::shell_quote(&launch.shell.to_string_lossy())
            ));
        }
        command
            .current_dir(launch.cwd)
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env("TERM_PROGRAM", "kelpmux")
            .env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"))
            .env_remove("NO_COLOR")
            .env("KELPMUX", "1")
            .env("KELPMUX_PANE", format!("%{}", launch.id))
            .env("KELPMUX_STATE_DIR", launch.state_dir)
            .env_remove("SSH_CONNECTION")
            .env_remove("SSH_CLIENT")
            .env_remove("SSH_TTY")
            .env_remove("KELPMUX_DEV_BADGE")
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        let open_files = OPEN_FILES.get().copied();
        // Only async-signal-safe libc operations occur between fork and exec.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Shells get the limit the server started with, not the
                // raised one (programs using `select` expect a low limit).
                if let Some(limit) = &open_files {
                    libc::setrlimit(libc::RLIMIT_NOFILE, limit);
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
            launch_cwd: launch.cwd.to_path_buf(),
            launch_shell: launch.shell.to_path_buf(),
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
            program: None,
            created_at: crate::now(),
            startup: launch.startup.map(str::to_owned),
            argv: Vec::new(),
            exit: None,
            spawned_by: None,
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
    fn program_event(&mut self, event: super::program_status::Event) {
        use super::program_status::{Event, Records, Snapshot};
        if event == Event::Reset {
            self.program = None;
            return;
        }
        if event == Event::Prompt {
            if let Some(snapshot) = &mut self.program {
                snapshot.records.apply(&event);
            }
            return;
        }
        let group = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
        let Ok(owner) = u32::try_from(group)
            .ok()
            .filter(|id| *id > 0)
            .ok_or(())
            .and_then(|id| process::read(Path::new("/proc"), id).map_err(|_| ()))
        else {
            self.program = None;
            return;
        };
        let owner = (owner.pid, owner.start);
        if self.program.as_ref().is_none_or(|p| {
            p.owner != owner
                || p.agent.is_some_and(|(pid, birth)| {
                    !process::read(Path::new("/proc"), pid).is_ok_and(|p| p.start == birth)
                })
        }) {
            self.program = Some(Snapshot {
                owner,
                agent: None,
                records: Records::default(),
                event_at: crate::now(),
            });
        }
        if let Some(snapshot) = &mut self.program {
            let before = [crate::model::Kind::Codex, crate::model::Kind::Claude]
                .map(|kind| snapshot.records.status(kind));
            snapshot.records.apply(&event);
            if before
                != [crate::model::Kind::Codex, crate::model::Kind::Claude]
                    .map(|kind| snapshot.records.status(kind))
            {
                snapshot.event_at = crate::now();
            }
        }
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
    stream: Option<StreamClient>,
    socket: UnixStream,
    input: Vec<u8>,
    started: Option<Instant>,
    /// Framed messages, each marked when it is a download event.
    output: VecDeque<(Vec<u8>, bool)>,
    /// Bytes of download events in `output`.
    download_queued: usize,
    writing: Option<(Vec<u8>, usize)>,
    /// The one replaceable aggregate update.
    update: Option<Vec<u8>>,
    queued: usize,
    failed: bool,
    /// A record the server cannot accept; reported once before closing.
    rejected: Option<String>,
    /// Close once the queued output (the rejection) has been written.
    closing: bool,
    /// Framed file replies waiting for room in the output queue, each marked
    /// when it holds an operation slot (`FileJobs::running`).
    file_replies: VecDeque<(Vec<u8>, bool)>,
}
struct StreamClient {
    clipboard: bool,
    session_events: bool,
    host: bool,
    /// `binary-screens`: screens go in a binary record after each update.
    binary: bool,
    panes: Vec<PaneId>,
    previous: BTreeMap<PaneId, super::stream::PaneSnapshot>,
    metadata: Option<super::stream::Update>,
}
struct PaneLease {
    owner: u64,
    size: (u16, u16),
    theme: Option<[[u8; 3]; 2]>,
}
fn wire(message: &ServerMessage) -> Result<Vec<u8>> {
    wire_bytes(serde_json::to_vec(message)?)
}
/// Frame one payload: a JSON message or a binary screen record.
fn wire_bytes(bytes: Vec<u8>) -> Result<Vec<u8>> {
    if bytes.len() > MAX_MESSAGE {
        return Err("native response is too large".into());
    }
    let mut packet = Vec::with_capacity(bytes.len() + 4);
    packet.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    packet.extend(bytes);
    Ok(packet)
}
/// The reply to a record that is not a stream request, for example a client
/// of the composed protocol 58 that KelpServer 0.3 removed.
fn unsupported(error: &serde_json::Error) -> String {
    format!(
        "KelpServer {} accepts only stream protocol requests ({{\"Stream\":{{\"type\":\"hello\",\"version\":{}}}}} first); \
         the composed protocol 58 was removed in 0.3, use a matching client: {error}",
        env!("CARGO_PKG_VERSION"),
        super::stream::VERSION
    )
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
            stream: None,
            socket,
            input: vec![],
            started: None,
            output: VecDeque::new(),
            download_queued: 0,
            writing: None,
            update: None,
            queued: 0,
            failed: false,
            rejected: None,
            closing: false,
            file_replies: VecDeque::new(),
        })
    }
    fn send(&mut self, message: ServerMessage) -> Result<()> {
        let bytes = wire(&message)?;
        if self.queued + bytes.len() > MAX_QUEUED || self.output.len() >= MAX_OUTPUT_MESSAGES {
            return Err("slow native client".into());
        }
        self.queued += bytes.len();
        self.output.push_back((bytes, false));
        Ok(())
    }
    /// Hold a file reply until the output queue has room for it (see
    /// `pump_files`). File replies can be large (a read is up to about
    /// 11 MiB) and several can finish at once, so they wait here instead of
    /// overflowing the queue, which would disconnect the client. `false`
    /// when too many replies are held.
    fn hold_file_reply(&mut self, event: super::stream::Event, counted: bool) -> bool {
        let Ok(bytes) = wire(&ServerMessage::Stream(event)) else {
            return false;
        };
        if !counted && self.file_replies.len() >= MAX_HELD_FILE_REPLIES {
            return false;
        }
        self.file_replies.push_back((bytes, counted));
        true
    }
    /// Move held file replies into the output queue, in order, while it
    /// keeps room for other reliable messages; returns how many operation
    /// slots that released.
    fn pump_files(&mut self) -> usize {
        let mut released = 0;
        while let Some((bytes, _)) = self.file_replies.front() {
            if self.output.len() >= MAX_OUTPUT_MESSAGES / 2
                || self.queued + bytes.len() + FILE_REPLY_HEADROOM > MAX_QUEUED
            {
                break;
            }
            let (bytes, counted) = self.file_replies.pop_front().unwrap();
            released += usize::from(counted);
            self.queued += bytes.len();
            self.output.push_back((bytes, false));
        }
        released
    }
    /// Whether a download event of `len` bytes may enter the output queue:
    /// the room file replies leave for other messages, and never ahead of a
    /// pending screen update, which the queue's messages would otherwise
    /// delay for as long as a download keeps refilling it, and never more
    /// than `MAX_QUEUED_DOWNLOAD_BYTES` of download events in all, so an
    /// update that arrives after them waits for little.
    fn download_room(&self, len: usize) -> bool {
        self.update.is_none()
            && self.output.len() < MAX_OUTPUT_MESSAGES / 2
            && self.queued + len + FILE_REPLY_HEADROOM <= MAX_QUEUED
            && self.download_queued + len <= MAX_QUEUED_DOWNLOAD_BYTES
    }
    /// Queue a download event (after `download_room`).
    fn queue_download(&mut self, bytes: Vec<u8>) {
        self.queued += bytes.len();
        self.download_queued += bytes.len();
        self.output.push_back((bytes, true));
    }
    /// Operation slots held by replies not yet in the output queue.
    fn held_slots(&self) -> usize {
        self.file_replies
            .iter()
            .filter(|(_, counted)| *counted)
            .count()
    }
    fn idle(&self) -> bool {
        self.update.is_none() && self.writing.is_none() && self.output.is_empty()
    }
    fn flush(&mut self) -> Result<()> {
        if self.failed {
            return Err("slow native client".into());
        }
        // Work per client is bounded so even a permanently writable observer cannot monopolize PTYs.
        for _ in 0..8 {
            if self.writing.is_none() {
                self.writing = match self.output.pop_front() {
                    Some((bytes, download)) => {
                        if download {
                            self.download_queued -= bytes.len();
                        }
                        Some(bytes)
                    }
                    None => self.update.take(),
                }
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
        if self.closing && self.idle() {
            return Err("native client rejected".into());
        }
        Ok(())
    }
    fn receive(&mut self) -> Result<Vec<ClientMessage>> {
        let mut messages = vec![];
        if self.closing {
            return Ok(messages);
        }
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
                match serde_json::from_slice(&self.input[4..n + 4]) {
                    Ok(message) => messages.push(message),
                    Err(error) => {
                        // Deliver earlier records, then report this one and close.
                        self.rejected = Some(unsupported(&error));
                        self.closing = true;
                        self.input.clear();
                        return Ok(messages);
                    }
                }
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
                // A client that closes with replies it never read resets the
                // connection: Linux reports ECONNRESET after the last byte
                // it sent. The requests read before the reset still count
                // (a final `shutdown`, `close` or input); the connection
                // closes once they are handled.
                Err(_) if !messages.is_empty() => {
                    self.closing = true;
                    break;
                }
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
}

type RecoveryResult = Result<(
    Vec<super::recovery::RecoveryItem>,
    Vec<super::history::Entry>,
)>;
struct RecoveryJob {
    receiver: std::sync::mpsc::Receiver<RecoveryResult>,
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
    /// Retry cache of creation requests: pane, directory, arguments, shell.
    spawn_requests: BTreeMap<String, (PaneId, PathBuf, Vec<String>, Option<PathBuf>)>,
    /// Unused shells for `PaneSession::unused_shell`, sampled only while a
    /// stream connection has `session_events` enabled.
    unused_shells: BTreeSet<PaneId>,
    /// Closed-session records last listed for stream clients; `restore` looks
    /// records up here by key.
    records: Vec<super::history::Entry>,
    /// Stream connections waiting for the running recovery job.
    recovery_waiters: Vec<(u64, String)>,
    history: super::history::History,
    epoch: String,
    leases: BTreeMap<PaneId, PaneLease>,
    recovery_job: Option<RecoveryJob>,
    recovery_pending: bool,
    discovered_agents: Vec<crate::model::Agent>,
    activity: super::activity::Activity,
    started: Instant,
    pty_bytes: u64,
    vitals: super::vitals::Sampler,
    status: super::status::Monitor,
    config: ServerConfig,
    /// Pane observations in creation order, as sent in `Update.panes`.
    infos: Vec<PaneInfo>,
    panes: BTreeMap<PaneId, NativePane>,
    clients: BTreeMap<u64, Client>,
    /// Unfinished upload batches and the upload registry.
    uploads: super::uploads::Uploads,
    /// File operations in progress (`files`).
    files: FileJobs,
    /// Downloads in progress (`file-downloads`).
    downloads: Downloads,
    next_pane: u64,
    next_client: u64,
    dirty: bool,
    shutdown: bool,
}
impl Server {
    fn base_size(&self) -> (u16, u16) {
        (self.config.cols, self.config.rows)
    }
    fn apply_theme(&mut self) {
        for (id, pane) in &mut self.panes {
            let colors = self
                .leases
                .get(id)
                .and_then(|lease| lease.theme)
                .map_or([None; 2], |colors| colors.map(Some));
            pane.terminal.set_theme(colors);
        }
        self.dirty = true;
    }
    fn touch(&mut self, pane: PaneId) {
        if let Some(info) = self.infos.iter_mut().find(|info| info.id == pane) {
            info.last_activity = crate::now();
        }
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
    fn pane_session(&self, id: PaneId) -> Option<super::stream::PaneSession> {
        let pane = self.panes.get(&id)?;
        Some(super::stream::PaneSession {
            pane: id,
            created_at: pane.created_at,
            launch_cwd: pane.launch_cwd.to_string_lossy().into_owned(),
            shell: pane.launch_shell.to_string_lossy().into_owned(),
            argv: pane.argv.clone(),
            startup: pane.startup.clone(),
            terminal_title: pane.terminal_title.clone(),
            unused_shell: self.unused_shells.contains(&id),
            spawned_by: pane.spawned_by,
        })
    }
    /// Deliver a lifecycle event to every connection that opted in. A pending
    /// replaceable update predates the event, so it is discarded together with
    /// that connection's baselines; the next update follows the event.
    fn session_event(&mut self, event: super::stream::Event) {
        for client in self.clients.values_mut() {
            let Some(stream) = client.stream.as_mut() else {
                continue;
            };
            if !stream.session_events {
                continue;
            }
            if let Some(old) = client.update.take() {
                client.queued = client.queued.saturating_sub(old.len());
                stream.previous.clear();
                stream.metadata = None;
            }
            // A connection that cannot take a lifecycle event has lost
            // continuity; the next flush disconnects it. Removing it here
            // would invalidate callers that still address the requester.
            if client.send(ServerMessage::Stream(event.clone())).is_err() {
                client.failed = true;
            }
        }
        self.dirty = true;
    }
    fn stop(&mut self, reason: super::stream::StopReason) {
        if !self.shutdown {
            self.shutdown = true;
            self.session_event(super::stream::Event::ServerStopping { reason });
        }
    }
    fn start_recovery(&mut self) {
        if self.recovery_job.is_some() {
            return;
        }
        let state = self.config.state_dir.clone();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || {
            let result = super::recovery::collect_cancellable(&state, &worker_cancel)
                .map(|agents| (agents, super::history::collect(&state)));
            let _ = sender.send(result);
        });
        self.recovery_job = Some(RecoveryJob {
            receiver,
            cancel,
            worker: Some(worker),
        });
    }
    /// Creates a pane for the stream `spawn` request and replacement shells.
    /// `shell` replaces the server's shell; `spawned_by` is the requesting
    /// stream connection.
    fn spawn_requested(
        &mut self,
        request: &str,
        cwd: PathBuf,
        argv: Vec<String>,
        shell: Option<PathBuf>,
        spawned_by: Option<u64>,
    ) -> Result<PaneId> {
        if !super::stream::valid_request(request)
            || !cwd.is_absolute()
            || argv.len() > 32
            || argv.iter().map(String::len).sum::<usize>() > 8192
            || argv.iter().any(|arg| arg.contains('\0'))
            || argv.first().is_some_and(|arg| arg.is_empty())
        {
            return Err("invalid pane creation request".into());
        }
        if let Some(shell) = &shell {
            valid_shell(shell)?;
        }
        let cwd = cwd.canonicalize()?;
        if !cwd.is_dir() {
            return Err("working directory must be a directory".into());
        }
        if let Some((pane, previous_cwd, previous_argv, previous_shell)) =
            self.spawn_requests.get(request)
        {
            if previous_cwd != &cwd || previous_argv != &argv || previous_shell != &shell {
                return Err("creation request was already used with different parameters".into());
            }
            if !self.panes.contains_key(pane) {
                return Err("created pane has already closed".into());
            }
            return Ok(*pane);
        }
        let startup = (!argv.is_empty()).then(|| {
            argv.iter()
                .map(|arg| crate::shell_quote(arg))
                .collect::<Vec<_>>()
                .join(" ")
        });
        let pane = self.spawn_pane(
            Some(&cwd),
            shell.as_deref(),
            startup.as_deref(),
            argv.clone(),
            spawned_by,
        )?;
        if self.spawn_requests.len() >= 512 {
            self.spawn_requests.pop_first();
        }
        self.spawn_requests
            .insert(request.to_owned(), (pane, cwd, argv, shell));
        self.resize();
        self.dirty = true;
        Ok(pane)
    }
    fn stream_request(&mut self, id: u64, request: super::stream::Request) -> Result<()> {
        use super::stream::{Event, Request};
        if let Request::Hello { version } = request {
            if version != super::stream::VERSION {
                return Err("unsupported stream protocol version".into());
            }
            let welcome = Event::Welcome {
                version,
                client: id,
                epoch: self.epoch.clone(),
                capabilities: super::stream::CAPABILITIES
                    .iter()
                    .map(|s| (*s).into())
                    .collect(),
                server_version: Some(env!("CARGO_PKG_VERSION").into()),
                startup_panes: Some(super::stream::StartupPanes::None),
                recovery_pending: self.recovery_pending,
                limits: Some(super::stream::LIMITS),
                notification_run: Some(self.activity.run().to_owned()),
                upload_limits: Some(super::uploads::LIMITS),
                file_limits: Some(super::files::Limits::with_root(self.files.root.as_ref())),
            };
            let client = self.clients.get_mut(&id).unwrap();
            if client.stream.is_some() {
                return Err("connection already initialized".into());
            }
            client.stream = Some(StreamClient {
                clipboard: false,
                session_events: false,
                host: false,
                binary: false,
                panes: Vec::new(),
                previous: BTreeMap::new(),
                metadata: None,
            });
            client.send(ServerMessage::Stream(welcome))?;
            self.dirty = true;
            return Ok(());
        }
        if self.clients[&id].stream.is_none() {
            return Err("stream hello required".into());
        }
        match request {
            Request::Clipboard { enabled } => {
                self.clients
                    .get_mut(&id)
                    .unwrap()
                    .stream
                    .as_mut()
                    .unwrap()
                    .clipboard = enabled;
            }
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
                        theme: None,
                    },
                );
                self.clients
                    .get_mut(&id)
                    .unwrap()
                    .send(ServerMessage::Stream(Event::Lease {
                        pane,
                        owner: Some(id),
                    }))?;
                self.apply_theme();
            }
            Request::Release { pane } => {
                if self.leases.contains_key(&pane) {
                    self.stream_owned(id, pane)?;
                } else if !self.panes.contains_key(&pane) {
                    return Err("unknown pane".into());
                }
                if self.leases.remove(&pane).is_some() {
                    self.resize();
                }
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
            Request::Theme {
                pane,
                foreground,
                background,
            } => {
                self.stream_owned(id, pane)?;
                self.leases.get_mut(&pane).unwrap().theme = Some([foreground, background]);
                self.apply_theme();
            }
            Request::Input { pane, bytes } => {
                self.stream_owned(id, pane)?;
                if bytes.len() > MAX_INPUT {
                    return Err("pane input too large".into());
                }
                self.panes.get_mut(&pane).unwrap().queue(&bytes)?;
                self.touch(pane);
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
                self.touch(pane);
            }
            Request::Mouse { pane, event } => {
                self.stream_owned(id, pane)?;
                let pty = self.panes.get_mut(&pane).unwrap();
                if event.position.x >= pty.size.0 || event.position.y >= pty.size.1 {
                    return Err("mouse outside pane".into());
                }
                let modes = pty.terminal.snapshot().modes;
                if modes.mouse_click || modes.mouse_drag || modes.mouse_motion {
                    if let Some(info) = self.infos.iter_mut().find(|info| info.id == pane) {
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
                    } else if event.position.x < 223 && event.position.y < 223 {
                        let button = if event.kind == MouseKind::Release {
                            3
                        } else {
                            button
                        };
                        pty.queue(&[
                            27,
                            b'[',
                            b'M',
                            button as u8 + 32,
                            event.position.x as u8 + 33,
                            event.position.y as u8 + 33,
                        ])?;
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
            Request::AcknowledgeAll => {
                self.activity.acknowledge_all()?;
            }
            Request::SessionEvents { enabled } => {
                let stream = self.clients.get_mut(&id).unwrap().stream.as_mut().unwrap();
                stream.session_events = enabled;
                // The next update carries the per-pane session data.
                stream.metadata = None;
            }
            Request::BinaryScreens { enabled } => {
                // Baselines stay valid: a client applies the next binary patch
                // to the screen it holds, whichever format delivered it.
                let stream = self.clients.get_mut(&id).unwrap().stream.as_mut().unwrap();
                stream.binary = enabled;
                stream.metadata = None;
            }
            Request::HostStatus { enabled } => {
                let stream = self.clients.get_mut(&id).unwrap().stream.as_mut().unwrap();
                stream.host = enabled;
                stream.metadata = None;
            }
            Request::Spawn {
                request,
                cwd,
                argv,
                shell,
            } => {
                let cwd = cwd.unwrap_or_else(|| self.config.cwd.clone());
                let pane = self.spawn_requested(&request, cwd, argv, shell, Some(id))?;
                self.reply(id, Event::Spawned { request, pane })?;
            }
            Request::DismissRecovery { request } => {
                // One shared signal for every client.
                self.recovery_pending = false;
                self.reply(id, Event::Done { request })?;
            }
            Request::Close { request, pane } => {
                if !self.panes.contains_key(&pane) {
                    return Err("unknown pane".into());
                }
                if self
                    .leases
                    .get(&pane)
                    .is_some_and(|lease| lease.owner != id)
                {
                    return Err("pane is leased by another client".into());
                }
                // Closing the last pane first creates a replacement shell.
                self.remove_pane(pane, true)?;
                self.reply(id, Event::Done { request })?;
            }
            Request::ClosedSessions { request } => {
                if self.recovery_waiters.len() >= 64 {
                    return Err("too many pending closed-session requests".into());
                }
                self.recovery_waiters.push((id, request));
                self.start_recovery();
            }
            Request::Restore {
                request,
                record,
                rerun,
            } => {
                let entry = self
                    .records
                    .iter()
                    .find(|entry| entry.key == record)
                    .cloned()
                    .ok_or("unknown closed-session record; list closed sessions again")?;
                let (pane, existing) = self.restore_record(&entry, rerun, Some(id))?;
                self.resize();
                self.reply(
                    id,
                    Event::Restored {
                        request,
                        record,
                        pane,
                        existing,
                    },
                )?;
            }
            Request::History {
                request,
                pane,
                start,
                limit,
            } => {
                if limit > super::stream::MAX_HISTORY_LINES {
                    return Err("history page exceeds 200 lines".into());
                }
                let lines = self
                    .panes
                    .get(&pane)
                    .ok_or("unknown pane")?
                    .terminal
                    .history(start, limit);
                self.reply(
                    id,
                    Event::History {
                        request,
                        pane,
                        lines,
                    },
                )?;
            }
            Request::Search {
                request,
                panes,
                query,
                limit,
            } => {
                let unique: std::collections::BTreeSet<_> = panes.iter().copied().collect();
                if query.is_empty()
                    || query.len() > 256
                    || query.chars().any(char::is_control)
                    || limit == 0
                    || limit > 64
                    || unique.len() != panes.len()
                    || panes.iter().any(|pane| !self.panes.contains_key(pane))
                {
                    return Err("invalid pane search".into());
                }
                let targets: Vec<PaneId> = if panes.is_empty() {
                    self.panes.keys().copied().collect()
                } else {
                    panes
                };
                let mut results = Vec::new();
                let mut remaining = limit;
                let mut budget = super::stream::MAX_SEARCH_CONTEXT;
                let mut truncated = false;
                for pane in &targets {
                    if remaining == 0 {
                        // Unsearched panes may hold further matches.
                        truncated = true;
                        break;
                    }
                    let (hits, pane_truncated) =
                        self.panes[pane].terminal.web_search(&query, remaining);
                    let mut kept = Vec::new();
                    let mut cut = false;
                    for hit in hits {
                        let bytes: usize = hit.lines.iter().map(String::len).sum();
                        if bytes > budget {
                            cut = true;
                            break;
                        }
                        budget -= bytes;
                        kept.push(hit);
                    }
                    remaining -= kept.len();
                    truncated |= pane_truncated || cut;
                    if !kept.is_empty() || pane_truncated || cut {
                        results.push(super::stream::PaneSearchResult {
                            pane: *pane,
                            hits: kept,
                            truncated: pane_truncated || cut,
                        });
                    }
                    if cut {
                        break;
                    }
                }
                self.reply(
                    id,
                    Event::Search {
                        request,
                        results,
                        truncated,
                    },
                )?;
            }
            Request::Scroll { pane, lines } => {
                self.stream_owned(id, pane)?;
                self.panes
                    .get_mut(&pane)
                    .unwrap()
                    .terminal
                    .scroll(lines.clamp(-10000, 10000));
            }
            Request::Reveal { pane, line } => {
                self.stream_owned(id, pane)?;
                self.panes
                    .get_mut(&pane)
                    .unwrap()
                    .terminal
                    .scroll_to(line)?;
            }
            Request::Shutdown { request } => {
                self.reply(id, Event::Done { request })?;
                self.stop(super::stream::StopReason::Shutdown);
            }
            Request::Diagnostics { request } => {
                let report = self.diagnostics();
                self.reply(id, Event::Diagnostics { request, report })?;
            }
            // Uploads change no pane state, so they never trigger an update.
            // Any stream connection may upload: it is the same trust as
            // typing into a shell, which any connection may claim.
            Request::UploadBegin {
                request,
                files,
                dest,
            } => {
                let (batch, dir) =
                    self.uploads
                        .begin(id, &files, dest.as_deref(), self.files.root.as_ref())?;
                return self.reply(
                    id,
                    Event::UploadStarted {
                        request,
                        batch,
                        dir,
                    },
                );
            }
            Request::UploadChunk {
                batch,
                file_index,
                offset,
                data,
            } => {
                let event = match self.uploads.chunk(id, batch, file_index, offset, &data) {
                    super::uploads::Chunk::Accepted(received) => Event::UploadProgress {
                        batch,
                        file_index,
                        received,
                    },
                    super::uploads::Chunk::Failed(message) => {
                        Event::UploadFailed { batch, message }
                    }
                    super::uploads::Chunk::Ignored => return Ok(()),
                };
                return self.reply(id, event);
            }
            Request::UploadFinish { request, batch } => {
                let paths = self.uploads.finish(id, batch)?;
                return self.reply(
                    id,
                    Event::UploadDone {
                        request,
                        batch,
                        paths,
                    },
                );
            }
            Request::UploadCancel { request, batch } => {
                self.uploads.cancel(id, batch)?;
                return self.reply(id, Event::Done { request });
            }
            // File operations change no pane state either, and carry the same
            // trust as uploads. They run on worker threads (`poll_files`).
            request @ (Request::FileList { .. }
            | Request::FileStat { .. }
            | Request::FileRead { .. }
            | Request::FileWrite { .. }
            | Request::FileMkdir { .. }
            | Request::FileRename { .. }
            | Request::FileTrash { .. }
            | Request::FileDelete { .. }) => return self.start_file(id, request),
            // Downloads stream on their own threads (`poll_downloads`).
            Request::FileDownloadBegin {
                request,
                paths,
                archive,
            } => return self.begin_download(id, request, paths, archive),
            Request::FileDownloadAck { download, offset } => {
                self.ack_download(id, download, offset);
                return Ok(());
            }
            Request::FileDownloadCancel { request, download } => {
                // A download that already ended (or was never this
                // connection's) needs nothing more: `done` either way.
                if self
                    .downloads
                    .active
                    .get(&download)
                    .is_some_and(|state| state.client == id)
                {
                    self.downloads.remove(download);
                }
                return self.reply(id, Event::Done { request });
            }
            Request::Hello { .. } => unreachable!(),
        }
        self.dirty = true;
        Ok(())
    }
    /// Run a file request on a worker thread, within the per-connection and
    /// per-server bounds; the reply is delivered by `poll_files`.
    fn start_file(&mut self, id: u64, request: super::stream::Request) -> Result<()> {
        use super::files::{self, Code, Error};
        let correlation = request.correlation().unwrap_or_default().to_owned();
        let delete = matches!(request, super::stream::Request::FileDelete { .. });
        let running = self.files.running.get(&id).copied().unwrap_or(0);
        let total: usize = self.files.running.values().sum();
        let refusal = if running >= files::MAX_IN_FLIGHT || total >= files::MAX_SERVER_IN_FLIGHT {
            Some(format!(
                "at most {} file operations per connection and {} per server run at once",
                files::MAX_IN_FLIGHT,
                files::MAX_SERVER_IN_FLIGHT
            ))
        } else if delete && self.files.deletes >= files::MAX_SERVER_DELETES {
            Some(format!(
                "at most {} deletions per server run at once",
                files::MAX_SERVER_DELETES
            ))
        } else {
            let sender = self.files.sender.clone();
            let settings = self.config.files.clone();
            let root = self.files.root.clone();
            std::thread::Builder::new()
                .name("kelpserver-files".into())
                .spawn(move || {
                    let event = files::execute(request, &settings, root.as_ref());
                    let _ = sender.send((id, delete, event));
                })
                .err()
                .map(|error| error.to_string())
        };
        let Some(message) = refusal else {
            *self.files.running.entry(id).or_default() += 1;
            self.files.deletes += usize::from(delete);
            return Ok(());
        };
        // The refusal waits with the other file replies, so a burst of
        // requests answered `busy` at once cannot overflow the output queue.
        let client = self.clients.get_mut(&id).ok_or("unknown client")?;
        if !client.hold_file_reply(
            files::failure(correlation, Error::new(Code::Busy, message)),
            false,
        ) {
            return Err("slow native client".into());
        }
        let released = client.pump_files();
        self.files.release(id, released);
        Ok(())
    }
    /// Hold finished file operations for their connections, and move held
    /// replies into output queues with room.
    fn poll_files(&mut self) {
        while let Ok((id, delete, event)) = self.files.receiver.try_recv() {
            self.files.deletes -= usize::from(delete);
            match self.clients.get_mut(&id) {
                // Bounded by the connection's own slots; only a reply that
                // cannot be framed is refused (`execute` keeps them small).
                Some(client) => {
                    if !client.hold_file_reply(event, true) {
                        client.failed = true;
                        self.files.release(id, 1);
                    }
                }
                None => self.files.release(id, 1),
            }
        }
        for (id, client) in &mut self.clients {
            if !client.file_replies.is_empty() {
                let released = client.pump_files();
                self.files.release(*id, released);
            }
        }
    }
    /// Start a download on its own thread, within the per-connection and
    /// per-server bounds (`busy` beyond, like other file requests).
    fn begin_download(
        &mut self,
        id: u64,
        request: String,
        paths: Vec<String>,
        archive: Option<super::downloads::Archive>,
    ) -> Result<()> {
        use super::{
            downloads,
            files::{self, Code, Error},
        };
        use std::sync::atomic::Ordering;
        // Downloads it ended whose thread has not returned yet count too.
        let mine = self.downloads.held_by(id);
        let threads = self.downloads.threads.load(Ordering::Relaxed);
        let refusal =
            if mine >= downloads::MAX_DOWNLOADS || threads >= downloads::MAX_SERVER_DOWNLOADS {
                Error::new(
                    Code::Busy,
                    format!(
                        "at most {} downloads per connection and {} per server run at once",
                        downloads::MAX_DOWNLOADS,
                        downloads::MAX_SERVER_DOWNLOADS
                    ),
                )
            } else {
                match downloads::start(
                    paths,
                    archive,
                    self.files.root.clone(),
                    &self.downloads.threads,
                ) {
                    Ok(job) => {
                        self.downloads.next += 1;
                        self.downloads.active.insert(
                            self.downloads.next,
                            Download {
                                client: id,
                                request,
                                job,
                                credits: downloads::MAX_CHUNKS_IN_FLIGHT,
                                in_flight: VecDeque::new(),
                                offset: 0,
                                started: false,
                                next: None,
                                finished: false,
                                progress: Instant::now(),
                            },
                        );
                        return Ok(());
                    }
                    Err(error) => Error::new(Code::Io, error.to_string()),
                }
            };
        let client = self.clients.get_mut(&id).ok_or("unknown client")?;
        if !client.hold_file_reply(files::failure(request, refusal), false) {
            return Err("slow native client".into());
        }
        let released = client.pump_files();
        self.files.release(id, released);
        Ok(())
    }
    /// One acknowledged chunk returns one credit. An acknowledgement that
    /// is not the oldest chunk's end fails the download; one for a download
    /// that ended, or another connection's, is ignored.
    fn ack_download(&mut self, id: u64, download: u64, offset: u64) {
        let Some(state) = self.downloads.active.get_mut(&download) else {
            return;
        };
        if state.client != id || state.finished {
            return;
        }
        if state.in_flight.front() == Some(&offset) {
            state.in_flight.pop_front();
            state.credits += 1;
            state.progress = Instant::now();
            return;
        }
        let error = super::files::Error::new(
            super::files::Code::InvalidRequest,
            "acknowledgement out of order: acknowledge each chunk's end offset in order",
        );
        let event = super::stream::Event::FileError {
            request: state.request.clone(),
            error,
            download: Some(download),
        };
        // The chunk waiting to be queued is dropped with the download.
        state.next = wire(&ServerMessage::Stream(event)).ok();
        state.finished = true;
        if state.next.is_none() {
            self.downloads.remove(download);
        }
    }
    /// Move downloads forward: take the worker's pieces while the client has
    /// credits, and queue each framed event once the output queue has room.
    fn poll_downloads(&mut self) {
        use super::{downloads::Piece, stream::Event};
        use base64::{Engine, engine::general_purpose::STANDARD};
        let now = Instant::now();
        let ids: Vec<u64> = self.downloads.active.keys().copied().collect();
        for download in ids {
            let state = self.downloads.active.get_mut(&download).unwrap();
            let Some(client) = self.clients.get_mut(&state.client) else {
                self.downloads.active.remove(&download);
                continue;
            };
            let waiting = state.next.is_some() || !state.in_flight.is_empty();
            if download_stalled(waiting, state.progress, now) {
                // The client left the download behind: stop it, and tell
                // the client if it ever reads again.
                let event = Event::FileError {
                    request: state.request.clone(),
                    error: super::files::Error::new(
                        super::files::Code::Io,
                        format!(
                            "no acknowledgement for {} seconds: the download was stopped",
                            super::downloads::STALL_TIMEOUT.as_secs()
                        ),
                    ),
                    download: state.started.then_some(download),
                };
                let id = state.client;
                self.downloads.remove(download);
                if client.hold_file_reply(event, false) {
                    let released = client.pump_files();
                    self.files.release(id, released);
                }
                continue;
            }
            let mut remove = false;
            // Bounded work per connection and loop: a window's worth.
            for _ in 0..super::downloads::MAX_CHUNKS_IN_FLIGHT + 2 {
                if let Some(bytes) = state.next.take() {
                    if !client.download_room(bytes.len()) {
                        state.next = Some(bytes);
                        break;
                    }
                    client.queue_download(bytes);
                    state.progress = now;
                    if state.finished {
                        remove = true;
                        break;
                    }
                    continue;
                }
                if state.finished {
                    remove = true;
                    break;
                }
                if state.started && state.credits == 0 {
                    break;
                }
                let piece = match state.job.receiver.try_recv() {
                    Ok(piece) => piece,
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Piece::Failed(super::files::Error::new(
                            super::files::Code::Io,
                            "the download stopped unexpectedly",
                        ))
                    }
                };
                let event = match piece {
                    Piece::Started {
                        name,
                        size,
                        archive,
                        entries,
                        input_bytes,
                    } => {
                        state.started = true;
                        Event::FileDownloadStarted {
                            request: state.request.clone(),
                            download,
                            name,
                            size,
                            archive,
                            entries,
                            input_bytes,
                        }
                    }
                    Piece::Chunk { data, input } => {
                        let offset = state.offset;
                        state.offset += data.len() as u64;
                        state.credits -= 1;
                        state.in_flight.push_back(state.offset);
                        Event::FileDownloadChunk {
                            download,
                            offset,
                            data: STANDARD.encode(&data),
                            input,
                        }
                    }
                    Piece::Done {
                        size,
                        hash,
                        skipped,
                        changed,
                    } => {
                        state.finished = true;
                        Event::FileDownloadDone {
                            request: state.request.clone(),
                            download,
                            size,
                            hash,
                            skipped,
                            changed,
                        }
                    }
                    Piece::Failed(error) => {
                        state.finished = true;
                        Event::FileError {
                            request: state.request.clone(),
                            error,
                            download: state.started.then_some(download),
                        }
                    }
                };
                match wire(&ServerMessage::Stream(event)) {
                    Ok(bytes) => state.next = Some(bytes),
                    Err(_) => {
                        remove = true;
                        break;
                    }
                }
            }
            if remove {
                self.downloads.remove(download);
            }
        }
    }
    fn reply(&mut self, id: u64, event: super::stream::Event) -> Result<()> {
        self.clients
            .get_mut(&id)
            .ok_or("unknown client")?
            .send(ServerMessage::Stream(event))
    }
    fn stream_frames(
        &mut self,
        vitals: super::vitals::Vitals,
        host: Option<Box<super::status::Snapshot>>,
    ) {
        use super::stream::{Event, Lease, PaneSnapshot, Update};
        self.leases
            .retain(|pane, _| self.panes.get(pane).is_some_and(|pty| !pty.dead));
        let sessions: Vec<_> = if self.clients.values().any(|client| {
            client
                .stream
                .as_ref()
                .is_some_and(|stream| stream.session_events)
        }) {
            self.panes
                .keys()
                .filter_map(|id| self.pane_session(*id))
                .collect()
        } else {
            Vec::new()
        };
        let mut removed = Vec::new();
        for (id, client) in &mut self.clients {
            let idle = client.idle();
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
                panes: self.infos.clone(),
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
                sessions: if stream.session_events {
                    sessions.clone()
                } else {
                    Vec::new()
                },
                host: if stream.host { host.clone() } else { None },
                binary_screens: stream.binary,
            };
            let mut screens = stream
                .binary
                .then(kelpclient_protocol::screens::RecordBuilder::new);
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
                    match screens.as_mut() {
                        Some(screens) => screens.patch(&patch, &screen),
                        None => update.patches.push(patch),
                    }
                    revision
                } else {
                    match screens.as_mut() {
                        Some(screens) => screens.snapshot(*pane, 1, &screen),
                        None => update.snapshots.push(PaneSnapshot {
                            pane: *pane,
                            revision: 1,
                            screen: screen.clone(),
                        }),
                    }
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
                sessions: update.sessions.clone(),
                host: update.host.clone(),
                binary_screens: update.binary_screens,
            };
            if update.snapshots.is_empty()
                && update.patches.is_empty()
                && screens.as_ref().is_none_or(|screens| screens.is_empty())
                && stream.metadata.as_ref() == Some(&metadata)
            {
                continue;
            }
            stream.metadata = Some(metadata);
            let packet =
                wire(&ServerMessage::Stream(Event::Update(update))).and_then(|mut packet| {
                    // The update and its screens are one replaceable packet.
                    if let Some(screens) = screens {
                        packet.extend(wire_bytes(screens.finish())?);
                    }
                    Ok(packet)
                });
            let packet = match packet {
                Ok(packet) => packet,
                Err(_) => {
                    removed.push(*id);
                    continue;
                }
            };
            if let Some(old) = client.update.take() {
                client.queued = client.queued.saturating_sub(old.len());
            }
            if client.queued + packet.len() > 2 * MAX_QUEUED {
                removed.push(*id);
                continue;
            }
            client.queued += packet.len();
            client.update = Some(packet);
        }
        for id in removed {
            self.remove_client(id);
        }
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
        let Some(result) = result else {
            return;
        };
        self.recovery_job = None;
        self.dirty = true;
        let (entries, mut closed) = match result {
            Ok(result) => result,
            Err(error) => {
                for (client, request) in std::mem::take(&mut self.recovery_waiters) {
                    if let Some(client) = self.clients.get_mut(&client) {
                        let _ = client.send(ServerMessage::Stream(super::stream::Event::Error {
                            message: format!("closed sessions: {error}"),
                            request: Some(request),
                        }));
                    }
                }
                return;
            }
        };
        for item in entries.iter().take(256) {
            let e = &item.entry;
            if let Some(at) = e.ended_at.or_else(|| {
                (!process::read(Path::new("/proc"), e.pid)
                    .is_ok_and(|p| p.start == e.process_start))
                .then_some(e.last_seen)
            }) {
                if closed.iter().any(|h| {
                    h.workspace == item.workspace
                        && (h.session_id.is_some() && h.session_id == e.session_id
                            || e.session_id.as_deref().is_some_and(|id| {
                                h.sessions.iter().any(|s| s.kind == e.kind && s.id == id)
                            })
                            || !h.key.starts_with("legacy:")
                                && h.cwd == e.cwd
                                && h.kind == Some(e.kind)
                                && h.closed_at.is_some_and(|t| t.abs_diff(at) < 5))
                }) {
                    continue;
                }
                closed.push(super::history::Entry {
                    key: format!("legacy:{}:{}:{}", item.workspace, e.pid, e.process_start),
                    workspace: item.workspace.clone(),
                    pane: 0,
                    cwd: e.cwd.clone(),
                    name: e.name.clone(),
                    kind: Some(e.kind),
                    session_id: e.session_id.clone(),
                    argv: vec![],
                    closed_at: Some(at),
                    reason: "Previous run".into(),
                    sessions: e
                        .session_id
                        .iter()
                        .filter(|id| crate::session_id(id))
                        .map(|id| super::history::Session {
                            kind: e.kind,
                            id: id.clone(),
                            cwd: e.cwd.clone(),
                            first_seen: e.first_seen,
                            last_seen: e.last_seen,
                        })
                        .collect(),
                });
            }
        }
        closed.retain(|e| !self.history.restored.contains(&e.key));
        closed.sort_by_key(|e| std::cmp::Reverse(e.closed_at));
        closed.truncate(256);
        for (client, request) in std::mem::take(&mut self.recovery_waiters) {
            if let Some(client) = self.clients.get_mut(&client) {
                let _ = client.send(ServerMessage::Stream(
                    super::stream::Event::ClosedSessions {
                        request,
                        records: closed.iter().cloned().map(Into::into).collect(),
                    },
                ));
            }
        }
        self.records = closed;
    }
    /// Remove a pane. `replace_last` (a client's `close`) first creates a
    /// replacement shell for the last pane; the last pane's own exit stops
    /// the server instead.
    fn remove_pane(&mut self, id: PaneId, replace_last: bool) -> Result<()> {
        if !self.panes.contains_key(&id) {
            return Err("unknown pane".into());
        }
        let last = self.panes.len() == 1;
        // Create the replacement first so a spawn failure cannot leave the
        // closing client without a pane.
        if last && replace_last {
            self.spawn_pane(None, None, None, Vec::new(), None)?;
        }
        self.history.close(&self.epoch, id, crate::now());
        self.history
            .save(&self.config.state_dir.join("tab-history.json"))?;
        let key = format!("{}:{id}", self.epoch);
        let record = self
            .history
            .entries
            .iter()
            .any(|entry| entry.key == key)
            .then_some(key);
        let reason = if self.panes[&id].dead {
            super::stream::CloseReason::Exited
        } else {
            super::stream::CloseReason::Closed
        };
        self.panes.remove(&id);
        self.infos.retain(|pane| pane.id != id);
        self.leases.remove(&id);
        self.session_event(super::stream::Event::PaneClosed {
            pane: id,
            at: crate::now(),
            reason,
            record,
        });
        self.unused_shells.remove(&id);
        if last && !replace_last {
            self.stop(super::stream::StopReason::LastPaneExited);
        }
        self.resize();
        Ok(())
    }
    /// Start a pane at the base size. Without `cwd` or `shell`, the server's
    /// launch directory and shell are used.
    fn spawn_pane(
        &mut self,
        cwd: Option<&Path>,
        shell: Option<&Path>,
        startup: Option<&str>,
        argv: Vec<String>,
        spawned_by: Option<u64>,
    ) -> Result<PaneId> {
        let id = self.next_pane;
        let requested = cwd.unwrap_or(&self.config.cwd);
        let folder = launch_folder(requested, &self.config.cwd);
        if folder != requested {
            eprintln!(
                "kelpserver: pane %{id}: folder {} is missing or not a directory; starting its shell in {}",
                requested.display(),
                folder.display()
            );
        }
        let shell = shell.unwrap_or(&self.config.shell).to_path_buf();
        let mut pane = NativePane::spawn(&Launch {
            state_dir: &self.config.state_dir,
            shell: &shell,
            cwd: &folder,
            size: self.base_size(),
            id,
            startup,
        })
        .map_err(|e| {
            format!(
                "pane %{id}: cannot start shell {} in {}: {e}",
                shell.display(),
                folder.display()
            )
        })?;
        pane.argv = argv;
        pane.spawned_by = spawned_by;
        self.infos.push(PaneInfo {
            id,
            pid: pane.child.id(),
            cwd: folder.to_string_lossy().into_owned(),
            title: "shell".into(),
            status: Status::Unknown,
            session_id: None,
            dead: false,
            kind: None,
            last_activity: crate::now(),
            status_since: crate::now(),
            observation: crate::model::Observation::default(),
            command: None,
        });
        self.panes.insert(id, pane);
        self.next_pane += 1;
        if let Some(session) = self.pane_session(id) {
            self.session_event(super::stream::Event::PaneCreated { session });
        }
        self.dirty = true;
        Ok(id)
    }
    /// Restore a closed-session record. Returns the pane and whether it
    /// already existed (the conversation was already open).
    fn restore_record(
        &mut self,
        entry: &super::history::Entry,
        rerun: bool,
        spawned_by: Option<u64>,
    ) -> Result<(PaneId, bool)> {
        let cwd = PathBuf::from(&entry.cwd);
        let command = entry
            .command()
            .or_else(|| rerun.then(|| entry.previous_command()).flatten());
        // A plain shell may open in the fallback folder, but never resume or
        // rerun a program somewhere other than where it originally ran.
        if !cwd.is_absolute() || (command.is_some() && !cwd.is_dir()) {
            return Err("the original folder no longer exists".into());
        }
        if entry.kind.is_some() && entry.session_id.is_some() {
            if let Ok(records) = super::recovery::collect(&self.config.state_dir) {
                if let Some(live) = records.iter().find(|r| {
                    r.entry.ended_at.is_none()
                        && Some(r.entry.kind) == entry.kind
                        && r.entry.session_id == entry.session_id
                        && process::read(Path::new("/proc"), r.entry.pid)
                            .is_ok_and(|p| p.start == r.entry.process_start)
                }) {
                    let current = self
                        .config
                        .state_dir
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    if live.workspace != current {
                        return Err(format!(
                            "This conversation is already running in {} · {}",
                            live.workspace, live.entry.pane
                        )
                        .into());
                    }
                }
            }
        }
        let existing = self
            .infos
            .iter()
            .find(|pane| {
                entry.session_id.is_some()
                    && pane.kind == entry.kind
                    && pane.session_id == entry.session_id
            })
            .map(|pane| pane.id);
        let id = if let Some(id) = existing {
            id
        } else {
            let id =
                self.spawn_pane(Some(&cwd), None, command.as_deref(), Vec::new(), spawned_by)?;
            if let Some(pane) = self.infos.iter_mut().find(|pane| pane.id == id) {
                pane.kind = entry.kind;
                pane.session_id = entry.session_id.clone();
                if entry.kind.is_some() {
                    pane.title = entry.name.clone();
                }
            }
            id
        };
        self.history.consume(&entry.key);
        self.history
            .save(&self.config.state_dir.join("tab-history.json"))?;
        self.records.retain(|e| e.key != entry.key);
        self.dirty = true;
        Ok((id, existing.is_some()))
    }
    /// Leased panes have their lease's size; the others the base size.
    fn resize(&mut self) {
        let base = self.base_size();
        for (id, pane) in &mut self.panes {
            let (cols, rows) = self.leases.get(id).map_or(base, |lease| lease.size);
            pane.resize(cols.max(1), rows.max(1));
        }
        self.apply_theme();
    }
    fn diagnostics(&self) -> super::diagnostics::Report {
        super::diagnostics::Report {
            version: env!("CARGO_PKG_VERSION").into(),
            protocol: super::stream::VERSION,
            cols: self.config.cols,
            rows: self.config.rows,
            panes: self.panes.len(),
            clients: self.clients.len(),
            uptime_seconds: self.started.elapsed().as_secs(),
            pty_bytes: self.pty_bytes,
        }
    }
    fn message(&mut self, id: u64, message: ClientMessage) -> Result<()> {
        let request = match message {
            ClientMessage::Stream(request) => request,
            ClientMessage::Diagnostics => {
                let reply = if self.clients[&id].stream.is_some() {
                    ServerMessage::Error(
                        "stream peers request diagnostics with the stream diagnostics request"
                            .into(),
                    )
                } else {
                    ServerMessage::Diagnostics(self.diagnostics())
                };
                return self.clients.get_mut(&id).unwrap().send(reply);
            }
        };
        let correlation = request.correlation().map(str::to_owned);
        let result = if correlation
            .as_deref()
            .is_some_and(|request| !super::stream::valid_request(request))
        {
            Err("invalid request identifier".into())
        } else {
            self.stream_request(id, request)
        };
        if let Err(error) = result {
            if let Some(client) = self.clients.get_mut(&id) {
                client.send(ServerMessage::Stream(super::stream::Event::Error {
                    message: error.to_string(),
                    request: correlation.filter(|r| super::stream::valid_request(r)),
                }))?;
            }
        }
        Ok(())
    }
    fn poll_ptys(&mut self) {
        let mut clipboard = vec![];
        let mut exited = vec![];
        let mut programs = BTreeSet::new();
        for (id, pane) in &mut self.panes {
            let mut buf = [0; 8192];
            for _ in 0..16 {
                match pane.master.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        self.pty_bytes = self.pty_bytes.saturating_add(n as u64);
                        self.dirty = true;
                        if let Some(info) = self.infos.iter_mut().find(|info| info.id == *id) {
                            info.last_activity = crate::now();
                        }
                        for effect in pane.terminal.feed(&buf[..n]) {
                            match effect {
                                Effect::Program(event) => {
                                    pane.program_event(event);
                                    programs.insert(*id);
                                }
                                Effect::WritePty(bytes) => {
                                    let _ = pane.queue(&bytes);
                                }
                                Effect::Title(title) => {
                                    pane.terminal_title = (!title.is_empty())
                                        .then(|| title.chars().take(1024).collect());
                                    // Discovery owns an agent pane's name; OSC cannot race it.
                                    if pane.agent_named {
                                        continue;
                                    }
                                    if let Some(info) = self.infos.iter_mut().find(|p| p.id == *id)
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
                                                info.status == Status::Idle,
                                            )
                                        } else {
                                            title.chars().take(1024).collect()
                                        };
                                    }
                                }
                                Effect::Clipboard(text) if text.len() <= MAX_INPUT => {
                                    clipboard.push((*id, text))
                                }
                                Effect::Bell => {
                                    if let Some(info) =
                                        self.infos.iter().find(|info| info.id == *id)
                                    {
                                        if let Err(error) = self.activity.bell(info, crate::now()) {
                                            eprintln!("kelpserver: notification history: {error}");
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
            let status = if pane.dead {
                None
            } else {
                pane.child.try_wait().ok().flatten()
            };
            if let Some(status) = status {
                use std::os::unix::process::ExitStatusExt;
                pane.exit = Some((status.code(), status.signal()));
                exited.push(*id);
                pane.dead = true;
                pane.pending.clear();
                if let Some(info) = self.infos.iter_mut().find(|p| p.id == *id) {
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
            let (code, signal) = self.panes[&id].exit.unwrap_or_default();
            self.session_event(super::stream::Event::PaneExited {
                pane: id,
                at: crate::now(),
                code,
                signal,
            });
            if let Err(error) = self.remove_pane(id, false) {
                eprintln!("kelpserver: close exited pane %{id}: {error}");
            }
        }
        self.refresh_programs(programs);
        // Clipboard writes reach only the pane's lease owner, if it opted in.
        for (pane, text) in clipboard {
            if let Some(client) = self
                .leases
                .get(&pane)
                .and_then(|lease| self.clients.get_mut(&lease.owner))
            {
                if client
                    .stream
                    .as_ref()
                    .is_some_and(|stream| stream.clipboard)
                {
                    let _ = client.send(ServerMessage::Stream(super::stream::Event::Clipboard {
                        pane,
                        text,
                    }));
                }
            }
        }
    }
    fn refresh_programs(&mut self, ids: impl IntoIterator<Item = PaneId>) {
        for id in ids {
            let Some(mut agent) = self
                .discovered_agents
                .iter()
                .find(|a| a.pane == format!("%{id}"))
                .cloned()
            else {
                continue;
            };
            let Some(info) = self.infos.iter_mut().find(|p| p.id == id) else {
                continue;
            };
            let current = self
                .panes
                .get_mut(&id)
                .and_then(|pane| pane.program.as_mut())
                .is_some_and(|snapshot| {
                    let tree = [agent.pid, snapshot.owner.0]
                        .into_iter()
                        .filter_map(|pid| {
                            process::read(Path::new("/proc"), pid)
                                .ok()
                                .map(|p| (pid, p))
                        })
                        .collect();
                    if snapshot.apply(&mut agent, &tree, crate::now()) {
                        snapshot.agent = Some((agent.pid, agent.process_start));
                        true
                    } else {
                        false
                    }
                });
            if current {
                if info.status != agent.status {
                    info.status_since = crate::now();
                }
                self.dirty |= info.status != agent.status || info.observation != agent.observation;
                info.status = agent.status;
                info.observation = agent.observation;
            } else if info.observation.source == crate::model::StatusSource::Terminal {
                info.status = Status::Unknown;
                info.observation = Default::default();
                info.status_since = crate::now();
                self.dirty = true;
            }
        }
    }
    fn discovery_snapshot(&self) -> DiscoveryRequest {
        let bridge: Vec<_> = self
            .infos
            .iter()
            .enumerate()
            .map(|(index, p)| crate::discovery::Pane {
                id: format!("%{}", p.id),
                window: "@0".into(),
                pid: p.pid,
                index: index + 1,
                cwd: p.cwd.clone(),
                dead: p.dead,
                floating: false,
            })
            .collect();
        DiscoveryRequest {
            programs: self
                .panes
                .iter()
                .filter_map(|(id, pane)| pane.program.clone().map(|p| (*id, p)))
                .collect(),
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
        self.discovered_agents = reconciled.clone();
        for info in &mut self.infos {
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
                info.command = None;
                info.session_id = agent.session_id.clone();
                if let Some(pane) = self.panes.get_mut(&info.id) {
                    let (name, authoritative) =
                        super::tracking::tab_name(agent, pane.terminal_title.as_deref());
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
                let alternate = self
                    .panes
                    .get_mut(&info.id)
                    .is_some_and(|pane| pane.terminal.take_alternate_used());
                let shell = info.pid;
                info.command = super::tracking::track_command(
                    info.command.take(),
                    info.status,
                    alternate,
                    || {
                        let first = process::descendants(&tree, shell)
                            .into_iter()
                            .filter(|pid| *pid != shell)
                            .filter_map(|pid| tree.get(&pid).map(|p| p.start))
                            .min()?;
                        process::started_at(Path::new("/proc"), first)
                    },
                    info.last_activity,
                    crate::now(),
                );
                info.title = super::tracking::shell_tab_name(
                    self.panes
                        .get(&info.id)
                        .and_then(|pane| pane.terminal_title.as_deref()),
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
        self.refresh_programs(self.infos.iter().map(|p| p.id).collect::<Vec<_>>());
        let mut commands = BTreeMap::new();
        for info in &self.infos {
            if info.kind.is_some() {
                continue;
            }
            if let Some(shell) = tree.get(&info.pid) {
                if let Some(app) = tree.values().find(|p| {
                    p.group == shell.foreground
                        && p.pid != info.pid
                        && p.tty == shell.tty
                        && process::descendants(&tree, info.pid).contains(&p.pid)
                }) {
                    let argv = process::args(Path::new("/proc"), app.pid);
                    if argv.len() <= 128 && argv.iter().map(String::len).sum::<usize>() <= 8192 {
                        commands.insert(info.id, argv);
                    }
                }
            }
        }
        let old = self.history.clone();
        let workspace = self
            .config
            .state_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        self.history
            .observe(&self.epoch, &workspace, &self.infos, &commands);
        if self.history != old {
            if let Err(error) = self
                .history
                .save(&self.config.state_dir.join("tab-history.json"))
            {
                eprintln!("kelpserver: closed-session history: {error}");
            }
        }
    }
    fn updates(&mut self) {
        if let Err(error) = self.activity.observe(&self.infos, crate::now()) {
            eprintln!("kelpserver: notification history: {error}");
        }
        // Detached shells still drain PTYs and record notifications, but have
        // no display or metrics consumer. Do not snapshot their screens.
        if !self.clients.values().any(|client| client.stream.is_some()) {
            self.status.poll(false);
            self.dirty = false;
            return;
        }
        let vitals = self.vitals.sample();
        let host_wanted = self
            .clients
            .values()
            .any(|client| client.stream.as_ref().is_some_and(|stream| stream.host));
        let host = if host_wanted {
            Some(Box::new(self.status.poll(true).clone()))
        } else {
            self.status.poll(false);
            None
        };
        self.stream_frames(vitals, host);
        self.dirty = false;
    }
    /// Handle the records a client sent before a write to it failed. A
    /// client that sends a final request (`shutdown`, `close`, input) and
    /// disconnects makes the next write fail, possibly before those records
    /// were read; they must still count.
    fn last_requests(&mut self, id: u64) {
        let Some(client) = self.clients.get_mut(&id) else {
            return;
        };
        let Ok(messages) = client.receive() else {
            return;
        };
        for message in messages {
            if self.message(id, message).is_err() {
                break;
            }
        }
    }
    fn remove_client(&mut self, id: u64) {
        if let Some(client) = self.clients.remove(&id) {
            // Replies held for it are dropped with their slots; operations
            // still running release theirs when they finish.
            self.files.release(id, client.held_slots());
        }
        // Dropping a download cancels its thread.
        self.downloads.active.retain(|_, state| state.client != id);
        self.uploads.disconnect(id);
        self.recovery_waiters.retain(|(client, _)| *client != id);
        let before = self.leases.len();
        self.leases.retain(|_, lease| lease.owner != id);
        if before != self.leases.len() {
            self.resize();
        }
    }
}

struct DiscoveryRequest {
    programs: BTreeMap<PaneId, super::program_status::Snapshot>,
    bridge: Vec<crate::discovery::Pane>,
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
        let mut poller = crate::discovery::polling::Poller::new(Discovery::new(&home));
        while let Ok(request) = receiver.recv() {
            let Ok(cycle) = poller.poll(&request.bridge, &[], crate::now()) else {
                continue;
            };
            let mut agents = cycle.agents;
            for agent in &mut agents {
                if let Some(snapshot) = agent
                    .pane
                    .strip_prefix('%')
                    .and_then(|id| id.parse::<u64>().ok())
                    .and_then(|id| request.programs.get(&id))
                {
                    snapshot.apply(agent, &cycle.tree, crate::now());
                }
            }
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
                tree: cycle.tree,
            };
            if sender.send(result).is_err() {
                break;
            }
        }
    });
    (requests, results, handle)
}

/// Run the server in the foreground until a `shutdown` request or the last
/// pane's exit. It starts without panes; clients create them with `spawn`.
pub fn serve(config: ServerConfig) -> Result<()> {
    super::stream::validate_size(config.cols, config.rows)?;
    raise_open_file_limit();
    let (listener, _endpoint) = endpoint(&config)?;
    super::lifecycle::save_pid(&config.state_dir)?;
    let mut uploads = super::uploads::Uploads::new(&config.state_dir, &config.uploads)?;
    uploads.sweep(crate::now());
    // Terminals do not depend on it: without a root, file operations are
    // refused (`outside_root`) and the server still runs. An extra root
    // that cannot be used is skipped with a warning.
    let file_root = match super::files::Root::with_extras(
        config.files.root.as_deref(),
        &config.files.extra_roots,
    ) {
        Ok((root, warnings)) => {
            for warning in warnings {
                eprintln!("kelpserver: {warning}");
            }
            Some(root)
        }
        Err(error) => {
            eprintln!("kelpserver: file operations are disabled: {error}");
            None
        }
    };
    let mut server = Server {
        history: super::history::History::load(&config.state_dir.join("tab-history.json"))?,
        epoch: fs::read_to_string("/proc/sys/kernel/random/uuid")?
            .trim()
            .to_owned(),
        leases: BTreeMap::new(),
        recovery_job: None,
        recovery_pending: false,
        discovered_agents: Vec::new(),
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
        infos: Vec::new(),
        panes: BTreeMap::new(),
        clients: BTreeMap::new(),
        uploads,
        files: FileJobs::new(file_root),
        downloads: Downloads::default(),
        next_pane: 1,
        spawn_requests: BTreeMap::new(),
        unused_shells: Default::default(),
        records: Vec::new(),
        recovery_waiters: Vec::new(),
        next_client: 1,
        dirty: true,
        shutdown: false,
    };
    server.history.begin_run(crate::now());
    server
        .history
        .save(&server.config.state_dir.join("tab-history.json"))?;
    let unclean = super::lifecycle::begin_run(&server.config.state_dir)?;
    server.recovery_pending = unclean;
    let (requests, results, worker) = discovery_worker(server.config.state_dir.clone());
    let mut discovery = Instant::now() - Duration::from_secs(2);
    let mut render_time = Instant::now() - Duration::from_secs(1);
    let mut metrics_time = Instant::now();
    let mut idle_poll = Instant::now() - Duration::from_millis(100);
    let mut unused_poll = Instant::now() - Duration::from_secs(1);
    // While `accept` lacks descriptors or memory, the listener is left alone
    // until this instant (polling it would wake the loop at once).
    let mut accept_paused: Option<Instant> = None;
    // Logged once per episode of failures, not on every retry.
    let mut accept_failing = false;
    while !server.shutdown {
        if accept_paused.is_some_and(|until| Instant::now() >= until) {
            accept_paused = None;
        }
        for _ in 0..MAX_CLIENTS {
            if accept_paused.is_some() {
                break;
            }
            match listener.accept() {
                Ok((socket, _)) if server.clients.len() < MAX_CLIENTS => {
                    accept_failing = false;
                    if let Ok(client) = Client::new(socket) {
                        let id = server.next_client;
                        server.next_client += 1;
                        server.clients.insert(id, client);
                    }
                }
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                // The pending connection failed or a signal interrupted.
                Err(e)
                    if matches!(
                        e.raw_os_error(),
                        Some(libc::EINTR | libc::ECONNABORTED | libc::EPROTO)
                    ) => {}
                // Transient exhaustion must not stop the server: retry soon.
                Err(e)
                    if matches!(
                        e.raw_os_error(),
                        Some(libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM)
                    ) =>
                {
                    if !accept_failing {
                        eprintln!("kelpserver: accepting a connection: {e}; retrying shortly");
                        accept_failing = true;
                    }
                    accept_paused = Some(Instant::now() + ACCEPT_BACKOFF);
                }
                Err(e) => return Err(e.into()),
            }
        }
        let ids: Vec<_> = server.clients.keys().copied().collect();
        for id in ids {
            let client = server.clients.get_mut(&id).unwrap();
            let messages = match client.receive() {
                Ok(m) => m,
                Err(_) => {
                    server.remove_client(id);
                    continue;
                }
            };
            for message in messages {
                if server.message(id, message).is_err() {
                    server.remove_client(id);
                    break;
                }
            }
            if let Some(client) = server.clients.get_mut(&id) {
                if let Some(error) = client.rejected.take() {
                    if client.send(ServerMessage::Error(error)).is_err() {
                        server.remove_client(id);
                    }
                }
            }
        }
        server.poll_ptys();
        server.poll_recovery();
        server.poll_files();
        server.poll_downloads();
        server.uploads.sweep_due();
        let streaming = server
            .clients
            .values()
            .any(|client| client.stream.is_some());
        // The unused-shell observation is sampled only while a connection
        // receives session data.
        let session_streaming = server.clients.values().any(|client| {
            client
                .stream
                .as_ref()
                .is_some_and(|stream| stream.session_events)
        });
        let idle_interval = if session_streaming {
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
        if !session_streaming && !server.unused_shells.is_empty() {
            // Nobody receives the observation; never deliver a stale one later.
            server.unused_shells.clear();
        }
        // This process-table scan only supplies unused-shell observations.
        // Never scan the whole host ten times a second per workspace.
        if session_streaming && unused_poll.elapsed() >= Duration::from_millis(500) {
            if let Ok(tree) = process::snapshot(Path::new("/proc")) {
                let unused: BTreeSet<PaneId> = server
                    .panes
                    .iter()
                    .filter(|(_, pane)| pane.unused(&tree))
                    .map(|(id, _)| *id)
                    .collect();
                if unused != server.unused_shells {
                    server.unused_shells = unused;
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
        if streaming && metrics_time.elapsed() >= Duration::from_secs(1) {
            server.dirty = true;
            metrics_time = Instant::now();
        }
        if server.dirty && render_time.elapsed() >= Duration::from_millis(33) {
            server.updates();
            render_time = Instant::now();
        }
        let ids: Vec<_> = server.clients.keys().copied().collect();
        for id in ids {
            if server.clients.get_mut(&id).unwrap().flush().is_err() {
                server.last_requests(id);
                server.remove_client(id);
            }
        }
        // poll yields without blocking a disconnected observer or PTY output pump.
        let mut fds = vec![libc::pollfd {
            fd: listener.as_raw_fd(),
            events: if accept_paused.is_some() {
                0
            } else {
                libc::POLLIN
            },
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
                if streaming { 10 } else { 100 },
            );
        }
    }
    // Give queued replies and `server_stopping` a bounded chance to reach
    // their clients before the sockets close.
    let deadline = Instant::now() + Duration::from_millis(200);
    while Instant::now() < deadline {
        server.clients.retain(|_, client| client.flush().is_ok());
        if server
            .clients
            .values()
            .all(|client| client.writing.is_none() && client.output.is_empty())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
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
    super::lifecycle::finish_run(&server.config.state_dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_events_queue_at_most_two_chunks_ahead_of_updates() {
        let (socket, mut peer) = UnixStream::pair().unwrap();
        let mut client = Client::new(socket).unwrap();
        // A framed chunk as large as a download chunk gets.
        let chunk = vec![b'x'; super::super::downloads::CHUNK_BYTES.div_ceil(3) * 4 + 200];
        assert!(client.download_room(chunk.len()));
        client.queue_download(chunk.clone());
        assert!(client.download_room(chunk.len()));
        client.queue_download(chunk.clone());
        // A third waits, whatever the room left in the queue.
        assert!(client.queued + chunk.len() + FILE_REPLY_HEADROOM <= MAX_QUEUED);
        assert!(!client.download_room(chunk.len()));
        assert!(!client.download_room(4096));
        // Other messages are not download bytes.
        client.send(ServerMessage::Error("other".into())).unwrap();
        assert_eq!(client.download_queued, 2 * chunk.len());
        // Once the first chunk is taken for writing, a chunk fits again.
        peer.set_nonblocking(true).unwrap();
        let mut sink = vec![0u8; 1 << 16];
        while client.download_queued == 2 * chunk.len() {
            client.flush().unwrap();
            let _ = peer.read(&mut sink);
        }
        assert_eq!(client.download_queued, chunk.len());
        assert!(client.download_room(chunk.len()));
        // Never ahead of a pending screen update.
        client.update = Some(vec![0; 8]);
        assert!(!client.download_room(16));
    }

    #[test]
    fn ended_downloads_count_against_their_connection_until_their_thread_returns() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let mut downloads = Downloads::default();
        let running = Arc::new(AtomicBool::new(true));
        let thread = super::super::downloads::Thread::from_flag(running.clone());
        downloads.stopping.push((1, thread.clone()));
        downloads.stopping.push((1, thread));
        assert_eq!(downloads.held_by(1), 2);
        assert_eq!(downloads.held_by(2), 0);
        running.store(false, Ordering::Release);
        assert_eq!(downloads.held_by(1), 0);
        assert!(downloads.stopping.is_empty());
    }

    #[test]
    fn a_download_stalls_after_its_timeout_while_it_waits_for_its_client() {
        let timeout = super::super::downloads::STALL_TIMEOUT;
        assert_eq!(timeout, Duration::from_secs(600));
        let start = Instant::now();
        let later = start + timeout;
        assert!(download_stalled(true, start, later));
        assert!(!download_stalled(
            true,
            start,
            later - Duration::from_secs(1)
        ));
        // A download waiting for its own thread (planning a large tree, or
        // reading a slow disk) is not stalled.
        assert!(!download_stalled(false, start, later + timeout));
        // A progress reported after `now` (a racing acknowledgement).
        assert!(!download_stalled(true, later, start));
    }
}
