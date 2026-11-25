//! The web client's History: closed terminals with the agent conversations
//! they hosted, plus conversations started outside KelpMux in a project's
//! folder, each with a title taken from its first user prompt.
//!
//! The transcripts belong to Claude Code and Codex, whose formats are
//! internal and change; they are read defensively and as little as possible:
//! only up to the first user prompt, of which a one-line title of at most
//! [`TITLE_CHARS`] characters is kept. Nothing else of a transcript leaves
//! this module, and a transcript that cannot be read or parsed only loses
//! its title (or, for an outside conversation, its row); it never fails the
//! list.
//!
//! - Claude Code: `$CLAUDE_CONFIG_DIR` (default `~/.claude`)
//!   `/projects/<cwd with '/' and '.' replaced by '-'>/<session id>.jsonl`.
//! - Codex: `$CODEX_HOME` (default `~/.codex`)
//!   `/sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl`, whose first record is
//!   the `session_meta`.
//!
//! Every directory below those roots is entered only when it is a real
//! directory (never a symbolic link) owned by the bridge's user, and every
//! transcript is opened without following symbolic links or blocking, and
//! read only when it is a regular file owned by that user. The outside
//! conversations are bounded: transcripts modified in the last [`DAYS`]
//! days, at most [`PER_PROJECT`] per project and [`MAX_EXTERNAL`] in all,
//! the Codex walk limited to those days' date folders. What was parsed is
//! kept in memory per bridge ([`Index`]) and parsed again only when the file
//! was replaced, or changed while it still had no title. The work runs on
//! the bridge's own History thread ([`Worker`]), never on the terminals'.
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Longest title, in characters (an ellipsis replaces the rest).
pub const TITLE_CHARS: usize = 80;
/// Outside conversations are those modified in the last this many days.
pub const DAYS: u64 = 30;
/// Most outside conversations per project (Scratch included).
pub const PER_PROJECT: usize = 200;
/// Most outside conversations in one listing.
pub const MAX_EXTERNAL: usize = 1000;
/// Most entries read from one directory.
const MAX_DIR_ENTRIES: usize = 4096;
/// Most Codex transcripts considered in one walk.
const MAX_CODEX_FILES: usize = 5000;
/// A transcript line longer than this is skipped unparsed.
const LINE_LIMIT: usize = 1024 * 1024;
/// Most bytes read from one transcript looking for its first prompt.
const READ_LIMIT: u64 = 4 * 1024 * 1024;
/// Most lines read from one transcript looking for its first prompt.
const LINE_COUNT: usize = 400;
/// Most transcripts kept in the in-memory index.
const INDEX_LIMIT: usize = 20_000;
/// A queued listing older than this is refused (the browser asks again).
const STALE: Duration = Duration::from_secs(20);
const BUSY: &str = "History is busy; try again shortly";

/// A project (or Scratch) whose folder outside conversations are matched
/// against.
#[derive(Debug, Clone)]
pub struct Scope {
    pub id: String,
    pub folder: PathBuf,
}

/// One History listing.
#[derive(Debug, Clone, Default)]
pub struct Request {
    /// This workspace's name.
    pub workspace: String,
    /// The bridge's `closed` entries (with `project` and `choices`).
    pub entries: Vec<Value>,
    /// The conversations live terminals run: `(kind, id)`.
    pub live: Vec<(String, String)>,
    /// The projects and Scratch, for outside conversations.
    pub scopes: Vec<Scope>,
}

/// Where the transcripts live.
#[derive(Debug, Clone, Default)]
pub struct Roots {
    pub claude: Option<PathBuf>,
    pub codex: Option<PathBuf>,
}
impl Roots {
    /// From this process's environment: `$CLAUDE_CONFIG_DIR` or
    /// `~/.claude`, `$CODEX_HOME` or `~/.codex` (absolute paths only).
    pub fn current() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        let pick = |variable: &str, default: &str| {
            std::env::var_os(variable)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .or_else(|| home.as_ref().map(|home| home.join(default)))
        };
        Self {
            claude: pick("CLAUDE_CONFIG_DIR", ".claude"),
            codex: pick("CODEX_HOME", ".codex"),
        }
    }
}

/// What a transcript says about its conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Meta {
    kind: &'static str,
    id: String,
    cwd: Option<String>,
    title: Option<String>,
    /// When the first prompt (Codex: the session) was sent, epoch seconds.
    first: Option<u64>,
    /// A subagent's or side chain's transcript, never listed on its own.
    sub: bool,
}

#[derive(Debug, Clone)]
struct Cached {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: i64,
    meta: Option<Meta>,
}

/// A transcript found: its facts and when it last changed.
#[derive(Debug, Clone)]
struct Found {
    meta: Meta,
    modified: u64,
}

/// The bridge's in-memory transcript index.
#[derive(Debug)]
pub struct Index {
    uid: u32,
    files: HashMap<PathBuf, Cached>,
}
impl Default for Index {
    fn default() -> Self {
        Self::for_user(unsafe { libc::geteuid() })
    }
}

impl Index {
    /// An index that trusts only files and folders owned by `uid`.
    pub fn for_user(uid: u32) -> Self {
        Self {
            uid,
            files: HashMap::new(),
        }
    }

