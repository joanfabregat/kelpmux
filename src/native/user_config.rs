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
}
impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Version => "integer",
            Self::Folder => "folder path (string)",
            Self::FolderList => "list of folder paths (strings)",
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
        applies: "Immediately. Stored for project support in a later release; nothing uses it yet.",
    },
    Key {
        section: "scratch",
        name: "folder",
        kind: Kind::Folder,
        default: "\"~/scratch\"",
        example: None,
        doc: "The folder terminals outside any project start in. It must lie inside `files.root` or one of `files.extra_roots`, and need not exist yet.",
        applies: "Immediately. Stored for a later release; nothing uses it yet.",
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

/// A valid configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub files_root: Folder,
    pub files_extra_roots: Vec<Folder>,
    pub projects_base: Folder,
    pub scratch_folder: Folder,
}
impl Config {
    /// The values as written, and resolved, in the file's shape.
    pub fn to_json(&self) -> Json {
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
        json!({
            "values": shape(&|folder| json!(folder.written)),
            "resolved": shape(&|folder| json!(folder.path)),
        })
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
    let top_level = || {
        KEYS.iter()
            .filter(|key| key.section.is_empty())
            .map(|key| key.name)
            .chain(SECTIONS.iter().map(|(name, _)| *name))
    };
    for (name, item) in document.iter() {
        let key_span = document.key(name).and_then(|key| key.span());
        if let Some(section) = SECTIONS.iter().find(|(section, _)| *section == name) {
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
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Config {
        files_root: root,
        files_extra_roots: extra,
        projects_base: base,
        scratch_folder: scratch,
    })
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
            Kind::Folder => match item.as_str() {
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
            Kind::FolderList => json!({
                "type": "array",
                "items": { "type": "string", "minLength": 1, "maxLength": MAX_PATH },
                "maxItems": MAX_EXTRA_ROOTS,
                "uniqueItems": true,
            }),
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
            let key = key(name)
                .filter(|key| key.kind != Kind::Version)
                .ok_or_else(|| format!("`{name}` cannot be changed here"))?;
            let value = match (key.kind, value) {
                (Kind::Folder, Json::String(text)) => vec![text.clone()],
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
struct WriteLock(#[allow(dead_code)] File);
impl WriteLock {
    fn acquire(target: &Path) -> Result<Self> {
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
                return Err("another KelpMux is writing the configuration; try again".into());
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
fn replace(target: &Path, text: &str) -> Result<()> {
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
        assert!(!text.contains("# folder = "), "{text}");
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
