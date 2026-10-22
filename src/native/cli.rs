//! Foreground Linux frontend. TTY state belongs to an attach client, never to
//! the server or its persistent PTYs.
use super::{
    input::{Decoder, Router},
    protocol::*,
    render::terminal_diff,
    server::{self, ServerConfig},
};
use crate::Result;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    collections::VecDeque,
    fs,
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileTypeExt, MetadataExt},
            net::UnixStream,
        },
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub const HELP: &str = "Owned native terminal workspaces (Linux):
  kelpmux start [--workspace NAME] [--config PATH] [--template PATH] [--shell PATH] [--cwd PATH]
  kelpmux ssh-entry [--workspace NAME] [--takeover]
  kelpmux status|stop|recover|diagnostics|templates [--workspace NAME]
  kelpmux benchmark
  kelpmux bridge [--workspace NAME] [--socket PATH]
  kelpmux stream-inspect [--workspace NAME] [PANE_ID ...]
  kelpmux server [--socket PATH] [--state-dir PATH] [--shell PATH] [--cwd PATH] [--size CxR] [--config PATH]
  kelpmux attach [--workspace NAME] [--socket PATH] [--takeover] [--current-client]
  kelpmux ctl [--socket PATH] list|new|park ID|restore ID|focus ID|read ID|fullscreen ID|reclaim ID|close|shutdown

start owns a detached server; ssh-entry starts or attaches. stop ends its processes.
--current-client explicitly skips retained-client selection; the protocol handshake remains mandatory.
Custom sockets infer adjacent state; specify --state-dir when state lives elsewhere.
Default workspace: main; socket: $XDG_STATE_HOME/kelpmux/workspaces/main/control.sock
  (falls back to ~/.local/state/kelpmux/workspaces/main/control.sock)
Grid geometry is automatic from unparked tabs and client dimensions.
Attach keys: Ctrl-B d detach, z grid/tabs, f full width, g/e automatic grid,
  a agents, ! attention, n finished, s move, ? help, , settings,
  o overview, b notifications, r recovery, / search, c scratch,
  N next question, P next completion, Space selection, Enter new, p park, u parked.
Configurable in --config JSON. Scratch is persistent until explicitly closed.";
const TIMEOUT: Duration = Duration::from_secs(3);
const ESCAPE_TIMEOUT: Duration = Duration::from_millis(35);
// JSON encodes byte arrays as decimal integers: a 64 KiB paste can exceed
// 256 KiB on the wire even when the raw payload is within its limit.
const MAX_PENDING: usize = 512 * 1024;
static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static RESIZED: AtomicBool = AtomicBool::new(false);
extern "C" fn signal(signal: libc::c_int) {
    if signal == libc::SIGWINCH {
        RESIZED.store(true, Ordering::Relaxed);
    } else {
        INTERRUPTED.store(true, Ordering::Relaxed);
    }
}

