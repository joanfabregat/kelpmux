//! File operations over the pane stream (capability `files`, ADR 0003
//! section 3): list, stat, read, write, create a folder, rename, move to the
//! freedesktop trash and delete, on the host where the server and its shells
//! run. They serve the file browsers and editors of stream clients.
//!
//! These operations grant nothing a stream connection does not already have:
//! any connection may type into a shell running as the server's user. They
//! run with the server's credentials and never elevate. What they add is a
//! boundary for the file browser and care against surprises:
//!
//! - Paths are absolute (or `~`-relative to the server user's home),
//!   lexically normalised, and may not contain `..`.
//! - Every path must lie inside the file root ([`Root`]: the server user's
//!   home directory unless [`Settings::root`] names another folder), and is
//!   resolved beneath it: `openat2` with `RESOLVE_BENEATH`, or, where that
//!   refuses (an absolute symlink) or is missing, a walk that opens one
//!   component at a time with `O_NOFOLLOW` and resolves each symlink itself
//!   (at most [`MAX_SYMLINKS`]), refusing one that leads out of the root.
//!   Anything outside fails with `outside_root`. A symlink pointing outside
//!   is still listed and stat'ed as a link (its target shown), but nothing
//!   is read, written, listed or downloaded through it. Mount points inside
//!   the root stay usable. The root itself can be listed and stat'ed, never
//!   written, renamed, moved, trashed or deleted.
//! - Extra roots ([`Settings::extra_roots`], outside the primary root and
//!   each other) are reachable the same way: a path inside one is resolved
//!   beneath that root only, so a symlink never leads from one root into
//!   another (`outside_root`), and a rename between two roots fails with
//!   `cross_root` (nothing is copied).
//! - Every change (write, mkdir, rename, trash, delete) acts on the final
//!   component through its parent's directory descriptor and never follows a
//!   symlink there: writing to a symlink is refused, deleting or trashing one
//!   removes the link, never its target, and recursive deletion opens every
//!   directory with `O_NOFOLLOW`. Reads, stats and listings follow symlinks
//!   in intermediate components like a shell does (within the root);
//!   `file_stat` and listing entries describe a symlink itself (`lstat`) and
//!   report its target.
//! - Nothing that is not a regular file is opened for reading or writing:
//!   the type is checked first, since opening a device or FIFO can have side
//!   effects.
//! - Writes are atomic replacements: a temporary file in the same directory,
//!   synced, then renamed over the target, keeping the target's permission
//!   bits and group. A read-only target is refused, like a shell's `>`. A
//!   `base_hash` turns the write into a compare-and-swap on the content's
//!   SHA-256, re-checked against the file's metadata just before the rename.
//! - `/`, the server user's home directory and the file roots are never
//!   removed, and recursive deletion never crosses into another file system.
//! - Sizes, entry counts, paths per request and concurrent operations are
//!   bounded ([`LIMITS`], advertised in Welcome `file_limits`).
//!
//! Operations run on worker threads, at most [`MAX_IN_FLIGHT`] per
//! connection and [`MAX_SERVER_IN_FLIGHT`] per server (of which at most
//! [`MAX_SERVER_DELETES`] deletions), so a slow disk never stalls pane
//! output. There is no cancellation: an operation stuck on a hung mount keeps
//! its slot until it returns.
//!
//! The descriptor-relative design, the text/binary classification and the
//! atomic replacement are adapted from KelpHub's workspace file browser
//! (`server/src/management/files.rs`), moved here with the owner's approval
//! (decision 185b).
use super::stream::{Event, Request};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{CStr, CString, OsStr},
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::ffi::OsStrExt,
    },
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// Largest file `file_read` returns. Its base64 form (about 10.7 MiB) fits
/// one 16 MiB record.
pub const MAX_READ_BYTES: u64 = 8 * 1024 * 1024;
/// Largest content `file_write` accepts, after decoding.
pub const MAX_WRITE_BYTES: u64 = 8 * 1024 * 1024;
/// Most entries one `file_list` returns: the first ones in listing order.
pub const MAX_LIST_ENTRIES: usize = 5000;
/// Most directory entries one listing scans. Only the kept entries are held
/// and examined, so this bounds the time of a listing, not its memory.
const MAX_LIST_SCAN: usize = 200_000;
/// Most paths in one `file_trash` or `file_delete`.
pub const MAX_PATHS: usize = 256;
/// Concurrent file operations per connection.
pub const MAX_IN_FLIGHT: usize = 4;
/// Concurrent file operations per server.
pub const MAX_SERVER_IN_FLIGHT: usize = 16;
/// Concurrent `file_delete` operations per server. A recursive deletion
/// holds one directory descriptor per level (up to [`MAX_DEPTH`]), so this
/// bounds the descriptors all file operations can hold to a few hundred.
pub const MAX_SERVER_DELETES: usize = 4;
/// Longest accepted path, in bytes.
pub const MAX_PATH_BYTES: usize = 4096;
/// Deepest directory nesting `file_delete` removes.
pub const MAX_DEPTH: usize = 64;
/// Longest symlink target reported.
const MAX_TARGET_BYTES: usize = 4096;
/// Largest serialized reply, below the 16 MiB record with room to spare.
const REPLY_BUDGET: usize = 15 * 1024 * 1024;
/// Largest JSON-escaped text content; beyond it content is sent as base64.
const TEXT_BUDGET: usize = 12 * 1024 * 1024;
/// Longest file name on Linux file systems.
const NAME_MAX: usize = 255;
const TRASHINFO: &str = ".trashinfo";
/// Most symlinks followed while resolving one path (Linux's own bound).
pub const MAX_SYMLINKS: usize = 40;
/// Most extra file roots ([`Settings::extra_roots`]); further ones are
/// skipped.
pub const MAX_EXTRA_ROOTS: usize = 64;

/// File bounds advertised in Welcome `file_limits`, with the file roots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub max_read_bytes: u64,
    pub max_write_bytes: u64,
    pub max_list_entries: usize,
    pub max_paths: usize,
    pub max_in_flight: usize,
    pub max_path_bytes: usize,
    pub max_depth: usize,
    /// `file-downloads`: download bounds and the flow-control window.
    /// Absent from servers without downloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<super::downloads::Limits>,
    /// The primary file root (canonical, without symlinks). Every path of a
    /// file operation must lie inside it or inside an extra root (`roots`).
    /// Absent from servers without the boundary, and from a server that
    /// could not resolve its root (every file operation then fails with
    /// `outside_root`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// Every file root with a label, the primary root (`root`) first, then
    /// the extra roots. Absent from servers without extra roots support,
    /// and whenever `root` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots: Option<Vec<NamedRoot>>,
}
/// The constant bounds; the server adds its [`Root`] (`root`, `roots`).
pub const LIMITS: Limits = Limits {
    max_read_bytes: MAX_READ_BYTES,
    max_write_bytes: MAX_WRITE_BYTES,
    max_list_entries: MAX_LIST_ENTRIES,
    max_paths: MAX_PATHS,
    max_in_flight: MAX_IN_FLIGHT,
    max_path_bytes: MAX_PATH_BYTES,
    max_depth: MAX_DEPTH,
    downloads: Some(super::downloads::LIMITS),
    root: None,
    roots: None,
};
impl Limits {
    /// The bounds with `root`'s paths (both absent without a root).
    pub fn with_root(root: Option<&Root>) -> Self {
        Self {
            root: root.map(|root| path_text(root.path())),
            roots: root.map(Root::named),
            ..LIMITS
        }
    }
}

/// Server configuration of file operations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// The trash directory (holding `files/` and `info/`). `None` is the
    /// freedesktop home trash, `$XDG_DATA_HOME/Trash` or
    /// `~/.local/share/Trash`.
    pub trash_dir: Option<PathBuf>,
    /// The folder every file operation is confined to (`--files-root`).
    /// `None` is the server user's home directory (`HOME`, or the password
    /// database's entry when `HOME` is unset or relative). It is resolved
    /// once, when the server starts ([`Root::with_extras`]).
    pub root: Option<PathBuf>,
    /// Further absolute folders outside the primary root that file
    /// operations may also reach (`--files-extra-root`). Resolved with the
    /// primary root; one that is missing or overlaps another root is
    /// skipped.
    pub extra_roots: Vec<PathBuf>,
}

/// One folder of the file roots, resolved when the server starts:
/// canonical (its own path has no symlink), an existing directory. Paths
/// inside it are resolved beneath it and never leave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Tree {
    path: PathBuf,
    /// The configured spelling when it differs from the canonical one (a
    /// home directory reached through a symlink, say): request paths under
    /// it name the same entries and are answered in the canonical spelling.
    alias: Option<PathBuf>,
    /// The directory's device and inode at startup. Every later open of
    /// the folder must find the same directory ([`Tree::open`]), so that
    /// replacing it or a parent (with a symlink, say) never redirects file
    /// operations elsewhere.
    id: (libc::dev_t, libc::ino_t),
}
impl Tree {
    /// Resolve the configured folder `given` (`what` names it in errors).
    fn resolve(given: &Path, what: &str) -> std::result::Result<Self, String> {
        if !given.is_absolute() {
            return Err(format!(
                "the {what} {} is not an absolute path",
                given.display()
            ));
        }
        let path = std::fs::canonicalize(given)
            .map_err(|error| format!("the {what} {}: {error}", given.display()))?;
        // The canonical path's last component is no symlink: opened without
        // following one, it is the directory recorded below.
        let fd = cstring(path.as_os_str().as_bytes())
            .and_then(|cpath| open_at(libc::AT_FDCWD, &cpath, libc::O_PATH | libc::O_NOFOLLOW, 0))
            .and_then(|fd| stat_fd(fd.as_raw_fd()))
            .map_err(|error| format!("the {what} {}: {error}", given.display()))?;
        if !is_dir(&fd) {
            return Err(format!("the {what} {} is not a directory", given.display()));
        }
        let alias = normalize_home(given).ok().filter(|alias| *alias != path);
        Ok(Self {
            path,
            alias,
            id: (fd.st_dev, fd.st_ino),
        })
    }
    /// Whether `st` describes this folder (as recorded at startup).
    fn is(&self, st: &libc::stat) -> bool {
        (st.st_dev, st.st_ino) == self.id
    }
    /// The canonical path of the folder.
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    /// The longest prefix of `path` naming this folder (its canonical path
    /// or its alias), in components; `None` when `path` is outside.
    fn prefix(&self, path: &Path) -> Option<usize> {
        [Some(&self.path), self.alias.as_ref()]
            .into_iter()
            .flatten()
            .filter(|prefix| path.starts_with(prefix))
            .map(|prefix| prefix.components().count())
            .max()
    }
    /// `path` spelled from the canonical folder; `outside_root` when it is
    /// not inside it. Purely lexical.
    fn confine(&self, path: &Path) -> Outcome<PathBuf> {
        if path.starts_with(&self.path) {
            return Ok(path.to_path_buf());
        }
        if let Some(rest) = self
            .alias
            .as_deref()
            .and_then(|alias| path.strip_prefix(alias).ok())
        {
            return Ok(if rest.as_os_str().is_empty() {
                self.path.clone()
            } else {
                self.path.join(rest)
            });
        }
        Err(outside(path))
    }
    /// A confined path relative to the folder (empty for the folder itself).
    fn relative<'a>(&self, path: &'a Path) -> Outcome<&'a Path> {
        path.strip_prefix(&self.path).map_err(|_| outside(path))
    }
    /// Open the folder (`O_PATH`), refusing anything but the directory
    /// recorded at startup: a root replaced since, or reached through a
    /// parent that was, fails with `outside_root` until the server
    /// restarts.
    fn open(&self) -> Outcome<OwnedFd> {
        let cpath =
            cstring(self.path.as_os_str().as_bytes()).map_err(|e| io_error(e, &self.path))?;
        let fd = open_at(libc::AT_FDCWD, &cpath, libc::O_PATH | libc::O_NOFOLLOW, 0)
            .map_err(|e| io_error(e, &self.path))?;
        let st = stat_fd(fd.as_raw_fd()).map_err(|e| io_error(e, &self.path))?;
        if !self.is(&st) {
            return Err(Error::new(
                Code::OutsideRoot,
                "the file root is no longer the folder the server started with",
            )
            .at(&self.path));
        }
        Ok(fd)
    }
}

