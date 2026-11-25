//! The user configuration file, `$XDG_CONFIG_HOME/kelpmux/config.toml`
//! (`~/.config/kelpmux/config.toml` by default): the single place for a
//! user's KelpMux settings, edited by hand, by agents or by the web
//! client's Settings, and applied live by every web bridge.
//!
//! [`KEYS`] is the one definition of the file's keys: the validator, the
//! commented default file ([`template`]), the JSON Schema ([`schema`]) and
//! the reference in `docs/config.md` ([`reference`]) are all derived from
//! it. `kelpmux config check`, the bridge's reload and its saves share
//! [`validate`].
//!
//! Writers (several bridges, one per browser connection, and `kelpmux
//! config path`) take `flock` on a lock file beside the file, write a
//! temporary file in the same folder, fsync it and rename it over the file.
//! Saves edit the document with `toml_edit`, keeping the user's comments
//! and formatting.
use crate::Result;
use serde_json::{Value as Json, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    ops::Range,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use toml_edit::{Array, DocumentMut, Item, TableLike, Value};

/// The schema version this KelpMux reads (`version`).
pub const VERSION: i64 = 1;
/// Largest accepted file.
const MAX_BYTES: u64 = 64 * 1024;
/// Most `files.extra_roots`.
pub const MAX_EXTRA_ROOTS: usize = 16;
/// Longest accepted path, in bytes.
const MAX_PATH: usize = 4096;
/// The file's name in its folder.
pub const FILE_NAME: &str = "config.toml";
/// Where the reference lives, written at the top of the default file.
const REFERENCE_URL: &str = "https://github.com/joanfabregat/kelpmux/blob/main/docs/config.md";

/// The type of a key's value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// The schema version, an integer.
    Version,
    /// A folder: an absolute path, or one starting with `~/`.
    Folder,
    /// A list of folders.
    FolderList,
    /// A range of TCP ports, `"FIRST-LAST"`.
    PortRange,
    /// A string, checked by its key; empty leaves it unset.
    Text,
    /// A file path: absolute or starting with `~/`; empty leaves it unset.
    File,
}
impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Version => "integer",
            Self::Folder => "folder path (string)",
            Self::FolderList => "list of folder paths (strings)",
            Self::PortRange => "port range (string, `FIRST-LAST`)",
            Self::Text => "string",
            Self::File => "file path (string)",
        }
    }
}

/// One key of the file.
#[derive(Debug, PartialEq, Eq)]
pub struct Key {
    /// The table the key belongs to; empty for a top-level key.
    pub section: &'static str,
    pub name: &'static str,
    pub kind: Kind,
    /// The default, as a TOML value.
    pub default: &'static str,
    /// An example value, as a TOML value, when the default shows too little.
    pub example: Option<&'static str>,
    /// What the key does (one paragraph).
    pub doc: &'static str,
    /// When a change takes effect.
    pub applies: &'static str,
}
impl Key {
    /// The dotted name, as errors and the web client name it.
    pub fn path(&self) -> String {
        if self.section.is_empty() {
            self.name.to_owned()
        } else {
            format!("{}.{}", self.section, self.name)
        }
    }
}

/// The tables of the file, in order, with what they configure.
pub const SECTIONS: &[(&str, &str)] = &[
    ("files", "The web client's file browser."),
    ("projects", "Projects."),
    ("scratch", "Terminals outside any project."),
    (
        "environments",
        "Project environments: dev servers and deployments declared in a project's repositories (`.kelp/environments/`), started only on request from the web client or `kelpmux env`.",
    ),
    (
        "github",
        "GitHub in the web client: the repositories your GitHub account can see (Add repository on a project's Repos view), read-only facts about the GitHub repositories a project's checkouts come from, and cloning. The bridge reads the token this machine already has (the GitHub CLI, an environment variable or a command) and keeps it in memory only, for a few minutes; it never goes to the browser, a file or a log. Clones use your own Git credentials (SSH agent or credential helper). These keys are edited here only, never from the web client.",
    ),
];

/// Every key of the file. The validator, the default file, the JSON Schema
/// and `docs/config.md` are generated from this list.
pub const KEYS: &[Key] = &[
    Key {
        section: "",
        name: "version",
        kind: Kind::Version,
        default: "1",
        example: None,
        doc: "Schema version of this file. KelpMux reads version 1 and refuses a file written for a newer version rather than misreading it.",
        applies: "Immediately.",
    },
    Key {
        section: "files",
        name: "root",
        kind: Kind::Folder,
        default: "\"~\"",
        example: None,
        doc: "The folder the web file browser is confined to: KelpServer refuses every file operation, download and upload destination outside it and outside `files.extra_roots`. It must be an existing folder. This limits the file browser, not you: a shell in a terminal can still reach anything your account can.",
        applies: "On the next workspace start: KelpServer reads its file root when it starts (`kelpmux start` records it in the workspace's `workspace.json` as `files_root`).",
    },
    Key {
        section: "files",
        name: "extra_roots",
        kind: Kind::FolderList,
        default: "[]",
        example: Some("[\"/mnt/data\"]"),
        doc: "Folders outside `files.root` that the file browser may also open, such as a data drive. Each must be an existing folder that is not inside `files.root`; at most 16, without duplicates.",
        applies: "On the next workspace start (recorded as `files_extra_roots` in `workspace.json`), with a KelpServer that supports extra roots; an older server ignores them.",
    },
    Key {
        section: "projects",
        name: "base",
        kind: Kind::Folder,
        default: "\"~/projects\"",
        example: None,
        doc: "The default folder for new projects: a project named `name` goes in `<base>/<name>`. It must lie inside `files.root` or one of `files.extra_roots`, and need not exist yet.",
        applies: "Immediately: the web client offers `<base>/<name>` for the next project it creates (and creates that folder).",
    },
    Key {
        section: "scratch",
        name: "folder",
        kind: Kind::Folder,
        default: "\"~/scratch\"",
        example: None,
        doc: "The folder of the built-in Scratch project: terminals created outside any project start in it and belong to Scratch. It must lie inside `files.root` or one of `files.extra_roots`, and need not exist yet.",
        applies: "Immediately: the next terminal created outside a project starts there (the web client creates the folder when it is missing).",
    },
    Key {
        section: "environments",
        name: "ports",
        kind: Kind::PortRange,
        default: "\"41000-41999\"",
        example: None,
        doc: "The TCP ports given to environments as `KELP_PORT`: each (project, environment) gets the lowest port of this range that no other one holds and nothing listens on, and keeps it across restarts while it stays in the range. Two port numbers from 1024 to 65535, the first not above the second.",
        applies: "On the next start of an environment: one whose port falls outside a new range gets a new port.",
    },
    Key {
        section: "github",
        name: "auth",
        kind: Kind::Text,
        default: "\"\"",
        example: Some("\"gh\""),
        doc: "Where the GitHub token comes from: `\"gh\"` runs `gh auth token` (the GitHub CLI's sign-in), `\"env\"` reads `GH_TOKEN`, then `GITHUB_TOKEN`, from the web bridge's environment. Empty, with `github.token_command` empty too, leaves GitHub signed out: the repository list and the facts on the Repos view are off, cloning a URL still works. Set this or `github.token_command`, not both.",
        applies: "Immediately: the next GitHub request reads the token again.",
    },
    Key {
        section: "github",
        name: "token_command",
        kind: Kind::File,
        default: "\"\"",
        example: Some("\"~/bin/github-token\""),
        doc: "A program that prints the GitHub token on its first line of output, run without arguments or a shell (for example a script that decrypts a SOPS-encrypted token). It must finish within 30 seconds and print at most 8 KiB; what it writes to its error output is never shown. Set this or `github.auth`, not both.",
        applies: "Immediately: the next GitHub request runs it again.",
    },
    Key {
        section: "github",
        name: "ssh_auth_sock",
        kind: Kind::File,
        default: "\"\"",
        example: Some("\"~/.ssh/agent.sock\""),
        doc: "The SSH agent socket Git uses to clone over SSH (`SSH_AUTH_SOCK`). Empty keeps the web bridge's own `SSH_AUTH_SOCK`, which a bridge started by the web relay's SSH connection usually lacks: point this at an agent socket that outlives your sessions for SSH clones to work from the web client.",
        applies: "Immediately: the next clone uses it.",
    },
];

fn key(path: &str) -> Option<&'static Key> {
    KEYS.iter().find(|key| key.path() == path)
}

/// A folder as written in the file and as resolved (`~` expanded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub written: String,
    pub path: PathBuf,
    span: Option<Range<usize>>,
}

/// A project (`[[project]]`): a name and a folder. Its `id` is the stable
/// name terminals and closed terminals are linked by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub folder: Folder,
    pub color: Option<String>,
}

/// The id of the built-in Scratch project, which no `[[project]]` may take.
pub const SCRATCH: &str = "scratch";
/// Most `[[project]]` tables.
pub const MAX_PROJECTS: usize = 256;
/// Longest project name, in characters, and id, in bytes.
pub const MAX_NAME: usize = 64;
/// The named project colours; `#rrggbb` is accepted too.
pub const COLORS: &[&str] = &[
    "red", "orange", "yellow", "green", "teal", "blue", "purple", "pink", "gray",
];

/// One field of a `[[project]]` table.
pub struct ProjectField {
    pub name: &'static str,
    pub kind: &'static str,
    pub required: bool,
    pub example: &'static str,
    pub doc: &'static str,
}
/// The fields of a `[[project]]` table, for the validator, the default
/// file, the schema and the reference.
pub const PROJECT_FIELDS: &[ProjectField] = &[
    ProjectField {
        name: "id",
        kind: "string",
        required: true,
        example: "\"kelp\"",
        doc: "A stable identifier: lowercase letters, digits and `-`, at most 64, starting with a letter or digit; unique, and not `scratch`. Terminals are linked to their project by it, so keep it when renaming. The web client makes one from the name.",
    },
    ProjectField {
        name: "name",
        kind: "string",
        required: true,
        example: "\"Kelp\"",
        doc: "The name shown: at most 64 characters, without `/` or control characters, unique regardless of case, and not `Scratch`.",
    },
    ProjectField {
        name: "folder",
        kind: "folder path (string)",
        required: true,
        example: "\"~/projects/Kelp\"",
        doc: "The project's folder. It must lie inside `files.root` or one of `files.extra_roots`, and may not be inside, or contain, another project's folder or `scratch.folder`. It need not exist: a project whose folder is missing shows as missing. Deleting a project never deletes its folder.",
    },
    ProjectField {
        name: "color",
        kind: "string",
        required: false,
        example: "\"teal\"",
        doc: "Optional: the colour that marks the project in the sidebar and the file browser, one of red, orange, yellow, green, teal, blue, purple, pink, gray, or `#rrggbb`.",
    },
];
/// What `[[project]]` is, for the default file and the reference.
const PROJECTS_DOC: &str = "Projects: one `[[project]]` table each, with a name and a folder. Every terminal belongs to one project; terminals created outside any project belong to the built-in Scratch project (`scratch.folder`), which is not listed here. The web client adds, renames and deletes projects here too.";

/// A valid configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub files_root: Folder,
    pub files_extra_roots: Vec<Folder>,
    pub projects_base: Folder,
    pub scratch_folder: Folder,
    /// The `[[project]]` tables, in the file's order.
    pub projects: Vec<Project>,
    /// `environments.ports`: as written, and the first and last port.
    pub environment_ports: (String, (u16, u16)),
    /// `[github]`.
    pub github: GitHubSettings,
}

