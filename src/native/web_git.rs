//! Read-only Git facts for the web client's per-project Repos view: each
//! checkout of a project (the project folder itself when it is one, and its
//! direct subfolders that are), with its branch, upstream, ahead/behind
//! counts, changed files and the last commit.
//!
//! # Threat model
//!
//! The repositories are the bridge user's own, but their content is not
//! trusted: a cloned repository, an unpacked archive or a checkout an agent
//! touched can carry a `.git/config` (or a file it includes) that names
//! programs Git runs on its own during commands that look read-only. Git
//! runs an fsmonitor hook from `core.fsmonitor` during `status`, clean and
//! smudge filter drivers (`filter.<name>.clean`, `.process`) when it
//! compares a file whose stat data changed, textconv and external diff
//! drivers (`diff.<driver>.textconv`, `.command`, `diff.external`) during
//! `diff`, `gpg.program` when `log.showSignature` is set, hooks such as
//! `post-index-change` when it refreshes the index, and transport programs
//! (`core.sshCommand`, `ext::` URLs, a partial clone's lazy fetch from its
//! promisor remote) whenever it reaches a remote. Viewing a repository must
//! never run any of them: everything shown is computed by [`Checkout::git`],
//! the only place that starts Git, which
//!
//! - starts a fixed Git binary with an argument vector, never a shell, with
//!   a cleared environment (only `HOME`, `XDG_CONFIG_HOME` and `PATH` are
//!   kept, `LC_ALL=C`), `GIT_CONFIG_NOSYSTEM=1`, `GIT_TERMINAL_PROMPT=0`,
//!   `GIT_OPTIONAL_LOCKS=0` (so `status` never rewrites the index),
//!   `GIT_ALLOW_PROTOCOL` empty and `protocol.allow=never` (no transport at
//!   all), `GIT_NO_LAZY_FETCH=1`, and `GIT_CEILING_DIRECTORIES` set to the
//!   checkout's parent so discovery cannot climb out of it;
//! - overrides on the command line, which wins over every configuration
//!   file: `core.fsmonitor=false`, `core.hooksPath=/dev/null`,
//!   `diff.external=`, `diff.autoRefreshIndex=false`,
//!   `log.showSignature=false`, `credential.helper=`,
//!   `submodule.recurse=false`, `status.submoduleSummary=false`, and every
//!   filter driver the configuration names (read first with `config -z
//!   --list`, includes followed) with empty `clean`, `smudge` and `process`
//!   commands and `required=false`;
//! - passes `--no-ext-diff --no-textconv` to `diff-index` and `diff-files`
//!   and `--ignore-submodules=all` to them and `status` (so it never starts
//!   Git inside a submodule, whose configuration it did not read);
//! - runs only `config --list`, `status`, the plumbing `diff-index` and
//!   `diff-files --numstat` (which, unlike `diff`, never refresh the index)
//!   and `log -1`, none of which writes or touches the network (the
//!   history operations of `web_git_history` add `rev-parse --verify`,
//!   `for-each-ref`, `log`, `diff-tree`, `ls-files` and patches from
//!   `diff-index` and `diff-files`, all read-only plumbing or `log`, with
//!   `--no-ext-diff --no-textconv` on every diff);
//! - runs in its own session with standard input closed, is killed with its
//!   process group after [`TIMEOUT`], at the request's deadline
//!   ([`BUDGET`]) or when the bridge dies, and has its standard output and
//!   error read up to fixed limits;
//! - only in a verified checkout root: a real directory owned by the
//!   bridge's user whose `.git` (a directory, or a file naming a worktree's
//!   or submodule's Git directory) is not a symbolic link and is owned by
//!   that user (see `web_repos::git_dir`). Symbolic links to subfolders are
//!   not followed and repositories whose metadata another user owns are
//!   skipped;
//! - only when nobody else can change what configures it between the
//!   listing that names the filter drivers and a later command: every file
//!   the repository configuration comes from or would include (local and
//!   worktree scope, `include.path` and `includeIf` targets even when
//!   missing or empty), `core.attributesFile`, `info/attributes`, the Git
//!   directory, a `commondir` target and alternate object directories must
//!   be owned by the user and not writable by group (unless it is the
//!   user's private group) or others, under directories owned by the user
//!   or root and not so writable unless sticky (see [`check_config`],
//!   [`check_git_dir`]); otherwise the checkout shows "skipped: untrusted
//!   config". The user's global configuration stays trusted. A checkout
//!   whose configuration (any scope) sets a `core.worktree` other than its
//!   own root (a submodule's names its own) shows "skipped: core.worktree is
//!   elsewhere": Git would treat another folder as its working tree.
//!
//! The browser never supplies a path: it names a project, whose folder
//! comes from the user configuration. Untracked files are read directly (to
//! count their lines) without following symbolic links or blocking, inside
//! the checkout and on its file system only, within size and count limits
//! and the request's deadline. Work is bounded per project ([`MAX_REPOS`],
//! [`BUDGET`]) and per bridge ([`Worker`]: a few threads and a short
//! queue); results are cached for a few seconds.
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Most checkouts inspected per project.
pub const MAX_REPOS: usize = 32;
/// Most directory entries of a project folder examined for checkouts.
const MAX_ENTRIES: usize = 4096;
/// Most changed files listed per checkout (all are counted).
pub const MAX_LISTED: usize = 200;
/// Untracked files read per checkout to count their lines.
const UNTRACKED_SCAN: usize = 200;
/// Larger untracked files are counted without lines.
const UNTRACKED_SIZE: u64 = 1024 * 1024;
/// Standard output kept of `status` and `diff`; beyond it counts are lower
/// bounds.
const OUTPUT_LIMIT: usize = 1024 * 1024;
/// Standard output kept of `config --list` and `log -1`.
const SMALL_OUTPUT: usize = 256 * 1024;
/// Standard error kept, for the error message.
const ERROR_LIMIT: usize = 16 * 1024;
/// Most filter drivers a configuration may name.
const MAX_FILTERS: usize = 64;
/// Longest a Git process may run.
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// A project stops starting checkouts after this long (the browser gives up
/// on a request after 15 seconds).
pub const BUDGET: Duration = Duration::from_secs(9);
/// How long a checkout's facts are reused while its `HEAD` and index are
/// unchanged, unless the browser asks for fresh ones.
const CACHE_TTL: Duration = Duration::from_secs(4);
/// Cached checkouts per bridge.
const CACHE_ENTRIES: usize = 256;
/// Threads computing projects, and projects waiting for one.
const THREADS: usize = 2;
const QUEUE: usize = 4;
/// A queued request older than this is refused rather than started.
const STALE: Duration = Duration::from_secs(4);

/// Where Git may be installed. A fixed list rather than `PATH`, so a
/// directory earlier in the user's `PATH` cannot supply another `git`.
const GIT_BINARIES: &[&str] = &["/usr/bin/git", "/usr/local/bin/git", "/bin/git"];

/// The last commit of a checkout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    /// Committer time, Unix seconds.
    pub time: i64,
}

/// One changed file of a checkout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Change {
    /// Relative to the checkout.
    pub path: String,
    /// One letter: `M`odified, `A`dded, `D`eleted, `R`enamed, `C`opied,
    /// `T`ype changed, `U`nmerged or `?` untracked.
    pub status: String,
    pub staged: bool,
    pub unstaged: bool,
    /// Lines against HEAD; absent for binary, oversized or unscanned files.
    pub added: Option<u64>,
    pub removed: Option<u64>,
    /// A rename's or copy's original path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// What the Repos view shows of one checkout.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Repo {
    /// The checkout root.
    pub path: String,
    /// Its folder name.
    pub name: String,
    /// The checked-out branch; absent when HEAD is detached.
    pub branch: Option<String>,
    /// HEAD's commit; absent before the first commit.
    pub head: Option<String>,
    pub detached: bool,
    /// The branch's upstream (`origin/main`), as configured.
    pub upstream: Option<String>,
    /// Commits on the branch and not on its upstream's local ref, and the
    /// reverse; absent without an upstream or when it is gone. No fetch.
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    /// Changed files: with staged changes, with unstaged changes (a file
    /// can be both), untracked and unmerged; `files` counts each once.
    pub files: u64,
    pub staged: u64,
    pub unstaged: u64,
    pub untracked: u64,
    pub conflicted: u64,
    /// Lines added and removed against HEAD, untracked text files included.
    pub added: u64,
    pub removed: u64,
    /// Changed files without line counts (binary, oversized or unscanned).
    pub uncounted: u64,
    pub last: Option<Commit>,
    /// The changed files (at most [`MAX_LISTED`]), when asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changes: Option<Vec<Change>>,
    /// Some output reached its limit: the counts are lower bounds.
    pub truncated: bool,
    /// Why the checkout could not be inspected.
    pub error: Option<String>,
    /// `owner/name` when the `origin` remote is a github.com repository
    /// (never the URL itself, which may hold credentials).
    pub github: Option<String>,
}

