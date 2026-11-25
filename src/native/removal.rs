//! Safe removal of a project folder: what would be lost, and the guarded
//! removal itself.
//!
//! [`check`] lists everything in a folder that is not safely elsewhere,
//! before anything is deleted: for each Git checkout ([`web_git::discover`],
//! inspected only through its hardened runner, so no hook, filter or
//! transport runs and nothing is written) its uncommitted changes, untracked
//! files that are not ignored, ignored files, stashes, local branches with
//! commits on no remote-tracking branch, branches without an upstream and a
//! detached HEAD with commits on no branch, commits reachable only from
//! tags or other refs, and linked worktrees outside the folder; and the
//! files of the folder outside any checkout, counted with their size. The
//! checkouts found inside a checkout are inspected the same way: its
//! checked-out submodules (and a submodule repository in `.git/modules`
//! that is not checked out is reported), repositories inside its ignored
//! or untracked folders, and its linked worktrees inside the folder.
//! Remote-tracking branches are what the last fetch saw: nothing here
//! contacts a remote.
//!
//! Each finding has a severity. Before a deletion, everything Git alone
//! holds (and a checkout or nested repository that could not be checked) is
//! a blocker, and files outside Git are a warning, since the deletion loses
//! them. Before an archive the Git findings become warnings: the archive
//! holds the `.git` folders too, but those commits and stashes then exist
//! in the archive only, on no remote. A mount point inside the folder, or a
//! folder the removal guards refuse, is always a blocker.
//!
//! [`Check::fingerprint`] summarizes the findings (their counts and sizes
//! included). The check is shown before the user confirms, and run again
//! immediately before the folder is removed; a different fingerprint means
//! something changed, and the removal is abandoned.
//!
//! [`remove`] deletes a folder only inside the configured file roots, never
//! a root, the projects base, the Scratch folder, the home folder or a
//! folder that contains one of them or another project's folder, never
//! across a mount point and never through a symbolic link: it renames the
//! folder to a hidden sibling (`.kelpmux-removing-<random>`, one atomic
//! step, after which nothing at the old path can be half deleted), then
//! deletes that without following links.
use super::{user_config, web_git};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    fs,
    os::unix::{
        ffi::OsStrExt,
        fs::{FileTypeExt, MetadataExt},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, String>;

/// What the folder is checked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Before archiving: Git findings are warnings.
    Archive,
    /// Before deleting: Git findings are blockers.
    Delete,
}
impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::Delete => "delete",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "archive" => Some(Self::Archive),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

/// How bad a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Blocker,
}
impl Severity {
    fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Blocker => "blocker",
        }
    }
}

/// One finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub severity: Severity,
    /// `uncommitted`, `untracked`, `ignored`, `stash`, `unpushed`,
    /// `no_upstream`, `detached`, `unchecked`, `nested_repository`,
    /// `other_files`, `special`, `mount`, `guard` or `missing`.
    pub kind: &'static str,
    /// The checkout or path it concerns.
    pub path: String,
    pub message: String,
}

/// One local branch worth showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub upstream: Option<String>,
    /// Commits on the branch and on no remote-tracking branch.
    pub unpushed: u64,
}

/// What a checkout holds that Git alone has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckoutState {
    pub path: PathBuf,
    pub name: String,
    pub error: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    /// Tracked files with changes (staged or not), and unmerged ones.
    pub uncommitted: u64,
    /// Untracked files that are not ignored.
    pub untracked: u64,
    /// Ignored paths (a wholly ignored folder counts once).
    pub ignored: u64,
    pub stashes: u64,
    /// Commits reachable from a detached HEAD and from no branch.
    pub detached_unpushed: u64,
    /// Branches with unpushed commits or without an upstream.
    pub branches: Vec<Branch>,
    /// Some output reached its limit: the counts are lower bounds.
    pub truncated: bool,
    /// `checkout` (found in the folder), `submodule`, `nested` (inside an
    /// ignored or untracked folder of a checkout) or `worktree`.
    pub kind: &'static str,
    /// Commits reachable from tags, notes, other refs or another
    /// worktree's HEAD, and from no branch, remote branch or this HEAD.
    pub other_refs_unpushed: u64,
    /// Linked worktrees of this repository outside the folder.
    pub outside_worktrees: Vec<PathBuf>,
    /// Searching its ignored and untracked folders for repositories
    /// stopped at the limit.
    pub unscanned: bool,
    /// Its Git folder, resolved.
    pub git_dir: Option<PathBuf>,
    /// Submodule repositories in its `.git/modules`, resolved.
    pub modules: Vec<PathBuf>,
    /// Repositories found inside it to inspect: path and kind.
    pub inner: Vec<(PathBuf, &'static str)>,
}

/// Files of the folder outside every checkout.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OtherFiles {
    /// Regular files and symbolic links.
    pub files: u64,
    pub bytes: u64,
    /// The walk stopped at its limit: lower bounds.
    pub truncated: bool,
}

