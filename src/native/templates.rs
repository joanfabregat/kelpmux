//! Declarative shell workspaces. Templates contain no agent commands or IDs.
use crate::Result;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
const MAX_TEMPLATE: u64 = 64 * 1024;
const PRESETS: [(u8, u8); 6] = [(1, 1), (2, 1), (2, 2), (3, 2), (2, 3), (3, 3)];
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    pub grid: (u8, u8),
    pub panes: Vec<TemplatePane>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplatePane {
    pub name: String,
    pub cwd: PathBuf,
    #[serde(default)]
    pub shell: Option<PathBuf>,
}
impl Template {
    pub fn validate(&self) -> Result<()> {
        self.validate_schema()?;
        for pane in &self.panes {
            if !pane.cwd.is_dir() {
                return Err("template working directories must exist".into());
            }
            if let Some(shell) = &pane.shell {
                let meta = fs::metadata(shell)?;
                if !meta.is_file() || meta.mode() & 0o111 == 0 {
                    return Err("template shell must be an executable file".into());
                }
            }
        }
        Ok(())
    }
    /// Validate saved settings without requiring a live terminal's directory to
    /// still exist. Filesystem checks belong to startup, not attach or stop.
    pub fn validate_schema(&self) -> Result<()> {
        if !PRESETS.contains(&self.grid)
            || self.panes.len() > usize::from(self.grid.0) * usize::from(self.grid.1)
        {
            return Err(
                "template requires a supported grid with enough slots for its panes".into(),
            );
        }
        for pane in &self.panes {
            if pane.name.trim().is_empty()
                || pane.name.len() > 128
                || pane.name.chars().any(char::is_control)
            {
                return Err("template pane names require 1–128 safe bytes".into());
            }
            if !pane.cwd.is_absolute() {
                return Err(
                    "template working directories must be existing absolute directories".into(),
                );
            }
            if let Some(shell) = &pane.shell {
                if !shell.is_absolute() {
                    return Err("template shell must be an absolute executable file".into());
                }
            }
        }
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Self> {
        load(path)
    }
}
fn no_symlinks(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if let Ok(meta) = fs::symlink_metadata(ancestor) {
            if meta.file_type().is_symlink() {
                return Err("template path contains a symlink".into());
            }
        }
    }
    Ok(())
}
pub fn load(path: &Path) -> Result<Template> {
    no_symlinks(path)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.len() > MAX_TEMPLATE {
        return Err("template must be an owned regular JSON file no larger than 64 KiB".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_TEMPLATE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_TEMPLATE {
        return Err("template grew beyond 64 KiB".into());
    }
    let template: Template = serde_json::from_slice(&bytes)?;
    template.validate()?;
    Ok(template)
}
/// List valid named templates under STATE/templates. Missing directories are
/// empty; unsafe directories/files are refused rather than followed or ignored.
/// At most 128 files, sorted by name. Explicit load accepts owned project files.
pub fn list(state: &Path) -> Result<Vec<(String, Template)>> {
    if !state.is_absolute() {
        return Err("template state directory must be absolute".into());
    }
    let directory = state.join("templates");
    no_symlinks(&directory)?;
    let state_meta = match fs::symlink_metadata(state) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    if !state_meta.is_dir()
        || state_meta.uid() != unsafe { libc::geteuid() }
        || state_meta.mode() & 0o077 != 0
    {
        return Err("template state directory must be private and owned".into());
    }
    let meta = match fs::symlink_metadata(&directory) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err("template directory must be private and owned".into());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("invalid template name")?;
        if name.is_empty()
            || name.len() > 48
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("invalid stored template name".into());
        }
        if entries.len() >= 128 {
            return Err("too many stored templates".into());
        }
        entries.push((name.to_owned(), load(&path)?));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}
