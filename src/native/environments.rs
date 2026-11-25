//! Environments ([ADR 0005](../../docs/adr/0005-environments.md)): dev
//! servers and deployments a project's repositories declare, started and
//! stopped only on an explicit request from the web client or `kelpmux env`.
//!
//! - **Declaration.** `.kelp/environments/<name>/environment.toml` (plus its
//!   scripts) in the project folder or one of its checkouts (the Repos
//!   view's discovery: the folder and its direct subfolders that are Git
//!   checkouts). Every directory on the way and every file read or run must
//!   be one only the user can change (the Repos view's rule), and not a
//!   symbolic link; the manifest is validated strictly ([`parse_manifest`]).
//! - **Instances.** One per (project, environment). A `process` environment
//!   is one long-running command in a dedicated KelpServer pane; a `deploy`
//!   environment's `up` and `down` scripts run in a short-lived pane each,
//!   and its `status` script is asked for its state only on an explicit
//!   check: listing never runs anything a repository declares. The pane
//!   runs `kelpmux env exec` ([`exec`]), which re-reads the manifest, sets
//!   the `KELP_*` variables, runs the command, and records how it ended in
//!   the instance's state folder (`run.json`).
//! - **State.** Ports ([`Ports`]) and instance folders live under
//!   `$XDG_STATE_HOME/kelpmux/environments/`; which pane runs which
//!   environment lives in the workspace's state folder ([`Instances`]).
//!   Both are written under a `flock` with an atomic replace.
use super::{
    projects::Links,
    session::{Session, request_id},
    stream::{Event, Request},
    user_config,
};
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        process::{CommandExt, ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// The manifest's file name in an environment's folder.
pub const MANIFEST: &str = "environment.toml";
/// Where environments are declared, relative to a repository.
pub const DECLARED_IN: &str = ".kelp/environments";
/// Most environments listed per project.
pub const MAX_ENVIRONMENTS: usize = 64;
/// Most entries read from one `.kelp/environments` folder.
const MAX_ENTRIES: usize = 256;
/// Largest accepted manifest.
const MAX_MANIFEST: u64 = 16 * 1024;
/// Longest environment name.
pub const MAX_NAME: usize = 32;
/// Longest DNS label, hence slug.
pub const MAX_SLUG: usize = 63;
/// The default `environments.ports` range.
pub const DEFAULT_PORTS: (u16, u16) = (41000, 41999);
/// How long a `status` script may run.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest `status` output read.
const STATUS_OUTPUT: usize = 64 * 1024;
/// How long Stop waits for a process environment to exit after SIGTERM
/// before its pane is closed (which kills what is left).
pub const STOP_GRACE: Duration = Duration::from_secs(10);
/// Most ports remembered.
const MAX_PORTS: usize = 4096;
/// Most pane records kept per workspace.
const MAX_INSTANCES: usize = 1024;
/// Largest state file read.
const MAX_STATE: u64 = 1024 * 1024;

// ───────────────────────── Manifest ─────────────────────────

/// What an environment is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// One long-running command (a dev server).
    Process,
    /// `up` and `down` scripts that exit, and a `status` script.
    Deploy,
}

/// How a process environment says it is ready.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ready {
    /// A TCP port accepts connections on the loopback (`None`: `KELP_PORT`).
    Port(Option<u16>),
    /// An HTTP URL on the loopback answers with a status below 500.
    Http(String),
}

/// A validated `environment.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub kind: Kind,
    pub description: Option<String>,
    /// Where the environment is reached, with `${KELP_PORT}` and
    /// `${KELP_SLUG}` substituted when shown.
    pub url: Option<String>,
    /// `process`: the command, run with `/bin/sh -c`.
    pub command: Option<String>,
    pub ready: Ready,
    /// `deploy`: script file names in the environment's folder.
    pub up: Option<String>,
    pub down: Option<String>,
    pub status: Option<String>,
    pub logs: Option<String>,
}

/// An environment or script name: lowercase letters, digits and `-`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

fn valid_script(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `${KELP_PORT}` and `${KELP_SLUG}` in a URL.
pub fn substitute(text: &str, port: u16, slug: &str) -> String {
    text.replace("${KELP_PORT}", &port.to_string())
        .replace("${KELP_SLUG}", slug)
}

/// A readiness URL: `http://` on `127.0.0.1`, `localhost` or `[::1]`, with
/// an optional port and path. Returns (connect address, Host, path).
pub fn loopback_http(url: &str) -> Option<(Vec<SocketAddr>, String, String)> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if path.bytes().any(|b| b <= b' ' || b == 0x7f) || authority.contains('@') {
        return None;
    }
    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        let (host, after) = inner.split_once(']')?;
        let port = match after {
            "" => 80,
            _ => after.strip_prefix(':')?.parse().ok()?,
        };
        (host, port)
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, port.parse().ok()?),
            None => (authority, 80),
        }
    };
    if port == 0 {
        return None;
    }
    let addresses: Vec<SocketAddr> = match host {
        "127.0.0.1" => vec![SocketAddr::from(([127, 0, 0, 1], port))],
        "::1" => vec![SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port))],
        "localhost" => vec![
            SocketAddr::from(([127, 0, 0, 1], port)),
            SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)),
        ],
        _ => return None,
    };
    Some((addresses, authority.to_owned(), path.to_owned()))
}

/// Parses and validates a manifest. Unknown keys, wrong types, keys of the
/// other kind and missing required keys are errors.
pub fn parse_manifest(text: &str) -> std::result::Result<Manifest, String> {
    let document = toml_edit::Document::parse(text).map_err(|error| {
        format!(
            "invalid TOML: {}",
            error
                .message()
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join("; ")
        )
    })?;
    let string = |item: &toml_edit::Item, key: &str, limit: usize| {
        let text = item
            .as_str()
            .ok_or_else(|| format!("`{key}` must be a string"))?;
        if text.is_empty() || text.len() > limit || text.contains('\0') {
            return Err(format!("`{key}` must be 1–{limit} bytes"));
        }
        Ok(text.to_owned())
    };
    let mut kind = None;
    let mut description = None;
    let mut url = None;
    let mut command = None;
    let mut ready = None;
    let mut scripts = None;
    for (key, item) in document.iter() {
        match key {
            "kind" => {
                kind = Some(match item.as_str() {
                    Some("process") => Kind::Process,
                    Some("deploy") => Kind::Deploy,
                    _ => return Err("`kind` must be \"process\" or \"deploy\"".into()),
                })
            }
            "description" => {
                let text = string(item, key, 200)?;
                if text.chars().any(char::is_control) {
                    return Err("`description` must be one line".into());
                }
                description = Some(text);
            }
            "url" => {
                let text = string(item, key, 2048)?;
                if !(text.starts_with("http://") || text.starts_with("https://"))
                    || text.bytes().any(|b| b <= b' ' || b == 0x7f)
                {
                    return Err("`url` must be an http:// or https:// URL".into());
                }
                url = Some(text);
            }
            "command" => command = Some(string(item, key, 4096)?),
            "ready" => {
                let table = item
                    .as_table_like()
                    .ok_or("`ready` must be a table (`[ready]`)")?;
                let mut found = None;
                for (name, value) in table.iter() {
                    if found.is_some() {
                        return Err("`ready` takes one of `port` or `http`".into());
                    }
                    found = Some(match name {
                        "port" => Ready::Port(Some(
                            value
                                .as_integer()
                                .and_then(|port| u16::try_from(port).ok())
                                .filter(|port| *port > 0)
                                .ok_or("`ready.port` must be a port number (1–65535)")?,
                        )),
                        "http" => {
                            let text = string(value, "ready.http", 2048)?;
                            if loopback_http(&substitute(&text, 1, "slug")).is_none() {
                                return Err("`ready.http` must be an http:// URL on 127.0.0.1, localhost or [::1]".into());
                            }
                            Ready::Http(text)
                        }
                        other => return Err(format!("unknown key `ready.{other}`")),
                    });
                }
                ready = Some(found.ok_or("`ready` takes one of `port` or `http`")?);
            }
            "scripts" => {
                let table = item
                    .as_table_like()
                    .ok_or("`scripts` must be a table (`[scripts]`)")?;
                let mut found: BTreeMap<&str, String> = BTreeMap::new();
                for (name, value) in table.iter() {
                    if !matches!(name, "up" | "down" | "status" | "logs") {
                        return Err(format!("unknown key `scripts.{name}`"));
                    }
                    let script = string(value, &format!("scripts.{name}"), 64)?;
                    if !valid_script(&script) {
                        return Err(format!(
                            "`scripts.{name}` must be a file name in the environment's folder"
                        ));
                    }
                    found.insert(name, script);
                }
                scripts = Some(found);
            }
            other => return Err(format!("unknown key `{other}`")),
        }
    }
    let kind = kind.ok_or("`kind` is required")?;
    match kind {
        Kind::Process => {
            if scripts.is_some() {
                return Err("a process environment has no `[scripts]`".into());
            }
            Ok(Manifest {
                kind,
                description,
                url,
                command: Some(command.ok_or("a process environment needs `command`")?),
                ready: ready.unwrap_or(Ready::Port(None)),
                up: None,
                down: None,
                status: None,
                logs: None,
            })
        }
        Kind::Deploy => {
            if command.is_some() || ready.is_some() {
                return Err("a deploy environment has no `command` or `[ready]`".into());
            }
            let mut scripts = scripts.ok_or("a deploy environment needs `[scripts]`")?;
            let mut take = |name: &str| scripts.remove(name);
            let (up, down, status, logs) = (take("up"), take("down"), take("status"), take("logs"));
            if up.is_none() || down.is_none() || status.is_none() {
                return Err("`[scripts]` needs `up`, `down` and `status`".into());
            }
            Ok(Manifest {
                kind,
                description,
                url,
                command: None,
                ready: Ready::Port(None),
                up,
                down,
                status,
                logs,
            })
        }
    }
}