/// A file root advertised in Welcome `file_limits.roots`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedRoot {
    /// The canonical path.
    pub path: String,
    /// A short name for a file browser: `Home` for the primary root when it
    /// is the home directory, else the folder's name (the full path when
    /// that name is empty or shared by another root).
    pub label: String,
}

/// The folders file operations are confined to, resolved when the server
/// starts: the primary root, then the extra roots ([`Settings::extra_roots`]).
/// No root lies inside another, so every confined path belongs to exactly
/// one of them, and it is resolved beneath that one only: a symlink never
/// leads from one root into another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    /// The primary root first, then the extra roots in configured order.
    trees: Vec<Tree>,
    /// The home directory `~` names, canonical when it can be resolved.
    home: Option<PathBuf>,
}
impl Root {
    /// Resolve `configured`, or the server user's home directory, with no
    /// extra root.
    pub fn new(configured: Option<&Path>) -> std::result::Result<Self, String> {
        Self::with_extras(configured, &[]).map(|(root, _)| root)
    }
    /// Resolve the primary root (`configured`, or the server user's home
    /// directory) and the extra roots. An extra root that is missing, not
    /// a directory or not absolute is skipped, and so is one that repeats
    /// another, lies inside the primary root or another extra root (the
    /// outer one is kept), or contains the primary root (which would
    /// widen the boundary around it). Each skip is described in the
    /// returned warnings; only a primary root that cannot be resolved is an
    /// error.
    pub fn with_extras(
        configured: Option<&Path>,
        extras: &[PathBuf],
    ) -> std::result::Result<(Self, Vec<String>), String> {
        let home = home();
        let given = match configured {
            Some(path) => path.to_path_buf(),
            None => home
                .clone()
                .ok_or("the server user has no absolute home directory")?,
        };
        let primary = Tree::resolve(&given, "file root")?;
        let mut warnings = Vec::new();
        let skip = |warnings: &mut Vec<String>, given: &Path, reason: &str| {
            warnings.push(format!(
                "the extra file root {} is skipped: {reason}",
                given.display()
            ));
        };
        // Resolved, and apart from the primary root.
        let mut candidates: Vec<(&Path, Tree)> = Vec::new();
        for (index, extra) in extras.iter().enumerate() {
            if index >= MAX_EXTRA_ROOTS {
                skip(&mut warnings, extra, "too many extra file roots");
                continue;
            }
            match Tree::resolve(extra, "extra file root") {
                Ok(tree) if tree.path.starts_with(&primary.path) => {
                    skip(&mut warnings, extra, "it is inside the file root")
                }
                Ok(tree) if primary.path.starts_with(&tree.path) => {
                    skip(&mut warnings, extra, "it contains the file root")
                }
                Ok(tree) => candidates.push((extra, tree)),
                Err(error) => warnings.push(format!("{error}; it is skipped")),
            }
        }
        let mut trees = vec![primary];
        for (index, (given, tree)) in candidates.iter().enumerate() {
            if candidates[..index]
                .iter()
                .any(|(_, other)| other.path == tree.path)
            {
                skip(&mut warnings, given, "it repeats another extra file root");
            } else if candidates
                .iter()
                .any(|(_, other)| other.path != tree.path && tree.path.starts_with(&other.path))
            {
                skip(&mut warnings, given, "it is inside another extra file root");
            } else {
                trees.push(tree.clone());
            }
        }
        let home = home.and_then(|home| {
            std::fs::canonicalize(&home)
                .ok()
                .or_else(|| normalize_home(&home).ok())
        });
        Ok((Self { trees, home }, warnings))
    }
    /// The canonical path of the primary root.
    pub fn path(&self) -> &Path {
        &self.trees[0].path
    }
    /// The canonical paths of every root, the primary first.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.trees.iter().map(|tree| tree.path.as_path())
    }
    /// Every root with its label, the primary first (Welcome
    /// `file_limits.roots`).
    pub fn named(&self) -> Vec<NamedRoot> {
        let base = |path: &Path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let primary = self.path();
        let first = if self.home.as_deref() == Some(primary) {
            "Home".to_owned()
        } else {
            base(primary)
        };
        let mut labels = vec![first];
        labels.extend(self.trees[1..].iter().map(|tree| base(&tree.path)));
        self.trees
            .iter()
            .zip(&labels)
            .enumerate()
            .map(|(index, (tree, label))| {
                let shared = labels.iter().filter(|other| *other == label).count() > 1;
                NamedRoot {
                    path: path_text(&tree.path),
                    label: if label.is_empty() || (index > 0 && shared) {
                        path_text(&tree.path)
                    } else {
                        label.clone()
                    },
                }
            })
            .collect()
    }
    /// The home directory `~` names.
    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }
    /// An absolute, normalised path inside one of the roots, spelled from
    /// that root's canonical path; `outside_root` otherwise. The root
    /// whose path or alias is the longest prefix decides. Purely lexical:
    /// symlinks are checked when the path is opened ([`open_beneath`]).
    pub fn confine(&self, path: &Path) -> Outcome<PathBuf> {
        self.trees
            .iter()
            .filter_map(|tree| tree.prefix(path).map(|length| (length, tree)))
            .max_by_key(|(length, _)| *length)
            .ok_or_else(|| outside(path))?
            .1
            .confine(path)
    }
    /// A request path, normalised (`~` is the home directory) and confined.
    pub fn request_path(&self, path: &str) -> Outcome<PathBuf> {
        self.confine(&normalize(path, self.home())?)
    }
    /// The root a confined path lies in.
    pub(super) fn tree(&self, path: &Path) -> Outcome<&Tree> {
        self.trees
            .iter()
            .find(|tree| path.starts_with(&tree.path))
            .ok_or_else(|| outside(path))
    }
    /// The primary root.
    #[cfg(test)]
    pub(super) fn primary(&self) -> &Tree {
        &self.trees[0]
    }
    /// A confined path relative to its root (empty for a root itself).
    pub(super) fn relative<'a>(&self, path: &'a Path) -> Outcome<&'a Path> {
        self.tree(path)?.relative(path)
    }
}
fn outside(path: &Path) -> Error {
    Error::new(Code::OutsideRoot, "the path is outside the file root").at(path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    File,
    Dir,
    Symlink,
    /// A FIFO, socket or device.
    Other,
}
/// One file system entry, as `lstat` sees it (a symlink is never followed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    /// Bytes (the link length for a symlink).
    pub size: u64,
    /// Last modification, Unix seconds.
    pub modified: i64,
    /// Permission bits, including set-id and sticky bits (`st_mode & 0o7777`).
    /// The entry is executable when `mode & 0o111` is not zero.
    pub mode: u32,
    /// A symlink's target as text, not followed; invalid UTF-8 is replaced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}
/// How `content` is carried in `file_read` replies and `file_write` requests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    /// The content is the text itself.
    #[default]
    Utf8,
    /// Standard padded base64 of the bytes.
    Base64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// Not absolute, not normalisable, too long, or with no final name.
    InvalidPath,
    /// A request field combination or count that is not accepted.
    InvalidRequest,
    /// `file_write` content that is not valid base64.
    InvalidContent,
    NotFound,
    PermissionDenied,
    NotADirectory,
    IsADirectory,
    /// Not a regular file (a FIFO, socket or device).
    NotAFile,
    /// The final component is a symlink, which this operation never follows.
    Symlink,
    Exists,
    NotEmpty,
    /// `base_hash` differs from the current content (`current_hash`).
    Conflict,
    /// Over a size, entry or depth bound (`size` carries a file's size).
    TooLarge,
    /// Different file systems: rename cannot move it, or the trash is
    /// elsewhere.
    CrossDevice,
    ReadOnly,
    NoSpace,
    /// The connection or server already runs the most file operations.
    Busy,
    /// Any other I/O error; `message` has the system's description.
    Io,
    /// The path, or a symlink on the way to it, leads outside the file
    /// roots (or out of the root the path lies in).
    OutsideRoot,
    /// A rename whose two ends lie in different file roots: entries are not
    /// moved or copied between roots.
    CrossRoot,
}
/// A structured failure of a file operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Error {
    pub code: Code,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `conflict`: the hash of the content now on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_hash: Option<String>,
    /// `too_large`: the file's size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}
impl Error {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            path: None,
            current_hash: None,
            size: None,
        }
    }
    pub(super) fn at(mut self, path: &Path) -> Self {
        self.path = Some(path.to_string_lossy().into_owned());
        self
    }
}
pub(super) type Outcome<T> = std::result::Result<T, Error>;
/// A system error, classified.
pub(super) fn io_error(error: io::Error, path: &Path) -> Error {
    let code = match error.raw_os_error() {
        Some(libc::ENOENT) => Code::NotFound,
        Some(libc::EACCES | libc::EPERM) => Code::PermissionDenied,
        Some(libc::ENOTDIR) => Code::NotADirectory,
        Some(libc::EISDIR) => Code::IsADirectory,
        // Opening a socket, or a device without a driver.
        Some(libc::ENXIO | libc::ENODEV) => Code::NotAFile,
        Some(libc::ELOOP) => Code::Symlink,
        Some(libc::EEXIST) => Code::Exists,
        Some(libc::ENOTEMPTY) => Code::NotEmpty,
        Some(libc::EXDEV) => Code::CrossDevice,
        Some(libc::EROFS) => Code::ReadOnly,
        Some(libc::ENOSPC | libc::EDQUOT) => Code::NoSpace,
        Some(libc::EFBIG) => Code::TooLarge,
        Some(libc::ENAMETOOLONG | libc::EINVAL) => Code::InvalidPath,
        _ => Code::Io,
    };
    Error::new(code, error.to_string()).at(path)
}
/// A removed path of `file_trash` or `file_delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Removed {
    pub path: String,
    /// `file_trash`: where the entry now is (`<trash>/files/<name>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trashed_as: Option<String>,
}
/// A path of `file_trash` or `file_delete` that was not removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub path: String,
    pub error: Error,
}

