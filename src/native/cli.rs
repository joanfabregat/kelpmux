//! Command line and the TUI's terminal plumbing. TTY state belongs to an
//! attach client, never to the server or its persistent PTYs.
use super::{protocol::*, render::terminal_diff};
use crate::Result;
use std::{
    collections::VecDeque,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub const HELP: &str = "Owned native terminal workspaces (Linux):
  kelpmux start [--workspace NAME] [--config PATH] [--template PATH] [--shell PATH] [--cwd PATH] [--replace-running] [--server-binary PATH] [--upload-dir PATH] [--upload-retention-days DAYS]
  kelpmux ssh-entry [--workspace NAME] [--takeover]
  kelpmux status|stop|recover|diagnostics|templates [--workspace NAME]
  kelpmux bridge [--workspace NAME] [--socket PATH]
  kelpmux web-bridge [--workspace NAME] [--socket PATH] [--state-dir PATH]
  kelpmux web-relay --config PATH   (browser client; see docs/web.md)
  kelpmux stream-inspect [--workspace NAME] [PANE_ID ...]
  kelpmux server [--socket PATH] [--state-dir PATH] [--shell PATH] [--cwd PATH] [--size CxR] [--config PATH] [--upload-dir PATH] [--upload-retention-days DAYS]
  kelpmux attach [--workspace NAME] [--socket PATH] [--takeover] [--current-client]
  kelpmux ctl [--socket PATH] list|new|reclaim ID|shutdown

start owns a detached server; ssh-entry starts or attaches. stop ends its processes.
The server (KelpServer 0.3 or later) starts without panes; the TUI or web client
  creates the tabs from its saved layout or the workspace's template.
--replace-running ends an outdated server's shells/jobs before starting the selected binary.
--upload-dir and --upload-retention-days (default 7, 0 keeps them) set where and how long KelpServer keeps
  files dropped onto terminals; start records them in workspace.json for the server.
attach runs the TUI: a client of the server's pane stream that renders
  locally and keeps its UI state in the workspace's tui-state.json.
  A workspace started by another installed kelpmux runs that binary's attach;
  --current-client skips this retained-client selection.
Custom sockets infer adjacent state; specify --state-dir when state lives elsewhere.
Default workspace: main; socket: $XDG_STATE_HOME/kelpmux/workspaces/main/control.sock
  (falls back to ~/.local/state/kelpmux/workspaces/main/control.sock)
Grid geometry is automatic from unparked tabs and client dimensions.
Attach keys: Ctrl-B d detach, z grid/tabs, f full width, g/e automatic grid,
  a agents, ! attention, n finished, s move, ? help, , settings,
  o overview, b notifications, r recovery, / search, c scratch,
  N next question, P next completion, Space selection, Enter new, p park, u parked.
Configurable in --config JSON. Scratch is persistent until explicitly closed.";
pub(crate) const ESCAPE_TIMEOUT: Duration = Duration::from_millis(35);
// JSON encodes byte arrays as decimal integers: a 64 KiB paste can exceed
// 256 KiB on the wire even when the raw payload is within its limit.
const MAX_PENDING: usize = 512 * 1024;
pub(crate) static INTERRUPTED: AtomicBool = AtomicBool::new(false);
pub(crate) static RESIZED: AtomicBool = AtomicBool::new(false);
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
    /// Skips retained-client selection; the handshake stays mandatory.
    current_client: bool,
    replace_running: bool,
    server_binary: Option<PathBuf>,
    command: Vec<String>,
    background: bool,
    /// `workspace.json` or `--config`, with `--template` applied.
    settings: super::config::Settings,
    /// KelpServer's upload options (`--upload-dir`,
    /// `--upload-retention-days`) of `start` and `server`.
    uploads: super::uploads::Settings,
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
        replace_running: false,
        server_binary: None,
        command: vec![],
        background: false,
        settings: super::config::Settings::default(),
        uploads: super::uploads::Settings::default(),
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
        // Accepted without effect for one release, so existing scripts and
        // SSH forced commands keep working: the stream TUI is the only TUI.
        if arg == "--stream" && matches!(verb.as_str(), "attach" | "ssh-entry") {
            continue;
        }
        if arg == "--current-client" && matches!(verb.as_str(), "attach" | "ssh-entry") {
            result.current_client = true;
            continue;
        }
        if arg == "--legacy" && matches!(verb.as_str(), "attach" | "ssh-entry") {
            return Err("the composed TUI was removed; attach without --legacy (a workspace's own registered kelpmux keeps its TUI)".into());
        }
        if arg == "--replace-running" && verb == "start" {
            result.replace_running = true;
            continue;
        }
        if arg.starts_with("--") {
            let value = args.get(position).ok_or("option needs a value")?;
            position += 1;
            match arg.as_str() {
                "--server-binary" if verb == "start" => {
                    result.server_binary = Some(PathBuf::from(value))
                }
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
                // Accepted for existing scripts: every server starts without
                // panes. `layout` fails with KelpServer's explanation.
                "--startup-panes" if matches!(verb.as_str(), "server" | "start" | "ssh-entry") => {
                    value.parse::<super::stream::StartupPanes>()?;
                }
                "--size" if matches!(verb.as_str(), "server" | "start" | "ssh-entry") => {
                    result.size = parse_size(value)?;
                    validate_size(result.size.0, result.size.1)?;
                }
                // KelpServer's upload options, validated like KelpServer
                // does: `server` passes them on, `start` records them in
                // `workspace.json` for the server it starts.
                "--upload-dir" if matches!(verb.as_str(), "server" | "start") => {
                    result.uploads.dir = Some(PathBuf::from(value));
                }
                "--upload-retention-days" if matches!(verb.as_str(), "server" | "start") => {
                    let days: u32 = value
                        .parse()
                        .map_err(|_| "--upload-retention-days needs a number of days")?;
                    if days > super::uploads::MAX_RETENTION_DAYS {
                        return Err(format!(
                            "--upload-retention-days must be at most {}",
                            super::uploads::MAX_RETENTION_DAYS
                        )
                        .into());
                    }
                    result.uploads.retention_days = Some(days);
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
            "attach"
                | "ssh-entry"
                | "status"
                | "diagnostics"
                | "ctl"
                | "bridge"
                | "web-bridge"
                | "stream-inspect"
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
    let settings_path = if let Some(path) = config_path {
        result.settings = super::config::Settings::load(&path)?;
        result.settings.ui_config()?;
        Some(path)
    } else if matches!(verb.as_str(), "start" | "ssh-entry")
        && result.state.join(super::config::FILE).is_file()
    {
        let path = result.state.join(super::config::FILE);
        result.settings = super::config::Settings::load(&path)?;
        Some(path)
    } else {
        None
    };
    // Like `kelpserver start`: command-line upload options override the
    // configuration file's (`upload_dir`, `upload_retention_days`).
    if verb == "start" {
        if let Some(path) = &settings_path {
            let server = super::lifecycle::Settings::load(path)?;
            if result.uploads.dir.is_none() {
                result.uploads.dir = server.upload_dir;
            }
            if result.uploads.retention_days.is_none() {
                result.uploads.retention_days = server.upload_retention_days;
            }
        }
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
        result.settings.template = Some(super::templates::Template::load(&absolute(&path)?)?);
    }
    result.socket = absolute(&result.socket)?;
    result.state = absolute(&result.state)?;
    result.shell = absolute(&result.shell)?;
    result.cwd = absolute(&result.cwd)?;
    if let Some(dir) = &result.uploads.dir {
        result.uploads.dir = Some(absolute(dir)?);
    }
    Ok(result)
}
/// Serves the embedded browser client through the generic relay. A build
/// without `web/dist` has no assets and refuses to start rather than serving
/// a page that cannot work.
fn web_relay(config: &Path, assets: super::web_relay::Assets) -> Result<()> {
    if !assets.iter().any(|(path, _)| *path == "/index.html") {
        return Err("this kelpmux build has no web client assets: build web/dist (see docs/web.md), then rebuild kelpmux".into());
    }
    super::web_relay::run_with_assets(config, assets)
}
/// Starts the workspace server for `kelpmux start` and `ssh-entry`.
fn start_server(options: &Options) -> Result<()> {
    let selected_server = options
        .server_binary
        .clone()
        .or_else(|| std::env::var_os("KELPSERVER_BINARY").map(PathBuf::from))
        .unwrap_or(std::env::current_exe()?.with_file_name("kelpserver"));
    // KelpServer records only `shell` and `cwd` in `workspace.json`. The
    // template and UI settings are KelpMux's keys of the same file, saved
    // like before only when this start launches (or replaces) the server; a
    // running workspace keeps its configuration.
    let launching = options.replace_running || connect(&options.socket).is_err();
    if launching {
        if let Some(template) = &options.settings.template {
            template.validate()?;
        }
    }
    // Upload options are recorded like `kelpserver start` does, whether or
    // not a server is running: the server reads them from `workspace.json`
    // (its `--config`) when it next starts.
    if options.uploads != super::uploads::Settings::default() {
        super::lifecycle::private_state(&options.state)?;
        super::lifecycle::persist_upload_settings(
            &options.state.join(super::config::FILE),
            &options.uploads,
        )?;
    }
    // The server starts without panes: the TUI or web client that connects
    // creates the workspace's tabs.
    super::lifecycle::start(super::lifecycle::StartRequest {
        socket: &options.socket,
        state: &options.state,
        shell: &options.shell,
        cwd: &options.cwd,
        cols: options.size.0,
        rows: options.size.1,
        replace_running: options.replace_running,
        executable: Some(&selected_server),
    })?;
    if launching {
        options.settings.persist(&options.state)?;
    }
    Ok(())
}
/// Runs the TUI until it detaches, the server stops or a signal ends it.
fn attach(options: &Options) -> Result<()> {
    // Before raw mode: a workspace started by another installed kelpmux
    // runs that binary's own attach (exec, does not return).
    let socket = if options.current_client {
        connect(&options.socket)
    } else {
        super::upgrade::attach_matching(&options.socket, &options.state, options.takeover)
    };
    let result =
        socket.and_then(|socket| super::tui::attach::run(socket, &options.state, options.takeover));
    if INTERRUPTED.load(Ordering::Relaxed) {
        Ok(())
    } else {
        result
    }
}
/// `kelpmux start --state-dir STATE --socket SOCKET`: the web bridge's start
/// of its own, configured workspace. Same option handling and start path as
/// the command line, with nothing taken from the browser: never replaces a
/// running server and never chooses another server binary.
pub fn start_workspace(state: &Path, socket: &Path) -> Result<()> {
    let args = [
        "start".to_owned(),
        "--state-dir".to_owned(),
        state
            .to_str()
            .ok_or("workspace path must be UTF-8")?
            .to_owned(),
        "--socket".to_owned(),
        socket
            .to_str()
            .ok_or("workspace socket must be UTF-8")?
            .to_owned(),
    ];
    let options = options(&args)?;
    debug_assert!(!options.replace_running && options.server_binary.is_none());
    start_server(&options)
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
    if args.first().is_some_and(|arg| arg == "web-relay") {
        if args.len() != 3 || args[1] != "--config" {
            return Err("usage: kelpmux web-relay --config PATH".into());
        }
        return web_relay(Path::new(&args[2]), super::web_assets::ASSETS);
    }
    if args.first().is_some_and(|arg| arg == "web-bridge") {
        let (socket, state) = super::web_bridge::paths(&args[1..])?;
        return super::web_bridge::run(&socket, &state);
    }
    let options = options(args)?;
    match args[0].as_str() {
        "server" => kelpserver::native::cli::run(args),
        "start" | "ssh-entry" => {
            start_server(&options)?;
            if args[0] == "ssh-entry" {
                attach(&options)
            } else {
                control(&options.socket, &["list".into()])
            }
        }
        "status" => {
            let stream = connect(&options.socket)?;
            let identity = super::upgrade::identify(&stream, &options.state)?;
            eprintln!(
                "Kelpmux workspace {}: server {} ({}), client {} (stream protocol {}), PID {}",
                options
                    .state
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                identity.version,
                identity.protocol_label(),
                env!("CARGO_PKG_VERSION"),
                super::stream::VERSION,
                identity.pid
            );
            control(&options.socket, &["list".into()])
        }
        "diagnostics" => diagnostics(&options.socket),
        "templates" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&super::templates::list(&options.state)?)?
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
        "attach" => attach(&options),
        "ctl" => control(&options.socket, &options.command),
        _ => Err("unknown native command".into()),
    }
}
pub use kelpserver::native::connection::{connect, response};
/// `kelpmux ctl`: same-user administration through the stream session API.
/// Tab focus, parking and reading modes are each client's own UI state
/// (ADR 0002), so `ctl` only lists, creates, reclaims and stops.
fn control(path: &Path, args: &[String]) -> Result<()> {
    use super::{
        session::{Session, request_id},
        stream::{Event, Request},
    };
    let [command, rest @ ..] = args else {
        return Err("ctl requires a command".into());
    };
    let id = || -> Result<u64> {
        if rest.len() != 1 {
            return Err("command requires one pane ID".into());
        }
        Ok(rest[0].parse()?)
    };
    if command == "diagnostics" && rest.is_empty() {
        return diagnostics(path);
    }
    if !matches!(
        (command.as_str(), rest.is_empty()),
        ("list" | "new" | "shutdown", true) | ("reclaim", false)
    ) {
        return Err("invalid ctl command; see kelpmux --help".into());
    }
    let mut session = Session::open(connect(path)?)?;
    let value = match command.as_str() {
        "list" => {
            let update = session.sessions()?;
            serde_json::json!({
                "epoch": session.epoch,
                "server_version": session.server_version,
                "panes": update.panes,
                "sessions": update.sessions,
                "leases": update.leases,
            })
        }
        "new" => {
            session.require("pane-lifecycle")?;
            let Event::Spawned { pane, .. } = session.call(Request::Spawn {
                request: request_id("ctl-new"),
                cwd: None,
                argv: Vec::new(),
                shell: None,
            })?
            else {
                return Err("terminal server did not identify the created pane".into());
            };
            serde_json::json!({ "pane": pane })
        }
        "reclaim" => {
            // Returns a pane another client sized to the base geometry.
            let pane = id()?;
            session.claim(pane)?;
            session.release(pane)?;
            serde_json::json!({ "pane": pane })
        }
        _ => {
            session.require("server-lifecycle")?;
            session.call(Request::Shutdown {
                request: request_id("ctl-shutdown"),
            })?;
            // Return once the server has closed its sockets.
            session.wait_closed(std::time::Duration::from_secs(5));
            serde_json::json!({ "stopped": true })
        }
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
/// `kelpmux diagnostics`: the server's same-user diagnostics report, a
/// management request outside the stream.
fn diagnostics(path: &Path) -> Result<()> {
    let mut socket = connect(path)?;
    write_message(&mut socket, &ClientMessage::Diagnostics)?;
    match response(&mut socket)? {
        report @ ServerMessage::Diagnostics(_) => {
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        ServerMessage::Error(error) => Err(error.into()),
        _ => Err("native server did not return diagnostics".into()),
    }
}
pub(crate) fn tty_size() -> Result<(u16, u16)> {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if size.ws_col == 0 || size.ws_row == 0 || size.ws_col > 1000 || size.ws_row > 500 {
        return Err("TTY size must be 1–1000 columns and 1–500 rows".into());
    }
    Ok((size.ws_col, size.ws_row))
}
pub(crate) fn backend_size(size: (u16, u16)) -> (u16, u16) {
    (size.0.clamp(40, 400), size.1.clamp(10, 160))
}
pub(crate) struct RawTty {
    original: libc::termios,
    flags: i32,
    actions: Vec<(i32, libc::sigaction)>,
    entered: bool,
}
pub(crate) struct TerminalWriter;
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
    pub(crate) fn enter() -> Result<Self> {
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
            b"\x1b[?1l\x1b>\x1b[?1049h\x1b[?25l\x1b[?1002h\x1b[?1003h\x1b[?1006h\x1b[?1004h\x1b[?2004h\x1b[>1u",
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
                let mut bytes: &[u8]=b"\x1b[<1u\x1b[0m\x1b[?1l\x1b>\x1b[?2004l\x1b[?1004l\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?25h\x1b[?1049l";
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
pub(crate) struct Channel {
    pub(crate) socket: UnixStream,
    input: Vec<u8>,
    pub(crate) output: VecDeque<u8>,
    pub(crate) eof: bool,
}
impl Channel {
    pub(crate) fn new(socket: UnixStream) -> Result<Self> {
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
    pub(crate) fn send(&mut self, message: &ClientMessage) -> Result<()> {
        let bytes = serde_json::to_vec(message)?;
        if bytes.len() > MAX_MESSAGE || self.output.len() + bytes.len() + 4 > MAX_PENDING {
            return Err("native server is not consuming input".into());
        }
        self.output.extend((bytes.len() as u32).to_be_bytes());
        self.output.extend(bytes);
        Ok(())
    }
    pub(crate) fn flush(&mut self) -> Result<()> {
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
    pub(crate) fn receive(&mut self) -> Result<Vec<ServerMessage>> {
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
pub(crate) fn fitted_mouse(frame: &Frame, size: (u16, u16), mut point: Point) -> Option<Point> {
    if frame.sidebar_status() {
        let height = super::workspace::sidebar_status_rows(frame.rows).min(size.1);
        let width = frame.sidebar_width() - 1;
        if point.x < width && point.y >= size.1 - height {
            point.y = frame.rows - height + point.y - (size.1 - height);
        }
    } else if point.y == size.1 - 1 {
        point.y = frame.rows - 1;
        let fixed = (frame.footer_cols() + if frame.controller { 0 } else { 10 })
            .min(size.0)
            .min(frame.cols);
        let tail = size
            .0
            .saturating_sub(fixed)
            .min(frame.cols.saturating_sub(fixed));
        if point.x >= size.0 - tail && point.x < size.0 {
            point.x = frame.cols - tail + point.x - (size.0 - tail);
        } else if point.x >= fixed {
            return None;
        }
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
        let width = (frame.sidebar_width() - 1).min(size.0);
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
            let y = size
                .1
                .saturating_sub(3 * super::workspace::sidebar_button_rows(frame.rows) + 1);
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
        // A clipped observer may have no room after the expanded controls.
        // Always retain an explicit read-only marker in its own viewport.
        if size.0 < frame.footer_cols() + 3 {
            super::render::write_text(
                &mut fit,
                size.0.saturating_sub(3),
                size.1 - 1,
                3,
                "Obs",
                Color::Indexed(250),
                Color::Indexed(super::ui::FOOTER_BG),
                true,
            );
        }
    }
    fit
}
pub(crate) fn queue_events(
    pending: &mut VecDeque<InputEvent>,
    events: Vec<InputEvent>,
) -> Result<()> {
    for event in events {
        let idle_move = |event: &InputEvent| matches!(event, InputEvent::Mouse(mouse) if mouse.kind == MouseKind::Move && mouse.button == 3);
        if idle_move(&event) && pending.back().is_some_and(idle_move) {
            pending.pop_back();
        }
        pending.push_back(event);
    }
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
pub(crate) fn paint(
    stdout: &mut impl Write,
    painted: &mut Option<Frame>,
    frame: &Frame,
    size: (u16, u16),
    notice: Option<&str>,
    hover: Option<Rect>,
) -> Result<()> {
    let mut fit = fitted(frame, size, notice);
    if let Some(area) = hover {
        let strength = super::hover::strength(frame, area);
        for y in 0..fit.rows {
            for x in 0..fit.cols {
                if fitted_mouse(frame, size, Point { x, y })
                    .is_some_and(|point| area.contains(point))
                {
                    super::hover::highlight(
                        &mut fit.cells[usize::from(y * fit.cols + x)],
                        frame.ui.terminal_theme,
                        strength,
                    );
                }
            }
        }
    }
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

#[cfg(test)]
mod fit_tests {
    use super::*;
    #[test]
    fn idle_motion_bursts_coalesce_without_discarding_clicks_or_drags() {
        let mouse = |kind, button, x| {
            InputEvent::Mouse(MouseEvent {
                position: Point { x, y: 1 },
                kind,
                button,
                shift: false,
                alt: false,
                ctrl: false,
            })
        };
        let mut pending = VecDeque::new();
        queue_events(
            &mut pending,
            (0..10000)
                .map(|i| mouse(MouseKind::Move, 3, i % 80))
                .collect(),
        )
        .unwrap();
        assert_eq!(pending.len(), 1);
        queue_events(
            &mut pending,
            vec![
                mouse(MouseKind::Press, 0, 2),
                mouse(MouseKind::Move, 0, 3),
                mouse(MouseKind::Move, 0, 4),
            ],
        )
        .unwrap();
        assert_eq!(pending.len(), 4);
    }

    fn grid_workspace() -> super::super::workspace::Workspace {
        let mut workspace = super::super::workspace::Workspace::new(120, 40).unwrap();
        workspace.panes.push(super::super::protocol::Tab {
            info: super::super::protocol::PaneInfo {
                id: 1,
                ..Default::default()
            },
            number: 1,
            ..Default::default()
        });
        workspace.active = 1;
        workspace.reading = super::super::protocol::ReadingMode::Grid;
        workspace
    }

    #[test]
    fn hover_paint_maps_footer_controls_on_smaller_frontends() {
        let mut ws = grid_workspace();
        ws.ui.config.tab_position = super::super::ui::TabPosition::Left;
        let frame = super::super::render::compose(&ws, &Default::default(), true, None, 0);
        let mut output = Vec::new();
        paint(
            &mut output,
            &mut None,
            &frame,
            (40, 10),
            None,
            Some(frame.controls()[0]),
        )
        .unwrap();
        let mut terminal = super::super::engine::Terminal::new(40, 10);
        terminal.feed(&output);
        assert_eq!(
            terminal.snapshot().cells[9 * 40 + usize::from(frame.controls()[0].x) + 2].bg,
            Color::Rgb(24, 119, 159)
        );
        assert_eq!(
            frame.cells[39 * 120 + usize::from(frame.controls()[0].x) + 2].bg,
            Color::Indexed(24)
        );
    }
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
        let mut workspace = grid_workspace();
        workspace.ui.config.tab_position = super::super::ui::TabPosition::Left;
        workspace.ui.vitals = vec![("cpu 51%".into(), 114), ("clock 12:34".into(), 250)];
        let frame = super::super::render::compose(&workspace, &Default::default(), false, None, 0);
        let fit = fitted(&frame, (40, 10), None);
        let footer: String = fit.cells[9 * 40..]
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert!(footer.starts_with(" ▦ Tabs"));
        assert!(footer.contains("Obs"));
    }
    #[test]
    fn fitted_grid_footer_metrics_open_status() {
        use super::super::{
            input::Router,
            protocol::{Action, InputEvent, MouseEvent, MouseKind, UiMessage},
            status::Sort,
            ui::TabPosition,
        };
        let position = TabPosition::Left;
        for controller in [true, false] {
            let mut workspace = grid_workspace();
            workspace.ui.config.tab_position = position;
            workspace.ui.vitals = vec![("cpu 51%".into(), 114), ("12:34".into(), 250)];
            let frame =
                super::super::render::compose(&workspace, &Default::default(), controller, None, 0);
            for size in [(40, 10), (120, 40), (160, 50)] {
                if size.0 <= frame.footer_cols() + if controller { 0 } else { 10 } {
                    continue;
                }
                let point = fitted_mouse(
                    &frame,
                    size,
                    Point {
                        x: size.0 - 2,
                        y: size.1 - 1,
                    },
                )
                .unwrap();
                assert_eq!(
                    point,
                    Point {
                        x: frame.cols - 2,
                        y: frame.rows - 1
                    }
                );
                let messages = Router::default().route(
                    InputEvent::Mouse(MouseEvent {
                        position: point,
                        kind: MouseKind::Press,
                        button: 0,
                        shift: false,
                        alt: false,
                        ctrl: false,
                    }),
                    &frame,
                );
                assert_eq!(
                    messages.iter().any(|message| matches!(
                        message,
                        UiMessage::Action(Action::StatusOpen(Sort::Cpu))
                    )),
                    controller
                );
                assert_eq!(
                    fitted_mouse(
                        &frame,
                        size,
                        Point {
                            x: 1,
                            y: size.1 - 1
                        }
                    ),
                    Some(Point {
                        x: 1,
                        y: frame.rows - 1
                    })
                );
            }
        }
    }
    #[test]
    fn fitted_footer_keeps_controls_left_vitals_right_and_notices_separate() {
        let mut workspace = grid_workspace();
        workspace.ui.config.tab_position = super::super::ui::TabPosition::Left;
        workspace.ui.vitals = vec![("cpu 51%".into(), 114), ("clock 12:34".into(), 250)];
        let frame = super::super::render::compose(&workspace, &Default::default(), true, None, 0);
        for size in [(40, 10), (120, 40), (160, 50)] {
            for notice in [None, Some("connection notice")] {
                let fit = fitted(&frame, size, notice);
                let row = usize::from((size.1 - 1) * size.0);
                let source = usize::from((frame.rows - 1) * frame.cols);
                assert_eq!(
                    &fit.cells[row..row + usize::from(frame.footer_cols().min(size.0))],
                    &frame.cells[source..source + usize::from(frame.footer_cols().min(size.0))]
                );
                let text: String = fit.cells[row..].iter().map(|c| c.text.as_str()).collect();
                assert!(text.starts_with(" ▦ Tabs"));
                if size.0 >= frame.footer_cols() + 5 {
                    assert!(text.contains(if notice.is_some() { "conn" } else { "51%" }));
                }
                assert!(!text.contains("12:34"));
                let mut terminal = super::super::engine::Terminal::new(size.0, size.1);
                terminal.feed(&super::super::render::terminal_diff(None, &fit));
                assert_eq!(terminal.snapshot().cells, fit.cells);
            }
        }
    }
}

#[cfg(test)]
mod web_tests {
    use super::*;
    #[test]
    fn relay_without_embedded_assets_fails_before_reading_configuration() {
        let error = web_relay(Path::new("/nonexistent/kelpmux-web.json"), &[])
            .unwrap_err()
            .to_string();
        assert!(error.contains("no web client assets"), "{error}");
        let partial: super::super::web_relay::Assets = &[("/favicon.svg", b"<svg/>")];
        assert!(web_relay(Path::new("/nonexistent"), partial).is_err());
    }
}
#[cfg(test)]
mod attach_option_tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Options> {
        options(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    }
    #[test]
    fn stream_is_an_accepted_no_op_and_legacy_is_gone() {
        assert!(parse(&["attach"]).is_ok());
        assert!(parse(&["attach", "--stream"]).is_ok());
        assert!(parse(&["ssh-entry", "--stream"]).is_ok());
        assert!(parse(&["attach", "--current-client"]).is_ok());
        let error = parse(&["attach", "--legacy"]).err().unwrap().to_string();
        assert!(error.contains("composed TUI was removed"), "{error}");
        assert!(parse(&["ssh-entry", "--legacy"]).is_err());
        // Only the attaching verbs take it.
        assert!(parse(&["status", "--stream"]).is_err());
    }
}
