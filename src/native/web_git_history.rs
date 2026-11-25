//! Read-only repository history and diffs for the web client's Repos view
//! (bridge API minor 10): a checkout's branches, its log, a commit's
//! details, per-file diffs of a commit or of the working changes, and the
//! working changes themselves (staged, unstaged, untracked).
//!
//! Every Git process goes through the hardened runner of `web_git`
//! ([`Checkout::run`]: fixed binary, cleared environment, command-line
//! overrides of every program a configuration can name, no network, no
//! index refresh, a deadline and bounded output); see that module's threat
//! model. On top of it:
//!
//! - the browser names a checkout only by the path `git` reported for one of
//!   its project's checkouts: the bridge discovers them again and refuses any
//!   other path ([`task`]);
//! - a revision must be hexadecimal (4 to 64 digits) or a ref name under
//!   `git check-ref-format`'s rules, never starting with `-` and without
//!   `@{`, `..`, `:`, `^`, `~` or any other revision syntax
//!   ([`valid_revision`]), and is resolved to a commit with `rev-parse
//!   --verify --end-of-options <rev>^{commit}` before use: later commands
//!   only see the full hash it returned;
//! - a path must be relative, without empty, `.`, `..` or `.git`
//!   components, control characters or a leading `-` or `:` ([`valid_path`]),
//!   and is passed after `--` with `--literal-pathspecs` (no pathspec magic,
//!   no globbing);
//! - every diff is plumbing (`diff-tree`, `diff-index`, `diff-files`, which
//!   never refresh the index) with `--no-ext-diff --no-textconv --no-color`,
//!   `core.quotePath=false` and fixed prefixes, and names come from `-z`
//!   listings, never from patch headers;
//! - untracked files are read directly, without following symbolic links,
//!   on the checkout's file system and within a size limit (`web_git`'s
//!   `read_untracked`);
//! - output is capped per file ([`FILE_BYTES`]) and per response
//!   ([`RESPONSE_BYTES`]), listings by count, and each cap says so.
use super::web_git::{self, Checkout, Untracked};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::Instant,
};

type Result<T> = std::result::Result<T, String>;

pub const INVALID_REVISION: &str = "invalid revision";
pub const UNKNOWN_REVISION: &str = "unknown revision";
pub const INVALID_PATH: &str = "invalid path: a relative path inside the repository";
pub const UNKNOWN_REPOSITORY: &str = "this repository is not one of the project's checkouts";

/// Commits per log page, by default and at most.
pub const LOG_DEFAULT: u64 = 50;
pub const LOG_LIMIT: u64 = 200;
/// Furthest a log page may start.
pub const SKIP_LIMIT: u64 = 100_000;
/// Branches listed per kind (local, remote-tracking).
pub const BRANCH_LIMIT: usize = 300;
/// Files listed per commit or working-change section.
pub const FILE_LIMIT: usize = 1000;
/// Patch text kept per file and per response; beyond either, the diff is
/// marked truncated.
pub const FILE_BYTES: usize = 512 * 1024;
pub const RESPONSE_BYTES: usize = 1024 * 1024;
/// Lines kept per file's diff.
pub const FILE_LINES: usize = 20_000;
/// Untracked files larger than this are listed, not shown.
pub const UNTRACKED_SHOW: u64 = 256 * 1024;
/// A commit message kept.
const MESSAGE_BYTES: usize = 64 * 1024;
/// Output of listings (`for-each-ref`, `log`, name-status, numstat).
const LISTING_BYTES: usize = 2 * 1024 * 1024;
/// Refs named per commit.
const REFS_LIMIT: usize = 32;

/// A full or abbreviated object name.
pub fn valid_hex(revision: &str) -> bool {
    (4..=64).contains(&revision.len()) && revision.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A ref name under `git check-ref-format`'s rules (with
/// `--allow-onelevel`), and never an option: no leading `-` or `/`, no
/// ASCII control character, space, `~ ^ : ? * [ \`, no `..`, `@{` or `//`,
/// not `@`, no component starting with `.` or ending with `.lock`, no
/// trailing `/` or `.`; at most 255 bytes.
pub fn valid_ref_name(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 255
        || name == "@"
        || name.starts_with(['-', '/'])
        || name.ends_with(['/', '.'])
        || name.contains("..")
        || name.contains("@{")
        || name.contains("//")
    {
        return false;
    }
    if name
        .bytes()
        .any(|b| b < 0x20 || b == 0x7f || b" ~^:?*[\\".contains(&b))
    {
        return false;
    }
    name.split('/')
        .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

/// What a browser may name as a revision: hexadecimal or a ref name.
pub fn valid_revision(revision: &str) -> bool {
    valid_hex(revision) || valid_ref_name(revision)
}

/// A path inside the repository, as the browser may name one: relative,
/// at most 4096 bytes, no control characters, no leading `-` or `:`, no
/// empty, `.`, `..` or `.git` component.
pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.starts_with(['-', ':', '/'])
        && !path.chars().any(char::is_control)
        && path.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && !part.eq_ignore_ascii_case(".git")
        })
}

fn is_hash(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Printable text: control characters dropped, at most `limit` characters.
fn clean(text: &str, limit: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect()
}

/// Which diff [`Query::Diff`] asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffKind {
    /// A commit against its first parent (or the empty tree).
    Commit(String),
    /// The index against HEAD (or the empty tree before the first commit).
    Staged,
    /// The working tree against the index.
    Unstaged,
    /// An untracked file's content.
    Untracked,
}

/// One validated history request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Query {
    Branches,
    Log {
        rev: String,
        skip: u64,
        limit: u64,
        first_parent: bool,
        path: Option<String>,
    },
    Commit {
        rev: String,
    },
    Diff {
        kind: DiffKind,
        path: String,
        from: Option<String>,
    },
    Changes,
}

/// The history operations, as the bridge reports and routes them.
pub const METHODS: &[&str] = &[
    "git_branches",
    "git_log",
    "git_commit",
    "git_diff",
    "git_changes",
];

fn revision_param(p: &Value, key: &str) -> Result<Option<String>> {
    match &p[key] {
        Value::Null => Ok(None),
        Value::String(rev) if valid_revision(rev) => Ok(Some(rev.clone())),
        _ => Err(INVALID_REVISION.into()),
    }
}

fn path_param(p: &Value, key: &str) -> Result<Option<String>> {
    match &p[key] {
        Value::Null => Ok(None),
        Value::String(path) if valid_path(path) => Ok(Some(path.clone())),
        _ => Err(INVALID_PATH.into()),
    }
}

fn number_param(p: &Value, key: &str, default: u64, max: u64) -> Result<u64> {
    match &p[key] {
        Value::Null => Ok(default),
        value => value
            .as_u64()
            .filter(|n| *n <= max)
            .ok_or_else(|| format!("{key} must be a number from 0 to {max}")),
    }
}

/// Validates a history request's parameters (everything but the checkout,
/// which [`task`] checks against the project's discovered checkouts).
pub fn parse(method: &str, p: &Value) -> Result<Query> {
    match method {
        "git_branches" => Ok(Query::Branches),
        "git_changes" => Ok(Query::Changes),
        "git_log" => {
            let limit = number_param(p, "limit", LOG_DEFAULT, LOG_LIMIT)?;
            if limit == 0 {
                return Err(format!("limit must be a number from 1 to {LOG_LIMIT}"));
            }
            Ok(Query::Log {
                rev: revision_param(p, "rev")?.unwrap_or_else(|| "HEAD".into()),
                skip: number_param(p, "skip", 0, SKIP_LIMIT)?,
                limit,
                first_parent: p["first_parent"] == true,
                path: path_param(p, "path")?,
            })
        }
        "git_commit" => Ok(Query::Commit {
            rev: revision_param(p, "rev")?.ok_or(INVALID_REVISION)?,
        }),
        "git_diff" => {
            let kind = match p["kind"].as_str() {
                Some("commit") => {
                    DiffKind::Commit(revision_param(p, "rev")?.ok_or(INVALID_REVISION)?)
                }
                Some("staged") => DiffKind::Staged,
                Some("unstaged") => DiffKind::Unstaged,
                Some("untracked") => DiffKind::Untracked,
                _ => return Err("kind must be commit, staged, unstaged or untracked".into()),
            };
            let path = path_param(p, "path")?.ok_or(INVALID_PATH)?;
            let from = path_param(p, "from")?.filter(|from| *from != path);
            Ok(Query::Diff { kind, path, from })
        }
        _ => Err("unsupported web operation".into()),
    }
}