/// Parses `"FIRST-LAST"`: ports from 1024 to 65535, FIRST ≤ LAST.
pub fn port_range(text: &str) -> std::result::Result<(u16, u16), String> {
    let (first, last) = text
        .split_once('-')
        .ok_or("must be two ports joined by `-`, such as \"41000-41999\"")?;
    let port = |text: &str| {
        text.trim()
            .parse::<u16>()
            .ok()
            .filter(|port| *port >= 1024)
            .ok_or("ports must be numbers from 1024 to 65535")
    };
    let (first, last) = (port(first)?, port(last)?);
    if first > last {
        return Err("the first port must not be above the last".into());
    }
    Ok((first, last))
}

/// Where the GitHub token comes from (`github.auth`, `github.token_command`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHubAuth {
    /// `gh auth token`.
    Gh,
    /// `GH_TOKEN`, then `GITHUB_TOKEN`.
    Env,
    /// This program's first line of output.
    Command(PathBuf),
}

/// The `[github]` table, checked. Empty strings are `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitHubSettings {
    /// `None`: signed out.
    pub auth: Option<GitHubAuth>,
    pub ssh_auth_sock: Option<PathBuf>,
}

/// A GitHub login or repository name as GitHub allows them: 1–100 ASCII
/// letters, digits, `-`, `_` or `.`, and not `.` or `..`.
pub fn github_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}
impl Config {
    /// The project `id` names, if any.
    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|project| project.id == id)
    }
    /// The values as written, and resolved, in the file's shape, with the
    /// projects (`projects`: each with its folder as written and resolved,
    /// and whether that folder is missing, checked now).
    pub fn to_json(&self) -> Json {
        let mut value = self.values_json();
        value["projects"] = Json::Array(
            self.projects
                .iter()
                .map(|project| {
                    json!({
                        "id": project.id,
                        "name": project.name,
                        "folder": project.folder.written,
                        "path": project.folder.path,
                        "color": project.color,
                        "missing": !project.folder.path.is_dir(),
                    })
                })
                .collect(),
        );
        value
    }
    fn values_json(&self) -> Json {
        let shape = |pick: &dyn Fn(&Folder) -> Json| {
            json!({
                "files": {
                    "root": pick(&self.files_root),
                    "extra_roots": self.files_extra_roots.iter().map(pick).collect::<Vec<_>>(),
                },
                "projects": { "base": pick(&self.projects_base) },
                "scratch": { "folder": pick(&self.scratch_folder) },
            })
        };
        let ports = &self.environment_ports;
        let (mut values, mut resolved) = (
            shape(&|folder| json!(folder.written)),
            shape(&|folder| json!(folder.path)),
        );
        values["environments"] = json!({ "ports": ports.0 });
        resolved["environments"] = json!({ "ports": [ports.1.0, ports.1.1] });
        json!({ "values": values, "resolved": resolved })
    }
    /// The file root and extra roots KelpServer is given.
    pub fn roots(&self) -> (PathBuf, Vec<PathBuf>) {
        (
            self.files_root.path.clone(),
            self.files_extra_roots
                .iter()
                .map(|folder| folder.path.clone())
                .collect(),
        )
    }
}

/// A problem in the file, at a 1-based line and column (in characters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The key it concerns (`files.root`), when it concerns one.
    pub key: Option<String>,
    pub line: usize,
    pub column: usize,
    pub message: String,
}
impl Diagnostic {
    fn new(text: &str, span: Option<Range<usize>>, key: Option<String>, message: String) -> Self {
        let offset = span.map_or(0, |span| span.start).min(text.len());
        let before = text.get(..offset).unwrap_or(text);
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        Self {
            key,
            line,
            column,
            message,
        }
    }
    /// `path:line:col: message`, as `kelpmux config check` prints it.
    pub fn display(&self, path: &Path) -> String {
        format!(
            "{}:{}:{}: {}",
            path.display(),
            self.line,
            self.column,
            self.message
        )
    }
    pub fn to_json(&self) -> Json {
        json!({
            "key": self.key,
            "line": self.line,
            "column": self.column,
            "message": self.message,
        })
    }
}

/// What validation needs from the environment.
#[derive(Debug, Clone)]
pub struct Env {
    /// The folder `~` stands for.
    pub home: PathBuf,
    /// Check that `files.root` and `files.extra_roots` are existing folders.
    pub check_dirs: bool,
}
impl Env {
    /// This process's home folder (`HOME`), with folder checks.
    pub fn current() -> Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or("HOME is not an absolute folder")?;
        Ok(Self {
            home,
            check_dirs: true,
        })
    }
}

/// `$XDG_CONFIG_HOME/kelpmux/config.toml`, else `~/.config/kelpmux/config.toml`.
pub fn default_path() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        Some(base) => base,
        None => Env::current()?.home.join(".config"),
    };
    Ok(base.join("kelpmux").join(FILE_NAME))
}

/// The file's text, or `None` when there is no file. A symbolic link is
/// followed (a dotfiles manager may link the file); the target must be a
/// regular UTF-8 file of at most 64 KiB.
pub fn read(path: &Path) -> Result<Option<String>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err("the configuration must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("the configuration must be smaller than 64 KiB".into());
    }
    Ok(Some(
        String::from_utf8(bytes).map_err(|_| "the configuration is not valid UTF-8")?,
    ))
}

/// Expands `~` and `~/…` and checks that the result is an absolute path
/// without `..`. `.` components and trailing slashes are dropped.
fn expand(written: &str, home: &Path) -> std::result::Result<PathBuf, String> {
    if written.is_empty() {
        return Err("must not be empty".into());
    }
    if written.len() > MAX_PATH {
        return Err("is too long".into());
    }
    if written.chars().any(char::is_control) {
        return Err("must not contain control characters".into());
    }
    let path = if written == "~" {
        home.to_path_buf()
    } else if let Some(rest) = written.strip_prefix("~/") {
        home.join(rest)
    } else if written.starts_with('~') {
        return Err("only `~` and `~/…` are expanded; write an absolute path".into());
    } else {
        PathBuf::from(written)
    };
    if !path.is_absolute() {
        return Err("must be an absolute path or start with `~/`".into());
    }
    if path.components().any(|c| c == Component::ParentDir) {
        return Err("must not contain `..`".into());
    }
    Ok(path.components().collect())
}

/// Levenshtein distance, for "did you mean".
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let current = row[j + 1];
            row[j + 1] = (previous + usize::from(ca != *cb))
                .min(row[j] + 1)
                .min(current + 1);
            previous = current;
        }
    }
    row[b.len()]
}

/// The closest of `candidates` to an unknown key, when it is close.
fn suggestion<'a>(unknown: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    candidates
        .into_iter()
        .map(|candidate| (distance(unknown, candidate), candidate))
        .filter(|(distance, _)| *distance <= 2.max(unknown.chars().count() / 3))
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate)
}

fn unknown_message(shown: &str, hint: Option<String>) -> String {
    match hint {
        Some(hint) => format!("unknown key `{shown}`; did you mean `{hint}`?"),
        None => format!("unknown key `{shown}`"),
    }
}

/// Parses and validates a configuration, collecting every problem.
pub fn validate(text: &str, env: &Env) -> std::result::Result<Config, Vec<Diagnostic>> {
    let document = match toml_edit::Document::parse(text) {
        Ok(document) => document,
        Err(error) => {
            let message = error
                .message()
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(vec![Diagnostic::new(
                text,
                error.span(),
                None,
                format!("invalid TOML: {message}"),
            )]);
        }
    };
    let mut errors = Vec::new();
    let mut error = |span: Option<Range<usize>>, key: Option<String>, message: String| {
        errors.push(Diagnostic::new(text, span, key, message));
    };
    let mut strings: Strings = Vec::new();
    let mut raw_projects: Vec<RawProject> = Vec::new();
    let top_level = || {
        KEYS.iter()
            .filter(|key| key.section.is_empty())
            .map(|key| key.name)
            .chain(SECTIONS.iter().map(|(name, _)| *name))
            .chain([PROJECT_TABLE])
    };
    for (name, item) in document.iter() {
        let key_span = document.key(name).and_then(|key| key.span());
        if name == PROJECT_TABLE {
            raw_projects = read_projects(item, key_span, &mut error);
        } else if let Some(section) = SECTIONS.iter().find(|(section, _)| *section == name) {
            let Some(table) = item.as_table_like() else {
                error(
                    item.span().or(key_span),
                    Some(name.to_owned()),
                    format!("`{name}` must be a table (`[{name}]`)"),
                );
                continue;
            };
            check_table(section.0, table, &mut strings, &mut error);
        } else if let Some(key) = KEYS
            .iter()
            .find(|key| key.section.is_empty() && key.name == name)
        {
            match (key.kind, item.as_integer()) {
                (Kind::Version, Some(VERSION)) => {}
                (Kind::Version, Some(version)) if version > VERSION => error(
                    item.span(),
                    Some(key.path()),
                    format!(
                        "this file is for a newer KelpMux (version {version}); this one reads version {VERSION}"
                    ),
                ),
                _ => error(
                    item.span(),
                    Some(key.path()),
                    format!("`version` must be {VERSION}"),
                ),
            }
        } else {
            // `root = …` at the top: suggest `files.root`.
            let hint = KEYS
                .iter()
                .find(|key| !key.section.is_empty() && key.name == name)
                .map(Key::path)
                .or_else(|| suggestion(name, top_level()).map(str::to_owned));
            error(key_span, None, unknown_message(name, hint));
        }
    }
    let ports_key = key("environments.ports").expect("known key");
    let environment_ports = match strings
        .iter()
        .find(|(found, _)| std::ptr::eq(*found, ports_key))
        .and_then(|(_, values)| values.first().cloned())
    {
        Some((written, span)) => match port_range(&written) {
            Ok(range) => Some((written, range)),
            Err(message) => {
                error(
                    span,
                    Some(ports_key.path()),
                    format!("`environments.ports` {message}"),
                );
                None
            }
        },
        None => {
            let written = ports_key.default.trim_matches('"').to_owned();
            port_range(&written).ok().map(|range| (written, range))
        }
    };
    let home = &env.home;
    let mut folder =
        |key: &str, written: &str, span: Option<Range<usize>>| match expand(written, home) {
            Ok(path) => Some(Folder {
                written: written.to_owned(),
                path,
                span,
            }),
            Err(message) => {
                error(span, Some(key.to_owned()), format!("`{key}` {message}"));
                None
            }
        };
    let mut single = |path: &str| -> Option<Folder> {
        let key = key(path).expect("known key");
        match strings.iter().find(|(found, _)| std::ptr::eq(*found, key)) {
            Some((_, values)) => values
                .first()
                .and_then(|(written, span)| folder(path, written, span.clone())),
            None => folder(path, key.default.trim_matches('"'), None),
        }
    };
    let root = single("files.root");
    let base = single("projects.base");
    let scratch = single("scratch.folder");
    let extra: Vec<Option<Folder>> = strings
        .iter()
        .find(|(key, _)| key.path() == "files.extra_roots")
        .map(|(_, values)| values.clone())
        .unwrap_or_default()
        .into_iter()
        .map(|(written, span)| folder("files.extra_roots", &written, span))
        .collect();
    let mut error = |span: Option<Range<usize>>, key: &str, message: String| {
        errors.push(Diagnostic::new(text, span, Some(key.to_owned()), message));
    };
    let (Some(root), Some(base), Some(scratch)) = (root, base, scratch) else {
        return Err(errors);
    };
    if extra.iter().any(Option::is_none) {
        return Err(errors);
    }
    let extra: Vec<Folder> = extra.into_iter().flatten().collect();
    if extra.len() > MAX_EXTRA_ROOTS {
        error(
            extra[MAX_EXTRA_ROOTS].span.clone(),
            "files.extra_roots",
            format!("`files.extra_roots` holds at most {MAX_EXTRA_ROOTS} folders"),
        );
    }
    if env.check_dirs {
        for (key, folder) in std::iter::once(("files.root", &root))
            .chain(extra.iter().map(|folder| ("files.extra_roots", folder)))
        {
            if !folder.path.is_dir() {
                error(
                    folder.span.clone(),
                    key,
                    format!(
                        "`{key}`: {} does not exist or is not a folder",
                        folder.path.display()
                    ),
                );
            }
        }
    }
    for (index, folder) in extra.iter().enumerate() {
        if extra[..index].iter().any(|other| other.path == folder.path) {
            error(
                folder.span.clone(),
                "files.extra_roots",
                format!("`files.extra_roots` lists {} twice", folder.path.display()),
            );
        } else if folder.path.starts_with(&root.path) {
            error(
                folder.span.clone(),
                "files.extra_roots",
                format!(
                    "`files.extra_roots`: {} is already inside `files.root` ({})",
                    folder.path.display(),
                    root.path.display()
                ),
            );
        }
    }
    for (key, folder) in [("projects.base", &base), ("scratch.folder", &scratch)] {
        let allowed = std::iter::once(&root)
            .chain(&extra)
            .any(|allowed| folder.path.starts_with(&allowed.path));
        if !allowed {
            error(
                folder.span.clone(),
                key,
                format!(
                    "`{key}` ({}) must lie inside `files.root` ({}) or one of `files.extra_roots`",
                    folder.path.display(),
                    root.path.display()
                ),
            );
        }
    }
    let projects = check_projects(&raw_projects, home, &root, &extra, &scratch, &mut error);
    let github = check_github(&strings, home, &mut error);
    let Some(environment_ports) = environment_ports.filter(|_| errors.is_empty()) else {
        return Err(errors);
    };
    Ok(Config {
        files_root: root,
        files_extra_roots: extra,
        projects_base: base,
        scratch_folder: scratch,
        projects,
        environment_ports,
        github,
    })
}