/// The result of [`check`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub folder: PathBuf,
    pub mode: Mode,
    pub exists: bool,
    pub checkouts: Vec<CheckoutState>,
    pub other_files: OtherFiles,
    pub items: Vec<Item>,
    pub checked_at: u64,
}
impl Check {
    pub fn blockers(&self) -> usize {
        self.count(Severity::Blocker)
    }
    pub fn warnings(&self) -> usize {
        self.count(Severity::Warning)
    }
    fn count(&self, severity: Severity) -> usize {
        self.items.iter().filter(|i| i.severity == severity).count()
    }
    /// Whether the action may go ahead: no blocker.
    pub fn allowed(&self) -> bool {
        self.blockers() == 0
    }
    /// A summary of the findings: equal for two checks that found the same
    /// things (counts and sizes included).
    pub fn fingerprint(&self) -> String {
        let mut text = format!(
            "{}\n{}\n{}\n",
            self.mode.as_str(),
            self.folder.display(),
            self.exists
        );
        for item in &self.items {
            text.push_str(&format!(
                "{}\t{}\t{}\t{}\n",
                item.severity.as_str(),
                item.kind,
                item.path,
                item.message
            ));
        }
        format!("{:016x}", fnv(text.as_bytes()))
    }
    pub fn to_json(&self, project: &str) -> Value {
        json!({
            "project": project,
            "folder": self.folder,
            "mode": self.mode.as_str(),
            "exists": self.exists,
            "checked_at": self.checked_at,
            "fingerprint": self.fingerprint(),
            "checkouts": self.checkouts.iter().map(|c| json!({
                "path": c.path,
                "name": c.name,
                "error": c.error,
                "branch": c.branch,
                "detached": c.detached,
                "uncommitted": c.uncommitted,
                "untracked": c.untracked,
                "ignored": c.ignored,
                "stashes": c.stashes,
                "detached_unpushed": c.detached_unpushed,
                "kind": c.kind,
                "other_refs_unpushed": c.other_refs_unpushed,
                "outside_worktrees": c.outside_worktrees,
                "truncated": c.truncated,
                "branches": c.branches.iter().map(|b| json!({
                    "name": b.name,
                    "upstream": b.upstream,
                    "unpushed": b.unpushed,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "other_files": {
                "files": self.other_files.files,
                "bytes": self.other_files.bytes,
                "truncated": self.other_files.truncated,
            },
            "items": self.items.iter().map(|i| json!({
                "severity": i.severity.as_str(),
                "kind": i.kind,
                "path": i.path,
                "message": i.message,
            })).collect::<Vec<_>>(),
            "blockers": self.blockers(),
            "warnings": self.warnings(),
            "allowed": self.allowed(),
        })
    }
}

/// 64-bit FNV-1a.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The folders a removal must never delete or contain, and the roots it
/// must stay inside.
#[derive(Clone, Debug, Default)]
pub struct Guards {
    /// `files.root` and `files.extra_roots`: a removed folder lies strictly
    /// inside one.
    pub roots: Vec<PathBuf>,
    /// Never removed, nor any folder containing one: the roots, the
    /// projects base, the Scratch folder, the home folder, other projects'
    /// folders, KelpMux's own configuration and state.
    pub protected: Vec<PathBuf>,
}
impl Guards {
    /// The guards of `config` for removing project `id`'s folder.
    pub fn for_project(config: &user_config::Config, id: &str, home: &Path) -> Self {
        let mut roots = vec![config.files_root.path.clone()];
        roots.extend(config.files_extra_roots.iter().map(|f| f.path.clone()));
        let mut protected = roots.clone();
        protected.push(config.projects_base.path.clone());
        protected.push(config.scratch_folder.path.clone());
        protected.push(home.to_path_buf());
        protected.push(PathBuf::from("/"));
        protected.extend(
            config
                .projects
                .iter()
                .filter(|p| p.id != id)
                .map(|p| p.folder.path.clone()),
        );
        if let Ok(path) = user_config::default_path()
            && let Some(parent) = path.parent()
        {
            protected.push(parent.to_path_buf());
        }
        for (variable, fallback) in [
            ("XDG_STATE_HOME", ".local/state"),
            ("XDG_CACHE_HOME", ".cache"),
        ] {
            let base = std::env::var_os(variable)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(fallback));
            protected.push(base.join("kelpmux"));
        }
        Self { roots, protected }
    }
}

/// `path` with symbolic links resolved when it exists, else as written.
fn resolved(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The mount points of this process's mount namespace.
fn mount_points() -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let Ok(text) = fs::read("/proc/self/mountinfo") else {
        return Vec::new();
    };
    text.split(|b| *b == b'\n')
        .filter_map(|line| {
            // The fifth field is the mount point, with spaces, tabs,
            // newlines and backslashes as `\ooo` octal escapes.
            let field = line.split(|b| *b == b' ').nth(4)?;
            let mut bytes = Vec::with_capacity(field.len());
            let mut i = 0;
            while i < field.len() {
                if field[i] == b'\\'
                    && i + 3 < field.len()
                    && field[i + 1..i + 4]
                        .iter()
                        .all(|d| (b'0'..=b'7').contains(d))
                {
                    let digit = |d: u8| u16::from(d - b'0');
                    let value = (digit(field[i + 1]) * 64
                        + digit(field[i + 2]) * 8
                        + digit(field[i + 3])) as u8;
                    bytes.push(value);
                    i += 4;
                } else {
                    bytes.push(field[i]);
                    i += 1;
                }
            }
            Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
        })
        .collect()
}

/// Why `folder` may not be removed, if it may not: it must be a real folder
/// of this user (not a symbolic link), strictly inside a root, contain no
/// protected folder and no mount point, and not be one.
pub fn guard(folder: &Path, guards: &Guards) -> Result<()> {
    let uid = unsafe { libc::geteuid() };
    if !folder.is_absolute() {
        return Err(format!("{} is not an absolute path", folder.display()));
    }
    let meta =
        fs::symlink_metadata(folder).map_err(|_| format!("{} does not exist", folder.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!(
            "{} is a symbolic link; KelpMux removes real folders only",
            folder.display()
        ));
    }
    if !meta.is_dir() || meta.uid() != uid {
        return Err(format!(
            "{} is not a folder of your account",
            folder.display()
        ));
    }
    let canonical = folder
        .canonicalize()
        .map_err(|e| format!("{}: {e}", folder.display()))?;
    let inside = guards
        .roots
        .iter()
        .map(|r| resolved(r))
        .any(|root| canonical.starts_with(&root) && canonical != root);
    if !inside {
        return Err(format!(
            "{} is not inside the file roots (files.root, files.extra_roots)",
            canonical.display()
        ));
    }
    for protected in &guards.protected {
        for candidate in [protected.clone(), resolved(protected)] {
            if candidate.starts_with(&canonical) {
                return Err(format!(
                    "{} is or contains {}, which KelpMux never removes",
                    canonical.display(),
                    candidate.display()
                ));
            }
        }
    }
    if let Some(mount) = mount_points()
        .into_iter()
        .find(|m| m.starts_with(&canonical))
    {
        return Err(format!(
            "{} is or contains the mount point {}; unmount it first",
            canonical.display(),
            mount.display()
        ));
    }
    Ok(())
}

/// Most directory entries the walk of the files outside the checkouts
/// visits.
pub const MAX_WALK: u64 = 1_000_000;
/// Longest a check takes, all checkouts together.
pub const BUDGET: Duration = Duration::from_secs(120);
/// Longest one Git command of a check may run.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(60);
/// Most local branches compared with the remote-tracking ones.
const MAX_BRANCHES: usize = 256;
/// Most repositories one check inspects.
const MAX_CHECKOUTS: usize = 256;

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A byte count for people: `12.3 MB`.
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The NUL-separated lines of Git output.
fn lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes
        .split(|b| *b == b'\n' || *b == 0)
        .filter(|line| !line.is_empty())
}

fn count_lines(bytes: &[u8]) -> u64 {
    lines(bytes).count() as u64
}

/// What one [`check`] shares between its inspections: the folder (resolved),
/// the user, the deadline and the entries left for searching ignored and
/// untracked folders.
struct Scope {
    folder: PathBuf,
    uid: u32,
    deadline: Instant,
    budget: u64,
}

/// Inspects one checkout for what Git alone holds.
fn inspect(root: &Path, kind: &'static str, follow: bool, scope: &mut Scope) -> CheckoutState {
    let mut state = CheckoutState {
        path: root.to_path_buf(),
        name: root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned()),
        kind,
        ..CheckoutState::default()
    };
    if let Err(error) = inspect_into(&mut state, root, follow, scope) {
        state.error = Some(error);
    }
    state
}

/// A commit count Git printed.
fn count(bytes: &[u8]) -> Result<u64> {
    String::from_utf8_lossy(bytes)
        .trim()
        .parse::<u64>()
        .map_err(|_| "unexpected Git commit count".into())
}

/// A relative path Git printed, when it stays inside the checkout.
fn inside(root: &Path, relative: &[u8]) -> Option<PathBuf> {
    let relative = Path::new(std::ffi::OsStr::from_bytes(relative));
    let plain = !relative.as_os_str().is_empty()
        && relative
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
    plain.then(|| root.join(relative))
}

fn inspect_into(
    state: &mut CheckoutState,
    root: &Path,
    follow: bool,
    scope: &mut Scope,
) -> Result<()> {
    let checkout =
        web_git::Checkout::open(root, scope.uid, follow, scope.deadline)?.with_timeout(GIT_TIMEOUT);
    let (bytes, truncated) = checkout.output(&[
        "status",
        "--porcelain=v2",
        "--branch",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=all",
    ])?;
    state.truncated |= truncated;
    let status = web_git::parse_status(&bytes, truncated);
    let has_head = status.head.is_some();
    state.branch = status.branch.clone();
    state.detached = status.branch.is_none() && status.head.is_some();
    for change in &status.changes {
        if change.status == "?" {
            state.untracked += 1;
        } else {
            state.uncommitted += 1;
        }
    }
    // Ignored paths, a wholly ignored folder once (`traditional`), and
    // untracked folders: both are searched for repositories below.
    let (listing, truncated) = checkout.output(&[
        "status",
        "--porcelain=v2",
        "-z",
        "--ignored=traditional",
        "--untracked-files=normal",
        "--ignore-submodules=all",
    ])?;
    state.truncated |= truncated;
    state.unscanned |= truncated;
    state.ignored = listing
        .split(|b| *b == 0)
        .filter(|record| record.starts_with(b"! "))
        .count() as u64;
    // Stashes: the entries of `refs/stash`'s reflog.
    let (bytes, _) = checkout.output(&["for-each-ref", "--format=%(objectname)", "refs/stash"])?;
    if count_lines(&bytes) > 0 {
        let (bytes, truncated) =
            checkout.output(&["log", "--walk-reflogs", "--format=%H", "refs/stash", "--"])?;
        state.truncated |= truncated;
        state.stashes = count_lines(&bytes).max(1);
    }
    // Local branches: their upstream, and commits on no remote-tracking
    // branch. Compared by object name, never by a name Git could read as
    // an option.
    let (bytes, truncated) = checkout.output(&[
        "for-each-ref",
        "--format=%(objectname)%00%(refname:short)%00%(upstream:short)",
        "refs/heads/",
    ])?;
    state.truncated |= truncated;
    let rows: Vec<&[u8]> = bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .collect();
    if rows.len() > MAX_BRANCHES {
        return Err(format!(
            "more than {MAX_BRANCHES} local branches; check this checkout in a terminal"
        ));
    }
    for row in rows {
        let mut fields = row.split(|b| *b == 0);
        let (Some(object), Some(name), upstream) = (fields.next(), fields.next(), fields.next())
        else {
            return Err("unexpected Git branch listing".into());
        };
        let object = String::from_utf8_lossy(object).into_owned();
        if object.is_empty() || !object.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("unexpected Git branch listing".into());
        }
        let upstream = upstream
            .filter(|u| !u.is_empty())
            .map(|u| String::from_utf8_lossy(u).into_owned());
        let (bytes, _) =
            checkout.output(&["rev-list", "--count", &object, "--not", "--remotes", "--"])?;
        let unpushed = count(&bytes)?;
        if unpushed > 0 || upstream.is_none() {
            state.branches.push(Branch {
                name: String::from_utf8_lossy(name)
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect(),
                upstream,
                unpushed,
            });
        }
    }
    if state.detached {
        let (bytes, _) = checkout.output(&[
            "rev-list",
            "--count",
            "HEAD",
            "--not",
            "--remotes",
            "--branches",
            "--",
        ])?;
        state.detached_unpushed = count(&bytes)?;
    }
    // Its Git folder, and the common one its worktrees share.
    let (bytes, _) = checkout.output(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-dir",
        "--git-common-dir",
    ])?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let mut dirs = text.lines().map(|line| PathBuf::from(line.trim()));
    let (Some(git_dir), Some(common)) = (dirs.next(), dirs.next()) else {
        return Err("unexpected Git folder answer".into());
    };
    if !git_dir.is_absolute() || !common.is_absolute() {
        return Err("unexpected Git folder answer".into());
    }
    state.git_dir = Some(resolved(&git_dir));
    let common = resolved(&common);
    let repository_goes = common.starts_with(&scope.folder);
    // Linked worktrees: their HEADs count below; one outside the folder
    // loses its repository with the folder, one inside is inspected.
    let (bytes, truncated) = checkout.output(&["worktree", "list", "--porcelain", "-z"])?;
    if truncated {
        return Err("too many worktrees to check".into());
    }
    let mut heads: Vec<String> = Vec::new();
    let this = resolved(root);
    let mut path: Option<PathBuf> = None;
    let mut head: Option<String> = None;
    let mut bare = false;
    let mut first = true;
    let is_main = state.git_dir.as_deref() == Some(common.as_path());
    // Records are NUL-terminated lines, worktrees separated by an empty one.
    for line in bytes.split(|b| *b == 0).chain(std::iter::once(&b""[..])) {
        if line.is_empty() {
            if let Some(worktree) = path.take() {
                let worktree = resolved(&worktree);
                // The first is the main worktree: this checkout itself unless
                // this is a linked worktree (a submodule's may name its Git
                // folder).
                let main_is_this = first && is_main;
                if worktree != this && !bare && !main_is_this {
                    if let Some(head) = head.take() {
                        heads.push(head);
                    }
                    if repository_goes && fs::symlink_metadata(&worktree).is_ok() {
                        if worktree.starts_with(&scope.folder) {
                            state.inner.push((worktree, "worktree"));
                        } else {
                            state.outside_worktrees.push(worktree);
                        }
                    }
                }
            }
            head = None;
            first = false;
            bare = false;
        } else if let Some(rest) = line.strip_prefix(b"worktree ") {
            path = Some(PathBuf::from(std::ffi::OsStr::from_bytes(rest)));
        } else if let Some(rest) = line.strip_prefix(b"HEAD ") {
            let rest = String::from_utf8_lossy(rest).into_owned();
            if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_hexdigit()) {
                head = Some(rest);
            }
        } else if line == b"bare" {
            bare = true;
        }
    }
    // Commits only on tags, notes or other refs (or another worktree's
    // HEAD): on no branch, no remote branch, and not this HEAD (a detached
    // one is counted above). The stash is counted as stashes.
    let mut args = vec!["rev-list", "--count", "--exclude=refs/stash", "--all"];
    args.extend(heads.iter().map(String::as_str));
    args.extend(["--not", "--remotes", "--branches"]);
    if has_head {
        args.push("HEAD");
    }
    args.push("--");
    let (bytes, _) = checkout.output(&args)?;
    state.other_refs_unpushed = count(&bytes)?;
    // Submodules: the paths `.gitmodules` names and the gitlinks of the
    // index; a checked-out one is inspected like any checkout.
    let mut submodules: Vec<PathBuf> = Vec::new();
    if fs::symlink_metadata(root.join(".gitmodules")).is_ok_and(|m| m.is_file())
        && let Ok((bytes, _)) = checkout.output(&[
            "config",
            "-z",
            "--file",
            ".gitmodules",
            "--get-regexp",
            r"^submodule\..*\.path$",
        ])
    {
        for record in bytes.split(|b| *b == 0) {
            if let Some(newline) = record.iter().position(|b| *b == b'\n')
                && let Some(path) = inside(root, &record[newline + 1..])
            {
                submodules.push(path);
            }
        }
    }
    let (bytes, truncated) = checkout.output(&["ls-files", "--stage", "-z"])?;
    if !truncated {
        for record in bytes.split(|b| *b == 0) {
            if let Some(rest) = record.strip_prefix(b"160000 ")
                && let Some(tab) = rest.iter().position(|b| *b == b'\t')
                && let Some(path) = inside(root, &rest[tab + 1..])
            {
                submodules.push(path);
            }
        }
    }
    submodules.sort();
    submodules.dedup();
    for submodule in submodules {
        let real_dir = fs::symlink_metadata(&submodule).is_ok_and(|m| m.is_dir());
        if real_dir && fs::symlink_metadata(submodule.join(".git")).is_ok() {
            state.inner.push((submodule, "submodule"));
        }
    }
    // Submodule repositories kept in the common Git folder: each must be
    // one a checked-out submodule uses, or it is reported.
    let modules = common.join("modules");
    if repository_goes && fs::symlink_metadata(&modules).is_ok_and(|m| m.is_dir()) {
        let mut stack = vec![(modules, 0)];
        while let Some((dir, depth)) = stack.pop() {
            if fs::symlink_metadata(dir.join("HEAD")).is_ok_and(|m| m.is_file())
                && fs::symlink_metadata(dir.join("objects")).is_ok_and(|m| m.is_dir())
            {
                state.modules.push(resolved(&dir));
                continue;
            }
            if depth >= 16 || scope.budget == 0 {
                state.unscanned = true;
                continue;
            }
            for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
                scope.budget = scope.budget.saturating_sub(1);
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push((entry.path(), depth + 1));
                }
            }
        }
    }
    // Repositories inside ignored or untracked folders (`vendor/` with a
    // clone of its own, say), which Git reports as one folder.
    let device = fs::symlink_metadata(root).map_err(|e| e.to_string())?.dev();
    for record in listing.split(|b| *b == 0) {
        let Some(relative) = record
            .strip_prefix(b"! ")
            .or_else(|| record.strip_prefix(b"? "))
        else {
            continue;
        };
        let Some(relative) = relative.strip_suffix(b"/") else {
            continue;
        };
        let Some(start) = inside(root, relative) else {
            continue;
        };
        let mut stack = vec![start];
        while let Some(dir) = stack.pop() {
            if Instant::now() >= scope.deadline || scope.budget == 0 {
                state.unscanned = true;
                break;
            }
            let Ok(meta) = fs::symlink_metadata(&dir) else {
                continue;
            };
            if !meta.is_dir() || meta.dev() != device {
                continue;
            }
            if fs::symlink_metadata(dir.join(".git")).is_ok() {
                state.inner.push((dir, "nested"));
                continue;
            }
            let Ok(entries) = fs::read_dir(&dir) else {
                state.unscanned = true;
                continue;
            };
            for entry in entries.flatten() {
                scope.budget = scope.budget.saturating_sub(1);
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(entry.path());
                }
            }
        }
    }
    Ok(())
}