// ───────────────────────── Names and paths ─────────────────────────

/// A DNS-safe `<project>-<env>` (lowercase letters, digits and `-`, at
/// most 63). A longer one keeps its start and ends with a hash of the full
/// name, so it stays unique.
pub fn slug(project: &str, env: &str) -> String {
    let mut slug = String::new();
    for c in format!("{project}-{env}").chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let mut slug = slug.trim_end_matches('-').to_owned();
    if slug.is_empty() {
        slug = "env".into();
    }
    if slug.len() > MAX_SLUG {
        let hash = fnv(format!("{project}\0{env}").as_bytes());
        slug.truncate(MAX_SLUG - 9);
        let base = slug.trim_end_matches('-').to_owned();
        slug = format!("{base}-{:08x}", hash as u32);
    }
    slug
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

/// `$XDG_STATE_HOME/kelpmux/environments` (default
/// `~/.local/state/kelpmux/environments`).
pub fn state_base() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        Some(base) => base,
        None => user_config::Env::current()?.home.join(".local/state"),
    };
    Ok(base.join("kelpmux").join("environments"))
}

fn private_dir(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    Ok(())
}

// ───────────────────────── Discovery ─────────────────────────

/// A directory (not a symbolic link) only this user can change: owned by
/// `uid`, not writable by others or by a group other than the user's
/// private one, under folders no one else can change (the Repos view's
/// rule, [`super::web_git::trusted_path`]).
fn trusted_dir(path: &Path, uid: u32) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) && super::web_git::trusted_path(path, uid)
}

/// A regular file (not a symbolic link) only this user can change, as
/// [`trusted_dir`].
fn trusted_file(path: &Path, uid: u32) -> std::result::Result<fs::Metadata, String> {
    let meta = fs::symlink_metadata(path).map_err(|_| "missing".to_owned())?;
    if !meta.file_type().is_file() {
        return Err("not a regular file (symbolic links are refused)".into());
    }
    if meta.uid() != uid {
        return Err("owned by another user".into());
    }
    if !super::web_git::trusted_path(path, uid) {
        return Err("writable by another user or group".into());
    }
    Ok(meta)
}

fn read_manifest(path: &Path, uid: u32) -> std::result::Result<String, String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| format!("{MANIFEST} is missing or a symbolic link"))?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.uid() != uid || !super::web_git::trusted_path(path, uid) {
        return Err(format!(
            "{MANIFEST} must be a regular file only this user can change"
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return Err(format!("{MANIFEST} is larger than 16 KiB"));
    }
    String::from_utf8(bytes).map_err(|_| format!("{MANIFEST} is not UTF-8"))
}

/// An environment found in a project.
#[derive(Clone, Debug)]
pub struct Declared {
    pub name: String,
    /// The repository (or project folder) that declares it: the scripts'
    /// working directory.
    pub root: PathBuf,
    /// Its folder, `<root>/.kelp/environments/<name>`.
    pub dir: PathBuf,
    /// The manifest, or why it cannot be used.
    pub manifest: std::result::Result<Manifest, String>,
}

/// Reads one environment folder: its manifest, checked, and its scripts,
/// which must be executable files of `uid` in that folder.
pub fn load(root: &Path, name: &str, uid: u32) -> Declared {
    let dir = root.join(DECLARED_IN).join(name);
    let manifest = (|| {
        if !valid_name(name) {
            return Err(format!(
                "invalid name: lowercase letters, digits and `-`, at most {MAX_NAME}"
            ));
        }
        if !super::web_git::trusted_path(root, uid) {
            return Err(format!(
                "{} must be a folder only this user can change",
                root.display()
            ));
        }
        let mut walked = root.to_path_buf();
        for part in [".kelp", "environments", name] {
            walked.push(part);
            if !trusted_dir(&walked, uid) {
                return Err(format!(
                    "{} must be a folder only this user can change (not a symbolic link)",
                    walked.display()
                ));
            }
        }
        let manifest = parse_manifest(&read_manifest(&dir.join(MANIFEST), uid)?)?;
        for (key, script) in [
            ("up", &manifest.up),
            ("down", &manifest.down),
            ("status", &manifest.status),
            ("logs", &manifest.logs),
        ] {
            if let Some(script) = script {
                let meta = trusted_file(&dir.join(script), uid)
                    .map_err(|why| format!("script `{key}` ({script}): {why}"))?;
                if meta.mode() & 0o100 == 0 {
                    return Err(format!("script `{key}` ({script}) is not executable"));
                }
            }
        }
        Ok(manifest)
    })();
    Declared {
        name: name.to_owned(),
        root: root.to_path_buf(),
        dir,
        manifest,
    }
}

/// The folders environments are looked for in: the project folder, then
/// its checkouts (the Repos view's discovery).
pub fn roots(folder: &Path, uid: u32) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if fs::metadata(folder).is_ok_and(|m| m.is_dir() && m.uid() == uid) {
        roots.push(folder.to_path_buf());
    }
    for checkout in super::web_git::discover(folder, uid).0 {
        if !roots.contains(&checkout) {
            roots.push(checkout);
        }
    }
    roots
}

/// Every environment of a project folder, by name. A name declared in
/// several repositories is reported with an error on each, and cannot run.
pub fn discover(folder: &Path, uid: u32) -> (Vec<Declared>, bool) {
    let mut found: Vec<Declared> = Vec::new();
    let mut truncated = false;
    for root in roots(folder, uid) {
        let base = root.join(DECLARED_IN);
        if !trusted_dir(&root.join(".kelp"), uid) || !trusted_dir(&base, uid) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&base) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .take(MAX_ENTRIES)
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let name = entry.file_name().into_string().ok()?;
                (!name.starts_with('.') && entry.file_type().ok()?.is_dir()).then_some(name)
            })
            .collect();
        names.sort();
        for name in names {
            if found.len() >= MAX_ENVIRONMENTS {
                truncated = true;
                break;
            }
            found.push(load(&root, &name, uid));
        }
    }
    let roots_of = |name: &str, found: &[Declared]| -> Vec<PathBuf> {
        found
            .iter()
            .filter(|d| d.name == name)
            .map(|d| d.root.clone())
            .collect()
    };
    let snapshot = found.clone();
    for declared in &mut found {
        let roots = roots_of(&declared.name, &snapshot);
        if roots.len() > 1 {
            declared.manifest = Err(format!(
                "declared in several repositories: {}",
                roots
                    .iter()
                    .map(|r| r.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name).then(a.root.cmp(&b.root)));
    (found, truncated)
}

// ───────────────────────── Locked JSON files ─────────────────────────

fn load_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    let Ok(file) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    else {
        return T::default();
    };
    let mut bytes = Vec::new();
    if file.take(MAX_STATE).read_to_end(&mut bytes).is_err() {
        return T::default();
    }
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn change_json<T: for<'de> Deserialize<'de> + Serialize + Default, R>(
    path: &Path,
    edit: impl FnOnce(&mut T) -> Result<R>,
) -> Result<R> {
    let _lock = user_config::WriteLock::acquire(path)?;
    let mut value: T = load_json(path);
    let result = edit(&mut value)?;
    user_config::replace(path, &serde_json::to_string(&value)?)?;
    Ok(result)
}

// ───────────────────────── Ports ─────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
struct PortEntry {
    port: u16,
    at: u64,
    /// The `KELP_SLUG` this pair holds.
    #[serde(default)]
    slug: String,
}
#[derive(Serialize, Deserialize, Default, Debug)]
struct StoredPorts {
    version: u32,
    ports: BTreeMap<String, PortEntry>,
}

/// The ports allocated to (project, environment) pairs, in `ports.json`:
/// stable across restarts, while they stay in the configured range.
#[derive(Debug, Clone)]
pub struct Ports {
    path: PathBuf,
}
impl Ports {
    pub fn at(dir: &Path) -> Self {
        Self {
            path: dir.join("ports.json"),
        }
    }
    pub fn key(project: &str, env: &str) -> String {
        format!("{project}/{env}")
    }
    /// The port already allocated to `key`, if any.
    pub fn get(&self, key: &str) -> Option<u16> {
        load_json::<StoredPorts>(&self.path)
            .ports
            .get(key)
            .map(|entry| entry.port)
    }
    /// The port of `key`: the one it has when it lies in `range`, else the
    /// lowest port of the range no other pair holds and `free` accepts.
    /// `slug` is recorded as `key`'s: a slug another pair already holds
    /// (`kelp` + `mux-dev` and `kelp-mux` + `dev` are both `kelp-mux-dev`)
    /// is refused, so `KELP_SLUG` names one environment only.
    pub fn allocate(
        &self,
        key: &str,
        slug: &str,
        range: (u16, u16),
        free: impl Fn(u16) -> bool,
    ) -> Result<u16> {
        if let Some(parent) = self.path.parent() {
            private_dir(parent)?;
        }
        change_json(&self.path, |stored: &mut StoredPorts| {
            stored.version = 1;
            if let Some((other, _)) = stored
                .ports
                .iter()
                .find(|(other, entry)| *other != key && entry.slug == slug)
            {
                let (project, env) = other.split_once('/').unwrap_or((other, ""));
                return Err(format!(
                    "the slug `{slug}` is already used by environment `{env}` of project `{project}`; rename one of them"
                )
                .into());
            }
            if let Some(entry) = stored.ports.get_mut(key)
                && (range.0..=range.1).contains(&entry.port)
            {
                entry.slug = slug.to_owned();
                return Ok(entry.port);
            }
            let taken: std::collections::BTreeSet<u16> = stored
                .ports
                .iter()
                .filter(|(other, _)| *other != key)
                .map(|(_, entry)| entry.port)
                .collect();
            let port = (range.0..=range.1)
                .find(|port| !taken.contains(port) && free(*port))
                .ok_or("no free port left in `environments.ports`")?;
            stored.ports.insert(
                key.to_owned(),
                PortEntry {
                    port,
                    at: crate::now(),
                    slug: slug.to_owned(),
                },
            );
            if stored.ports.len() > MAX_PORTS {
                let oldest = stored
                    .ports
                    .iter()
                    .filter(|(other, _)| *other != key)
                    .min_by_key(|(_, entry)| entry.at)
                    .map(|(other, _)| other.clone());
                if let Some(oldest) = oldest {
                    stored.ports.remove(&oldest);
                }
            }
            Ok(port)
        })
    }
}

/// Whether nothing listens on a loopback port now.
pub fn port_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

// ───────────────────────── Pane records ─────────────────────────

/// Which environment a pane runs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    pub project: String,
    pub env: String,
    /// `process`, `up` or `down`.
    pub action: String,
    /// The run it belongs to (`run.json`).
    pub run: String,
    pub at: u64,
}
#[derive(Serialize, Deserialize, Default, Debug)]
struct StoredInstances {
    version: u32,
    panes: BTreeMap<String, Instance>,
}

/// A workspace's environment panes, in `environments.json` in its state
/// folder, keyed like the project links (`<epoch>:<pane>`). Records of
/// earlier server runs are dropped on each change.
#[derive(Debug, Clone)]
pub struct Instances {
    path: PathBuf,
}
impl Instances {
    pub fn at(state: &Path) -> Self {
        Self {
            path: state.join("environments.json"),
        }
    }
    /// This run's environment panes.
    pub fn panes(&self, epoch: &str) -> BTreeMap<u64, Instance> {
        let prefix = format!("{epoch}:");
        load_json::<StoredInstances>(&self.path)
            .panes
            .into_iter()
            .filter_map(|(key, instance)| {
                Some((key.strip_prefix(&prefix)?.parse().ok()?, instance))
            })
            .collect()
    }
    pub fn set(&self, epoch: &str, pane: u64, instance: Option<Instance>) -> Result<()> {
        let prefix = format!("{epoch}:");
        change_json(&self.path, |stored: &mut StoredInstances| {
            stored.version = 1;
            stored.panes.retain(|key, _| key.starts_with(&prefix));
            let key = Links::key(epoch, pane);
            match instance {
                Some(instance) => {
                    stored.panes.insert(key, instance);
                }
                None => {
                    stored.panes.remove(&key);
                }
            }
            while stored.panes.len() > MAX_INSTANCES {
                let oldest = stored
                    .panes
                    .iter()
                    .min_by_key(|(_, i)| i.at)
                    .map(|(k, _)| k.clone());
                match oldest {
                    Some(oldest) => stored.panes.remove(&oldest),
                    None => break,
                };
            }
            Ok(())
        })
    }
}

// ───────────────────────── Runs ─────────────────────────

/// How a pane's command ended.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub at: u64,
}
impl Exit {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
    pub fn describe(&self) -> String {
        match (self.code, self.signal) {
            (Some(code), _) => format!("exited with status {code}"),
            (None, Some(signal)) => format!("ended by signal {signal}"),
            _ => "ended".into(),
        }
    }
}