struct Options {
    socket: PathBuf,
    state: PathBuf,
    shell: PathBuf,
    cwd: PathBuf,
    size: (u16, u16),
    takeover: bool,
    current_client: bool,
    command: Vec<String>,
    background: bool,
    settings: super::lifecycle::Settings,
}
fn options(args: &[String]) -> Result<Options> {
    let state = super::lifecycle::workspace_path("main")?;
    let mut result = Options {
        socket: state.join("control.sock"),
        state,
        shell: std::env::var_os("SHELL")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/bin/bash")),
        cwd: std::env::current_dir()?,
        size: (120, 40),
        takeover: false,
        current_client: false,
        command: vec![],
        background: false,
        settings: super::lifecycle::Settings::default(),
    };
    let verb = args.first().ok_or("missing native command")?;
    let mut position = 1;
    let mut workspace = None;
    let mut state_explicit = false;
    let mut socket_explicit = false;
    let mut config_path = None;
    let mut shell_explicit = false;
    let mut cwd_explicit = false;
    let mut template_path = None;
    while position < args.len() {
        let arg = &args[position];
        position += 1;
        if arg == "--background" && verb == "server" {
            result.background = true;
            continue;
        }
        if arg == "--takeover" && matches!(verb.as_str(), "attach" | "ssh-entry") {
            result.takeover = true;
            continue;
        }
        if arg == "--current-client" && matches!(verb.as_str(), "attach" | "ssh-entry") {
            result.current_client = true;
            continue;
        }
        if arg.starts_with("--") {
            let value = args.get(position).ok_or("option needs a value")?;
            position += 1;
            match arg.as_str() {
                "--template" => template_path = Some(PathBuf::from(value)),
                "--socket" => {
                    result.socket = PathBuf::from(value);
                    socket_explicit = true;
                }
                "--state-dir" => {
                    result.state = PathBuf::from(value);
                    state_explicit = true;
                }
                "--workspace" => workspace = Some(value.clone()),
                "--config" => config_path = Some(PathBuf::from(value)),
                "--shell" if matches!(verb.as_str(), "server" | "start" | "ssh-entry") => {
                    result.shell = PathBuf::from(value);
                    shell_explicit = true;
                }
                "--cwd" if matches!(verb.as_str(), "server" | "start" | "ssh-entry") => {
                    result.cwd = PathBuf::from(value);
                    cwd_explicit = true;
                }
                "--size" if matches!(verb.as_str(), "server" | "start" | "ssh-entry") => {
                    result.size = parse_size(value)?;
                    validate_size(result.size.0, result.size.1)?;
                }
                _ => return Err(format!("unknown {verb} option {arg}").into()),
            }
        } else if matches!(verb.as_str(), "ctl" | "stream-inspect") {
            result.command.push(arg.clone());
        } else {
            return Err(format!("unexpected argument {arg}").into());
        }
    }
    // Default custom sockets to adjacent state instead of reading unrelated main
    // metadata/config. Separately stored state requires explicit --state-dir.
    if socket_explicit
        && !state_explicit
        && workspace.is_none()
        && matches!(
            verb.as_str(),
            "attach" | "ssh-entry" | "status" | "diagnostics" | "ctl" | "bridge" | "stream-inspect"
        )
    {
        result.state = result
            .socket
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
    }
    if let Some(workspace) = workspace {
        let path = super::lifecycle::workspace_path(&workspace)?;
        if !state_explicit {
            result.state = path;
        }
    }
    if !socket_explicit {
        result.socket = result.state.join("control.sock");
    }
    if let Some(path) = config_path {
        result.settings = super::lifecycle::Settings::load(&path)?;
    } else if matches!(verb.as_str(), "start" | "ssh-entry")
        && result.state.join("workspace.json").is_file()
    {
        result.settings = super::lifecycle::Settings::load(&result.state.join("workspace.json"))?;
    }
    if let Some(shell) = &result.settings.shell {
        if !shell_explicit {
            result.shell = shell.clone();
        }
    }
    if let Some(cwd) = &result.settings.cwd {
        if !cwd_explicit {
            result.cwd = cwd.clone();
        }
    }
    // Daemon children must not reinterpret relative paths after their caller exits.
    let absolute = |path: &Path| -> Result<PathBuf> {
        if path.is_absolute() {
            Ok(path.to_path_buf())
        } else {
            Ok(std::env::current_dir()?.join(path))
        }
    };
    if let Some(path) = template_path {
        let template = super::templates::Template::load(&absolute(&path)?)?;
        result.settings.grid = template.grid;
        result.settings.tab_count = Some(template.grid.0 * template.grid.1);
        result.settings.grid_weights = [Vec::new(), Vec::new()];
        result.settings.template = Some(template);
    }
    result.socket = absolute(&result.socket)?;
    result.state = absolute(&result.state)?;
    result.shell = absolute(&result.shell)?;
    result.cwd = absolute(&result.cwd)?;
    Ok(result)
}
fn parse_size(value: &str) -> Result<(u16, u16)> {
    let (cols, rows) = value.split_once('x').ok_or("size must be CxR")?;
    Ok((cols.parse()?, rows.parse()?))
}
pub fn run(args: &[String]) -> Result<()> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{HELP}");
        return Ok(());
    }
    let options = options(args)?;
    match args[0].as_str() {
        "server" => {
            if options.background && !super::lifecycle::detach(&options.state)? {
                return Ok(());
            }
            server::serve(ServerConfig {
                initial_weights: options.settings.grid_weights,
                template: options.settings.template,
                socket: options.socket,
                state_dir: options.state,
                shell: options.shell,
                cwd: options.cwd,
                cols: options.size.0,
                rows: options.size.1,
                ui: options.settings.ui,
                initial_grid: options.settings.grid,
                initial_tabs: options.settings.tab_count,
            })
        }
        "start" | "ssh-entry" => {
            super::lifecycle::start(super::lifecycle::StartRequest {
                socket: &options.socket,
                state: &options.state,
                shell: &options.shell,
                cwd: &options.cwd,
                cols: options.size.0,
                rows: options.size.1,
                settings: &options.settings,
            })?;
            if args[0] == "ssh-entry" {
                attach(attach_socket(&options)?, options.takeover)
            } else {
                control(&options.socket, &["list".into()])
            }
        }
        "status" => {
            let stream = connect(&options.socket)?;
            let identity = super::upgrade::identify(&stream, &options.state)?;
            eprintln!(
                "Kelpmux workspace {}: server {} (protocol {}), client {} (protocol {}), PID {}",
                options
                    .state
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                identity.version,
                identity.protocol_label(),
                env!("CARGO_PKG_VERSION"),
                VERSION,
                identity.pid
            );
            control(&options.socket, &["list".into()])
        }
        "diagnostics" => control(&options.socket, &["diagnostics".into()]),
        "templates" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&super::templates::list(&options.state)?)?
            );
            Ok(())
        }
        "benchmark" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&super::diagnostics::benchmark()?)?
            );
            Ok(())
        }
        "bridge" => super::bridge::run(connect(&options.socket)?),
        "stream-inspect" => {
            let panes = options
                .command
                .iter()
                .map(|id| id.parse())
                .collect::<std::result::Result<Vec<u64>, _>>()?;
            let state = super::reference::inspect(connect(&options.socket)?, panes)?;
            println!("{}", serde_json::to_string_pretty(&state)?);
            Ok(())
        }
        "stop" => control(&options.socket, &["shutdown".into()]),
        "recover" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&super::recovery::collect(&options.state)?)?
            );
            Ok(())
        }
        "attach" => {
            let result = attach(attach_socket(&options)?, options.takeover);
            if INTERRUPTED.load(Ordering::Relaxed) {
                Ok(())
            } else {
                result
            }
        }
        "ctl" => control(&options.socket, &options.command),
        _ => Err("unknown native command".into()),
    }
}
pub(crate) fn connect(path: &Path) -> Result<UnixStream> {
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
pub(crate) fn response(socket: &mut UnixStream) -> Result<ServerMessage> {
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
fn control(path: &Path, args: &[String]) -> Result<()> {
    let [command, rest @ ..] = args else {
        return Err("ctl requires a command".into());
    };
    let id = || -> Result<u64> {
        if rest.len() != 1 {
            return Err("command requires one pane ID".into());
        }
        Ok(rest[0].parse()?)
    };
    let message = match command.as_str() {
        "list" if rest.is_empty() => ClientMessage::Inspect,
        "diagnostics" if rest.is_empty() => ClientMessage::Diagnostics,
        "new" if rest.is_empty() => ClientMessage::Command(Action::NewTab),
        "park" => ClientMessage::Command(Action::Park(id()?)),
        "restore" => ClientMessage::Command(Action::Restore(id()?)),
        "focus" => ClientMessage::Command(Action::Focus(id()?)),
        "reclaim" => ClientMessage::Command(Action::ClaimPane(id()?)),
        "read" => ClientMessage::Command(Action::Read(id()?)),
        "fullscreen" => ClientMessage::Command(Action::Fullscreen(id()?)),
        "close" if rest.is_empty() => ClientMessage::Command(Action::CloseReading),
        "shutdown" if rest.is_empty() => ClientMessage::Command(Action::Shutdown),
        _ => return Err("invalid ctl command; see kelpmux --help".into()),
    };
    let mut socket = connect(path)?;
    write_message(&mut socket, &message)?;
    match response(&mut socket)? {
        state @ (ServerMessage::State { .. } | ServerMessage::Diagnostics(_)) => {
            println!("{}", serde_json::to_string_pretty(&state)?);
            Ok(())
        }
        ServerMessage::Error(error) => Err(error.into()),
        _ => Err("native server did not acknowledge the management command".into()),
    }
}
fn tty_size() -> Result<(u16, u16)> {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if size.ws_col == 0 || size.ws_row == 0 || size.ws_col > 1000 || size.ws_row > 500 {
        return Err("TTY size must be 1–1000 columns and 1–500 rows".into());
    }
    Ok((size.ws_col, size.ws_row))
}
fn backend_size(size: (u16, u16)) -> (u16, u16) {
    (size.0.clamp(40, 400), size.1.clamp(10, 160))
}
struct RawTty {
    original: libc::termios,
    flags: i32,
    actions: Vec<(i32, libc::sigaction)>,
    entered: bool,
}
struct TerminalWriter;
impl Write for TerminalWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if INTERRUPTED.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "attach interrupted",
            ));
        }
        let count = unsafe { libc::write(1, bytes.as_ptr().cast(), bytes.len().min(4096)) };
        if count >= 0 {
            return Ok(count as usize);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted && INTERRUPTED.load(Ordering::Relaxed) {
            Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "attach interrupted",
            ))
        } else {
            Err(error)
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl RawTty {
    fn enter() -> Result<Self> {
        INTERRUPTED.store(false, Ordering::Relaxed);
        RESIZED.store(false, Ordering::Relaxed);
        let mut original = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(0, &mut original) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let flags = unsafe { libc::fcntl(0, libc::F_GETFL) };
        if flags < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut guard = Self {
            original,
            flags,
            actions: vec![],
            entered: false,
        };
        for sig in [
            libc::SIGWINCH,
            libc::SIGINT,
            libc::SIGTERM,
            libc::SIGHUP,
            libc::SIGQUIT,
            libc::SIGTSTP,
        ] {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            let mut old = unsafe { std::mem::zeroed() };
            action.sa_sigaction = signal as *const () as usize;
            unsafe {
                libc::sigemptyset(&mut action.sa_mask);
            }
            if unsafe { libc::sigaction(sig, &action, &mut old) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            guard.actions.push((sig, old));
        }
        let mut raw = original;
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        if unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // Mark entered before writing: a partial write still needs restoration.
        guard.entered = true;
        TerminalWriter.write_all(
            b"\x1b[?1l\x1b>\x1b[?1049h\x1b[?25l\x1b[?1002h\x1b[?1006h\x1b[?1004h\x1b[?2004h\x1b[>1u",
        )?;
        TerminalWriter.write_all(b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\")?;
        Ok(guard)
    }
}
impl Drop for RawTty {
    fn drop(&mut self) {
        if self.entered {
            // Never wait indefinitely for a blocked/closed display on exit.
            // Nonblocking mode is temporary and original descriptor flags are
            // restored below, including when stdin/stdout share one OFD.
            let flags = unsafe { libc::fcntl(1, libc::F_GETFL) };
            if flags >= 0 && unsafe { libc::fcntl(1, libc::F_SETFL, flags | libc::O_NONBLOCK) } == 0
            {
                let mut bytes: &[u8]=b"\x1b[<1u\x1b[0m\x1b[?1l\x1b>\x1b[?2004l\x1b[?1004l\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[?25h\x1b[?1049l";
                let deadline = Instant::now() + Duration::from_millis(100);
                while !bytes.is_empty() && Instant::now() < deadline {
                    let count = unsafe { libc::write(1, bytes.as_ptr().cast(), bytes.len()) };
                    if count > 0 {
                        bytes = &bytes[count as usize..];
                        continue;
                    }
                    let error = std::io::Error::last_os_error();
                    if !matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) {
                        break;
                    }
                    let mut poll = libc::pollfd {
                        fd: 1,
                        events: libc::POLLOUT,
                        revents: 0,
                    };
                    unsafe {
                        libc::poll(&mut poll, 1, 10);
                    }
                }
                unsafe {
                    libc::fcntl(1, libc::F_SETFL, flags);
                }
            }
        }
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &self.original);
            libc::fcntl(0, libc::F_SETFL, self.flags);
        }
        for (sig, action) in &self.actions {
            unsafe {
                libc::sigaction(*sig, action, std::ptr::null_mut());
            }
        }
    }
}
struct Channel {
    socket: UnixStream,
    input: Vec<u8>,
    output: VecDeque<u8>,
    eof: bool,
}
impl Channel {
    fn new(socket: UnixStream) -> Result<Self> {
        socket.set_read_timeout(None)?;
        socket.set_write_timeout(None)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            input: vec![],
            output: VecDeque::new(),
            eof: false,
        })
    }
    fn send(&mut self, message: &ClientMessage) -> Result<()> {
        let bytes = serde_json::to_vec(message)?;
        if bytes.len() > MAX_MESSAGE || self.output.len() + bytes.len() + 4 > MAX_PENDING {
            return Err("native server is not consuming input".into());
        }
        self.output.extend((bytes.len() as u32).to_be_bytes());
        self.output.extend(bytes);
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        while !self.output.is_empty() {
            let (bytes, _) = self.output.as_slices();
            match self.socket.write(bytes) {
                Ok(0) => return Err("native server disconnected".into()),
                Ok(n) => {
                    self.output.drain(..n);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
    fn receive(&mut self) -> Result<Vec<ServerMessage>> {
        let mut result = vec![];
        let mut bytes = [0; 8192];
        // Yield after a bounded batch even if the server writes continuously.
        for _ in 0..128 {
            match self.socket.read(&mut bytes) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(n) => {
                    if self.input.len() + n > MAX_MESSAGE + 4 {
                        return Err("native message exceeds the size limit".into());
                    }
                    self.input.extend_from_slice(&bytes[..n]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
            loop {
                if self.input.len() < 4 {
                    break;
                }
                let size = u32::from_be_bytes(self.input[..4].try_into().unwrap()) as usize;
                if size == 0 || size > MAX_MESSAGE {
                    return Err("invalid native message size".into());
                }
                if self.input.len() < size + 4 {
                    break;
                }
                result.push(serde_json::from_slice(&self.input[4..size + 4])?);
                self.input.drain(..size + 4);
            }
        }
        if self.eof && !self.input.is_empty() {
            return Err("native server disconnected mid-message".into());
        }
        Ok(result)
    }
}
fn validate_frame(frame: &Frame) -> Result<()> {
    frame.ui.config.validate()?;
    validate_size(frame.cols, frame.rows)?;
    if frame.cells.len() != usize::from(frame.cols) * usize::from(frame.rows)
        || frame.panes.len() > 10
    {
        return Err("invalid native frame dimensions".into());
    }
    for cell in &frame.cells {
        if cell.width > 2 || cell.text.len() > 1024 || cell.text.chars().any(char::is_control) {
            return Err("invalid native cell".into());
        }
        let mut chars = cell.text.chars();
        if cell.width == 0 {
            if !cell.text.is_empty() {
                return Err("invalid native continuation".into());
            }
        } else if let Some(first) = chars.next() {
            if unicode_width::UnicodeWidthChar::width(first).unwrap_or(0) != usize::from(cell.width)
                || chars.any(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) != 0)
            {
                return Err("invalid native glyph width".into());
            }
        }
    }
    for view in &frame.panes {
        if view.modes.keyboard_flags & !31 != 0 {
            return Err("unsupported native keyboard flags".into());
        }
        for rect in [view.rect, view.body] {
            if u32::from(rect.x) + u32::from(rect.cols) > u32::from(frame.cols)
                || u32::from(rect.y) + u32::from(rect.rows) > u32::from(frame.rows)
            {
                return Err("native pane is outside its frame".into());
            }
        }
    }
    Ok(())
}
fn fitted_mouse(frame: &Frame, size: (u16, u16), mut point: Point) -> Option<Point> {
    if frame.sidebar_status() {
        let height = super::workspace::sidebar_status_rows(frame.rows).min(size.1);
        let width = super::workspace::sidebar_width(frame.cols) - 1;
        if point.x < width && point.y >= size.1 - height {
            point.y = frame.rows - height + point.y - (size.1 - height);
        }
    } else if point.y == size.1 - 1 && point.x < frame.footer_cols().min(frame.cols) {
        point.y = frame.rows - 1;
    } else if point.y >= size.1.saturating_sub(1) {
        return None;
    }
    Some(point)
}
fn fitted(frame: &Frame, size: (u16, u16), notice: Option<&str>) -> Frame {
    let mut fit = frame.clone();
    fit.cols = size.0;
    fit.rows = size.1;
    fit.cells = vec![Cell::default(); usize::from(size.0) * usize::from(size.1)];
    let footer_rows = u16::from(!frame.sidebar_status());
    for y in 0..size
        .1
        .saturating_sub(footer_rows)
        .min(frame.rows.saturating_sub(footer_rows))
    {
        for x in 0..size.0.min(frame.cols) {
            let mut cell =
                frame.cells[usize::from(y) * usize::from(frame.cols) + usize::from(x)].clone();
            if cell.width == 2 && x + 1 >= size.0 {
                cell.width = 1;
                cell.text = " ".into();
            }
            fit.cells[usize::from(y) * usize::from(size.0) + usize::from(x)] = cell;
        }
    }
    if frame.sidebar_status() {
        let height = super::workspace::sidebar_status_rows(frame.rows).min(size.1);
        let width = (super::workspace::sidebar_width(frame.cols) - 1).min(size.0);
        let source_top = frame.rows - super::workspace::sidebar_status_rows(frame.rows);
        let bg = frame.cells[usize::from(source_top * frame.cols)].bg;
        for y in source_top..frame.rows.min(size.1) {
            for x in 0..width {
                fit.cells[usize::from(y * size.0 + x)] = Cell {
                    bg,
                    ..Cell::default()
                };
            }
        }
        for y in 0..height {
            for x in 0..width {
                fit.cells[usize::from((size.1 - height + y) * size.0 + x)] =
                    frame.cells[usize::from((frame.rows - height + y) * frame.cols + x)].clone();
            }
        }
        if let Some(notice) = notice {
            let y = size.1.saturating_sub(3);
            for x in 0..width {
                fit.cells[usize::from(y * size.0 + x)] = Cell {
                    fg: Color::Indexed(231),
                    bg: Color::Indexed(88),
                    ..Cell::default()
                };
            }
            super::render::write_text(
                &mut fit,
                0,
                y,
                width,
                notice,
                Color::Indexed(231),
                Color::Indexed(88),
                false,
            );
        }
        fit.cursor = frame
            .cursor
            .filter(|c| frame.controller && c.position.x < size.0 && c.position.y < size.1);
        return fit;
    }
    let target = usize::from(size.1 - 1) * usize::from(size.0);
    let source = usize::from(frame.rows - 1) * usize::from(frame.cols);
    for cell in &mut fit.cells[target..target + usize::from(size.0)] {
        cell.bg = Color::Indexed(super::ui::FOOTER_BG);
    }
    let controls = frame.footer_cols().min(size.0).min(frame.cols);
    let fixed = (controls + if frame.controller { 0 } else { 10 })
        .min(size.0)
        .min(frame.cols);
    for x in 0..usize::from(fixed) {
        fit.cells[target + x] = frame.cells[source + x].clone();
    }
    let tail = size
        .0
        .saturating_sub(fixed)
        .min(frame.cols.saturating_sub(fixed));
    for x in 0..usize::from(tail) {
        let mut cell = frame.cells[source + usize::from(frame.cols - tail) + x].clone();
        if x == 0 && cell.width == 0 || cell.width == 2 && x + 1 == usize::from(tail) {
            cell = Cell::default();
        }
        fit.cells[target + usize::from(size.0 - tail) + x] = cell;
    }
    if let Some(notice) = notice {
        let text = format!(" {}", super::engine::safe_text(notice));
        let mut x = usize::from(controls);
        for cell in &mut fit.cells[target + usize::from(controls)..target + usize::from(size.0)] {
            *cell = Cell {
                fg: Color::Indexed(231),
                bg: Color::Indexed(88),
                ..Cell::default()
            };
        }
        for c in text.chars() {
            let width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if width == 0 || x + width > usize::from(size.0) {
                continue;
            }
            fit.cells[target + x] = Cell {
                text: c.to_string(),
                width: width as u8,
                fg: Color::Indexed(231),
                bg: Color::Indexed(88),
                ..Cell::default()
            };
            if width == 2 {
                fit.cells[target + x + 1] = Cell {
                    text: String::new(),
                    width: 0,
                    fg: Color::Indexed(231),
                    bg: Color::Indexed(88),
                    ..Cell::default()
                };
            }
            x += width;
        }
    }
    fit.cursor = frame
        .cursor
        .filter(|c| c.position.x < size.0 && c.position.y < size.1.saturating_sub(1));
    if !frame.controller {
        fit.cursor = None;
    }
    fit
}
fn queue_events(pending: &mut VecDeque<InputEvent>, events: Vec<InputEvent>) -> Result<()> {
    pending.extend(events);
    let size: usize = pending
        .iter()
        .map(|event| match event {
            InputEvent::Bytes(bytes) | InputEvent::Paste(bytes) => bytes.len(),
            InputEvent::Mouse(_) | InputEvent::ThemeColor { .. } => 32,
        })
        .sum();
    if size > MAX_PENDING || pending.len() > 4096 {
        return Err("too much input before the first native frame".into());
    }
    Ok(())
}
fn paint(
    stdout: &mut impl Write,
    painted: &mut Option<Frame>,
    frame: &Frame,
    size: (u16, u16),
    notice: Option<&str>,
) -> Result<()> {
    let fit = fitted(frame, size, notice);
    let keypad = |frame: &Frame| {
        frame.controller
            && !frame.help
            && frame
                .panes
                .iter()
                .rev()
                .find(|view| view.pane.id == frame.active)
                .is_some_and(|view| view.modes.application_keypad)
    };
    if painted
        .as_ref()
        .is_none_or(|previous| keypad(previous) != keypad(frame))
    {
        stdout.write_all(if keypad(frame) { b"\x1b=" } else { b"\x1b>" })?;
    }
    stdout.write_all(&terminal_diff(painted.as_ref(), &fit))?;
    stdout.flush()?;
    *painted = Some(fit);
    Ok(())
}
fn attach_socket(options: &Options) -> Result<UnixStream> {
    if options.current_client {
        connect(&options.socket)
    } else {
        super::upgrade::attach_matching(&options.socket, &options.state, options.takeover)
    }
}
fn attach(mut socket: UnixStream, takeover: bool) -> Result<()> {
    if unsafe { libc::isatty(0) } != 1 || unsafe { libc::isatty(1) } != 1 {
        return Err("attach requires stdin and stdout to be TTYs".into());
    }
    let mut size = tty_size()?;
    let (cols, rows) = backend_size(size);
    write_message(
        &mut socket,
        &ClientMessage::Attach {
            version: VERSION,
            cols,
            rows,
            takeover,
        },
    )?;
    match response(&mut socket)? {
        ServerMessage::Welcome { version, .. } if version == VERSION => {}
        ServerMessage::Welcome { version, .. } => return Err(format!(
            "workspace uses native protocol {version}, client uses {VERSION}; attach with the workspace's retained matching client; existing sessions are unchanged"
        ).into()),
        ServerMessage::Error(error) => return Err(format!(
            "native attach failed: {error}; client protocol {VERSION}; existing sessions are unchanged"
        ).into()),
        _ => return Err("incompatible native server handshake".into()),
    }
    // All connection/handshake failures precede raw and alternate-screen mode.
    let mut channel = Channel::new(socket)?;
    let _tty = RawTty::enter()?;
    let mut decoder = Decoder::default();
    let mut router = Router::default();
    let mut latest: Option<Frame> = None;
    let mut painted: Option<Frame> = None;
    let mut escape = None;
    let mut stdout = TerminalWriter;
    let mut notice: Option<String> = None;
    let mut pending = VecDeque::new();
    let mut revision = 0;
    let mut keyboard_mode = 1;
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) {
            return Ok(());
        }
        if RESIZED.swap(false, Ordering::Relaxed) {
            size = tty_size()?;
            painted = None;
            // Server stores observer geometry without resizing the workspace.
            let (cols, rows) = backend_size(size);
            channel.send(&ClientMessage::Resize { cols, rows })?;
            channel.flush()?;
            if let Some(frame) = latest.as_ref() {
                paint(&mut stdout, &mut painted, frame, size, notice.as_deref())?;
            }
        }
        let mut newest = None;
        let mut error_changed = false;
        for message in channel.receive()? {
            match message {
                ServerMessage::Frame(frame) => {
                    validate_frame(&frame)?;
                    revision = 1;
                    newest = Some(frame);
                }
                ServerMessage::Patch(patch) => {
                    let previous = newest
                        .as_ref()
                        .or(latest.as_ref())
                        .ok_or("frame patch arrived before a snapshot")?;
                    let frame = super::delta::apply(previous, revision, &patch)?;
                    validate_frame(&frame)?;
                    revision = patch.revision;
                    newest = Some(frame);
                }
                ServerMessage::Clipboard(text) => {
                    if text.len() <= MAX_INPUT {
                        write!(stdout, "\x1b]52;c;{}\x07", STANDARD.encode(text.as_bytes()))?;
                        stdout.flush()?;
                    }
                }
                // Routine action errors keep the frontend and its PTYs alive.
                ServerMessage::Error(error) => {
                    notice = Some(error.chars().take(1024).collect());
                    error_changed = true;
                }
                ServerMessage::Detached => return Ok(()),
                _ => {}
            }
        }
        if let Some(frame) = newest {
            let mode = if frame.controller && !frame.help && frame.ui.dialog.is_none() {
                frame
                    .panes
                    .iter()
                    .find(|p| p.pane.id == frame.active)
                    .map_or(1, |p| p.modes.keyboard_flags | 1)
            } else {
                1
            };
            if mode != keyboard_mode {
                write!(stdout, "\x1b[={mode}u")?;
                stdout.flush()?;
                keyboard_mode = mode;
            }
            let sound = frame.controller
                && frame.ui.config.notification_sound
                && latest.as_ref().is_some_and(|old| {
                    frame.ui.notifications.iter().any(|event| {
                        !event.read && !old.ui.notifications.iter().any(|seen| seen.id == event.id)
                    })
                });
            paint(&mut stdout, &mut painted, &frame, size, notice.as_deref())?;
            if sound {
                stdout.write_all(b"\x07")?;
                stdout.flush()?;
            }
            latest = Some(frame);
        } else if error_changed {
            if let Some(frame) = latest.as_ref() {
                paint(&mut stdout, &mut painted, frame, size, notice.as_deref())?;
            }
        }
        if channel.eof {
            return Ok(());
        }
        if escape.is_some_and(|time: Instant| time.elapsed() >= ESCAPE_TIMEOUT) {
            queue_events(&mut pending, decoder.flush())?;
            escape = None;
        }
        if let Some(frame) = latest.as_ref() {
            while let Some(mut event) = pending.pop_front() {
                // Footer controls stay at the left edge when fitted.
                // Pane and help-panel coordinates stay in backend geometry.
                if let InputEvent::Mouse(mouse) = &mut event {
                    let Some(point) = fitted_mouse(frame, size, mouse.position) else {
                        continue;
                    };
                    mouse.position = point;
                }
                let messages = router.route(event, frame);
                if !messages.is_empty() && notice.take().is_some() {
                    paint(&mut stdout, &mut painted, frame, size, None)?;
                }
                for message in messages {
                    channel.send(&message)?;
                }
            }
        }
        channel.flush()?;
        let mut polls = [
            libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: channel.socket.as_raw_fd(),
                events: libc::POLLIN
                    | if channel.output.is_empty() {
                        0
                    } else {
                        libc::POLLOUT
                    },
                revents: 0,
            },
        ];
        let result = unsafe { libc::poll(polls.as_mut_ptr(), 2, 10) };
        if result < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(std::io::Error::last_os_error().into());
        }
        if polls[0].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            // Leave the shared TTY file description blocking: stdin/stdout may
            // be dup'd from one descriptor. One read follows a readiness poll.
            let mut bytes = [0; 8192];
            let count = unsafe { libc::read(0, bytes.as_mut_ptr().cast(), bytes.len()) };
            if count == 0 {
                return Ok(());
            }
            if count < 0 {
                let error = std::io::Error::last_os_error();
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) {
                    continue;
                }
                return Err(error.into());
            }
            queue_events(&mut pending, decoder.feed(&bytes[..count as usize]))?;
            escape = Some(Instant::now());
        }
    }
}