/// What the walk of the folder outside its checkouts found.
#[derive(Default)]
struct Walk {
    other: OtherFiles,
    /// Folders with Git metadata that are not checked checkouts.
    nested: Vec<PathBuf>,
    /// Sockets, FIFOs and devices.
    special: Vec<PathBuf>,
    /// Folders on another file system.
    mounts: Vec<PathBuf>,
    /// Entries that could not be read.
    unreadable: Vec<PathBuf>,
}

/// Walks `folder` without following symbolic links, skipping `skip` (the
/// checkouts) and staying on its file system.
fn walk(folder: &Path, skip: &[PathBuf], deadline: Instant) -> Walk {
    let mut found = Walk::default();
    let Ok(root) = fs::symlink_metadata(folder) else {
        return found;
    };
    if skip.iter().any(|s| s == folder) {
        // The folder is a checkout: Git's status covers all of it.
        return found;
    }
    let device = root.dev();
    let mut stack = vec![folder.to_path_buf()];
    let mut visited = 0u64;
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => {
                found.unreadable.push(dir);
                continue;
            }
        };
        for entry in entries {
            visited += 1;
            if visited > MAX_WALK || Instant::now() >= deadline {
                found.other.truncated = true;
                return found;
            }
            let Ok(entry) = entry else {
                found.unreadable.push(dir.clone());
                continue;
            };
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                found.unreadable.push(path);
                continue;
            };
            let kind = meta.file_type();
            if entry.file_name() == ".git" && !skip.contains(&dir) {
                // Git metadata in a folder nobody checked.
                if !found.nested.contains(&dir) {
                    found.nested.push(dir.clone());
                }
            }
            if kind.is_dir() {
                if meta.dev() != device {
                    found.mounts.push(path);
                } else if !skip.contains(&path) {
                    stack.push(path);
                }
            } else if kind.is_file() || kind.is_symlink() {
                found.other.files += 1;
                if kind.is_file() {
                    found.other.bytes += meta.len();
                }
            } else if kind.is_socket()
                || kind.is_fifo()
                || kind.is_block_device()
                || kind.is_char_device()
            {
                found.special.push(path);
            }
        }
    }
    found
}

