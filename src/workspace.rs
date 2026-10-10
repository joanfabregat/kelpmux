use crate::{
    Result,
    journal::{Lock, atomic, private_dir},
    process, shell_quote,
    tmux::{Pane, Tmux},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub cols: usize,
    pub rows: usize,
}
impl Grid {
    pub fn parse(value: &str) -> Result<Self> {
        let (c, r) = value.split_once('x').ok_or("layout must be COLSxROWS")?;
        let grid = Self {
            cols: c.parse()?,
            rows: r.parse()?,
        };
        if ![(1, 1), (2, 1), (2, 2), (3, 2), (2, 3), (3, 3)].contains(&(grid.cols, grid.rows)) {
            return Err("supported grids: 1x1, 2x1, 2x2, 3x2, 2x3, 3x3".into());
        }
        Ok(grid)
    }
    pub fn count(self) -> usize {
        self.cols * self.rows
    }
}

pub fn checksum(layout: &str) -> u16 {
    layout
        .bytes()
        .fold(0_u16, |sum, b| sum.rotate_right(1).wrapping_add(b as u16))
}
fn sizes(total: usize, count: usize) -> Result<Vec<usize>> {
    let usable = total.checked_sub(count - 1).ok_or("terminal too small")?;
    if usable / count < 3 {
        return Err("terminal too small for requested grid".into());
    }
    Ok((0..count)
        .map(|i| usable / count + usize::from(i < usable % count))
        .collect())
}
pub fn layout(grid: Grid, width: usize, height: usize, panes: &[String]) -> Result<String> {
    if panes.len() != grid.count() {
        return Err("pane count does not match grid".into());
    }
    if panes.iter().any(|p| !valid_id(p, '%')) {
        return Err("invalid pane id".into());
    }
    let widths = sizes(width, grid.cols)?;
    let heights = sizes(height, grid.rows)?;
    let mut rows = vec![];
    let mut y = 0;
    for (r, h) in heights.into_iter().enumerate() {
        let mut cells = vec![];
        let mut x = 0;
        for (c, w) in widths.iter().enumerate() {
            cells.push(format!(
                "{w}x{h},{x},{y},{}",
                &panes[r * grid.cols + c][1..]
            ));
            x += w + 1;
        }
        rows.push(if cells.len() == 1 {
            cells.remove(0)
        } else {
            format!("{width}x{h},0,{y}{{{}}}", cells.join(","))
        });
        y += h + 1;
    }
    let body = if rows.len() == 1 {
        rows.remove(0)
    } else {
        format!("{width}x{height},0,0[{}]", rows.join(","))
    };
    Ok(format!("{:04x},{body}", checksum(&body)))
}
pub fn valid_id(s: &str, prefix: char) -> bool {
    s.starts_with(prefix) && s.len() > 1 && s[1..].bytes().all(|b| b.is_ascii_digit())
}

pub fn shrink_plan(panes: &[Pane], idle: impl Fn(u32) -> bool, target: usize) -> Result<Vec<Pane>> {
    let remove = panes.len().saturating_sub(target);
    let mut candidates: Vec<_> = panes
        .iter()
        .filter(|p| !p.dead && idle(p.pid))
        .cloned()
        .collect();
    candidates.sort_by_key(|p| std::cmp::Reverse(p.index));
    if candidates.len() < remove {
        return Err(format!(
            "shrink refused: need {remove} idle shell panes, found {}",
            candidates.len()
        )
        .into());
    }
    candidates.truncate(remove);
    Ok(candidates)
}

