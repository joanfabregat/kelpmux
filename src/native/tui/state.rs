//! Client UI state of the stream TUI, persisted in one private JSON file per
//! workspace (`tui-state.json` in the workspace state directory).
//!
//! KelpServer stores no client UI state (ADR 0002): names, colours, tab order,
//! parking, the scratch marker and TUI settings belong to each client. Pane
//! state is keyed by server epoch and pane ID, because pane IDs are scoped to
//! an epoch. An epoch-independent layout template records the tabs (folder,
//! arguments, name, colour, parking) so the TUI can recreate its tabs after a
//! server restart.
//!
//! Several TUIs may attach to one workspace. Writers hold an exclusive `flock`
//! on `tui-state.lock`, read the current file, apply only the parts they
//! changed since their last write, prune panes that no longer exist and
//! replace the file by an atomic rename, so concurrent TUIs do not discard
//! each other's changes.
use super::super::{lifecycle, protocol::PaneId, ui::UiConfig};
use crate::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const FILE: &str = "tui-state.json";
const LOCK: &str = "tui-state.lock";
const MAX_BYTES: u64 = 1024 * 1024;
pub const VERSION: u32 = 1;
const MAX_NAME: usize = 128;
const MAX_TABS: usize = 512;

/// One pane's client state within an epoch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneState {
    /// Explicit tab name; `None` shows the server's title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Palette index; `None` uses the default colour for the pane ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<u8>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub parked: bool,
    /// This pane is the client's scratch terminal (an ordinary server pane).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub scratch: bool,
}

/// Client state for the panes of one server epoch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EpochState {
    /// Unix seconds of the last write for this epoch.
    pub updated_at: u64,
    /// Main tab order (pane IDs); the scratch pane is not part of it.
    pub order: Vec<PaneId>,
    pub panes: BTreeMap<PaneId, PaneState>,
}

/// An epoch-independent description of one tab, used to recreate tabs after
/// a server restart.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutTab {
    /// The folder the tab was started in.
    pub cwd: String,
    /// The shell the server reported for the tab; informational, because the
    /// stream `spawn` always uses the server's shell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// `spawn` arguments; empty for an interactive shell.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<u8>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub parked: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Document {
    pub version: u32,
    /// The one-time migration from the composed server's files has run.
    pub migrated: bool,
    /// TUI settings (accent, sidebar, notification sound, key bindings…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<UiConfig>,
    /// Pane state keyed by server epoch, then pane ID.
    pub epochs: BTreeMap<String, EpochState>,
    /// The latest tab layout, independent of any epoch.
    pub layout: Vec<LayoutTab>,
}
impl Document {
    fn validate(&self) -> Result<()> {
        if self.version > VERSION {
            return Err("TUI state was written by a newer KelpMux".into());
        }
        let name_ok = |name: &Option<String>| {
            name.as_ref()
                .is_none_or(|n| n.len() <= MAX_NAME && !n.chars().any(char::is_control))
        };
        let color_ok = |color: &Option<u8>| color.is_none_or(|c| usize::from(c) < 9);
        if self.layout.len() > MAX_TABS
            || self.epochs.len() > 8
            || self.layout.iter().any(|tab| {
                !name_ok(&tab.name)
                    || !color_ok(&tab.color)
                    || !Path::new(&tab.cwd).is_absolute()
                    || tab.argv.len() > 256
            })
            || self.epochs.iter().any(|(epoch, state)| {
                epoch.is_empty()
                    || epoch.len() > 128
                    || state.panes.len() > MAX_TABS
                    || state.order.len() > MAX_TABS
                    || state
                        .panes
                        .values()
                        .any(|pane| !name_ok(&pane.name) || !color_ok(&pane.color))
            })
        {
            return Err("invalid TUI state".into());
        }
        if let Some(settings) = &self.settings {
            settings.validate()?;
        }
        Ok(())
    }
}