/// Checks `folder` for what removing it would lose (see the module
/// documentation). `guards` are the removal's: a folder they refuse gets a
/// blocker.
pub fn check(folder: &Path, mode: Mode, guards: &Guards) -> Check {
    let mut check = Check {
        folder: folder.to_path_buf(),
        mode,
        exists: fs::symlink_metadata(folder).is_ok(),
        checkouts: Vec::new(),
        other_files: OtherFiles::default(),
        items: Vec::new(),
        checked_at: crate::now(),
    };
    let show = |path: &Path| path.to_string_lossy().into_owned();
    if !check.exists {
        check.items.push(Item {
            severity: Severity::Info,
            kind: "missing",
            path: show(folder),
            message: "the folder does not exist: there is nothing to remove".into(),
        });
        return check;
    }
    if let Err(error) = guard(folder, guards) {
        check.items.push(Item {
            severity: Severity::Blocker,
            kind: "guard",
            path: show(folder),
            message: error,
        });
        return check;
    }
    let uid = unsafe { libc::geteuid() };
    let deadline = Instant::now() + BUDGET;
    let git = if mode == Mode::Delete {
        Severity::Blocker
    } else {
        Severity::Warning
    };
    let only = if mode == Mode::Archive {
        " (kept in the archive's .git only, on no remote)"
    } else {
        ""
    };
    let (roots, truncated) = web_git::discover(folder, uid);
    let mut scope = Scope {
        folder: resolved(folder),
        uid,
        deadline,
        budget: MAX_WALK,
    };
    // The checkouts found in the folder, then those found inside them
    // (submodules, repositories in ignored folders, worktrees), each once.
    let mut queue: VecDeque<(PathBuf, &'static str)> = roots
        .iter()
        .map(|root| (root.clone(), "checkout"))
        .collect();
    let mut seen: Vec<PathBuf> = roots.iter().map(|root| resolved(root)).collect();
    let mut inspected: Vec<PathBuf> = Vec::new();
    while let Some((root, kind)) = queue.pop_front() {
        if inspected.len() >= MAX_CHECKOUTS {
            check.items.push(Item {
                severity: git,
                kind: "unchecked",
                path: show(&root),
                message: format!(
                    "more than {MAX_CHECKOUTS} repositories: this one and the rest were not checked"
                ),
            });
            break;
        }
        let state = inspect(&root, kind, root == folder, &mut scope);
        for (path, kind) in &state.inner {
            let real = resolved(path);
            if !seen.contains(&real) {
                seen.push(real);
                queue.push_back((path.clone(), kind));
            }
        }
        inspected.push(root.clone());
        let root = &root;
        let at = show(root);
        let mut item = |severity: Severity, kind: &'static str, message: String| {
            check.items.push(Item {
                severity,
                kind,
                path: at.clone(),
                message,
            });
        };
        let more = if state.truncated { "at least " } else { "" };
        if let Some(error) = &state.error {
            item(
                git,
                "unchecked",
                format!("could not be checked ({error}): treat everything in it as at risk"),
            );
        } else {
            if state.uncommitted > 0 {
                item(
                    git,
                    "uncommitted",
                    format!(
                        "{more}{} with uncommitted changes{only}",
                        plural(state.uncommitted, "file", "files")
                    ),
                );
            }
            if state.untracked > 0 {
                item(
                    git,
                    "untracked",
                    format!(
                        "{more}{} not committed and not ignored{only}",
                        plural(state.untracked, "untracked file", "untracked files")
                    ),
                );
            }
            if state.stashes > 0 {
                item(
                    git,
                    "stash",
                    format!("{}{only}", plural(state.stashes, "stash", "stashes")),
                );
            }
            for branch in &state.branches {
                if branch.unpushed > 0 {
                    item(
                        git,
                        "unpushed",
                        format!(
                            "branch {}: {} on no remote branch{only}",
                            branch.name,
                            plural(branch.unpushed, "commit", "commits")
                        ),
                    );
                } else {
                    item(
                        Severity::Warning,
                        "no_upstream",
                        format!(
                            "branch {} has no upstream (its commits are on a remote branch)",
                            branch.name
                        ),
                    );
                }
            }
            if state.detached_unpushed > 0 {
                item(
                    git,
                    "detached",
                    format!(
                        "detached HEAD: {} on no branch{only}",
                        plural(state.detached_unpushed, "commit", "commits")
                    ),
                );
            }
            if state.other_refs_unpushed > 0 {
                item(
                    git,
                    "other_refs",
                    format!(
                        "{} only on tags, notes or other refs (or another worktree's HEAD), on no branch or remote branch{only}",
                        plural(state.other_refs_unpushed, "commit", "commits")
                    ),
                );
            }
            for worktree in &state.outside_worktrees {
                item(
                    git,
                    "worktree",
                    format!(
                        "a linked worktree outside the folder, at {}, uses this repository: {}",
                        worktree.display(),
                        if mode == Mode::Delete {
                            "deleting the folder breaks it and loses its work; remove it first (git worktree remove)"
                        } else {
                            "it stops working once the folder is archived"
                        }
                    ),
                );
            }
            if state.unscanned {
                item(
                    git,
                    "unchecked",
                    "its ignored and untracked folders were not fully searched for repositories of their own".into(),
                );
            }
            if state.ignored > 0 {
                item(
                    if mode == Mode::Delete {
                        Severity::Warning
                    } else {
                        Severity::Info
                    },
                    "ignored",
                    format!(
                        "{more}{} (build output, dependencies, local settings such as .env) that Git does not keep{}",
                        plural(state.ignored, "ignored path", "ignored paths"),
                        if mode == Mode::Delete {
                            ": deleting loses them"
                        } else {
                            ": the archive keeps them"
                        }
                    ),
                );
            }
        }
        check.checkouts.push(state);
    }
    // Submodule repositories no checked-out submodule uses.
    let git_dirs: Vec<&PathBuf> = check
        .checkouts
        .iter()
        .filter_map(|c| c.git_dir.as_ref())
        .collect();
    for state in &check.checkouts {
        for module in &state.modules {
            if !git_dirs.contains(&module) {
                check.items.push(Item {
                    severity: git,
                    kind: "unchecked",
                    path: show(module),
                    message: format!(
                        "a submodule repository of {} that is not checked out, so it was not checked: check it in a terminal",
                        state.name
                    ),
                });
            }
        }
    }
    let found = walk(folder, &inspected, deadline);
    check.other_files = found.other.clone();
    if truncated {
        check.items.push(Item {
            severity: git,
            kind: "unchecked",
            path: show(folder),
            message: format!(
                "more than {} checkouts: the others were not checked",
                web_git::MAX_REPOS
            ),
        });
    }
    for nested in &found.nested {
        if inspected.contains(nested) {
            continue;
        }
        check.items.push(Item {
            severity: git,
            kind: "nested_repository",
            path: show(nested),
            message: format!(
                "a Git repository KelpMux did not check (hidden or nested){}",
                if mode == Mode::Delete {
                    ": check it in a terminal"
                } else {
                    "; the archive keeps it"
                }
            ),
        });
    }
    for mount in &found.mounts {
        check.items.push(Item {
            severity: Severity::Blocker,
            kind: "mount",
            path: show(mount),
            message: "another file system is mounted here; unmount it first".into(),
        });
    }
    for path in &found.unreadable {
        check.items.push(Item {
            severity: Severity::Blocker,
            kind: "unchecked",
            path: show(path),
            message: "could not be read".into(),
        });
    }
    if !found.special.is_empty() {
        check.items.push(Item {
            severity: Severity::Warning,
            kind: "special",
            path: show(&found.special[0]),
            message: format!(
                "{} (sockets, FIFOs or devices){}",
                plural(found.special.len() as u64, "special file", "special files"),
                if mode == Mode::Archive {
                    " are not archived"
                } else {
                    ""
                }
            ),
        });
    }
    if found.other.files > 0 || found.other.truncated {
        let more = if found.other.truncated {
            "at least "
        } else {
            ""
        };
        let (severity, tail) = match mode {
            Mode::Delete => (Severity::Warning, ": deleting loses them"),
            Mode::Archive => (Severity::Info, ": the archive keeps them"),
        };
        check.items.push(Item {
            severity: if found.other.truncated && mode == Mode::Delete {
                Severity::Blocker
            } else {
                severity
            },
            kind: "other_files",
            path: show(folder),
            message: format!(
                "{more}{} ({more}{}) outside Git repositories{tail}",
                plural(found.other.files, "file", "files"),
                size_text(found.other.bytes)
            ),
        });
    }
    check
}