/// The `owner/name` of a github.com remote URL: `https://github.com/o/n`,
/// `git@github.com:o/n` or `ssh://git@github.com/o/n`, with or without
/// `.git`. URLs with credentials, another host or extra path are `None`.
pub fn github_slug(url: &str) -> Option<String> {
    let path = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let path = path.strip_suffix('/').unwrap_or(path);
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    let valid = super::user_config::github_name;
    (valid(owner) && valid(name)).then(|| format!("{owner}/{name}"))
}

type Result<T> = std::result::Result<T, String>;

/// A process's bounded output.
pub(crate) struct Output {
    pub(crate) bytes: Vec<u8>,
    pub(crate) truncated: bool,
}

fn drain(mut pipe: impl Read, limit: usize) -> Output {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    // Keeps reading past the limit (discarding) so the child never blocks
    // on a full pipe before it exits or is killed.
    while let Ok(n) = pipe.read(&mut chunk) {
        if n == 0 {
            break;
        }
        let take = n.min(limit.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&chunk[..take]);
        truncated |= take < n;
    }
    Output { bytes, truncated }
}

pub(crate) fn git_binary() -> Result<&'static str> {
    GIT_BINARIES
        .iter()
        .copied()
        .find(|path| fs::metadata(path).is_ok_and(|m| m.is_file()))
        .ok_or_else(|| "Git is not installed on this server".to_owned())
}

/// Configuration every Git process gets on its command line (see the
/// module's threat model).
const HARDENING: &[&str] = &[
    "core.fsmonitor=false",
    "core.hooksPath=/dev/null",
    "diff.external=",
    // Plumbing never refreshes the index; this keeps any porcelain from
    // rewriting it (`diff` ignores `GIT_OPTIONAL_LOCKS` when it does).
    "diff.autoRefreshIndex=false",
    "log.showSignature=false",
    "credential.helper=",
    "protocol.allow=never",
    "submodule.recurse=false",
    "status.submoduleSummary=false",
    "core.pager=cat",
    "color.ui=false",
    "gc.auto=0",
    "maintenance.auto=false",
    // A repository could raise this so that a diff loads a huge blob into
    // memory to look for NUL bytes and lines; above it a blob is binary.
    "core.bigFileThreshold=8m",
    // The log and commit parsers read UTF-8, whatever the repository asks.
    "i18n.logOutputEncoding=UTF-8",
];

/// A verified checkout root, the overrides of the filter drivers its
/// configuration names, its object format's empty tree and the instant its
/// Git commands must end by.
pub struct Checkout {
    root: PathBuf,
    filters: Vec<String>,
    empty_tree: &'static str,
    deadline: Instant,
    /// The `origin` remote's github.com `owner/name` ([`github_slug`]).
    github: Option<String>,
}

const EMPTY_TREE_SHA1: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const EMPTY_TREE_SHA256: &str = "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321";

/// Why a checkout is refused when a file its configuration comes from could
/// be changed by someone else.
pub const UNTRUSTED: &str = "skipped: untrusted config";

/// Why a checkout whose configuration sets a `core.worktree` other than its
/// own root (a submodule's Git directory names its own) is refused: Git
/// would read (and list) another folder's files as its working tree.
pub const WORKTREE: &str = "skipped: core.worktree is elsewhere";

/// The most alternate object directories followed (Git's own limit).
const ALTERNATES_DEPTH: usize = 5;

/// The bridge user's private group, when it has one (the usual umask-002
/// setup, where checkouts are group-writable): the effective group, with
/// no member and no primary user in the local user database but this user.
/// Writable by that group means writable by this user only.
fn private_group() -> Option<u32> {
    static GROUP: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *GROUP.get_or_init(|| {
        let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
        private_group_of(
            uid,
            gid,
            &fs::read_to_string("/etc/passwd").ok()?,
            &fs::read_to_string("/etc/group").ok()?,
        )
    })
}

/// [`private_group`] from the text of `/etc/passwd` and `/etc/group`.
pub fn private_group_of(uid: u32, gid: u32, passwd: &str, groups: &str) -> Option<u32> {
    {
        let mut name = None;
        for line in passwd.lines() {
            let fields: Vec<&str> = line.split(':').collect();
            let (Some(user), Some(Ok(u)), Some(Ok(g))) = (
                fields.first(),
                fields.get(2).map(|f| f.parse::<u32>()),
                fields.get(3).map(|f| f.parse::<u32>()),
            ) else {
                continue;
            };
            if u == uid {
                name = Some(*user);
            } else if g == gid {
                return None;
            }
        }
        let name = name?;
        let entry = groups.lines().find_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            (fields.get(2)?.parse::<u32>().ok()? == gid).then(|| fields.get(3).copied())
        })?;
        entry
            .unwrap_or("")
            .split(',')
            .all(|member| member.is_empty() || member == name)
            .then_some(gid)
    }
}

/// Whether a file or directory only the bridge's user can change: owned by
/// `uid` and not writable by group (other than [`private_group`]) or
/// others, with every ancestor directory owned by `uid` or root and not so
/// writable unless sticky (like `/tmp`, where nobody else can rename or
/// replace an entry). Symbolic links are resolved first.
/// A path that does not exist is judged by its nearest existing ancestor,
/// since whoever can write there could create it.
pub(super) fn trusted_path(path: &Path, uid: u32) -> bool {
    let mut path = path.to_path_buf();
    let canonical = loop {
        match path.canonicalize() {
            Ok(canonical) => break canonical,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !path.pop() || path.as_os_str().is_empty() {
                    return false;
                }
            }
            Err(_) => return false,
        }
    };
    for (depth, ancestor) in canonical.ancestors().enumerate() {
        let Ok(metadata) = fs::metadata(ancestor) else {
            return false;
        };
        let writable = metadata.mode() & 0o002 != 0
            || (metadata.mode() & 0o020 != 0 && Some(metadata.gid()) != private_group());
        let ok = if depth == 0 {
            metadata.uid() == uid && !writable
        } else {
            (metadata.uid() == uid || metadata.uid() == 0)
                && (!writable || metadata.mode() & 0o1000 != 0)
        };
        if !ok {
            return false;
        }
    }
    true
}

/// One entry of `config -z --list --show-scope --show-origin`.
#[derive(Debug, PartialEq)]
pub struct ConfigEntry<'a> {
    pub scope: &'a [u8],
    pub origin: &'a [u8],
    pub key: &'a [u8],
    pub value: &'a [u8],
}

/// Parses `config -z --list --show-scope --show-origin` output: per entry,
/// `scope\0origin\0key\nvalue\0`.
pub fn parse_config(bytes: &[u8]) -> Result<Vec<ConfigEntry<'_>>> {
    let mut fields = bytes.split(|b| *b == 0);
    let mut entries = Vec::new();
    while let Some(scope) = fields.next() {
        if scope.is_empty() {
            // The listing ends with a terminator.
            break;
        }
        let (Some(origin), Some(entry)) = (fields.next(), fields.next()) else {
            return Err("unexpected Git configuration listing".into());
        };
        let (key, value) = match entry.iter().position(|b| *b == b'\n') {
            Some(newline) => (&entry[..newline], &entry[newline + 1..]),
            None => (entry, &entry[entry.len()..]),
        };
        entries.push(ConfigEntry {
            scope,
            origin,
            key,
            value,
        });
    }
    Ok(entries)
}