/// The latest run of an instance, `run.json` in its state folder.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    pub run: String,
    pub action: String,
    pub started_at: u64,
    pub exit: Option<Exit>,
    /// Stopped on request (a process environment's SIGTERM is no failure).
    #[serde(default)]
    pub stopped: bool,
}
fn run_path(state: &Path) -> PathBuf {
    state.join("run.json")
}
pub fn read_run(state: &Path) -> Option<Run> {
    let run: Run = load_json(&run_path(state));
    (!run.run.is_empty()).then_some(run)
}
fn write_run(state: &Path, edit: impl FnOnce(&mut Run)) -> Result<()> {
    change_json(&run_path(state), |run: &mut Run| {
        edit(run);
        Ok(())
    })
}

fn new_run_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    format!(
        "{:x}",
        fnv(format!(
            "{nanos}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
        .as_bytes())
    )
}

// ───────────────────────── Context ─────────────────────────

/// What an operation needs: the project, the workspace's state folder,
/// the port range and where instance state lives.
#[derive(Clone, Debug)]
pub struct Context {
    pub project: user_config::Project,
    /// The workspace's state folder (environment panes, project links).
    pub workspace: PathBuf,
    pub ports: (u16, u16),
    /// `$XDG_STATE_HOME/kelpmux/environments`.
    pub base: PathBuf,
    /// The kelpmux a pane runs (`kelpmux env exec`).
    pub exe: PathBuf,
}

/// An instance's state folder, `<base>/<project>/<env>` (`KELP_STATE_DIR`):
/// two levels, since the slugs of different pairs can be equal, and both
/// parts are validated names (no `/`, `.` or `..`).
pub fn instance_dir(base: &Path, project: &str, env: &str) -> PathBuf {
    base.join(project).join(env)
}

/// The variables an environment's commands get.
pub struct Variables {
    pub project: String,
    pub env: String,
    pub project_dir: PathBuf,
    pub slug: String,
    pub port: u16,
    pub state: PathBuf,
    pub dir: PathBuf,
}
impl Variables {
    pub fn pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("KELP_PROJECT", self.project.clone()),
            ("KELP_ENV", self.env.clone()),
            (
                "KELP_PROJECT_DIR",
                self.project_dir.to_string_lossy().into_owned(),
            ),
            ("KELP_SLUG", self.slug.clone()),
            ("KELP_PORT", self.port.to_string()),
            ("KELP_STATE_DIR", self.state.to_string_lossy().into_owned()),
            ("KELP_ENV_DIR", self.dir.to_string_lossy().into_owned()),
        ]
    }
}

impl Context {
    fn uid() -> u32 {
        unsafe { libc::geteuid() }
    }
    pub fn discover(&self) -> (Vec<Declared>, bool) {
        discover(&self.project.folder.path, Self::uid())
    }
    fn find(&self, name: &str) -> Result<Declared> {
        if !valid_name(name) {
            return Err("invalid environment name".into());
        }
        self.discover()
            .0
            .into_iter()
            .find(|d| d.name == name)
            .ok_or_else(|| {
                format!(
                    "no environment `{name}` in project `{}` (.kelp/environments/{name}/{MANIFEST})",
                    self.project.id
                )
                .into()
            })
    }
    pub fn slug(&self, env: &str) -> String {
        slug(&self.project.id, env)
    }
    pub fn state_dir(&self, env: &str) -> PathBuf {
        instance_dir(&self.base, &self.project.id, env)
    }
    fn port_of(&self, env: &str) -> Option<u16> {
        Ports::at(&self.base)
            .get(&Ports::key(&self.project.id, env))
            .filter(|port| (self.ports.0..=self.ports.1).contains(port))
    }
    fn variables(&self, declared: &Declared, port: u16) -> Variables {
        Variables {
            project: self.project.id.clone(),
            env: declared.name.clone(),
            project_dir: self.project.folder.path.clone(),
            slug: self.slug(&declared.name),
            port,
            state: self.state_dir(&declared.name),
            dir: declared.dir.clone(),
        }
    }
}

/// Opens a stream session on the workspace server.
pub type Opener<'a> = &'a dyn Fn(&str) -> Result<Session>;

/// The panes of this run that are alive: id → pid.
fn live_panes(session: &mut Session) -> Result<BTreeMap<u64, u32>> {
    Ok(session
        .update()?
        .panes
        .iter()
        .filter(|pane| !pane.dead)
        .map(|pane| (pane.id, pane.pid))
        .collect())
}

/// An environment's state, as the web client and `kelpmux env` show it.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Status {
    /// `starting`, `running`, `stopping`, `stopped`, `failed`, `invalid`, or `unknown` (a deploy environment not checked since its last action).
    pub state: &'static str,
    pub detail: Option<String>,
    pub url: Option<String>,
    /// The pane running it (or its latest `up`/`down`), while open.
    pub pane: Option<u64>,
    pub action: Option<String>,
}

/// The newest open pane of an environment in this run.
fn instance_pane(
    instances: &BTreeMap<u64, Instance>,
    live: &BTreeMap<u64, u32>,
    project: &str,
    env: &str,
) -> Option<(u64, Instance)> {
    instances
        .iter()
        .filter(|(pane, i)| i.project == project && i.env == env && live.contains_key(pane))
        .max_by_key(|(pane, i)| (i.at, **pane))
        .map(|(pane, i)| (*pane, i.clone()))
}

/// Whether a process environment answers now.
pub fn probe(ready: &Ready, port: u16, slug: &str) -> bool {
    let timeout = Duration::from_millis(700);
    match ready {
        Ready::Port(explicit) => {
            let port = explicit.unwrap_or(port);
            [
                SocketAddr::from(([127, 0, 0, 1], port)),
                SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)),
            ]
            .iter()
            .any(|address| TcpStream::connect_timeout(address, timeout).is_ok())
        }
        Ready::Http(url) => {
            let Some((addresses, host, path)) = loopback_http(&substitute(url, port, slug)) else {
                return false;
            };
            addresses.iter().any(|address| {
                let Ok(mut stream) = TcpStream::connect_timeout(address, timeout) else {
                    return false;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = stream.set_write_timeout(Some(timeout));
                if write!(
                    stream,
                    "GET {path} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: kelpmux\r\nConnection: close\r\n\r\n"
                )
                .is_err()
                {
                    return false;
                }
                let mut head = [0u8; 16];
                let mut read = 0;
                while read < head.len() {
                    match stream.read(&mut head[read..]) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => read += n,
                    }
                }
                let head = String::from_utf8_lossy(&head[..read]);
                head.starts_with("HTTP/")
                    && head
                        .split(' ')
                        .nth(1)
                        .and_then(|code| code.get(..3)?.parse::<u16>().ok())
                        .is_some_and(|code| (100..500).contains(&code))
            })
        }
    }
}