/// The server user's home directory: `HOME`, or the password database's
/// entry for the effective user when `HOME` is unset or not absolute.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(passwd_home)
}
fn passwd_home() -> Option<PathBuf> {
    let mut buffer = vec![0u8; 16 * 1024];
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let status = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            &mut entry,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut found,
        )
    };
    if status != 0 || found.is_null() || entry.pw_dir.is_null() {
        return None;
    }
    let dir = unsafe { CStr::from_ptr(entry.pw_dir) };
    Some(PathBuf::from(OsStr::from_bytes(dir.to_bytes()))).filter(|path| path.is_absolute())
}
/// Lexically normalise a request path: `~` and `~/…` start at `home`;
/// otherwise it must be absolute. Empty and `.` components are dropped; `..`,
/// `~user`, NUL and paths over `MAX_PATH_BYTES` are refused.
pub fn normalize(path: &str, home: Option<&Path>) -> Outcome<PathBuf> {
    let invalid = |message: &str| Error::new(Code::InvalidPath, message);
    if path.is_empty() || path.len() > MAX_PATH_BYTES || path.contains('\0') {
        return Err(invalid("path must be 1 to 4096 bytes without NUL"));
    }
    let (mut result, rest) = if path == "~" || path.starts_with("~/") {
        let home = home.ok_or_else(|| invalid("the server user has no absolute HOME"))?;
        (normalize_home(home)?, &path[1..])
    } else if path.starts_with('~') {
        return Err(invalid("only ~ and ~/ paths name a home directory"));
    } else if path.starts_with('/') {
        (PathBuf::from("/"), path)
    } else {
        return Err(invalid("path must be absolute or start with ~/"));
    };
    for part in rest.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(invalid("path must not contain ..")),
            name => result.push(name),
        }
    }
    if result.as_os_str().len() > MAX_PATH_BYTES {
        return Err(invalid("path is too long"));
    }
    Ok(result)
}
fn normalize_home(home: &Path) -> Outcome<PathBuf> {
    let mut result = PathBuf::from("/");
    for component in home.components() {
        match component {
            Component::Normal(name) => result.push(name),
            Component::RootDir | Component::CurDir => {}
            _ => {
                return Err(Error::new(
                    Code::InvalidPath,
                    "the server user's HOME is not a normalised path",
                ));
            }
        }
    }
    Ok(result)
}
pub(super) fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(super) fn check(result: libc::c_int) -> io::Result<libc::c_int> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}
pub(super) fn cstring(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
}
pub(super) fn open_at(
    dir: RawFd,
    name: &CStr,
    flags: libc::c_int,
    mode: libc::c_uint,
) -> io::Result<OwnedFd> {
    let fd = check(unsafe { libc::openat(dir, name.as_ptr(), flags | libc::O_CLOEXEC, mode) })?;
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
pub(super) const DIRECTORY: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY;
pub(super) fn stat_at(dir: RawFd, name: &CStr) -> io::Result<libc::stat> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    check(unsafe { libc::fstatat(dir, name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) })?;
    Ok(st)
}
pub(super) fn stat_fd(fd: RawFd) -> io::Result<libc::stat> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    check(unsafe { libc::fstat(fd, &mut st) })?;
    Ok(st)
}
pub(super) fn is_dir(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFDIR
}
pub(super) fn is_regular(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFREG
}
pub(super) fn is_symlink(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFLNK
}
/// Refuse what is not a regular file, before it is opened.
pub(super) fn regular(st: &libc::stat, path: &Path, verb: &str) -> Outcome<()> {
    if is_dir(st) {
        return Err(Error::new(Code::IsADirectory, format!("cannot {verb} a directory")).at(path));
    }
    if !is_regular(st) {
        return Err(Error::new(Code::NotAFile, "not a regular file").at(path));
    }
    Ok(())
}
/// The same inode, unchanged in size, content time and status time.
fn same_version(a: &libc::stat, b: &libc::stat) -> bool {
    (
        a.st_dev,
        a.st_ino,
        a.st_size,
        a.st_mtime,
        a.st_mtime_nsec,
        a.st_ctime,
        a.st_ctime_nsec,
    ) == (
        b.st_dev,
        b.st_ino,
        b.st_size,
        b.st_mtime,
        b.st_mtime_nsec,
        b.st_ctime,
        b.st_ctime_nsec,
    )
}
fn read_link_at(dir: RawFd, name: &CStr) -> Option<String> {
    let mut buffer = vec![0u8; MAX_TARGET_BYTES];
    let length =
        unsafe { libc::readlinkat(dir, name.as_ptr(), buffer.as_mut_ptr().cast(), buffer.len()) };
    if length < 0 {
        return None;
    }
    buffer.truncate(length as usize);
    Some(String::from_utf8_lossy(&buffer).into_owned())
}
fn entry(name: String, st: &libc::stat, dir: RawFd, cname: &CStr) -> Entry {
    let kind = match st.st_mode & libc::S_IFMT {
        libc::S_IFREG => Kind::File,
        libc::S_IFDIR => Kind::Dir,
        libc::S_IFLNK => Kind::Symlink,
        _ => Kind::Other,
    };
    Entry {
        name,
        kind,
        size: st.st_size as u64,
        modified: st.st_mtime,
        mode: st.st_mode & 0o7777,
        target: (kind == Kind::Symlink)
            .then(|| read_link_at(dir, cname))
            .flatten(),
    }
}
/// A directory's entry names (without `.` and `..`), at most `limit` of
/// them; `true` when the directory had more.
pub(super) fn names(dir: RawFd, limit: usize) -> io::Result<(Vec<CString>, bool)> {
    let copy = check(unsafe { libc::fcntl(dir, libc::F_DUPFD_CLOEXEC, 0) })?;
    let stream = unsafe { libc::fdopendir(copy) };
    if stream.is_null() {
        let error = io::Error::last_os_error();
        unsafe { libc::close(copy) };
        return Err(error);
    }
    unsafe { libc::rewinddir(stream) };
    let mut names = Vec::new();
    let mut more = false;
    loop {
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        if names.len() == limit {
            more = true;
            break;
        }
        names.push(name.to_owned());
    }
    unsafe { libc::closedir(stream) };
    Ok((names, more))
}
/// The first [`MAX_LIST_ENTRIES`] names of a directory in listing order
/// (directories first, then by name bytes), selected while scanning at most
/// [`MAX_LIST_SCAN`] entries: only the kept names are held, and the type
/// comes from `d_type` (a `stat` only where the file system leaves it
/// unknown). Returns the kept names, whether entries were left out, and how
/// many names were not UTF-8.
fn select(dir: RawFd) -> io::Result<(Vec<CString>, bool, usize)> {
    let copy = check(unsafe { libc::fcntl(dir, libc::F_DUPFD_CLOEXEC, 0) })?;
    let stream = unsafe { libc::fdopendir(copy) };
    if stream.is_null() {
        let error = io::Error::last_os_error();
        unsafe { libc::close(copy) };
        return Err(error);
    }
    unsafe { libc::rewinddir(stream) };
    // A max-heap of `(not a directory, name)`: its top is the entry that
    // sorts last, the first to drop when a smaller one arrives.
    let mut kept: std::collections::BinaryHeap<(bool, CString)> =
        std::collections::BinaryHeap::with_capacity(MAX_LIST_ENTRIES + 1);
    let mut truncated = false;
    let mut skipped = 0;
    let mut scanned = 0;
    loop {
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        if scanned == MAX_LIST_SCAN {
            truncated = true;
            break;
        }
        scanned += 1;
        // A name that is not UTF-8 cannot be addressed by a JSON path.
        if std::str::from_utf8(name.to_bytes()).is_err() {
            skipped += 1;
            continue;
        }
        let directory = match unsafe { (*entry).d_type } {
            libc::DT_DIR => true,
            libc::DT_UNKNOWN => stat_at(dir, name).is_ok_and(|st| is_dir(&st)),
            _ => false,
        };
        if kept.len() == MAX_LIST_ENTRIES {
            truncated = true;
            let later = kept.peek().is_some_and(|(top_file, top)| {
                (!directory, name.to_bytes_with_nul()) >= (*top_file, top.to_bytes_with_nul())
            });
            if later {
                continue;
            }
            kept.pop();
        }
        kept.push((!directory, name.to_owned()));
    }
    unsafe { libc::closedir(stream) };
    Ok((
        kept.into_iter().map(|(_, name)| name).collect(),
        truncated,
        skipped,
    ))
}
fn random_hex() -> io::Result<String> {
    let mut bytes = [0u8; 8];
    if unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), 0) } != bytes.len() as isize
    {
        return Err(io::Error::last_os_error());
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// `RESOLVE_*` flags of `openat2` (`linux/openat2.h`), and its `open_how`,
/// written out so that any `libc` 0.2 builds.
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_BENEATH: u64 = 0x08;
#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}
/// Set once `openat2` is known to be missing (Linux before 5.6, or a
/// seccomp filter answering `ENOSYS`).
static NO_OPENAT2: AtomicBool = AtomicBool::new(false);
fn openat2_beneath(dir: RawFd, path: &CStr, flags: libc::c_int) -> io::Result<OwnedFd> {
    let how = OpenHow {
        flags: (flags | libc::O_CLOEXEC) as u64,
        mode: 0,
        resolve: RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS,
    };
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            dir,
            path.as_ptr(),
            &how as *const OpenHow,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd as RawFd) })
}
/// Open `rel` (relative to the root `root`, as [`Root::relative`] returns
/// it) beneath that root with `flags`, following symlinks only where they
/// stay inside it, never into another root; `follow` decides for the final
/// component (`O_NOFOLLOW` otherwise). `path` is the absolute path, for
/// errors.
///
/// `openat2` with `RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS` resolves it in
/// the kernel. It refuses an absolute symlink even when the link points
/// inside the root, and some systems lack it; then [`walk`] decides.
pub(super) fn open_beneath(
    root: &Tree,
    rel: &Path,
    flags: libc::c_int,
    follow: bool,
    path: &Path,
) -> Outcome<OwnedFd> {
    let base = root.open()?;
    let flags = if follow {
        flags
    } else {
        flags | libc::O_NOFOLLOW
    };
    if !NO_OPENAT2.load(Ordering::Relaxed) {
        let name = if rel.as_os_str().is_empty() {
            c".".to_owned()
        } else {
            cstring(rel.as_os_str().as_bytes()).map_err(|e| io_error(e, path))?
        };
        // `EAGAIN`: a rename elsewhere raced the lookup; the kernel asks
        // for a retry.
        for _ in 0..4 {
            match openat2_beneath(base.as_raw_fd(), &name, flags) {
                Ok(fd) => return Ok(fd),
                Err(error) => match error.raw_os_error() {
                    Some(libc::EAGAIN) => continue,
                    Some(libc::ENOSYS) => {
                        NO_OPENAT2.store(true, Ordering::Relaxed);
                        break;
                    }
                    // Out of the root, or an absolute symlink.
                    Some(libc::EXDEV) => break,
                    _ => return Err(io_error(error, path)),
                },
            }
        }
    }
    walk(root, &base, rel, flags, path)
}
/// Push the components of `bytes` so that the first is popped first.
fn push_components(pending: &mut Vec<Vec<u8>>, bytes: &[u8]) {
    for part in bytes.rsplit(|&b| b == b'/') {
        if !part.is_empty() {
            pending.push(part.to_vec());
        }
    }
}
/// Resolve `rel` beneath the root one component at a time: every
/// directory is opened with `O_NOFOLLOW` relative to the one before, so the
/// kernel never follows a symlink; each symlink met is read and its target
/// resolved here, an absolute one only when it names a path inside the
/// root, and `..` returns to the directory it came from, never above the
/// root. A symlink that leads outside, into another root included, fails
/// with `outside_root`.
fn walk(
    root: &Tree,
    base: &OwnedFd,
    rel: &Path,
    flags: libc::c_int,
    path: &Path,
) -> Outcome<OwnedFd> {
    let fail = |error: io::Error| io_error(error, path);
    let follow = flags & libc::O_NOFOLLOW == 0;
    // The directories entered below the root, innermost last.
    let mut stack: Vec<OwnedFd> = Vec::new();
    let mut pending = Vec::new();
    push_components(&mut pending, rel.as_os_str().as_bytes());
    let mut links = 0;
    let link = |links: &mut usize| {
        *links += 1;
        if *links > MAX_SYMLINKS {
            return Err(fail(io::Error::from_raw_os_error(libc::ELOOP)));
        }
        Ok(())
    };
    while let Some(name) = pending.pop() {
        let dir = stack.last().unwrap_or(base).as_raw_fd();
        match name.as_slice() {
            b"." => continue,
            b".." => {
                if stack.pop().is_none() {
                    return Err(outside(path));
                }
                continue;
            }
            _ => {}
        }
        let last = pending.is_empty();
        let cname = cstring(&name).map_err(fail)?;
        if last && !follow {
            return open_at(dir, &cname, flags, 0).map_err(fail);
        }
        let st = stat_at(dir, &cname).map_err(fail)?;
        if is_symlink(&st) {
            link(&mut links)?;
            let target = read_link_bytes(dir, &cname).map_err(fail)?;
            if target.first() == Some(&b'/') {
                let inside = root
                    .confine(Path::new(OsStr::from_bytes(&target)))
                    .map_err(|_| outside(path))?;
                let rest = root.relative(&inside)?;
                stack.clear();
                push_components(&mut pending, rest.as_os_str().as_bytes());
            } else {
                push_components(&mut pending, &target);
            }
            continue;
        }
        let child_flags = if last {
            flags | libc::O_NOFOLLOW
        } else {
            libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW
        };
        match open_at(dir, &cname, child_flags, 0) {
            Ok(fd) if last => return Ok(fd),
            Ok(fd) => stack.push(fd),
            // Replaced by a symlink since it was examined: examine it again.
            Err(error)
                if matches!(error.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR))
                    && stat_at(dir, &cname).is_ok_and(|st| is_symlink(&st)) =>
            {
                link(&mut links)?;
                pending.push(name);
            }
            Err(error) => return Err(fail(error)),
        }
    }
    // The root itself, or a directory reached through `..`.
    let dir = stack.last().unwrap_or(base).as_raw_fd();
    open_at(dir, c".", flags, 0).map_err(fail)
}
/// A symlink's target as bytes, not followed.
fn read_link_bytes(dir: RawFd, name: &CStr) -> io::Result<Vec<u8>> {
    let mut buffer = vec![0u8; MAX_PATH_BYTES + 1];
    let length =
        unsafe { libc::readlinkat(dir, name.as_ptr(), buffer.as_mut_ptr().cast(), buffer.len()) };
    if length < 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize > MAX_PATH_BYTES {
        return Err(io::Error::from_raw_os_error(libc::ENAMETOOLONG));
    }
    buffer.truncate(length as usize);
    Ok(buffer)
}

