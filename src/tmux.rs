use crate::{
    Result, label,
    model::{Agent, Status, WORKING_FRAMES},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

pub const HEADER_BACKGROUND: &str = "#{?pane_active,#{?@kelpmux-color,#{@kelpmux-color},colour245},#{?@kelpmux-color-muted,#{@kelpmux-color-muted},colour239}}";
pub const BORDER_STYLE: &str = "fg=colour238,bg=colour234";
pub const PANE_PALETTE: [(&str, &str); 9] = [
    ("colour39", "colour24"),
    ("colour42", "colour29"),
    ("colour141", "colour60"),
    ("colour214", "colour94"),
    ("colour80", "colour30"),
    ("colour205", "colour89"),
    ("colour185", "colour58"),
    ("colour209", "colour95"),
    ("colour111", "colour67"),
];

pub fn header_format() -> String {
    let badge = "#[fg=#{?pane_active,colour231,colour117},bg=#{?pane_active,colour24,colour17},bold] #{?pane_active,▶, } #{pane_index} ";
    let foreground = "#{?pane_active,colour16,colour252}";
    let text = " #{?@kelpmux-name,#{@kelpmux-name},#{b:pane_current_path}} #{@kelpmux-marker} ";
    let controls = concat!(
        "#[align=right]#[fg=colour231,bg=colour24]#[range=control|7] #{?@kelpmux-reading-width,↙,↗} #[norange]",
        "#[fg=colour16,bg=colour39]#[range=control|8] #{?@kelpmux-reading-full,▣,⛶} #[norange]",
        "#{?@kelpmux-reading-width,#[fg=colour231#,bg=colour88]#[range=control|9] × #[norange],}",
    );
    // Fill the entire title independently of the quiet one-column borders.
    format!(
        "#[fill={HEADER_BACKGROUND}]{badge}#[fg={foreground},bg={HEADER_BACKGROUND},nobold]{text}{controls}"
    )
}

#[derive(Debug, Clone, Default)]
pub struct Tmux {
    pub socket: Option<PathBuf>,
    pub binary: Option<PathBuf>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pane {
    pub id: String,
    pub window: String,
    pub pid: u32,
    pub index: usize,
    pub cwd: String,
    pub dead: bool,
    pub floating: bool,
}

impl Tmux {
    pub fn run(&self, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new(
            self.binary
                .as_deref()
                .unwrap_or_else(|| std::path::Path::new("tmux")),
        );
        // Metadata commands always exchange UTF-8, even when a noninteractive
        // shell has no UTF-8 locale or terminal environment.
        cmd.arg("-u");
        if let Some(socket) = &self.socket {
            cmd.arg("-S").arg(socket);
        }
        let output = cmd.args(args).output()?;
        if !output.status.success() {
            return Err(format!("tmux: {}", String::from_utf8_lossy(&output.stderr).trim()).into());
        }
        Ok(String::from_utf8(output.stdout)?
            .trim_end_matches('\n')
            .to_owned())
    }
    pub fn format(&self, target: &str, value: &str) -> Result<String> {
        self.run(&["display-message", "-p", "-t", target, value])
    }
    pub fn panes(&self) -> Result<Vec<Pane>> {
        let text = self.run(&[
            "list-panes",
            "-a",
            "-F",
            "#{pane_id}|#{window_id}|#{pane_pid}|#{pane_index}|#{pane_dead}|#{pane_floating_flag}|#{@kelpmux-placeholder}|#{pane_current_path}",
        ])?;
        let mut panes = BTreeMap::new();
        for line in text.lines() {
            let f: Vec<_> = line.splitn(8, '|').collect();
            if f.len() != 8 {
                return Err(format!("invalid tmux pane listing: {line:?}").into());
            }
            // A process-free reading slot is decoration, never an agent or grid slot.
            if !f[6].is_empty() && f[2].is_empty() {
                continue;
            }
            panes.insert(
                f[0].to_owned(),
                Pane {
                    id: f[0].into(),
                    window: f[1].into(),
                    pid: if f[2].is_empty() { 0 } else { f[2].parse()? },
                    index: f[3].parse()?,
                    dead: f[4] == "1",
                    cwd: f[7].into(),
                    floating: f[5] == "1",
                },
            );
        }
        Ok(panes.into_values().collect())
    }
    pub fn option(&self, target: &str, name: &str) -> Result<String> {
        self.run(&["show-options", "-pqv", "-t", target, name])
    }
    pub fn set(&self, target: &str, name: &str, value: &str) -> Result<()> {
        self.run(&["set-option", "-p", "-t", target, name, value])?;
        Ok(())
    }
    pub fn header(&self, agent: &Agent) -> Result<()> {
        self.pane_palette(&agent.pane)?;
        self.set(&agent.pane, "@kelpmux-name", &label(&agent.name))?;
        self.pane_status(&agent.pane, agent.status)?;
        self.set(
            &agent.pane,
            "@kelpmux-session",
            agent.session_id.as_deref().unwrap_or(""),
        )?;
        Ok(())
    }
    pub fn pane_palette(&self, pane: &str) -> Result<()> {
        let saved = self.option(pane, "@kelpmux-palette")?;
        let index = match saved.parse::<usize>() {
            Ok(index) if index < PANE_PALETTE.len() => index,
            _ => {
                let number: usize = self.format(pane, "#{pane_index}")?.parse()?;
                let index = number.saturating_sub(1) % PANE_PALETTE.len();
                self.set(pane, "@kelpmux-palette", &index.to_string())?;
                index
            }
        };
        let (bright, muted) = PANE_PALETTE[index];
        self.set(pane, "@kelpmux-color", bright)?;
        self.set(pane, "@kelpmux-color-muted", muted)?;
        Ok(())
    }
    pub fn pane_status(&self, pane: &str, status: Status) -> Result<()> {
        if status == Status::Working {
            // A metadata refresh must not restart an already-running spinner.
            self.run(&[
                "if-shell",
                "-F",
                "-t",
                pane,
                "#{!=:#{@kelpmux-status},WORKING}",
                &format!(
                    "set-option -p -t {pane} @kelpmux-marker {}",
                    status.marker()
                ),
            ])?;
        } else {
            self.set(pane, "@kelpmux-marker", status.marker())?;
        }
        self.set(pane, "@kelpmux-status", status.text())?;
        Ok(())
    }
    pub fn animate_working(&self, frame: usize) -> Result<()> {
        // One query and one batched update per frame, regardless of pane count.
        // Background windows need no redraw; they catch up when made visible.
        let panes = self.run(&[
            "list-panes",
            "-a",
            "-F",
            "#{?#{&&:#{window_active},#{==:#{@kelpmux-status},WORKING}},#{pane_id},}",
        ])?;
        let mut commands = Vec::new();
        for pane in panes
            .lines()
            .filter(|pane| crate::workspace::valid_id(pane, '%'))
        {
            if !commands.is_empty() {
                commands.push(";".to_owned());
            }
            commands.extend([
                "if-shell".to_owned(),
                "-F".to_owned(),
                "-t".to_owned(),
                pane.to_owned(),
                "#{==:#{@kelpmux-status},WORKING}".to_owned(),
                format!(
                    "set-option -p -t {pane} @kelpmux-marker {}",
                    WORKING_FRAMES[frame % WORKING_FRAMES.len()]
                ),
            ]);
        }
        if !commands.is_empty() {
            self.run(&commands.iter().map(String::as_str).collect::<Vec<_>>())?;
        }
        Ok(())
    }
    pub fn socket_path(&self) -> Result<String> {
        if let Some(s) = &self.socket {
            return Ok(s.to_string_lossy().into_owned());
        }
        self.run(&["display-message", "-p", "#{socket_path}"])
    }
}
