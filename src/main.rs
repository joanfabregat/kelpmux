use kelpmux::{
    Result,
    discovery::Discovery,
    journal::{Lock, Store, private_dir},
    model::Kind,
    now,
    tmux::Tmux,
    workspace::{self, Grid},
};
use std::{
    env,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

const HELP: &str = "kelpmux — agent-aware tmux workspace controller (Linux)\n\nUsage: kelpmux [--state-dir PATH] [--tmux-socket PATH] COMMAND [OPTIONS]\n\nCommands:\n  daemon [--interval SECONDS]          Discover agents and refresh headers (foreground)\n  scan                                Discover once; update journal and pane options\n  list                                Print discovered live agents as JSON\n  journal                             Print seven-day session history as JSON\n  name --pane %ID NAME                 Override an agent's display name\n  layout COLSxROWS --window @ID [--dry-run]\n                                      Grow or safely shrink a grid\n  zoom --pane %ID --client CLIENT [--width COLS]\n                                      Toggle centered reading view (default 120 columns)\n  fullscreen --pane %ID --client CLIENT\n                                      Open a full-width view\n  modal-action grid|full|restore|select:N --origin @ID\n                                      Internal modal navigation\n  recover-modal --origin @ID           Restore a pane after interrupted modal cleanup\n  event codex|claude [--pane %ID]      Optional hook receiver; JSON on stdin\n  tmux-config                         Print optional tmux integration; does not apply it\n  --help | --version\n\nDiscovery is the default. Hooks are optional. Nothing is installed or started automatically.";

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
    for pane in opts.tmux.panes()? {
        if !store
            .registry
            .agents
            .values()
            .any(|a| a.pane == pane.id && a.ended_at.is_none())
        {
            for name in [
                "@kelpmux-name",
                "@kelpmux-status",
                "@kelpmux-color",
                "@kelpmux-session",
            ] {
                opts.tmux.set(&pane.id, name, "")?;
            }
        }
    }
    Ok(())
}