/// A pipe being read: the reader (until its end), its descriptor, how
/// much of it is kept and what was.
type Pipe = (Option<Box<dyn Read>>, i32, usize, Vec<u8>);

/// How long the pipes are still read after the script exited: a process
/// it detached (`setsid … &`) may hold them open for ever.
const PIPE_GRACE: Duration = Duration::from_millis(250);

/// Waits for `child` and reads its standard output (up to
/// [`STATUS_OUTPUT`]) and error (up to 4 KiB) on this thread, polling,
/// never past `deadline` (nor [`PIPE_GRACE`] after it exited): no reader
/// can block on a pipe that someone else keeps open. The exit status is
/// `None` when the deadline came first.
fn collect(
    child: &mut std::process::Child,
    deadline: Instant,
) -> (Option<std::process::ExitStatus>, Vec<u8>, Vec<u8>) {
    use std::os::fd::AsRawFd;
    let mut pipes: [Pipe; 2] = [
        (None, -1, STATUS_OUTPUT, Vec::new()),
        (None, -1, 4096, Vec::new()),
    ];
    if let Some(out) = child.stdout.take() {
        pipes[0].1 = out.as_raw_fd();
        pipes[0].0 = Some(Box::new(out));
    }
    if let Some(err) = child.stderr.take() {
        pipes[1].1 = err.as_raw_fd();
        pipes[1].0 = Some(Box::new(err));
    }
    for (_, fd, _, _) in &pipes {
        if *fd >= 0 {
            unsafe {
                let flags = libc::fcntl(*fd, libc::F_GETFL);
                libc::fcntl(*fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
    }
    let mut deadline = deadline;
    let mut status = None;
    let mut chunk = [0u8; 4096];
    loop {
        if status.is_none()
            && let Ok(Some(exited)) = child.try_wait()
        {
            status = Some(exited);
            deadline = deadline.min(Instant::now() + PIPE_GRACE);
        }
        let open: Vec<usize> = (0..2).filter(|&i| pipes[i].0.is_some()).collect();
        let now = Instant::now();
        if (status.is_some() && open.is_empty()) || now >= deadline {
            break;
        }
        let wait = (deadline - now).min(Duration::from_millis(20));
        let mut fds: Vec<libc::pollfd> = open
            .iter()
            .map(|&i| libc::pollfd {
                fd: pipes[i].1,
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        if fds.is_empty() {
            std::thread::sleep(wait);
            continue;
        }
        unsafe {
            libc::poll(
                fds.as_mut_ptr(),
                fds.len() as libc::nfds_t,
                wait.as_millis() as i32,
            );
        }
        for (&i, fd) in open.iter().zip(&fds) {
            if fd.revents == 0 {
                continue;
            }
            let (pipe, _, limit, bytes) = &mut pipes[i];
            // Drain what is there; past the limit it is read and dropped.
            while let Some(reader) = pipe.as_mut() {
                match reader.read(&mut chunk) {
                    Ok(0) => *pipe = None,
                    Ok(n) => {
                        let room = limit.saturating_sub(bytes.len());
                        bytes.extend_from_slice(&chunk[..n.min(room)]);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => *pipe = None,
                }
            }
        }
    }
    let [(_, _, _, out), (_, _, _, err)] = pipes;
    (status, out, err)
}

/// Runs a deploy environment's `status` script: at most
/// [`STATUS_TIMEOUT`], its output bounded, its process group killed when
/// it overruns. It must print `{"state": "running" | "stopped" | "failed",
/// "url": …, "detail": …}`.
pub fn run_status(declared: &Declared, variables: &Variables) -> Status {
    let failed = |detail: String| Status {
        state: "failed",
        detail: Some(detail),
        url: None,
        pane: None,
        action: None,
    };
    let Ok(manifest) = &declared.manifest else {
        return failed("invalid manifest".into());
    };
    let Some(script) = &manifest.status else {
        return failed("no status script".into());
    };
    let mut command = Command::new(declared.dir.join(script));
    command
        .current_dir(&declared.root)
        .envs(variables.pairs())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return failed(format!("status script did not start: {error}")),
    };
    let pid = child.id() as i32;
    let (status, out, err) = collect(&mut child, Instant::now() + STATUS_TIMEOUT);
    // Whatever it left behind in its group ends with it.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        return failed(format!(
            "status script did not answer within {} s",
            STATUS_TIMEOUT.as_secs()
        ));
    };
    if !status.success() {
        let message = String::from_utf8_lossy(&err);
        let last = message.lines().rev().find(|l| !l.trim().is_empty());
        return failed(format!(
            "status script {}{}",
            match status.code() {
                Some(code) => format!("exited with status {code}"),
                None => "was killed".into(),
            },
            last.map(|l| format!(": {}", clip(l, 200)))
                .unwrap_or_default()
        ));
    }
    parse_status(&out)
}

fn clip(text: &str, limit: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect()
}

/// Reads a `status` script's JSON.
pub fn parse_status(bytes: &[u8]) -> Status {
    let failed = |detail: &str| Status {
        state: "failed",
        detail: Some(detail.into()),
        url: None,
        pane: None,
        action: None,
    };
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return failed("status script did not print JSON");
    };
    let state = match value["state"].as_str() {
        Some("running") => "running",
        Some("stopped") => "stopped",
        Some("failed") => "failed",
        _ => return failed("status script printed no valid `state`"),
    };
    let url = value["url"].as_str().filter(|url| {
        url.len() <= 2048
            && (url.starts_with("http://") || url.starts_with("https://"))
            && !url.bytes().any(|b| b <= b' ' || b == 0x7f)
    });
    Status {
        state,
        detail: value["detail"]
            .as_str()
            .map(|detail| clip(detail, 500))
            .filter(|d| !d.is_empty()),
        url: url.map(str::to_owned),
        pane: None,
        action: None,
    }
}

/// An environment's current state. Asks a deploy environment's `status`
/// script unless an `up` or `down` is running.
fn status_of(
    context: &Context,
    declared: &Declared,
    instances: &BTreeMap<u64, Instance>,
    live: &BTreeMap<u64, u32>,
    check: bool,
) -> Status {
    let manifest = match &declared.manifest {
        Ok(manifest) => manifest,
        Err(error) => {
            return Status {
                state: "invalid",
                detail: Some(error.clone()),
                url: None,
                pane: None,
                action: None,
            };
        }
    };
    let state = context.state_dir(&declared.name);
    let run = read_run(&state);
    let port = context.port_of(&declared.name);
    let slug = context.slug(&declared.name);
    let url = manifest
        .url
        .as_ref()
        .map(|url| substitute(url, port.unwrap_or(0), &slug))
        .filter(|_| {
            port.is_some()
                || !manifest
                    .url
                    .as_deref()
                    .unwrap_or("")
                    .contains("${KELP_PORT}")
        });
    let open = instance_pane(instances, live, &context.project.id, &declared.name);
    let mut status = Status {
        state: "stopped",
        detail: None,
        url: url.clone(),
        pane: open.as_ref().map(|(pane, _)| *pane),
        action: open.as_ref().map(|(_, i)| i.action.clone()),
    };
    // The open pane's run and how it ended, if it did.
    let ended = |instance: &Instance| -> Option<Option<Exit>> {
        let run = run.as_ref().filter(|run| run.run == instance.run)?;
        Some(run.exit.clone())
    };
    match manifest.kind {
        Kind::Process => match &open {
            Some((_, instance)) => match ended(instance) {
                Some(Some(exit)) => {
                    status.state = if exit.success() { "stopped" } else { "failed" };
                    status.detail = Some(exit.describe());
                }
                _ => {
                    status.state = if probe(&manifest.ready, port.unwrap_or(0), &slug) {
                        "running"
                    } else {
                        "starting"
                    };
                }
            },
            None => {
                if let Some(run) = &run
                    && let Some(exit) = &run.exit
                    && !run.stopped
                {
                    status.state = if exit.success() { "stopped" } else { "failed" };
                    status.detail = Some(exit.describe());
                }
            }
        },
        Kind::Deploy => {
            if let Some((_, instance)) = &open
                && matches!(ended(instance), Some(None))
            {
                status.state = if instance.action == "down" {
                    "stopping"
                } else {
                    "starting"
                };
                return status;
            }
            // The `status` script is repository code: it runs only on an
            // explicit check, never to list.
            let checked = if check {
                let asked = run_status(declared, &context.variables(declared, port.unwrap_or(0)));
                let checked = Checked {
                    state: asked.state.to_owned(),
                    url: asked.url,
                    detail: asked.detail,
                    at: crate::now(),
                };
                let _ = private_dir(&state).and_then(|()| write_check(&state, &checked));
                Some(checked)
            } else {
                read_check(&state)
            };
            let last_failed = run.as_ref().and_then(|run| {
                let exit = run.exit.as_ref().filter(|exit| !exit.success())?;
                Some((
                    format!("last `{}` {}", run.action, exit.describe()),
                    exit.at,
                ))
            });
            if let Some(url) = checked.as_ref().and_then(|c| c.url.clone()) {
                status.url = Some(url);
            }
            match (&checked, last_failed) {
                // Fresh from the script.
                (Some(checked), failed) if check => {
                    status.state = match checked.state.as_str() {
                        "running" => "running",
                        "stopped" => "stopped",
                        _ => "failed",
                    };
                    status.detail = checked.detail.clone().or(failed.map(|(detail, _)| detail));
                }
                // An `up` or `down` that failed since the last check.
                (checked, Some((detail, at))) if checked.as_ref().is_none_or(|c| c.at <= at) => {
                    status.state = "failed";
                    status.detail = Some(detail);
                }
                _ => status.state = "unknown",
            }
        }
    }
    status
}

/// A deploy environment's last explicit status check (`check.json` in its
/// state folder), shown with its time until the next one.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub state: String,
    pub url: Option<String>,
    pub detail: Option<String>,
    /// When it was checked (epoch seconds).
    pub at: u64,
}