/// Checks the `[github]` values collected by [`check_table`].
fn check_github(
    strings: &Strings,
    home: &Path,
    error: &mut impl FnMut(Option<Range<usize>>, &str, String),
) -> GitHubSettings {
    let text = |name: &str| {
        strings
            .iter()
            .find(|(key, _)| key.section == "github" && key.name == name)
            .and_then(|(_, values)| values.first().cloned())
            .filter(|(v, _)| !v.is_empty())
    };
    let mut path = |name: &str| -> Option<(PathBuf, Option<Range<usize>>)> {
        let (written, span) = text(name)?;
        match expand(&written, home) {
            Ok(path) => Some((path, span)),
            Err(message) => {
                error(
                    span,
                    &format!("github.{name}"),
                    format!("`github.{name}` {message}"),
                );
                None
            }
        }
    };
    let command = path("token_command");
    let ssh_auth_sock = path("ssh_auth_sock").map(|(path, _)| path);
    let auth = match text("auth") {
        Some((auth, span)) => match auth.as_str() {
            "gh" | "env" if command.is_some() => {
                error(
                    span,
                    "github.auth",
                    "set `github.auth` or `github.token_command`, not both".into(),
                );
                None
            }
            "gh" => Some(GitHubAuth::Gh),
            "env" => Some(GitHubAuth::Env),
            _ => {
                error(
                    span,
                    "github.auth",
                    "`github.auth` must be `\"gh\"`, `\"env\"` or empty".into(),
                );
                None
            }
        },
        None => command.map(|(path, _)| GitHubAuth::Command(path)),
    };
    GitHubSettings {
        auth,
        ssh_auth_sock,
    }
}

/// The array of project tables: `[[project]]`.
pub const PROJECT_TABLE: &str = "project";

type Span = Option<Range<usize>>;
/// A `[[project]]` table as written, before its values are checked.
#[derive(Default)]
struct RawProject {
    /// Where the table starts (its first key), for problems with it as a whole.
    span: Span,
    id: Option<(String, Span)>,
    name: Option<(String, Span)>,
    folder: Option<(String, Span)>,
    color: Option<(String, Span)>,
}

/// The diagnostic key of a project's field: `project[0].folder`.
pub fn project_key(index: usize, field: &str) -> String {
    format!("{PROJECT_TABLE}[{index}].{field}")
}

/// Reads the `project` item: an array of tables (`[[project]]`) or of
/// inline tables, each with known string fields.
fn read_projects(
    item: &Item,
    key_span: Span,
    error: &mut impl FnMut(Span, Option<String>, String),
) -> Vec<RawProject> {
    let mut tables: Vec<(&dyn TableLike, Span)> = Vec::new();
    if let Some(array) = item.as_array_of_tables() {
        tables.extend(
            array
                .iter()
                .map(|table| (table as &dyn TableLike, table.span())),
        );
    } else if let Some(array) = item.as_array() {
        for value in array.iter() {
            match value.as_inline_table() {
                Some(table) => tables.push((table as &dyn TableLike, value.span())),
                None => error(
                    value.span(),
                    Some(PROJECT_TABLE.into()),
                    "each `project` must be a table".into(),
                ),
            }
        }
    } else {
        error(
            item.span().or(key_span),
            Some(PROJECT_TABLE.into()),
            "`project` must be an array of tables (`[[project]]`)".into(),
        );
        return Vec::new();
    }
    let mut projects = Vec::new();
    for (index, (table, span)) in tables.into_iter().enumerate() {
        let first = table
            .iter()
            .next()
            .and_then(|(name, _)| table.key(name))
            .and_then(|key| key.span());
        let mut project = RawProject {
            span: span.or(first).or(key_span.clone()),
            ..RawProject::default()
        };
        for (name, value) in table.iter() {
            let span = table.key(name).and_then(|key| key.span());
            let Some(field) = PROJECT_FIELDS.iter().find(|field| field.name == name) else {
                let hint = suggestion(name, PROJECT_FIELDS.iter().map(|field| field.name))
                    .map(|field| format!("{PROJECT_TABLE}.{field}"));
                error(
                    span,
                    None,
                    unknown_message(&format!("{PROJECT_TABLE}.{name}"), hint),
                );
                continue;
            };
            let Some(text) = value.as_str() else {
                error(
                    value.span().or(span),
                    Some(project_key(index, field.name)),
                    format!("`{PROJECT_TABLE}.{}` must be a {}", field.name, field.kind),
                );
                continue;
            };
            let entry = Some((text.to_owned(), value.span().or(span)));
            match field.name {
                "id" => project.id = entry,
                "name" => project.name = entry,
                "folder" => project.folder = entry,
                _ => project.color = entry,
            }
        }
        projects.push(project);
    }
    projects
}

/// Why `id` is not a valid project id, if it is not.
pub fn id_problem(id: &str) -> Option<&'static str> {
    let valid = !id.is_empty()
        && id.len() <= MAX_NAME
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-');
    if !valid {
        Some(
            "must be 1–64 lowercase letters, digits and `-`, starting and ending with a letter or digit",
        )
    } else if id == SCRATCH {
        Some("`scratch` is the built-in Scratch project's")
    } else {
        None
    }
}

/// Why `name` is not a valid project name, if it is not.
pub fn name_problem(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        Some("must not be empty")
    } else if name.trim() != name {
        Some("must not start or end with spaces")
    } else if name.chars().count() > MAX_NAME {
        Some("must be at most 64 characters")
    } else if name.chars().any(char::is_control) || name.contains('/') {
        Some("must not contain `/` or control characters")
    } else if name == "." || name == ".." {
        Some("must not be `.` or `..`")
    } else if name.eq_ignore_ascii_case("scratch") {
        Some("`Scratch` is the built-in project's name")
    } else {
        None
    }
}