    /// The History listing: `{workspace, entries, external, truncated}`.
    /// Each entry gains `conversations` (newest first); `external` lists the
    /// outside conversations, newest first, each with its `project`.
    pub fn list(&mut self, roots: &Roots, request: &Request, now: SystemTime) -> Value {
        let now_secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let since = now_secs.saturating_sub(DAYS * 86_400);
        let codex = roots
            .codex
            .as_deref()
            .map(|root| self.codex_files(root, now_secs))
            .unwrap_or_default();
        let mut linked: HashSet<(String, String)> = request.live.iter().cloned().collect();
        let mut entries = Vec::with_capacity(request.entries.len());
        for entry in &request.entries {
            let mut entry = entry.clone();
            let conversations = conversations_of(&entry);
            let terminal_cwd = entry["cwd"].as_str().unwrap_or_default().to_owned();
            let name = entry["name"].as_str().unwrap_or_default().to_owned();
            let mut rows = Vec::with_capacity(conversations.len());
            for conversation in conversations.into_iter().rev() {
                linked.insert((conversation.kind.to_owned(), conversation.id.clone()));
                let found = self.find(roots, &codex, &conversation);
                let (title, first, last) = match &found {
                    Some(found) => (
                        found.meta.title.clone(),
                        conversation.first_seen.or(found.meta.first),
                        conversation.last_seen.or(Some(found.modified)),
                    ),
                    None => (None, conversation.first_seen, conversation.last_seen),
                };
                let folder = [conversation.cwd.as_str(), terminal_cwd.as_str()]
                    .into_iter()
                    .find(|path| is_folder(path))
                    .map(str::to_owned);
                rows.push(json!({
                    "kind": conversation.kind,
                    "id": conversation.id,
                    "cwd": conversation.cwd,
                    "folder": folder,
                    "title": title.unwrap_or_else(|| name.clone()),
                    "titled": found.as_ref().is_some_and(|f| f.meta.title.is_some()),
                    "first_seen": first,
                    "last_seen": last,
                }));
            }
            entry["conversations"] = Value::Array(rows);
            entries.push(entry);
        }
        let (external, truncated) = self.external(roots, &codex, request, &linked, since);
        json!({
            "workspace": request.workspace,
            "entries": entries,
            "external": external,
            "truncated": truncated,
        })
    }

    /// The transcript of a closed terminal's conversation, if found.
    fn find(&mut self, roots: &Roots, codex: &[CodexFile], c: &Conversation) -> Option<Found> {
        match c.kind {
            "claude" => {
                let projects = roots.claude.as_deref()?.join("projects");
                if !self.owned_dir(&projects, false) {
                    return None;
                }
                for name in claude_dir_names(&c.cwd) {
                    let dir = projects.join(&name);
                    if !self.owned_dir(&dir, false) {
                        continue;
                    }
                    let path = dir.join(format!("{}.jsonl", c.id));
                    if let Some(found) = self.read(&path, "claude")
                        && found.meta.id == c.id
                    {
                        return Some(found);
                    }
                }
                None
            }
            _ => {
                let suffix = format!("-{}.jsonl", c.id);
                let mut paths: Vec<PathBuf> = codex
                    .iter()
                    .filter(|file| file.name.ends_with(&suffix))
                    .map(|file| file.path.clone())
                    .collect();
                // Older than the walk: the date folders around when the
                // server first saw it.
                if paths.is_empty()
                    && let (Some(root), Some(first)) = (roots.codex.as_deref(), c.first_seen)
                {
                    for day in [first / 86_400 - 1, first / 86_400, first / 86_400 + 1] {
                        let dir = date_dir(&root.join("sessions"), day);
                        if self.owned_path(&root.join("sessions"), &dir) {
                            paths.extend(
                                self.list_dir(&dir)
                                    .into_iter()
                                    .filter(|(name, _)| name.ends_with(&suffix))
                                    .map(|(_, path)| path),
                            );
                        }
                    }
                }
                paths.into_iter().find_map(|path| {
                    self.read(&path, "codex")
                        .filter(|found| found.meta.id == c.id)
                })
            }
        }
    }