/// What this client changed since its last write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub settings: bool,
    pub layout: bool,
    pub order: bool,
    pub migrated: bool,
    pub panes: BTreeSet<PaneId>,
    pub removed: BTreeSet<PaneId>,
}
impl Changes {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Applies `mine` (this client's view of `epoch`) to `disk`, the document
/// other clients may have written since, touching only what `changes` names.
/// `live` prunes pane state of panes the server no longer has.
pub fn merge(
    mut disk: Document,
    mine: &Document,
    epoch: &str,
    changes: &Changes,
    live: Option<&BTreeSet<PaneId>>,
    now: u64,
) -> Document {
    disk.version = VERSION;
    disk.migrated |= mine.migrated;
    if changes.settings {
        disk.settings = mine.settings.clone();
    }
    if changes.layout {
        disk.layout = mine.layout.clone();
    }
    if !epoch.is_empty() {
        let own = mine.epochs.get(epoch).cloned().unwrap_or_default();
        let entry = disk.epochs.entry(epoch.to_owned()).or_default();
        for id in &changes.panes {
            match own.panes.get(id) {
                Some(pane) => {
                    entry.panes.insert(*id, pane.clone());
                }
                None => {
                    entry.panes.remove(id);
                }
            }
        }
        for id in &changes.removed {
            entry.panes.remove(id);
        }
        if changes.order {
            let mut order = own.order.clone();
            // Keep tabs another client knows about and this one does not.
            for id in &entry.order {
                if !order.contains(id) && !changes.removed.contains(id) {
                    order.push(*id);
                }
            }
            entry.order = order;
        }
        entry.order.retain(|id| !changes.removed.contains(id));
        if let Some(live) = live {
            entry.panes.retain(|id, _| live.contains(id));
            entry.order.retain(|id| live.contains(id));
        }
        let mut seen = BTreeSet::new();
        entry.order.retain(|id| seen.insert(*id));
        entry.updated_at = now;
        // Pane IDs of older epochs no longer identify anything: drop epochs
        // not written after this one. A newer epoch only appears if a client
        // of a restarted server wrote while this one was still connected.
        disk.epochs
            .retain(|name, state| name == epoch || state.updated_at > now);
    }
    disk
}

fn check_private(path: &Path) -> Result<bool> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || meta.len() > MAX_BYTES
    {
        return Err(format!(
            "{} must be an owned private file smaller than 1 MiB",
            path.display()
        )
        .into());
    }
    Ok(true)
}

/// Reads the document, or the default when there is none yet.
pub fn load(path: &Path) -> Result<Document> {
    if !check_private(path)? {
        return Ok(Document::default());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let mut bytes = Vec::new();
    (&mut file).take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("TUI state is too large".into());
    }
    let document: Document = serde_json::from_slice(&bytes)?;
    document.validate()?;
    Ok(document)
}

struct Lock(fs::File);
impl Lock {
    fn acquire(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Self(file));
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::WouldBlock || Instant::now() >= deadline {
                return Err(format!("TUI state lock: {error}").into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn write_atomic(path: &Path, document: &Document) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(document)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("TUI state is too large".into());
    }
    let directory = path.parent().ok_or("TUI state needs a directory")?;
    let temporary = directory.join(format!(".{FILE}.{}.tmp", std::process::id()));
    let _ = fs::remove_file(&temporary);
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// The persisted state of one workspace plus this client's pending changes.
#[derive(Debug)]
pub struct Store {
    directory: PathBuf,
    /// This client's view. Pane state is read from and written to
    /// `document.epochs[epoch]`.
    pub document: Document,
    pub epoch: String,
    pub changes: Changes,
    /// Pane IDs the server currently has; prunes state of closed panes.
    pub live: Option<BTreeSet<PaneId>>,
}
impl Store {
    /// Opens the store of the workspace whose state directory is `directory`.
    pub fn open(directory: &Path) -> Result<Self> {
        let document = load(&directory.join(FILE))?;
        Ok(Self {
            directory: directory.to_path_buf(),
            document,
            epoch: String::new(),
            changes: Changes::default(),
            live: None,
        })
    }
    /// A store that is never written, for tests and read-only sessions.
    pub fn memory(document: Document) -> Self {
        Self {
            directory: PathBuf::new(),
            document,
            epoch: String::new(),
            changes: Changes::default(),
            live: None,
        }
    }
    pub fn path(&self) -> PathBuf {
        self.directory.join(FILE)
    }
    /// This client's state for the current epoch, if it has any.
    pub fn current(&self) -> Option<&EpochState> {
        self.document.epochs.get(&self.epoch)
    }
    pub fn current_mut(&mut self) -> &mut EpochState {
        self.document.epochs.entry(self.epoch.clone()).or_default()
    }
    pub fn pane(&self, id: PaneId) -> Option<&PaneState> {
        self.current().and_then(|state| state.panes.get(&id))
    }
    pub fn set_pane(&mut self, id: PaneId, pane: PaneState) {
        let state = self.current_mut();
        if pane == PaneState::default() {
            if state.panes.remove(&id).is_none() {
                return;
            }
        } else if state.panes.get(&id) == Some(&pane) {
            return;
        } else {
            state.panes.insert(id, pane);
        }
        self.changes.panes.insert(id);
    }
    pub fn remove_pane(&mut self, id: PaneId) {
        let state = self.current_mut();
        state.panes.remove(&id);
        state.order.retain(|pane| *pane != id);
        self.changes.panes.remove(&id);
        self.changes.removed.insert(id);
    }
    pub fn set_order(&mut self, order: Vec<PaneId>) {
        if self.current().is_none_or(|state| state.order != order) {
            self.current_mut().order = order;
            self.changes.order = true;
        }
    }
    pub fn set_layout(&mut self, layout: Vec<LayoutTab>) {
        if self.document.layout != layout {
            self.document.layout = layout;
            self.changes.layout = true;
        }
    }
    pub fn set_settings(&mut self, mut settings: UiConfig) {
        // Colours are pane state, keyed by epoch; never save them by pane ID.
        settings.pane_colors.clear();
        if self.document.settings.as_ref() != Some(&settings) {
            self.document.settings = Some(settings);
            self.changes.settings = true;
        }
    }
    pub fn mark_migrated(&mut self) {
        if !self.document.migrated {
            self.document.migrated = true;
            self.changes.migrated = true;
        }
    }
    /// Writes pending changes, merged with what other clients wrote.
    pub fn flush(&mut self) -> Result<()> {
        if self.changes.is_empty() || self.directory.as_os_str().is_empty() {
            self.changes = Changes::default();
            return Ok(());
        }
        let path = self.path();
        let _lock = Lock::acquire(&self.directory.join(LOCK))?;
        // An unreadable file is replaced rather than blocking the TUI.
        let disk = load(&path).unwrap_or_default();
        let merged = merge(
            disk,
            &self.document,
            &self.epoch,
            &self.changes,
            self.live.as_ref(),
            crate::now(),
        );
        write_atomic(&path, &merged)?;
        // Adopt state other clients wrote for panes this client did not touch.
        if let (Some(theirs), false) = (merged.epochs.get(&self.epoch), self.epoch.is_empty()) {
            let own = self.current_mut();
            for (id, pane) in &theirs.panes {
                own.panes.entry(*id).or_insert_with(|| pane.clone());
            }
            own.updated_at = theirs.updated_at;
        }
        self.document.epochs = merged
            .epochs
            .iter()
            .filter(|(name, _)| **name == self.epoch)
            .map(|(name, state)| {
                let mut state = state.clone();
                if let Some(own) = self.document.epochs.get(name) {
                    state.panes = own.panes.clone();
                    state.order = own.order.clone();
                }
                (name.clone(), state)
            })
            .collect();
        self.changes = Changes::default();
        Ok(())
    }
}

/// One-time migration input from the composed server's files in the
/// workspace state directory: UI settings from `workspace.json` and the
/// separate accent/fold preferences, and the tab layout from
/// `tab-layout.json`. The files are read, never modified.
pub fn legacy_settings(directory: &Path) -> Option<UiConfig> {
    let path = directory.join("workspace.json");
    let mut settings = path
        .is_file()
        .then(|| lifecycle::Settings::load(&path).ok())
        .flatten()
        .map(|settings| settings.ui)?;
    if let Ok(folded) = lifecycle::sidebar_folded(&directory.join("sidebar-folded.json")) {
        settings.sidebar_folded = folded;
    }
    if let Ok(accent) = lifecycle::sidebar_accent(&directory.join("sidebar-accent.json")) {
        settings.accent = accent;
    }
    settings.pane_colors.clear();
    Some(settings)
}
pub fn legacy_layout(directory: &Path) -> Vec<LayoutTab> {
    lifecycle::TabLayout::load(&directory.join("tab-layout.json"))
        .map(|layout| {
            layout
                .panes
                .into_iter()
                .map(|pane| LayoutTab {
                    cwd: pane.cwd.to_string_lossy().into_owned(),
                    shell: Some(pane.shell.to_string_lossy().into_owned()),
                    argv: Vec::new(),
                    name: pane.name,
                    color: None,
                    parked: false,
                })
                .collect()
        })
        .unwrap_or_default()
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "kelpmux-tui-state-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            crate::journal::private_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn named(name: &str) -> PaneState {
        PaneState {
            name: Some(name.into()),
            ..Default::default()
        }
    }

    #[test]
    fn concurrent_writers_merge_instead_of_clobbering() {
        let fixture = Fixture::new();
        let mut a = Store::open(&fixture.0).unwrap();
        let mut b = Store::open(&fixture.0).unwrap();
        for store in [&mut a, &mut b] {
            store.epoch = "e1".into();
        }
        a.set_pane(1, named("alpha"));
        a.set_order(vec![1, 2]);
        a.flush().unwrap();
        // B never saw A's write, then changes another pane and the settings.
        b.set_pane(2, named("beta"));
        b.set_settings(UiConfig {
            notification_sound: true,
            ..UiConfig::default()
        });
        b.flush().unwrap();
        let disk = load(&fixture.0.join(FILE)).unwrap();
        let epoch = &disk.epochs["e1"];
        assert_eq!(epoch.panes[&1].name.as_deref(), Some("alpha"));
        assert_eq!(epoch.panes[&2].name.as_deref(), Some("beta"));
        assert_eq!(epoch.order, vec![1, 2]);
        assert!(disk.settings.unwrap().notification_sound);
        // B adopted A's pane after its write, so a later flush keeps it too.
        assert_eq!(b.pane(1).unwrap().name.as_deref(), Some("alpha"));
        let meta = fs::metadata(fixture.0.join(FILE)).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o600);
    }