/// Splits `-z`-style output into records of `fields` NUL-terminated
/// fields, as `--format=…%x00` writes them one per line: the newline Git
/// adds after each record is removed from the next one's first field. A
/// truncated output loses its last record, which may be cut short.
fn records(bytes: &[u8], fields: usize, truncated: bool) -> Vec<Vec<String>> {
    let parts: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    let mut out: Vec<Vec<String>> = parts
        .chunks(fields)
        .filter(|chunk| chunk.len() == fields)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .map(|(i, field)| {
                    let field = if i == 0 {
                        field.strip_prefix(b"\n").unwrap_or(field)
                    } else {
                        field
                    };
                    web_git::text(field)
                })
                .collect()
        })
        .collect();
    if truncated {
        out.pop();
    }
    out
}

/// Whether a runner error is Git's own refusal (rather than a timeout).
fn refused(error: &str) -> bool {
    error.starts_with("Git: ") || error.starts_with("Git exited")
}

/// Resolves a validated revision to the full hash of its commit.
pub fn resolve(checkout: &Checkout, revision: &str) -> Result<String> {
    if !valid_revision(revision) {
        return Err(INVALID_REVISION.into());
    }
    let spec = format!("{revision}^{{commit}}");
    let out = checkout
        .run(
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &spec,
            ],
            4096,
        )
        .map_err(|error| {
            if refused(&error) {
                UNKNOWN_REVISION.to_owned()
            } else {
                error
            }
        })?;
    let hash = web_git::text(&out.bytes).trim().to_owned();
    if out.truncated || !is_hash(&hash) {
        return Err(UNKNOWN_REVISION.into());
    }
    Ok(hash)
}

/// HEAD's commit, `None` before the first commit.
fn head(checkout: &Checkout) -> Result<Option<String>> {
    match resolve(checkout, "HEAD") {
        Ok(hash) => Ok(Some(hash)),
        Err(error) if error == UNKNOWN_REVISION => Ok(None),
        Err(error) => Err(error),
    }
}

/// One branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Branch {
    /// Short name (`main`, `origin/main`).
    pub name: String,
    /// Full ref (`refs/heads/main`).
    #[serde(rename = "ref")]
    pub refname: String,
    pub current: bool,
    pub upstream: Option<String>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    /// The upstream is configured but its ref is gone.
    pub gone: bool,
    pub sha: String,
    pub subject: String,
    pub author: String,
    /// Committer time, Unix seconds.
    pub time: i64,
}

/// `ahead 2, behind 1` (`%(upstream:track,nobracket)`).
fn track(upstream: &str, text: &str) -> (Option<u64>, Option<u64>, bool) {
    if upstream.is_empty() {
        return (None, None, false);
    }
    if text == "gone" {
        return (None, None, true);
    }
    let (mut ahead, mut behind) = (0, 0);
    for part in text.split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.parse().unwrap_or(0);
        }
    }
    (Some(ahead), Some(behind), false)
}

const BRANCH_FORMAT: &str = "--format=%(refname)%00%(refname:short)%00%(HEAD)%00%(upstream:short)%00%(upstream:track,nobracket)%00%(objectname)%00%(contents:subject)%00%(authorname)%00%(committerdate:unix)%00%(symref)%00";

/// Parses [`BRANCH_FORMAT`] records: local and remote-tracking branches,
/// symbolic refs (`origin/HEAD`) left out.
pub fn parse_branches(bytes: &[u8], truncated: bool) -> (Vec<Branch>, Vec<Branch>) {
    let (mut local, mut remote) = (Vec::new(), Vec::new());
    for f in records(bytes, 10, truncated) {
        if !f[9].is_empty() {
            continue;
        }
        let (ahead, behind, gone) = track(&f[3], &f[4]);
        let branch = Branch {
            name: clean(&f[1], 255),
            refname: clean(&f[0], 255),
            current: f[2] == "*",
            upstream: (!f[3].is_empty()).then(|| clean(&f[3], 255)),
            ahead,
            behind,
            gone,
            sha: clean(&f[5], 64),
            subject: clean(&f[6], 200),
            author: clean(&f[7], 200),
            time: f[8].trim().parse().unwrap_or(0),
        };
        if f[0].starts_with("refs/heads/") {
            local.push(branch);
        } else if f[0].starts_with("refs/remotes/") {
            remote.push(branch);
        }
    }
    // The current branch first, then by last commit (Git's order).
    local.sort_by_key(|b| !b.current);
    (local, remote)
}

/// The checkout's branches: local ones (current first, with upstream and
/// ahead/behind as of the last fetch), then remote-tracking ones, each with
/// its last commit, newest first.
pub fn branches(checkout: &Checkout) -> Result<Value> {
    let out = checkout.run(
        &[
            "for-each-ref",
            "--sort=-committerdate",
            BRANCH_FORMAT,
            "--end-of-options",
            "refs/heads",
            "refs/remotes",
        ],
        LISTING_BYTES,
    )?;
    let (mut local, mut remote) = parse_branches(&out.bytes, out.truncated);
    let truncated = out.truncated || local.len() > BRANCH_LIMIT || remote.len() > BRANCH_LIMIT;
    local.truncate(BRANCH_LIMIT);
    remote.truncate(BRANCH_LIMIT);
    let head = head(checkout)?;
    let current = local.iter().find(|b| b.current).map(|b| b.name.clone());
    Ok(json!({
        "head": head,
        "current": current,
        "detached": current.is_none() && head.is_some(),
        "branches": local,
        "remotes": remote,
        "truncated": truncated,
    }))
}

/// One commit of a log page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LogEntry {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    /// Author time, Unix seconds.
    pub time: i64,
    pub subject: String,
    /// Decorations: `HEAD -> main`, `origin/main`, `tag: v1`.
    pub refs: Vec<String>,
}

const LOG_FORMAT: &str = "--format=%H%x00%P%x00%an%x00%at%x00%D%x00%s%x00";

pub fn parse_log(bytes: &[u8], truncated: bool) -> Vec<LogEntry> {
    records(bytes, 6, truncated)
        .into_iter()
        .filter(|f| is_hash(f[0].trim()))
        .map(|f| LogEntry {
            sha: f[0].trim().to_owned(),
            parents: f[1]
                .split_whitespace()
                .filter(|p| is_hash(p))
                .map(str::to_owned)
                .collect(),
            author: clean(&f[2], 200),
            time: f[3].trim().parse().unwrap_or(0),
            refs: f[4]
                .split(", ")
                .filter(|r| !r.is_empty())
                .take(REFS_LIMIT)
                .map(|r| clean(r, 255))
                .collect(),
            subject: clean(&f[5], 500),
        })
        .collect()
}

/// A page of the history of `rev` (resolved first), `limit` commits after
/// `skip`, optionally first-parent only or limited to a path.
pub fn log(
    checkout: &Checkout,
    rev: &str,
    skip: u64,
    limit: u64,
    first_parent: bool,
    path: Option<&str>,
) -> Result<Value> {
    let hash = resolve(checkout, rev)?;
    let skip_arg = format!("--skip={skip}");
    let count_arg = format!("--max-count={}", limit + 1);
    let mut args = vec![
        "--literal-pathspecs",
        "-c",
        "log.follow=false",
        "log",
        "--no-color",
        "--no-show-signature",
        "--no-mailmap",
        LOG_FORMAT,
        &skip_arg,
        &count_arg,
    ];
    if first_parent {
        args.push("--first-parent");
    }
    args.extend(["--end-of-options", &hash, "--"]);
    if let Some(path) = path {
        args.push(path);
    }
    let out = checkout.run(&args, LISTING_BYTES)?;
    let mut commits = parse_log(&out.bytes, out.truncated);
    let more = out.truncated || commits.len() as u64 > limit;
    commits.truncate(limit as usize);
    Ok(json!({
        "rev": rev,
        "head": hash,
        "skip": skip,
        "first_parent": first_parent,
        "path": path,
        "commits": commits,
        "more": more,
    }))
}