#[cfg(test)]
mod fit_tests {
    use super::*;
    #[test]
    fn fitted_sidebar_keeps_controls_bottom_aligned_and_maps_their_clicks() {
        let mut workspace = super::super::workspace::Workspace::new(120, 40).unwrap();
        workspace.ui.config.tab_position = super::super::ui::TabPosition::Left;
        workspace.ui.vitals = vec![("cpu 12%".into(), 114), ("12:34".into(), 250)];
        let frame = super::super::render::compose(&workspace, &Default::default(), false, None, 0);
        for size in [(40, 10), (120, 40), (160, 50)] {
            for notice in [None, Some("connection notice")] {
                let fit = fitted(&frame, size, notice);
                for control in frame.controls() {
                    let point = Point {
                        x: control.x,
                        y: size.1 - (frame.rows - control.y),
                    };
                    assert_eq!(
                        fitted_mouse(&frame, size, point),
                        Some(Point {
                            x: control.x,
                            y: control.y
                        })
                    );
                    assert_eq!(
                        fit.cells[usize::from(point.y * fit.cols + point.x)].bg,
                        frame.cells[usize::from(control.y * frame.cols + control.x)].bg
                    );
                }
                let mut terminal = super::super::engine::Terminal::new(size.0, size.1);
                terminal.feed(&super::super::render::terminal_diff(None, &fit));
                assert_eq!(terminal.snapshot().cells, fit.cells);
            }
        }
    }
    #[test]
    fn fitted_narrow_observer_preserves_observer_marker() {
        let mut workspace = super::super::workspace::Workspace::new(120, 40).unwrap();
        workspace.ui.config.tab_position = super::super::ui::TabPosition::Top;
        workspace.ui.vitals = vec![("clock 12:34".into(), 250)];
        let frame = super::super::render::compose(&workspace, &Default::default(), false, None, 0);
        let fit = fitted(&frame, (40, 10), None);
        let footer: String = fit.cells[9 * 40..]
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert!(footer.starts_with(" ⚙︎ Config"));
        assert!(footer.contains("Obs"));
    }
    #[test]
    fn fitted_footer_keeps_controls_left_vitals_right_and_notices_separate() {
        let mut workspace = super::super::workspace::Workspace::new(120, 40).unwrap();
        workspace.ui.config.tab_position = super::super::ui::TabPosition::Top;
        workspace.ui.vitals = vec![("clock 12:34".into(), 250)];
        let frame = super::super::render::compose(&workspace, &Default::default(), true, None, 0);
        for size in [(40, 10), (120, 40), (160, 50)] {
            for notice in [None, Some("connection notice")] {
                let fit = fitted(&frame, size, notice);
                let row = usize::from((size.1 - 1) * size.0);
                let source = usize::from((frame.rows - 1) * frame.cols);
                assert_eq!(
                    &fit.cells[row..row + usize::from(frame.footer_cols())],
                    &frame.cells[source..source + usize::from(frame.footer_cols())]
                );
                let text: String = fit.cells[row..].iter().map(|c| c.text.as_str()).collect();
                assert!(text.starts_with(" ⚙︎ Config"));
                assert!(text.contains(if notice.is_some() { "conn" } else { "12:34" }));
                let mut terminal = super::super::engine::Terminal::new(size.0, size.1);
                terminal.feed(&super::super::render::terminal_diff(None, &fit));
                assert_eq!(terminal.snapshot().cells, fit.cells);
            }
        }
    }
}