/// Removes `folder` after [`guard`]: renames it to a hidden sibling
/// (`.kelpmux-removing-<random>`), then deletes that without following
/// symbolic links or crossing a mount point. When deleting fails part way,
/// the error names the hidden folder that is left.
pub fn remove(folder: &Path, guards: &Guards) -> Result<()> {
    guard(folder, guards)?;
    let canonical = folder
        .canonicalize()
        .map_err(|e| format!("{}: {e}", folder.display()))?;
    let before = fs::symlink_metadata(&canonical).map_err(|e| e.to_string())?;
    let parent = canonical
        .parent()
        .ok_or("the folder has no parent")?
        .to_path_buf();
    let hidden = parent.join(format!(".kelpmux-removing-{}", random_hex()?));
    rename_new(&canonical, &hidden)?;
    let after = fs::symlink_metadata(&hidden).map_err(|e| e.to_string())?;
    if after.file_type().is_symlink() || after.dev() != before.dev() || after.ino() != before.ino()
    {
        return Err(format!(
            "{} changed while it was being removed; left at {}",
            canonical.display(),
            hidden.display()
        ));
    }
    // A last look for another file system below it.
    let mounts = mount_points();
    if let Some(mount) = mounts.iter().find(|m| m.starts_with(&hidden)) {
        return Err(format!(
            "{} has a mount point inside; left at {}",
            mount.display(),
            hidden.display()
        ));
    }
    if let Some(mount) = foreign_device(&hidden, before.dev()) {
        return Err(format!(
            "{} is on another file system; nothing was deleted, the folder is at {}",
            mount.display(),
            hidden.display()
        ));
    }
    // std's remove_dir_all never follows symbolic links (it opens each
    // folder relative to its parent with O_NOFOLLOW).
    fs::remove_dir_all(&hidden).map_err(|e| {
        format!(
            "deleting {} failed ({e}); what is left is at {}",
            canonical.display(),
            hidden.display()
        )
    })
}