/// One changed file of a commit or of the working changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileChange {
    pub path: String,
    /// A rename's or copy's original path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// `A`, `M`, `D`, `R`, `C`, `T`, `U`.
    pub status: String,
    pub added: Option<u64>,
    pub removed: Option<u64>,
    pub binary: bool,
}

/// `--name-status -z` records: `(status letter, from, path)`.
pub fn parse_name_status(bytes: &[u8], truncated: bool) -> Vec<(String, Option<String>, String)> {
    let mut tokens: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    if truncated {
        tokens.pop();
    }
    let mut tokens = tokens.into_iter();
    let mut out = Vec::new();
    while let Some(code) = tokens.next() {
        let Some(letter) = code.first().copied() else {
            continue;
        };
        let letter = if letter.is_ascii_uppercase() {
            (letter as char).to_string()
        } else {
            "M".to_owned()
        };
        if matches!(letter.as_str(), "R" | "C") {
            let (Some(from), Some(to)) = (tokens.next(), tokens.next()) else {
                break;
            };
            out.push((letter, Some(web_git::text(from)), web_git::text(to)));
        } else {
            let Some(path) = tokens.next() else { break };
            out.push((letter, None, web_git::text(path)));
        }
    }
    out
}

/// `--numstat -z` records with rename detection: new path → lines added and
/// removed, `None` for a binary file. A rename's record is
/// `added\tremoved\t\0from\0to\0`.
pub fn parse_numstat(bytes: &[u8]) -> HashMap<String, Option<(u64, u64)>> {
    let mut tokens = bytes.split(|b| *b == 0);
    let mut counts = HashMap::new();
    let number = |field: &str| field.parse::<u64>().ok();
    while let Some(token) = tokens.next() {
        let token = web_git::text(token);
        let mut parts = token.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            tokens.next();
            match tokens.next() {
                Some(to) => web_git::text(to),
                None => break,
            }
        } else {
            path.to_owned()
        };
        counts.insert(path, number(added).zip(number(removed)));
    }
    counts
}

/// Joins name-status and numstat listings.
fn file_changes(
    names: Vec<(String, Option<String>, String)>,
    counts: &HashMap<String, Option<(u64, u64)>>,
) -> Vec<FileChange> {
    names
        .into_iter()
        .map(|(status, from, path)| {
            let counted = counts.get(&path).copied();
            FileChange {
                binary: matches!(counted, Some(None)),
                added: counted.flatten().map(|c| c.0),
                removed: counted.flatten().map(|c| c.1),
                path,
                from,
                status,
            }
        })
        .collect()
}

/// Options every listing and patch of a diff gets: no external programs,
/// no colour, rename detection, submodules left alone.
const DIFF_OPTIONS: &[&str] = &[
    "--no-ext-diff",
    "--no-textconv",
    "--no-color",
    "--ignore-submodules=all",
];

/// The files a diff changes (name-status and numstat), at most
/// [`FILE_LIMIT`]. `base` is the plumbing command and its fixed arguments.
fn listing(checkout: &Checkout, base: &[&str], paths: &[&str]) -> Result<(Vec<FileChange>, bool)> {
    let run = |mode: &str| {
        let mut args = vec![
            "--literal-pathspecs",
            "-c",
            "core.quotePath=false",
            base[0],
            "-z",
            mode,
        ];
        args.extend_from_slice(DIFF_OPTIONS);
        args.extend_from_slice(&base[1..]);
        args.push("--");
        args.extend_from_slice(paths);
        checkout.run(&args, LISTING_BYTES)
    };
    let names = run("--name-status")?;
    let counts = run("--numstat")?;
    let mut files = file_changes(
        parse_name_status(&names.bytes, names.truncated),
        &parse_numstat(&counts.bytes),
    );
    let truncated = names.truncated || files.len() > FILE_LIMIT;
    files.truncate(FILE_LIMIT);
    Ok((files, truncated))
}

/// The first parent of a resolved commit, `None` for a root commit.
fn parents(checkout: &Checkout, hash: &str) -> Result<Vec<String>> {
    let out = checkout.run(
        &[
            "log",
            "-1",
            "--no-show-signature",
            "--format=%P",
            "--end-of-options",
            hash,
            "--",
        ],
        4096,
    )?;
    Ok(web_git::text(&out.bytes)
        .split_whitespace()
        .filter(|p| is_hash(p))
        .map(str::to_owned)
        .collect())
}

/// `diff-tree` from a commit's first parent (or `--root`) to the commit.
fn commit_base<'a>(hash: &'a str, parent: Option<&'a str>) -> Vec<&'a str> {
    match parent {
        Some(parent) => vec!["diff-tree", "-r", "-M", "--no-commit-id", parent, hash],
        None => vec!["diff-tree", "-r", "-M", "--no-commit-id", "--root", hash],
    }
}

/// A commit's details: message, author and committer, parents, refs, and
/// its changed files against its first parent (a merge says so).
pub fn commit(checkout: &Checkout, rev: &str) -> Result<Value> {
    let hash = resolve(checkout, rev)?;
    let out = checkout.run(
        &[
            "log",
            "-1",
            "--no-color",
            "--no-show-signature",
            "--no-mailmap",
            "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%D%x00%B",
            "--end-of-options",
            &hash,
            "--",
        ],
        MESSAGE_BYTES + 4096,
    )?;
    let text = web_git::text(&out.bytes);
    let f: Vec<&str> = text.splitn(10, '\0').collect();
    if f.len() != 10 {
        return Err("Git returned incomplete commit details".into());
    }
    let parent_list: Vec<String> = f[1]
        .split_whitespace()
        .filter(|p| is_hash(p))
        .map(str::to_owned)
        .collect();
    let (files, files_truncated) = listing(
        checkout,
        &commit_base(&hash, parent_list.first().map(String::as_str)),
        &[],
    )?;
    let message: String = f[9]
        .trim_end()
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    Ok(json!({
        "sha": hash,
        "parents": parent_list,
        "merge": parent_list.len() > 1,
        "author": clean(f[2], 200),
        "author_email": clean(f[3], 200),
        "author_time": f[4].trim().parse::<i64>().unwrap_or(0),
        "committer": clean(f[5], 200),
        "committer_email": clean(f[6], 200),
        "commit_time": f[7].trim().parse::<i64>().unwrap_or(0),
        "refs": f[8].split(", ").filter(|r| !r.is_empty()).take(REFS_LIMIT).map(|r| clean(r, 255)).collect::<Vec<_>>(),
        "message": message,
        "message_truncated": out.truncated,
        "files": files,
        "files_truncated": files_truncated,
    }))
}

/// One hunk of a unified diff. `lines` keep their first character: ` `
/// context, `+` added, `-` removed, `\` a "No newline" note.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Hunk {
    pub header: String,
    pub old_start: u64,
    pub old_lines: u64,
    pub new_start: u64,
    pub new_lines: u64,
    pub lines: Vec<String>,
}

/// One file of a diff.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct FileDiff {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub status: String,
    pub binary: bool,
    /// Extended header lines (modes, similarity, rename), for display.
    pub meta: Vec<String>,
    pub hunks: Vec<Hunk>,
    /// Some of the diff was left out (file or response cap).
    pub truncated: bool,
    /// Untracked and too large to show: its size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub too_large: Option<u64>,
}

/// `-a,b` or `+c,d` of a hunk header.
fn range(text: &str) -> (u64, u64) {
    let text = &text[1.min(text.len())..];
    match text.split_once(',') {
        Some((start, count)) => (start.parse().unwrap_or(0), count.parse().unwrap_or(0)),
        None => (text.parse().unwrap_or(0), 1),
    }
}

