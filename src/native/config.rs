//! KelpMux's part of a workspace's `workspace.json`, and read-only access to
//! the files the composed UI of KelpServer 0.2 left in the workspace state
//! directory.
//!
//! Since KelpServer 0.3 the server reads and records only `shell` and `cwd`
//! (`lifecycle::Settings`) and preserves every other key of the file. The
//! startup template (`kelpmux start --template`) and the UI settings of a
//! `--config` file are KelpMux's: `start` writes them beside the server's
//! keys, the TUI creates the tabs of a new server run from the template and
//! migrates `ui` once into its own state. The legacy files
//! (`tab-layout.json`, `sidebar-folded.json`, `sidebar-accent.json`) are
//! read for that one-time migration and never written.
use super::{templates::Template, ui::UiConfig};
use crate::{Result, journal};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs::OpenOptions,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub const FILE: &str = "workspace.json";
const MAX_SETTINGS: u64 = 64 * 1024;
const MAX_LAYOUT: u64 = 512 * 1024;
const MAX_PREFERENCE: u64 = 128;

/// The keys of `workspace.json` (or of a `--config` file) KelpMux reads.
/// Unknown keys are ignored, like the server does: `tab_count`, `grid` and
/// `grid_weights` of older files configured the composed UI's startup tabs,
/// which no server creates any more.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub shell: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub template: Option<Template>,
    /// The composed UI's settings, kept as JSON: the TUI's own copy of the
    /// type converts them when it migrates them.
    pub ui: Option<serde_json::Value>,
}
impl Settings {
    /// Reads a regular file of at most 64 KiB. A saved template must still
    /// be well formed; its folders are only checked when a server starts.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = read_bounded(path, MAX_SETTINGS, false).map_err(|e| {
            format!("configuration must be a regular file smaller than 64 KiB: {e}")
        })?;
        let settings: Self = serde_json::from_slice(&bytes)?;
        if let Some(template) = &settings.template {
            template.validate_schema()?;
        }
        Ok(settings)
    }
    /// The UI settings, validated. `None` without a `ui` key.
    pub fn ui_config(&self) -> Result<Option<UiConfig>> {
        let Some(ui) = &self.ui else {
            return Ok(None);
        };
        let config: UiConfig = serde_json::from_value(ui.clone())?;
        config.validate()?;
        Ok(Some(config))
    }
    /// Records KelpMux's keys (`template` and `ui`) in the workspace's
    /// `workspace.json`, keeping the server's and any other key unchanged.
    pub fn persist(&self, state: &Path) -> Result<()> {
        let path = state.join(FILE);
        let mut object = match read_bounded(&path, MAX_SETTINGS, false) {
            Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes)? {
                serde_json::Value::Object(object) => object,
                _ => return Err("workspace.json must contain a JSON object".into()),
            },
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                serde_json::Map::new()
            }
            Err(error) => return Err(error),
        };
        object.insert("template".into(), serde_json::to_value(&self.template)?);
        match &self.ui {
            // Saved in the TUI's form when it accepts them (legacy values
            // such as `"tab_position": "top"` normalized, obsolete keys
            // dropped), else unchanged: the TUI then ignores them.
            Some(ui) => {
                let ui = match self.ui_config() {
                    Ok(Some(config)) => serde_json::to_value(config)?,
                    _ => ui.clone(),
                };
                object.insert("ui".into(), ui);
            }
            None => {
                object.remove("ui");
            }
        }
        journal::atomic(&path, &serde_json::Value::Object(object))
    }
}

/// The workspace's `workspace.json`, if it exists and is valid.
pub fn workspace(state: &Path) -> Option<Settings> {
    let path = state.join(FILE);
    path.is_file().then(|| Settings::load(&path).ok()).flatten()
}

/// Reads a regular file without following a symlink, refusing one larger
/// than `limit`, and with `private`, one not owned by this user or readable
/// by others.
fn read_bounded(path: &Path, limit: u64, private: bool) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.len() > limit
        || (private && (meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0))
    {
        return Err(format!("{} must be a private regular file", path.display()).into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(format!("{} is too large", path.display()).into());
    }
    Ok(bytes)
}
fn legacy_json<T: DeserializeOwned>(path: &Path, limit: u64) -> Result<T> {
    Ok(serde_json::from_slice(&read_bounded(path, limit, true)?)?)
}

/// The composed UI's saved tab layout (`tab-layout.json`, written by
/// KelpServer 0.2), in display order.
#[derive(Debug, Clone, Deserialize)]
pub struct LegacyTabLayout {
    /// The workspace shell when the layout was saved.
    pub shell: PathBuf,
    pub panes: Vec<LegacyTab>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct LegacyTab {
    pub name: Option<String>,
    pub cwd: PathBuf,
    pub shell: PathBuf,
}
/// The saved layout, or `None` without a valid one.
pub fn legacy_tab_layout(state: &Path) -> Option<LegacyTabLayout> {
    let layout: LegacyTabLayout = legacy_json(&state.join("tab-layout.json"), MAX_LAYOUT).ok()?;
    let valid = !layout.panes.is_empty()
        && layout.panes.iter().all(|pane| {
            pane.cwd.is_absolute()
                && !pane.shell.as_os_str().is_empty()
                && pane.name.as_ref().is_none_or(|name| {
                    !name.trim().is_empty()
                        && name.len() <= 128
                        && !name.chars().any(char::is_control)
                })
        });
    valid.then_some(layout)
}
/// The composed UI's separate sidebar preferences: folded, and the accent
/// colour (KelpServer 0.2's `sidebar-folded.json`, `sidebar-accent.json`).
pub fn legacy_sidebar_folded(state: &Path) -> Option<bool> {
    legacy_json(&state.join("sidebar-folded.json"), MAX_PREFERENCE).ok()
}
pub fn legacy_sidebar_accent(state: &Path) -> Option<super::ui::Accent> {
    legacy_json(&state.join("sidebar-accent.json"), MAX_PREFERENCE).ok()
}