/// Whether `color` is a named colour or `#rrggbb`.
pub fn color_valid(color: &str) -> bool {
    COLORS.contains(&color)
        || (color.len() == 7
            && color.starts_with('#')
            && color[1..].bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Checks the projects' values and how their folders relate.
fn check_projects(
    raw: &[RawProject],
    home: &Path,
    root: &Folder,
    extra: &[Folder],
    scratch: &Folder,
    error: &mut impl FnMut(Span, &str, String),
) -> Vec<Project> {
    if raw.len() > MAX_PROJECTS {
        error(
            raw[MAX_PROJECTS].span.clone(),
            PROJECT_TABLE,
            format!("at most {MAX_PROJECTS} projects"),
        );
    }
    let mut projects: Vec<(usize, Project)> = Vec::new();
    for (index, project) in raw.iter().enumerate() {
        let label = match &project.name {
            Some((name, _)) => format!("project “{name}”"),
            None => format!("project {}", index + 1),
        };
        let mut ok = true;
        for (field, value) in [
            ("id", &project.id),
            ("name", &project.name),
            ("folder", &project.folder),
        ] {
            if value.is_none() {
                error(
                    project.span.clone(),
                    &project_key(index, field),
                    format!("{label} needs `{field}`"),
                );
                ok = false;
            }
        }
        let (Some((id, id_span)), Some((name, name_span)), Some((written, folder_span))) =
            (&project.id, &project.name, &project.folder)
        else {
            continue;
        };
        if let Some(problem) = id_problem(id) {
            error(
                id_span.clone(),
                &project_key(index, "id"),
                format!("{label}: `id` {problem}"),
            );
            ok = false;
        } else if projects.iter().any(|(_, other)| other.id == *id) {
            error(
                id_span.clone(),
                &project_key(index, "id"),
                format!("{label}: another project already has the id `{id}`"),
            );
            ok = false;
        }
        if let Some(problem) = name_problem(name) {
            error(
                name_span.clone(),
                &project_key(index, "name"),
                format!("{label}: the name {problem}"),
            );
            ok = false;
        } else if projects
            .iter()
            .any(|(_, other)| other.name.to_lowercase() == name.to_lowercase())
        {
            error(
                name_span.clone(),
                &project_key(index, "name"),
                format!("another project is already named “{name}”"),
            );
            ok = false;
        }
        if let Some((color, span)) = &project.color
            && !color_valid(color)
        {
            error(
                span.clone(),
                &project_key(index, "color"),
                format!(
                    "{label}: `color` must be one of {} or `#rrggbb`",
                    COLORS.join(", ")
                ),
            );
            ok = false;
        }
        let key = project_key(index, "folder");
        let path = match expand(written, home) {
            Ok(path) => path,
            Err(message) => {
                error(
                    folder_span.clone(),
                    &key,
                    format!("{label}: the folder {message}"),
                );
                continue;
            }
        };
        let inside_roots = std::iter::once(root)
            .chain(extra)
            .any(|allowed| path.starts_with(&allowed.path));
        if !inside_roots {
            error(
                folder_span.clone(),
                &key,
                format!(
                    "{label}: the folder {} must lie inside `files.root` ({}) or one of `files.extra_roots`",
                    path.display(),
                    root.path.display()
                ),
            );
            ok = false;
        }
        let overlaps = |other: &Path| path.starts_with(other) || other.starts_with(&path);
        if overlaps(&scratch.path) {
            error(
                folder_span.clone(),
                &key,
                format!(
                    "{label}: the folder {} overlaps the Scratch folder ({})",
                    path.display(),
                    scratch.path.display()
                ),
            );
            ok = false;
        }
        if let Some((_, other)) = projects
            .iter()
            .find(|(_, other)| overlaps(&other.folder.path))
        {
            let relation = if path == other.folder.path {
                "is also the folder of"
            } else if path.starts_with(&other.folder.path) {
                "is inside the folder of"
            } else {
                "contains the folder of"
            };
            error(
                folder_span.clone(),
                &key,
                format!(
                    "{label}: the folder {} {relation} project “{}” ({})",
                    path.display(),
                    other.name,
                    other.folder.path.display()
                ),
            );
            ok = false;
        }
        if ok {
            projects.push((
                index,
                Project {
                    id: id.clone(),
                    name: name.clone(),
                    folder: Folder {
                        written: written.clone(),
                        path,
                        span: folder_span.clone(),
                    },
                    color: project.color.as_ref().map(|(color, _)| color.clone()),
                },
            ));
        }
    }
    projects.into_iter().map(|(_, project)| project).collect()
}

type Strings = Vec<(&'static Key, Vec<(String, Option<Range<usize>>)>)>;

/// Checks one table's keys and collects their string values.
fn check_table(
    section: &str,
    table: &dyn TableLike,
    strings: &mut Strings,
    error: &mut impl FnMut(Option<Range<usize>>, Option<String>, String),
) {
    for (name, item) in table.iter() {
        let key_span = table.key(name).and_then(|key| key.span());
        let Some(key) = KEYS
            .iter()
            .find(|key| key.section == section && key.name == name)
        else {
            let hint = KEYS
                .iter()
                .find(|key| key.section != section && !key.section.is_empty() && key.name == name)
                .map(Key::path)
                .or_else(|| {
                    suggestion(
                        name,
                        KEYS.iter()
                            .filter(|key| key.section == section)
                            .map(|key| key.name),
                    )
                    .map(|name| format!("{section}.{name}"))
                });
            error(
                key_span,
                None,
                unknown_message(&format!("{section}.{name}"), hint),
            );
            continue;
        };
        let path = key.path();
        let mistyped = || format!("`{path}` must be a {}", key.kind.label());
        match key.kind {
            Kind::Folder | Kind::PortRange | Kind::Text | Kind::File => match item.as_str() {
                Some(value) => strings.push((key, vec![(value.to_owned(), item.span())])),
                None => error(item.span().or(key_span), Some(path.clone()), mistyped()),
            },
            Kind::FolderList => match item.as_array() {
                Some(array) => {
                    let mut values = Vec::new();
                    for value in array.iter() {
                        match value.as_str() {
                            Some(text) => values.push((text.to_owned(), value.span())),
                            None => error(value.span(), Some(path.clone()), mistyped()),
                        }
                    }
                    strings.push((key, values));
                }
                None => error(item.span().or(key_span), Some(path.clone()), mistyped()),
            },
            Kind::Version => unreachable!("version is top-level"),
        }
    }
}

/// Wraps a paragraph as `# ` comment lines of at most 78 columns.
fn comment(text: &str) -> String {
    let mut out = String::new();
    let mut line = String::from("#");
    for word in text.split_whitespace() {
        if line.len() > 1 && line.len() + 1 + word.len() > 78 {
            out.push_str(&line);
            out.push('\n');
            line = String::from("#");
        }
        line.push(' ');
        line.push_str(word);
    }
    out.push_str(&line);
    out.push('\n');
    out
}

/// The default file: every key commented out at its default, with its
/// documentation.
pub fn template() -> String {
    let mut out = String::new();
    out.push_str("# KelpMux user configuration.\n#\n");
    out.push_str(&comment(
        "Every key is optional: a commented-out key shows its default. Edit this file directly or in the web client (Settings, Files & folders); open web clients apply changes live. After editing, check the file with `kelpmux config check`. Paths are absolute or start with `~/` (your home folder).",
    ));
    out.push_str(&format!("#\n# Reference: {REFERENCE_URL}\n"));
    let entry = |out: &mut String, key: &Key| {
        out.push('\n');
        out.push_str(&comment(key.doc));
        out.push_str(&comment(&format!("Applies: {}", key.applies)));
        if let Some(example) = key.example {
            out.push_str(&format!("# Example: {} = {example}\n", key.name));
        }
        out.push_str(&format!("# {} = {}\n", key.name, key.default));
    };
    for key in KEYS.iter().filter(|key| key.section.is_empty()) {
        entry(&mut out, key);
    }
    for (section, about) in SECTIONS {
        out.push_str(&format!("\n[{section}]\n"));
        out.push_str(&comment(about));
        for key in KEYS.iter().filter(|key| key.section == *section) {
            entry(&mut out, key);
        }
    }
    // Projects are added at the end of the file, after this example.
    out.push('\n');
    out.push_str(&comment(PROJECTS_DOC));
    for field in PROJECT_FIELDS {
        out.push_str(&comment(&format!(
            "`{}`{}: {}",
            field.name,
            if field.required { "" } else { " (optional)" },
            field.doc
        )));
    }
    out.push_str("# Example:\n#\n");
    out.push_str(&format!("# [[{PROJECT_TABLE}]]\n"));
    for field in PROJECT_FIELDS {
        out.push_str(&format!("# {} = {}\n", field.name, field.example));
    }
    out
}

/// A TOML default or example as JSON (strings, integers and arrays).
fn toml_json(literal: &str) -> Json {
    fn convert(value: &Value) -> Json {
        match value {
            Value::String(text) => json!(text.value()),
            Value::Integer(number) => json!(number.value()),
            Value::Array(array) => Json::Array(array.iter().map(convert).collect()),
            _ => Json::Null,
        }
    }
    literal
        .parse::<Value>()
        .map(|value| convert(&value))
        .unwrap_or(Json::Null)
}

/// The file's JSON Schema (draft 2020-12).
pub fn schema() -> Json {
    let property = |key: &Key| {
        let mut value = match key.kind {
            Kind::Version => json!({ "type": "integer", "const": VERSION }),
            Kind::Folder => json!({ "type": "string", "minLength": 1, "maxLength": MAX_PATH }),
            Kind::PortRange => json!({ "type": "string", "pattern": "^[0-9]{4,5}-[0-9]{4,5}$" }),
            Kind::FolderList => json!({
                "type": "array",
                "items": { "type": "string", "minLength": 1, "maxLength": MAX_PATH },
                "maxItems": MAX_EXTRA_ROOTS,
                "uniqueItems": true,
            }),
            Kind::Text | Kind::File => json!({ "type": "string", "maxLength": MAX_PATH }),
        };
        value["description"] = json!(format!("{} Applies: {}", key.doc, key.applies));
        value["default"] = toml_json(key.default);
        if let Some(example) = key.example {
            value["examples"] = json!([toml_json(example)]);
        }
        value
    };
    let mut properties = serde_json::Map::new();
    for key in KEYS.iter().filter(|key| key.section.is_empty()) {
        properties.insert(key.name.into(), property(key));
    }
    for (section, about) in SECTIONS {
        let fields: serde_json::Map<String, Json> = KEYS
            .iter()
            .filter(|key| key.section == *section)
            .map(|key| (key.name.to_owned(), property(key)))
            .collect();
        properties.insert(
            (*section).into(),
            json!({
                "type": "object",
                "description": about,
                "additionalProperties": false,
                "properties": fields,
            }),
        );
    }
    let fields: serde_json::Map<String, Json> = PROJECT_FIELDS
        .iter()
        .map(|field| {
            let mut value = match field.name {
                "id" => json!({ "type": "string", "pattern": "^[a-z0-9]([a-z0-9-]*[a-z0-9])?$", "maxLength": MAX_NAME, "not": { "const": SCRATCH } }),
                "name" => json!({ "type": "string", "minLength": 1, "maxLength": MAX_NAME, "pattern": "^[^/]*$" }),
                "folder" => json!({ "type": "string", "minLength": 1, "maxLength": MAX_PATH }),
                _ => json!({ "type": "string", "anyOf": [{ "enum": COLORS }, { "pattern": "^#[0-9a-fA-F]{6}$" }] }),
            };
            value["description"] = json!(field.doc);
            value["examples"] = json!([toml_json(field.example)]);
            (field.name.to_owned(), value)
        })
        .collect();
    properties.insert(
        PROJECT_TABLE.into(),
        json!({
            "type": "array",
            "description": PROJECTS_DOC,
            "maxItems": MAX_PROJECTS,
            "items": {
                "type": "object",
                "additionalProperties": false,
                "required": PROJECT_FIELDS.iter().filter(|field| field.required).map(|field| field.name).collect::<Vec<_>>(),
                "properties": fields,
            },
        }),
    );
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": REFERENCE_URL,
        "title": "KelpMux user configuration (config.toml)",
        "description": "Folder paths are absolute or start with `~/`. `projects.base` and `scratch.folder` must lie inside `files.root` or one of `files.extra_roots`; `kelpmux config check` enforces what a schema cannot.",
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
    })
}

/// The key reference of `docs/config.md`, printed by `kelpmux config docs`.
pub fn reference() -> String {
    let mut out = String::new();
    let entry = |out: &mut String, key: &Key| {
        out.push_str(&format!("\n### `{}`\n\n{}\n\n", key.path(), key.doc));
        out.push_str("| | |\n| --- | --- |\n");
        out.push_str(&format!("| Type | {} |\n", key.kind.label()));
        out.push_str(&format!("| Default | `{}` |\n", key.default));
        if let Some(example) = key.example {
            out.push_str(&format!("| Example | `{} = {example}` |\n", key.name));
        }
        out.push_str(&format!("| Applies | {} |\n", key.applies));
    };
    out.push_str("## Top level\n");
    for key in KEYS.iter().filter(|key| key.section.is_empty()) {
        entry(&mut out, key);
    }
    for (section, about) in SECTIONS {
        out.push_str(&format!("\n## `[{section}]`\n\n{about}\n"));
        for key in KEYS.iter().filter(|key| key.section == *section) {
            entry(&mut out, key);
        }
    }
    out.push_str(&format!("\n## `[[{PROJECT_TABLE}]]`\n\n{PROJECTS_DOC}\n\n"));
    out.push_str("```toml\n");
    out.push_str(&format!("[[{PROJECT_TABLE}]]\n"));
    for field in PROJECT_FIELDS {
        out.push_str(&format!("{} = {}\n", field.name, field.example));
    }
    out.push_str("```\n");
    for field in PROJECT_FIELDS {
        out.push_str(&format!(
            "\n### `{PROJECT_TABLE}.{}`\n\n{}\n\n",
            field.name, field.doc
        ));
        out.push_str("| | |\n| --- | --- |\n");
        out.push_str(&format!("| Type | {} |\n", field.kind));
        out.push_str(&format!(
            "| Required | {} |\n",
            if field.required { "yes" } else { "no" }
        ));
        out.push_str(&format!(
            "| Example | `{} = {}` |\n",
            field.name, field.example
        ));
        out.push_str("| Applies | Immediately. |\n");
    }
    // A blank line ends the last table before what follows.
    out.push('\n');
    out
}

/// A change the web client asks for: a key and its new value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub key: &'static Key,
    pub value: Vec<String>,
}