/// The parent directory of a confined path, opened beneath the root
/// (following symlinks in the parent's own path that stay inside it, like a
/// shell), and the final name. The root itself has none.
pub(super) struct Location {
    pub(super) dir: OwnedFd,
    pub(super) name: CString,
}
pub(super) fn locate(root: &Root, path: &Path) -> Outcome<Location> {
    let tree = root.tree(path)?;
    let rel = tree.relative(path)?;
    let Some(name) = rel.file_name() else {
        return Err(Error::new(
            Code::InvalidRequest,
            "this operation does not apply to a file root itself",
        )
        .at(path));
    };
    let parent = rel.parent().unwrap_or(Path::new(""));
    let dir = open_beneath(tree, parent, DIRECTORY, true, path)?;
    Ok(Location {
        dir,
        name: cstring(name.as_bytes()).map_err(|e| io_error(e, path))?,
    })
}
/// Open a confined path beneath its root (see [`open_beneath`]).
pub(super) fn open_path(
    root: &Root,
    path: &Path,
    flags: libc::c_int,
    follow: bool,
) -> Outcome<OwnedFd> {
    let tree = root.tree(path)?;
    open_beneath(tree, tree.relative(path)?, flags, follow, path)
}
/// Open a regular file for reading, following a final symlink within the
/// root. The type comes first, from an `O_PATH` descriptor: opening a
/// device or FIFO can have side effects, such as releasing a FIFO's writer
/// or arming a watchdog. `O_NONBLOCK` and `O_NOCTTY` limit the effect of an
/// entry swapped for one in between, which the second check refuses.
pub(super) fn open_regular(root: &Root, path: &Path, verb: &str) -> Outcome<(OwnedFd, libc::stat)> {
    let probe = open_path(root, path, libc::O_PATH, true)?;
    let st = stat_fd(probe.as_raw_fd()).map_err(|e| io_error(e, path))?;
    regular(&st, path, verb)?;
    drop(probe);
    let fd = open_path(
        root,
        path,
        libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOCTTY,
        true,
    )?;
    let st = stat_fd(fd.as_raw_fd()).map_err(|e| io_error(e, path))?;
    regular(&st, path, verb)?;
    Ok((fd, st))
}
/// Rename without replacing an existing target. A file system without
/// `RENAME_NOREPLACE` gets a check, then a plain rename.
fn rename_noreplace(from_dir: RawFd, from: &CStr, to_dir: RawFd, to: &CStr) -> io::Result<()> {
    let renamed = unsafe {
        libc::renameat2(
            from_dir,
            from.as_ptr(),
            to_dir,
            to.as_ptr(),
            libc::RENAME_NOREPLACE as _,
        )
    };
    if renamed == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if !matches!(error.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS)) {
        return Err(error);
    }
    match stat_at(to_dir, to) {
        Ok(_) => return Err(io::Error::from_raw_os_error(libc::EEXIST)),
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {}
        Err(error) => return Err(error),
    }
    check(unsafe { libc::renameat(from_dir, from.as_ptr(), to_dir, to.as_ptr()) })?;
    Ok(())
}
fn sync_dir(dir: RawFd) {
    // Durability of the directory entry; the operation already happened.
    unsafe { libc::fsync(dir) };
}

/// The bytes after classification: `binary` when they hold NUL or are not
/// UTF-8. Text goes as UTF-8 unless its JSON escaping would exceed the reply
/// budget (many control characters); binary always goes as base64.
fn content(bytes: Vec<u8>) -> (bool, Encoding, String) {
    if bytes.contains(&0) {
        return (true, Encoding::Base64, STANDARD.encode(&bytes));
    }
    match String::from_utf8(bytes) {
        Ok(text) if escaped_len(&text) <= TEXT_BUDGET => (false, Encoding::Utf8, text),
        Ok(text) => (false, Encoding::Base64, STANDARD.encode(text.as_bytes())),
        Err(error) => (true, Encoding::Base64, STANDARD.encode(error.as_bytes())),
    }
}
/// The length of `text` as a JSON string body.
fn escaped_len(text: &str) -> usize {
    text.chars()
        .map(|c| match c {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
            c if (c as u32) < 0x20 => 6,
            c => c.len_utf8(),
        })
        .sum()
}
/// Stream a file through SHA-256; `None` when it holds more than `limit`
/// bytes.
fn hash_file(file: &mut File, limit: u64) -> io::Result<Option<String>> {
    let mut hasher = super::sha256::Sha256::default();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                total += n as u64;
                if total > limit {
                    return Ok(None);
                }
                hasher.update(&buffer[..n]);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(Some(hasher.hex()))
}

fn list(root: &Root, path: &Path) -> Outcome<(Vec<Entry>, bool, usize)> {
    let dir = open_path(root, path, DIRECTORY, true)?;
    let (found, mut truncated, skipped) = select(dir.as_raw_fd()).map_err(|e| io_error(e, path))?;
    let mut entries = Vec::with_capacity(found.len());
    for name in found {
        let Ok(text) = name.to_str().map(str::to_owned) else {
            continue;
        };
        // An entry removed while listing is simply absent.
        let Ok(st) = stat_at(dir.as_raw_fd(), &name) else {
            continue;
        };
        entries.push(entry(text, &st, dir.as_raw_fd(), &name));
    }
    // Directories first, then everything else, each in byte order of the
    // name (again: an entry may have changed type since it was selected).
    entries.sort_by(|a, b| {
        (a.kind != Kind::Dir)
            .cmp(&(b.kind != Kind::Dir))
            .then_with(|| a.name.cmp(&b.name))
    });
    // Long names and targets with many escapes still fit one record.
    let mut budget = 0usize;
    for (index, item) in entries.iter().enumerate() {
        budget += serde_json::to_vec(item).map_or(usize::MAX / 2, |bytes| bytes.len() + 1);
        if budget > REPLY_BUDGET - 64 * 1024 {
            entries.truncate(index);
            truncated = true;
            break;
        }
    }
    Ok((entries, truncated, skipped))
}

fn stat(root: &Root, path: &Path) -> Outcome<Entry> {
    let name = path.file_name().map_or_else(
        || "/".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let tree = root.tree(path)?;
    if tree.relative(path)?.as_os_str().is_empty() {
        // A root itself: canonical, so never a symlink.
        let fd = tree.open()?;
        let st = stat_fd(fd.as_raw_fd()).map_err(|e| io_error(e, path))?;
        return Ok(entry(name, &st, fd.as_raw_fd(), c"."));
    }
    let location = locate(root, path)?;
    let st = stat_at(location.dir.as_raw_fd(), &location.name).map_err(|e| io_error(e, path))?;
    Ok(entry(name, &st, location.dir.as_raw_fd(), &location.name))
}

struct Content {
    size: u64,
    modified: i64,
    mode: u32,
    hash: String,
    binary: bool,
    encoding: Encoding,
    content: String,
}
fn read(root: &Root, path: &Path, max_bytes: Option<u64>) -> Outcome<Content> {
    let limit = max_bytes.unwrap_or(MAX_READ_BYTES).min(MAX_READ_BYTES);
    let (fd, st) = open_regular(root, path, "read")?;
    let too_large = |size: u64| {
        let mut error = Error::new(
            Code::TooLarge,
            format!("the file is larger than {limit} bytes"),
        )
        .at(path);
        error.size = Some(size);
        error
    };
    if st.st_size as u64 > limit {
        return Err(too_large(st.st_size as u64));
    }
    let mut bytes = Vec::with_capacity(st.st_size as usize);
    File::from(fd)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_error(e, path))?;
    if bytes.len() as u64 > limit {
        // It grew while being read.
        return Err(too_large(bytes.len() as u64));
    }
    let hash = super::sha256::hex(&bytes);
    let size = bytes.len() as u64;
    let (binary, encoding, content) = content(bytes);
    Ok(Content {
        size,
        modified: st.st_mtime,
        mode: st.st_mode & 0o7777,
        hash,
        binary,
        encoding,
        content,
    })
}