/// Splits a patch into files and hunks, keeping at most [`FILE_BYTES`] and
/// [`FILE_LINES`] of each file and [`RESPONSE_BYTES`] in all. `truncated`
/// says the output was cut: its last line, maybe partial, is dropped and
/// its last file marked.
pub fn parse_patch(bytes: &[u8], truncated: bool) -> Vec<FileDiff> {
    let mut lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
    if truncated || lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let mut files: Vec<FileDiff> = Vec::new();
    let (mut file_bytes, mut file_lines, mut total) = (0usize, 0usize, 0usize);
    for line in lines {
        if line.starts_with(b"diff --git ") || line.starts_with(b"diff --cc ") {
            files.push(FileDiff::default());
            file_bytes = 0;
            file_lines = 0;
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        let text = web_git::text(line);
        if line.starts_with(b"@@ ") {
            let mut parts = text.split(' ');
            let (_, old, new) = (parts.next(), parts.next(), parts.next());
            let (old_start, old_lines) = range(old.unwrap_or("-0,0"));
            let (new_start, new_lines) = range(new.unwrap_or("+0,0"));
            if file.truncated {
                continue;
            }
            file.hunks.push(Hunk {
                header: clean(&text, 500),
                old_start,
                old_lines,
                new_start,
                new_lines,
                lines: Vec::new(),
            });
            continue;
        }
        if file.hunks.is_empty() {
            if line.starts_with(b"Binary files ") || line.starts_with(b"GIT binary patch") {
                file.binary = true;
            } else if !line.starts_with(b"--- ")
                && !line.starts_with(b"+++ ")
                && !line.starts_with(b"index ")
                && file.meta.len() < 16
            {
                file.meta.push(clean(&text, 500));
            }
            continue;
        }
        if file.truncated {
            continue;
        }
        if file_bytes + line.len() > FILE_BYTES
            || file_lines >= FILE_LINES
            || total + line.len() > RESPONSE_BYTES
        {
            file.truncated = true;
            continue;
        }
        file_bytes += line.len() + 1;
        file_lines += 1;
        total += line.len() + 1;
        let text = if text.is_empty() {
            " ".to_owned()
        } else {
            text
        };
        if let Some(hunk) = file.hunks.last_mut() {
            hunk.lines.push(text);
        }
    }
    if truncated && let Some(file) = files.last_mut() {
        file.truncated = true;
    }
    files
}

/// The diff of one file (and, for a rename, its original path): its
/// listing gives names and statuses, its patch the hunks, matched in order.
fn plumbing_diff(checkout: &Checkout, base: &[&str], paths: &[&str]) -> Result<Vec<FileDiff>> {
    let (names, _) = listing(checkout, base, paths)?;
    let mut args = vec![
        "--literal-pathspecs",
        "-c",
        "core.quotePath=false",
        base[0],
        "-p",
        "-U3",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ];
    args.extend_from_slice(DIFF_OPTIONS);
    args.extend_from_slice(&base[1..]);
    args.push("--");
    args.extend_from_slice(paths);
    let out = checkout.run(&args, RESPONSE_BYTES)?;
    let mut files = parse_patch(&out.bytes, out.truncated);
    for (index, file) in files.iter_mut().enumerate() {
        match names.get(index) {
            Some(name) => {
                file.path = name.path.clone();
                file.from = name.from.clone();
                file.status = name.status.clone();
                file.binary |= name.binary;
            }
            None => {
                file.path = paths.first().copied().unwrap_or_default().to_owned();
                file.status = "M".into();
            }
        }
    }
    // A file only its stat data changed (diff-files) has no hunks.
    files.retain(|file| !file.hunks.is_empty() || file.binary || !file.meta.is_empty());
    Ok(files)
}

/// An untracked file's content as an all-added diff: text up to
/// [`UNTRACKED_SHOW`] bytes; a larger or binary file is summarised.
fn untracked_diff(checkout: &Checkout, path: &str) -> Result<Vec<FileDiff>> {
    let out = checkout.run(
        &[
            "--literal-pathspecs",
            "ls-files",
            "-z",
            "--others",
            "--exclude-standard",
            "--",
            path,
        ],
        64 * 1024,
    )?;
    if !out
        .bytes
        .split(|b| *b == 0)
        .any(|name| name == path.as_bytes())
    {
        return Err("this file is not untracked".into());
    }
    let root = checkout.root();
    let device = std::fs::metadata(root)
        .map_err(|_| "the checkout is unavailable")?
        .dev();
    let mut file = FileDiff {
        path: path.to_owned(),
        status: "?".into(),
        ..FileDiff::default()
    };
    match web_git::read_untracked(root, device, path, UNTRACKED_SHOW) {
        None => {
            return Err("this file cannot be shown (not a regular file in the checkout)".into());
        }
        Some(Untracked::TooLarge(size)) => file.too_large = Some(size),
        Some(Untracked::Content(bytes)) => {
            if web_git::text_lines(&bytes).is_none() {
                file.binary = true;
            } else if !bytes.is_empty() {
                let text = web_git::text(&bytes);
                let mut lines: Vec<String> = text
                    .strip_suffix('\n')
                    .unwrap_or(&text)
                    .split('\n')
                    .map(|line| format!("+{line}"))
                    .collect();
                if !text.ends_with('\n') {
                    lines.push("\\ No newline at end of file".into());
                }
                if lines.len() > FILE_LINES {
                    lines.truncate(FILE_LINES);
                    file.truncated = true;
                }
                let count = lines.iter().filter(|l| l.starts_with('+')).count() as u64;
                file.hunks.push(Hunk {
                    header: format!("@@ -0,0 +1,{count} @@"),
                    old_start: 0,
                    old_lines: 0,
                    new_start: 1,
                    new_lines: count,
                    lines,
                });
            }
        }
    }
    Ok(vec![file])
}

/// One file's diff: of a commit (against its first parent), of the staged
/// or unstaged changes, or an untracked file's content.
pub fn diff(checkout: &Checkout, kind: &DiffKind, path: &str, from: Option<&str>) -> Result<Value> {
    let mut paths = vec![path];
    paths.extend(from);
    let (files, sha) = match kind {
        DiffKind::Commit(rev) => {
            let hash = resolve(checkout, rev)?;
            let parent = parents(checkout, &hash)?.into_iter().next();
            let files = plumbing_diff(checkout, &commit_base(&hash, parent.as_deref()), &paths)?;
            (files, Some(hash))
        }
        DiffKind::Staged => {
            let base = head(checkout)?.unwrap_or_else(|| checkout.empty_tree().to_owned());
            let files = plumbing_diff(checkout, &["diff-index", "--cached", "-M", &base], &paths)?;
            (files, None)
        }
        DiffKind::Unstaged => (plumbing_diff(checkout, &["diff-files"], &paths)?, None),
        DiffKind::Untracked => (untracked_diff(checkout, path)?, None),
    };
    let kind_name = match kind {
        DiffKind::Commit(_) => "commit",
        DiffKind::Staged => "staged",
        DiffKind::Unstaged => "unstaged",
        DiffKind::Untracked => "untracked",
    };
    Ok(json!({
        "kind": kind_name,
        "sha": sha,
        "path": path,
        "from": from,
        "truncated": files.iter().any(|f| f.truncated),
        "files": files,
    }))
}

/// One side of a porcelain v2 status record: staged (`X`) or unstaged (`Y`).
#[derive(Debug, Default, PartialEq)]
pub struct StatusSides {
    /// `(status, from, path)` of the index against HEAD.
    pub staged: Vec<(String, Option<String>, String)>,
    /// `(status, path)` of the working tree against the index.
    pub unstaged: Vec<(String, String)>,
    pub untracked: Vec<String>,
    pub conflicted: Vec<String>,
}

/// Parses `status --porcelain=v2 -z` into its staged and unstaged sides.
pub fn parse_status_sides(bytes: &[u8], truncated: bool) -> StatusSides {
    let mut sides = StatusSides::default();
    let mut records: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    if truncated {
        records.pop();
    }
    let mut records = records.into_iter();
    while let Some(record) = records.next() {
        let fields = |n: usize| record.splitn(n, |b| *b == b' ').collect::<Vec<_>>();
        let (xy, path, from) = match record.first() {
            Some(b'1') => {
                let f = fields(9);
                if f.len() != 9 {
                    continue;
                }
                (f[1], web_git::text(f[8]), None)
            }
            Some(b'2') => {
                let f = fields(10);
                let from = records.next().map(web_git::text);
                if f.len() != 10 {
                    continue;
                }
                (f[1], web_git::text(f[9]), from)
            }
            Some(b'u') => {
                let f = fields(11);
                if f.len() == 11 {
                    sides.conflicted.push(web_git::text(f[10]));
                }
                continue;
            }
            Some(b'?') => {
                if let Some(path) = record.strip_prefix(b"? ") {
                    sides.untracked.push(web_git::text(path));
                }
                continue;
            }
            _ => continue,
        };
        let x = xy.first().copied().unwrap_or(b'.');
        let y = xy.get(1).copied().unwrap_or(b'.');
        if x != b'.' {
            let from = if matches!(x, b'R' | b'C') { from } else { None };
            sides
                .staged
                .push(((x as char).to_string(), from, path.clone()));
        }
        if y != b'.' {
            sides.unstaged.push(((y as char).to_string(), path));
        }
    }
    sides
}

/// The working changes: staged and unstaged files with their lines,
/// untracked files (lines counted for small text files) and conflicts.
pub fn changes(checkout: &Checkout) -> Result<Value> {
    let out = checkout.run(
        &[
            "-c",
            "core.quotePath=false",
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=all",
        ],
        LISTING_BYTES,
    )?;
    let sides = parse_status_sides(&out.bytes, out.truncated);
    let mut truncated = out.truncated;
    let head = head(checkout)?;
    let base = head
        .clone()
        .unwrap_or_else(|| checkout.empty_tree().to_owned());
    let numstat = |args: &[&str]| -> Result<HashMap<String, Option<(u64, u64)>>> {
        let mut all = vec![args[0], "-z", "--numstat"];
        all.extend_from_slice(DIFF_OPTIONS);
        all.extend_from_slice(&args[1..]);
        all.push("--");
        let out = checkout.run(&all, LISTING_BYTES)?;
        Ok(parse_numstat(&out.bytes))
    };
    let staged_counts = if sides.staged.is_empty() {
        HashMap::new()
    } else {
        numstat(&["diff-index", "--cached", "-M", &base])?
    };
    let unstaged_counts = if sides.unstaged.is_empty() {
        HashMap::new()
    } else {
        numstat(&["diff-files"])?
    };
    let mut staged = file_changes(sides.staged, &staged_counts);
    let mut unstaged = file_changes(
        sides
            .unstaged
            .into_iter()
            .map(|(status, path)| (status, None, path))
            .collect(),
        &unstaged_counts,
    );
    let device = std::fs::metadata(checkout.root()).map(|m| m.dev()).ok();
    let mut untracked: Vec<Value> = Vec::new();
    for path in sides.untracked.iter().take(FILE_LIMIT) {
        let content = (untracked.len() < 200 && Instant::now() < checkout.deadline())
            .then_some(device)
            .flatten()
            .and_then(|device| {
                web_git::read_untracked(checkout.root(), device, path, UNTRACKED_SHOW)
            });
        let (lines, size, binary) = match content {
            Some(Untracked::Content(bytes)) => {
                let lines = web_git::text_lines(&bytes);
                (lines, Some(bytes.len() as u64), lines.is_none())
            }
            Some(Untracked::TooLarge(size)) => (None, Some(size), false),
            None => (None, None, false),
        };
        untracked.push(json!({"path": path, "added": lines, "size": size, "binary": binary}));
    }
    truncated |= staged.len() > FILE_LIMIT
        || unstaged.len() > FILE_LIMIT
        || sides.untracked.len() > FILE_LIMIT;
    staged.truncate(FILE_LIMIT);
    unstaged.truncate(FILE_LIMIT);
    let mut conflicted = sides.conflicted;
    conflicted.truncate(FILE_LIMIT);
    Ok(json!({
        "head": head,
        "staged": staged,
        "unstaged": unstaged,
        "untracked": untracked,
        "conflicted": conflicted,
        "truncated": truncated,
    }))
}

/// Runs a validated query in a verified checkout.
pub fn run(checkout: &Checkout, query: &Query) -> Result<Value> {
    match query {
        Query::Branches => branches(checkout),
        Query::Log {
            rev,
            skip,
            limit,
            first_parent,
            path,
        } => log(checkout, rev, *skip, *limit, *first_parent, path.as_deref()),
        Query::Commit { rev } => commit(checkout, rev),
        Query::Diff { kind, path, from } => diff(checkout, kind, path, from.as_deref()),
        Query::Changes => changes(checkout),
    }
}

/// The Git thread's work for a history request on the checkout `repo` of
/// the project folder `folder`: the checkout must be one the project's
/// discovery finds now (the folder itself or a direct subfolder), opened
/// and verified like the Repos view's, within the request's
/// [`web_git::BUDGET`].
pub fn task(folder: PathBuf, repo: String, query: Query) -> web_git::Task {
    Box::new(move |_cache: &web_git::Cache| {
        let uid = unsafe { libc::geteuid() };
        let deadline = Instant::now() + web_git::BUDGET;
        let wanted = Path::new(&repo);
        let (roots, _) = web_git::discover(&folder, uid);
        let root = roots
            .into_iter()
            .find(|root| root == wanted)
            .ok_or(UNKNOWN_REPOSITORY)?;
        let checkout = Checkout::open(&root, uid, root == folder, deadline)?;
        let mut value = run(&checkout, &query)?;
        value["repo"] = json!(repo);
        Ok(value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        process::{Command, Stdio},
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    #[test]
    fn revisions_are_hex_or_ref_names_never_options_or_syntax() {
        for good in [
            "HEAD",
            "main",
            "feat/repo-history",
            "origin/main",
            "refs/heads/main",
            "v1.2.3",
            "abcd",
            "4f2a9c1e8b7d6a5f4e3d2c1b0a9f8e7d6c5b4a39",
            "release-2026_11",
            "café",
        ] {
            assert!(valid_revision(good), "{good}");
        }
        for bad in [
            "",
            "-c",
            "--output=x",
            "--end-of-options",
            "-",
            "HEAD:../",
            "HEAD:src/main.rs",
            "@{-1}",
            "main@{1}",
            "@",
            "HEAD~1",
            "HEAD^",
            "HEAD^{tree}",
            "main..evil",
            "a...b",
            "/etc/passwd",
            "refs//heads",
            "refs/heads/",
            "main.",
            "main.lock",
            "refs/.hidden",
            ".git",
            "a b",
            "a\tb",
            "a\nb",
            "a\u{7f}b",
            "a\0b",
            "a?b",
            "a*b",
            "a[b",
            "a\\b",
            ":/message",
            "ma:in",
        ] {
            assert!(!valid_revision(bad), "{bad:?}");
        }
        assert!(!valid_revision(&"a".repeat(256)));
        assert!(valid_hex("abcdef0123"));
        assert!(!valid_hex("abc"));
        assert!(!valid_hex("xyz1"));
    }

    #[test]
    fn paths_are_relative_and_inside_the_repository() {
        for good in [
            "a.txt",
            "src/main.rs",
            "dir with space/f",
            "é/ü.md",
            "a..b",
            ".gitignore",
        ] {
            assert!(valid_path(good), "{good}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../outside",
            "a/../../b",
            "./a",
            "a/./b",
            "a//b",
            "a/",
            "-rf",
            "--output=x",
            ":(glob)*",
            ":/top",
            ".git/config",
            "sub/.GIT/hooks",
            "a\nb",
            "a\0b",
            "a\u{1b}[31m",
        ] {
            assert!(!valid_path(bad), "{bad:?}");
        }
        assert!(!valid_path(&"a/".repeat(2100)));
    }

    #[test]
    fn requests_are_checked_before_any_git() {
        let q = |method: &str, p: Value| parse(method, &p);
        assert_eq!(
            q("git_log", json!({})),
            Ok(Query::Log {
                rev: "HEAD".into(),
                skip: 0,
                limit: LOG_DEFAULT,
                first_parent: false,
                path: None
            })
        );
        assert_eq!(
            q("git_log", json!({"rev": "-c"})),
            Err(INVALID_REVISION.into())
        );
        assert_eq!(
            q("git_log", json!({"rev": "HEAD:../"})),
            Err(INVALID_REVISION.into())
        );
        assert_eq!(
            q("git_log", json!({"rev": 3})),
            Err(INVALID_REVISION.into())
        );
        assert!(q("git_log", json!({"limit": 0})).is_err());
        assert!(q("git_log", json!({"limit": 201})).is_err());
        assert!(q("git_log", json!({"skip": -1})).is_err());
        assert_eq!(
            q("git_log", json!({"path": "../x"})),
            Err(INVALID_PATH.into())
        );
        assert_eq!(q("git_commit", json!({})), Err(INVALID_REVISION.into()));
        assert_eq!(
            q("git_commit", json!({"rev": "@{-1}"})),
            Err(INVALID_REVISION.into())
        );
        assert!(q("git_diff", json!({"kind": "staged"})).is_err());
        assert!(q("git_diff", json!({"kind": "worktree", "path": "a"})).is_err());
        assert!(q("git_diff", json!({"kind": "commit", "path": "a"})).is_err());
        assert_eq!(
            q(
                "git_diff",
                json!({"kind": "unstaged", "path": "a", "from": "-x"})
            ),
            Err(INVALID_PATH.into())
        );
        assert_eq!(
            q(
                "git_diff",
                json!({"kind": "commit", "rev": "abcd", "path": "b", "from": "a"})
            ),
            Ok(Query::Diff {
                kind: DiffKind::Commit("abcd".into()),
                path: "b".into(),
                from: Some("a".into())
            })
        );
        assert!(q("git_push", json!({})).is_err());
    }

    #[test]
    fn parsers() {
        let (local, remote) = parse_branches(
            b"refs/heads/feat\0feat\0 \0origin/feat\0ahead 2, behind 1\0aaaa\0Work\0Ann\x00100\0\0\n\
              refs/heads/main\0main\0*\0origin/main\0\0bbbb\0Init\0Bob\x0090\0\0\n\
              refs/heads/old\0old\0 \0origin/old\0gone\0cccc\0Old\0Bob\x0080\0\0\n\
              refs/remotes/origin/HEAD\0origin/HEAD\0 \0\0\0bbbb\0Init\0Bob\x0090\0refs/remotes/origin/main\0\n\
              refs/remotes/origin/main\0origin/main\0 \0\0\0bbbb\0Init\0Bob\x0090\0\0\n",
            false,
        );
        assert_eq!(
            local.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            ["main", "feat", "old"]
        );
        assert!(local[0].current);
        assert_eq!((local[0].ahead, local[0].behind), (Some(0), Some(0)));
        assert_eq!((local[1].ahead, local[1].behind), (Some(2), Some(1)));
        assert!(local[2].gone);
        assert_eq!(remote.len(), 1);
        assert_eq!(remote[0].name, "origin/main");
        let log = parse_log(
            format!(
                "{a}\0{b} {c}\0Ann\x00100\0HEAD -> main, tag: v1\0Merge\0\n{b}\0\0Bob\x0090\0\0First\0\n",
                a = "a".repeat(40),
                b = "b".repeat(40),
                c = "c".repeat(40)
            )
            .as_bytes(),
            false,
        );
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].parents.len(), 2);
        assert_eq!(log[0].refs, ["HEAD -> main", "tag: v1"]);
        assert_eq!(log[1].subject, "First");
        assert!(log[1].parents.is_empty());
        assert_eq!(
            parse_name_status(b"M\0a b.rs\0R087\0old\0new\0A\0x\0", false),
            [
                ("M".into(), None, "a b.rs".into()),
                ("R".into(), Some("old".into()), "new".into()),
                ("A".into(), None, "x".into()),
            ]
        );
        let counts = parse_numstat(b"1\t2\ta.rs\0-\t-\timg.png\x003\t0\t\0old\0new\0");
        assert_eq!(counts["a.rs"], Some((1, 2)));
        assert_eq!(counts["img.png"], None);
        assert_eq!(counts["new"], Some((3, 0)));
        let sides = parse_status_sides(
            b"1 MM N... 100644 100644 100644 aa bb both.rs\0\
              2 R. N... 100644 100644 100644 aa bb R100 new.rs\0old.rs\0\
              1 .D N... 100644 100644 000000 aa aa gone.rs\0\
              u UU N... 100644 100644 100644 100644 aa bb cc c.rs\0? n.txt\0",
            false,
        );
        assert_eq!(
            sides.staged,
            [
                ("M".into(), None, "both.rs".into()),
                ("R".into(), Some("old.rs".into()), "new.rs".into())
            ]
        );
        assert_eq!(
            sides.unstaged,
            [
                ("M".into(), "both.rs".into()),
                ("D".into(), "gone.rs".into())
            ]
        );
        assert_eq!(sides.conflicted, ["c.rs"]);
        assert_eq!(sides.untracked, ["n.txt"]);
        let files = parse_patch(
            b"diff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n@@ -1,2 +1,3 @@ fn\n a\n-b\n+c\n+d\n\\ No newline at end of file\n\
              diff --git a/i.png b/i.png\nnew file mode 100644\nindex 0..1\nBinary files /dev/null and b/i.png differ\n",
            false,
        );
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].hunks[0].old_start, 1);
        assert_eq!(files[0].hunks[0].new_lines, 3);
        assert_eq!(
            files[0].hunks[0].lines,
            [" a", "-b", "+c", "+d", "\\ No newline at end of file"]
        );
        assert!(files[1].binary);
        assert_eq!(files[1].meta, ["new file mode 100644"]);
        // A cut output loses its partial last line and says so.
        let cut = parse_patch(b"diff --git a/x b/x\n@@ -1 +1 @@\n-a\n+partial", true);
        assert_eq!(cut[0].hunks[0].lines, ["-a"]);
        assert!(cut[0].truncated);
    }

    // ───── Real repositories ─────

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

    /// The test's own Git (not the hardened runner), ignoring the user's
    /// and system configuration, with fixed identities and dates.
    fn git(dir: &Path, args: &[&str]) {
        // Each call a minute later, so the history's order is fixed.
        static CLOCK: AtomicU64 = AtomicU64::new(0);
        let date = format!(
            "@{} +0000",
            1_700_000_000 + 60 * CLOCK.fetch_add(1, Ordering::Relaxed)
        );
        let status = Command::new(web_git::git_binary().expect("git is needed for these tests"))
            .args(args)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Ann")
            .env("GIT_AUTHOR_EMAIL", "ann@example.invalid")
            .env("GIT_COMMITTER_NAME", "Bob")
            .env("GIT_COMMITTER_EMAIL", "bob@example.invalid")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }
    fn commit_all(dir: &Path, message: &str) {
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", message]);
    }
    fn open(path: &Path) -> Checkout {
        let uid = unsafe { libc::geteuid() };
        Checkout::open(path, uid, true, Instant::now() + web_git::BUDGET).unwrap()
    }
    fn subjects(value: &Value) -> Vec<String> {
        value["commits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["subject"].as_str().unwrap().to_owned())
            .collect()
    }

    /// main: First → Second → Merge feat (feat: Feature) → Rename; plus a
    /// binary file and a large one.
    fn history_repo(temp: &Temp) -> PathBuf {
        let path = temp.0.join("repo");
        fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        fs::write(path.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        commit_all(&path, "First");
        fs::write(path.join("a.txt"), "one\n2\nthree\n").unwrap();
        commit_all(&path, "Second");
        git(&path, &["checkout", "-q", "-b", "feat"]);
        fs::write(path.join("feature.txt"), "f\n").unwrap();
        commit_all(&path, "Feature");
        git(&path, &["checkout", "-q", "main"]);
        fs::write(path.join("main.txt"), "m\n").unwrap();
        commit_all(&path, "Main work");
        git(
            &path,
            &["merge", "-q", "--no-ff", "-m", "Merge feat", "feat"],
        );
        git(&path, &["mv", "a.txt", "renamed.txt"]);
        fs::write(path.join("image.bin"), [0u8, 1, 2, 3, 0, 255]).unwrap();
        commit_all(&path, "Rename and binary");
        git(&path, &["tag", "v1"]);
        path
    }

    #[test]
    fn log_pages_first_parent_and_path_filter() {
        let temp = Temp::new();
        let path = history_repo(&temp);
        let checkout = open(&path);
        let all = log(&checkout, "HEAD", 0, 50, false, None).unwrap();
        assert_eq!(
            subjects(&all),
            [
                "Rename and binary",
                "Merge feat",
                "Main work",
                "Feature",
                "Second",
                "First"
            ]
        );
        assert_eq!(all["more"], false);
        let first = &all["commits"][0];
        assert!(
            first["refs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r == "HEAD -> main")
        );
        assert!(
            first["refs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r == "tag: v1")
        );
        assert_eq!(first["author"], "Ann");
        assert_eq!(all["commits"][1]["parents"].as_array().unwrap().len(), 2);
        // Pages.
        let page = log(&checkout, "HEAD", 2, 2, false, None).unwrap();
        assert_eq!(subjects(&page), ["Main work", "Feature"]);
        assert_eq!(page["more"], true);
        // First parent skips the feature branch's commit.
        let fp = log(&checkout, "main", 0, 50, true, None).unwrap();
        assert!(!subjects(&fp).contains(&"Feature".to_owned()));
        // A path filter, and another branch.
        let a = log(&checkout, "HEAD", 0, 50, false, Some("a.txt")).unwrap();
        assert_eq!(subjects(&a), ["Rename and binary", "Second", "First"]);
        let feat = log(&checkout, "feat", 0, 50, false, None).unwrap();
        assert_eq!(subjects(&feat)[0], "Feature");
        // Unknown and abbreviated revisions.
        assert_eq!(
            log(&checkout, "nope", 0, 5, false, None).unwrap_err(),
            UNKNOWN_REVISION
        );
        let short = &first["sha"].as_str().unwrap()[..7];
        assert_eq!(resolve(&checkout, short).unwrap(), first["sha"]);
        // A pathspec with magic characters is literal.
        assert_eq!(
            subjects(&log(&checkout, "HEAD", 0, 5, false, Some("*.txt")).unwrap()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn commit_details_renames_merges_and_binary() {
        let temp = Temp::new();
        let path = history_repo(&temp);
        let checkout = open(&path);
        let top = commit(&checkout, "HEAD").unwrap();
        assert_eq!(top["message"], "Rename and binary");
        assert_eq!(top["author"], "Ann");
        assert_eq!(top["author_email"], "ann@example.invalid");
        assert_eq!(top["committer"], "Bob");
        assert_eq!(top["merge"], false);
        let files = top["files"].as_array().unwrap();
        let find = |p: &str| files.iter().find(|f| f["path"] == p).unwrap().clone();
        assert_eq!(find("renamed.txt")["status"], "R");
        assert_eq!(find("renamed.txt")["from"], "a.txt");
        assert_eq!(find("image.bin")["status"], "A");
        assert_eq!(find("image.bin")["binary"], true);
        // The rename's diff names both paths; the binary one is summarised.
        let rename = diff(
            &checkout,
            &DiffKind::Commit("HEAD".into()),
            "renamed.txt",
            Some("a.txt"),
        )
        .unwrap();
        assert_eq!(rename["files"].as_array().unwrap().len(), 1, "{rename}");
        assert_eq!(rename["files"][0]["status"], "R");
        assert_eq!(rename["files"][0]["from"], "a.txt");
        let binary = diff(
            &checkout,
            &DiffKind::Commit("HEAD".into()),
            "image.bin",
            None,
        )
        .unwrap();
        assert_eq!(binary["files"][0]["binary"], true);
        assert_eq!(binary["files"][0]["hunks"], json!([]));
        // The merge: against its first parent.
        let merge_sha = log(&checkout, "HEAD", 1, 1, false, None).unwrap()["commits"][0]["sha"]
            .as_str()
            .unwrap()
            .to_owned();
        let merge = commit(&checkout, &merge_sha).unwrap();
        assert_eq!(merge["merge"], true);
        assert_eq!(merge["parents"].as_array().unwrap().len(), 2);
        let files = merge["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["path"], "feature.txt");
        // A root commit, and its diff (against the empty tree).
        let root_sha = log(&checkout, "HEAD", 5, 1, false, None).unwrap()["commits"][0]["sha"]
            .as_str()
            .unwrap()
            .to_owned();
        let root = commit(&checkout, &root_sha).unwrap();
        assert_eq!(root["parents"], json!([]));
        assert_eq!(root["files"][0]["added"], 3);
        let first = diff(&checkout, &DiffKind::Commit(root_sha), "a.txt", None).unwrap();
        let hunk = &first["files"][0]["hunks"][0];
        assert_eq!(hunk["new_start"], 1);
        assert_eq!(hunk["lines"], json!(["+one", "+two", "+three"]));
        // Branches: current first, the other with its last commit.
        let listed = branches(&checkout).unwrap();
        assert_eq!(listed["current"], "main");
        assert_eq!(listed["detached"], false);
        assert_eq!(listed["branches"][0]["name"], "main");
        assert_eq!(listed["branches"][0]["current"], true);
        assert_eq!(listed["branches"][1]["name"], "feat");
        assert_eq!(listed["branches"][1]["subject"], "Feature");
        assert_eq!(listed["remotes"], json!([]));
    }

    #[test]
    fn branches_report_upstreams_as_of_the_last_fetch() {
        let temp = Temp::new();
        let upstream = temp.0.join("upstream");
        fs::create_dir_all(&upstream).unwrap();
        git(&upstream, &["init", "-q", "-b", "main"]);
        fs::write(upstream.join("a"), "1\n").unwrap();
        commit_all(&upstream, "Up");
        git(&temp.0, &["clone", "-q", "upstream", "work"]);
        let work = temp.0.join("work");
        fs::write(work.join("a"), "2\n").unwrap();
        commit_all(&work, "Local");
        git(&work, &["checkout", "-q", "--detach"]);
        let listed = branches(&open(&work)).unwrap();
        assert_eq!(listed["detached"], true);
        assert_eq!(listed["current"], Value::Null);
        let main = &listed["branches"][0];
        assert_eq!(main["upstream"], "origin/main");
        assert_eq!(
            (main["ahead"].clone(), main["behind"].clone()),
            (json!(1), json!(0))
        );
        assert_eq!(listed["remotes"][0]["name"], "origin/main");
        assert_eq!(
            listed["remotes"].as_array().unwrap().len(),
            1,
            "no origin/HEAD"
        );
    }

    #[test]
    fn working_changes_staged_unstaged_untracked_and_their_diffs() {
        let temp = Temp::new();
        let path = temp.0.join("work");
        fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        fs::write(path.join("both.txt"), "1\n2\n3\n").unwrap();
        fs::write(path.join("old.txt"), "keep\nthis\ncontent\n").unwrap();
        commit_all(&path, "Base");
        fs::write(path.join("both.txt"), "1\nstaged\n3\n").unwrap();
        git(&path, &["add", "both.txt"]);
        fs::write(path.join("both.txt"), "1\nstaged\n3\nunstaged\n").unwrap();
        git(&path, &["mv", "old.txt", "moved.txt"]);
        fs::write(path.join("new.txt"), "hello\nworld").unwrap();
        fs::write(path.join("blob.bin"), [0u8, 9, 0]).unwrap();
        fs::write(path.join("huge.txt"), "x\n".repeat(200_000)).unwrap();
        let checkout = open(&path);
        let listed = changes(&checkout).unwrap();
        let staged = listed["staged"].as_array().unwrap();
        let s = |p: &str| staged.iter().find(|f| f["path"] == p).unwrap().clone();
        assert_eq!(s("both.txt")["status"], "M");
        assert_eq!(
            (
                s("both.txt")["added"].clone(),
                s("both.txt")["removed"].clone()
            ),
            (json!(1), json!(1))
        );
        assert_eq!(s("moved.txt")["status"], "R");
        assert_eq!(s("moved.txt")["from"], "old.txt");
        let unstaged = listed["unstaged"].as_array().unwrap();
        assert_eq!(unstaged.len(), 1);
        assert_eq!(unstaged[0]["path"], "both.txt");
        assert_eq!(unstaged[0]["added"], 1);
        let untracked = listed["untracked"].as_array().unwrap();
        let u = |p: &str| untracked.iter().find(|f| f["path"] == p).unwrap().clone();
        assert_eq!(u("new.txt")["added"], 2);
        assert_eq!(u("blob.bin")["binary"], true);
        assert_eq!(u("huge.txt")["size"], 400_000);
        // Diffs per side.
        let staged = diff(&checkout, &DiffKind::Staged, "both.txt", None).unwrap();
        assert_eq!(
            staged["files"][0]["hunks"][0]["lines"],
            json!([" 1", "-2", "+staged", " 3"])
        );
        let unstaged = diff(&checkout, &DiffKind::Unstaged, "both.txt", None).unwrap();
        assert!(
            unstaged["files"][0]["hunks"][0]["lines"]
                .as_array()
                .unwrap()
                .contains(&json!("+unstaged"))
        );
        let moved = diff(&checkout, &DiffKind::Staged, "moved.txt", Some("old.txt")).unwrap();
        assert_eq!(moved["files"][0]["status"], "R");
        let new = diff(&checkout, &DiffKind::Untracked, "new.txt", None).unwrap();
        assert_eq!(
            new["files"][0]["hunks"][0]["lines"],
            json!(["+hello", "+world", "\\ No newline at end of file"])
        );
        let blob = diff(&checkout, &DiffKind::Untracked, "blob.bin", None).unwrap();
        assert_eq!(blob["files"][0]["binary"], true);
        let huge = diff(&checkout, &DiffKind::Untracked, "huge.txt", None).unwrap();
        assert_eq!(huge["files"][0]["too_large"], 400_000);
        // A tracked file is not untracked; a symbolic link is not followed.
        assert!(diff(&checkout, &DiffKind::Untracked, "both.txt", None).is_err());
        std::os::unix::fs::symlink("/etc/passwd", path.join("link")).unwrap();
        assert!(diff(&checkout, &DiffKind::Untracked, "link", None).is_err());
        // Before the first commit, staged diffs compare with the empty tree.
        let fresh = temp.0.join("fresh");
        fs::create_dir_all(&fresh).unwrap();
        git(&fresh, &["init", "-q", "-b", "main"]);
        fs::write(fresh.join("f"), "x\n").unwrap();
        git(&fresh, &["add", "f"]);
        let checkout = open(&fresh);
        assert_eq!(changes(&checkout).unwrap()["staged"][0]["added"], 1);
        let first = diff(&checkout, &DiffKind::Staged, "f", None).unwrap();
        assert_eq!(first["files"][0]["hunks"][0]["lines"], json!(["+x"]));
    }

    #[test]
    fn huge_diffs_are_truncated_and_say_so() {
        let temp = Temp::new();
        let path = temp.0.join("big");
        fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        fs::write(path.join("big.txt"), "").unwrap();
        commit_all(&path, "Empty");
        let line = "x".repeat(99);
        fs::write(path.join("big.txt"), format!("{line}\n").repeat(30_000)).unwrap();
        commit_all(&path, "Big");
        let checkout = open(&path);
        let value = diff(&checkout, &DiffKind::Commit("HEAD".into()), "big.txt", None).unwrap();
        assert_eq!(value["truncated"], true);
        let file = &value["files"][0];
        assert_eq!(file["truncated"], true);
        let kept: usize = file["hunks"][0]["lines"].as_array().unwrap().len();
        assert!(kept > 1000 && kept * 101 <= FILE_BYTES + 101, "{kept}");
        assert!(value.to_string().len() < RESPONSE_BYTES + 64 * 1024);
    }

    /// Programs a repository's configuration names never run for history
    /// and diffs, and nothing is written to the repository.
    #[test]
    fn history_runs_no_configured_program_and_writes_nothing() {
        let temp = Temp::new();
        let path = temp.0.join("evil");
        fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        fs::write(path.join(".gitattributes"), "* diff=evil filter=evil\n").unwrap();
        fs::write(path.join("data.txt"), "one\n").unwrap();
        commit_all(&path, "c1");
        fs::write(path.join("data.txt"), "two\n").unwrap();
        commit_all(&path, "c2");
        let marker = temp.0.join("ran");
        let program = format!("sh -c 'echo $0 >> {}' ", marker.display());
        let mut config = fs::read_to_string(path.join(".git/config")).unwrap();
        config.push_str(&format!(
            "[core]\n\tfsmonitor = {program}fsmonitor\n\tpager = {program}pager\n\
             [filter \"evil\"]\n\tclean = {program}clean\n\tsmudge = {program}smudge\n\tprocess = {program}process\n\
             [diff \"evil\"]\n\ttextconv = {program}textconv\n\tcommand = {program}diff-command\n\
             [diff]\n\texternal = {program}external\n\torderFile = /dev/stdin\n\
             [log]\n\tshowSignature = true\n\tfollow = true\n[gpg]\n\tprogram = {program}gpg\n"
        ));
        fs::write(path.join(".git/config"), config).unwrap();
        std::thread::sleep(Duration::from_millis(1100));
        fs::write(path.join("data.txt"), "three\n").unwrap();
        fs::write(path.join("data.txt"), "four\n").unwrap();
        fs::write(path.join("untracked.txt"), "u\n").unwrap();
        let index = fs::read(path.join(".git/index")).unwrap();
        let checkout = open(&path);
        log(&checkout, "HEAD", 0, 10, false, Some("data.txt")).unwrap();
        commit(&checkout, "HEAD").unwrap();
        branches(&checkout).unwrap();
        changes(&checkout).unwrap();
        let shown = diff(
            &checkout,
            &DiffKind::Commit("HEAD".into()),
            "data.txt",
            None,
        )
        .unwrap();
        assert_eq!(
            shown["files"][0]["hunks"][0]["lines"],
            json!(["-one", "+two"])
        );
        diff(&checkout, &DiffKind::Unstaged, "data.txt", None).unwrap();
        diff(&checkout, &DiffKind::Staged, "data.txt", None).unwrap();
        diff(&checkout, &DiffKind::Untracked, "untracked.txt", None).unwrap();
        assert!(
            !marker.exists(),
            "a configured program ran: {}",
            fs::read_to_string(&marker).unwrap_or_default()
        );
        assert_eq!(fs::read(path.join(".git/index")).unwrap(), index);
        assert!(!path.join(".git/index.lock").exists());
    }

    /// A repository's `core.bigFileThreshold` cannot make a diff load a
    /// huge blob (it stays at 8 MiB: above it a file is binary), and its
    /// `i18n.logOutputEncoding` cannot change what the parsers read.
    #[test]
    fn repository_settings_cannot_raise_limits_or_change_encodings() {
        let temp = Temp::new();
        let path = temp.0.join("big");
        fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        fs::write(path.join("small.txt"), "one\n").unwrap();
        commit_all(&path, "Première");
        // 12 MiB of text lines: a text diff below a raised threshold.
        fs::write(path.join("big.txt"), "a\n".repeat(6 << 20)).unwrap();
        fs::write(path.join("small.txt"), "two\n").unwrap();
        commit_all(&path, "Grosse modification");
        let mut config = fs::read_to_string(path.join(".git/config")).unwrap();
        config.push_str("[core]\n\tbigFileThreshold = 8g\n[i18n]\n\tlogOutputEncoding = UTF-16\n");
        fs::write(path.join(".git/config"), config).unwrap();
        let checkout = open(&path);
        let shown = commit(&checkout, "HEAD").unwrap();
        assert_eq!(shown["message"], "Grosse modification", "{shown}");
        let big = shown["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == "big.txt")
            .unwrap();
        assert_eq!(big["binary"], true, "{big}");
        let diffed = diff(&checkout, &DiffKind::Commit("HEAD".into()), "big.txt", None).unwrap();
        assert_eq!(diffed["files"][0]["binary"], true, "{diffed}");
        assert_eq!(diffed["files"][0]["hunks"], json!([]));
        let logged = log(&checkout, "HEAD", 0, 10, false, None).unwrap();
        assert_eq!(subjects(&logged), ["Grosse modification", "Première"]);
        assert_eq!(
            branches(&checkout).unwrap()["branches"][0]["subject"],
            "Grosse modification"
        );
    }

    /// The checkout must be one discovery finds in the project's folder.
    #[test]
    fn only_discovered_checkouts_are_opened() {
        let temp = Temp::new();
        let project = temp.0.join("project");
        let inside = project.join("api");
        fs::create_dir_all(&inside).unwrap();
        git(&inside, &["init", "-q", "-b", "main"]);
        fs::write(inside.join("f"), "x\n").unwrap();
        commit_all(&inside, "c");
        let elsewhere = temp.0.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        git(&elsewhere, &["init", "-q", "-b", "main"]);
        let cache = web_git::Cache::default();
        let ok = task(
            project.clone(),
            inside.to_string_lossy().into(),
            Query::Branches,
        )(&cache)
        .unwrap();
        assert_eq!(ok["current"], "main");
        assert_eq!(ok["repo"], inside.to_string_lossy().as_ref());
        for repo in [
            elsewhere.to_string_lossy().into_owned(),
            format!("{}/../elsewhere", project.display()),
            "api".into(),
            "/".into(),
        ] {
            assert_eq!(
                task(project.clone(), repo.clone(), Query::Branches)(&cache).unwrap_err(),
                UNKNOWN_REPOSITORY,
                "{repo}"
            );
        }
    }
}
