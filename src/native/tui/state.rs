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
//!
//! Each TUI also follows the others live: it remembers the identity of the
//! file version it last read or wrote (`Stamp`) and, when `poll` or `flush`
//! finds another version, adopts it with its own pending changes merged on
//! top, so local changes win for the fields they touched.
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
    /// The tab's shell when it is not the workspace's; recreating the tab
    /// passes it to `spawn` (`spawn-shell`). `None` uses the server's shell.
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

/// The identity of one version of the state file. Every write creates a new
/// file and renames it over the old one, so a new version has a new inode
/// and change time; size and modification time guard against inode reuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
impl Stamp {
    fn of(meta: &fs::Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            size: meta.size(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
            ctime: (meta.ctime(), meta.ctime_nsec()),
        }
    }
    /// The stamp of the file at `path`, `None` when there is none.
    fn at(path: &Path) -> Result<Option<Self>> {
        match fs::symlink_metadata(path) {
            Ok(meta) => Ok(Some(Self::of(&meta))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

/// Reads the document, or the default when there is none yet.
pub fn load(path: &Path) -> Result<Document> {
    read(path).map(|(document, _)| document)
}

/// Reads the document and the stamp of the version read.
fn read(path: &Path) -> Result<(Document, Option<Stamp>)> {
    if !check_private(path)? {
        return Ok((Document::default(), None));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let stamp = Stamp::of(&file.metadata()?);
    let mut bytes = Vec::new();
    (&mut file).take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("TUI state is too large".into());
    }
    let document: Document = serde_json::from_slice(&bytes)?;
    document.validate()?;
    Ok((document, Some(stamp)))
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
    /// The file version this client last read or wrote.
    seen: Option<Stamp>,
    /// `document` adopted another client's write; see `take_reloaded`.
    reloaded: bool,
}
impl Store {
    /// Opens the store of the workspace whose state directory is `directory`.
    pub fn open(directory: &Path) -> Result<Self> {
        let (document, seen) = read(&directory.join(FILE))?;
        Ok(Self {
            directory: directory.to_path_buf(),
            document,
            epoch: String::new(),
            changes: Changes::default(),
            live: None,
            seen,
            reloaded: false,
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
            seen: None,
            reloaded: false,
        }
    }
    /// Whether `document` adopted another client's write since the last
    /// call. The caller then applies it to the live UI.
    pub fn take_reloaded(&mut self) -> bool {
        std::mem::take(&mut self.reloaded)
    }
    /// Adopts `disk` with this client's pending changes merged on top: local
    /// changes win for the fields they touched. The changes stay pending.
    fn adopt(&mut self, disk: Document) {
        let mut view = merge(
            disk,
            &self.document,
            &self.epoch,
            &self.changes,
            self.live.as_ref(),
            crate::now(),
        );
        if !self.epoch.is_empty() {
            view.epochs.retain(|name, _| *name == self.epoch);
        }
        self.document = view;
        self.reloaded = true;
    }
    /// Adopts the file when another client replaced it since this client
    /// last read or wrote it. One `lstat` when nothing changed. Returns
    /// whether it adopted a new version.
    pub fn poll(&mut self) -> Result<bool> {
        if self.directory.as_os_str().is_empty() {
            return Ok(false);
        }
        let path = self.path();
        let current = Stamp::at(&path)?;
        if current == self.seen {
            return Ok(false);
        }
        match read(&path) {
            Ok((disk, Some(stamp))) => {
                self.seen = Some(stamp);
                self.adopt(disk);
                Ok(true)
            }
            // Removed: keep the current view; the next flush recreates it.
            Ok((_, None)) => {
                self.seen = None;
                Ok(false)
            }
            Err(error) => {
                // Report an unreadable version once, not on every poll.
                self.seen = current;
                Err(error)
            }
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
        let (disk, external) = match read(&path) {
            Ok((disk, stamp)) => (disk, stamp != self.seen),
            Err(_) => (Document::default(), false),
        };
        let merged = merge(
            disk,
            &self.document,
            &self.epoch,
            &self.changes,
            self.live.as_ref(),
            crate::now(),
        );
        write_atomic(&path, &merged)?;
        // Still under the lock: this is the version just written, which a
        // later poll must not mistake for another client's.
        self.seen = Stamp::at(&path).ok().flatten();
        if external {
            // Another client wrote since this one last looked: adopt the
            // merged document, which already carries this client's changes.
            self.changes = Changes::default();
            self.adopt(merged);
            return Ok(());
        }
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
/// `tab-layout.json`. The files are read, never modified. KelpServer's
/// composed settings types are converted through their JSON form, which the
/// TUI's copies share.
pub fn legacy_settings(directory: &Path) -> Option<UiConfig> {
    let path = directory.join("workspace.json");
    let saved = path
        .is_file()
        .then(|| lifecycle::Settings::load(&path).ok())
        .flatten()?;
    let mut settings: UiConfig =
        serde_json::from_value(serde_json::to_value(saved.ui).ok()?).ok()?;
    if let Ok(folded) = lifecycle::sidebar_folded(&directory.join("sidebar-folded.json")) {
        settings.sidebar_folded = folded;
    }
    if let Some(accent) = lifecycle::sidebar_accent(&directory.join("sidebar-accent.json"))
        .ok()
        .and_then(|accent| serde_json::to_value(accent).ok())
        .and_then(|accent| serde_json::from_value(accent).ok())
    {
        settings.accent = accent;
    }
    settings.pane_colors.clear();
    settings.validate().ok()?;
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
                    // The composed layout names every tab's shell; only one
                    // other than the layout's workspace shell is kept.
                    shell: (pane.shell != layout.shell)
                        .then(|| pane.shell.to_string_lossy().into_owned()),
                    argv: Vec::new(),
                    name: pane.name,
                    color: None,
                    parked: false,
                })
                .collect()
        })
        .unwrap_or_default()
}
/// The workspace's configured startup template (`kelpmux start --template`,
/// saved in `workspace.json`) as layout tabs, and the workspace shell, which
/// `spawn` uses when a tab names none.
pub fn workspace_template(directory: &Path) -> (Vec<LayoutTab>, Option<String>) {
    let path = directory.join("workspace.json");
    let Some(settings) = path
        .is_file()
        .then(|| lifecycle::Settings::load(&path).ok())
        .flatten()
    else {
        return (Vec::new(), None);
    };
    let shell = settings
        .shell
        .as_ref()
        .map(|shell| shell.to_string_lossy().into_owned());
    let tabs = settings
        .template
        .map(|template| {
            template
                .panes
                .into_iter()
                .map(|pane| LayoutTab {
                    cwd: pane.cwd.to_string_lossy().into_owned(),
                    shell: pane
                        .shell
                        .map(|shell| shell.to_string_lossy().into_owned())
                        .filter(|pane_shell| Some(pane_shell) != shell.as_ref()),
                    argv: Vec::new(),
                    name: Some(pane.name),
                    color: None,
                    parked: false,
                })
                .collect()
        })
        .unwrap_or_default();
    (tabs, shell)
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
    fn poll_adopts_other_writes_and_ignores_its_own() {
        let fixture = Fixture::new();
        let mut a = Store::open(&fixture.0).unwrap();
        let mut b = Store::open(&fixture.0).unwrap();
        for store in [&mut a, &mut b] {
            store.epoch = "e1".into();
        }
        a.set_pane(1, named("alpha"));
        a.set_order(vec![2, 1]);
        a.flush().unwrap();
        // A's own write is not a change for A.
        assert!(!a.poll().unwrap());
        assert!(!a.take_reloaded());
        // B sees it once.
        assert!(b.poll().unwrap());
        assert!(b.take_reloaded());
        assert_eq!(b.pane(1).unwrap().name.as_deref(), Some("alpha"));
        assert_eq!(b.current().unwrap().order, vec![2, 1]);
        assert!(!b.poll().unwrap());
        // B's write is new for A, and not for B.
        b.set_pane(2, named("beta"));
        b.flush().unwrap();
        assert!(!b.take_reloaded());
        assert!(!b.poll().unwrap());
        assert!(a.poll().unwrap());
        assert_eq!(a.pane(2).unwrap().name.as_deref(), Some("beta"));
    }

    #[test]
    fn a_reload_keeps_pending_local_changes() {
        let fixture = Fixture::new();
        let mut a = Store::open(&fixture.0).unwrap();
        let mut b = Store::open(&fixture.0).unwrap();
        for store in [&mut a, &mut b] {
            store.epoch = "e1".into();
        }
        a.set_order(vec![1, 2, 3]);
        a.flush().unwrap();
        b.poll().unwrap();
        // B parks 3 and changes the order; not flushed yet.
        b.set_pane(
            3,
            PaneState {
                parked: true,
                ..Default::default()
            },
        );
        b.set_order(vec![3, 1, 2]);
        // Meanwhile A names 1 and 3 and reorders.
        a.set_pane(1, named("alpha"));
        a.set_pane(3, named("gamma"));
        a.set_order(vec![2, 1, 3]);
        a.flush().unwrap();
        assert!(b.poll().unwrap());
        // B adopted A's pane 1 and kept its own pending pane 3 and order.
        assert_eq!(b.pane(1).unwrap().name.as_deref(), Some("alpha"));
        assert!(b.pane(3).unwrap().parked);
        assert_eq!(b.pane(3).unwrap().name, None);
        assert_eq!(b.current().unwrap().order, vec![3, 1, 2]);
        assert!(b.changes.order && b.changes.panes.contains(&3));
        // Its flush writes them over A's version of the same fields only.
        b.flush().unwrap();
        let disk = load(&fixture.0.join(FILE)).unwrap();
        let epoch = &disk.epochs["e1"];
        assert_eq!(epoch.panes[&1].name.as_deref(), Some("alpha"));
        assert!(epoch.panes[&3].parked);
        assert_eq!(epoch.order, vec![3, 1, 2]);
        // A flush that finds another client's write adopts it as well.
        a.set_pane(2, named("beta"));
        b.set_pane(1, named("one"));
        b.flush().unwrap();
        a.flush().unwrap();
        assert!(a.take_reloaded());
        assert_eq!(a.pane(1).unwrap().name.as_deref(), Some("one"));
        assert_eq!(a.pane(2).unwrap().name.as_deref(), Some("beta"));
        assert!(!a.poll().unwrap());
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