pub fn resize(tmux: &Tmux, root: &Path, window: &str, grid: Grid, dry: bool) -> Result<String> {
    if !valid_id(window, '@') {
        return Err("layout requires a window id such as @0".into());
    }
    private_dir(root)?;
    let _lock = Lock::acquire(&root.join("workspace.lock"), false)?;
    if modal_path(root, window).exists() {
        return Err("return to grid before changing layout".into());
    }
    let mut panes: Vec<_> = tmux
        .panes()?
        .into_iter()
        .filter(|p| p.window == window)
        .collect();
    panes.sort_by_key(|p| p.index);
    if panes.is_empty() {
        return Err("window has no panes".into());
    }
    if tmux.format(window, "#{window_zoomed_flag}")? == "1" {
        return Err("unzoom before changing layout".into());
    }
    let dims = tmux.format(window, "#{window_width} #{window_height}")?;
    let (w, h) = dimensions(&dims)?;
    sizes(w, grid.cols)?;
    sizes(h, grid.rows)?;
    let tree = process::snapshot(Path::new("/proc"))?;
    let victims = shrink_plan(&panes, |pid| process::idle(&tree, pid, false), grid.count())?;
    let summary = format!(
        "{}x{}: {} -> {} panes; remove [{}]",
        grid.cols,
        grid.rows,
        panes.len(),
        grid.count(),
        victims
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if dry {
        return Ok(summary);
    }
    // Recheck the entire removal set before touching a pane. tmux resumes stopped
    // pane shells itself, so SIGSTOP cannot provide a reliable transaction here.
    if !victims.is_empty() {
        let current = process::snapshot(Path::new("/proc"))?;
        let live = tmux.panes()?;
        for pane in &victims {
            if !live
                .iter()
                .any(|p| p.id == pane.id && p.pid == pane.pid && p.window == window)
                || !process::idle(&current, pane.pid, false)
                || current
                    .get(&pane.pid)
                    .is_none_or(|p| p.start != tree[&pane.pid].start)
            {
                return Err("pane changed during validation; shrink cancelled".into());
            }
        }
    }
    for pane in &victims {
        let current = process::snapshot(Path::new("/proc"))?;
        if !process::idle(&current, pane.pid, false)
            || current
                .get(&pane.pid)
                .is_none_or(|p| p.start != tree[&pane.pid].start)
        {
            return Err("pane became busy; remaining removals cancelled".into());
        }
        tmux.run(&[
            "if-shell",
            "-F",
            "-t",
            &pane.id,
            &format!(
                "#{{&&:#{{==:#{{pane_pid}},{}}},#{{==:#{{pane_current_command}},{}}}}}",
                pane.pid, tree[&pane.pid].command
            ),
            &format!("kill-pane -t {}", pane.id),
            "display-message -p 'pane became busy; removal refused'",
        ])?;
        if tmux.panes()?.iter().any(|p| p.id == pane.id) {
            return Err("pane became busy; remaining removals cancelled".into());
        }
    }
    panes.retain(|p| !victims.iter().any(|v| p.id == v.id));
    while panes.len() < grid.count() {
        let largest = tmux.run(&["list-panes", "-t", window, "-F", "#{pane_id} #{pane_width}"])?;
        let target = largest
            .lines()
            .filter_map(|l| l.split_once(' '))
            .max_by_key(|(_, w)| w.parse::<usize>().unwrap_or(0))
            .ok_or("missing pane")?
            .0;
        tmux.run(&[
            "split-window",
            "-h",
            "-d",
            "-t",
            target,
            "-c",
            &panes[0].cwd,
        ])?;
        tmux.run(&["select-layout", "-t", window, "tiled"])?;
        panes = tmux
            .panes()?
            .into_iter()
            .filter(|p| p.window == window)
            .collect();
        panes.sort_by_key(|p| p.index);
    }
    tmux.run(&[
        "select-layout",
        "-t",
        window,
        &layout(
            grid,
            w,
            h,
            &panes.iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
        )?,
    ])?;
    Ok(summary)
}
fn dimensions(text: &str) -> Result<(usize, usize)> {
    let f: Vec<_> = text.split_whitespace().collect();
    if f.len() != 2 {
        return Err("missing terminal dimensions".into());
    }
    Ok((f[0].parse()?, f[1].parse()?))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Modal {
    pub origin: String,
    pub pane: String,
    pub placeholder: String,
    pub session: String,
    pub full: bool,
    pub action: Option<String>,
    pub client: String,
}
fn modal_path(root: &Path, window: &str) -> PathBuf {
    root.join(format!("modal-{}.json", window.trim_start_matches('@')))
}
fn modal_read(root: &Path, origin: &str) -> Result<Modal> {
    if !valid_id(origin, '@') {
        return Err("invalid origin window".into());
    }
    Ok(serde_json::from_reader(fs::File::open(modal_path(
        root, origin,
    ))?)?)
}
fn base_command(exe: &Path, root: &Path, tmux: &Tmux) -> Result<String> {
    Ok(format!(
        "{} --state-dir {} --tmux-socket {}",
        shell_quote(&exe.to_string_lossy()),
        shell_quote(&root.to_string_lossy()),
        shell_quote(&tmux.socket_path()?)
    ))
}

pub fn modal_action(tmux: &Tmux, root: &Path, origin: &str, action: &str) -> Result<()> {
    let _lock = Lock::acquire(&root.join("modal-action.lock"), true)?;
    let mut modal = modal_read(root, origin)?;
    if !["grid", "full", "restore"].contains(&action)
        && !action
            .strip_prefix("select:")
            .is_some_and(|s| s.parse::<usize>().is_ok_and(|n| (1..=9).contains(&n)))
    {
        return Err("invalid modal action".into());
    }
    modal.action = Some(action.into());
    atomic(&modal_path(root, origin), &modal)?;
    tmux.run(&["detach-client", "-s", &modal.session])?;
    Ok(())
}

pub fn recover_modal(tmux: &Tmux, root: &Path, origin: &str) -> Result<()> {
    if !valid_id(origin, '@') {
        return Err("invalid origin window".into());
    }
    let _lock = Lock::acquire(&root.join(format!("zoom-{}.lock", &origin[1..])), false)?;
    let modal = modal_read(root, origin)?;
    restore(tmux, root, &modal)
}
fn restore(tmux: &Tmux, root: &Path, modal: &Modal) -> Result<()> {
    let _lock = Lock::acquire(&root.join("workspace.lock"), true)?;
    if !valid_id(&modal.origin, '@')
        || !valid_id(&modal.pane, '%')
        || !valid_id(&modal.placeholder, '%')
        || !modal.session.starts_with("kelpmux-")
    {
        return Err("invalid modal recovery state".into());
    }
    let panes = tmux.panes()?;
    let selected = panes.iter().find(|p| p.id == modal.pane);
    let placeholder = panes.iter().find(|p| p.id == modal.placeholder);
    if let Some(slot) = placeholder {
        let tree = process::snapshot(Path::new("/proc"))?;
        let args = process::args(Path::new("/proc"), slot.pid);
        if tmux.option(&slot.id, "@kelpmux-placeholder")? != modal.session
            || !tree.get(&slot.pid).is_some_and(|p| p.command == "sleep")
            || process::descendants(&tree, slot.pid).len() != 1
            || args.last().map(String::as_str) != Some("2147483647")
        {
            return Err("placeholder was reused; all panes preserved for manual recovery".into());
        }
    }
    match (selected, placeholder) {
        (Some(p), Some(slot)) if slot.window == modal.origin && p.window != modal.origin => {
            tmux.run(&["swap-pane", "-s", &modal.pane, "-t", &modal.placeholder])?;
        }
        (Some(p), _) if p.window == modal.origin => (),
        // The original application was closed by the user; remove only the known empty placeholder.
        (None, Some(slot)) if slot.window == modal.origin => {
            tmux.run(&["kill-pane", "-t", &modal.placeholder])?;
        }
        _ => {
            return Err(
                "modal cannot be restored automatically; agent remains in its temporary session"
                    .into(),
            );
        }
    }
    // Never kill a session with an unexpected pane or a still-running agent in it.
    if let Ok(text) = tmux.run(&["list-panes", "-s", "-t", &modal.session, "-F", "#{pane_id}"]) {
        if text.lines().any(|id| id != modal.placeholder) {
            return Err(
                "temporary session contains additional panes; preserved for manual recovery".into(),
            );
        }
        tmux.run(&["kill-session", "-t", &modal.session])?;
    }
    if selected.is_some() {
        tmux.run(&["select-pane", "-t", &modal.pane])?;
    }
    fs::remove_file(modal_path(root, &modal.origin))?;
    Ok(())
}

pub fn zoom(
    tmux: &Tmux,
    root: &Path,
    pane: &str,
    client: &str,
    max_width: usize,
    full: bool,
) -> Result<()> {
    if !valid_id(pane, '%') || !(40..=500).contains(&max_width) {
        return Err("zoom requires a pane id and width between 40 and 500".into());
    }
    private_dir(root)?;
    let origin = tmux.format(pane, "#{window_id}")?;
    let inner_origin = tmux.format(pane, "#{@kelpmux-origin}")?;
    if valid_id(&inner_origin, '@') {
        return modal_action(
            tmux,
            root,
            &inner_origin,
            if full { "full" } else { "grid" },
        );
    }
    let _lock = Lock::acquire(
        &root.join(format!("zoom-{}.lock", origin.trim_start_matches('@'))),
        false,
    )?;
    let setup_lock = Lock::acquire(&root.join("workspace.lock"), true)?;
    if modal_path(root, &origin).exists() {
        return Err("stale modal state: use recover-modal before zooming".into());
    }
    if tmux.format(pane, "#{window_zoomed_flag}")? == "1" {
        tmux.run(&["resize-pane", "-Z", "-t", pane])?;
    }
    let exe = std::env::current_exe()?;
    let command = base_command(&exe, root, tmux)?;
    let session = format!(
        "kelpmux-{}-{}",
        std::process::id(),
        origin.trim_start_matches('@')
    );
    let placeholder = tmux.run(&[
        "new-session",
        "-d",
        "-s",
        &session,
        "-P",
        "-F",
        "#{pane_id}",
        "exec sleep 2147483647",
    ])?;
    let mut modal = Modal {
        origin: origin.clone(),
        pane: pane.into(),
        placeholder,
        session,
        full,
        action: None,
        client: client.into(),
    };
    tmux.set(&modal.placeholder, "@kelpmux-placeholder", &modal.session)?;
    atomic(&modal_path(root, &origin), &modal)?;
    drop(setup_lock);
    let result = (|| -> Result<()> {
        tmux.run(&["swap-pane", "-s", &modal.pane, "-t", &modal.placeholder])?;
        tmux.run(&[
            "set-option",
            "-t",
            &modal.session,
            "@kelpmux-origin",
            &origin,
        ])?;
        tmux.run(&["set-option", "-t", &modal.session, "status", "on"])?;
        tmux.run(&["set-option", "-t", &modal.session, "status-right", ""])?;
        tmux.run(&[
            "set-option",
            "-t",
            &modal.session,
            "status-left-length",
            "150",
        ])?;
        tmux.run(&[
            "set-option",
            "-t",
            &modal.session,
            "status-format[0]",
            "#{status-left}",
        ])?;
        tmux.run(&[
            "set-option",
            "-t",
            &modal.session,
            "key-table",
            "kelpmux-modal",
        ])?;
        tmux.run(&[
            "set-option",
            "-t",
            &modal.session,
            "destroy-unattached",
            "off",
        ])?;
        for (key, action) in [
            ("M-Tab", "grid"),
            ("M-0", "grid"),
            ("M-f", "full"),
            ("M-r", "restore"),
        ] {
            tmux.run(&[
                "bind-key",
                "-T",
                "kelpmux-modal",
                key,
                "run-shell",
                "-b",
                &format!("{command} modal-action {action} --origin '#{{@kelpmux-origin}}'"),
            ])?;
        }
        for n in 1..=9 {
            tmux.run(&[
                "bind-key",
                "-T",
                "kelpmux-modal",
                &format!("M-{n}"),
                "run-shell",
                "-b",
                &format!("{command} modal-action select:{n} --origin '#{{@kelpmux-origin}}'"),
            ])?;
        }
        tmux.run(&[
            "bind-key",
            "-T",
            "kelpmux-modal",
            "MouseDown1Status",
            "run-shell",
            "-b",
            &format!(
                "{command} modal-action '#{{mouse_status_range}}' --origin '#{{@kelpmux-origin}}'"
            ),
        ])?;
        loop {
            let toolbar = format!(
                "#[range=user|grid] GRID #[norange] #[range=user|{}] {} #[norange]",
                if modal.full { "restore" } else { "full" },
                if modal.full { "RESTORE" } else { "FULL" }
            );
            tmux.run(&["set-option", "-t", &modal.session, "status-left", &toolbar])?;
            let dims = tmux.run(&[
                "display-message",
                "-p",
                "-c",
                client,
                "#{client_width} #{client_height}",
            ])?;
            let (width, height) = dimensions(&dims)?;
            let popup_width = if modal.full {
                width
            } else {
                width.min(max_width + 2)
            };
            let popup_height = if modal.full || width <= max_width + 2 {
                height
            } else {
                (height * 95 / 100).max(5)
            };
            let attach = format!(
                "env -u TMUX tmux -S {} attach-session -t {}",
                shell_quote(&tmux.socket_path()?),
                shell_quote(&modal.session)
            );
            let mut args = vec![
                "display-popup",
                "-E",
                "-c",
                client,
                "-x",
                "C",
                "-y",
                "C",
                "-w",
            ];
            let w = popup_width.to_string();
            let h = popup_height.to_string();
            args.extend([&w, "-h", &h]);
            if modal.full || width <= max_width + 2 {
                args.push("-B");
            }
            args.push(&attach);
            tmux.run(&args)?;
            let _action_lock = Lock::acquire(&root.join("modal-action.lock"), true)?;
            let updated = modal_read(root, &origin)?;
            let Some(action) = updated.action else {
                break;
            };
            if action == "grid" {
                break;
            }
            if let Some(number) = action.strip_prefix("select:") {
                let index: usize = number.parse()?;
                let slots = tmux.panes()?;
                if let Some(target) = slots
                    .iter()
                    .find(|p| p.window == origin && p.index == index)
                {
                    if target.id == modal.placeholder {
                        break;
                    }
                    // Return the old pane, then move the newly selected pane into the same temporary session.
                    tmux.run(&["swap-pane", "-s", &modal.pane, "-t", &modal.placeholder])?;
                    modal.pane = target.id.clone();
                    tmux.run(&["swap-pane", "-s", &modal.pane, "-t", &modal.placeholder])?;
                }
            } else {
                modal.full = action == "full";
            }
            modal.action = None;
            atomic(&modal_path(root, &origin), &modal)?;
        }
        Ok(())
    })();
    let cleanup = restore(tmux, root, &modal);
    result.and(cleanup)
}