/// A folder below `root` on another device than `device`, if any. On the
/// way, makes each of this user's folders below `root` (which is being
/// deleted) writable and searchable by the user, so read-only folders go
/// too; never through a symbolic link (an `O_PATH | O_NOFOLLOW` descriptor
/// is changed through `/proc/self/fd`).
fn foreign_device(root: &Path, device: u64) -> Option<PathBuf> {
    let uid = unsafe { libc::geteuid() };
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(meta) = fs::symlink_metadata(&dir)
            && meta.is_dir()
            && meta.uid() == uid
            && meta.mode() & 0o700 != 0o700
        {
            owner_full(&dir, meta.mode());
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                if meta.dev() != device {
                    return Some(path);
                }
                stack.push(path);
            }
        }
    }
    None
}

/// Adds `u+rwx` to the folder `dir` (not a link to one).
fn owner_full(dir: &Path, mode: u32) {
    use std::os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    };
    let Ok(handle) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(dir)
    else {
        return;
    };
    let proc = PathBuf::from(format!("/proc/self/fd/{}", handle.as_raw_fd()));
    let _ = fs::set_permissions(proc, fs::Permissions::from_mode((mode & 0o7777) | 0o700));
}

/// 64 random bits as 16 hexadecimal digits.
pub fn random_hex() -> Result<String> {
    let mut bytes = [0u8; 8];
    // SAFETY: writes at most `bytes.len()` bytes into `bytes`.
    let n = unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), 0) };
    if n != bytes.len() as isize {
        return Err("no randomness available".into());
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Renames `from` to `to`, failing when `to` exists (`RENAME_NOREPLACE`).
pub fn rename_new(from: &Path, to: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let from_c = std::ffi::CString::new(from.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let to_c = std::ffi::CString::new(to.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: both are NUL-terminated paths that outlive the call.
    let renamed = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from_c.as_ptr(),
            libc::AT_FDCWD,
            to_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if renamed == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    Err(match error.raw_os_error() {
        Some(libc::EEXIST) => format!("{} already exists", to.display()),
        Some(libc::EINVAL | libc::ENOSYS) => format!(
            "the file system of {} cannot rename without replacing",
            to.display()
        ),
        Some(libc::EXDEV) => format!(
            "{} and {} are on different file systems",
            from.display(),
            to.display()
        ),
        _ => format!("renaming {} failed: {error}", from.display()),
    })
}

#[cfg(test)]
mod tests;