    #[test]
    fn order_merge_keeps_tabs_only_the_other_writer_knows() {
        let fixture = Fixture::new();
        let mut a = Store::open(&fixture.0).unwrap();
        a.epoch = "e1".into();
        a.set_order(vec![1, 2, 3]);
        a.flush().unwrap();
        let mut b = Store::open(&fixture.0).unwrap();
        b.epoch = "e1".into();
        b.set_order(vec![2, 1]);
        b.flush().unwrap();
        assert_eq!(
            load(&fixture.0.join(FILE)).unwrap().epochs["e1"].order,
            vec![2, 1, 3]
        );
    }

    #[test]
    fn closed_and_vanished_panes_are_pruned() {
        let fixture = Fixture::new();
        let mut a = Store::open(&fixture.0).unwrap();
        a.epoch = "e1".into();
        for id in 1..=4 {
            a.set_pane(id, named(&format!("p{id}")));
        }
        a.set_order(vec![1, 2, 3, 4]);
        a.flush().unwrap();
        a.remove_pane(2);
        // Pane 4 disappeared while this client was not looking.
        a.live = Some([1, 3].into_iter().collect());
        a.flush().unwrap();
        let epoch = load(&fixture.0.join(FILE)).unwrap().epochs["e1"].clone();
        assert_eq!(epoch.panes.keys().copied().collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(epoch.order, vec![1, 3]);
    }

    #[test]
    fn a_new_epoch_drops_old_pane_state_but_keeps_the_layout() {
        let fixture = Fixture::new();
        let mut a = Store::open(&fixture.0).unwrap();
        a.epoch = "old".into();
        a.set_pane(1, named("agent"));
        a.set_layout(vec![LayoutTab {
            cwd: "/tmp".into(),
            name: Some("agent".into()),
            ..Default::default()
        }]);
        a.flush().unwrap();
        let mut b = Store::open(&fixture.0).unwrap();
        b.epoch = "new".into();
        assert!(b.current().is_none());
        b.set_pane(7, named("fresh"));
        b.flush().unwrap();
        let disk = load(&fixture.0.join(FILE)).unwrap();
        assert_eq!(disk.epochs.keys().collect::<Vec<_>>(), vec!["new"]);
        assert_eq!(disk.layout[0].name.as_deref(), Some("agent"));
    }

    #[test]
    fn merge_is_pure_and_only_touches_named_changes() {
        let mut mine = Document::default();
        mine.epochs
            .entry("e".into())
            .or_default()
            .panes
            .insert(1, named("mine"));
        mine.settings = Some(UiConfig {
            prefix: 3,
            ..UiConfig::default()
        });
        let mut disk = Document::default();
        disk.epochs
            .entry("e".into())
            .or_default()
            .panes
            .insert(1, named("theirs"));
        disk.settings = Some(UiConfig::default());
        let merged = merge(disk.clone(), &mine, "e", &Changes::default(), None, 10);
        assert_eq!(merged.epochs["e"].panes[&1].name.as_deref(), Some("theirs"));
        assert_eq!(merged.settings.as_ref().unwrap().prefix, 2);
        let changes = Changes {
            panes: [1].into_iter().collect(),
            settings: true,
            ..Default::default()
        };
        let merged = merge(disk, &mine, "e", &changes, None, 10);
        assert_eq!(merged.epochs["e"].panes[&1].name.as_deref(), Some("mine"));
        assert_eq!(merged.settings.unwrap().prefix, 3);
    }

    #[test]
    fn unsafe_files_are_rejected() {
        let fixture = Fixture::new();
        let path = fixture.0.join(FILE);
        fs::write(&path, b"{}").unwrap();
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        assert!(load(&path).is_err());
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("/etc/hostname", &path).unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn legacy_tab_layout_migrates_names_and_folders() {
        let fixture = Fixture::new();
        let layout = lifecycle::TabLayout {
            template: None,
            cwd: "/tmp".into(),
            shell: "/bin/sh".into(),
            panes: vec![lifecycle::StartupPane {
                name: Some("Build".into()),
                cwd: "/var".into(),
                shell: "/bin/sh".into(),
            }],
        };
        crate::journal::atomic(&fixture.0.join("tab-layout.json"), &layout).unwrap();
        let tabs = legacy_layout(&fixture.0);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].cwd, "/var");
        assert_eq!(tabs[0].name.as_deref(), Some("Build"));
        assert!(legacy_layout(&fixture.0.join("missing")).is_empty());
    }
}