struct Written {
    hash: String,
    size: u64,
    modified: i64,
}
fn decode(content: String, encoding: Encoding) -> Outcome<Vec<u8>> {
    let too_large = || {
        Error::new(
            Code::TooLarge,
            format!("content is limited to {MAX_WRITE_BYTES} bytes"),
        )
    };
    let bytes = match encoding {
        Encoding::Utf8 => content.into_bytes(),
        Encoding::Base64 => {
            if content.len() as u64 > MAX_WRITE_BYTES.div_ceil(3) * 4 {
                return Err(too_large());
            }
            STANDARD
                .decode(content.as_bytes())
                .map_err(|_| Error::new(Code::InvalidContent, "content is not valid base64"))?
        }
    };
    if bytes.len() as u64 > MAX_WRITE_BYTES {
        return Err(too_large());
    }
    Ok(bytes)
}
fn write(
    root: &Root,
    path: &Path,
    bytes: &[u8],
    base_hash: Option<&str>,
    create: bool,
) -> Outcome<Written> {
    if create && base_hash.is_some() {
        return Err(Error::new(
            Code::InvalidRequest,
            "base_hash names existing content; it cannot be combined with create",
        ));
    }
    let fail = |e: io::Error| io_error(e, path);
    let location = locate(root, path)?;
    let dir = location.dir.as_raw_fd();
    let name = &location.name;
    let symlink = || Error::new(Code::Symlink, "refusing to write through a symlink").at(path);
    // The file being replaced, examined without opening it: never through a
    // symlink, only a regular file of this user (a replacement would change
    // its owner) that the user may write, as a shell's `>` requires.
    let existing = if create {
        None
    } else {
        let st = stat_at(dir, name).map_err(fail)?;
        if is_symlink(&st) {
            return Err(symlink());
        }
        regular(&st, path, "write")?;
        let euid = unsafe { libc::geteuid() };
        if st.st_uid != euid {
            return Err(
                Error::new(Code::PermissionDenied, "the file belongs to another user").at(path),
            );
        }
        if euid != 0 && st.st_mode & libc::S_IWUSR == 0 {
            return Err(Error::new(Code::PermissionDenied, "the file is read-only").at(path));
        }
        if let Some(base) = base_hash {
            let conflict = |current: Option<String>| {
                let mut error =
                    Error::new(Code::Conflict, "the file changed since it was read").at(path);
                error.current_hash = current;
                error
            };
            // A file larger than any read cannot match a hash read from it:
            // it is a conflict, reported without hashing it.
            let too_large = || {
                let mut error = conflict(None);
                error.message = format!(
                    "the file is now larger than {MAX_READ_BYTES} bytes, so it changed since it was read"
                );
                error.size = Some(st.st_size as u64);
                error
            };
            if st.st_size as u64 > MAX_READ_BYTES {
                return Err(too_large());
            }
            let fd = open_at(
                dir,
                name,
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY,
                0,
            )
            .map_err(|e| {
                if e.raw_os_error() == Some(libc::ELOOP) {
                    symlink()
                } else {
                    fail(e)
                }
            })?;
            let opened = stat_fd(fd.as_raw_fd()).map_err(fail)?;
            if opened.st_dev != st.st_dev || opened.st_ino != st.st_ino {
                return Err(conflict(None));
            }
            match hash_file(&mut File::from(fd), MAX_READ_BYTES).map_err(fail)? {
                None => return Err(too_large()),
                Some(current) if !current.eq_ignore_ascii_case(base) => {
                    return Err(conflict(Some(current)));
                }
                Some(_) => {}
            }
        }
        Some(st)
    };
    let temporary = cstring(format!(".kelpserver-{}.tmp", random_hex().map_err(fail)?).as_bytes())
        .map_err(fail)?;
    // A new file gets 0666 less the umask, like a shell's; a replacement
    // starts private and takes the existing permission bits (without set-id).
    let fd = open_at(
        dir,
        &temporary,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
        if create { 0o666 } else { 0o600 },
    )
    .map_err(fail)?;
    let result = (|| -> Outcome<Written> {
        let mut file = File::from(fd);
        file.write_all(bytes).map_err(fail)?;
        if let Some(st) = &existing {
            // The group first: changing it may clear set-id bits, and the
            // mode below is set afterwards. A group the user is not a member
            // of cannot be kept (EPERM); the file then gets the default one.
            // Extended attributes and ACLs are not carried over.
            let own = stat_fd(file.as_raw_fd()).map_err(fail)?;
            if own.st_gid != st.st_gid
                && unsafe { libc::fchown(file.as_raw_fd(), libc::uid_t::MAX, st.st_gid) } != 0
            {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::EPERM) {
                    return Err(fail(error));
                }
            }
            check(unsafe { libc::fchmod(file.as_raw_fd(), st.st_mode & 0o777) }).map_err(fail)?;
        }
        file.sync_all().map_err(fail)?;
        let written = stat_fd(file.as_raw_fd()).map_err(fail)?;
        match &existing {
            None => rename_noreplace(dir, &temporary, dir, name).map_err(fail)?,
            Some(st) => {
                // Refuse if the name now points elsewhere (replaced
                // meanwhile) or, for a compare-and-swap, if the file changed
                // in place since it was hashed. What lands between this
                // check and the rename (microseconds) is still overwritten.
                let now = stat_at(dir, name).map_err(fail)?;
                let changed = if base_hash.is_some() {
                    !same_version(&now, st)
                } else {
                    now.st_dev != st.st_dev || now.st_ino != st.st_ino
                };
                if changed {
                    return Err(Error::new(
                        Code::Conflict,
                        "the file changed while it was being saved",
                    )
                    .at(path));
                }
                check(unsafe { libc::renameat(dir, temporary.as_ptr(), dir, name.as_ptr()) })
                    .map_err(fail)?;
            }
        }
        Ok(Written {
            hash: super::sha256::hex(bytes),
            size: bytes.len() as u64,
            modified: written.st_mtime,
        })
    })();
    if result.is_err() {
        unsafe { libc::unlinkat(dir, temporary.as_ptr(), 0) };
    } else {
        sync_dir(dir);
    }
    result
}

fn mkdir(root: &Root, path: &Path) -> Outcome<()> {
    let location = locate(root, path)?;
    check(unsafe { libc::mkdirat(location.dir.as_raw_fd(), location.name.as_ptr(), 0o777) })
        .map_err(|e| io_error(e, path))?;
    sync_dir(location.dir.as_raw_fd());
    Ok(())
}

fn rename(root: &Root, from: &Path, to: &Path, overwrite: bool) -> Outcome<()> {
    // Each end is resolved beneath its own root; nothing moves between roots
    // (often different file systems, and never copied here).
    if root.tree(from)?.path() != root.tree(to)?.path() {
        return Err(Error::new(Code::CrossRoot, "cannot move an entry between file roots").at(to));
    }
    let source = locate(root, from)?;
    let target = locate(root, to)?;
    // The source must exist (a symlink is moved as a link), and be neither
    // `/`, the home directory nor a root under another name (a bind
    // mount); nor may an overwritten target be one.
    let st = stat_at(source.dir.as_raw_fd(), &source.name).map_err(|e| io_error(e, from))?;
    protect(&st, root, from)?;
    let replaced = overwrite
        .then(|| stat_at(target.dir.as_raw_fd(), &target.name).ok())
        .flatten();
    if let Some(st) = replaced {
        protect(&st, root, to)?;
    }
    let result = if overwrite {
        check(unsafe {
            libc::renameat(
                source.dir.as_raw_fd(),
                source.name.as_ptr(),
                target.dir.as_raw_fd(),
                target.name.as_ptr(),
            )
        })
        .map(drop)
    } else {
        rename_noreplace(
            source.dir.as_raw_fd(),
            &source.name,
            target.dir.as_raw_fd(),
            &target.name,
        )
    };
    result.map_err(|error| {
        let path = if error.raw_os_error() == Some(libc::EEXIST) {
            to
        } else {
            from
        };
        io_error(error, path)
    })?;
    sync_dir(target.dir.as_raw_fd());
    sync_dir(source.dir.as_raw_fd());
    Ok(())
}

/// Remove a directory's contents without following symlinks or leaving the
/// file system `dev`, in bounded batches of names.
fn clear(dir: RawFd, depth: usize, dev: libc::dev_t, path: &Path) -> Outcome<()> {
    if depth >= MAX_DEPTH {
        return Err(Error::new(
            Code::TooLarge,
            format!("directories are nested deeper than {MAX_DEPTH} levels"),
        )
        .at(path));
    }
    loop {
        let (batch, _) = names(dir, 4096).map_err(|e| io_error(e, path))?;
        if batch.is_empty() {
            return Ok(());
        }
        let mut removed = 0;
        for name in batch {
            let child = path.join(OsStr::from_bytes(name.to_bytes()));
            let st = match stat_at(dir, &name) {
                Ok(st) => st,
                Err(error) if error.raw_os_error() == Some(libc::ENOENT) => continue,
                Err(error) => return Err(io_error(error, &child)),
            };
            if remove_entry(dir, &name, &st, depth, dev, &child)? {
                removed += 1;
            }
        }
        // A file system that keeps listing names it reports as missing
        // would otherwise loop forever.
        if removed == 0 {
            return Err(Error::new(
                Code::Io,
                "the directory keeps listing entries that cannot be removed",
            )
            .at(path));
        }
    }
}
/// Remove one entry (a directory with its tree); `false` when it was
/// already gone.
fn remove_entry(
    dir: RawFd,
    name: &CStr,
    st: &libc::stat,
    depth: usize,
    dev: libc::dev_t,
    path: &Path,
) -> Outcome<bool> {
    let removed = if is_dir(st) {
        // Like `rm --one-file-system`: a mount point inside the tree stops
        // the deletion instead of emptying another file system.
        if st.st_dev != dev {
            return Err(Error::new(
                Code::CrossDevice,
                "refusing to delete across a mount point into another file system",
            )
            .at(path));
        }
        let child =
            open_at(dir, name, DIRECTORY | libc::O_NOFOLLOW, 0).map_err(|e| io_error(e, path))?;
        clear(child.as_raw_fd(), depth + 1, dev, path)?;
        drop(child);
        unsafe { libc::unlinkat(dir, name.as_ptr(), libc::AT_REMOVEDIR) }
    } else {
        unsafe { libc::unlinkat(dir, name.as_ptr(), 0) }
    };
    match check(removed) {
        Ok(_) => Ok(true),
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => Ok(false),
        Err(error) => Err(io_error(error, path)),
    }
}
/// Refuse to remove `/`, the server user's home directory or a file root
/// itself, under whatever path names it (a bind mount, say; `st` is the
/// entry's `lstat`, so a symlink to one is not protected: removing it
/// removes only the link). The roots are matched by the device and inode
/// recorded at startup, not by looking their paths up again.
fn protect(st: &libc::stat, root: &Root, path: &Path) -> Outcome<()> {
    let refuse = || {
        Err(Error::new(
            Code::InvalidRequest,
            "refusing to remove or move /, the home directory or a file root",
        )
        .at(path))
    };
    if root.trees.iter().any(|tree| tree.is(st)) {
        return refuse();
    }
    for protected in [Path::new("/")].into_iter().chain(root.home()) {
        let Ok(name) = cstring(protected.as_os_str().as_bytes()) else {
            continue;
        };
        let mut other: libc::stat = unsafe { std::mem::zeroed() };
        // Following symlinks: a symlinked HOME protects its directory.
        if unsafe { libc::fstatat(libc::AT_FDCWD, name.as_ptr(), &mut other, 0) } == 0
            && (other.st_dev, other.st_ino) == (st.st_dev, st.st_ino)
        {
            return refuse();
        }
    }
    Ok(())
}
fn delete(root: &Root, path: &Path) -> Outcome<()> {
    let location = locate(root, path)?;
    let dir = location.dir.as_raw_fd();
    let st = stat_at(dir, &location.name).map_err(|e| io_error(e, path))?;
    protect(&st, root, path)?;
    remove_entry(dir, &location.name, &st, 0, st.st_dev, path)?;
    sync_dir(dir);
    Ok(())
}