fn read_check(state: &Path) -> Option<Checked> {
    let checked: Option<Checked> = load_json(&state.join("check.json"));
    checked
}

fn write_check(state: &Path, checked: &Checked) -> Result<()> {
    change_json(&state.join("check.json"), |stored: &mut Option<Checked>| {
        *stored = Some(checked.clone());
        Ok(())
    })
}

fn entry_json(context: &Context, declared: &Declared, status: &Status) -> Value {
    let manifest = declared.manifest.as_ref().ok();
    let checked = manifest
        .filter(|m| m.kind == Kind::Deploy)
        .and_then(|_| read_check(&context.state_dir(&declared.name)));
    json!({
        "checked": checked,
        "name": declared.name,
        "kind": manifest.map(|m| m.kind),
        "description": manifest.and_then(|m| m.description.clone()),
        "root": declared.root,
        "repo": declared.root.file_name().map(|n| n.to_string_lossy().into_owned()),
        "dir": declared.dir,
        "slug": context.slug(&declared.name),
        "port": context.port_of(&declared.name),
        "logs": manifest.is_some_and(|m| m.kind == Kind::Process || m.logs.is_some()),
        "state": status.state,
        "detail": status.detail,
        "url": status.url,
        "pane": status.pane,
        "action": status.action,
        "error": declared.manifest.as_ref().err(),
    })
}

/// Every environment of the project with its state: `{project, folder,
/// environments, truncated}`. Nothing declared in a repository runs: a
/// deploy environment shows `unknown` (with its last explicit check)
/// unless an `up` or `down` is running or failed since; a process
/// environment's state comes from its pane and readiness probe.
pub fn list(context: &Context, open: Opener) -> Result<Value> {
    let mut session = open("pane-lifecycle")?;
    let epoch = session.epoch.clone();
    let live = live_panes(&mut session)?;
    drop(session);
    let instances = Instances::at(&context.workspace).panes(&epoch);
    let (declared, truncated) = context.discover();
    let statuses: Vec<Status> = std::thread::scope(|scope| {
        let handles: Vec<_> = declared
            .iter()
            .map(|d| scope.spawn(|| status_of(context, d, &instances, &live, false)))
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join().unwrap_or(Status {
                    state: "failed",
                    detail: Some("internal error".into()),
                    url: None,
                    pane: None,
                    action: None,
                })
            })
            .collect()
    });
    Ok(json!({
        "project": context.project.id,
        "folder": context.project.folder.path,
        "exists": context.project.folder.path.is_dir(),
        "ports": [context.ports.0, context.ports.1],
        "environments": declared
            .iter()
            .zip(&statuses)
            .map(|(d, s)| entry_json(context, d, s))
            .collect::<Vec<_>>(),
        "truncated": truncated,
    }))
}

/// One environment's entry, as in [`list`].
pub fn status(context: &Context, name: &str, open: Opener, check: bool) -> Result<Value> {
    let declared = context.find(name)?;
    let mut session = open("pane-lifecycle")?;
    let epoch = session.epoch.clone();
    let live = live_panes(&mut session)?;
    drop(session);
    let instances = Instances::at(&context.workspace).panes(&epoch);
    let status = status_of(context, &declared, &instances, &live, check);
    Ok(entry_json(context, &declared, &status))
}

/// This run's environment panes, for the sidebar: pane → `{project, env,
/// action}`.
pub fn panes(workspace: &Path, epoch: &str) -> Value {
    let panes: serde_json::Map<String, Value> = Instances::at(workspace)
        .panes(epoch)
        .into_iter()
        .map(|(pane, i)| {
            (
                pane.to_string(),
                json!({"project": i.project, "env": i.env, "action": i.action}),
            )
        })
        .collect();
    json!({ "panes": panes })
}

/// The pane `argv` that runs an environment action (`kelpmux env exec`).
fn exec_argv(
    context: &Context,
    declared: &Declared,
    action: &str,
    port: u16,
    run: &str,
) -> Vec<String> {
    let text = |path: &Path| path.to_string_lossy().into_owned();
    vec![
        text(&context.exe),
        "env".into(),
        "exec".into(),
        "--project".into(),
        context.project.id.clone(),
        "--project-dir".into(),
        text(&context.project.folder.path),
        "--root".into(),
        text(&declared.root),
        "--name".into(),
        declared.name.clone(),
        "--action".into(),
        action.into(),
        "--port".into(),
        port.to_string(),
        "--state".into(),
        text(&context.state_dir(&declared.name)),
        "--run".into(),
        run.into(),
    ]
}

/// The process group of the runner of `run` when it is in front of the
/// pane whose process is `pane_pid`. KelpServer runs a pane's command as a
/// job of the user's shell, so the runner leads its own group: Stop signals
/// that group only, never the shell.
fn runner_group(proc_root: &Path, pane_pid: u32, run: &str) -> Option<i32> {
    let pane = crate::process::read(proc_root, pane_pid).ok()?;
    let group = pane.foreground;
    if group <= 0 {
        return None;
    }
    let args = crate::process::args(proc_root, group as u32);
    let ours = args
        .windows(2)
        .any(|pair| pair[0] == "--run" && pair[1] == run)
        && args.iter().any(|arg| arg == "exec");
    ours.then_some(group)
}

/// Whether a pane's shell is at its prompt (nothing in front of it).
fn at_prompt(proc_root: &Path, pane_pid: u32) -> bool {
    crate::process::read(proc_root, pane_pid)
        .is_ok_and(|process| process.foreground <= 0 || process.foreground == process.group)
}

/// Starts an environment: a process environment's command, or a deploy
/// environment's `up`, in a new pane of the project. `request` makes a
/// retry return the same pane.
pub fn start(context: &Context, name: &str, request: &str, open: Opener) -> Result<Value> {
    act(context, name, request, open, true)
}

/// Stops an environment. A process environment gets SIGTERM (its pane's
/// process group), up to [`STOP_GRACE`] to exit, then its pane is closed;
/// a deploy environment runs `down` in a new pane.
pub fn stop(context: &Context, name: &str, request: &str, open: Opener) -> Result<Value> {
    act(context, name, request, open, false)
}