/// A path named in a configuration value, as Git resolves it: `~/` from
/// `HOME`, relative to `base`. `None` for forms this module does not
/// resolve (`~user/`, `%(prefix)/`), which are refused.
fn config_path(value: &[u8], base: &Path) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    if value.is_empty() || value.contains(&0) {
        return None;
    }
    if let Some(rest) = value.strip_prefix(b"~/") {
        let home = std::env::var_os("HOME")?;
        return Some(Path::new(&home).join(std::ffi::OsStr::from_bytes(rest)));
    }
    if value.starts_with(b"~") || value.starts_with(b"%(") {
        return None;
    }
    Some(base.join(std::ffi::OsStr::from_bytes(value)))
}

/// Refuses a checkout whose repository configuration (local and worktree
/// scope, with everything they include or would include) or attributes
/// come from a file someone else could change: they could otherwise name
/// a program between the listing that [`filter_overrides`] reads and a
/// later command. The user's global configuration stays trusted.
pub fn check_config(entries: &[ConfigEntry], root: &Path, uid: u32) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let untrusted = || Err(UNTRUSTED.to_owned());
    let mut checked = std::collections::HashSet::new();
    let mut check = |path: PathBuf| -> Result<()> {
        if checked.insert(path.clone()) && !trusted_path(&path, uid) {
            return untrusted();
        }
        Ok(())
    };
    for entry in entries {
        if !matches!(entry.scope, b"local" | b"worktree") {
            continue;
        }
        let Some(origin) = entry.origin.strip_prefix(b"file:") else {
            return untrusted();
        };
        if origin.is_empty() {
            return untrusted();
        }
        // Relative origins are relative to the checkout root (Git's
        // working directory).
        let file = root.join(std::ffi::OsStr::from_bytes(origin));
        check(file.clone())?;
        let base = file.parent().unwrap_or(root).to_path_buf();
        let include = entry.key == b"include.path"
            || (entry.key.starts_with(b"includeif.") && entry.key.ends_with(b".path"));
        if include {
            // An include that is missing or empty now lists nothing, so its
            // target is checked from the key that names it.
            check(config_path(entry.value, &base).ok_or(UNTRUSTED)?)?;
        } else if entry.key == b"core.attributesfile" {
            check(config_path(entry.value, root).ok_or(UNTRUSTED)?)?;
        }
    }
    Ok(())
}

/// Refuses a checkout whose configuration (any scope) sets a
/// `core.worktree` that is not the checkout root itself ([`WORKTREE`]): Git
/// would read, list and diff another folder's files. A relative value is
/// relative to the Git directory, as Git reads it; a submodule's Git
/// directory names its own checkout this way.
pub fn check_worktree(entries: &[ConfigEntry], git_dir: &Path, root: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let root = fs::canonicalize(root).map_err(|_| WORKTREE.to_owned())?;
    for entry in entries.iter().filter(|e| e.key == b"core.worktree") {
        let named = git_dir.join(std::ffi::OsStr::from_bytes(entry.value));
        match fs::canonicalize(named) {
            Ok(path) if path == root => {}
            _ => return Err(WORKTREE.to_owned()),
        }
    }
    Ok(())
}
/// The first line of a small Git metadata file, `None` when it is missing.
fn metadata_line(path: &Path) -> Result<Option<String>> {
    let mut text = String::new();
    match fs::File::open(path) {
        Ok(file) => {
            file.take(4096)
                .read_to_string(&mut text)
                .map_err(|_| UNTRUSTED.to_owned())?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(UNTRUSTED.into()),
    }
    Ok(Some(text.lines().next().unwrap_or("").trim().to_owned()))
}

/// Refuses a Git directory whose shared metadata (`commondir`), alternate
/// object directories or `info/attributes` someone else could change.
pub fn check_git_dir(git_dir: &Path, uid: u32) -> Result<()> {
    let untrusted = || Err(UNTRUSTED.to_owned());
    if !trusted_path(git_dir, uid) {
        return untrusted();
    }
    let common = match metadata_line(&git_dir.join("commondir"))? {
        Some(target) if target.is_empty() || target.contains('\0') => return untrusted(),
        Some(target) => {
            let common = git_dir.join(target);
            if !trusted_path(&git_dir.join("commondir"), uid) || !trusted_path(&common, uid) {
                return untrusted();
            }
            common
        }
        None => git_dir.to_path_buf(),
    };
    if !trusted_path(&common.join("info/attributes"), uid) {
        return untrusted();
    }
    // Alternates, and theirs, as far as Git follows them.
    let mut objects = vec![common.join("objects")];
    for _ in 0..=ALTERNATES_DEPTH {
        let mut next = Vec::new();
        for directory in objects {
            let file = directory.join("info/alternates");
            let text = match fs::File::open(&file) {
                Ok(handle) => {
                    if !trusted_path(&file, uid) {
                        return untrusted();
                    }
                    let mut text = String::new();
                    handle
                        .take(64 * 1024)
                        .read_to_string(&mut text)
                        .map_err(|_| UNTRUSTED.to_owned())?;
                    text
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return untrusted(),
            };
            for line in text.lines() {
                let line = line.trim_end();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                // Quoted (escaped) paths are not resolved here.
                if line.starts_with('"') {
                    return untrusted();
                }
                let target = directory.join(line);
                if !trusted_path(&target, uid) {
                    return untrusted();
                }
                next.push(target);
            }
        }
        if next.is_empty() {
            return Ok(());
        }
        objects = next;
    }
    untrusted()
}

/// Whether `path` is a checkout root Git may run in: a directory owned by
/// `uid` (not a symbolic link unless `follow`) with Git metadata of its own
/// (see `web_repos::git_dir`). Returns its Git directory.
pub fn checkout_root(path: &Path, uid: u32, follow: bool) -> Option<PathBuf> {
    let metadata = if follow {
        fs::metadata(path)
    } else {
        fs::symlink_metadata(path)
    }
    .ok()?;
    if !metadata.is_dir() || metadata.uid() != uid || !path.is_absolute() {
        return None;
    }
    super::web_repos::git_dir(path, uid)
}

impl Checkout {
    /// Opens a checkout root after verifying it and the files its
    /// configuration comes from (see [`check_git_dir`], [`check_config`]),
    /// and reads which filter drivers that configuration names. Its Git
    /// commands must end by `deadline`.
    pub fn open(root: &Path, uid: u32, follow: bool, deadline: Instant) -> Result<Self> {
        let git_dir = checkout_root(root, uid, follow)
            .ok_or_else(|| "not a Git checkout of this user".to_owned())?;
        check_git_dir(&git_dir, uid)?;
        let mut checkout = Self {
            root: root.to_path_buf(),
            filters: Vec::new(),
            empty_tree: EMPTY_TREE_SHA1,
            deadline,
            github: None,
        };
        let list = checkout.git(
            &["config", "-z", "--list", "--show-scope", "--show-origin"],
            SMALL_OUTPUT,
        )?;
        if list.truncated {
            return Err("the Git configuration is too large".into());
        }
        let entries = parse_config(&list.bytes)?;
        check_config(&entries, root, uid)?;
        check_worktree(&entries, &git_dir, root)?;
        checkout.filters = filter_overrides(&entries)?;
        if entries
            .iter()
            .any(|e| e.key == b"extensions.objectformat" && e.value == b"sha256")
        {
            checkout.empty_tree = EMPTY_TREE_SHA256;
        }
        checkout.github = entries
            .iter()
            .rev()
            .find(|e| e.key == b"remote.origin.url")
            .and_then(|e| std::str::from_utf8(e.value).ok())
            .and_then(github_slug);
        Ok(checkout)
    }

    /// The verified checkout root.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// The empty tree of the repository's object format.
    pub(crate) fn empty_tree(&self) -> &'static str {
        self.empty_tree
    }

    /// The instant this checkout's Git commands must end by.
    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Runs one Git command in the checkout through the hardened runner
    /// (see [`Checkout::git`]); for `web_git_history`, whose commands are
    /// read-only too and take every argument after `--end-of-options` or
    /// `--`.
    pub(crate) fn run(&self, args: &[&str], limit: usize) -> Result<Output> {
        self.git(args, limit)
    }

    /// Runs one Git command in the checkout through the hardened runner,
    /// killed after [`TIMEOUT`] or at the checkout's deadline.
    fn git(&self, args: &[&str], limit: usize) -> Result<Output> {
        let started = Instant::now();
        if started >= self.deadline {
            return Err(OUT_OF_TIME.into());
        }
        let deadline = (started + TIMEOUT).min(self.deadline);
        let mut command = Command::new(git_binary()?);
        command.env_clear();
        for key in ["HOME", "XDG_CONFIG_HOME"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let ceiling = self.root.parent().unwrap_or(Path::new("/"));
        command
            .env("PATH", "/usr/bin:/bin")
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_ALLOW_PROTOCOL", "")
            // Git 2.44 and later; older Git ignores it, and the empty
            // `GIT_ALLOW_PROTOCOL` with `protocol.allow=never` still keep a
            // lazy fetch (like any transport) off the network.
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_PAGER", "cat")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_CEILING_DIRECTORIES", ceiling)
            .arg("--no-pager")
            .arg("--no-optional-locks");
        for setting in HARDENING {
            command.arg("-c").arg(setting);
        }
        for setting in &self.filters {
            command.arg("-c").arg(setting);
        }
        command
            .args(args)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 || libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("Git could not start: {error}"))?;
        let stdout = child.stdout.take().ok_or("Git output unavailable")?;
        let stderr = child.stderr.take().ok_or("Git output unavailable")?;
        let out = thread::spawn(move || drain(stdout, limit));
        let err = thread::spawn(move || drain(stderr, ERROR_LIMIT));
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                _ => {
                    // The whole session: Git and anything it started.
                    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let output = out.join().map_err(|_| "Git output reader failed")?;
        let error = err.join().map_err(|_| "Git output reader failed")?;
        match status {
            None if deadline == self.deadline => Err(OUT_OF_TIME.into()),
            None => Err(format!("Git took longer than {}s", TIMEOUT.as_secs())),
            Some(status) if !status.success() => Err(failure(&error.bytes, status.code())),
            Some(_) => Ok(output),
        }
    }
}

/// A short, printable message from Git's standard error.
fn failure(stderr: &[u8], code: Option<i32>) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let line: String = line.chars().filter(|c| !c.is_control()).take(200).collect();
    match (line.is_empty(), code) {
        (false, _) => format!("Git: {line}"),
        (true, Some(code)) => format!("Git exited with status {code}"),
        (true, None) => "Git was stopped by a signal".into(),
    }
}