/// Reads `{"files.root": "~", "files.extra_roots": ["/mnt/data"], …}`.
pub fn changes(params: &Json) -> std::result::Result<Vec<Change>, String> {
    let object = params
        .as_object()
        .ok_or("the changes must be an object of keys")?;
    if object.is_empty() {
        return Err("nothing to change".into());
    }
    object
        .iter()
        .map(|(name, value)| {
            // `[github]` names programs this account runs: it is edited in
            // the file only, never from a browser.
            let key = key(name)
                .filter(|key| matches!(key.kind, Kind::Folder | Kind::FolderList | Kind::PortRange))
                .ok_or_else(|| format!("`{name}` cannot be changed here"))?;
            let value = match (key.kind, value) {
                (Kind::Folder | Kind::PortRange, Json::String(text)) => vec![text.clone()],
                (Kind::FolderList, Json::Array(items)) if items.len() <= 64 => items
                    .iter()
                    .map(|item| item.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| format!("`{name}` must be a list of strings"))?,
                _ => return Err(format!("`{name}` must be a {}", key.kind.label())),
            };
            Ok(Change { key, value })
        })
        .collect()
}

/// Turns a commented-out default (`# root = "~"`, as [`template`] writes
/// it) into a real line, so a value set from the web client lands under its
/// documentation instead of at the end of the table.
fn uncomment(text: &str, key: &Key) -> Option<String> {
    let header = format!("[{}]", key.section);
    let mut offset = 0;
    let mut inside = key.section.is_empty();
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            if inside {
                // Past the key's table: no commented default there.
                return None;
            }
            let header_line = trimmed.trim_end();
            inside = header_line == header
                || header_line
                    .strip_prefix(&header)
                    .is_some_and(|rest| rest.trim_start().starts_with('#'));
        } else if inside && trimmed.starts_with("# [[") {
            // The commented `[[project]]` example: not this table's.
            return None;
        } else if inside && let Some(body) = trimmed.strip_prefix('#') {
            // `body` and `trimmed` are suffixes of `line`.
            let body = body.trim_start_matches([' ', '\t']);
            if body
                .strip_prefix(key.name)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
            {
                let hash = offset + line.len() - trimmed.len();
                let value = offset + line.len() - body.len();
                return Some(format!("{}{}", &text[..hash], &text[value..]));
            }
        }
        offset += line.len();
    }
    None
}

/// Applies `changes` to the file's text, keeping comments and formatting,
/// and validates the result. Returns the new text, or the problems.
pub fn edit(
    text: &str,
    changes: &[Change],
    env: &Env,
) -> std::result::Result<String, Vec<Diagnostic>> {
    let mut text = text.to_owned();
    if let Err(errors) = validate(
        &text,
        &Env {
            check_dirs: false,
            ..env.clone()
        },
    ) && errors
        .iter()
        .any(|error| error.message.starts_with("invalid TOML"))
    {
        return Err(errors);
    }
    for change in changes {
        let present = text
            .parse::<DocumentMut>()
            .ok()
            .and_then(|document| {
                document
                    .get(change.key.section)
                    .and_then(Item::as_table_like)
                    .map(|table| table.contains_key(change.key.name))
            })
            .unwrap_or(false);
        if !present
            && let Some(candidate) = uncomment(&text, change.key)
            && let Ok(document) = candidate.parse::<DocumentMut>()
            && document
                .get(change.key.section)
                .and_then(Item::as_table_like)
                .is_some_and(|table| table.contains_key(change.key.name))
        {
            text = candidate;
        }
    }
    let mut document = text.parse::<DocumentMut>().map_err(|error| {
        vec![Diagnostic::new(
            &text,
            error.span(),
            None,
            format!("invalid TOML: {}", error.message().trim()),
        )]
    })?;
    for change in changes {
        let key = change.key;
        let mut value: Value = match key.kind {
            Kind::FolderList => {
                let mut array = Array::new();
                for item in &change.value {
                    array.push(item.as_str());
                }
                Value::Array(array)
            }
            _ => Value::from(change.value.first().cloned().unwrap_or_default()),
        };
        let root = document.as_table_mut();
        if !root.contains_key(key.section) {
            root.insert(key.section, toml_edit::table());
        }
        let table = root
            .get_mut(key.section)
            .and_then(Item::as_table_like_mut)
            .ok_or_else(|| {
                vec![Diagnostic {
                    key: Some(key.section.to_owned()),
                    line: 1,
                    column: 1,
                    message: format!("`{}` must be a table", key.section),
                }]
            })?;
        match table.get_mut(key.name) {
            Some(Item::Value(old)) => {
                *value.decor_mut() = old.decor().clone();
                *old = value;
            }
            Some(item) => *item = Item::Value(value),
            None => {
                table.insert(key.name, Item::Value(value));
            }
        }
    }
    let text = document.to_string();
    validate(&text, env)?;
    Ok(text)
}