fn act(context: &Context, name: &str, request: &str, open: Opener, up: bool) -> Result<Value> {
    let declared = context.find(name)?;
    let manifest = declared
        .manifest
        .clone()
        .map_err(|error| format!("environment `{name}` cannot run: {error}"))?;
    if !context.project.folder.path.is_dir() {
        return Err("the project's folder is missing".into());
    }
    let mut session = open("pane-lifecycle")?;
    let epoch = session.epoch.clone();
    let live = live_panes(&mut session)?;
    let instances = Instances::at(&context.workspace);
    let current = instance_pane(&instances.panes(&epoch), &live, &context.project.id, name);
    let state = context.state_dir(name);
    let run = read_run(&state);
    let running = |instance: &Instance| {
        run.as_ref()
            .is_some_and(|run| run.run == instance.run && run.exit.is_none())
    };
    let action = match (manifest.kind, up) {
        (Kind::Process, true) => "process",
        (Kind::Deploy, true) => "up",
        (Kind::Deploy, false) => "down",
        (Kind::Process, false) => {
            let Some((pane, instance)) = current else {
                if let Some(run) = &run
                    && !run.stopped
                {
                    write_run(&state, |run| run.stopped = true)?;
                }
                return Ok(json!({ "state": "stopped" }));
            };
            if running(&instance)
                && let Some(group) = live
                    .get(&pane)
                    .and_then(|pid| runner_group(Path::new("/proc"), *pid, &instance.run))
            {
                unsafe {
                    libc::kill(-group, libc::SIGTERM);
                }
                let deadline = Instant::now() + STOP_GRACE;
                while Instant::now() < deadline
                    && read_run(&state)
                        .is_some_and(|run| run.run == instance.run && run.exit.is_none())
                {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            if run.as_ref().is_some_and(|run| run.run == instance.run) {
                write_run(&state, |run| run.stopped = true)?;
            }
            // The pane may hold the terminal's lease of a browser: take it.
            session.claim(pane)?;
            session.call(Request::Close {
                request: request_id("env-stop"),
                pane,
            })?;
            instances.set(&epoch, pane, None)?;
            return Ok(json!({ "state": "stopped", "pane": pane }));
        }
    };
    if let Some((pane, instance)) = &current
        && running(instance)
    {
        let what = match (manifest.kind, instance.action.as_str()) {
            (Kind::Process, _) => "is already running".to_owned(),
            (_, action) => format!("is still running `{action}`"),
        };
        return Err(format!("environment `{name}` {what} (terminal {pane})").into());
    }
    // Earlier panes of this environment whose run ended and that sit at
    // their shell's prompt are closed: a new run replaces them.
    for (pane, instance) in instances.panes(&epoch) {
        if instance.project == context.project.id
            && instance.env == name
            && !running(&instance)
            && let Some(pid) = live.get(&pane)
            && at_prompt(Path::new("/proc"), *pid)
            && session.claim(pane).is_ok()
            && session
                .call(Request::Close {
                    request: request_id("env-replace"),
                    pane,
                })
                .is_ok()
        {
            instances.set(&epoch, pane, None)?;
        }
    }
    let port = Ports::at(&context.base).allocate(
        &Ports::key(&context.project.id, name),
        &context.slug(name),
        context.ports,
        port_free,
    )?;
    private_dir(&state)?;
    let run_id = new_run_id();
    write_run(&state, |run| {
        *run = Run {
            run: run_id.clone(),
            action: action.into(),
            started_at: crate::now(),
            exit: None,
            stopped: false,
        }
    })?;
    let Event::Spawned { pane, .. } = session.call(Request::Spawn {
        request: request.to_owned(),
        cwd: Some(declared.root.clone()),
        argv: exec_argv(context, &declared, action, port, &run_id),
        shell: None,
    })?
    else {
        return Err("terminal server did not identify the created pane".into());
    };
    instances.set(
        &epoch,
        pane,
        Some(Instance {
            project: context.project.id.clone(),
            env: name.to_owned(),
            action: action.into(),
            run: run_id,
            at: crate::now(),
        }),
    )?;
    Links::at(&context.workspace).set(&Links::key(&epoch, pane), Some(&context.project.id))?;
    Ok(json!({
        "pane": pane,
        "action": action,
        "state": if action == "down" { "stopping" } else { "starting" },
        "port": port,
        "slug": context.slug(name),
    }))
}

/// The last `lines` lines of a process environment's pane (its
/// scrollback is its log), or `None` when it has no open pane.
pub fn pane_log(
    context: &Context,
    name: &str,
    lines: usize,
    open: Opener,
) -> Result<Option<Vec<String>>> {
    context.find(name)?;
    let mut session = open("pane-history")?;
    let epoch = session.epoch.clone();
    let live = live_panes(&mut session)?;
    let Some((pane, _)) = instance_pane(
        &Instances::at(&context.workspace).panes(&epoch),
        &live,
        &context.project.id,
        name,
    ) else {
        return Ok(None);
    };
    let mut rows: Vec<(i32, String)> = Vec::new();
    let mut start = -(lines.min(10_000) as i32);
    for _ in 0..64 {
        let Event::History { lines: page, .. } = session.call(Request::History {
            request: request_id("env-logs"),
            pane,
            start,
            limit: 200,
        })?
        else {
            return Err("terminal server did not return history".into());
        };
        let count = page.len();
        let Some(last) = page.last().map(|(line, _)| *line) else {
            break;
        };
        rows.extend(page);
        if count < 200 {
            break;
        }
        start = last + 1;
    }
    let mut text: Vec<String> = rows
        .into_iter()
        .map(|(_, row)| row.trim_end().to_owned())
        .collect();
    while text.last().is_some_and(|l| l.is_empty()) {
        text.pop();
    }
    let keep = text.len().saturating_sub(lines);
    Ok(Some(text.split_off(keep)))
}

/// Runs a deploy environment's `logs` script in the foreground.
pub fn run_logs(context: &Context, name: &str) -> Result<i32> {
    let declared = context.find(name)?;
    let manifest = declared
        .manifest
        .clone()
        .map_err(|error| format!("environment `{name}` cannot run: {error}"))?;
    let script = manifest
        .logs
        .ok_or_else(|| format!("environment `{name}` declares no `logs` script"))?;
    let port = context.port_of(name).unwrap_or(0);
    let status = Command::new(declared.dir.join(script))
        .current_dir(&declared.root)
        .envs(context.variables(&declared, port).pairs())
        .status()?;
    Ok(status.code().unwrap_or(1))
}

// ───────────────────────── The pane's runner ─────────────────────────

/// `kelpmux env exec …`: what an environment pane runs. Re-reads the
/// manifest (nothing of the command comes through the command line), sets
/// the variables, runs the command or script in the declaring repository,
/// records how it ended in `run.json`. KelpServer runs it as a job of the
/// user's shell, which stays in the pane afterwards with the output above.
pub fn exec(args: &[String]) -> Result<()> {
    let mut values: BTreeMap<&str, String> = BTreeMap::new();
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let key = match flag.as_str() {
            "--project" | "--project-dir" | "--root" | "--name" | "--action" | "--port"
            | "--state" | "--run" => &flag[2..],
            _ => return Err(format!("unknown env exec option {flag}").into()),
        };
        values.insert(key, rest.next().ok_or("option needs a value")?.clone());
    }
    let value = |key: &str| -> Result<String> {
        values
            .get(key)
            .cloned()
            .ok_or_else(|| format!("env exec needs --{key}").into())
    };
    let (project, name, action, run) = (
        value("project")?,
        value("name")?,
        value("action")?,
        value("run")?,
    );
    let root = PathBuf::from(value("root")?);
    let state = PathBuf::from(value("state")?);
    let record = |exit: Exit| {
        let _ = write_run(&state, |recorded| {
            if recorded.run == run {
                recorded.exit = Some(exit);
            }
        });
    };
    let prepared = (|| -> Result<Command> {
        let port: u16 = value("port")?.parse().map_err(|_| "invalid --port")?;
        let declared = load(&root, &name, unsafe { libc::geteuid() });
        let manifest = declared
            .manifest
            .clone()
            .map_err(|error| format!("environment `{name}` cannot run: {error}"))?;
        let variables = Variables {
            slug: slug(&project, &name),
            project: project.clone(),
            env: name.clone(),
            project_dir: PathBuf::from(value("project-dir")?),
            port,
            state: state.clone(),
            dir: declared.dir.clone(),
        };
        let mut command = runner_command(&declared, &manifest, &action)?;
        command.current_dir(&root).envs(variables.pairs());
        Ok(command)
    })();
    let mut out = std::io::stdout();
    let mut command = match prepared {
        Ok(command) => command,
        Err(error) => {
            let _ = writeln!(out, "[kelpmux] environment {name}: {error}");
            record(Exit {
                code: Some(2),
                signal: None,
                at: crate::now(),
            });
            return Err(error);
        }
    };
    // The pane's title names the environment.
    let _ = write!(out, "\x1b]0;env {name} ({action})\x07");
    let _ = writeln!(
        out,
        "\x1b[2m[kelpmux] environment {name}: {action} in {}\x1b[0m",
        root.display()
    );
    let _ = out.flush();
    // The child keeps the default signal dispositions; this runner ignores
    // Ctrl-C and Stop's SIGTERM so it can record how the child ended.
    unsafe {
        command.pre_exec(|| {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            Ok(())
        });
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
    let exit = match command.status() {
        Ok(status) => Exit {
            code: status.code(),
            signal: status.signal(),
            at: crate::now(),
        },
        Err(error) => {
            let _ = writeln!(out, "[kelpmux] could not start: {error}");
            Exit {
                code: Some(127),
                signal: None,
                at: crate::now(),
            }
        }
    };
    let described = exit.describe();
    record(exit);
    // KelpServer runs a pane's command as a job of the user's shell: the
    // pane stays open, as that shell in the repository, with the output
    // above it.
    let _ = writeln!(
        out,
        "\n\x1b[2m[kelpmux] environment {name}: {action} {described}.\x1b[0m",
    );
    let _ = out.flush();
    Ok(())
}

/// What the runner starts for `action`.
fn runner_command(declared: &Declared, manifest: &Manifest, action: &str) -> Result<Command> {
    let name = &declared.name;
    Ok(match (manifest.kind, action) {
        (Kind::Process, "process") => {
            let mut command = Command::new("/bin/sh");
            command
                .arg("-c")
                .arg(manifest.command.as_deref().unwrap_or(""));
            command
        }
        (Kind::Deploy, "up") => {
            Command::new(declared.dir.join(manifest.up.as_deref().unwrap_or("")))
        }
        (Kind::Deploy, "down") => {
            Command::new(declared.dir.join(manifest.down.as_deref().unwrap_or("")))
        }
        _ => return Err(format!("environment `{name}` has no action `{action}`").into()),
    })
}

// ───────────────────────── Command line ─────────────────────────

pub const USAGE: &str = "usage: kelpmux env list|up NAME [--wait]|down NAME [--wait]|status NAME|logs NAME [--lines N] [--project ID] [--workspace NAME]";

/// How long `--wait` waits for an environment to settle.
const WAIT: Duration = Duration::from_secs(180);

/// `kelpmux env …`, for agents and people in a project's terminal. The
/// project is `--project`, else the one whose folder holds the current
/// directory; the workspace is `--workspace`, else the one this terminal
/// belongs to (`KELPMUX_STATE_DIR`), else `main`.
pub fn cli(args: &[String]) -> Result<()> {
    let Some(verb) = args.first() else {
        return Err(USAGE.into());
    };
    if verb == "exec" {
        return exec(&args[1..]);
    }
    let mut name = None;
    let mut project = None;
    let mut workspace = None;
    let mut wait = false;
    let mut lines = 200usize;
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--project" => project = Some(rest.next().ok_or("--project needs an id")?.clone()),
            "--workspace" => {
                workspace = Some(rest.next().ok_or("--workspace needs a name")?.clone())
            }
            "--wait" if matches!(verb.as_str(), "up" | "down") => wait = true,
            "--lines" if verb == "logs" => {
                lines = rest
                    .next()
                    .and_then(|n| n.parse().ok())
                    .filter(|n| (1..=10_000).contains(n))
                    .ok_or("--lines needs a number from 1 to 10000")?
            }
            other if !other.starts_with("--") && name.is_none() => name = Some(other.to_owned()),
            _ => return Err(format!("unexpected argument {arg}\n{USAGE}").into()),
        }
    }
    let needs_name = verb != "list";
    if !matches!(verb.as_str(), "list" | "up" | "down" | "status" | "logs")
        || needs_name != name.is_some()
    {
        return Err(USAGE.into());
    }
    let state = match workspace {
        Some(name) => super::lifecycle::workspace_path(&name)?,
        None => match std::env::var_os("KELPMUX_STATE_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
        {
            Some(state) => state,
            None => super::lifecycle::workspace_path("main")?,
        },
    };
    let socket = state.join("control.sock");
    let context = cli_context(project.as_deref(), &state)?;
    let open = |capability: &str| -> Result<Session> {
        let session = Session::open(super::cli::connect(&socket)?)?;
        session.require(capability)?;
        Ok(session)
    };
    let print = |value: &Value| -> Result<()> {
        println!("{}", serde_json::to_string_pretty(value)?);
        Ok(())
    };
    let name = name.unwrap_or_default();
    match verb.as_str() {
        "list" => print(&list(&context, &open)?),
        "status" => print(&status(&context, &name, &open, true)?),
        "up" | "down" => {
            let request = request_id(&format!("env-{verb}"));
            let started = if verb == "up" {
                start(&context, &name, &request, &open)?
            } else {
                stop(&context, &name, &request, &open)?
            };
            if !wait {
                return print(&started);
            }
            let deadline = Instant::now() + WAIT;
            loop {
                let mut current = status(&context, &name, &open, false)?;
                let mut state = current["state"].as_str().unwrap_or("").to_owned();
                if !matches!(state.as_str(), "starting" | "stopping") || Instant::now() > deadline {
                    // The action is over: this explicit `up`/`down --wait`
                    // checks a deploy environment's status once.
                    if state == "unknown" {
                        current = status(&context, &name, &open, true)?;
                        state = current["state"].as_str().unwrap_or("").to_owned();
                    }
                    let state = state.as_str();
                    print(&current)?;
                    return match state {
                        "failed" => Err(format!(
                            "environment `{name}` failed{}",
                            current["detail"]
                                .as_str()
                                .map(|d| format!(": {d}"))
                                .unwrap_or_default()
                        )
                        .into()),
                        "starting" | "stopping" => {
                            Err(format!("environment `{name}` is still {state}").into())
                        }
                        _ => Ok(()),
                    };
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        _ => {
            if let Some(log) = pane_log(&context, &name, lines, &open)? {
                for line in log {
                    println!("{line}");
                }
                return Ok(());
            }
            match run_logs(&context, &name) {
                Ok(0) => Ok(()),
                Ok(code) => Err(format!("logs script exited with status {code}").into()),
                Err(error) => Err(format!(
                    "environment `{name}` has no open terminal and no logs script: {error}"
                )
                .into()),
            }
        }
    }
}

/// The project of `kelpmux env` and what its operations need.
fn cli_context(project: Option<&str>, workspace: &Path) -> Result<Context> {
    let path = user_config::default_path()?;
    let text = user_config::read(&path)?.unwrap_or_default();
    let config = user_config::validate(&text, &user_config::Env::current()?).map_err(|errors| {
        format!(
            "the user configuration is invalid: {}",
            errors
                .iter()
                .map(|e| e.display(&path))
                .collect::<Vec<_>>()
                .join("; ")
        )
    })?;
    let project = match project {
        Some(id) => config
            .project(id)
            .cloned()
            .ok_or_else(|| format!("no project `{id}` in {}", path.display()))?,
        None => {
            let cwd = std::env::current_dir()?;
            let cwd = cwd.canonicalize().unwrap_or(cwd);
            config
                .projects
                .iter()
                .find(|project| {
                    project
                        .folder
                        .path
                        .canonicalize()
                        .is_ok_and(|folder| cwd.starts_with(folder))
                })
                .cloned()
                .ok_or("the current folder is in no project's folder; pass --project ID")?
        }
    };
    Ok(Context {
        project,
        workspace: workspace.to_path_buf(),
        ports: config.environment_ports.1,
        base: state_base()?,
        exe: std::env::current_exe()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "kelpmux-env-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn uid() -> u32 {
        unsafe { libc::geteuid() }
    }
    fn declare(root: &Path, name: &str, manifest: &str) -> PathBuf {
        let dir = root.join(DECLARED_IN).join(name);
        fs::create_dir_all(&dir).unwrap();
        for path in [root.join(".kelp"), root.join(DECLARED_IN), dir.clone()] {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(dir.join(MANIFEST), manifest).unwrap();
        fs::set_permissions(dir.join(MANIFEST), fs::Permissions::from_mode(0o644)).unwrap();
        dir
    }
    fn script(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(dir.join(name), fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn manifests_are_validated_strictly() {
        let process = parse_manifest(
            "kind = \"process\"\ndescription = \"Dev\"\ncommand = \"npm run dev -- --port $KELP_PORT\"\nurl = \"http://localhost:${KELP_PORT}/\"\n",
        )
        .unwrap();
        assert_eq!(process.kind, Kind::Process);
        assert_eq!(process.ready, Ready::Port(None));
        let ready = parse_manifest(
            "kind = \"process\"\ncommand = \"x\"\n[ready]\nhttp = \"http://127.0.0.1:${KELP_PORT}/health\"\n",
        )
        .unwrap();
        assert!(matches!(ready.ready, Ready::Http(_)));
        let deploy = parse_manifest(
            "kind = \"deploy\"\n[scripts]\nup = \"up.sh\"\ndown = \"down.sh\"\nstatus = \"status.sh\"\n",
        )
        .unwrap();
        assert_eq!(deploy.up.as_deref(), Some("up.sh"));
        assert_eq!(deploy.logs, None);
        for (bad, why) in [
            ("command = \"x\"", "`kind` is required"),
            ("kind = \"vm\"", "`kind` must be"),
            ("kind = \"process\"", "needs `command`"),
            (
                "kind = \"process\"\ncommand = \"x\"\nextra = 1",
                "unknown key `extra`",
            ),
            (
                "kind = \"process\"\ncommand = 3",
                "`command` must be a string",
            ),
            (
                "kind = \"process\"\ncommand = \"x\"\n[ready]\nport = 70000",
                "port number",
            ),
            (
                "kind = \"process\"\ncommand = \"x\"\n[ready]\nport = 1\nhttp = \"http://localhost/\"",
                "one of",
            ),
            (
                "kind = \"process\"\ncommand = \"x\"\n[ready]\nhttp = \"http://example.com/\"",
                "127.0.0.1",
            ),
            (
                "kind = \"process\"\ncommand = \"x\"\n[scripts]\nup = \"u\"",
                "no `[scripts]`",
            ),
            ("kind = \"deploy\"\ncommand = \"x\"", "no `command`"),
            (
                "kind = \"deploy\"\n[scripts]\nup = \"u\"\ndown = \"d\"",
                "needs `up`, `down` and `status`",
            ),
            (
                "kind = \"deploy\"\n[scripts]\nup = \"../u\"\ndown = \"d\"\nstatus = \"s\"",
                "file name",
            ),
            (
                "kind = \"deploy\"\n[scripts]\nup = \"u\"\ndown = \"d\"\nstatus = \"s\"\nrestart = \"r\"",
                "unknown key `scripts.restart`",
            ),
            (
                "kind = \"process\"\ncommand = \"x\"\nurl = \"ftp://x\"",
                "`url` must be",
            ),
            ("kind = [", "invalid TOML"),
        ] {
            let error = parse_manifest(bad).unwrap_err();
            assert!(error.contains(why), "{bad:?}: {error}");
        }
    }

    #[test]
    fn readiness_urls_stay_on_the_loopback() {
        let (addresses, host, path) = loopback_http("http://localhost:5173/health?x=1").unwrap();
        assert_eq!(addresses.len(), 2);
        assert_eq!(
            (host.as_str(), path.as_str()),
            ("localhost:5173", "/health?x=1")
        );
        assert_eq!(loopback_http("http://[::1]:80").unwrap().2, "/");
        for bad in [
            "https://127.0.0.1/",
            "http://10.0.0.1/",
            "http://user@127.0.0.1/",
            "http://127.0.0.1:0/",
            "http://127.0.0.1:99999/",
            "http://127.0.0.1/a b",
        ] {
            assert!(loopback_http(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn slugs_are_dns_labels() {
        assert_eq!(slug("kelp", "web"), "kelp-web");
        assert_eq!(slug("my-app", "dev-2"), "my-app-dev-2");
        let long = slug(&"a".repeat(64), &"b".repeat(32));
        assert!(long.len() <= MAX_SLUG, "{long}");
        assert!(
            long.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        );
        assert!(!long.starts_with('-') && !long.ends_with('-'));
        // The hash keeps long slugs apart.
        assert_ne!(long, slug(&"a".repeat(64), &format!("{}c", "b".repeat(31))));
        assert_eq!(long, slug(&"a".repeat(64), &"b".repeat(32)));
        assert!(valid_name("web") && valid_name("dev-2"));
        for bad in ["", "-web", "web-", "Web", "w_b", &"x".repeat(33)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn ports_are_stable_unique_and_follow_the_range() {
        let temp = Temp::new("ports");
        let ports = Ports::at(&temp.0);
        let range = (42000, 42003);
        let a = ports.allocate("p/a", "p-a", range, |_| true).unwrap();
        let b = ports.allocate("p/b", "p-b", range, |_| true).unwrap();
        assert_eq!((a, b), (42000, 42001));
        // Stable across calls, even when the port is busy now (its own server).
        assert_eq!(ports.allocate("p/a", "p-a", range, |_| false).unwrap(), a);
        assert_eq!(ports.get("p/a"), Some(a));
        // A busy port is skipped for a new allocation.
        assert_eq!(
            ports
                .allocate("p/c", "p-c", range, |port| port != 42002)
                .unwrap(),
            42003
        );
        assert_eq!(
            ports.allocate("p/d", "p-d", range, |_| true).unwrap(),
            42002
        );
        assert!(ports.allocate("p/e", "p-e", range, |_| true).is_err());
        // A new range moves a port that falls outside it.
        assert_eq!(
            ports
                .allocate("p/a", "p-a", (43000, 43001), |_| true)
                .unwrap(),
            43000
        );
        assert_eq!(
            ports
                .allocate("p/b", "p-b", (42000, 42003), |_| true)
                .unwrap(),
            b
        );
        // Concurrent writers each get their own port.
        let threads: Vec<_> = (0..8)
            .map(|n| {
                let ports = Ports::at(&temp.0);
                std::thread::spawn(move || {
                    ports
                        .allocate(&format!("q/{n}"), &format!("q-{n}"), (44000, 44100), |_| {
                            true
                        })
                        .unwrap()
                })
            })
            .collect();
        let mut got: Vec<u16> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        got.sort();
        got.dedup();
        assert_eq!(got.len(), 8);
    }

    #[test]
    fn a_slug_belongs_to_one_environment_and_state_folders_do_not_collide() {
        // Different pairs, the same slug.
        assert_eq!(slug("kelp", "mux-dev"), slug("kelp-mux", "dev"));
        let temp = Temp::new("slugs");
        let ports = Ports::at(&temp.0);
        let range = (42000, 42010);
        ports
            .allocate("kelp/mux-dev", "kelp-mux-dev", range, |_| true)
            .unwrap();
        let error = ports
            .allocate("kelp-mux/dev", "kelp-mux-dev", range, |_| true)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("`kelp-mux-dev`")
                && error.contains("environment `mux-dev` of project `kelp`"),
            "{error}"
        );
        assert_eq!(ports.get("kelp-mux/dev"), None);
        // The holder keeps it.
        assert_eq!(
            ports
                .allocate("kelp/mux-dev", "kelp-mux-dev", range, |_| true)
                .unwrap(),
            42000
        );
        // State folders are per (project, environment), never per slug.
        let base = temp.0.join("base");
        assert_ne!(
            instance_dir(&base, "kelp", "mux-dev"),
            instance_dir(&base, "kelp-mux", "dev")
        );
        assert_eq!(
            instance_dir(&base, "kelp", "mux-dev"),
            base.join("kelp/mux-dev")
        );
    }

    #[test]
    fn discovery_finds_declared_environments_and_applies_the_trust_rules() {
        let temp = Temp::new("discover");
        let folder = temp.0.join("project");
        fs::create_dir_all(folder.join("api/.git")).unwrap();
        fs::write(folder.join("api/.git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::create_dir_all(folder.join("web/.git")).unwrap();
        fs::write(folder.join("web/.git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::create_dir_all(folder.join("notes")).unwrap();
        declare(&folder, "all", "kind = \"process\"\ncommand = \"true\"\n");
        declare(
            &folder.join("api"),
            "dev",
            "kind = \"process\"\ncommand = \"true\"\n",
        );
        let deploy = declare(
            &folder.join("api"),
            "staging",
            "kind = \"deploy\"\n[scripts]\nup = \"up\"\ndown = \"down\"\nstatus = \"status\"\n",
        );
        script(&deploy, "up", "true");
        script(&deploy, "down", "true");
        script(&deploy, "status", "echo '{\"state\":\"stopped\"}'");
        // A plain folder that is not a checkout is not searched.
        declare(
            &folder.join("notes"),
            "hidden",
            "kind = \"process\"\ncommand = \"true\"\n",
        );
        // Declared twice.
        declare(
            &folder.join("web"),
            "dev",
            "kind = \"process\"\ncommand = \"true\"\n",
        );
        // A symbolic link to an environment folder is refused.
        let outside = temp.0.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(
            outside.join(MANIFEST),
            "kind = \"process\"\ncommand = \"true\"\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(
            &outside,
            folder.join("web").join(DECLARED_IN).join("linked"),
        )
        .unwrap();
        // A manifest that is a symbolic link is refused.
        let linked = declare(&folder.join("web"), "manifest-link", "");
        fs::remove_file(linked.join(MANIFEST)).unwrap();
        std::os::unix::fs::symlink(outside.join(MANIFEST), linked.join(MANIFEST)).unwrap();
        // A manifest others can write is refused.
        let open = declare(
            &folder.join("web"),
            "writable",
            "kind = \"process\"\ncommand = \"true\"\n",
        );
        fs::set_permissions(open.join(MANIFEST), fs::Permissions::from_mode(0o666)).unwrap();
        // A script that is not executable, or a symbolic link, is refused.
        let noexec = declare(
            &folder.join("web"),
            "noexec",
            "kind = \"deploy\"\n[scripts]\nup = \"up\"\ndown = \"down\"\nstatus = \"status\"\n",
        );
        script(&noexec, "up", "true");
        script(&noexec, "down", "true");
        std::os::unix::fs::symlink("/bin/true", noexec.join("status")).unwrap();

        let (found, truncated) = discover(&folder, uid());
        assert!(!truncated);
        let named =
            |name: &str| -> Vec<&Declared> { found.iter().filter(|d| d.name == name).collect() };
        assert!(named("all")[0].manifest.is_ok());
        assert_eq!(named("all")[0].root, folder);
        assert!(named("staging")[0].manifest.is_ok());
        assert!(named("hidden").is_empty());
        assert!(
            named("linked").is_empty(),
            "symbolic link folders are not listed"
        );
        let dev = named("dev");
        assert_eq!(dev.len(), 2);
        assert!(
            dev.iter()
                .all(|d| d.manifest.as_ref().unwrap_err().contains("several"))
        );
        assert!(
            named("manifest-link")[0]
                .manifest
                .as_ref()
                .unwrap_err()
                .contains("symbolic link")
        );
        assert!(
            named("writable")[0]
                .manifest
                .as_ref()
                .unwrap_err()
                .contains("only this user")
        );
        // A checkout others can write (not sticky) is refused, even though
        // its environment's files are the user's own.
        fs::set_permissions(folder.join("web"), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            load(&folder.join("web"), "dev", uid())
                .manifest
                .unwrap_err()
                .contains("only this user")
        );
        fs::set_permissions(folder.join("web"), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            named("noexec")[0]
                .manifest
                .as_ref()
                .unwrap_err()
                .contains("status")
        );
        // A writable `.kelp` hides everything under it.
        fs::set_permissions(folder.join("api/.kelp"), fs::Permissions::from_mode(0o777)).unwrap();
        let (found, _) = discover(&folder, uid());
        assert!(found.iter().all(|d| d.root != folder.join("api")));
        assert!(
            load(&folder.join("api"), "staging", uid())
                .manifest
                .is_err()
        );
        // A status script's JSON.
        fs::set_permissions(folder.join("api/.kelp"), fs::Permissions::from_mode(0o755)).unwrap();
        let declared = load(&folder.join("api"), "staging", uid());
        let variables = Variables {
            project: "p".into(),
            env: "staging".into(),
            project_dir: folder.clone(),
            slug: "p-staging".into(),
            port: 42000,
            state: temp.0.join("state"),
            dir: declared.dir.clone(),
        };
        assert_eq!(run_status(&declared, &variables).state, "stopped");
        // A detached child holding its standard output open does not hold
        // up the answer.
        script(
            &deploy,
            "status",
            "setsid sleep 30 &\necho '{\"state\":\"running\"}'",
        );
        let asked = Instant::now();
        assert_eq!(run_status(&declared, &variables).state, "running");
        assert!(
            asked.elapsed() < Duration::from_secs(3),
            "{:?}",
            asked.elapsed()
        );
        // Nor does one that never ends, past the deadline.
        script(&deploy, "status", "exec sleep 60");
        let asked = Instant::now();
        let stuck = run_status(&declared, &variables);
        assert_eq!(stuck.state, "failed");
        assert!(stuck.detail.unwrap().contains("did not answer"));
        assert!(asked.elapsed() < STATUS_TIMEOUT + Duration::from_secs(2));
    }

    #[test]
    fn status_output_is_parsed_defensively() {
        let ok = parse_status(br#"{"state":"running","url":"https://x.test/","detail":"3 pods"}"#);
        assert_eq!(
            (ok.state, ok.url.as_deref(), ok.detail.as_deref()),
            ("running", Some("https://x.test/"), Some("3 pods"))
        );
        assert_eq!(parse_status(b"nope").state, "failed");
        assert_eq!(parse_status(br#"{"state":"weird"}"#).state, "failed");
        assert_eq!(
            parse_status(br#"{"state":"stopped","url":"javascript:x"}"#).url,
            None
        );
    }

    #[test]
    fn instances_keep_this_run_only() {
        let temp = Temp::new("instances");
        let instances = Instances::at(&temp.0);
        let instance = |env: &str| Instance {
            project: "p".into(),
            env: env.into(),
            action: "process".into(),
            run: "r".into(),
            at: 1,
        };
        instances.set("old", 3, Some(instance("a"))).unwrap();
        instances.set("new", 4, Some(instance("b"))).unwrap();
        assert!(instances.panes("old").is_empty());
        assert_eq!(instances.panes("new")[&4].env, "b");
        instances.set("new", 4, None).unwrap();
        assert!(instances.panes("new").is_empty());
    }
}