fn run() -> Result<()> {
    let mut args: Vec<_> = env::args().skip(1).collect();
    if args.is_empty() || args == ["--help"] || args == ["-h"] {
        println!("{HELP}");
        return Ok(());
    }
    if args == ["--version"] {
        println!("kelpmux {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?);
    let socket = option(&mut args, "--tmux-socket")?.map(PathBuf::from);
    let root = option(&mut args, "--state-dir")?
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/state"))
                .join("kelpmux")
        });
    let opts = Options {
        tmux: Tmux { socket },
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
            loop {
                if let Err(error) = scan(&opts) {
                    eprintln!("kelpmux: discovery: {error}");
                }
                std::thread::sleep(Duration::from_secs(interval));
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
            let client = required(&mut args, "--client")?;
            if args.len() != 1 {
                return Err("select requires a pane number".into());
            }
            let index: usize = args[0].parse()?;
            let origin = opts.tmux.format(&window, "#{@kelpmux-origin}")?;
            if origin.starts_with('@') {
                return workspace::modal_action(
                    &opts.tmux,
                    &opts.root,
                    &origin,
                    &format!("select:{index}"),
                );
            }
            if opts
                .root
                .join(format!("modal-{}.json", window.trim_start_matches('@')))
                .exists()
            {
                return workspace::modal_action(
                    &opts.tmux,
                    &opts.root,
                    &window,
                    &format!("select:{index}"),
                );
            }
            let pane = opts
                .tmux
                .panes()?
                .into_iter()
                .find(|p| p.window == window && p.index == index)
                .ok_or("no pane with that number")?;
            workspace::zoom(&opts.tmux, &opts.root, &pane.id, &client, 120, false)?;
        }
        "help-menu" => {
            let client = required(&mut args, "--client")?;
            clean(&args)?;
            let mut arguments = vec![
                "display-menu",
                "-c",
                &client,
                "-T",
                "Kelpmux shortcuts",
                "-x",
                "C",
                "-y",
                "C",
            ];
            for description in [
                "-Read / grid: Ctrl+b z or Alt+Tab / Alt+0",
                "-Read numbered pane: Ctrl+b p, number, Enter",
                "-Switch reading pane: Alt+1 through Alt+9",
                "-Fullscreen: Ctrl+b f or Alt+f",
                "-Restore reading width: Ctrl+b R or Alt+r",
                "-Grid size (from grid): Ctrl+b g",
                "-Rows / columns (from grid): Ctrl+b v / h",
                "-Help: Ctrl+b ? or click ?",
            ] {
                arguments.extend([description, "", ""]);
            }
            opts.tmux.run(&arguments)?;
        }
        "layout-menu" => {
            let window = required(&mut args, "--window")?;
            let client = required(&mut args, "--client")?;
            clean(&args)?;
            let base = base_command(&env::current_exe()?, &opts);
            let mut arguments: Vec<String> = [
                "display-menu",
                "-c",
                &client,
                "-T",
                "Kelpmux grid",
                "-x",
                "C",
                "-y",
                "C",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect();
            for (index, grid) in ["1x1", "2x1", "2x2", "3x2", "2x3", "3x3"]
                .iter()
                .enumerate()
            {
                arguments.extend([
                    grid.to_string(),
                    (index + 1).to_string(),
                    format!(
                        "run-shell -b {}",
                        tmux_quote(&format!(
                            "{base} layout {grid} --window {}",
                            kelpmux::shell_quote(&window)
                        ))
                    ),
                ]);
            }
            opts.tmux
                .run(&arguments.iter().map(String::as_str).collect::<Vec<_>>())?;
        }
        "zoom" | "fullscreen" => {
            let pane = required(&mut args, "--pane")?;
            let client = required(&mut args, "--client")?;
            let width = option(&mut args, "--width")?
                .unwrap_or_else(|| "120".into())
                .parse()?;
            clean(&args)?;
            workspace::zoom(
                &opts.tmux,
                &opts.root,
                &pane,
                &client,
                width,
                command == "fullscreen",
            )?;
        }
        "modal-action" => {
            let origin = required(&mut args, "--origin")?;
            if args.len() != 1 {
                return Err("modal-action requires an action".into());
            }
            workspace::modal_action(&opts.tmux, &opts.root, &origin, &args[0])?;
        }
        "recover-modal" => {
            let origin = required(&mut args, "--origin")?;
            clean(&args)?;
            workspace::recover_modal(&opts.tmux, &opts.root, &origin)?;
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
            clean(&args)?;
            print_config(&env::current_exe()?, &opts)?;
        }
        _ => return Err(format!("unknown command: {command}; use --help").into()),
    }
    Ok(())
}
fn base_command(exe: &Path, opts: &Options) -> String {
    use kelpmux::shell_quote;
    let base = format!(
        "{} --state-dir {}",
        shell_quote(&exe.to_string_lossy()),
        shell_quote(&opts.root.to_string_lossy())
    );
    if let Some(socket) = &opts.tmux.socket {
        format!(
            "{base} --tmux-socket {}",
            shell_quote(&socket.to_string_lossy())
        )
    } else {
        base
    }
}
fn print_config(exe: &Path, opts: &Options) -> Result<()> {
    let base = base_command(exe, opts);
    let header = kelpmux::tmux::HEADER_FORMAT;
    let style = kelpmux::tmux::HEADER_STYLE;
    println!(
        "# Optional Kelpmux integration. Review before sourcing; it replaces the listed bindings.\nset -g pane-base-index 1\nset -g window-style 'fg=colour248,bg=default'\nset -g window-active-style 'fg=colour255,bg=default'\nset -g pane-border-lines spaces\nset -g pane-border-status top\nset -g pane-border-format '{header}'\nset -g pane-border-style '{style}'\nset -g pane-active-border-style '{style},bold'\nset -g status-left '#[range=user|kelpmux-help] ? #[norange]'\nset -g status-left-length 5"
    );
    let zoom = tmux_quote(&format!(
        "{base} zoom --pane '#{{pane_id}}' --client '#{{client_name}}'"
    ));
    let full = tmux_quote(&format!(
        "{base} fullscreen --pane '#{{pane_id}}' --client '#{{client_name}}'"
    ));
    let menu = tmux_quote(&format!(
        "{base} layout-menu --window '#{{window_id}}' --client '#{{client_name}}'"
    ));
    for key in ["M-Tab", "M-0"] {
        println!("bind -n {key} run-shell -b {zoom}");
    }
    println!("bind -n M-f run-shell -b {full}");
    println!("bind z run-shell -b {zoom}");
    println!("bind f run-shell -b {full}");
    println!(
        "bind R run-shell -b {}",
        tmux_quote(&format!(
            "{base} modal-action restore --origin '#{{?@kelpmux-origin,#{{@kelpmux-origin}},#{{window_id}}}}'"
        ))
    );
    println!(
        "bind p command-prompt -p 'Read pane:' {}",
        tmux_quote(&format!(
            "run-shell -b {}",
            tmux_quote(&format!(
                "{base} select %1 --window '#{{window_id}}' --client '#{{client_name}}'"
            ))
        ))
    );
    for n in 1..=9 {
        println!(
            "bind -n M-{n} run-shell -b {}",
            tmux_quote(&format!(
                "{base} select {n} --window '#{{window_id}}' --client '#{{client_name}}'"
            ))
        );
    }
    println!("bind g run-shell -b {menu}");
    let help = tmux_quote(&format!("{base} help-menu --client '#{{client_name}}'"));
    println!("bind ? run-shell -b {help}");
    println!(
        "bind -n MouseDown1Status if-shell -F '#{{==:#{{mouse_status_range}},kelpmux-help}}' {{ run-shell -b {help} }} {{ select-window -t = }}"
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