    /// Conversations whose folder is inside a project's (or Scratch's) and
    /// that no closed or live terminal hosted.
    fn external(
        &mut self,
        roots: &Roots,
        codex: &[CodexFile],
        request: &Request,
        linked: &HashSet<(String, String)>,
        since: u64,
    ) -> (Vec<Value>, bool) {
        let mut found: Vec<(usize, Found)> = Vec::new();
        let mut counts = vec![0usize; request.scopes.len()];
        let mut truncated = false;
        // Claude Code: the folders named after a project folder or one
        // inside it, newest transcripts first.
        if let Some(root) = roots.claude.as_deref() {
            let projects = root.join("projects");
            if self.owned_dir(&projects, false) {
                let dirs: Vec<(String, PathBuf)> =
                    self.list_entries(&projects, true).into_iter().collect();
                for (index, scope) in request.scopes.iter().enumerate() {
                    let prefix = normalize(&scope.folder.to_string_lossy());
                    let mut candidates: Vec<(u64, PathBuf)> = Vec::new();
                    for (name, dir) in &dirs {
                        let name = normalize(name);
                        if name != prefix && !name.starts_with(&format!("{prefix}-")) {
                            continue;
                        }
                        for (file, path) in self.list_dir(dir) {
                            if !file.ends_with(".jsonl") {
                                continue;
                            }
                            if let Some(modified) = self.modified(&path)
                                && modified >= since
                            {
                                candidates.push((modified, path));
                            }
                        }
                    }
                    candidates.sort_by_key(|c| std::cmp::Reverse(c.0));
                    if candidates.len() > PER_PROJECT {
                        truncated = true;
                    }
                    for (_, path) in candidates.into_iter().take(PER_PROJECT) {
                        let Some(hit) = self.read(&path, "claude") else {
                            continue;
                        };
                        if hit.meta.sub
                            || !inside(hit.meta.cwd.as_deref(), &scope.folder)
                            || linked.contains(&("claude".into(), hit.meta.id.clone()))
                        {
                            continue;
                        }
                        counts[index] += 1;
                        found.push((index, hit));
                    }
                }
            }
        }
        // Codex: the date folders of the last days, newest first.
        for file in codex {
            if file.modified < since {
                continue;
            }
            let Some(hit) = self.read(&file.path, "codex") else {
                continue;
            };
            if hit.meta.sub || linked.contains(&("codex".into(), hit.meta.id.clone())) {
                continue;
            }
            let Some(index) = request
                .scopes
                .iter()
                .position(|scope| inside(hit.meta.cwd.as_deref(), &scope.folder))
            else {
                continue;
            };
            if counts[index] >= PER_PROJECT {
                truncated = true;
                continue;
            }
            counts[index] += 1;
            found.push((index, hit));
        }
        found.sort_by_key(|f| std::cmp::Reverse(f.1.modified));
        let mut seen = HashSet::new();
        found.retain(|(_, hit)| seen.insert((hit.meta.kind, hit.meta.id.clone())));
        if found.len() > MAX_EXTERNAL {
            truncated = true;
            found.truncate(MAX_EXTERNAL);
        }
        let rows = found
            .into_iter()
            .map(|(index, hit)| {
                let scope = &request.scopes[index];
                let cwd = hit.meta.cwd.unwrap_or_default();
                let folder = if is_folder(&cwd) {
                    Some(cwd.clone())
                } else {
                    Some(scope.folder.to_string_lossy().into_owned())
                        .filter(|folder| is_folder(folder))
                };
                json!({
                    "kind": hit.meta.kind,
                    "id": hit.meta.id,
                    "cwd": cwd,
                    "folder": folder,
                    "title": hit.meta.title,
                    "titled": hit.meta.title.is_some(),
                    "first_seen": hit.meta.first,
                    "last_seen": hit.modified,
                    "project": scope.id,
                })
            })
            .collect();
        (rows, truncated)
    }

    /// Codex transcripts in the date folders of the last [`DAYS`] days
    /// (and tomorrow's, for time zones ahead of UTC), newest first.
    fn codex_files(&mut self, root: &Path, now: u64) -> Vec<CodexFile> {
        let sessions = root.join("sessions");
        if !self.owned_dir(root, true) || !self.owned_dir(&sessions, false) {
            return Vec::new();
        }
        let today = now / 86_400;
        let mut files = Vec::new();
        for day in (today.saturating_sub(DAYS + 1)..=today + 1).rev() {
            let dir = date_dir(&sessions, day);
            if !self.owned_path(&sessions, &dir) {
                continue;
            }
            for (name, path) in self.list_dir(&dir) {
                if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                    continue;
                }
                if let Some(modified) = self.modified(&path) {
                    files.push(CodexFile {
                        name,
                        path,
                        modified,
                    });
                }
                if files.len() >= MAX_CODEX_FILES {
                    break;
                }
            }
        }
        files.sort_by_key(|f| std::cmp::Reverse(f.modified));
        files
    }

    /// `dir` and every folder between `base` and it are real directories
    /// owned by the user (`base` itself checked too).
    fn owned_path(&self, base: &Path, dir: &Path) -> bool {
        let Ok(rest) = dir.strip_prefix(base) else {
            return false;
        };
        let mut path = base.to_path_buf();
        if !self.owned_dir(&path, false) {
            return false;
        }
        for part in rest.components() {
            path.push(part);
            if !self.owned_dir(&path, false) {
                return false;
            }
        }
        true
    }

    /// A directory owned by the user; a symbolic link is followed only for
    /// a configured root (`follow`).
    fn owned_dir(&self, path: &Path, follow: bool) -> bool {
        let metadata = if follow {
            fs::metadata(path)
        } else {
            fs::symlink_metadata(path)
        };
        metadata.is_ok_and(|m| m.is_dir() && m.uid() == self.uid)
    }

    /// The regular files of a directory (not following symbolic links).
    fn list_dir(&self, dir: &Path) -> Vec<(String, PathBuf)> {
        self.list_entries(dir, false)
    }

    /// A directory's entries: its subdirectories (`dirs`) or its regular
    /// files, never symbolic links, at most [`MAX_DIR_ENTRIES`] read.
    fn list_entries(&self, dir: &Path, dirs: bool) -> Vec<(String, PathBuf)> {
        let Ok(read) = fs::read_dir(dir) else {
            return Vec::new();
        };
        read.take(MAX_DIR_ENTRIES)
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let kind = entry.file_type().ok()?;
                if (dirs && !kind.is_dir()) || (!dirs && !kind.is_file()) {
                    return None;
                }
                if dirs && entry.metadata().ok()?.uid() != self.uid {
                    return None;
                }
                Some((entry.file_name().into_string().ok()?, entry.path()))
            })
            .collect()
    }

    /// A user-owned regular file's modification time (epoch seconds).
    fn modified(&self, path: &Path) -> Option<u64> {
        let metadata = fs::symlink_metadata(path).ok()?;
        (metadata.is_file() && metadata.uid() == self.uid)
            .then(|| u64::try_from(metadata.mtime()).unwrap_or(0))
    }

    /// A transcript's facts, from the index when the file is the same one
    /// and either unchanged or already titled (its first prompt never
    /// changes), parsed otherwise.
    fn read(&mut self, path: &Path, kind: &'static str) -> Option<Found> {
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
            .ok()?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() || metadata.uid() != self.uid {
            return None;
        }
        let modified = u64::try_from(metadata.mtime()).unwrap_or(0);
        if let Some(cached) = self.files.get(path)
            && cached.dev == metadata.dev()
            && cached.ino == metadata.ino()
            && ((cached.size == metadata.size() && cached.mtime == metadata.mtime())
                || cached.meta.as_ref().is_some_and(|m| m.title.is_some()))
        {
            return cached.meta.clone().map(|meta| Found { meta, modified });
        }
        let meta = match kind {
            "claude" => {
                let id = path.file_stem()?.to_str()?;
                crate::session_id(id)
                    .then(|| parse_claude(file, id))
                    .flatten()
            }
            _ => parse_codex(file),
        };
        if self.files.len() >= INDEX_LIMIT {
            self.files.clear();
        }
        self.files.insert(
            path.to_path_buf(),
            Cached {
                dev: metadata.dev(),
                ino: metadata.ino(),
                size: metadata.size(),
                mtime: metadata.mtime(),
                meta: meta.clone(),
            },
        );
        meta.map(|meta| Found { meta, modified })
    }
}

