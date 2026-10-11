use crate::{Result, label, model::Agent};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

pub const HEADER_FORMAT: &str = "#[fg=colour231,bg=colour24,bold] #{?@kelpmux-number,#{@kelpmux-number},#{pane_index}} #[fg=colour16,bg=#{?@kelpmux-color,#{@kelpmux-color},colour245},nobold] #{?@kelpmux-name,#{@kelpmux-name},#{b:pane_current_path}} #{@kelpmux-status} ";
pub const HEADER_STYLE: &str = "fg=colour16,bg=#{?@kelpmux-color,#{@kelpmux-color},colour245}";

#[derive(Debug, Clone, Default)]
pub struct Tmux {
    pub socket: Option<PathBuf>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pane {
    pub id: String,
    pub window: String,
    pub pid: u32,
    pub index: usize,
    pub cwd: String,
    pub dead: bool,
}

impl Tmux {
    pub fn run(&self, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new("tmux");
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
            "#{pane_id}|#{window_id}|#{pane_pid}|#{pane_index}|#{pane_dead}|#{pane_current_path}",
        ])?;
        let mut panes = BTreeMap::new();
        for line in text.lines() {
            let f: Vec<_> = line.splitn(6, '|').collect();
            if f.len() != 6 {
                return Err(format!("invalid tmux pane listing: {line:?}").into());
            }
            panes.insert(
                f[0].to_owned(),
                Pane {
                    id: f[0].into(),
                    window: f[1].into(),
                    pid: f[2].parse()?,
                    index: f[3].parse()?,
                    dead: f[4] == "1",
                    cwd: f[5].into(),
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
        self.set(&agent.pane, "@kelpmux-name", &label(&agent.name))?;
        self.set(&agent.pane, "@kelpmux-status", agent.status.text())?;
        self.set(&agent.pane, "@kelpmux-color", agent.status.color())?;
        self.set(
            &agent.pane,
            "@kelpmux-session",
            agent.session_id.as_deref().unwrap_or(""),
        )?;
        Ok(())
    }
    pub fn socket_path(&self) -> Result<String> {
        if let Some(s) = &self.socket {
            return Ok(s.to_string_lossy().into_owned());
        }
        self.run(&["display-message", "-p", "#{socket_path}"])
    }
}
