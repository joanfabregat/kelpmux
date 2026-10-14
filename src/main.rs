use kelpmux::{
    Result,
    discovery::Discovery,
    journal::{Lock, Store, private_dir},
    model::{Kind, Status},
    now,
    tmux::Tmux,
    workspace::{self, Grid, Reading},
};
use std::{
    env,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const HELP: &str = r#"kelpmux — agent metadata and safe workspace helpers for tmux (Linux)

Usage: kelpmux [--state-dir PATH] [--tmux-socket PATH] [--tmux-binary PATH] COMMAND

Commands:
  daemon [--interval SECONDS]       Discover agents and update native pane options
  scan                             Discover once; update journal and pane options
  list                             Print current agent metadata as JSON
  journal                          Print seven-day session history as JSON
  name --pane %ID NAME              Override an agent's display name
  layout COLSxROWS --window @ID [--dry-run]
                                   Expand or safely shrink a grid
  read --pane %ID [--width COLS]    Toggle native floating reading view (default 120)
  fullscreen --pane %ID             Full-width native zoom of reading pane
  fullscreen-toggle --pane %ID      Toggle fullscreen / capped reading width
  close --pane %ID                  Return reading pane to grid; never kill it
  restore --pane %ID                Restore capped reading width
  grid --window @ID                 Return reading panes to their saved grid tiles
  select N --window @ID             Read a numbered pane without changing its index
  reading-resize --window @ID       Refit reading geometry after a native window resize
  event codex|claude [--pane %ID]   Optional hook receiver; JSON on stdin
  tmux-config [--bindings-only]     Print an optional example; does not apply it
  --help | --version

Native reading view requires tmux 3.8+. tmux owns terminals, rendering, and focus.
Discovery is the default; hooks are optional. Setup and startup remain explicit."#;

struct Options {
    tmux: Tmux,
    root: PathBuf,
    home: PathBuf,
}
fn option(args: &mut Vec<String>, name: &str) -> Result<Option<String>> {
    if let Some(index) = args.iter().position(|a| a == name) {
        args.remove(index);
        if index >= args.len() || args[index].starts_with("--") {
            return Err(format!("{name} needs a value").into());
        }
        return Ok(Some(args.remove(index)));
    }
    Ok(None)
}
fn required(args: &mut Vec<String>, name: &str) -> Result<String> {
    option(args, name)?.ok_or_else(|| format!("{name} is required").into())
}
fn clean(args: &[String]) -> Result<()> {
    if !args.is_empty() {
        Err(format!("unexpected arguments: {}", args.join(" ")).into())
    } else {
        Ok(())
    }
}
fn scan(opts: &Options) -> Result<()> {
    workspace::cleanup_reading_slots(&opts.tmux, &opts.root)?;
    let agents = Discovery::new(&opts.home).scan(&opts.tmux.panes()?, now())?;
    let mut store = Store::open(&opts.root)?;
    store.reconcile(agents, now())?;
    for agent in store
        .registry
        .agents
        .values()
        .filter(|a| a.ended_at.is_none())
    {
        opts.tmux.header(agent)?;
    }
    // Remove stale agent labels when a shell is reused.
    let processes = kelpmux::process::snapshot(Path::new("/proc"))?;
    for pane in opts.tmux.panes()? {
        opts.tmux.pane_palette(&pane.id)?;
        if !store
            .registry
            .agents
            .values()
            .any(|a| a.pane == pane.id && a.ended_at.is_none())
        {
            for name in ["@kelpmux-name", "@kelpmux-session"] {
                opts.tmux.set(&pane.id, name, "")?;
            }
            let status = if pane.dead {
                Status::Exited
            } else if kelpmux::process::idle(&processes, pane.pid, false) {
                Status::Idle
            } else {
                Status::Working
            };
            opts.tmux.pane_status(&pane.id, status)?;
        }
    }
    Ok(())
}

fn run() -> Result<()> {
    let mut args: Vec<_> = env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "native-event") {
        return kelpmux::native::hooks::run(&args[1..]);
    }
    if args.first().is_some_and(|arg| {
        matches!(
            arg.as_str(),
            "server" | "attach" | "ctl" | "start" | "status" | "stop" | "ssh-entry" | "recover"
        )
    }) {
        return kelpmux::native::cli::run(&args);
    }
    if args.is_empty() || args == ["--help"] || args == ["-h"] {
        println!(
            "{}\n  kelpmux native-event codex|claude [--pane ID] [--state-dir PATH]\n      Optional native lifecycle hook; JSON on stdin.\n\nLegacy tmux compatibility commands:\n{HELP}",
            kelpmux::native::cli::HELP
        );
        return Ok(());
    }
    if args == ["--version"] {
        println!("kelpmux {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?);
    let socket = option(&mut args, "--tmux-socket")?.map(PathBuf::from);
    let binary = option(&mut args, "--tmux-binary")?.map(PathBuf::from);
    let root = option(&mut args, "--state-dir")?
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/state"))
                .join("kelpmux")
        });
    let opts = Options {
        tmux: Tmux { socket, binary },
        root,
        home,
    };
    if args.is_empty() {
        return Err("a command is required".into());
    }
    let command = args.remove(0);
    match command.as_str() {
        "scan" => {
            clean(&args)?;
            scan(&opts)?;
        }
        "daemon" => {
            let interval: u64 = option(&mut args, "--interval")?
                .unwrap_or_else(|| "2".into())
                .parse()?;
            if !(1..=60).contains(&interval) {
                return Err("interval must be 1–60 seconds".into());
            }
            clean(&args)?;
            private_dir(&opts.root)?;
            let _lock = Lock::acquire(&opts.root.join("daemon.lock"), false)?;
            let mut next_scan = Instant::now();
            let mut frame = 0;
            loop {
                if Instant::now() >= next_scan {
                    if let Err(error) = scan(&opts) {
                        eprintln!("kelpmux: discovery: {error}");
                    }
                    next_scan = Instant::now() + Duration::from_secs(interval);
                }
                // Header animation does not rescan processes, transcripts or journal.
                let _ = opts.tmux.animate_working(frame);
                frame = (frame + 1) % kelpmux::model::WORKING_FRAMES.len();
                std::thread::sleep(Duration::from_millis(125));
            }
        }
        "list" => {
            clean(&args)?;
            let store = Store::open(&opts.root)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &store
                        .registry
                        .agents
                        .values()
                        .filter(|a| a.ended_at.is_none())
                        .collect::<Vec<_>>()
                )?
            );
        }
        "journal" => {
            clean(&args)?;
            let store = Store::open(&opts.root)?;
            store.prune(now())?;
            println!("{}", serde_json::to_string_pretty(&store.records()?)?);
        }
        "name" => {
            let pane = required(&mut args, "--pane")?;
            if args.len() != 1 || args[0].is_empty() || args[0].len() > 160 {
                return Err("name requires one nonempty name (maximum 160 bytes)".into());
            }
            let mut store = Store::open(&opts.root)?;
            store.rename(&pane, &args[0])?;
            if let Some(agent) = store
                .registry
                .agents
                .values()
                .find(|a| a.pane == pane && a.ended_at.is_none())
            {
                opts.tmux.header(agent)?;
            }
        }
        "layout" => {
            let window = required(&mut args, "--window")?;
            let dry = if let Some(i) = args.iter().position(|a| a == "--dry-run") {
                args.remove(i);
                true
            } else {
                false
            };
            if args.len() != 1 {
                return Err("layout requires COLSxROWS".into());
            }
            println!(
                "{}",
                workspace::resize(&opts.tmux, &opts.root, &window, Grid::parse(&args[0])?, dry)?
            );
        }
        "select" => {
            let window = required(&mut args, "--window")?;
            // Accepted for compatibility; native reading mode belongs to the
            // window, not a nested client.
            option(&mut args, "--client")?;
            if args.len() != 1 {
                return Err("select requires a pane number".into());
            }
            let index: usize = args[0].parse()?;
            let pane = opts
                .tmux
                .panes()?
                .into_iter()
                .find(|p| p.window == window && p.index == index)
                .ok_or("no pane with that number")?;
            workspace::reading(&opts.tmux, &opts.root, &pane.id, 120, Reading::Toggle)?;
        }
        "help-menu" => {
            let client = required(&mut args, "--client")?;
            clean(&args)?;
            let mut arguments = vec![
                "display-menu",
                "-O",
                "-M",
                "-c",
                &client,
                "-T",
                "Kelpmux shortcuts#[align=right]#[fg=colour231,bg=colour88] × ",
                "-x",
                "C",
                "-y",
                "C",
                "--",
            ];
            for description in [
                "-Read / grid: Ctrl+b z or Alt+Tab / Alt+0",
                "-Switch reading pane: Alt+1 through Alt+9",
                "-Fullscreen / reading width: Ctrl+b f or ⛶ / ▣",
                "-Return to grid: × in reading header",
                "-Grid picker: Ctrl+b g or Grid",
                "-Swap pane: Ctrl+b s, number / click",
                "-Rows / columns (from grid): Ctrl+b v / h",
                "-Markers: ⠋ working ! input ✓ done × failed ○ idle",
                "-Help: click ? again / outside / × to close",
            ] {
                arguments.extend([description, "", ""]);
            }
            opts.tmux.run(&arguments)?;
        }
        "layout-menu" => {
            let window = required(&mut args, "--window")?;
            let client = required(&mut args, "--client")?;
            clean(&args)?;
            // The grid control also returns managed reading panes to their tiles.
            workspace::grid(&opts.tmux, &opts.root, &window)?;
            let base = base_command(&env::current_exe()?, &opts);
            let choices = workspace::grid_choices(&opts.tmux, &window)?;
            let client_geometry = opts.tmux.run(&[
                "display-message",
                "-p",
                "-c",
                &client,
                "#{client_width} #{client_height}",
            ])?;
            let client_dims: Vec<usize> = client_geometry
                .split_whitespace()
                .map(str::parse)
                .collect::<std::result::Result<_, _>>()?;
            let geometry = opts
                .tmux
                .format(&window, "#{window_width} #{window_height}")?;
            let dims: Vec<usize> = geometry
                .split_whitespace()
                .map(str::parse)
                .collect::<std::result::Result<_, _>>()?;
            if dims[0] < kelpmux::picker::WIDTH
                || dims[1] < 17
                || client_dims[0] < 68
                || client_dims[1] < 18
            {
                return Err("grid cards need at least 68 columns and 17 rows".into());
            }
            let source = opts.tmux.format(&window, "#{pane_id}")?;
            let request = format!("@kelpmux-picker-{}", std::process::id());
            opts.tmux
                .run(&["set-option", "-w", "-t", &window, &request, ""])?;
            let command = format!(
                "exec {base} grid-dialog --window {} --request {} --choices {}",
                kelpmux::shell_quote(&window),
                kelpmux::shell_quote(&request),
                kelpmux::shell_quote(&serde_json::to_string(&choices)?)
            );
            let result = (|| -> Result<()> {
                let pane = opts.tmux.run(&[
                    "new-pane",
                    "-O",
                    "-K",
                    "-C",
                    "-D",
                    "-P",
                    "-F",
                    "#{pane_id}",
                    "-B",
                    "none",
                    "-t",
                    &window,
                    "-x",
                    "68",
                    "-y",
                    "17",
                    "-X",
                    &((dims[0] - 68) / 2).to_string(),
                    "-Y",
                    &((dims[1] - 17) / 2).to_string(),
                    &command,
                ])?;
                let start = std::time::Instant::now();
                while opts.tmux.panes()?.iter().any(|p| p.id == pane) {
                    if start.elapsed() > Duration::from_secs(180) {
                        return Err("grid dialog did not close".into());
                    }
                    std::thread::sleep(Duration::from_millis(80));
                }
                let shape = opts
                    .tmux
                    .run(&["show-options", "-wqv", "-t", &window, &request])?;
                if !shape.is_empty() {
                    workspace::resize(
                        &opts.tmux,
                        &opts.root,
                        &window,
                        Grid::parse(&shape)?,
                        false,
                    )?;
                }
                if opts.tmux.panes()?.iter().any(|p| p.id == source) {
                    opts.tmux.run(&["select-pane", "-t", &source])?;
                }
                Ok(())
            })();
            let _ = opts
                .tmux
                .run(&["set-option", "-wu", "-t", &window, &request]);
            result?;
        }
        "grid-dialog" => {
            let window = required(&mut args, "--window")?;
            let request = required(&mut args, "--request")?;
            let choices: Vec<workspace::GridChoice> =
                serde_json::from_str(&required(&mut args, "--choices")?)?;
            clean(&args)?;
            if !request
                .strip_prefix("@kelpmux-picker-")
                .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            {
                return Err("invalid picker request".into());
            }
            let pane = env::var("TMUX_PANE")?;
            opts.tmux
                .run(&["set-option", "-p", "-t", &pane, "pane-border-status", "off"])?;
            opts.tmux
                .run(&["set-option", "-p", "-t", &pane, "remain-on-exit", "off"])?;
            if let Some(index) = kelpmux::picker::choose(&choices)? {
                let grid = choices[index].grid;
                opts.tmux.run(&[
                    "set-option",
                    "-w",
                    "-t",
                    &window,
                    &request,
                    &format!("{}x{}", grid.cols, grid.rows),
                ])?;
            }
        }
        "swap-picker" => {
            let pane = required(&mut args, "--pane")?;
            clean(&args)?;
            if !workspace::valid_id(&pane, '%') {
                return Err("swap picker needs a pane ID".into());
            }
            let window = opts.tmux.format(&pane, "#{window_id}")?;
            workspace::grid(&opts.tmux, &opts.root, &window)?;
            opts.tmux.run(&[
                "display-panes",
                "-d",
                "0",
                "-t",
                &pane,
                &format!("swap-pane -d -s {pane} -t '%%'; select-pane -t {pane}"),
            ])?;
        }
        "close" => {
            let pane = required(&mut args, "--pane")?;
            clean(&args)?;
            workspace::close_reading(&opts.tmux, &opts.root, &pane)?;
        }
        "read" | "zoom" | "fullscreen" | "fullscreen-toggle" | "restore" => {
            let pane = required(&mut args, "--pane")?;
            option(&mut args, "--client")?;
            let width = option(&mut args, "--width")?
                .unwrap_or_else(|| "120".into())
                .parse()?;
            clean(&args)?;
            workspace::reading(
                &opts.tmux,
                &opts.root,
                &pane,
                width,
                match command.as_str() {
                    "fullscreen" => Reading::Fullscreen,
                    "fullscreen-toggle" => Reading::ToggleFullscreen,
                    "restore" => Reading::Restore,
                    _ => Reading::Toggle,
                },
            )?;
        }
        "grid" | "reading-resize" => {
            let window = required(&mut args, "--window")?;
            clean(&args)?;
            if command == "grid" {
                workspace::grid(&opts.tmux, &opts.root, &window)?;
            } else {
                workspace::refresh_reading(&opts.tmux, &opts.root, &window)?;
            }
        }
        "event" => {
            let pane = option(&mut args, "--pane")?
                .or_else(|| env::var("TMUX_PANE").ok())
                .ok_or("hook needs --pane or TMUX_PANE")?;
            if args.len() != 1 {
                return Err("event requires codex or claude".into());
            }
            let kind = match args[0].as_str() {
                "codex" => Kind::Codex,
                "claude" => Kind::Claude,
                _ => return Err("unknown agent type".into()),
            };
            let mut text = String::new();
            std::io::stdin()
                .take(1024 * 1024 + 1)
                .read_to_string(&mut text)?;
            if text.len() > 1024 * 1024 {
                return Err("hook payload too large".into());
            }
            let input = serde_json::from_str(&text)?;
            // A one-shot scan registers a newly launched agent even when the daemon is absent.
            scan(&opts)?;
            Store::open(&opts.root)?.hook(&pane, kind, &input)?;
        }
        "tmux-config" => {
            let bindings_only = if let Some(i) = args.iter().position(|a| a == "--bindings-only") {
                args.remove(i);
                true
            } else {
                false
            };
            clean(&args)?;
            print_config(&env::current_exe()?, &opts, !bindings_only)?;
        }
        _ => return Err(format!("unknown command: {command}; use --help").into()),
    }
    Ok(())
}
fn base_command(exe: &Path, opts: &Options) -> String {
    use kelpmux::shell_quote;
    let mut base = format!(
        "{} --state-dir {}",
        shell_quote(&exe.to_string_lossy()),
        shell_quote(&opts.root.to_string_lossy())
    );
    if let Some(binary) = &opts.tmux.binary {
        base.push_str(&format!(
            " --tmux-binary {}",
            shell_quote(&binary.to_string_lossy())
        ));
    }
    if let Some(socket) = &opts.tmux.socket {
        base.push_str(&format!(
            " --tmux-socket {}",
            shell_quote(&socket.to_string_lossy())
        ));
    }
    base
}
fn print_config(exe: &Path, opts: &Options, appearance: bool) -> Result<()> {
    let base = base_command(exe, opts);
    let header = kelpmux::tmux::header_format();
    let style = kelpmux::tmux::BORDER_STYLE;
    let status_left = concat!(
        "#[fg=colour255,bg=colour24]#[range=user|kelpmux-help] ? #[norange]#[default] ",
        "#[fg=colour16,bg=colour39]#[range=user|kelpmux-grid] Grid #[norange]#[default] ",
        "#{pane_index}: #{=/24/...:#{?@kelpmux-name,#{@kelpmux-name},#{b:pane_current_path}}} ",
        "#{?#{&&:#{@kelpmux-reading-width},#{window_zoomed_flag}},",
        "#[fg=colour16#,bg=colour39]#[range=user|kelpmux-full] #{?@kelpmux-reading-full,▣,⛶} #[norange]",
        "#[fg=colour231#,bg=colour88]#[range=user|kelpmux-close] × #[norange]#[default],}",
    );
    println!("# Optional tmux 3.8+ example. Your tmux configuration owns appearance and bindings.");
    if appearance {
        println!(
            "set -g pane-base-index 1\nset -g window-style 'fg=colour248,bg=default'\nset -g window-active-style 'fg=colour255,bg=default'\nset -g pane-border-lines spaces\nset -g pane-border-status top\nset -g pane-border-format '{header}'\nset -g pane-border-style '{style}'\nset -g pane-active-border-style '{style},bold'\nset -g status-left '{status_left}'\nset -g status-left-length 100"
        );
    }
    let zoom = tmux_quote(&format!("{base} read --pane '#{{pane_id}}'"));
    let full = tmux_quote(&format!("{base} fullscreen-toggle --pane '#{{pane_id}}'"));
    let close = tmux_quote(&format!("{base} close --pane '#{{pane_id}}'"));
    let menu = tmux_quote(&format!(
        "{base} layout-menu --window '#{{window_id}}' --client '#{{client_name}}'"
    ));
    for key in ["M-Tab", "M-0"] {
        println!("bind -n {key} run-shell -b {zoom}");
    }
    // Terminals often encode Option+Right as M-f: leave word editing alone.
    println!("unbind -n M-f\nunbind -n M-r");
    println!("bind -n M-Left send-keys M-Left\nbind -n M-Right send-keys M-Right");
    println!("bind z run-shell -b {zoom}");
    println!("bind f run-shell -b {full}");
    let restore = tmux_quote(&format!("{base} restore --pane '#{{pane_id}}'"));
    println!("bind R run-shell -b {restore}");
    println!("bind p previous-window");
    println!(
        "bind s run-shell -b {}",
        tmux_quote(&format!("{base} swap-picker --pane '#{{pane_id}}'"))
    );
    for press in ["MouseDown1", "DoubleClick1", "TripleClick1"] {
        println!("bind -n {press}Control7 run-shell -b -t = {zoom}");
        println!("bind -n {press}Control8 run-shell -b -t = {full}");
        println!(
            "bind -n {press}Control9 if-shell -F -t = '#{{@kelpmux-reading-width}}' {{ run-shell -b -t = {close} }}"
        );
    }
    for n in 1..=9 {
        println!(
            "bind -n M-{n} run-shell -b {}",
            tmux_quote(&format!("{base} select {n} --window '#{{window_id}}'"))
        );
    }
    println!("bind g run-shell -b {menu}");
    println!(
        "set-hook -g window-resized[80] {}",
        tmux_quote(&format!(
            "run-shell -b {}",
            tmux_quote(&format!("{base} reading-resize --window '#{{window_id}}'"))
        ))
    );
    let help = tmux_quote(&format!("{base} help-menu --client '#{{client_name}}'"));
    println!("bind ? run-shell -b {help}");
    for press in ["MouseDown1", "DoubleClick1", "TripleClick1"] {
        println!(
            "bind -n {press}Status if-shell -F '#{{==:#{{mouse_status_range}},kelpmux-help}}' {{ run-shell -b {help} }} {{ if-shell -F '#{{==:#{{mouse_status_range}},kelpmux-grid}}' {{ }} {{ if-shell -F '#{{==:#{{mouse_status_range}},kelpmux-read}}' {{ run-shell -b {zoom} }} {{ if-shell -F '#{{==:#{{mouse_status_range}},kelpmux-full}}' {{ run-shell -b {full} }} {{ if-shell -F '#{{==:#{{mouse_status_range}},kelpmux-close}}' {{ run-shell -b {close} }} {{ select-window -t = }} }} }} }} }}"
        );
    }
    // Open on release so the opening tap cannot immediately dismiss the picker.
    // Regular menu press/release selection works on touch without pointer hover.
    println!(
        "bind -n MouseUp1Status if-shell -F '#{{==:#{{mouse_status_range}},kelpmux-grid}}' {{ run-shell -b {menu} }}"
    );
    Ok(())
}
fn tmux_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
fn main() {
    if let Err(error) = run() {
        eprintln!("kelpmux: {error}");
        std::process::exit(1);
    }
}