#[derive(Debug, Clone)]
struct CodexFile {
    name: String,
    path: PathBuf,
    modified: u64,
}

/// One conversation of a closed entry, from KelpServer's `sessions` or,
/// before it has them, its `kind` and `session_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Conversation {
    kind: &'static str,
    id: String,
    cwd: String,
    first_seen: Option<u64>,
    last_seen: Option<u64>,
}

fn kind(value: &Value) -> Option<&'static str> {
    match value.as_str()? {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        _ => None,
    }
}

/// A closed entry's conversations, oldest first: its `sessions` when the
/// server records them, else its latest conversation.
fn conversations_of(entry: &Value) -> Vec<Conversation> {
    let cwd = entry["cwd"].as_str().unwrap_or_default();
    let mut list: Vec<Conversation> = match entry["sessions"].as_array() {
        Some(sessions) if !sessions.is_empty() => sessions
            .iter()
            .filter_map(|session| {
                Some(Conversation {
                    kind: kind(&session["kind"])?,
                    id: session["id"]
                        .as_str()
                        .filter(|id| crate::session_id(id))?
                        .into(),
                    cwd: session["cwd"].as_str().unwrap_or(cwd).into(),
                    first_seen: session["first_seen"].as_u64(),
                    last_seen: session["last_seen"].as_u64(),
                })
            })
            .collect(),
        _ => kind(&entry["kind"])
            .zip(
                entry["session_id"]
                    .as_str()
                    .filter(|id| crate::session_id(id)),
            )
            .map(|(kind, id)| Conversation {
                kind,
                id: id.into(),
                cwd: cwd.into(),
                first_seen: None,
                last_seen: None,
            })
            .into_iter()
            .collect(),
    };
    // A conversation resumed again later shows once, at its latest place.
    let mut seen = HashSet::new();
    list.reverse();
    list.retain(|c| seen.insert((c.kind, c.id.clone())));
    list.reverse();
    list
}

/// The folder names Claude Code may have used for `cwd`: every `/` and `.`
/// replaced by `-` (as documented), then every other character that is
/// not a letter or digit too (newer versions).
fn claude_dir_names(cwd: &str) -> Vec<String> {
    let documented = cwd.replace(['/', '.'], "-");
    let strict = normalize(cwd);
    if strict == documented {
        vec![documented]
    } else {
        vec![documented, strict]
    }
}

