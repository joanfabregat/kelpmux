//! Headless terminal server commands; no TTY frontend or private web management.
use super::{
    connection::{connect, response},
    protocol::*,
    server::{self, ServerConfig},
};
use crate::Result;
use std::path::{Path, PathBuf};
pub const HELP: &str = "KelpServer: server | start | status | stop | ctl | recover | diagnostics | templates | benchmark | bridge | stream-inspect | web-relay --config PATH. Use KelpMux for attach and KelpHub for web-bridge.";
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
        replace_running: false,
        server_binary: None,
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
        result.settings.tab_count =
            Some(usize::from(template.grid.0) * usize::from(template.grid.1));
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
    if args.first().is_some_and(|arg| arg == "web-relay") {
        if args.len() != 3 || args[1] != "--config" {
            return Err("usage: kelpmux web-relay --config PATH".into());
        }
        return super::web_relay::run(std::path::Path::new(&args[2]));
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
        "start" => {
            super::lifecycle::start(super::lifecycle::StartRequest {
                socket: &options.socket,
                state: &options.state,
                shell: &options.shell,
                cwd: &options.cwd,
                cols: options.size.0,
                rows: options.size.1,
                settings: &options.settings,
                replace_running: options.replace_running,
                executable: options.server_binary.as_deref(),
            })?;
            control(&options.socket, &["list".into()])
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
        "ctl" => control(&options.socket, &options.command),
        _ => Err("unknown native command".into()),
    }
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
