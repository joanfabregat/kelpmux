//! Headless terminal server commands; no TTY frontend or private web management.
use super::{
    connection::{Session, connect},
    server::{self, ServerConfig},
};
use crate::Result;
use std::path::{Path, PathBuf};
pub const HELP: &str = "KelpServer: server | start | status | stop | ctl list|diagnostics|shutdown | recover | diagnostics | bridge | stream-inspect. Servers start without panes; clients create them with the stream protocol. Use KelpMux for attach, web-bridge and web-relay.";
struct Options {
    socket: PathBuf,
    state: PathBuf,
    shell: PathBuf,
    cwd: PathBuf,
    size: (u16, u16),
    replace_running: bool,
    server_binary: Option<PathBuf>,
    command: Vec<String>,
    background: bool,
    uploads: super::uploads::Settings,
    /// `--files-root`: the folder file operations are confined to.
    files_root: Option<PathBuf>,
    /// `--files-extra-root` (repeatable): further folders file operations
    /// may reach. `None` when the command line names none.
    files_extra_roots: Option<Vec<PathBuf>>,
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
        replace_running: false,
        server_binary: None,
        command: vec![],
        background: false,
        uploads: Default::default(),
        files_root: None,
        files_extra_roots: None,
    };
    let verb = args.first().ok_or("missing native command")?;
    let mut position = 1;
    let mut workspace = None;
    let mut state_explicit = false;
    let mut socket_explicit = false;
    let mut config_path = None;
    let mut shell_explicit = false;
    let mut cwd_explicit = false;
    while position < args.len() {
        let arg = &args[position];
        position += 1;
        if arg == "--background" && verb == "server" {
            result.background = true;
            continue;
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
                "--shell" if matches!(verb.as_str(), "server" | "start") => {
                    result.shell = PathBuf::from(value);
                    shell_explicit = true;
                }
                "--cwd" if matches!(verb.as_str(), "server" | "start") => {
                    result.cwd = PathBuf::from(value);
                    cwd_explicit = true;
                }
                "--size" if matches!(verb.as_str(), "server" | "start") => {
                    result.size = parse_size(value)?;
                    super::stream::validate_size(result.size.0, result.size.1)?;
                }
                "--upload-dir" if matches!(verb.as_str(), "server" | "start") => {
                    result.uploads.dir = Some(PathBuf::from(value));
                }
                "--files-root" if matches!(verb.as_str(), "server" | "start") => {
                    result.files_root = Some(PathBuf::from(value));
                }
                "--files-extra-root" if matches!(verb.as_str(), "server" | "start") => {
                    result
                        .files_extra_roots
                        .get_or_insert_with(Vec::new)
                        .push(PathBuf::from(value));
                }
                "--upload-retention-days" if matches!(verb.as_str(), "server" | "start") => {
                    let days: u32 = value
                        .parse()
                        .map_err(|_| "--upload-retention-days needs a number of days")?;
                    if days > super::uploads::MAX_RETENTION_DAYS {
                        return Err("--upload-retention-days must be at most 3650".into());
                    }
                    result.uploads.retention_days = Some(days);
                }
                // Accepted for compatibility: a server always starts without panes.
                "--startup-panes" if matches!(verb.as_str(), "server" | "start") => {
                    value.parse::<super::stream::StartupPanes>()?;
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
            "status" | "diagnostics" | "ctl" | "bridge" | "stream-inspect"
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
    let settings = if let Some(path) = config_path {
        super::lifecycle::Settings::load(&path)?
    } else if verb == "start" && result.state.join("workspace.json").is_file() {
        super::lifecycle::Settings::load(&result.state.join("workspace.json"))?
    } else {
        super::lifecycle::Settings::default()
    };
    if let Some(shell) = settings.shell {
        if !shell_explicit {
            result.shell = shell;
        }
    }
    // Command-line upload options override the configuration file.
    if result.uploads.dir.is_none() {
        result.uploads.dir = settings.upload_dir;
    }
    if result.uploads.retention_days.is_none() {
        result.uploads.retention_days = settings.upload_retention_days;
    }
    if result.files_root.is_none() {
        result.files_root = settings.files_root;
    }
    // The command line's list replaces the configuration file's whole.
    if result.files_extra_roots.is_none() {
        result.files_extra_roots = settings.files_extra_roots;
    }
    if let Some(cwd) = settings.cwd {
        if !cwd_explicit {
            result.cwd = cwd;
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
    result.socket = absolute(&result.socket)?;
    result.state = absolute(&result.state)?;
    result.shell = absolute(&result.shell)?;
    result.cwd = absolute(&result.cwd)?;
    if let Some(dir) = &result.uploads.dir {
        result.uploads.dir = Some(absolute(dir)?);
    }
    if let Some(root) = &result.files_root {
        result.files_root = Some(absolute(root)?);
    }
    if let Some(roots) = &result.files_extra_roots {
        result.files_extra_roots = Some(
            roots
                .iter()
                .map(|root| absolute(root))
                .collect::<Result<_>>()?,
        );
    }
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
                socket: options.socket,
                state_dir: options.state,
                shell: options.shell,
                cwd: options.cwd,
                cols: options.size.0,
                rows: options.size.1,
                uploads: options.uploads,
                files: super::files::Settings {
                    root: options.files_root,
                    extra_roots: options.files_extra_roots.unwrap_or_default(),
                    ..Default::default()
                },
            })
        }
        "start" => {
            // The server reads them from workspace.json (`--config`) when it
            // next starts.
            if options.uploads != super::uploads::Settings::default() {
                super::lifecycle::private_state(&options.state)?;
                super::lifecycle::persist_upload_settings(
                    &options.state.join("workspace.json"),
                    &options.uploads,
                )?;
            }
            if let Some(root) = &options.files_root {
                super::lifecycle::private_state(&options.state)?;
                super::lifecycle::persist_files_root(&options.state.join("workspace.json"), root)?;
            }
            if let Some(roots) = &options.files_extra_roots {
                super::lifecycle::private_state(&options.state)?;
                super::lifecycle::persist_files_extra_roots(
                    &options.state.join("workspace.json"),
                    roots,
                )?;
            }
            super::lifecycle::start(super::lifecycle::StartRequest {
                socket: &options.socket,
                state: &options.state,
                shell: &options.shell,
                cwd: &options.cwd,
                cols: options.size.0,
                rows: options.size.1,
                replace_running: options.replace_running,
                executable: options.server_binary.as_deref(),
            })?;
            control(&options.socket, &["list".into()])
        }
        "status" => {
            let session = Session::open(&options.socket)?;
            let identity = super::upgrade::identify(&session.socket, &options.state)?;
            eprintln!(
                "KelpServer workspace {}: server {} ({}), client {} (stream protocol {}), PID {}",
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
            drop(session);
            control(&options.socket, &["list".into()])
        }
        "diagnostics" => control(&options.socket, &["diagnostics".into()]),
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
        "stop" => super::lifecycle::stop(&options.socket),
        "recover" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&super::recovery::collect(&options.state)?)?
            );
            Ok(())
        }
        "ctl" => control(&options.socket, &options.command),
        _ => Err("unknown native command".into()),
    }
}
/// Same-user administration over a stream connection.
fn control(path: &Path, args: &[String]) -> Result<()> {
    let [command] = args else {
        return Err("ctl requires one command: list, diagnostics or shutdown".into());
    };
    match command.as_str() {
        "list" => {
            let update = Session::open(path)?.update()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "epoch": update.epoch,
                    "panes": update.panes,
                    "leases": update.leases,
                }))?
            );
            Ok(())
        }
        "diagnostics" => {
            let mut session = Session::open(path)?;
            if !session.has("diagnostics") {
                return Err("this server has no diagnostics request; use its matching client".into());
            }
            let request = session.request_id();
            match session.request(super::stream::Request::Diagnostics { request })? {
                super::stream::Event::Diagnostics { report, .. } => {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                    Ok(())
                }
                _ => Err("server did not return diagnostics".into()),
            }
        }
        "shutdown" => super::lifecycle::stop(path),
        _ => Err(
            "invalid ctl command (list, diagnostics or shutdown); pane commands are stream requests"
                .into(),
        ),
    }
}