/// Every character that is not an ASCII letter or digit replaced by `-`.
fn normalize(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// `cwd` is `folder` or inside it, by whole path components.
fn inside(cwd: Option<&str>, folder: &Path) -> bool {
    cwd.is_some_and(|cwd| {
        let cwd = Path::new(cwd);
        cwd.is_absolute() && cwd.starts_with(folder)
    })
}

fn is_folder(path: &str) -> bool {
    !path.is_empty() && Path::new(path).is_absolute() && Path::new(path).is_dir()
}

/// `<sessions>/YYYY/MM/DD` of a day counted from the Unix epoch.
fn date_dir(sessions: &Path, day: u64) -> PathBuf {
    let (y, m, d) = civil(day as i64);
    sessions
        .join(format!("{y:04}"))
        .join(format!("{m:02}"))
        .join(format!("{d:02}"))
}

/// The calendar date of a day counted from 1970-01-01 (Howard Hinnant's
/// `civil_from_days`).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Days from 1970-01-01 to a calendar date (`days_from_civil`).
fn days(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Epoch seconds of an RFC 3339 time (`2026-11-24T10:02:03.123Z`, or with
/// an offset); `None` for anything else.
fn timestamp(value: &Value) -> Option<u64> {
    let text = value.as_str()?;
    let bytes = text.as_bytes();
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
    {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = text.get(range)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse().ok())
            .flatten()
    };
    let (y, mo, d) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (h, mi, s) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    let mut rest = &text[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        rest = &fraction[digits..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ if rest.len() == 6
            && matches!(rest.as_bytes()[0], b'+' | b'-')
            && rest.as_bytes()[3] == b':' =>
        {
            let hours: i64 = rest[1..3].parse().ok()?;
            let minutes: i64 = rest[4..6].parse().ok()?;
            let sign = if rest.starts_with('-') { -1 } else { 1 };
            sign * (hours * 3600 + minutes * 60)
        }
        _ => return None,
    };
    let seconds = days(y, mo as u32, d as u32) * 86_400 + h * 3600 + mi * 60 + s - offset;
    u64::try_from(seconds).ok()
}

/// A one-line title: whitespace runs as one space, control and format
/// characters dropped, at most [`TITLE_CHARS`] characters (the last an
/// ellipsis when cut); `None` when nothing is left.
pub fn title(text: &str) -> Option<String> {
    let mut words = String::new();
    for word in text.split_whitespace() {
        let word: String = word
            .chars()
            .filter(|c| !c.is_control() && !matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}'))
            .collect();
        if word.is_empty() {
            continue;
        }
        if !words.is_empty() {
            words.push(' ');
        }
        words.push_str(&word);
        if words.chars().count() > TITLE_CHARS {
            break;
        }
    }
    if words.is_empty() {
        return None;
    }
    if words.chars().count() > TITLE_CHARS {
        let mut cut: String = words.chars().take(TITLE_CHARS - 1).collect();
        cut.truncate(cut.trim_end().len());
        cut.push('…');
        return Some(cut);
    }
    Some(words)
}

/// Calls `line` with each line of a transcript (without its newline),
/// skipping lines over [`LINE_LIMIT`] bytes unread, until it returns true
/// or [`READ_LIMIT`] bytes or [`LINE_COUNT`] lines were read.
fn each_line(file: fs::File, mut line: impl FnMut(&[u8]) -> bool) {
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut current = Vec::new();
    let mut skipping = false;
    let mut read = 0u64;
    let mut lines = 0usize;
    loop {
        let Ok(buffer) = reader.fill_buf() else {
            return;
        };
        if buffer.is_empty() {
            if !skipping && !current.is_empty() {
                line(&current);
            }
            return;
        }
        let (used, end) = match buffer.iter().position(|&b| b == b'\n') {
            Some(at) => (at + 1, Some(at)),
            None => (buffer.len(), None),
        };
        let part = &buffer[..end.unwrap_or(used)];
        if !skipping {
            if current.len() + part.len() > LINE_LIMIT {
                skipping = true;
                current.clear();
            } else {
                current.extend_from_slice(part);
            }
        }
        reader.consume(used);
        read += used as u64;
        if end.is_some() {
            lines += 1;
            if !skipping && line(&current) {
                return;
            }
            current.clear();
            skipping = false;
        }
        if read >= READ_LIMIT || lines >= LINE_COUNT {
            return;
        }
    }
}

/// Text that is not the user's own words: Claude Code's command, local
/// output and caveat records, and markup the tools inject (`<tag>…`).
fn injected(text: &str) -> bool {
    let text = text.trim_start();
    if text.starts_with("Caveat:") {
        return true;
    }
    let Some(rest) = text.strip_prefix('<') else {
        return false;
    };
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    !name.is_empty() && rest[name.len()..].starts_with('>')
}

/// A Claude Code transcript: the first user record whose content is the
/// user's own text (not a tool result, meta or command record).
fn parse_claude(file: fs::File, id: &str) -> Option<Meta> {
    let mut meta = Meta {
        kind: "claude",
        id: id.into(),
        cwd: None,
        title: None,
        first: None,
        sub: false,
    };
    each_line(file, |line| {
        let Ok(record) = serde_json::from_slice::<Value>(line) else {
            return false;
        };
        if meta.cwd.is_none()
            && let Some(cwd) = record["cwd"].as_str()
        {
            meta.cwd = Some(cwd.into());
        }
        if record["type"] != "user" {
            return false;
        }
        if record["isSidechain"] == true {
            meta.sub = true;
            return true;
        }
        if record["isMeta"] == true || record["message"]["role"] != "user" {
            return false;
        }
        let text = match &record["message"]["content"] {
            Value::String(text) => text.clone(),
            Value::Array(items) => {
                if items.iter().any(|item| item["type"] == "tool_result") {
                    return false;
                }
                items
                    .iter()
                    .filter(|item| item["type"] == "text")
                    .filter_map(|item| item["text"].as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            _ => return false,
        };
        if injected(&text) {
            return false;
        }
        let Some(title) = title(&text) else {
            return false;
        };
        meta.title = Some(title);
        meta.first = timestamp(&record["timestamp"]);
        if let Some(cwd) = record["cwd"].as_str() {
            meta.cwd = Some(cwd.into());
        }
        true
    });
    Some(meta)
}

/// A Codex rollout: its `session_meta` first line (id, folder, whether a
/// subagent wrote it), then its first `user_message` event (or, failing
/// that, the first user message that is not injected instructions).
fn parse_codex(file: fs::File) -> Option<Meta> {
    let mut meta: Option<Meta> = None;
    let mut fallback: Option<String> = None;
    let mut first_line = true;
    each_line(file, |line| {
        let record = serde_json::from_slice::<Value>(line).ok();
        if first_line {
            first_line = false;
            let Some(record) = record.filter(|r| r["type"] == "session_meta") else {
                return true;
            };
            let payload = &record["payload"];
            let Some(id) = payload["id"].as_str().filter(|id| crate::session_id(id)) else {
                return true;
            };
            let sub = payload["parent_thread_id"]
                .as_str()
                .is_some_and(|parent| !parent.is_empty())
                || payload["source"].is_object();
            meta = Some(Meta {
                kind: "codex",
                id: id.into(),
                cwd: payload["cwd"].as_str().map(Into::into),
                title: None,
                first: timestamp(&payload["timestamp"]).or_else(|| timestamp(&record["timestamp"])),
                sub,
            });
            return sub;
        }
        let Some(record) = record else {
            return false;
        };
        let payload = &record["payload"];
        if record["type"] == "event_msg" && payload["type"] == "user_message" {
            if let Some(found) = payload["message"].as_str().and_then(title)
                && let Some(meta) = meta.as_mut()
            {
                meta.title = Some(found);
                return true;
            }
            return false;
        }
        if fallback.is_none()
            && record["type"] == "response_item"
            && payload["type"] == "message"
            && payload["role"] == "user"
            && let Some(items) = payload["content"].as_array()
        {
            let text = items
                .iter()
                .filter_map(|item| item["text"].as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let trimmed = text.trim_start();
            if !injected(trimmed) && !trimmed.starts_with('<') && !trimmed.starts_with('#') {
                fallback = title(&text);
            }
        }
        false
    });
    let mut meta = meta?;
    if meta.title.is_none() {
        meta.title = fallback;
    }
    Some(meta)
}

/// Answers a listing later, on the History thread.
pub type Reply = Box<dyn FnOnce(std::result::Result<Value, String>) + Send>;

struct Job {
    request: Request,
    queued: Instant,
    reply: Reply,
}

/// The bridge's History thread, with its index.
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
        let (jobs, queue) = mpsc::sync_channel::<Job>(2);
        let index = Arc::new(Mutex::new(Index::default()));
        thread::spawn(move || {
            while let Ok(job) = queue.recv() {
                if job.queued.elapsed() > STALE {
                    (job.reply)(Err(BUSY.into()));
                    continue;
                }
                let Ok(mut index) = index.lock() else {
                    return;
                };
                let listing = index.list(&Roots::current(), &job.request, SystemTime::now());
                drop(index);
                (job.reply)(Ok(listing));
            }
        });
        Self { jobs }
    }
    /// Queues a listing; `reply` receives it on the History thread.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "kelpmux-history-{}-{}",
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

    fn write(path: &Path, lines: &[String]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = fs::File::create(path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn uid() -> u32 {
        unsafe { libc::geteuid() }
    }

    struct World {
        temp: Temp,
        roots: Roots,
        alpha: PathBuf,
        scratch: PathBuf,
    }
    impl World {
        fn new() -> Self {
            let temp = Temp::new();
            let alpha = temp.0.join("work/alpha.app");
            let scratch = temp.0.join("scratch");
            fs::create_dir_all(alpha.join("repo")).unwrap();
            fs::create_dir_all(&scratch).unwrap();
            let roots = Roots {
                claude: Some(temp.0.join("claude")),
                codex: Some(temp.0.join("codex")),
            };
            Self {
                temp,
                roots,
                alpha,
                scratch,
            }
        }
        fn claude(&self, cwd: &Path, id: &str, lines: &[String]) -> PathBuf {
            let dir = cwd.to_string_lossy().replace(['/', '.'], "-");
            let path = self
                .roots
                .claude
                .as_ref()
                .unwrap()
                .join("projects")
                .join(dir)
                .join(format!("{id}.jsonl"));
            write(&path, lines);
            path
        }
        fn codex(&self, id: &str, lines: &[String]) -> PathBuf {
            let (y, m, d) = civil((crate::now() / 86_400) as i64);
            let path = self
                .roots
                .codex
                .as_ref()
                .unwrap()
                .join(format!("sessions/{y:04}/{m:02}/{d:02}"))
                .join(format!("rollout-2026-11-24T10-00-00-{id}.jsonl"));
            write(&path, lines);
            path
        }
        fn request(&self, entries: Vec<Value>) -> Request {
            Request {
                workspace: "main".into(),
                entries,
                live: vec![],
                scopes: vec![
                    Scope {
                        id: "alpha".into(),
                        folder: self.alpha.clone(),
                    },
                    Scope {
                        id: "scratch".into(),
                        folder: self.scratch.clone(),
                    },
                ],
            }
        }
    }

    fn claude_user(cwd: &Path, text: &str) -> String {
        json!({"type": "user", "isSidechain": false, "cwd": cwd, "timestamp": "2026-11-24T10:00:00.000Z",
            "message": {"role": "user", "content": text}})
        .to_string()
    }
    fn codex_meta(id: &str, cwd: &Path, sub: bool) -> String {
        let mut payload = json!({"id": id, "cwd": cwd, "timestamp": "2026-11-24T09:00:00Z",
            "base_instructions": {"text": "x".repeat(50_000)}, "source": "cli", "parent_thread_id": null});
        if sub {
            payload["source"] = json!({"subagent": "review"});
            payload["parent_thread_id"] = json!("parent");
        }
        json!({"type": "session_meta", "timestamp": "2026-11-24T09:00:00Z", "payload": payload})
            .to_string()
    }
    fn codex_user(text: &str) -> Vec<String> {
        vec![
            json!({"type": "response_item", "payload": {"type": "message", "role": "user",
                "content": [{"type": "input_text", "text": "# AGENTS.md instructions"}]}})
            .to_string(),
            json!({"type": "event_msg", "payload": {"type": "user_message", "message": text}})
                .to_string(),
        ]
    }

    fn closed(key: &str, cwd: &Path, kind: Option<&str>, id: Option<&str>) -> Value {
        json!({"key": key, "workspace": "main", "pane": 1, "cwd": cwd, "name": "agent tab",
            "kind": kind, "session_id": id, "argv": [], "closed_at": 100, "reason": "Closed",
            "choices": ["resume"], "project": "alpha"})
    }

    #[test]
    fn titles_are_one_bounded_line() {
        assert_eq!(title("  fix\nthe\t\tbug  ").as_deref(), Some("fix the bug"));
        assert_eq!(title(" \n\u{7}\t"), None);
        let long = title(&"word ".repeat(100)).unwrap();
        assert_eq!(long.chars().count(), TITLE_CHARS);
        assert!(long.ends_with('…'));
        assert_eq!(title("a\u{202e}b\u{1b}[31m").as_deref(), Some("ab[31m"));
    }

    #[test]
    fn timestamps_parse_rfc3339_only() {
        assert_eq!(timestamp(&json!("1970-01-02T00:00:00Z")), Some(86_400));
        assert_eq!(
            timestamp(&json!("2026-11-24T10:00:00.123Z")),
            Some(1_795_514_400)
        );
        assert_eq!(
            timestamp(&json!("2026-11-24T10:00:00+01:00")),
            Some(1_795_510_800)
        );
        assert_eq!(timestamp(&json!("yesterday")), None);
        assert_eq!(timestamp(&json!(12)), None);
        assert_eq!(civil(days(2024, 2, 29)), (2024, 2, 29));
    }

    #[test]
    fn closed_entries_get_their_conversations_with_titles() {
        let world = World::new();
        let repo = world.alpha.join("repo");
        world.claude(
            &repo,
            "c-1",
            &[
                json!({"type": "mode", "sessionId": "c-1"}).to_string(),
                "{not json".into(),
                json!({"type": "user", "isMeta": true, "message": {"role": "user", "content": "meta"}}).to_string(),
                json!({"type": "user", "message": {"role": "user", "content": "<command-name>/clear</command-name>"}}).to_string(),
                json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "content": "secret"}]}}).to_string(),
                claude_user(&repo, "Refactor the\nparser please"),
                claude_user(&repo, "second prompt stays private"),
            ],
        );
        let mut lines = vec![codex_meta("x-2", &repo, false)];
        lines.extend(codex_user("Write the release notes"));
        world.codex("x-2", &lines);
        let mut entry = closed("e:1", &repo, Some("codex"), Some("x-2"));
        entry["sessions"] = json!([
            {"kind": "claude", "id": "c-1", "cwd": repo, "first_seen": 10, "last_seen": 20},
            {"kind": "codex", "id": "x-2", "cwd": "/gone/folder", "first_seen": 30, "last_seen": 40},
            {"kind": "claude", "id": "bad id/..", "cwd": repo, "first_seen": 50, "last_seen": 60},
        ]);
        let mut index = Index::default();
        let listing = index.list(&world.roots, &world.request(vec![entry]), SystemTime::now());
        let conversations = listing["entries"][0]["conversations"].as_array().unwrap();
        assert_eq!(conversations.len(), 2, "{conversations:?}");
        // Newest first; a missing folder falls back to the terminal's.
        assert_eq!(conversations[0]["id"], "x-2");
        assert_eq!(conversations[0]["title"], "Write the release notes");
        assert_eq!(conversations[0]["folder"], json!(repo));
        assert_eq!(conversations[0]["first_seen"], 30);
        assert_eq!(conversations[1]["title"], "Refactor the parser please");
        assert_eq!(conversations[1]["titled"], true);
        // Nothing but the title leaves a transcript.
        let text = listing.to_string();
        assert!(!text.contains("secret") && !text.contains("private") && !text.contains("AGENTS"));
        // Linked conversations are not outside ones.
        assert_eq!(listing["external"], json!([]));
    }

    #[test]
    fn without_sessions_the_latest_conversation_is_used_and_names_fall_back() {
        let world = World::new();
        let entry = closed("e:1", &world.alpha, Some("claude"), Some("missing"));
        let shell = closed("e:2", &world.alpha, None, None);
        let mut index = Index::default();
        let listing = index.list(
            &world.roots,
            &world.request(vec![entry, shell]),
            SystemTime::now(),
        );
        let conversations = &listing["entries"][0]["conversations"];
        assert_eq!(conversations.as_array().unwrap().len(), 1);
        assert_eq!(conversations[0]["id"], "missing");
        assert_eq!(conversations[0]["title"], "agent tab");
        assert_eq!(conversations[0]["titled"], false);
        assert_eq!(listing["entries"][1]["conversations"], json!([]));
    }

    #[test]
    fn outside_conversations_are_matched_by_folder() {
        let world = World::new();
        let repo = world.alpha.join("repo");
        world.claude(&repo, "c-out", &[claude_user(&repo, "Outside claude")]);
        world.claude(
            &world.scratch,
            "c-scr",
            &[claude_user(&world.scratch, "Scratch idea")],
        );
        // A sibling folder whose encoded name shares the prefix.
        let sibling = world.temp.0.join("work/alpha.app-two");
        fs::create_dir_all(&sibling).unwrap();
        world.claude(&sibling, "c-sib", &[claude_user(&sibling, "Not alpha")]);
        // A side chain never lists on its own.
        world.claude(
            &repo,
            "c-side",
            &[json!({"type": "user", "isSidechain": true, "cwd": repo,
                "message": {"role": "user", "content": "sub"}})
            .to_string()],
        );
        let mut lines = vec![codex_meta("x-out", &repo, false)];
        lines.extend(codex_user("Outside codex"));
        world.codex("x-out", &lines);
        let mut sub = vec![codex_meta("x-sub", &repo, true)];
        sub.extend(codex_user("subagent"));
        world.codex("x-sub", &sub);
        world.codex("x-bad", &["{\"type\":\"turn_context\"}".into()]);
        let mut request = world.request(vec![]);
        request.live.push(("claude".into(), "c-live".into()));
        world.claude(&repo, "c-live", &[claude_user(&repo, "Live one")]);
        let mut index = Index::default();
        let listing = index.list(&world.roots, &request, SystemTime::now());
        let mut rows: Vec<(String, String, String)> = listing["external"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["id"].as_str().unwrap().into(),
                    row["project"].as_str().unwrap().into(),
                    row["title"].as_str().unwrap_or("").into(),
                )
            })
            .collect();
        rows.sort();
        assert_eq!(
            rows,
            [
                ("c-out".into(), "alpha".into(), "Outside claude".into()),
                ("c-scr".into(), "scratch".into(), "Scratch idea".into()),
                ("x-out".into(), "alpha".into(), "Outside codex".into()),
            ]
        );
    }

    #[test]
    fn hostile_files_are_skipped() {
        let world = World::new();
        let repo = world.alpha.join("repo");
        // A symbolic link to a transcript, and a symlinked project folder.
        let real = world.claude(
            &world.temp.0.join("elsewhere"),
            "c-real",
            &[claude_user(&repo, "Linked")],
        );
        let dir = world
            .roots
            .claude
            .as_ref()
            .unwrap()
            .join("projects")
            .join(repo.to_string_lossy().replace(['/', '.'], "-"));
        fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(&real, dir.join("c-link.jsonl")).unwrap();
        let linked_dir = world
            .roots
            .claude
            .as_ref()
            .unwrap()
            .join("projects")
            .join(world.scratch.to_string_lossy().replace(['/', '.'], "-"));
        std::os::unix::fs::symlink(real.parent().unwrap(), &linked_dir).unwrap();
        // A huge first line, then the prompt.
        world.claude(
            &repo,
            "c-huge",
            &[
                format!(
                    "{{\"type\":\"user\",\"pad\":\"{}\"}}",
                    "y".repeat(LINE_LIMIT + 10)
                ),
                claude_user(&repo, "After the huge line"),
            ],
        );
        // A FIFO named like a transcript must not block.
        let fifo = dir.join("c-fifo.jsonl");
        let name = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let mut index = Index::default();
        let listing = index.list(&world.roots, &world.request(vec![]), SystemTime::now());
        let ids: Vec<&str> = listing["external"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["c-huge"]);
        assert_eq!(listing["external"][0]["title"], "After the huge line");
        // Files another user owns are ignored.
        let mut foreign = Index::for_user(uid().wrapping_add(1));
        let listing = foreign.list(&world.roots, &world.request(vec![]), SystemTime::now());
        assert_eq!(listing["external"], json!([]));
    }

    #[test]
    fn outside_conversations_are_bounded() {
        let world = World::new();
        for n in 0..PER_PROJECT + 5 {
            world.claude(
                &world.alpha,
                &format!("c-{n}"),
                &[claude_user(&world.alpha, "hi")],
            );
        }
        let old = world.claude(
            &world.scratch,
            "c-old",
            &[claude_user(&world.scratch, "old")],
        );
        let ancient = SystemTime::now() - Duration::from_secs((DAYS + 2) * 86_400);
        fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(ancient)
            .unwrap();
        let mut index = Index::default();
        let listing = index.list(&world.roots, &world.request(vec![]), SystemTime::now());
        assert_eq!(listing["external"].as_array().unwrap().len(), PER_PROJECT);
        assert_eq!(listing["truncated"], true);
        // Cached: a second listing reads the same.
        let again = index.list(&world.roots, &world.request(vec![]), SystemTime::now());
        assert_eq!(again["external"].as_array().unwrap().len(), PER_PROJECT);
    }

    #[test]
    fn index_reparses_untitled_changed_files_only() {
        let world = World::new();
        let path = world.claude(&world.alpha, "c-1", &[json!({"type": "mode"}).to_string()]);
        let mut index = Index::default();
        assert_eq!(index.read(&path, "claude").unwrap().meta.title, None);
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{}", claude_user(&world.alpha, "Now titled")).unwrap();
        drop(file);
        assert_eq!(
            index.read(&path, "claude").unwrap().meta.title.as_deref(),
            Some("Now titled")
        );
    }

    #[test]
    fn conversations_dedupe_and_fall_back() {
        let entry = json!({"cwd": "/w", "kind": "claude", "session_id": "a",
        "sessions": [
            {"kind": "claude", "id": "a", "cwd": "/w/x", "first_seen": 1, "last_seen": 2},
            {"kind": "codex", "id": "b", "first_seen": 3, "last_seen": 4},
            {"kind": "claude", "id": "a", "cwd": "/w/y", "first_seen": 5, "last_seen": 6},
            {"kind": "other", "id": "c"},
        ]});
        let list = conversations_of(&entry);
        assert_eq!(
            list.iter()
                .map(|c| (c.id.as_str(), c.cwd.as_str()))
                .collect::<Vec<_>>(),
            [("b", "/w"), ("a", "/w/y")]
        );
        assert!(conversations_of(&json!({"kind": "claude", "session_id": null})).is_empty());
        assert_eq!(claude_dir_names("/a/b.c_d"), ["-a-b-c_d", "-a-b-c-d"]);
    }
}
