use crate::{
    Result,
    journal::{Lock, private_dir},
    process,
    tmux::{Pane, Tmux},
};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

#[derive(serde::Serialize, serde::Deserialize)]
pub struct GridChoice {
    pub grid: Grid,
    pub current: bool,
    pub blocked: Option<String>,
}

impl GridChoice {
    /// Separate native menu rows: tmux menu items cannot contain newlines.
    pub fn drawing(&self, compact: bool) -> Vec<String> {
        if compact {
            return vec![vec!["□"; self.grid.cols].join(" "); self.grid.rows];
        }
        let edge = |left, joint: &str, right| {
            format!("{left}{}{right}", vec!["───"; self.grid.cols].join(joint))
        };
        let mut lines = vec![edge("┌", "┬", "┐")];
        for row in 0..self.grid.rows {
            lines.push(format!("│{}│", vec!["   "; self.grid.cols].join("│")));
            if row + 1 < self.grid.rows {
                lines.push(edge("├", "┼", "┤"));
            }
        }
        lines.push(edge("└", "┴", "┘"));
        lines
    }
}

/// Read-only preview. Applying a choice still performs resize's fresh checks.
pub fn grid_choices(tmux: &Tmux, window: &str) -> Result<Vec<GridChoice>> {
    if !valid_id(window, '@') {
        return Err("grid picker requires a window id such as @0".into());
    }
    let panes: Vec<_> = tmux
        .panes()?
        .into_iter()
        .filter(|p| p.window == window)
        .collect();
    if panes.is_empty() {
        return Err("window has no panes".into());
    }
    let mut floating = panes.iter().any(|p| p.floating);
    for pane in &panes {
        floating |= reading_width(tmux, &pane.id)?.is_some();
    }
    let zoomed = tmux.format(window, "#{window_zoomed_flag}")? == "1";
    let (width, height) = dimensions(&tmux.format(window, "#{window_width} #{window_height}")?)?;
    let positions = tmux.run(&["list-panes", "-t", window, "-F", "#{pane_left} #{pane_top}"])?;
    let mut xs = std::collections::BTreeSet::new();
    let mut ys = std::collections::BTreeSet::new();
    for line in positions.lines() {
        let (x, y) = dimensions(line)?;
        xs.insert(x);
        ys.insert(y);
    }
    let tree = process::snapshot(Path::new("/proc"))?;
    let unused = panes
        .iter()
        .filter(|p| !p.dead && process::idle(&tree, p.pid, false))
        .count();
    ["1x1", "2x1", "2x2", "3x2", "2x3", "3x3"]
        .into_iter()
        .map(|shape| {
            let grid = Grid::parse(shape)?;
            let needed = panes.len().saturating_sub(grid.count());
            let blocked = if floating {
                Some("Return to grid first".into())
            } else if zoomed {
                Some("Unzoom first".into())
            } else if sizes(width, grid.cols).is_err() || sizes(height, grid.rows).is_err() {
                Some("Terminal too small".into())
            } else if unused < needed {
                Some(format!("Needs {needed} unused; {unused} available"))
            } else {
                None
            };
            Ok(GridChoice {
                grid,
                current: !floating
                    && panes.len() == grid.count()
                    && xs.len() == grid.cols
                    && ys.len() == grid.rows,
                blocked,
            })
        })
        .collect()
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
    let _lock = Lock::acquire(&root.join("workspace.lock"), true)?;
    let mut panes: Vec<_> = tmux
        .panes()?
        .into_iter()
        .filter(|p| p.window == window)
        .collect();
    panes.sort_by_key(|p| p.index);
    if panes.is_empty() {
        return Err("window has no panes".into());
    }
    for pane in &panes {
        if pane.floating || reading_width(tmux, &pane.id)?.is_some() {
            return Err("return floating panes to the grid before changing layout".into());
        }
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

/// tmux owns floating panes and their saved tile positions. No extra client,
/// temporary session, placeholder process, or recovery file is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    Toggle,
    Fullscreen,
    ToggleFullscreen,
    Restore,
}

pub fn native_floating_supported(version: &str) -> bool {
    let Some((major, rest)) = version.split_once('.') else {
        return false;
    };
    let minor: String = rest.chars().take_while(char::is_ascii_digit).collect();
    match (major.parse::<u32>(), minor.parse::<u32>()) {
        (Ok(major), Ok(minor)) => major > 3 || (major == 3 && minor >= 8),
        _ => false,
    }
}

fn require_native(tmux: &Tmux, pane: &str) -> Result<()> {
    let version = tmux.format(pane, "#{version}")?;
    if !native_floating_supported(&version) {
        return Err(format!("native reading view requires tmux 3.8+; server is {version}").into());
    }
    Ok(())
}

fn reading_width(tmux: &Tmux, pane: &str) -> Result<Option<usize>> {
    let value = tmux.option(pane, "@kelpmux-reading-width")?;
    if value.is_empty() {
        return Ok(None);
    }
    let width: usize = value.parse()?;
    if !(40..=500).contains(&width) {
        return Err("invalid saved reading width".into());
    }
    Ok(Some(width))
}

fn window_members(tmux: &Tmux, window: &str) -> Result<String> {
    let members = tmux.run(&["list-panes", "-t", window, "-F", "#{pane_id}:#{pane_index}"])?;
    let mut lines: Vec<_> = members.lines().collect();
    lines.sort_unstable();
    Ok(lines.join(" "))
}

fn return_pane(tmux: &Tmux, pane: &str) -> Result<()> {
    let window = tmux.format(pane, "#{window_id}")?;
    let layout = tmux.option(pane, "@kelpmux-reading-layout")?;
    let members = tmux.option(pane, "@kelpmux-reading-members")?;
    if tmux.format(pane, "#{window_zoomed_flag}")? == "1" {
        tmux.run(&["resize-pane", "-Z", "-t", pane])?;
    }
    if tmux.format(pane, "#{pane_floating_flag}")? == "1" {
        // tmux restores the original tile itself, including its original index.
        tmux.run(&["join-pane", "-s", pane, "-t", pane])?;
    }
    // Native reinsertion can shift an uneven split by one column. Restore the
    // original geometry only while every original pane/index is still present
    // and there are no unrelated floating panes. Never remove or replace panes.
    if !layout.is_empty()
        && !members.is_empty()
        && members == window_members(tmux, &window)?
        && !tmux
            .panes()?
            .iter()
            .any(|p| p.window == window && p.floating)
    {
        tmux.run(&["select-layout", "-t", &window, &layout])?;
        tmux.run(&["select-pane", "-t", pane])?;
    }
    for option in [
        "@kelpmux-reading-width",
        "@kelpmux-reading-full",
        "@kelpmux-reading-layout",
        "@kelpmux-reading-members",
    ] {
        tmux.run(&["set-option", "-pu", "-t", pane, option])?;
    }
    Ok(())
}

fn fit_reading(tmux: &Tmux, pane: &str, width: usize) -> Result<()> {
    let (w, h) = dimensions(&tmux.format(pane, "#{window_width} #{window_height}")?)?;
    if w < 5 || h < 5 {
        return Err("terminal too small for reading view".into());
    }
    if w <= width + 2 {
        // Native zoom uses every available column on small clients. The saved
        // floating cell remains owned by tmux and is restored on a larger screen.
        if tmux.format(pane, "#{window_zoomed_flag}")? != "1" {
            tmux.run(&["resize-pane", "-Z", "-t", pane])?;
        }
        return Ok(());
    }
    let border = if tmux.format(pane, "#{pane-border-lines}")? == "none" {
        0
    } else {
        2
    };
    let target_w = width.min(w.saturating_sub(border));
    let target_h = (h * 95 / 100).saturating_sub(border).max(3);
    let current = dimensions(&tmux.format(pane, "#{pane_width} #{pane_height}")?)?;
    if current != (target_w, target_h) {
        tmux.run(&[
            "resize-pane",
            "-t",
            pane,
            "-x",
            &(target_w + border).to_string(),
            "-y",
            &(target_h + border).to_string(),
        ])?;
    }
    tmux.run(&["move-pane", "-s", pane, "-P", "centre"])?;
    Ok(())
}

pub fn reading(tmux: &Tmux, root: &Path, pane: &str, width: usize, mode: Reading) -> Result<()> {
    if !valid_id(pane, '%') || !(40..=500).contains(&width) {
        return Err("reading view needs a pane ID and width between 40 and 500".into());
    }
    require_native(tmux, pane)?;
    private_dir(root)?;
    let _lock = Lock::acquire(&root.join("workspace.lock"), true)?;
    let window = tmux.format(pane, "#{window_id}")?;
    let saved = reading_width(tmux, pane)?;
    let mode = if mode == Reading::ToggleFullscreen {
        if saved.is_some() && tmux.option(pane, "@kelpmux-reading-full")? == "1" {
            Reading::Restore
        } else {
            Reading::Fullscreen
        }
    } else {
        mode
    };
    if saved.is_some() && mode == Reading::Toggle {
        return return_pane(tmux, pane);
    }
    if saved.is_none() && mode == Reading::Restore {
        return Err("pane is not in reading view".into());
    }
    // Return the previous reading pane before opening another. Ordinary floating
    // panes are left alone; they are not assumed to belong to Kelpmux.
    if saved.is_none() {
        if tmux.format(pane, "#{pane_floating_flag}")? == "1" {
            return Err("pane is already floating outside Kelpmux".into());
        }
        for other in tmux
            .panes()?
            .into_iter()
            .filter(|p| p.window == window && p.id != pane)
        {
            if reading_width(tmux, &other.id)?.is_some() {
                return_pane(tmux, &other.id)?;
            }
        }
        if tmux.format(pane, "#{window_zoomed_flag}")? == "1" {
            tmux.run(&["resize-pane", "-Z", "-t", pane])?;
        }
        tmux.set(
            pane,
            "@kelpmux-reading-layout",
            &tmux.format(pane, "#{window_layout}")?,
        )?;
        tmux.set(
            pane,
            "@kelpmux-reading-members",
            &window_members(tmux, &window)?,
        )?;
        tmux.set(pane, "@kelpmux-reading-width", &width.to_string())?;
        tmux.run(&["break-pane", "-W", "-s", pane])?;
    }
    tmux.run(&["select-pane", "-t", pane])?;
    if mode == Reading::Fullscreen {
        tmux.set(pane, "@kelpmux-reading-full", "1")?;
        if tmux.format(pane, "#{window_zoomed_flag}")? != "1" {
            tmux.run(&["resize-pane", "-Z", "-t", pane])?;
        }
    } else {
        if tmux.format(pane, "#{window_zoomed_flag}")? == "1" {
            tmux.run(&["resize-pane", "-Z", "-t", pane])?;
        }
        tmux.set(pane, "@kelpmux-reading-full", "0")?;
        fit_reading(tmux, pane, saved.unwrap_or(width))?;
    }
    Ok(())
}

pub fn close_reading(tmux: &Tmux, root: &Path, pane: &str) -> Result<()> {
    if !valid_id(pane, '%') {
        return Err("close needs a pane ID".into());
    }
    private_dir(root)?;
    let _lock = Lock::acquire(&root.join("workspace.lock"), true)?;
    if reading_width(tmux, pane)?.is_some() {
        return_pane(tmux, pane)?;
    }
    Ok(())
}

pub fn grid(tmux: &Tmux, root: &Path, window: &str) -> Result<()> {
    if !valid_id(window, '@') {
        return Err("grid needs a window ID".into());
    }
    private_dir(root)?;
    let _lock = Lock::acquire(&root.join("workspace.lock"), true)?;
    for pane in tmux.panes()?.into_iter().filter(|p| p.window == window) {
        if reading_width(tmux, &pane.id)?.is_some() {
            return_pane(tmux, &pane.id)?;
        }
    }
    Ok(())
}

/// Called from native resize hooks. Reading geometry follows the window shared
/// by its clients; tmux remains in charge of deciding that window's dimensions.
pub fn refresh_reading(tmux: &Tmux, root: &Path, window: &str) -> Result<()> {
    if !valid_id(window, '@') {
        return Err("resize needs a window ID".into());
    }
    private_dir(root)?;
    let _lock = Lock::acquire(&root.join("workspace.lock"), true)?;
    for pane in tmux.panes()?.into_iter().filter(|p| p.window == window) {
        if let Some(width) = reading_width(tmux, &pane.id)? {
            if tmux.option(&pane.id, "@kelpmux-reading-full")? == "1" {
                continue;
            }
            if tmux.format(&pane.id, "#{window_zoomed_flag}")? == "1" {
                tmux.run(&["resize-pane", "-Z", "-t", &pane.id])?;
            }
            fit_reading(tmux, &pane.id, width)?;
        }
    }
    Ok(())
}