/// `-c` settings that neutralize every filter driver named in the
/// configuration: empty clean, smudge and process commands, not required.
pub fn filter_overrides(entries: &[ConfigEntry]) -> Result<Vec<String>> {
    let mut drivers = std::collections::BTreeSet::new();
    for entry in entries {
        // Keys are case-insensitive in their first and last parts, and Git
        // lowercases them in this listing.
        let Some(rest) = entry.key.strip_prefix(b"filter.") else {
            continue;
        };
        let Some(dot) = rest.iter().rposition(|b| *b == b'.') else {
            continue;
        };
        let name = std::str::from_utf8(&rest[..dot])
            .map_err(|_| "the Git configuration names a filter driver that is not UTF-8")?;
        if name.contains('=') || name.chars().any(char::is_control) {
            return Err("the Git configuration names an unsupported filter driver".into());
        }
        drivers.insert(name.to_owned());
    }
    if drivers.len() > MAX_FILTERS {
        return Err("the Git configuration names too many filter drivers".into());
    }
    Ok(drivers
        .iter()
        .flat_map(|name| {
            ["clean=", "smudge=", "process=", "required=false"]
                .map(|setting| format!("filter.{name}.{setting}"))
        })
        .collect())
}

/// Parsed `status --porcelain=v2 --branch -z` output.
#[derive(Debug, Default, PartialEq)]
pub struct Status {
    pub head: Option<String>,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub changes: Vec<Change>,
}

pub(crate) fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The one letter a status pair shows: the worktree's change when there is
/// one, else the index's.
fn letter(xy: &[u8]) -> (String, bool, bool) {
    let x = xy.first().copied().unwrap_or(b'.');
    let y = xy.get(1).copied().unwrap_or(b'.');
    let shown = if y != b'.' { y } else { x };
    ((shown as char).to_string(), x != b'.', y != b'.')
}

pub fn parse_status(bytes: &[u8], truncated: bool) -> Status {
    let mut status = Status::default();
    let mut records: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    if truncated {
        // The last record may be cut short.
        records.pop();
    }
    let mut records = records.into_iter();
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix(b"# ") {
            let header = text(header);
            if let Some(oid) = header.strip_prefix("branch.oid ") {
                status.head = (oid != "(initial)").then(|| oid.to_owned());
            } else if let Some(head) = header.strip_prefix("branch.head ") {
                status.branch = (head != "(detached)").then(|| head.to_owned());
            } else if let Some(upstream) = header.strip_prefix("branch.upstream ") {
                status.upstream = Some(upstream.to_owned());
            } else if let Some(ab) = header.strip_prefix("branch.ab ") {
                let mut counts = ab.split_whitespace();
                status.ahead = counts
                    .next()
                    .and_then(|a| a.trim_start_matches('+').parse().ok());
                status.behind = counts
                    .next()
                    .and_then(|b| b.trim_start_matches('-').parse().ok());
            }
            continue;
        }
        let fields = |n: usize| record.splitn(n, |b| *b == b' ').collect::<Vec<_>>();
        let change = match record.first() {
            Some(b'1') => {
                let f = fields(9);
                (f.len() == 9).then(|| {
                    let (status, staged, unstaged) = letter(f[1]);
                    (text(f[8]), status, staged, unstaged, None)
                })
            }
            Some(b'2') => {
                let f = fields(10);
                // A rename or copy is followed by its original path.
                let from = records.next().map(text);
                (f.len() == 10).then(|| {
                    let (status, staged, unstaged) = letter(f[1]);
                    (text(f[9]), status, staged, unstaged, from)
                })
            }
            Some(b'u') => {
                let f = fields(11);
                (f.len() == 11).then(|| (text(f[10]), "U".to_owned(), false, false, None))
            }
            Some(b'?') => record
                .strip_prefix(b"? ")
                .map(|path| (text(path), "?".to_owned(), false, false, None)),
            _ => None,
        };
        if let Some((path, letter, staged, unstaged, from)) = change {
            status.changes.push(Change {
                path,
                status: letter,
                staged,
                unstaged,
                added: None,
                removed: None,
                from,
            });
        }
    }
    status
}

/// `diff --numstat -z --no-renames` records: path → (added, removed), with
/// `None` for a binary file.
pub fn parse_numstat(bytes: &[u8], truncated: bool) -> Vec<(String, Option<(u64, u64)>)> {
    let mut records: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    if truncated {
        records.pop();
    }
    let number = |field: &[u8]| std::str::from_utf8(field).ok()?.parse::<u64>().ok();
    records
        .into_iter()
        .filter_map(|record| {
            let mut fields = record.splitn(3, |b| *b == b'\t');
            let (added, removed, path) = (fields.next()?, fields.next()?, fields.next()?);
            let lines = number(added).zip(number(removed));
            Some((text(path), lines))
        })
        .collect()
}

/// Lines of text as Git counts an added file, or `None` for binary content
/// (a NUL byte in the first 8000 bytes, Git's heuristic).
pub fn text_lines(bytes: &[u8]) -> Option<u64> {
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return None;
    }
    let newlines = bytes.iter().filter(|b| **b == b'\n').count() as u64;
    Some(newlines + u64::from(bytes.last().is_some_and(|b| *b != b'\n')))
}

/// Lines of an untracked file, or `None` when not counted: not a regular
/// file inside `root` (no symbolic link on the way), too large, binary or
/// unreadable, or on another file system than the checkout root (`device`),
/// such as a mount that could hang. Never blocks on a FIFO.
fn untracked_lines(root: &Path, device: u64, relative: &str) -> Option<u64> {
    match read_untracked(root, device, relative, UNTRACKED_SIZE)? {
        Untracked::Content(bytes) => text_lines(&bytes),
        Untracked::TooLarge(_) => None,
    }
}

/// What [`read_untracked`] found.
pub(crate) enum Untracked {
    Content(Vec<u8>),
    /// Larger than the limit: its size, unread.
    TooLarge(u64),
}