/// An exclusive `flock` on `.config.toml.lock` beside the file, held until
/// dropped. Waits at most five seconds.
pub(crate) struct WriteLock(#[allow(dead_code)] File);
impl WriteLock {
    pub(crate) fn acquire(target: &Path) -> Result<Self> {
        let name = target
            .file_name()
            .ok_or("the configuration path has no file name")?
            .to_string_lossy();
        let path = target.with_file_name(format!(".{name}.lock"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Self(file));
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::WouldBlock {
                return Err(error.into());
            }
            if Instant::now() >= deadline {
                return Err(format!("another KelpMux is writing {name}; try again").into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// The file a write replaces: the target of a symbolic link, so the link
/// (from a dotfiles manager, say) stays in place.
fn write_target(path: &Path) -> Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Ok(fs::canonicalize(path)?),
        _ => Ok(path.to_path_buf()),
    }
}

/// Replaces `target` with `text`: a temporary file in the same folder,
/// fsynced, renamed over it, and the folder fsynced. Keeps an existing
/// file's permissions; a new file is private.
pub(crate) fn replace(target: &Path, text: &str) -> Result<()> {
    let name = target
        .file_name()
        .ok_or("the configuration path has no file name")?
        .to_string_lossy();
    let temp = target.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let mode = fs::metadata(target).map_or(0o600, |meta| meta.mode() & 0o777);
    let _ = fs::remove_file(&temp);
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temp)?;
        file.write_all(text.as_bytes())?;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.sync_all()?;
        fs::rename(&temp, target)?;
        File::open(target.parent().ok_or("missing parent")?)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn create_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or("the configuration path has no folder")?;
    if !parent.is_dir() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    Ok(())
}

/// Creates the documented default file when there is none.
pub fn ensure_exists(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path).is_ok() {
        return Ok(());
    }
    create_parent(path)?;
    let _lock = WriteLock::acquire(path)?;
    if fs::symlink_metadata(path).is_ok() {
        return Ok(());
    }
    replace(path, &template())
}

/// Applies `changes` to the file under the write lock, creating it from
/// the default first if needed. The outer error is a failure to read or
/// write; the inner one the problems that kept the change from being saved.
pub fn update(
    path: &Path,
    changes: &[Change],
    env: &Env,
) -> Result<std::result::Result<(), Vec<Diagnostic>>> {
    create_parent(path)?;
    let target = write_target(path)?;
    let _lock = WriteLock::acquire(&target)?;
    let current = read(&target)?.unwrap_or_else(template);
    match edit(&current, changes, env) {
        Ok(text) => {
            replace(&target, &text)?;
            Ok(Ok(()))
        }
        Err(errors) => Ok(Err(errors)),
    }
}

/// A change to the `[[project]]` tables, as the web client asks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectChange {
    Create {
        id: String,
        name: String,
        folder: String,
        color: Option<String>,
    },
    /// Renames a project or changes its colour (`Some(None)` removes it);
    /// its id and folder stay.
    Update {
        id: String,
        name: Option<String>,
        color: Option<Option<String>>,
    },
    /// Removes the project's table; its folder is left alone.
    Delete { id: String },
}

/// The project tables of a document, whether `[[project]]` or an array of
/// inline tables.
fn project_tables(document: &mut DocumentMut) -> Vec<&mut dyn TableLike> {
    match document.get_mut(PROJECT_TABLE) {
        Some(Item::ArrayOfTables(array)) => array
            .iter_mut()
            .map(|table| table as &mut dyn TableLike)
            .collect(),
        Some(Item::Value(Value::Array(array))) => array
            .iter_mut()
            .filter_map(Value::as_inline_table_mut)
            .map(|table| table as &mut dyn TableLike)
            .collect(),
        _ => Vec::new(),
    }
}

/// Sets a string value, keeping the old value's comments and spacing.
fn set_string(table: &mut dyn TableLike, name: &str, text: &str) {
    let mut value = Value::from(text);
    match table.get_mut(name) {
        Some(Item::Value(old)) => {
            *value.decor_mut() = old.decor().clone();
            *old = value;
        }
        Some(item) => *item = Item::Value(value),
        None => {
            table.insert(name, Item::Value(value));
        }
    }
}

/// Applies a project change to the file's text, keeping comments and
/// formatting (a new project is added at the end of the file), and
/// validates the result.
pub fn edit_projects(
    text: &str,
    change: &ProjectChange,
    env: &Env,
) -> std::result::Result<String, Vec<Diagnostic>> {
    let problem = |message: String| {
        vec![Diagnostic {
            key: Some(PROJECT_TABLE.into()),
            line: 1,
            column: 1,
            message,
        }]
    };
    let mut document = text.parse::<DocumentMut>().map_err(|error| {
        vec![Diagnostic::new(
            text,
            error.span(),
            None,
            format!("invalid TOML: {}", error.message().trim()),
        )]
    })?;
    let missing = |id: &str| problem(format!("no project has the id `{id}`"));
    let edited = match change {
        ProjectChange::Create {
            id,
            name,
            folder,
            color,
        } => {
            let mut table = toml_edit::Table::new();
            table.insert("id", toml_edit::value(id.as_str()));
            table.insert("name", toml_edit::value(name.as_str()));
            table.insert("folder", toml_edit::value(folder.as_str()));
            if let Some(color) = color {
                table.insert("color", toml_edit::value(color.as_str()));
            }
            match document.get_mut(PROJECT_TABLE) {
                Some(Item::Value(Value::Array(array))) => {
                    array.push(table.into_inline_table());
                    document.to_string()
                }
                None | Some(Item::ArrayOfTables(_)) => {
                    // Appended as text: the comments above stay where they are.
                    let mut out = text.to_owned();
                    if !out.is_empty() && !out.ends_with('\n') {
                        out.push('\n');
                    }
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    let mut added = DocumentMut::new();
                    let mut array = toml_edit::ArrayOfTables::new();
                    array.push(table);
                    added.insert(PROJECT_TABLE, Item::ArrayOfTables(array));
                    out.push_str(&added.to_string());
                    out
                }
                Some(_) => {
                    return Err(problem(
                        "`project` must be an array of tables (`[[project]]`)".into(),
                    ));
                }
            }
        }
        ProjectChange::Update { id, name, color } => {
            let mut tables = project_tables(&mut document);
            let table = tables
                .iter_mut()
                .find(|table| table.get("id").and_then(Item::as_str) == Some(id.as_str()))
                .ok_or_else(|| missing(id))?;
            if let Some(name) = name {
                set_string(&mut **table, "name", name);
            }
            match color {
                Some(Some(color)) => set_string(&mut **table, "color", color),
                Some(None) => {
                    table.remove("color");
                }
                None => {}
            }
            document.to_string()
        }
        ProjectChange::Delete { id } => {
            let is = |table: &dyn TableLike| table.get("id").and_then(Item::as_str) == Some(id);
            match document.get_mut(PROJECT_TABLE) {
                Some(Item::ArrayOfTables(array)) => {
                    let index = array
                        .iter()
                        .position(|table| is(table))
                        .ok_or_else(|| missing(id))?;
                    array.remove(index);
                    if array.is_empty() {
                        document.remove(PROJECT_TABLE);
                    }
                }
                Some(Item::Value(Value::Array(array))) => {
                    let index = array
                        .iter()
                        .position(|value| value.as_inline_table().is_some_and(|table| is(table)))
                        .ok_or_else(|| missing(id))?;
                    array.remove(index);
                }
                _ => return Err(missing(id)),
            }
            document.to_string()
        }
    };
    validate(&edited, env)?;
    Ok(edited)
}

/// Applies a project change to the file under the write lock, as
/// [`update`] does for keys.
pub fn update_projects(
    path: &Path,
    change: &ProjectChange,
    env: &Env,
) -> Result<std::result::Result<(), Vec<Diagnostic>>> {
    create_parent(path)?;
    let target = write_target(path)?;
    let _lock = WriteLock::acquire(&target)?;
    let current = read(&target)?.unwrap_or_else(template);
    match edit_projects(&current, change, env) {
        Ok(text) => {
            replace(&target, &text)?;
            Ok(Ok(()))
        }
        Err(errors) => Ok(Err(errors)),
    }
}

/// A project id made from `name`: lowercase ASCII letters and digits, other
/// characters as `-`, at most 48 bytes, followed by `-2`, `-3`… when
/// `taken` holds it (ids are never reused, so `taken` includes the ids
/// terminals are still linked to).
pub fn new_project_id(name: &str, taken: &[String]) -> String {
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let mut slug = slug.trim_end_matches('-').to_owned();
    if slug.is_empty() || slug == SCRATCH {
        slug = format!("project{}", if slug.is_empty() { "" } else { "-scratch" });
    }
    let free = |id: &str| !taken.iter().any(|taken| taken == id);
    if free(&slug) {
        return slug;
    }
    (2..)
        .map(|n| format!("{slug}-{n}"))
        .find(|id| free(id))
        .expect("a free id")
}

/// The file's state as the web client shows it.
#[derive(Debug, Clone)]
pub struct Snapshot {
    path: PathBuf,
    exists: bool,
    /// A hash of the file's text, equal in every bridge for the same file.
    revision: String,
    /// The configuration in effect: the file's, or after an invalid edit
    /// the last valid one, or the defaults.
    applied: Config,
    /// Whether `applied` is the file's current content.
    current: bool,
    errors: Vec<Diagnostic>,
}
impl Snapshot {
    /// Reads and validates the file. `previous` is the last valid
    /// configuration, kept when the file is invalid.
    pub fn load(path: &Path, env: &Env, previous: Option<&Config>) -> Self {
        let defaults = || {
            validate(
                "",
                &Env {
                    check_dirs: false,
                    ..env.clone()
                },
            )
            .expect("the defaults are valid")
        };
        let (exists, text, errors) = match read(path) {
            Ok(Some(text)) => (true, text, Vec::new()),
            Ok(None) => (false, String::new(), Vec::new()),
            Err(error) => (
                true,
                String::new(),
                vec![Diagnostic {
                    key: None,
                    line: 1,
                    column: 1,
                    message: error.to_string(),
                }],
            ),
        };
        let revision = format!("{:016x}", fnv(text.as_bytes()) ^ u64::from(exists));
        let result = if errors.is_empty() {
            validate(&text, env)
        } else {
            Err(errors)
        };
        match result {
            Ok(config) => Self {
                path: path.to_path_buf(),
                exists,
                revision,
                applied: config,
                current: true,
                errors: Vec::new(),
            },
            Err(errors) => Self {
                path: path.to_path_buf(),
                exists,
                revision,
                applied: previous.cloned().unwrap_or_else(defaults),
                current: false,
                errors,
            },
        }
    }
    pub fn applied(&self) -> &Config {
        &self.applied
    }
    pub fn to_json(&self) -> Json {
        let mut value = self.applied.to_json();
        value["path"] = json!(self.path);
        value["exists"] = json!(self.exists);
        value["revision"] = json!(self.revision);
        value["current"] = json!(self.current);
        value["errors"] = Json::Array(self.errors.iter().map(Diagnostic::to_json).collect());
        value
    }
}

/// 64-bit FNV-1a.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The configuration of `kelpmux start`: the file's roots when it exists
/// and is valid. An invalid file is reported and ignored, so a typo never
/// keeps a workspace from starting; KelpServer then keeps the roots it was
/// last given.
pub fn workspace_roots() -> Option<(PathBuf, Vec<PathBuf>)> {
    let path = default_path().ok()?;
    let env = Env::current().ok()?;
    match read(&path) {
        Ok(Some(text)) => match validate(&text, &env) {
            Ok(config) => Some(config.roots()),
            Err(errors) => {
                eprintln!(
                    "kelpmux: ignoring the file roots of an invalid configuration (the workspace keeps its previous ones):"
                );
                for error in errors {
                    eprintln!("  {}", error.display(&path));
                }
                None
            }
        },
        Ok(None) => None,
        Err(error) => {
            eprintln!("kelpmux: {}: {error}", path.display());
            None
        }
    }
}

/// What identifies a version of the file without reading it: device,
/// inode, size, modification and change times (following a link).
type Fingerprint = (u64, u64, u64, i64, i64, i64, i64);
fn fingerprint_of(path: &Path) -> Option<Fingerprint> {
    let meta = fs::metadata(path).ok()?;
    Some((
        meta.dev(),
        meta.ino(),
        meta.size(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
    ))
}

/// A bridge's live view of the file: reloaded when it changes, keeping the
/// last valid configuration while it is invalid.
pub struct Live {
    path: PathBuf,
    env: Env,
    state: Mutex<Snapshot>,
    push: Box<dyn Fn(Json) -> bool + Send + Sync>,
    stopped: AtomicBool,
}
impl Live {
    /// Loads the file and starts a thread that watches it (inotify on its
    /// folder, and a two-second check of its metadata for links and
    /// folders created later). `push` receives each new snapshot and
    /// returns false once nobody listens.
    pub fn start(
        path: PathBuf,
        env: Env,
        push: Box<dyn Fn(Json) -> bool + Send + Sync>,
    ) -> Arc<Self> {
        // Taken before the first load, so a change in between is seen.
        let fingerprint = fingerprint_of(&path);
        let state = Mutex::new(Snapshot::load(&path, &env, None));
        let live = Arc::new(Self {
            path,
            env,
            state,
            push,
            stopped: AtomicBool::new(false),
        });
        let watcher = live.clone();
        std::thread::spawn(move || watcher.watch(fingerprint));
        live
    }
    pub fn json(&self) -> Json {
        self.state
            .lock()
            .map(|state| state.to_json())
            .unwrap_or(Json::Null)
    }
    /// Re-reads the file; pushes and returns the snapshot when it changed.
    pub fn reload(&self) -> Json {
        let Ok(mut state) = self.state.lock() else {
            return Json::Null;
        };
        let next = Snapshot::load(&self.path, &self.env, Some(&state.applied));
        let changed = next.revision != state.revision
            || next.errors != state.errors
            || next.applied != state.applied;
        *state = next;
        let value = state.to_json();
        drop(state);
        if changed && !(self.push)(json!({ "Config": value.clone() })) {
            self.stopped.store(true, Ordering::Relaxed);
        }
        value
    }
    /// The `config_set` operation: validates and saves `params`' changes,
    /// then returns `{saved, errors, config}`.
    pub fn set(&self, params: &Json) -> Result<Json> {
        let changes = changes(params)?;
        let result = update(&self.path, &changes, &self.env)?;
        let config = self.reload();
        Ok(match result {
            Ok(()) => json!({ "saved": true, "errors": [], "config": config }),
            Err(errors) => json!({
                "saved": false,
                "errors": errors.iter().map(Diagnostic::to_json).collect::<Vec<_>>(),
                "config": config,
            }),
        })
    }
    /// The configuration in effect.
    pub fn config(&self) -> Option<Config> {
        self.state.lock().ok().map(|state| state.applied.clone())
    }
    /// Saves a project change, then returns `{saved, errors, config}`.
    pub fn change_projects(&self, change: &ProjectChange) -> Result<Json> {
        let result = update_projects(&self.path, change, &self.env)?;
        let config = self.reload();
        Ok(match result {
            Ok(()) => json!({ "saved": true, "errors": [], "config": config }),
            Err(errors) => json!({
                "saved": false,
                "errors": errors.iter().map(Diagnostic::to_json).collect::<Vec<_>>(),
                "config": config,
            }),
        })
    }
    fn watch(&self, mut fingerprint: Option<Fingerprint>) {
        // SAFETY: inotify_init1 returns a new descriptor or -1.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        let inotify = (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) });
        let folder = self.path.parent().map(Path::to_path_buf);
        let mut watched: Option<(u64, u64)> = None;
        let mut buffer = [0u8; 4096];
        while !self.stopped.load(Ordering::Relaxed) {
            // (Re)watch the folder when it appears or is replaced.
            if let (Some(inotify), Some(folder)) = (&inotify, &folder) {
                let identity = fs::metadata(folder)
                    .ok()
                    .map(|meta| (meta.dev(), meta.ino()));
                let mask = libc::IN_CLOSE_WRITE
                    | libc::IN_MOVED_TO
                    | libc::IN_MOVED_FROM
                    | libc::IN_CREATE
                    | libc::IN_DELETE
                    | libc::IN_ATTRIB
                    | libc::IN_MODIFY;
                if identity.is_some()
                    && identity != watched
                    && let Ok(name) = std::ffi::CString::new(folder.as_os_str().as_encoded_bytes())
                    // SAFETY: a valid descriptor and a NUL-terminated path.
                    && unsafe { libc::inotify_add_watch(inotify.as_raw_fd(), name.as_ptr(), mask) }
                        >= 0
                {
                    watched = identity;
                }
            }
            let mut event = false;
            if let Some(inotify) = &inotify {
                let mut poll = libc::pollfd {
                    fd: inotify.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: one valid pollfd.
                if unsafe { libc::poll(&mut poll, 1, 2000) } > 0 {
                    event = true;
                    // Let a burst of writes settle, then drain it.
                    std::thread::sleep(Duration::from_millis(75));
                    // SAFETY: reads into a buffer of its stated length.
                    while unsafe {
                        libc::read(
                            inotify.as_raw_fd(),
                            buffer.as_mut_ptr().cast(),
                            buffer.len(),
                        )
                    } > 0
                    {}
                }
            } else {
                std::thread::sleep(Duration::from_secs(2));
            }
            let next = fingerprint_of(&self.path);
            if event || next != fingerprint {
                fingerprint = next;
                self.reload();
            }
        }
    }
}
impl Drop for Live {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

/// `kelpmux config …`.
pub fn cli(args: &[String]) -> Result<()> {
    const USAGE: &str = "usage: kelpmux config path|check [FILE]|schema|docs";
    match args.first().map(String::as_str) {
        Some("path") if args.len() == 1 => {
            let path = default_path()?;
            ensure_exists(&path)?;
            println!("{}", path.display());
            Ok(())
        }
        Some("check") if args.len() <= 2 => {
            let path = match args.get(1) {
                Some(path) => PathBuf::from(path),
                None => default_path()?,
            };
            let env = Env::current()?;
            let text = match read(&path) {
                Ok(Some(text)) => text,
                Ok(None) => {
                    println!("{}: no file; the defaults apply", path.display());
                    return Ok(());
                }
                Err(error) => {
                    eprintln!("{}: {error}", path.display());
                    std::process::exit(1);
                }
            };
            match validate(&text, &env) {
                Ok(_) => {
                    println!("{}: valid", path.display());
                    Ok(())
                }
                Err(errors) => {
                    for error in &errors {
                        eprintln!("{}", error.display(&path));
                    }
                    std::process::exit(1);
                }
            }
        }
        Some("schema") if args.len() == 1 => {
            println!("{}", serde_json::to_string_pretty(&schema())?);
            Ok(())
        }
        Some("docs") if args.len() == 1 => {
            print!("{}", reference());
            Ok(())
        }
        Some("--help" | "-h" | "help") | None => {
            println!(
                "{USAGE}\n  path    print the file's path, creating the documented default file if there is none\n  check   validate a file (default: the user's); errors as path:line:col: message, exit 1\n  schema  print the JSON Schema\n  docs    print the key reference of docs/config.md"
            );
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "kelpmux-config-{name}-{}-{}",
                std::process::id(),
                crate::now()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn env(home: &Path) -> Env {
        Env {
            home: home.to_path_buf(),
            check_dirs: false,
        }
    }
    fn messages(text: &str) -> Vec<String> {
        validate(text, &env(Path::new("/home/u")))
            .unwrap_err()
            .iter()
            .map(|error| format!("{}:{}: {}", error.line, error.column, error.message))
            .collect()
    }

    #[test]
    fn the_default_file_and_an_empty_file_give_the_defaults() {
        let home = Path::new("/home/u");
        let defaults = validate("", &env(home)).unwrap();
        assert_eq!(validate(&template(), &env(home)).unwrap(), defaults);
        assert_eq!(defaults.files_root.path, home);
        assert!(defaults.files_extra_roots.is_empty());
        assert_eq!(defaults.projects_base.path, home.join("projects"));
        assert_eq!(defaults.scratch_folder.written, "~/scratch");
        // Every key is in the default file, commented out at its default.
        for key in KEYS {
            assert!(
                template().contains(&format!("\n# {} = {}\n", key.name, key.default)),
                "{}",
                key.path()
            );
            assert!(!toml_json(key.default).is_null(), "{}", key.path());
        }
    }

    #[test]
    fn values_are_read_and_expanded() {
        let config = validate(
            "version = 1\n[files]\nroot = \"/srv/./work/\"\nextra_roots = [\"/mnt/data\", \"~/../x\"]\n",
            &env(Path::new("/home/u")),
        );
        let errors = config.unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].key.as_deref(), Some("files.extra_roots"));
        assert_eq!((errors[0].line, errors[0].column), (4, 29));
        let config = validate(
            "[files]\nroot = \"/srv/./work/\"\nextra_roots = [\"/mnt/data\"]\n[projects]\nbase = \"/mnt/data/p\"\n[scratch]\nfolder = \"/srv/work/s\"\n",
            &env(Path::new("/home/u")),
        )
        .unwrap();
        assert_eq!(config.files_root.path, Path::new("/srv/work"));
        assert_eq!(config.files_root.written, "/srv/./work/");
        assert_eq!(config.roots().1, vec![PathBuf::from("/mnt/data")]);
        // Dotted keys and inline tables are tables too.
        validate("files.root = \"/srv\"\nscratch = { folder = \"/srv/s\" }\nprojects.base = \"/srv/p\"\n", &env(Path::new("/home/u"))).unwrap();
    }

    #[test]
    fn github_settings_are_read_and_checked() {
        let home = Path::new("/home/u");
        let config = validate("", &env(home)).unwrap();
        assert_eq!(config.github, GitHubSettings::default());
        let config = validate(
            "[github]\ntoken_command = \"~/bin/github-token\"\nssh_auth_sock = \"/run/user/1/agent\"\n",
            &env(home),
        )
        .unwrap();
        assert_eq!(
            config.github,
            GitHubSettings {
                auth: Some(GitHubAuth::Command("/home/u/bin/github-token".into())),
                ssh_auth_sock: Some("/run/user/1/agent".into()),
            }
        );
        let auth = |text: &str| validate(text, &env(home)).unwrap().github.auth;
        assert_eq!(auth("[github]\nauth = \"gh\"\n"), Some(GitHubAuth::Gh));
        assert_eq!(auth("github.auth = \"env\"\n"), Some(GitHubAuth::Env));
        // Empty strings leave a key unset.
        assert_eq!(auth("[github]\nauth = \"\"\ntoken_command = \"\"\n"), None);
        let problems = messages(
            "[github]\nauth = \"token\"\ntoken_command = \"bin/t\"\nssh_auth_sock = \"~x\"\nowner = \"x\"\n",
        );
        for expected in [
            "2:8: `github.auth` must be",
            "3:17: `github.token_command` must be an absolute path",
            "4:17: `github.ssh_auth_sock` only `~` and `~/…` are expanded",
            "5:1: unknown key `github.owner`",
        ] {
            assert!(
                problems.iter().any(|p| p.starts_with(expected)),
                "{expected} in {problems:?}"
            );
        }
        let problems = messages("[github]\nauth = \"gh\"\ntoken_command = \"/t\"\n");
        assert!(
            problems.iter().any(|p| p.contains("not both")),
            "{problems:?}"
        );
        // The web client cannot change them.
        assert!(changes(&json!({"github.token_command": "/bin/sh"})).is_err());
    }

    #[test]
    fn errors_have_positions_and_suggestions() {
        assert_eq!(
            messages("[files]\nrot = \"~\"\n"),
            ["2:1: unknown key `files.rot`; did you mean `files.root`?"]
        );
        assert_eq!(
            messages("root = \"~\"\n"),
            ["1:1: unknown key `root`; did you mean `files.root`?"]
        );
        assert_eq!(
            messages("[file]\n"),
            ["1:2: unknown key `file`; did you mean `files`?"]
        );
        assert_eq!(
            messages("[files]\nbase = \"~/p\"\n"),
            ["2:1: unknown key `files.base`; did you mean `projects.base`?"]
        );
        assert_eq!(messages("[nonsense]\n"), ["1:2: unknown key `nonsense`"]);
        assert_eq!(
            messages("[files]\nroot = 3\n"),
            ["2:8: `files.root` must be a folder path (string)"]
        );
        assert_eq!(
            messages("[files]\nextra_roots = \"/mnt\"\n"),
            ["2:15: `files.extra_roots` must be a list of folder paths (strings)"]
        );
        assert_eq!(
            messages("[scratch]\nfolder = \"relative\"\n"),
            ["2:10: `scratch.folder` must be an absolute path or start with `~/`"]
        );
        assert_eq!(
            messages("[scratch]\nfolder = \"~bob/x\"\n"),
            ["2:10: `scratch.folder` only `~` and `~/…` are expanded; write an absolute path"]
        );
        assert_eq!(
            messages("version = 2\n"),
            ["1:11: this file is for a newer KelpMux (version 2); this one reads version 1"]
        );
        let syntax = messages("[files]\nroot = \"~\n");
        assert_eq!(syntax.len(), 1);
        assert!(syntax[0].starts_with("2:"), "{syntax:?}");
        assert!(syntax[0].contains("invalid TOML"), "{syntax:?}");
    }

    #[test]
    fn folders_must_lie_inside_the_roots() {
        assert_eq!(
            messages("[projects]\nbase = \"/srv/p\"\n"),
            [
                "2:8: `projects.base` (/srv/p) must lie inside `files.root` (/home/u) or one of `files.extra_roots`"
            ]
        );
        // An extra root admits them.
        validate(
            "[files]\nextra_roots = [\"/srv\"]\n[projects]\nbase = \"/srv/p\"\n",
            &env(Path::new("/home/u")),
        )
        .unwrap();
        assert_eq!(
            messages("[files]\nextra_roots = [\"/mnt\", \"/mnt/\", \"~/x\"]\n"),
            [
                "2:24: `files.extra_roots` lists /mnt twice",
                "2:33: `files.extra_roots`: /home/u/x is already inside `files.root` (/home/u)"
            ]
        );
        // A sibling with a common prefix is not inside.
        assert_eq!(
            messages(
                "[files]\nroot = \"/srv/a\"\n[projects]\nbase = \"/srv/ab\"\n[scratch]\nfolder = \"/srv/a/s\"\n"
            ),
            [
                "4:8: `projects.base` (/srv/ab) must lie inside `files.root` (/srv/a) or one of `files.extra_roots`"
            ]
        );
    }

    #[test]
    fn roots_must_exist_when_checked() {
        let temp = Temp::new("exists");
        let checked = Env {
            home: temp.0.clone(),
            check_dirs: true,
        };
        validate("", &checked).unwrap();
        let errors = validate(
            "[files]\nextra_roots = [\"/nonexistent-kelpmux\"]\n",
            &checked,
        )
        .unwrap_err();
        assert_eq!(
            errors[0].message,
            "`files.extra_roots`: /nonexistent-kelpmux does not exist or is not a folder"
        );
    }

    #[test]
    fn edits_keep_comments_and_land_under_their_documentation() {
        let home = Path::new("/home/u");
        let changes = changes(&json!({
            "files.extra_roots": ["/mnt/data"],
            "scratch.folder": "/mnt/data/scratch",
        }))
        .unwrap();
        let text = edit(&template(), &changes, &env(home)).unwrap();
        assert!(
            text.contains(
                "# Example: extra_roots = [\"/mnt/data\"]\nextra_roots = [\"/mnt/data\"]\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("\nfolder = \"/mnt/data/scratch\"\n"),
            "{text}"
        );
        assert!(!text.contains("# folder = \"~/scratch\""), "{text}");
        // Untouched keys stay commented out, documentation and all.
        assert!(text.contains("\n# root = \"~\"\n"));
        assert_eq!(text.lines().count(), template().lines().count(), "{text}");
        let config = validate(&text, &env(home)).unwrap();
        assert_eq!(config.scratch_folder.path, Path::new("/mnt/data/scratch"));

        // A hand-written file keeps its own comments, order and formatting.
        let hand = "# mine\n[scratch]\nfolder   =   \"~/s\"   # where shells start\n\n[files] # boundary\nroot = \"~\"\n";
        let changes =
            super::changes(&json!({"scratch.folder": "~/t", "projects.base": "~/p"})).unwrap();
        let text = edit(hand, &changes, &env(home)).unwrap();
        assert_eq!(
            text,
            "# mine\n[scratch]\nfolder   =   \"~/t\"   # where shells start\n\n[files] # boundary\nroot = \"~\"\n\n[projects]\nbase = \"~/p\"\n"
        );
    }

    #[test]
    fn invalid_edits_and_unknown_changes_are_refused() {
        let home = Path::new("/home/u");
        let changes = changes(&json!({"projects.base": "/elsewhere"})).unwrap();
        let errors = edit("", &changes, &env(home)).unwrap_err();
        assert_eq!(errors[0].key.as_deref(), Some("projects.base"));
        // A file that does not parse is not rewritten.
        assert!(
            edit(
                "[files\n",
                &super::changes(&json!({"files.root": "~"})).unwrap(),
                &env(home)
            )
            .is_err()
        );
        assert!(super::changes(&json!({"version": 2})).is_err());
        assert!(super::changes(&json!({"files.nope": "x"})).is_err());
        assert!(super::changes(&json!({"files.root": ["x"]})).is_err());
        assert!(super::changes(&json!({})).is_err());
    }

    #[test]
    fn updates_create_the_file_and_follow_links() {
        let temp = Temp::new("update");
        let home = temp.0.join("home");
        fs::create_dir_all(&home).unwrap();
        let env = Env {
            home: home.clone(),
            check_dirs: true,
        };
        let path = temp.0.join("config/kelpmux/config.toml");
        let changes = changes(&json!({"scratch.folder": "~/s"})).unwrap();
        update(&path, &changes, &env).unwrap().unwrap();
        let text = read(&path).unwrap().unwrap();
        assert!(text.contains("\nfolder = \"~/s\"\n"));
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        // A link stays a link; its target changes.
        let real = temp.0.join("real.toml");
        fs::rename(&path, &real).unwrap();
        std::os::unix::fs::symlink(&real, &path).unwrap();
        let changes = super::changes(&json!({"scratch.folder": "~/t"})).unwrap();
        update(&path, &changes, &env).unwrap().unwrap();
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(read(&real).unwrap().unwrap().contains("folder = \"~/t\""));
        // An invalid change leaves the file alone.
        let before = read(&real).unwrap();
        let changes = super::changes(&json!({"files.root": "~/missing"})).unwrap();
        assert!(update(&path, &changes, &env).unwrap().is_err());
        assert_eq!(read(&real).unwrap(), before);
        // No temporary file is left behind.
        let leftovers: Vec<_> = fs::read_dir(&temp.0)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn concurrent_updates_all_land() {
        let temp = Temp::new("concurrent");
        let home = temp.0.join("home");
        fs::create_dir_all(&home).unwrap();
        let env = Env {
            home: home.clone(),
            check_dirs: true,
        };
        let path = temp.0.join("config.toml");
        let keys = ["projects.base", "scratch.folder"];
        let threads: Vec<_> = keys
            .iter()
            .map(|key| {
                let (path, env) = (path.clone(), env.clone());
                let key = (*key).to_owned();
                std::thread::spawn(move || {
                    let changes = changes(&json!({ key: "~/x" })).unwrap();
                    update(&path, &changes, &env).unwrap().unwrap();
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let config = validate(&read(&path).unwrap().unwrap(), &env).unwrap();
        assert_eq!(config.projects_base.written, "~/x");
        assert_eq!(config.scratch_folder.written, "~/x");
    }

    #[test]
    fn a_live_view_keeps_the_last_valid_configuration_and_pushes_changes() {
        let temp = Temp::new("live");
        let home = temp.0.join("home");
        fs::create_dir_all(&home).unwrap();
        let env = Env {
            home: home.clone(),
            check_dirs: true,
        };
        let path = temp.0.join("kelpmux/config.toml");
        let (sender, receiver) = std::sync::mpsc::channel::<Json>();
        let sender = Mutex::new(sender);
        let live = Live::start(
            path.clone(),
            env,
            Box::new(move |value| sender.lock().unwrap().send(value).is_ok()),
        );
        assert_eq!(live.json()["exists"], false);
        assert_eq!(live.json()["values"]["files"]["root"], "~");
        // Created after the bridge started: the folder is watched once it exists.
        ensure_exists(&path).unwrap();
        let wait = |check: &dyn Fn(&Json) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let value = receiver
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .expect("a pushed configuration");
                if check(&value["Config"]) {
                    return value["Config"].clone();
                }
            }
        };
        wait(&|config| config["exists"] == true);
        fs::write(&path, "[scratch]\nfolder = \"~/s\"\n").unwrap();
        let config = wait(&|config| config["values"]["scratch"]["folder"] == "~/s");
        assert_eq!(config["current"], true);
        // An invalid edit keeps the last valid values and reports where.
        fs::write(&path, "[scratch]\nfolder = \"~/s\"\nfloder = 1\n").unwrap();
        let config = wait(&|config| !config["errors"].as_array().unwrap().is_empty());
        assert_eq!(config["current"], false);
        assert_eq!(config["values"]["scratch"]["folder"], "~/s");
        assert_eq!(config["errors"][0]["line"], 3);
        assert_eq!(
            config["errors"][0]["message"],
            "unknown key `scratch.floder`; did you mean `scratch.folder`?"
        );
        // A save from the web client is validated and written.
        let saved = live.set(&json!({"projects.base": "/elsewhere"})).unwrap();
        assert_eq!(saved["saved"], false);
        fs::write(&path, "").unwrap();
        wait(&|config| config["current"] == true);
        let saved = live.set(&json!({"projects.base": "~/p"})).unwrap();
        assert_eq!(saved["saved"], true, "{saved}");
        assert_eq!(saved["config"]["values"]["projects"]["base"], "~/p");
    }

    #[test]
    fn projects_are_read_and_checked() {
        let home = Path::new("/home/u");
        let config = validate(
            "[[project]]\nid = \"kelp\"\nname = \"Kelp\"\nfolder = \"~/projects/kelp\"\ncolor = \"teal\"\n\n[[project]]\nid = \"web\"\nname = \"Web site\"\nfolder = \"~/work/web\"\n",
            &env(home),
        )
        .unwrap();
        assert_eq!(config.projects.len(), 2);
        assert_eq!(config.projects[0].folder.path, home.join("projects/kelp"));
        assert_eq!(config.projects[0].color.as_deref(), Some("teal"));
        assert_eq!(config.project("web").unwrap().name, "Web site");
        let json = config.to_json();
        assert_eq!(json["projects"][1]["id"], "web");
        assert_eq!(json["projects"][1]["missing"], true);
        // Inline tables are tables too.
        validate(
            "project = [{ id = \"a\", name = \"A\", folder = \"~/a\" }]\n",
            &env(home),
        )
        .unwrap();
        assert_eq!(
            messages(
                "[[project]]\nid = \"a\"\nname = \"A\"\nfolder = \"~/p/a\"\n[[project]]\nid = \"a\"\nname = \"a\"\nfolder = \"~/p/a/sub\"\nfoldr = \"x\"\n"
            ),
            [
                "9:1: unknown key `project.foldr`; did you mean `project.folder`?",
                "6:6: project “a”: another project already has the id `a`",
                "7:8: another project is already named “a”",
                "8:10: project “a”: the folder /home/u/p/a/sub is inside the folder of project “A” (/home/u/p/a)",
            ]
        );
        assert_eq!(
            messages(
                "[[project]]\nid = \"Bad\"\nname = \"x/y\"\nfolder = \"/srv\"\ncolor = \"plaid\"\n[[project]]\nname = \"B\"\nfolder = \"~\"\n"
            ),
            [
                "2:6: project “x/y”: `id` must be 1–64 lowercase letters, digits and `-`, starting and ending with a letter or digit",
                "3:8: project “x/y”: the name must not contain `/` or control characters",
                "5:9: project “x/y”: `color` must be one of red, orange, yellow, green, teal, blue, purple, pink, gray or `#rrggbb`",
                "4:10: project “x/y”: the folder /srv must lie inside `files.root` (/home/u) or one of `files.extra_roots`",
                "6:1: project “B” needs `id`",
            ]
        );
        // The Scratch folder is a project's folder too.
        assert_eq!(
            messages("[[project]]\nid = \"s\"\nname = \"S\"\nfolder = \"~/scratch/s\"\n"),
            [
                "4:10: project “S”: the folder /home/u/scratch/s overlaps the Scratch folder (/home/u/scratch)"
            ]
        );
        assert_eq!(
            messages("[[project]]\nid = \"scratch\"\nname = \"Scratch\"\nfolder = \"~/x\"\n"),
            [
                "2:6: project “Scratch”: `id` `scratch` is the built-in Scratch project's",
                "3:8: project “Scratch”: the name `Scratch` is the built-in project's name",
            ]
        );
    }

    #[test]
    fn project_changes_keep_the_file_and_are_validated() {
        let home = Path::new("/home/u");
        let create = |id: &str, name: &str, folder: &str| ProjectChange::Create {
            id: id.into(),
            name: name.into(),
            folder: folder.into(),
            color: None,
        };
        // Added after the documented example at the end of the default file.
        let text = edit_projects(
            &template(),
            &create("kelp", "Kelp", "~/projects/Kelp"),
            &env(home),
        )
        .unwrap();
        assert!(text.starts_with(&template()), "{text}");
        assert!(
            text.ends_with(
                "\n[[project]]\nid = \"kelp\"\nname = \"Kelp\"\nfolder = \"~/projects/Kelp\"\n"
            ),
            "{text}"
        );
        let text =
            edit_projects(&text, &create("web", "Web", "~/projects/Web"), &env(home)).unwrap();
        assert_eq!(validate(&text, &env(home)).unwrap().projects.len(), 2);
        // Overlapping folders are refused with the new project's key.
        let errors = edit_projects(
            &text,
            &create("sub", "Sub", "~/projects/Kelp/sub"),
            &env(home),
        )
        .unwrap_err();
        assert_eq!(errors[0].key.as_deref(), Some("project[2].folder"));
        // Rename and recolour keep the id, folder and comments.
        let hand = "# mine\n[[project]] # first\nid = \"kelp\"\nname = \"Kelp\"   # shown\nfolder = \"~/k\"\ncolor = \"red\"\n";
        let renamed = edit_projects(
            hand,
            &ProjectChange::Update {
                id: "kelp".into(),
                name: Some("Kelp 2".into()),
                color: Some(None),
            },
            &env(home),
        )
        .unwrap();
        assert_eq!(
            renamed,
            "# mine\n[[project]] # first\nid = \"kelp\"\nname = \"Kelp 2\"   # shown\nfolder = \"~/k\"\n"
        );
        let deleted = edit_projects(
            &renamed,
            &ProjectChange::Delete { id: "kelp".into() },
            &env(home),
        )
        .unwrap();
        assert!(validate(&deleted, &env(home)).unwrap().projects.is_empty());
        assert!(
            edit_projects(
                hand,
                &ProjectChange::Delete { id: "nope".into() },
                &env(home)
            )
            .is_err()
        );
        // An inline array grows inline.
        let inline = "project = [{ id = \"a\", name = \"A\", folder = \"~/a\" }]\n";
        let text = edit_projects(inline, &create("b", "B", "~/b"), &env(home)).unwrap();
        assert_eq!(validate(&text, &env(home)).unwrap().projects.len(), 2);
        assert!(text.starts_with("project = ["), "{text}");
    }

    #[test]
    fn project_ids_come_from_names_and_are_never_reused() {
        assert_eq!(new_project_id("Kelp Mux!", &[]), "kelp-mux");
        assert_eq!(new_project_id("Élan  vital", &[]), "lan-vital");
        assert_eq!(new_project_id("???", &[]), "project");
        assert_eq!(new_project_id("Scratch", &[]), "project-scratch");
        let taken = ["kelp".to_owned(), "kelp-2".to_owned()];
        assert_eq!(new_project_id("Kelp", &taken), "kelp-3");
        assert!(id_problem(&new_project_id(&"x".repeat(200), &[])).is_none());
    }

    #[test]
    fn the_reference_in_docs_is_current() {
        let docs = include_str!("../../docs/config.md");
        let begin = "<!-- BEGIN kelpmux config docs -->\n";
        let end = "<!-- END kelpmux config docs -->";
        let start = docs.find(begin).expect("generated reference marker") + begin.len();
        let stop = docs.find(end).expect("generated reference end marker");
        assert_eq!(
            &docs[start..stop],
            reference(),
            "docs/config.md is out of date: replace its reference with `kelpmux config docs`"
        );
    }

    #[test]
    fn the_schema_lists_every_key() {
        let schema = schema();
        for key in KEYS {
            let property = if key.section.is_empty() {
                &schema["properties"][key.name]
            } else {
                &schema["properties"][key.section]["properties"][key.name]
            };
            assert!(property.is_object(), "{}", key.path());
            assert!(!property["default"].is_null(), "{}", key.path());
        }
        assert_eq!(schema["additionalProperties"], false);
    }
}
