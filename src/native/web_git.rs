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
//!   `diff.external=`, `log.showSignature=false`, `credential.helper=`,
//!   `submodule.recurse=false`, `status.submoduleSummary=false`, and every
//!   filter driver the configuration names (read first with `config -z
//!   --list`, includes followed) with empty `clean`, `smudge` and `process`
//!   commands and `required=false`;
//! - passes `--no-ext-diff --no-textconv` to `diff` and
//!   `--ignore-submodules=all` to `status` and `diff` (so it never starts
//!   Git inside a submodule, whose configuration it did not read);
//! - runs only `config --list`, `status`, `diff --numstat` and `log -1`,
//!   none of which writes or touches the network;
//! - runs in its own session with standard input closed, is killed with its
//!   process group after [`TIMEOUT`] or when the bridge dies, and has its
//!   standard output and error read up to fixed limits;
//! - only in a verified checkout root: a real directory owned by the
//!   bridge's user whose `.git` (a directory, or a file naming a worktree's
//!   or submodule's Git directory) is not a symbolic link and is owned by
//!   that user (see `web_repos::git_dir`). Symbolic links to subfolders are
//!   not followed and repositories whose metadata another user owns are
//!   skipped.
//!
//! The browser never supplies a path: it names a project, whose folder
//! comes from the user configuration. Untracked files are read directly (to
//! count their lines) without following symbolic links or blocking, inside
//! the checkout only and within size and count limits. Work is bounded per
//! project ([`MAX_REPOS`], [`BUDGET`]) and per bridge ([`Worker`]: a few
//! threads and a short queue); results are cached for a few seconds.
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
}

type Result<T> = std::result::Result<T, String>;

/// A process's bounded output.
struct Output {
    bytes: Vec<u8>,
    truncated: bool,
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

fn git_binary() -> Result<&'static str> {
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
    "log.showSignature=false",
    "credential.helper=",
    "protocol.allow=never",
    "submodule.recurse=false",
    "status.submoduleSummary=false",
    "core.pager=cat",
    "color.ui=false",
    "gc.auto=0",
    "maintenance.auto=false",
];

/// A verified checkout root and the overrides of the filter drivers its
/// configuration names.
pub struct Checkout {
    root: PathBuf,
    filters: Vec<String>,
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
    /// Opens a checkout root after verifying it, and reads which filter
    /// drivers its configuration names.
    pub fn open(root: &Path, uid: u32, follow: bool) -> Result<Self> {
        checkout_root(root, uid, follow)
            .ok_or_else(|| "not a Git checkout of this user".to_owned())?;
        let mut checkout = Self {
            root: root.to_path_buf(),
            filters: Vec::new(),
        };
        let list = checkout.git(&["config", "-z", "--list"], SMALL_OUTPUT)?;
        if list.truncated {
            return Err("the Git configuration is too large".into());
        }
        checkout.filters = filter_overrides(&list.bytes)?;
        Ok(checkout)
    }

    /// Runs one Git command in the checkout through the hardened runner.
    fn git(&self, args: &[&str], limit: usize) -> Result<Output> {
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
        let deadline = Instant::now() + TIMEOUT;
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

/// `-c` settings that neutralize every filter driver named in `config -z
/// --list` output: empty clean, smudge and process commands, not required.
pub fn filter_overrides(list: &[u8]) -> Result<Vec<String>> {
    let mut drivers = std::collections::BTreeSet::new();
    for entry in list.split(|b| *b == 0) {
        // Each entry is `key\nvalue`; keys are case-insensitive in their
        // first and last parts, and Git lowercases them in this listing.
        let key = entry.split(|b| *b == b'\n').next().unwrap_or_default();
        let Some(rest) = key.strip_prefix(b"filter.") else {
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

fn text(bytes: &[u8]) -> String {
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
/// unreadable. Never blocks on a FIFO.
fn untracked_lines(root: &Path, relative: &str) -> Option<u64> {
    let relative = Path::new(relative);
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let path = root.join(relative);
    let parent = path.parent()?.canonicalize().ok()?;
    if !parent.starts_with(root.canonicalize().ok()?) {
        return None;
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(parent.join(path.file_name()?))
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > UNTRACKED_SIZE {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(UNTRACKED_SIZE + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > UNTRACKED_SIZE {
        return None;
    }
    text_lines(&bytes)
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
        ..Repo::default()
    };
    let mut changes = status.changes;
    let tracked = changes.iter().any(|c| c.status != "?");
    // Lines per path against HEAD (before the first commit: the index
    // against the empty tree, then the worktree against the index).
    let mut lines: HashMap<String, Option<(u64, u64)>> = HashMap::new();
    if tracked {
        let parts: &[&[&str]] = if repo.head.is_some() {
            &[&["HEAD"]]
        } else {
            &[&["--cached"], &[]]
        };
        for extra in parts {
            let mut args = vec![
                "diff",
                "--numstat",
                "-z",
                "--no-renames",
                "--no-ext-diff",
                "--no-textconv",
                "--ignore-submodules=all",
            ];
            args.extend_from_slice(extra);
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
            (scanned <= UNTRACKED_SCAN)
                .then(|| untracked_lines(&checkout.root, &change.path))
                .flatten()
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
    fn repo(&self, root: &Path, uid: u32, follow: bool, list: bool, fresh: bool) -> Repo {
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
        let mut repo = Checkout::open(root, uid, follow)
            .and_then(|checkout| inspect(&checkout, list))
            .unwrap_or_else(|error| Repo {
                error: Some(error),
                ..Repo::default()
            });
        repo.name = name;
        repo.path = path;
        if let (Some(current), Ok(mut cache)) = (current, self.0.lock()) {
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

/// The checkouts of a project folder, inspected within [`BUDGET`].
pub fn project(cache: &Cache, request: &Request) -> Value {
    let uid = unsafe { libc::geteuid() };
    let started = Instant::now();
    let (roots, truncated) = discover(&request.folder, uid);
    let repos: Vec<Repo> = roots
        .iter()
        .map(|root| {
            let follow = root == &request.folder;
            if started.elapsed() >= BUDGET {
                return Repo {
                    path: root.to_string_lossy().into_owned(),
                    name: root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    error: Some("not inspected: this project's time budget ran out".into()),
                    ..Repo::default()
                };
            }
            cache.repo(root, uid, follow, request.list, request.fresh)
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
struct Job {
    request: Request,
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
                    (job.reply)(Ok(project(&cache, &job.request)));
                }
            });
        }
        Self { jobs }
    }

    /// Queues a request; `reply` receives its result on a worker thread.
    pub fn submit(&self, request: Request, reply: Reply) -> std::result::Result<(), String> {
        self.jobs
            .try_send(Job {
                request,
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
    fn repo_at(path: &Path) -> Repo {
        let checkout = Checkout::open(path, uid(), true).unwrap();
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
        assert!(Checkout::open(&odd, uid(), true).is_err());
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
        assert_eq!(
            filter_overrides(b"filter.lfs.clean\ngit-lfs clean\0core.bare\nfalse\0").unwrap(),
            [
                "filter.lfs.clean=",
                "filter.lfs.smudge=",
                "filter.lfs.process=",
                "filter.lfs.required=false"
            ]
        );
        assert!(filter_overrides(b"filter.a=b.clean\nx\0").is_err());
        assert_eq!(text_lines(b"a\nb"), Some(2));
        assert_eq!(text_lines(b"a\0b"), None);
    }
}