/// An untracked file's content, at most `limit` bytes, under the rules of
/// [`untracked_lines`]: `None` when it is not a regular file inside `root`
/// reached without symbolic links, on the checkout's file system.
pub(crate) fn read_untracked(
    root: &Path,
    device: u64,
    relative: &str,
    limit: u64,
) -> Option<Untracked> {
    let relative = Path::new(relative);
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let path = root.join(relative);
    let parent = path.parent()?.canonicalize().ok()?;
    if !parent.starts_with(root.canonicalize().ok()?) || fs::metadata(&parent).ok()?.dev() != device
    {
        return None;
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(parent.join(path.file_name()?))
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.dev() != device {
        return None;
    }
    if metadata.len() > limit {
        return Some(Untracked::TooLarge(metadata.len()));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > limit {
        return Some(Untracked::TooLarge(bytes.len() as u64));
    }
    Some(Untracked::Content(bytes))
}

/// Inspects one verified checkout (see [`Checkout::open`]).
pub fn inspect(checkout: &Checkout, list: bool) -> Result<Repo> {
    let out = checkout.git(
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=all",
        ],
        OUTPUT_LIMIT,
    )?;
    let status = parse_status(&out.bytes, out.truncated);
    let mut repo = Repo {
        branch: status.branch.clone(),
        detached: status.branch.is_none() && status.head.is_some(),
        head: status.head.clone(),
        upstream: status.upstream.clone(),
        ahead: status.ahead,
        behind: status.behind,
        truncated: out.truncated,
        github: checkout.github.clone(),
        ..Repo::default()
    };
    let mut changes = status.changes;
    let tracked = changes.iter().any(|c| c.status != "?");
    // Lines per path against HEAD (before the first commit: the index
    // against the empty tree, then the worktree against the index).
    // Plumbing only: unlike `diff`, `diff-index` and `diff-files` never
    // refresh (rewrite) the index when stat data changed.
    let mut lines: HashMap<String, Option<(u64, u64)>> = HashMap::new();
    if tracked {
        let parts: &[&[&str]] = if repo.head.is_some() {
            &[&["diff-index", "HEAD"]]
        } else {
            &[
                &["diff-index", "--cached", checkout.empty_tree],
                &["diff-files"],
            ]
        };
        for part in parts {
            let mut args = vec![
                part[0],
                "--numstat",
                "-z",
                "--no-renames",
                "--no-ext-diff",
                "--no-textconv",
                "--ignore-submodules=all",
            ];
            args.extend_from_slice(&part[1..]);
            args.push("--");
            let out = checkout.git(&args, OUTPUT_LIMIT)?;
            repo.truncated |= out.truncated;
            for (path, counted) in parse_numstat(&out.bytes, out.truncated) {
                let entry = lines.entry(path).or_insert(Some((0, 0)));
                *entry = match (*entry, counted) {
                    (Some((a, r)), Some((more_a, more_r))) => Some((a + more_a, r + more_r)),
                    _ => None,
                };
            }
        }
        // Lines of paths no listed change names (a rename's original).
        for (path, counted) in &lines {
            if changes.iter().any(|c| &c.path == path) {
                continue;
            }
            if let Some((a, r)) = counted {
                repo.added += a;
                repo.removed += r;
            }
        }
    }
    let mut scanned = 0;
    let device = fs::metadata(&checkout.root).map(|m| m.dev()).ok();
    for change in &mut changes {
        repo.files += 1;
        match change.status.as_str() {
            "?" => repo.untracked += 1,
            "U" => repo.conflicted += 1,
            _ => {}
        }
        repo.staged += u64::from(change.staged);
        repo.unstaged += u64::from(change.unstaged);
        let counted = if change.status == "?" {
            scanned += 1;
            (scanned <= UNTRACKED_SCAN && Instant::now() < checkout.deadline)
                .then_some(device)
                .flatten()
                .and_then(|device| untracked_lines(&checkout.root, device, &change.path))
                .map(|added| (added, 0))
        } else {
            lines.get(&change.path).copied().flatten()
        };
        match counted {
            Some((added, removed)) => {
                change.added = Some(added);
                change.removed = Some(removed);
                repo.added += added;
                repo.removed += removed;
            }
            None => repo.uncounted += 1,
        }
    }
    if repo.head.is_some() {
        let out = checkout.git(
            &[
                "log",
                "-1",
                "--no-color",
                "--format=%H%x00%ct%x00%s",
                "HEAD",
                "--",
            ],
            SMALL_OUTPUT,
        )?;
        let mut fields = out.bytes.splitn(3, |b| *b == 0);
        if let (Some(sha), Some(time), Some(subject)) =
            (fields.next(), fields.next(), fields.next())
        {
            let subject = text(subject);
            repo.last = Some(Commit {
                sha: text(sha).trim().to_owned(),
                time: text(time).trim().parse().unwrap_or(0),
                subject: subject
                    .trim_end()
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(200)
                    .collect(),
            });
        }
    }
    if list {
        changes.truncate(MAX_LISTED);
        repo.changes = Some(changes);
    }
    Ok(repo)
}

/// The checkouts of a project folder: the folder itself when it is one,
/// then its direct subfolders that are (by name; hidden folders and
/// symbolic links skipped), at most [`MAX_REPOS`]. The second value says
/// whether more were left out.
pub fn discover(folder: &Path, uid: u32) -> (Vec<PathBuf>, bool) {
    let mut found = Vec::new();
    if checkout_root(folder, uid, true).is_some() {
        found.push(folder.to_path_buf());
    }
    let mut children: Vec<PathBuf> = match fs::read_dir(folder) {
        Ok(entries) => entries
            .take(MAX_ENTRIES)
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let name = entry.file_name();
                if name.as_encoded_bytes().starts_with(b".") {
                    return None;
                }
                // The entry's own type: a symbolic link is not a folder.
                entry.file_type().ok()?.is_dir().then(|| entry.path())
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    children.sort();
    let mut truncated = false;
    for child in children {
        if checkout_root(&child, uid, false).is_none() {
            continue;
        }
        if found.len() >= MAX_REPOS {
            truncated = true;
            break;
        }
        found.push(child);
    }
    (found, truncated)
}

/// The modification times that change with a commit, a checkout, a stage
/// or a branch switch.
type Signature = (Option<SystemTime>, Option<SystemTime>);
fn signature(git_dir: &Path) -> Signature {
    let modified = |name: &str| {
        fs::metadata(git_dir.join(name))
            .and_then(|m| m.modified())
            .ok()
    };
    (modified("HEAD"), modified("index"))
}

struct Cached {
    signature: Signature,
    at: Instant,
    repo: Repo,
}

/// Recently inspected checkouts of this bridge.
#[derive(Clone, Default)]
pub struct Cache(Arc<Mutex<HashMap<PathBuf, Cached>>>);

impl Cache {
    /// One checkout's facts: cached when fresh enough and `HEAD` and the
    /// index have not changed since, else inspected now.
    fn repo(
        &self,
        root: &Path,
        uid: u32,
        follow: bool,
        request: &Request,
        deadline: Instant,
    ) -> Repo {
        let (list, fresh) = (request.list, request.fresh);
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned());
        let path = root.to_string_lossy().into_owned();
        let git_dir = checkout_root(root, uid, follow);
        let current = git_dir.as_deref().map(signature);
        if !fresh
            && let Some(current) = current
            && let Ok(cache) = self.0.lock()
            && let Some(cached) = cache.get(root)
            && cached.signature == current
            && cached.at.elapsed() < CACHE_TTL
            && (cached.repo.changes.is_some() || !list)
        {
            let mut repo = cached.repo.clone();
            if !list {
                repo.changes = None;
            }
            return repo;
        }
        let mut repo = Checkout::open(root, uid, follow, deadline)
            .and_then(|checkout| inspect(&checkout, list))
            .unwrap_or_else(|error| Repo {
                error: Some(error),
                ..Repo::default()
            });
        repo.name = name;
        repo.path = path;
        let complete = repo.error.as_deref() != Some(OUT_OF_TIME);
        if let (true, Some(current), Ok(mut cache)) = (complete, current, self.0.lock()) {
            if cache.len() >= CACHE_ENTRIES {
                cache.retain(|_, cached| cached.at.elapsed() < CACHE_TTL);
                if cache.len() >= CACHE_ENTRIES {
                    cache.clear();
                }
            }
            cache.insert(
                root.to_path_buf(),
                Cached {
                    signature: current,
                    at: Instant::now(),
                    repo: repo.clone(),
                },
            );
        }
        repo
    }
}

/// What one `git` request asks for.
pub struct Request {
    /// The project's folder, from the user configuration.
    pub folder: PathBuf,
    /// List each checkout's changed files.
    pub list: bool,
    /// Ignore cached facts.
    pub fresh: bool,
}

/// The error of a checkout the project's [`BUDGET`] left no time for.
pub(crate) const OUT_OF_TIME: &str = "not inspected: this project's time budget ran out";

/// The checkouts of a project folder, inspected within [`BUDGET`]: every
/// Git command ends by the request's deadline.
pub fn project(cache: &Cache, request: &Request) -> Value {
    let uid = unsafe { libc::geteuid() };
    let deadline = Instant::now() + BUDGET;
    let (roots, truncated) = discover(&request.folder, uid);
    let repos: Vec<Repo> = roots
        .iter()
        .map(|root| {
            let follow = root == &request.folder;
            if Instant::now() >= deadline {
                return Repo {
                    path: root.to_string_lossy().into_owned(),
                    name: root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    error: Some(OUT_OF_TIME.into()),
                    ..Repo::default()
                };
            }
            cache.repo(root, uid, follow, request, deadline)
        })
        .collect();
    let sampled_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    json!({
        "folder": request.folder,
        "exists": request.folder.is_dir(),
        "repos": repos,
        "truncated": truncated,
        "sampled_at": sampled_at,
    })
}

type Reply = Box<dyn FnOnce(std::result::Result<Value, String>) + Send>;
/// Work for a Git thread: a project's facts, or a history operation
/// (`web_git_history`), given the bridge's checkout cache.
pub type Task = Box<dyn FnOnce(&Cache) -> std::result::Result<Value, String> + Send>;
struct Job {
    task: Task,
    queued: Instant,
    reply: Reply,
}

/// The bridge's Git work: [`THREADS`] threads and a queue of [`QUEUE`]
/// requests; a request beyond that, or one that waited longer than
/// [`STALE`], is refused so the browser asks again later.
pub struct Worker {
    jobs: mpsc::SyncSender<Job>,
}

impl Default for Worker {
    fn default() -> Self {
        Self::start()
    }
}

impl Worker {
    pub fn start() -> Self {
        let (jobs, queue) = mpsc::sync_channel::<Job>(QUEUE);
        let queue = Arc::new(Mutex::new(queue));
        let cache = Cache::default();
        for _ in 0..THREADS {
            let queue = queue.clone();
            let cache = cache.clone();
            thread::spawn(move || {
                loop {
                    let job = match queue.lock() {
                        Ok(queue) => queue.recv(),
                        Err(_) => return,
                    };
                    let Ok(job) = job else { return };
                    if job.queued.elapsed() > STALE {
                        (job.reply)(Err(BUSY.into()));
                        continue;
                    }
                    (job.reply)((job.task)(&cache));
                }
            });
        }
        Self { jobs }
    }

    /// Queues a project's request; `reply` receives its result on a worker
    /// thread.
    pub fn submit(&self, request: Request, reply: Reply) -> std::result::Result<(), String> {
        self.submit_task(Box::new(move |cache| Ok(project(cache, &request))), reply)
    }

    /// Queues any Git work under the same bounds (threads, queue, staleness).
    pub fn submit_task(&self, task: Task, reply: Reply) -> std::result::Result<(), String> {
        self.jobs
            .try_send(Job {
                task,
                queued: Instant::now(),
                reply,
            })
            .map_err(|_| BUSY.to_owned())
    }
}

const BUSY: &str = "Git status is busy; it refreshes again shortly";

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "kelpmux-git-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Sets up a test repository with the test's own Git (not the hardened
    /// runner), ignoring the user's and system configuration.
    fn setup(dir: &Path, args: &[&str]) {
        let status = Command::new(git_binary().expect("git is needed for these tests"))
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }
    fn uid() -> u32 {
        unsafe { libc::geteuid() }
    }
    /// What a group-writable file's checkout opens to.
    fn group_writable() -> Result<()> {
        if private_group() == Some(unsafe { libc::getegid() }) {
            Ok(())
        } else {
            Err(UNTRUSTED.to_owned())
        }
    }
    fn later() -> Instant {
        Instant::now() + BUDGET
    }
    fn mode(path: &Path, mode: u32) {
        fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(mode)).unwrap();
    }
    fn append_config(repo: &Path, text: &str) {
        let config = repo.join(".git/config");
        let mut existing = fs::read_to_string(&config).unwrap();
        existing.push_str(text);
        fs::write(config, existing).unwrap();
    }
    fn repo_at(path: &Path) -> Repo {
        let checkout = Checkout::open(path, uid(), true, later()).unwrap();
        inspect(&checkout, true).unwrap()
    }
    fn init(path: &Path) {
        fs::create_dir_all(path).unwrap();
        setup(path, &["init", "-q", "-b", "main"]);
    }

    #[test]
    fn status_counts_lines_upstream_and_last_commit() {
        let temp = Temp::new();
        let upstream = temp.0.join("upstream");
        init(&upstream);
        fs::write(upstream.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        fs::write(upstream.join("gone.txt"), "x\n").unwrap();
        setup(&upstream, &["add", "."]);
        setup(&upstream, &["commit", "-q", "-m", "First commit"]);
        let work = temp.0.join("work");
        setup(&temp.0, &["clone", "-q", "upstream", "work"]);
        fs::write(work.join("a.txt"), "one\n2\nthree\nfour\n").unwrap();
        fs::remove_file(work.join("gone.txt")).unwrap();
        fs::write(work.join("new.txt"), "a\nb").unwrap();
        fs::write(work.join("staged.txt"), "s\n").unwrap();
        setup(&work, &["add", "staged.txt"]);
        let repo = repo_at(&work);
        assert_eq!(repo.branch.as_deref(), Some("main"));
        assert_eq!(repo.upstream.as_deref(), Some("origin/main"));
        assert_eq!((repo.ahead, repo.behind), (Some(0), Some(0)));
        assert_eq!(repo.files, 4);
        assert_eq!((repo.staged, repo.unstaged, repo.untracked), (1, 2, 1));
        // a.txt: +2 −1; gone.txt: −1; new.txt: +2; staged.txt: +1.
        assert_eq!((repo.added, repo.removed), (5, 2));
        let last = repo.last.clone().unwrap();
        assert_eq!(last.subject, "First commit");
        assert_eq!(last.sha.len(), 40);
        assert!(last.time > 0);
        let changes = repo.changes.unwrap();
        let find = |path: &str| changes.iter().find(|c| c.path == path).unwrap().clone();
        assert_eq!(find("a.txt").status, "M");
        assert_eq!(
            (find("a.txt").added, find("a.txt").removed),
            (Some(2), Some(1))
        );
        assert_eq!(find("gone.txt").status, "D");
        assert_eq!(find("new.txt").status, "?");
        assert_eq!(find("new.txt").added, Some(2));
        assert!(find("staged.txt").staged && !find("staged.txt").unstaged);
        // A local commit is ahead of the upstream's local ref (no fetch).
        setup(&work, &["commit", "-q", "-m", "Second"]);
        let repo = repo_at(&work);
        assert_eq!((repo.ahead, repo.behind), (Some(1), Some(0)));
        assert_eq!(repo.last.unwrap().subject, "Second");
    }

    #[test]
    fn an_empty_repository_and_a_detached_head() {
        let temp = Temp::new();
        let path = temp.0.join("empty");
        init(&path);
        fs::write(path.join("f"), "x\n").unwrap();
        setup(&path, &["add", "f"]);
        let repo = repo_at(&path);
        assert_eq!(repo.head, None);
        assert_eq!(repo.branch.as_deref(), Some("main"));
        assert_eq!((repo.files, repo.added), (1, 1));
        assert_eq!(repo.last, None);
        setup(&path, &["commit", "-q", "-m", "c"]);
        setup(&path, &["checkout", "-q", "--detach"]);
        let repo = repo_at(&path);
        assert!(repo.detached);
        assert_eq!(repo.branch, None);
        assert_eq!(repo.files, 0);
    }

    /// A repository whose configuration names programs for every hook Git
    /// could run during status, diff and log: none of them may run.
    #[test]
    fn configured_programs_never_run() {
        let temp = Temp::new();
        let path = temp.0.join("evil");
        init(&path);
        fs::write(path.join(".gitattributes"), "* filter=evil diff=evil\n").unwrap();
        fs::write(path.join("data.txt"), "one\n").unwrap();
        setup(&path, &["add", "."]);
        setup(&path, &["commit", "-q", "-m", "c"]);
        let marker = temp.0.join("ran");
        let program = format!("sh -c 'echo $0 >> {}' ", marker.display());
        let hooks = temp.0.join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        for hook in ["post-index-change", "pre-commit", "post-checkout"] {
            let file = hooks.join(hook);
            fs::write(
                &file,
                format!("#!/bin/sh\necho {hook} >> {}\n", marker.display()),
            )
            .unwrap();
            fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
        }
        let included = temp.0.join("included.config");
        fs::write(
            &included,
            format!("[filter \"included\"]\n\tclean = {program}included-clean\n"),
        )
        .unwrap();
        let config = format!(
            "[core]\n\tfsmonitor = {program}fsmonitor\n\thooksPath = {hooks}\n\tsshCommand = {program}ssh\n\
             [filter \"evil\"]\n\tclean = {program}clean\n\tsmudge = {program}smudge\n\tprocess = {program}process\n\trequired = true\n\
             [diff \"evil\"]\n\ttextconv = {program}textconv\n\tcommand = {program}diff-command\n\
             [diff]\n\texternal = {program}external\n\
             [log]\n\tshowSignature = true\n[gpg]\n\tprogram = {program}gpg\n\
             [include]\n\tpath = {included}\n",
            hooks = hooks.display(),
            included = included.display(),
        );
        let mut existing = fs::read_to_string(path.join(".git/config")).unwrap();
        existing.push_str(&config);
        fs::write(path.join(".git/config"), existing).unwrap();
        fs::write(path.join(".gitattributes"), "* filter=included diff=evil\n").unwrap();
        // Same size, new content and time: Git must look at the content.
        std::thread::sleep(Duration::from_millis(1100));
        fs::write(path.join("data.txt"), "two\n").unwrap();
        let index_before = fs::read(path.join(".git/index")).unwrap();
        let repo = repo_at(&path);
        assert!(
            !marker.exists(),
            "a configured program ran: {}",
            fs::read_to_string(&marker).unwrap_or_default()
        );
        assert_eq!(repo.error, None);
        assert_eq!(repo.files, 2);
        assert_eq!((repo.added, repo.removed), (2, 2));
        assert!(repo.last.is_some());
        assert_eq!(fs::read(path.join(".git/index")).unwrap(), index_before);
    }

    /// Files whose stat data changed but whose content did not: inspecting
    /// must not refresh (rewrite) the index, split or not, nor leave a lock.
    #[test]
    fn inspecting_never_writes_the_index() {
        for split in [false, true] {
            let temp = Temp::new();
            let path = temp.0.join("repo");
            init(&path);
            if split {
                setup(&path, &["config", "core.splitIndex", "true"]);
            }
            for name in ["a.txt", "b.txt", "c.txt"] {
                fs::write(path.join(name), "same\n").unwrap();
            }
            setup(&path, &["add", "."]);
            setup(&path, &["commit", "-q", "-m", "c"]);
            // A tracked change, so the line counts run too.
            fs::write(path.join("c.txt"), "changed\n").unwrap();
            let listing = |path: &Path| {
                let mut names: Vec<_> = fs::read_dir(path.join(".git"))
                    .unwrap()
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect();
                names.sort();
                names
            };
            let index_before = fs::read(path.join(".git/index")).unwrap();
            let names_before = listing(&path);
            std::thread::sleep(Duration::from_millis(1100));
            for name in ["a.txt", "b.txt"] {
                // New modification time, same content.
                fs::write(path.join(name), "same\n").unwrap();
            }
            let repo = repo_at(&path);
            assert_eq!(repo.error, None);
            assert_eq!((repo.files, repo.added, repo.removed), (1, 1, 1));
            assert_eq!(
                fs::read(path.join(".git/index")).unwrap(),
                index_before,
                "split index: {split}"
            );
            let names = listing(&path);
            assert_eq!(names, names_before, "split index: {split}");
            assert!(!path.join(".git/index.lock").exists());
        }
    }

    /// Configuration included from a file, or Git metadata in a directory,
    /// that someone else could write is refused before any command reads it.
    #[test]
    fn untrusted_config_is_refused() {
        let temp = Temp::new();
        let path = temp.0.join("repo");
        init(&path);
        let shared = temp.0.join("shared");
        fs::create_dir_all(&shared).unwrap();
        let included = shared.join("shared.cfg");
        fs::write(&included, "").unwrap();
        append_config(
            &path,
            &format!("[include]\n\tpath = {}\n", included.display()),
        );
        let open = |path: &Path| Checkout::open(path, uid(), true, later()).map(|_| ());
        assert_eq!(open(&path), Ok(()));
        // A group- or other-writable include, existing or not yet (group
        // write is the user's own in a private group).
        mode(&included, 0o666);
        assert_eq!(open(&path), Err(UNTRUSTED.to_owned()));
        mode(&included, 0o620);
        assert_eq!(open(&path), group_writable());
        fs::remove_file(&included).unwrap();
        mode(&shared, 0o777);
        assert_eq!(open(&path), Err(UNTRUSTED.to_owned()));
        mode(&shared, 0o755);
        assert_eq!(open(&path), Ok(()));
        // Relative to the including file, through `includeIf` too.
        let other = temp.0.join("other");
        init(&other);
        fs::create_dir_all(temp.0.join("open")).unwrap();
        mode(&temp.0.join("open"), 0o777);
        append_config(
            &other,
            "[includeIf \"gitdir:/\"]\n\tpath = ../../open/x.cfg\n",
        );
        assert_eq!(open(&other), Err(UNTRUSTED.to_owned()));
        // An attributes file the configuration names.
        let attributes = temp.0.join("attrs");
        init(&attributes);
        append_config(
            &attributes,
            "[core]\n\tattributesFile = ../open/attributes\n",
        );
        assert_eq!(open(&attributes), Err(UNTRUSTED.to_owned()));
        let info = temp.0.join("info");
        init(&info);
        fs::create_dir_all(info.join(".git/info")).unwrap();
        fs::write(info.join(".git/info/attributes"), "").unwrap();
        mode(&info.join(".git/info/attributes"), 0o646);
        assert_eq!(open(&info), Err(UNTRUSTED.to_owned()));
        // The refusal reaches the view as the checkout's error.
        let value = project(
            &Cache::default(),
            &Request {
                folder: other.clone(),
                list: false,
                fresh: true,
            },
        );
        assert_eq!(value["repos"][0]["error"], UNTRUSTED);
    }

    /// A `core.worktree` elsewhere would list and diff another folder's
    /// files; one naming the checkout itself (as a submodule's) is fine.
    #[test]
    fn a_worktree_elsewhere_is_refused() {
        let temp = Temp::new();
        let path = temp.0.join("repo");
        init(&path);
        let outside = temp.0.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "secret\n").unwrap();
        let open = |path: &Path| Checkout::open(path, uid(), true, later()).map(|_| ());
        append_config(&path, "[core]\n\tworktree = ../../outside\n");
        assert_eq!(open(&path), Err(WORKTREE.to_owned()));
        let value = project(
            &Cache::default(),
            &Request {
                folder: path.clone(),
                list: true,
                fresh: true,
            },
        );
        assert_eq!(value["repos"][0]["error"], WORKTREE, "{value}");
        assert!(!value.to_string().contains("secret"), "{value}");
        // Its own root, relative to the Git directory or absolute.
        let own = temp.0.join("own");
        init(&own);
        append_config(&own, "[core]\n\tworktree = ..\n");
        assert_eq!(open(&own), Ok(()));
        let absolute = temp.0.join("absolute");
        init(&absolute);
        append_config(
            &absolute,
            &format!("[core]\n\tworktree = {}\n", absolute.display()),
        );
        assert_eq!(open(&absolute), Ok(()));
    }

    /// A `commondir` (even in a plain `.git` directory) or alternate object
    /// directory someone else owns or can write is refused.
    #[test]
    fn untrusted_common_dir_and_alternates_are_refused() {
        let temp = Temp::new();
        let open = |path: &Path| Checkout::open(path, uid(), true, later()).map(|_| ());
        let path = temp.0.join("repo");
        init(&path);
        let common = temp.0.join("common");
        fs::create_dir_all(&common).unwrap();
        fs::write(
            path.join(".git/commondir"),
            format!("{}\n", common.display()),
        )
        .unwrap();
        mode(&common, 0o777);
        assert_eq!(open(&path), Err(UNTRUSTED.to_owned()));
        mode(&common, 0o775);
        assert_eq!(open(&path), group_writable());
        if uid() != 0 {
            // Owned by root, not by the user.
            fs::write(path.join(".git/commondir"), "/usr\n").unwrap();
            assert_eq!(open(&path), Err(UNTRUSTED.to_owned()));
        }
        fs::remove_file(path.join(".git/commondir")).unwrap();
        assert_eq!(open(&path), Ok(()));
        let objects = temp.0.join("objects");
        fs::create_dir_all(&objects).unwrap();
        fs::create_dir_all(path.join(".git/objects/info")).unwrap();
        fs::write(
            path.join(".git/objects/info/alternates"),
            format!("{}\n", objects.display()),
        )
        .unwrap();
        assert_eq!(open(&path), Ok(()));
        mode(&objects, 0o777);
        assert_eq!(open(&path), Err(UNTRUSTED.to_owned()));
        mode(&objects, 0o755);
        // An alternate's own alternates are followed.
        fs::create_dir_all(objects.join("info")).unwrap();
        fs::write(objects.join("info/alternates"), "/tmp\n").unwrap();
        assert_eq!(open(&path), Err(UNTRUSTED.to_owned()));
    }

    /// The request's deadline bounds every Git command, not only the gaps
    /// between checkouts.
    #[test]
    fn a_passed_deadline_runs_no_git() {
        let temp = Temp::new();
        let path = temp.0.join("repo");
        init(&path);
        let past = Instant::now();
        assert_eq!(
            Checkout::open(&path, uid(), true, past).map(|_| ()),
            Err(OUT_OF_TIME.to_owned())
        );
    }

    #[test]
    fn discovery_takes_the_folder_and_its_direct_children_only() {
        let temp = Temp::new();
        let project = temp.0.join("project");
        init(&project);
        init(&project.join("b-repo"));
        init(&project.join("a-repo"));
        init(&project.join("plain/deeper"));
        init(&project.join(".hidden"));
        fs::create_dir_all(project.join("not-a-repo")).unwrap();
        let elsewhere = temp.0.join("elsewhere");
        init(&elsewhere);
        std::os::unix::fs::symlink(&elsewhere, project.join("linked")).unwrap();
        let (found, truncated) = discover(&project, uid());
        assert!(!truncated);
        assert_eq!(
            found,
            vec![
                project.clone(),
                project.join("a-repo"),
                project.join("b-repo")
            ]
        );
        // Through the bridge's request, with a cache.
        let value = super::project(
            &Cache::default(),
            &Request {
                folder: project.clone(),
                list: false,
                fresh: false,
            },
        );
        let repos = value["repos"].as_array().unwrap();
        assert_eq!(repos.len(), 3);
        assert_eq!(repos[1]["name"], "a-repo");
        assert!(repos[1].get("changes").is_none());
        // A symlinked `.git` is not a checkout.
        let odd = temp.0.join("odd");
        fs::create_dir_all(&odd).unwrap();
        std::os::unix::fs::symlink(elsewhere.join(".git"), odd.join(".git")).unwrap();
        assert!(Checkout::open(&odd, uid(), true, later()).is_err());
    }

    #[test]
    fn parsers() {
        let status = parse_status(
            b"# branch.oid 0123\0# branch.head feat/x\0# branch.upstream origin/feat/x\0# branch.ab +3 -2\0\
              1 .M N... 100644 100644 100644 aa bb src/a b.rs\0\
              2 R. N... 100644 100644 100644 aa bb R100 new.rs\0old.rs\0\
              u UU N... 100644 100644 100644 100644 aa bb cc both.rs\0? new dir/x\0",
            false,
        );
        assert_eq!(status.branch.as_deref(), Some("feat/x"));
        assert_eq!((status.ahead, status.behind), (Some(3), Some(2)));
        let paths: Vec<_> = status
            .changes
            .iter()
            .map(|c| (c.path.as_str(), c.status.as_str()))
            .collect();
        assert_eq!(
            paths,
            [
                ("src/a b.rs", "M"),
                ("new.rs", "R"),
                ("both.rs", "U"),
                ("new dir/x", "?")
            ]
        );
        assert_eq!(status.changes[1].from.as_deref(), Some("old.rs"));
        assert!(status.changes[1].staged && !status.changes[1].unstaged);
        assert_eq!(
            parse_numstat(b"1\t2\ta.rs\0-\t-\timg.png\0", false),
            [
                ("a.rs".to_owned(), Some((1, 2))),
                ("img.png".to_owned(), None)
            ]
        );
        const LISTING: &[u8] =
            b"global\0file:/home/u/.gitconfig\0filter.lfs.clean\ngit-lfs clean\0\
              local\0file:.git/config\0core.bare\nfalse\0command\0command line:\0flag\0";
        let entries = parse_config(LISTING).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries[1],
            ConfigEntry {
                scope: b"local",
                origin: b"file:.git/config",
                key: b"core.bare",
                value: b"false"
            }
        );
        assert_eq!(entries[2].value, b"");
        assert!(parse_config(b"local\0file:x").is_err());
        assert_eq!(
            filter_overrides(&entries).unwrap(),
            [
                "filter.lfs.clean=",
                "filter.lfs.smudge=",
                "filter.lfs.process=",
                "filter.lfs.required=false"
            ]
        );
        assert!(
            filter_overrides(
                &parse_config(b"local\0file:.git/config\0filter.a=b.clean\nx\0").unwrap()
            )
            .is_err()
        );
        let passwd = "root:x:0:0::/root:/bin/sh\njoan:x:1001:1002::/home/joan:/bin/zsh\n";
        let groups = "root:x:0:\njoan:x:1002:\nadm:x:4:syslog,joan\n";
        assert_eq!(private_group_of(1001, 1002, passwd, groups), Some(1002));
        let shared = "joan:x:1002:joan,eve\n";
        assert_eq!(private_group_of(1001, 1002, passwd, shared), None);
        let primary = format!("{passwd}eve:x:1003:1002::/home/eve:/bin/sh\n");
        assert_eq!(private_group_of(1001, 1002, &primary, groups), None);
        assert_eq!(private_group_of(1001, 4, passwd, groups), None);
        assert_eq!(text_lines(b"a\nb"), Some(2));
        assert_eq!(text_lines(b"a\0b"), None);
    }

    #[test]
    fn github_remotes_give_their_owner_and_name_only() {
        for url in [
            "https://github.com/joan/kelp.git",
            "https://github.com/joan/kelp",
            "git@github.com:joan/kelp.git",
            "ssh://git@github.com/joan/kelp.git",
        ] {
            assert_eq!(github_slug(url).as_deref(), Some("joan/kelp"), "{url}");
        }
        for url in [
            "https://x-access-token:secret@github.com/joan/kelp.git",
            "https://gitlab.com/joan/kelp.git",
            "https://github.com/joan/kelp/tree/main",
            "https://github.com/joan",
            "https://github.com/../kelp",
            "/srv/git/kelp.git",
        ] {
            assert_eq!(github_slug(url), None, "{url}");
        }
        // The Repos view reads it from the checkout's own configuration.
        let temp = Temp::new();
        let path = temp.0.join("r");
        init(&path);
        setup(
            &path,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/joan/kelp.git",
            ],
        );
        assert_eq!(repo_at(&path).github.as_deref(), Some("joan/kelp"));
    }
}