/// The freedesktop home trash: `$XDG_DATA_HOME/Trash`, or
/// `~/.local/share/Trash` when `XDG_DATA_HOME` is unset or not absolute.
pub fn default_trash() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home().map(|home| home.join(".local/share")))
        .map(|data| data.join("Trash"))
}
/// Percent-encode an absolute path for a `.trashinfo` `Path=` key, as in a
/// file URL: every byte except unreserved characters and `/`.
pub fn encode_trash_path(path: &[u8]) -> String {
    let mut encoded = String::with_capacity(path.len());
    for &byte in path {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}
/// The local time as `YYYY-MM-DDThh:mm:ss`, the `DeletionDate` format.
fn deletion_date(time: libc::time_t) -> String {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        // Without a local time zone, UTC is the most useful fallback.
        unsafe { libc::gmtime_r(&time, &mut tm) };
    }
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}
/// The `.trashinfo` contents for an absolute path.
pub fn trash_info(path: &Path, deletion_date: &str) -> String {
    format!(
        "[Trash Info]\nPath={}\nDeletionDate={deletion_date}\n",
        encode_trash_path(path.as_os_str().as_bytes())
    )
}
/// The trash name of the `n`th candidate (1 is the name itself; then
/// `stem.2.ext`, `stem.3.ext`, …), short enough for its `.trashinfo` name.
pub fn trash_name(name: &str, n: usize) -> String {
    let max = NAME_MAX - TRASHINFO.len();
    if n == 1 && name.len() <= max {
        return name.to_owned();
    }
    let (stem, extension) = match name.rfind('.') {
        Some(index) if index > 0 && name.len() - index <= 32 => name.split_at(index),
        _ => (name, ""),
    };
    let suffix = format!(".{n}{extension}");
    let mut keep = stem.len().min(max.saturating_sub(suffix.len()));
    while !stem.is_char_boundary(keep) {
        keep -= 1;
    }
    format!("{}{suffix}", &stem[..keep])
}
struct Trash {
    root: PathBuf,
    files: OwnedFd,
    info: OwnedFd,
    dev: libc::dev_t,
    /// The canonical path of `files/`, when it can be learnt: it decides
    /// which root's entries may be trashed ([`trash_allowed`]).
    place: Option<PathBuf>,
}
fn private_dir_at(parent: RawFd, name: &CStr, path: &Path) -> Outcome<OwnedFd> {
    if unsafe { libc::mkdirat(parent, name.as_ptr(), 0o700) } != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EEXIST) {
            return Err(io_error(error, path));
        }
    }
    let fd =
        open_at(parent, name, DIRECTORY | libc::O_NOFOLLOW, 0).map_err(|e| io_error(e, path))?;
    let st = stat_fd(fd.as_raw_fd()).map_err(|e| io_error(e, path))?;
    if st.st_uid != unsafe { libc::geteuid() } {
        return Err(Error::new(
            Code::PermissionDenied,
            "the trash directory belongs to another user",
        )
        .at(path));
    }
    Ok(fd)
}
fn open_trash(settings: &Settings) -> Outcome<Trash> {
    let root = settings
        .trash_dir
        .clone()
        .or_else(default_trash)
        .ok_or_else(|| {
            Error::new(
                Code::InvalidPath,
                "neither XDG_DATA_HOME nor HOME is an absolute path",
            )
        })?;
    let parent = root.parent().unwrap_or(Path::new("/"));
    std::fs::create_dir_all(parent).map_err(|e| io_error(e, parent))?;
    let parent_c = cstring(parent.as_os_str().as_bytes()).map_err(|e| io_error(e, parent))?;
    let parent_fd =
        open_at(libc::AT_FDCWD, &parent_c, DIRECTORY, 0).map_err(|e| io_error(e, parent))?;
    let name =
        cstring(root.file_name().unwrap_or_default().as_bytes()).map_err(|e| io_error(e, &root))?;
    let trash = private_dir_at(parent_fd.as_raw_fd(), &name, &root)?;
    let files = private_dir_at(trash.as_raw_fd(), c"files", &root.join("files"))?;
    let info = private_dir_at(trash.as_raw_fd(), c"info", &root.join("info"))?;
    let dev = stat_fd(files.as_raw_fd())
        .map_err(|e| io_error(e, &root))?
        .st_dev;
    // Where the opened `files/` directory really is, symlinks resolved.
    let place = std::fs::read_link(format!("/proc/self/fd/{}", files.as_raw_fd()))
        .ok()
        .filter(|path| path.is_absolute())
        .or_else(|| std::fs::canonicalize(root.join("files")).ok());
    Ok(Trash {
        root,
        files,
        info,
        dev,
        place,
    })
}
/// Refuse to trash `path` when that would move it into another file root:
/// an entry may go to a trash inside its own root, and an entry of the
/// primary root also to a trash outside every root (server configuration,
/// as before extra roots). A trash whose place is unknown takes nothing.
fn trash_allowed(trash: &Trash, root: &Root, path: &Path) -> Outcome<()> {
    let tree = root.tree(path)?;
    let allowed = trash.place.as_deref().is_some_and(|place| {
        match root
            .trees
            .iter()
            .find(|other| place.starts_with(&other.path))
        {
            Some(holder) => holder.path == tree.path,
            None => tree.path == root.trees[0].path,
        }
    });
    if allowed {
        return Ok(());
    }
    Err(Error::new(
        Code::CrossRoot,
        "the trash is outside this file root; delete the entry instead",
    )
    .at(path))
}
/// Move one entry to the trash: reserve a unique `info/<name>.trashinfo`
/// with `O_EXCL`, write and sync it, then rename the entry to
/// `files/<name>` without replacing anything. A failed move removes the
/// info file again.
fn trash_one(trash: &Trash, root: &Root, path: &Path) -> Outcome<PathBuf> {
    let location = locate(root, path)?;
    let dir = location.dir.as_raw_fd();
    let entry = stat_at(dir, &location.name).map_err(|e| io_error(e, path))?;
    protect(&entry, root, path)?;
    trash_allowed(trash, root, path)?;
    let st = stat_fd(dir).map_err(|e| io_error(e, path))?;
    if st.st_dev != trash.dev {
        return Err(Error::new(
            Code::CrossDevice,
            "the trash is on another file system; delete the entry instead",
        )
        .at(path));
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let info = trash_info(path, &deletion_date(crate::now() as libc::time_t));
    for n in 1..=1000 {
        let candidate = trash_name(&name, n);
        let info_name =
            cstring(format!("{candidate}{TRASHINFO}").as_bytes()).map_err(|e| io_error(e, path))?;
        let file = match open_at(
            trash.info.as_raw_fd(),
            &info_name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            0o600,
        ) {
            Ok(fd) => File::from(fd),
            Err(error) if error.raw_os_error() == Some(libc::EEXIST) => continue,
            Err(error) => return Err(io_error(error, &trash.root.join("info"))),
        };
        let unreserve = || unsafe {
            libc::unlinkat(trash.info.as_raw_fd(), info_name.as_ptr(), 0);
        };
        let mut file = file;
        if let Err(error) = file
            .write_all(info.as_bytes())
            .and_then(|()| file.sync_all())
        {
            unreserve();
            return Err(io_error(error, &trash.root.join("info")));
        }
        drop(file);
        // The info entry is durable before the payload moves.
        sync_dir(trash.info.as_raw_fd());
        let target = cstring(candidate.as_bytes()).map_err(|e| io_error(e, path))?;
        match rename_noreplace(dir, &location.name, trash.files.as_raw_fd(), &target) {
            Ok(()) => {
                sync_dir(trash.files.as_raw_fd());
                sync_dir(dir);
                return Ok(trash.root.join("files").join(candidate));
            }
            // An orphaned payload holds the name; try the next one.
            Err(error) if error.raw_os_error() == Some(libc::EEXIST) => unreserve(),
            Err(error) => {
                unreserve();
                return Err(io_error(error, path));
            }
        }
    }
    Err(Error::new(Code::Exists, "no free name in the trash for this entry").at(path))
}

/// The `file_error` reply for a failed request.
pub fn failure(request: String, error: Error) -> Event {
    Event::FileError {
        request,
        error,
        download: None,
    }
}
fn removal(
    request: String,
    paths: &[String],
    root: &Root,
    mut remove: impl FnMut(&Path) -> Outcome<Option<PathBuf>>,
) -> Event {
    if paths.is_empty() || paths.len() > MAX_PATHS {
        return failure(
            request,
            Error::new(Code::InvalidRequest, format!("name 1 to {MAX_PATHS} paths")),
        );
    }
    // Every path is checked before anything is removed: one outside the
    // root refuses the whole request.
    let mut normalized = Vec::with_capacity(paths.len());
    for path in paths {
        match root.request_path(path) {
            Ok(path) => normalized.push(path),
            Err(mut error) => {
                error.path = Some(path.clone());
                return failure(request, error);
            }
        }
    }
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    for path in normalized {
        match remove(&path) {
            Ok(trashed) => removed.push(Removed {
                path: path_text(&path),
                trashed_as: trashed.as_deref().map(path_text),
            }),
            Err(error) => failed.push(Failure {
                path: path_text(&path),
                error,
            }),
        }
    }
    Event::FileRemoved {
        request,
        removed,
        failed,
    }
}
/// Run one file request (blocking; called on a worker thread). Every outcome
/// is a reply event for the requester whose serialized form fits one record.
/// `root` is the server's file root; `None` (it could not be resolved)
/// refuses every operation with `outside_root`.
pub fn execute(request: Request, settings: &Settings, root: Option<&Root>) -> Event {
    let correlation = request.correlation().unwrap_or_default().to_owned();
    let Some(root) = root else {
        return failure(correlation, no_root());
    };
    let event = match run(request, settings, root) {
        Ok(event) => event,
        Err(error) => failure(correlation.clone(), error),
    };
    let size = serde_json::to_vec(&event).map_or(usize::MAX, |bytes| bytes.len());
    if size > REPLY_BUDGET {
        return failure(
            correlation,
            Error::new(Code::TooLarge, "the reply would exceed the record size"),
        );
    }
    event
}
/// The refusal of a server whose file root could not be resolved.
pub fn no_root() -> Error {
    Error::new(
        Code::OutsideRoot,
        "the server has no file root (it could not be resolved at startup)",
    )
}
fn run(request: Request, settings: &Settings, root: &Root) -> Outcome<Event> {
    fn at(path: &str) -> impl FnOnce(Error) -> Error + '_ {
        move |mut error| {
            error.path = Some(path.to_owned());
            error
        }
    }
    Ok(match request {
        Request::FileList { request, path } => {
            let path = root.request_path(&path).map_err(at(&path))?;
            let (entries, truncated, skipped) = list(root, &path)?;
            Event::FileList {
                request,
                path: path_text(&path),
                entries,
                truncated,
                skipped,
            }
        }
        Request::FileStat { request, path } => {
            let path = root.request_path(&path).map_err(at(&path))?;
            let entry = stat(root, &path)?;
            Event::FileStat {
                request,
                path: path_text(&path),
                entry,
            }
        }
        Request::FileRead {
            request,
            path,
            max_bytes,
        } => {
            let path = root.request_path(&path).map_err(at(&path))?;
            let content = read(root, &path, max_bytes)?;
            Event::FileContent {
                request,
                path: path_text(&path),
                size: content.size,
                modified: content.modified,
                mode: content.mode,
                hash: content.hash,
                binary: content.binary,
                encoding: content.encoding,
                content: content.content,
            }
        }
        Request::FileWrite {
            request,
            path,
            content,
            encoding,
            base_hash,
            create,
        } => {
            let path = root.request_path(&path).map_err(at(&path))?;
            let bytes = decode(content, encoding).map_err(|e| e.at(&path))?;
            let written = write(root, &path, &bytes, base_hash.as_deref(), create)?;
            Event::FileWritten {
                request,
                path: path_text(&path),
                hash: written.hash,
                size: written.size,
                modified: written.modified,
                created: create,
            }
        }
        Request::FileMkdir { request, path } => {
            let path = root.request_path(&path).map_err(at(&path))?;
            mkdir(root, &path)?;
            Event::Done { request }
        }
        Request::FileRename {
            request,
            from,
            to,
            overwrite,
        } => {
            let from = root.request_path(&from).map_err(at(&from))?;
            let to = root.request_path(&to).map_err(at(&to))?;
            rename(root, &from, &to, overwrite)?;
            Event::Done { request }
        }
        Request::FileTrash { request, paths } => {
            let trash = match open_trash(settings) {
                Ok(trash) => trash,
                Err(error) => return Ok(failure(request, error)),
            };
            removal(request, &paths, root, |path| {
                trash_one(&trash, root, path).map(Some)
            })
        }
        Request::FileDelete { request, paths } => removal(request, &paths, root, |path| {
            delete(root, path).map(|()| None)
        }),
        _ => {
            return Err(Error::new(Code::InvalidRequest, "not a file operation"));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_are_normalized() {
        let home = Path::new("/home/me");
        let ok = |path: &str| normalize(path, Some(home)).unwrap();
        assert_eq!(ok("/"), Path::new("/"));
        assert_eq!(ok("//"), Path::new("/"));
        assert_eq!(ok("/a//b/./c/"), Path::new("/a/b/c"));
        assert_eq!(ok("~"), Path::new("/home/me"));
        assert_eq!(ok("~/"), Path::new("/home/me"));
        assert_eq!(ok("~/notes/./a.txt"), Path::new("/home/me/notes/a.txt"));
        assert_eq!(
            normalize("~/x", Some(Path::new("/home//me/"))).unwrap(),
            Path::new("/home/me/x")
        );
        let code = |path: &str, home: Option<&Path>| normalize(path, home).unwrap_err().code;
        for bad in [
            "", "a/b", "./a", "/a/../b", "/..", "~other/x", "~x", "/a\0b",
        ] {
            assert_eq!(code(bad, Some(home)), Code::InvalidPath, "{bad:?}");
        }
        assert_eq!(code("~/x", None), Code::InvalidPath);
        assert_eq!(
            code(&format!("/{}", "a".repeat(4096)), None),
            Code::InvalidPath
        );
        assert!(normalize(&format!("/{}", "a".repeat(4095)), None).is_ok());
    }
    #[test]
    fn trash_info_is_url_encoded() {
        assert_eq!(
            encode_trash_path("/home/me/a b/é%#?.txt".as_bytes()),
            "/home/me/a%20b/%C3%A9%25%23%3F.txt"
        );
        assert_eq!(encode_trash_path(b"/x/A-z_0.9~"), "/x/A-z_0.9~");
        assert_eq!(
            trash_info(Path::new("/tmp/my file"), "2026-11-23T10:04:05"),
            "[Trash Info]\nPath=/tmp/my%20file\nDeletionDate=2026-11-23T10:04:05\n"
        );
        let date = deletion_date(1_795_300_000);
        assert_eq!(date.len(), 19);
        assert_eq!(&date[4..5], "-");
        assert_eq!(&date[10..11], "T");
        assert_eq!(&date[13..14], ":");
    }
    #[test]
    fn trash_names_are_numbered_and_bounded() {
        assert_eq!(trash_name("a.txt", 1), "a.txt");
        assert_eq!(trash_name("a.txt", 2), "a.2.txt");
        assert_eq!(trash_name("archive", 3), "archive.3");
        assert_eq!(trash_name(".bashrc", 2), ".bashrc.2");
        let long = format!("{}.jpeg", "é".repeat(127));
        assert!(long.len() > NAME_MAX - TRASHINFO.len());
        for n in [1, 2, 1000] {
            let name = trash_name(&long, n);
            assert!(name.len() <= NAME_MAX - TRASHINFO.len(), "{n}");
            assert!(name.ends_with(&format!(".{n}.jpeg")));
        }
    }
    #[test]
    fn content_is_classified() {
        assert_eq!(
            content(b"hello\n".to_vec()),
            (false, Encoding::Utf8, "hello\n".into())
        );
        assert_eq!(
            content(vec![0, 1, 2]),
            (true, Encoding::Base64, "AAEC".into())
        );
        assert_eq!(
            content(vec![0xff, 0xfe]),
            (true, Encoding::Base64, "//4=".into())
        );
        assert_eq!(escaped_len("a\"\n\u{1}é"), 1 + 2 + 2 + 6 + 2);
        // Text whose escaping would not fit one record goes as base64.
        let controls = "\u{1}".repeat(TEXT_BUDGET / 6 + 1);
        let (binary, encoding, _) = content(controls.into_bytes());
        assert_eq!((binary, encoding), (false, Encoding::Base64));
    }
    /// A fresh folder for a test, canonical, and a root over it.
    fn scratch(test: &str) -> (PathBuf, Root) {
        let path = std::env::temp_dir().join(format!(
            "kelpserver-files-unit-{}-{test}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        let path = std::fs::canonicalize(&path).unwrap();
        let root = Root::new(Some(&path)).unwrap();
        assert_eq!(root.path(), path);
        (path, root)
    }
    #[test]
    fn the_file_root_is_never_removed() {
        let (base, root) = scratch("protect");
        let trash_dir = base.with_extension("trash");
        std::fs::create_dir_all(base.join("notes")).unwrap();
        std::fs::write(base.join("notes/a.txt"), "a").unwrap();
        std::os::unix::fs::symlink(&base, base.join("link")).unwrap();
        let error = delete(&root, &base).unwrap_err();
        assert_eq!(error.code, Code::InvalidRequest);
        let trash = open_trash(&Settings {
            trash_dir: Some(trash_dir.clone()),
            ..Default::default()
        })
        .unwrap();
        let error = trash_one(&trash, &root, &base).unwrap_err();
        assert_eq!(error.code, Code::InvalidRequest);
        assert!(base.join("notes/a.txt").exists());
        // A symlink to the root is only a link.
        delete(&root, &base.join("link")).unwrap();
        assert!(base.join("notes/a.txt").exists());
        // Below it, everything can be removed.
        delete(&root, &base.join("notes")).unwrap();
        assert!(!base.join("notes").exists());
        // `/` is outside.
        assert_eq!(
            delete(&root, Path::new("/")).unwrap_err().code,
            Code::OutsideRoot
        );
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&trash_dir).unwrap();
    }
    #[test]
    fn request_paths_are_confined_lexically() {
        let (base, root) = scratch("confine");
        let ok = |path: &str| root.request_path(path).unwrap();
        let text = base.to_str().unwrap();
        assert_eq!(ok(text), base);
        assert_eq!(ok(&format!("{text}//a/./b/")), base.join("a/b"));
        for bad in [
            "/",
            "/etc/passwd",
            &format!("{text}x"),
            &format!("{text}x/a"),
        ] {
            assert_eq!(
                root.request_path(bad).unwrap_err().code,
                Code::OutsideRoot,
                "{bad:?}"
            );
        }
        // `..` stays an invalid path, before any confinement.
        assert_eq!(
            root.request_path(&format!("{text}/a/../.."))
                .unwrap_err()
                .code,
            Code::InvalidPath
        );
        // A root reached through a symlink: its spelling is an alias,
        // answered in the canonical spelling.
        let alias = base.with_extension("alias");
        let _ = std::fs::remove_file(&alias);
        std::os::unix::fs::symlink(&base, &alias).unwrap();
        let aliased = Root::new(Some(&alias)).unwrap();
        assert_eq!(aliased.path(), base);
        assert_eq!(aliased.confine(&alias.join("x")).unwrap(), base.join("x"));
        assert_eq!(aliased.confine(&alias).unwrap(), base);
        assert_eq!(aliased.confine(&base.join("y")).unwrap(), base.join("y"));
        std::fs::remove_file(&alias).unwrap();
        // A root must be an existing absolute directory.
        assert!(Root::new(Some(Path::new("relative"))).is_err());
        assert!(Root::new(Some(&base.join("missing"))).is_err());
        std::fs::write(base.join("file"), "").unwrap();
        assert!(Root::new(Some(&base.join("file"))).is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }
    #[test]
    fn extra_roots_are_resolved_apart_from_each_other() {
        use std::os::unix::fs::symlink;
        let (base, _) = scratch("extras");
        for dir in ["primary/in", "e/inner", "x/data", "y/data", "z"] {
            std::fs::create_dir_all(base.join(dir)).unwrap();
        }
        std::fs::write(base.join("f"), "").unwrap();
        // An extra root configured through a symlink inside the primary root.
        symlink(base.join("z"), base.join("primary/z-link")).unwrap();
        let primary = base.join("primary");
        let extras = [
            "e",
            "e/inner",
            "missing",
            "f",
            "primary/in",
            "",
            "e",
            "x/data",
            "y/data",
            "primary/z-link",
        ]
        .map(|rel| {
            if rel.is_empty() {
                base.clone()
            } else {
                base.join(rel)
            }
        });
        let mut configured = extras.to_vec();
        configured.push(PathBuf::from("relative"));
        let (root, warnings) = Root::with_extras(Some(&primary), &configured).unwrap();
        let paths: Vec<_> = root.paths().map(Path::to_path_buf).collect();
        assert_eq!(
            paths,
            [
                primary.clone(),
                base.join("e"),
                base.join("x/data"),
                base.join("y/data"),
                base.join("z"),
            ]
        );
        // One warning per skipped root, naming it.
        assert_eq!(warnings.len(), 7, "{warnings:?}");
        for (skipped, reason) in [
            (base.join("e/inner"), "inside another extra file root"),
            (base.join("e"), "repeats another extra file root"),
            (base.join("missing"), "No such file"),
            (base.join("f"), "not a directory"),
            (base.join("primary/in"), "inside the file root"),
            (base.clone(), "contains the file root"),
            (PathBuf::from("relative"), "not an absolute path"),
        ] {
            let name = skipped.display().to_string();
            assert!(
                warnings
                    .iter()
                    .any(|w| w.contains(&format!("root {name}")) && w.contains(reason)),
                "{name}: {warnings:?}"
            );
        }
        assert!(warnings.iter().all(|w| w.contains("skipped")));
        // The repeat of `e` is dropped too, without a separate root.
        assert_eq!(paths.iter().filter(|p| **p == base.join("e")).count(), 1);
        // Labels: the folder's name, the full path when two share it.
        let labels: Vec<_> = root.named().into_iter().map(|n| n.label).collect();
        let full = |rel: &str| base.join(rel).display().to_string();
        assert_eq!(
            labels,
            [
                "primary".into(),
                "e".into(),
                full("x/data"),
                full("y/data"),
                "z".into()
            ]
        );
        // Each path is confined to the root it lies in; the longest prefix
        // wins, so the symlinked spelling names the extra root `z`.
        assert_eq!(
            root.confine(&base.join("e/inner/a")).unwrap(),
            base.join("e/inner/a")
        );
        assert_eq!(
            root.confine(&base.join("primary/z-link/a")).unwrap(),
            base.join("z/a")
        );
        assert_eq!(root.tree(&base.join("z/a")).unwrap().path(), base.join("z"));
        for outside in [base.join("f"), base.join("x"), base.clone()] {
            assert_eq!(
                root.confine(&outside).unwrap_err().code,
                Code::OutsideRoot,
                "{outside:?}"
            );
        }
        // Too many extra roots: the ones beyond the bound are skipped.
        let many = vec![base.join("e"); MAX_EXTRA_ROOTS + 2];
        let (root, warnings) = Root::with_extras(Some(&primary), &many).unwrap();
        assert_eq!(root.paths().count(), 2);
        assert_eq!(
            warnings
                .iter()
                .filter(|w| w.contains("too many extra file roots"))
                .count(),
            2
        );
        // No extra root: as `Root::new`.
        assert_eq!(
            Root::with_extras(Some(&primary), &[]).unwrap().0,
            Root::new(Some(&primary)).unwrap()
        );
        std::fs::remove_dir_all(&base).unwrap();
    }
    #[test]
    fn every_root_is_protected_and_renames_stay_inside_one() {
        let (base, _) = scratch("protect-extras");
        for dir in ["primary/d", "extra/d"] {
            std::fs::create_dir_all(base.join(dir)).unwrap();
        }
        std::fs::write(base.join("primary/a"), "a").unwrap();
        let (primary, extra) = (base.join("primary"), base.join("extra"));
        let (root, _) = Root::with_extras(Some(&primary), std::slice::from_ref(&extra)).unwrap();
        assert_eq!(
            delete(&root, &extra).unwrap_err().code,
            Code::InvalidRequest
        );
        let error = rename(&root, &primary.join("a"), &extra.join("a"), false).unwrap_err();
        assert_eq!(error.code, Code::CrossRoot);
        assert_eq!(
            rename(&root, &extra.join("d"), &primary.join("e"), true)
                .unwrap_err()
                .code,
            Code::CrossRoot
        );
        rename(&root, &extra.join("d"), &extra.join("e"), false).unwrap();
        assert!(primary.join("a").exists() && extra.join("e").is_dir());
        std::fs::remove_dir_all(&base).unwrap();
    }
    #[test]
    fn the_trash_never_moves_an_entry_into_another_root() {
        let (base, _) = scratch("trash-extras");
        for dir in ["primary/d", "extra/d", "elsewhere"] {
            std::fs::create_dir_all(base.join(dir)).unwrap();
        }
        let (primary, extra) = (base.join("primary"), base.join("extra"));
        let (root, _) = Root::with_extras(Some(&primary), std::slice::from_ref(&extra)).unwrap();
        let trash_in = |dir: PathBuf| {
            open_trash(&Settings {
                trash_dir: Some(dir),
                ..Default::default()
            })
            .unwrap()
        };
        let reset = || {
            std::fs::write(primary.join("d/a"), "a").unwrap();
            std::fs::write(extra.join("d/b"), "b").unwrap();
        };
        // A trash inside the primary root, on the extra root's file system:
        // an entry of the extra root stays where it is.
        reset();
        let trash = trash_in(primary.join(".local/share/Trash"));
        let error = trash_one(&trash, &root, &extra.join("d/b")).unwrap_err();
        assert_eq!(error.code, Code::CrossRoot);
        assert!(extra.join("d/b").exists());
        trash_one(&trash, &root, &primary.join("d/a")).unwrap();
        // A trash inside the extra root takes only the extra root's entries.
        reset();
        let trash = trash_in(extra.join(".Trash"));
        trash_one(&trash, &root, &extra.join("d/b")).unwrap();
        let error = trash_one(&trash, &root, &primary.join("d/a")).unwrap_err();
        assert_eq!(error.code, Code::CrossRoot);
        assert!(primary.join("d/a").exists());
        // A trash outside every root (server configuration) takes the
        // primary root's entries, as before extra roots.
        reset();
        let trash = trash_in(base.join("elsewhere/Trash"));
        trash_one(&trash, &root, &primary.join("d/a")).unwrap();
        let error = trash_one(&trash, &root, &extra.join("d/b")).unwrap_err();
        assert_eq!(error.code, Code::CrossRoot);
        assert!(extra.join("d/b").exists());
        std::fs::remove_dir_all(&base).unwrap();
    }
    #[test]
    fn a_root_replaced_after_startup_is_refused() {
        use std::os::unix::fs::symlink;
        let (base, _) = scratch("swapped");
        for dir in ["primary", "data/extra", "secret"] {
            std::fs::create_dir_all(base.join(dir)).unwrap();
        }
        std::fs::write(base.join("data/extra/f.txt"), "extra").unwrap();
        std::fs::write(base.join("secret/key"), "secret").unwrap();
        let (primary, extra) = (base.join("primary"), base.join("data/extra"));
        let (root, _) = Root::with_extras(Some(&primary), std::slice::from_ref(&extra)).unwrap();
        assert_eq!(list(&root, &extra).unwrap().0.len(), 1);
        // Someone able to change the extra root's parent swaps the root for
        // a symlink to another folder; the server refuses that root.
        std::fs::rename(&extra, base.join("data/moved")).unwrap();
        symlink(base.join("secret"), &extra).unwrap();
        assert_eq!(list(&root, &extra).unwrap_err().code, Code::OutsideRoot);
        assert_eq!(
            read(&root, &extra.join("key"), None).err().unwrap().code,
            Code::OutsideRoot
        );
        assert_eq!(stat(&root, &extra).unwrap_err().code, Code::OutsideRoot);
        assert_eq!(
            write(&root, &extra.join("new"), b"x", None, true)
                .err()
                .unwrap()
                .code,
            Code::OutsideRoot
        );
        assert_eq!(std::fs::read_dir(base.join("secret")).unwrap().count(), 1);
        // A parent swapped for a symlink to a copy is refused the same way.
        std::fs::remove_file(&extra).unwrap();
        std::fs::rename(base.join("data"), base.join("data-old")).unwrap();
        std::fs::create_dir_all(base.join("copy/extra")).unwrap();
        symlink(base.join("copy"), base.join("data")).unwrap();
        assert_eq!(list(&root, &extra).unwrap_err().code, Code::OutsideRoot);
        // The primary root is held to its recorded identity too.
        std::fs::rename(&primary, base.join("primary-old")).unwrap();
        std::fs::create_dir(&primary).unwrap();
        assert_eq!(list(&root, &primary).unwrap_err().code, Code::OutsideRoot);
        std::fs::remove_dir_all(&base).unwrap();
    }
    #[test]
    fn the_walk_resolves_symlinks_beneath_the_root() {
        use std::os::unix::fs::symlink;
        let (base, root) = scratch("walk");
        let tree = root.primary();
        std::fs::create_dir_all(base.join("a/b")).unwrap();
        std::fs::write(base.join("a/b/f.txt"), "inside").unwrap();
        symlink("a/b", base.join("rel")).unwrap();
        symlink(base.join("a"), base.join("abs")).unwrap();
        symlink("../b/f.txt", base.join("a/b/up")).unwrap();
        symlink("..", base.join("parent")).unwrap();
        symlink("/etc", base.join("etc")).unwrap();
        symlink("../../..", base.join("a/b/escape")).unwrap();
        symlink("loop2", base.join("loop1")).unwrap();
        symlink("loop1", base.join("loop2")).unwrap();
        let read = |rel: &str| -> Outcome<String> {
            let base_fd = tree.open()?;
            let path = base.join(rel);
            let fd = walk(tree, &base_fd, Path::new(rel), libc::O_RDONLY, &path)?;
            let mut text = String::new();
            File::from(fd).read_to_string(&mut text).unwrap();
            Ok(text)
        };
        for inside in ["a/b/f.txt", "rel/f.txt", "abs/b/f.txt", "a/b/up", "rel/up"] {
            assert_eq!(read(inside).unwrap(), "inside", "{inside}");
        }
        for outside in [
            "parent/x",
            "etc/passwd",
            "etc",
            "a/b/escape",
            "abs/b/escape/x",
        ] {
            assert_eq!(
                read(outside).unwrap_err().code,
                Code::OutsideRoot,
                "{outside}"
            );
        }
        assert_eq!(read("loop1").unwrap_err().code, Code::Symlink);
        // The root itself, and a final symlink not followed.
        let base_fd = tree.open().unwrap();
        let fd = walk(tree, &base_fd, Path::new(""), DIRECTORY, &base).unwrap();
        assert!(is_dir(&stat_fd(fd.as_raw_fd()).unwrap()));
        let fd = walk(
            tree,
            &base_fd,
            Path::new("etc"),
            libc::O_PATH | libc::O_NOFOLLOW,
            &base.join("etc"),
        )
        .unwrap();
        assert!(is_symlink(&stat_fd(fd.as_raw_fd()).unwrap()));
        // `open_beneath` agrees, through `openat2` where it can.
        let open = |rel: &str| open_beneath(tree, Path::new(rel), libc::O_RDONLY, true, &base);
        assert!(open("abs/b/f.txt").is_ok());
        assert!(open("rel/up").is_ok());
        assert_eq!(open("etc/passwd").unwrap_err().code, Code::OutsideRoot);
        assert_eq!(open("parent/x").unwrap_err().code, Code::OutsideRoot);
        std::fs::remove_dir_all(&base).unwrap();
    }
    #[test]
    fn a_symlink_swapped_while_resolving_never_escapes() {
        use std::os::unix::fs::symlink;
        use std::sync::Arc;
        let (base, root) = scratch("race");
        let tree = root.primary();
        let outside = base.with_extension("outside");
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("f.txt"), "outside").unwrap();
        std::fs::create_dir(base.join("in")).unwrap();
        std::fs::write(base.join("in/f.txt"), "inside").unwrap();
        symlink("in", base.join("swap")).unwrap();
        // Another thread keeps replacing `swap` (atomically, by rename)
        // with a link inside and a link outside.
        let stop = Arc::new(AtomicBool::new(false));
        let swapper = {
            let (base, outside, stop) = (base.clone(), outside.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    let next = base.join(".swap-next");
                    let _ = std::fs::remove_file(&next);
                    if n % 2 == 0 {
                        symlink(&outside, &next).unwrap();
                    } else {
                        symlink("in", &next).unwrap();
                    }
                    std::fs::rename(&next, base.join("swap")).unwrap();
                    n += 1;
                }
                n
            })
        };
        let path = base.join("swap/f.txt");
        let (mut inside, mut refused) = (0, 0);
        for attempt in 0..4000 {
            // Through `openat2` and through the walk alone.
            let result = if attempt % 2 == 0 {
                read(&root, &path, None).map(|content| content.content)
            } else {
                tree.open()
                    .and_then(|base_fd| {
                        walk(
                            tree,
                            &base_fd,
                            Path::new("swap/f.txt"),
                            libc::O_RDONLY,
                            &path,
                        )
                    })
                    .map(|fd| {
                        let mut text = String::new();
                        File::from(fd).read_to_string(&mut text).unwrap();
                        text
                    })
            };
            match result {
                Ok(text) => {
                    assert_eq!(text, "inside");
                    inside += 1;
                }
                Err(error) => {
                    assert!(
                        matches!(
                            error.code,
                            Code::OutsideRoot | Code::NotFound | Code::NotADirectory
                        ),
                        "{error:?}"
                    );
                    refused += 1;
                }
            }
        }
        stop.store(true, Ordering::Relaxed);
        let swaps = swapper.join().unwrap();
        assert_eq!(inside + refused, 4000);
        assert!(swaps > 0);
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }
    #[test]
    fn versions_compare_inode_size_and_times() {
        let mut a: libc::stat = unsafe { std::mem::zeroed() };
        a.st_ino = 7;
        a.st_size = 3;
        a.st_mtime = 100;
        let mut b = a;
        assert!(same_version(&a, &b));
        b.st_ctime_nsec = 1;
        assert!(!same_version(&a, &b));
        b = a;
        b.st_mtime_nsec = 1;
        assert!(!same_version(&a, &b));
        b = a;
        b.st_size = 4;
        assert!(!same_version(&a, &b));
    }
    #[test]
    fn wire_names_match_the_contract() {
        let name = |value: serde_json::Value| value.as_str().unwrap().to_owned();
        assert_eq!(
            name(serde_json::json!(Code::NotADirectory)),
            "not_a_directory"
        );
        assert_eq!(
            name(serde_json::json!(Code::IsADirectory)),
            "is_a_directory"
        );
        assert_eq!(name(serde_json::json!(Code::CrossDevice)), "cross_device");
        assert_eq!(name(serde_json::json!(Code::OutsideRoot)), "outside_root");
        assert_eq!(name(serde_json::json!(Encoding::Utf8)), "utf8");
        assert_eq!(name(serde_json::json!(Kind::Symlink)), "symlink");
        let request: Request = serde_json::from_str(
            r#"{"type":"file_write","request":"f4","path":"~/a","content":"x","base_hash":"e3"}"#,
        )
        .unwrap();
        match request {
            Request::FileWrite {
                encoding,
                create,
                base_hash,
                ..
            } => assert_eq!(
                (encoding, create, base_hash.as_deref()),
                (Encoding::Utf8, false, Some("e3"))
            ),
            other => panic!("{other:?}"),
        }
        // Unknown fields are refused, as for every stream request.
        assert!(
            serde_json::from_str::<Request>(
                r#"{"type":"file_stat","request":"f","path":"/","x":1}"#
            )
            .is_err()
        );
        let event = failure(
            "f".into(),
            Error::new(Code::Conflict, "changed").at(Path::new("/a")),
        );
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::json!({"type":"file_error","request":"f","error":{"code":"conflict","message":"changed","path":"/a"}})
        );
    }
}
