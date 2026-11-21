//! Read-only facts the browser's Terminals table shows next to each pane:
//! the Git repository a working directory belongs to (its root, branch and
//! GitHub `owner/repo`) and a pane's foreground program.
//!
//! Everything here reads a few small files. It never runs `git` or any other
//! process, never writes, follows no symlinked Git metadata, opens files
//! without blocking (a FIFO named `HEAD` cannot stall the bridge) and bounds
//! every input: paths per call, ancestors walked and bytes read per file.
//! Only absolute directories owned by the bridge's own user are examined,
//! and Git metadata owned by anyone else is ignored. Results are cached for
//! one call only, so a later call always sees the current branch.
use serde::Serialize;
use std::{
    collections::HashMap,
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
};

/// Most paths accepted by one `repos` call.
pub const MAX_PATHS: usize = 64;
/// Longest accepted path, in bytes.
const MAX_PATH_BYTES: usize = 4096;
/// Most directories examined from a path up towards `/`.
const MAX_DEPTH: usize = 32;
/// Largest `HEAD`, `.git` file or `commondir` read, in bytes.
const SMALL_FILE: u64 = 4096;
/// Largest Git config read, in bytes. A larger config is not parsed: the
/// repository is still reported, without a remote.
pub const CONFIG_LIMIT: u64 = 65536;

/// The repository a directory belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Repo {
    /// The working tree root: the directory holding `.git`.
    pub root: String,
    /// The checked-out branch, without `refs/heads/`; absent when detached.
    pub branch: Option<String>,
    /// The first twelve hex digits of a detached HEAD's commit.
    pub detached: Option<String>,
    /// `owner/repo` of the preferred remote when it is on GitHub.
    pub github: Option<String>,
}

/// Looks up each path's repository, in order; `None` for a path that is
/// invalid, not the user's or in no repository. Errors only on more than
/// [`MAX_PATHS`] paths.
pub fn lookup(paths: &[String]) -> Result<Vec<Option<Repo>>, String> {
    if paths.len() > MAX_PATHS {
        return Err(format!("at most {MAX_PATHS} paths per call"));
    }
    let uid = unsafe { libc::geteuid() };
    let mut cache = Cache::default();
    Ok(paths.iter().map(|p| cache.repo(p, uid)).collect())
}

/// One call's cache: each examined directory's answer, and each Git
/// directory's parsed metadata, so panes in one repository read it once.
#[derive(Default)]
struct Cache {
    directories: HashMap<PathBuf, Option<Repo>>,
}

impl Cache {
    fn repo(&mut self, path: &str, uid: u32) -> Option<Repo> {
        let path = valid_path(path)?;
        let metadata = fs::metadata(&path).ok()?;
        if !metadata.is_dir() || metadata.uid() != uid {
            return None;
        }
        let mut walked = Vec::new();
        let mut found = None;
        let mut current = Some(path.as_path());
        for _ in 0..MAX_DEPTH {
            let Some(directory) = current else { break };
            if let Some(cached) = self.directories.get(directory) {
                found = cached.clone();
                break;
            }
            walked.push(directory.to_path_buf());
            if let Some(git) = git_dir(directory, uid) {
                found = read_repo(directory, &git, uid);
                break;
            }
            current = directory.parent();
        }
        for directory in walked {
            self.directories.insert(directory, found.clone());
        }
        found
    }
}

/// An absolute, normalized path without `..`, NUL or excessive length.
fn valid_path(path: &str) -> Option<PathBuf> {
    if path.is_empty() || path.len() > MAX_PATH_BYTES || path.contains('\0') {
        return None;
    }
    let path = Path::new(path);
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return None;
    }
    Some(path.components().collect())
}

/// The Git directory of a working tree root candidate: a real `.git`
/// directory, or the `gitdir:` a `.git` file names (worktrees, submodules).
/// A symlinked `.git` and metadata of another user are ignored.
fn git_dir(directory: &Path, uid: u32) -> Option<PathBuf> {
    let dot = directory.join(".git");
    let metadata = fs::symlink_metadata(&dot).ok()?;
    if metadata.uid() != uid {
        return None;
    }
    if metadata.is_dir() {
        return Some(dot);
    }
    if !metadata.file_type().is_file() {
        return None;
    }
    let text = read_small(&dot, SMALL_FILE, uid)?;
    let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
    if target.is_empty() || target.contains('\0') {
        return None;
    }
    let target = directory.join(target);
    real_dir(&target, uid).then_some(target)
}

/// Whether a path is a directory (not a symlink to one) owned by `uid`.
fn real_dir(path: &Path, uid: u32) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && m.uid() == uid)
}

fn read_repo(root: &Path, git: &Path, uid: u32) -> Option<Repo> {
    let head = read_small(&git.join("HEAD"), SMALL_FILE, uid)?;
    let head = head.lines().next().unwrap_or("").trim();
    let (branch, detached) = if let Some(reference) = head.strip_prefix("ref:") {
        let reference = reference.trim();
        let name = reference
            .strip_prefix("refs/heads/")
            .or_else(|| reference.strip_prefix("refs/"))
            .unwrap_or(reference);
        (printable(name, 256), None)
    } else if (head.len() == 40 || head.len() == 64) && head.bytes().all(|b| b.is_ascii_hexdigit())
    {
        (None, Some(head[..12].to_ascii_lowercase()))
    } else {
        return None;
    };
    let github = read_small(&common_dir(git, uid).join("config"), CONFIG_LIMIT, uid)
        .and_then(|config| preferred_remote(&remotes(&config)))
        .and_then(|url| github_repo(&url));
    Some(Repo {
        root: root.to_string_lossy().into_owned(),
        branch,
        detached,
        github,
    })
}

/// The directory holding the shared config: a linked worktree's `commondir`,
/// else the Git directory itself.
fn common_dir(git: &Path, uid: u32) -> PathBuf {
    read_small(&git.join("commondir"), SMALL_FILE, uid)
        .and_then(|text| {
            let target = text.lines().next()?.trim().to_owned();
            if target.is_empty() || target.contains('\0') {
                return None;
            }
            let common = git.join(target);
            real_dir(&common, uid).then_some(common)
        })
        .unwrap_or_else(|| git.to_path_buf())
}

/// Reads a small regular file owned by `uid`: never through a symlink,
/// never blocking on a FIFO or device, and never more than `limit` bytes.
fn read_small(path: &Path, limit: u64, uid: u32) -> Option<String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.uid() != uid || metadata.len() > limit {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > limit {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// `text` when it is non-empty, at most `limit` bytes and free of control
/// characters.
fn printable(text: &str, limit: usize) -> Option<String> {
    (!text.is_empty() && text.len() <= limit && !text.chars().any(char::is_control))
        .then(|| text.to_owned())
}

/// Each remote's first `url`, in config order. A minimal reader of Git's
/// config syntax: `[remote "name"]` sections, `url = value` keys (names
/// case-insensitive), comments and quoted values. Includes and `insteadOf`
/// rewrites are not followed.
pub fn remotes(config: &str) -> Vec<(String, String)> {
    let mut result: Vec<(String, String)> = Vec::new();
    let mut section: Option<String> = None;
    for line in config.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            section = header.split_once(']').and_then(|(inside, _)| {
                let (kind, name) = inside.trim().split_once(char::is_whitespace)?;
                if !kind.eq_ignore_ascii_case("remote") {
                    return None;
                }
                let name = name.trim().strip_prefix('"')?.strip_suffix('"')?;
                Some(name.replace("\\\"", "\"").replace("\\\\", "\\"))
            });
            continue;
        }
        let Some(name) = &section else { continue };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("url") || result.iter().any(|(n, _)| n == name) {
            continue;
        }
        result.push((name.clone(), config_value(value)));
    }
    result
}

/// A config value: quotes removed, an unquoted `#` or `;` starts a comment.
fn config_value(raw: &str) -> String {
    let mut value = String::new();
    let mut quoted = false;
    let mut chars = raw.trim().chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => quoted = !quoted,
            '\\' => {
                if let Some(next) = chars.next() {
                    value.push(match next {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                }
            }
            '#' | ';' if !quoted => break,
            _ => value.push(c),
        }
    }
    value.trim().to_owned()
}

/// The remote whose URL the table shows: `origin` when present, else the
/// first remote. Another remote on GitHub does not override `origin`.
pub fn preferred_remote(remotes: &[(String, String)]) -> Option<String> {
    remotes
        .iter()
        .find(|(name, _)| name == "origin")
        .or_else(|| remotes.first())
        .map(|(_, url)| url.clone())
}

/// `owner/repo` of a GitHub remote URL: `https://github.com/o/r(.git)`,
/// `ssh://git@github.com(:port)/o/r(.git)` or `git@github.com:o/r(.git)`.
/// Credentials in the URL are discarded, never returned. Anything else,
/// another host included, is `None`.
pub fn github_repo(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("ssh://"))
        .or_else(|| url.strip_prefix("git+ssh://"))
        .or_else(|| url.strip_prefix("ssh+git://"))
    {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let host = match host.split_once(':') {
            Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => host,
            Some(_) => return None,
            None => host,
        };
        github_host(host).then_some(path)?
    } else if !url.contains("://") {
        // scp-like: [user@]host:owner/repo
        let (authority, path) = url.split_once(':')?;
        if authority.contains('/') {
            return None;
        }
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        github_host(host).then_some(path)?
    } else {
        return None;
    };
    let path = rest.trim_start_matches('/').trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    let owner_ok = (1..=39).contains(&owner.len())
        && owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    let repo_ok = (1..=100).contains(&repo.len())
        && repo != "."
        && repo != ".."
        && repo
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    (owner_ok && repo_ok).then(|| format!("{owner}/{repo}"))
}

fn github_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("www.github.com")
}

/// The program in front of a pane: the leader of its terminal's foreground
/// process group when that is not the pane's own process (a shell at its
/// prompt, or a pane whose program runs directly), as the base name of its
/// first argument. Arguments are never returned: they can hold secrets.
pub fn foreground_program(proc_root: &Path, pid: u32) -> Option<String> {
    let process = crate::process::read(proc_root, pid).ok()?;
    if process.foreground <= 0 || process.foreground == process.group {
        return None;
    }
    let leader = process.foreground as u32;
    let first = crate::process::args(proc_root, leader).into_iter().next();
    let name = match first {
        Some(arg) => Path::new(&arg)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(arg),
        None => crate::process::read(proc_root, leader).ok()?.command,
    };
    let name: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "kelpmux-repos-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn write(&self, relative: &str, text: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            path
        }
        fn dir(&self, relative: &str) -> String {
            let path = self.0.join(relative);
            fs::create_dir_all(&path).unwrap();
            path.to_string_lossy().into_owned()
        }
        fn repo(&self, relative: &str, head: &str, config: &str) -> String {
            self.write(&format!("{relative}/.git/HEAD"), head);
            self.write(&format!("{relative}/.git/config"), config);
            self.dir(relative)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn one(path: &str) -> Option<Repo> {
        lookup(&[path.to_owned()]).unwrap().remove(0)
    }
    const ORIGIN_HTTPS: &str = "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = https://github.com/joanfabregat/kelpmux.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n";

    #[test]
    fn a_normal_repository_from_a_subdirectory() {
        let temp = Temp::new();
        let root = temp.repo("work", "ref: refs/heads/feat/table\n", ORIGIN_HTTPS);
        let nested = temp.dir("work/src/deep");
        let repo = one(&nested).unwrap();
        assert_eq!(repo.root, root);
        assert_eq!(repo.branch.as_deref(), Some("feat/table"));
        assert_eq!(repo.detached, None);
        assert_eq!(repo.github.as_deref(), Some("joanfabregat/kelpmux"));
        assert_eq!(one(&root), Some(repo));
    }

    #[test]
    fn a_linked_worktree_reads_its_head_and_the_shared_config() {
        let temp = Temp::new();
        temp.repo("main", "ref: refs/heads/main\n", ORIGIN_HTTPS);
        temp.write("main/.git/worktrees/wt/HEAD", "ref: refs/heads/feat/wt\n");
        temp.write("main/.git/worktrees/wt/commondir", "../..\n");
        let gitdir = temp.0.join("main/.git/worktrees/wt");
        temp.write("wt/.git", &format!("gitdir: {}\n", gitdir.display()));
        let tree = temp.dir("wt");
        let repo = one(&tree).unwrap();
        assert_eq!(repo.root, tree);
        assert_eq!(repo.branch.as_deref(), Some("feat/wt"));
        assert_eq!(repo.github.as_deref(), Some("joanfabregat/kelpmux"));
        // A relative gitdir (submodules) resolves from the `.git` file.
        temp.write("main/.git/modules/sub/HEAD", "ref: refs/heads/dev\n");
        temp.write("main/sub/.git", "gitdir: ../.git/modules/sub\n");
        let sub = one(&temp.dir("main/sub")).unwrap();
        assert_eq!(sub.branch.as_deref(), Some("dev"));
        assert_eq!(sub.github, None);
    }

    #[test]
    fn a_detached_head_reports_its_commit() {
        let temp = Temp::new();
        let root = temp.repo(
            "d",
            "0123456789ABCDEF0123456789abcdef01234567\n",
            ORIGIN_HTTPS,
        );
        let repo = one(&root).unwrap();
        assert_eq!(repo.branch, None);
        assert_eq!(repo.detached.as_deref(), Some("0123456789ab"));
        // Anything else in HEAD is not a repository this reads.
        temp.write("d/.git/HEAD", "garbage\n");
        assert_eq!(one(&root), None);
    }

    #[test]
    fn remotes_without_github_or_without_any_show_only_the_root() {
        let temp = Temp::new();
        let none = temp.repo("none", "ref: refs/heads/main\n", "[core]\n\tbare = false\n");
        let repo = one(&none).unwrap();
        assert_eq!((repo.root.as_str(), repo.github), (none.as_str(), None));
        let other = temp.repo(
            "other",
            "ref: refs/heads/main\n",
            "[remote \"origin\"]\n\turl = git@gitlab.com:group/project.git\n[remote \"gh\"]\n\turl = git@github.com:o/r.git\n",
        );
        // origin is preferred even when another remote is on GitHub.
        assert_eq!(one(&other).unwrap().github, None);
        let first = temp.repo(
            "first",
            "ref: refs/heads/main\n",
            "[remote \"upstream\"]\n\turl = git@github.com:o/first.git\n[remote \"fork\"]\n\turl = git@github.com:o/second.git\n",
        );
        assert_eq!(one(&first).unwrap().github.as_deref(), Some("o/first"));
    }

    #[test]
    fn github_url_forms() {
        for (url, expected) in [
            ("https://github.com/o/r.git", Some("o/r")),
            ("https://github.com/o/r", Some("o/r")),
            ("https://github.com/o/r/", Some("o/r")),
            ("https://user:token@github.com/o/r.git", Some("o/r")),
            ("https://www.GitHub.com/o-x/r.y_z", Some("o-x/r.y_z")),
            ("git@github.com:o/r.git", Some("o/r")),
            ("github.com:o/r", Some("o/r")),
            ("ssh://git@github.com/o/r.git", Some("o/r")),
            ("ssh://git@github.com:22/o/r", Some("o/r")),
            ("git+ssh://git@github.com/o/r", Some("o/r")),
            ("http://github.com/o/r", None),
            ("https://github.example.com/o/r", None),
            ("https://gitlab.com/o/r", None),
            ("git@gitlab.com:o/r.git", None),
            ("https://github.com/o", None),
            ("https://github.com/o/r/extra", None),
            ("https://github.com/o/..", None),
            ("/srv/git/r.git", None),
            ("../r.git", None),
            ("file:///srv/r", None),
        ] {
            assert_eq!(github_repo(url).as_deref(), expected, "{url}");
        }
    }

    #[test]
    fn config_reader_handles_case_comments_and_quotes() {
        let config = "# c\n[Remote \"origin\"]\n  URL = \"https://github.com/o/r.git\" ; note\n  url = https://github.com/o/ignored\n[branch \"main\"]\n\turl = nope\n";
        assert_eq!(
            remotes(config),
            [("origin".into(), "https://github.com/o/r.git".into())]
        );
    }

    #[test]
    fn paths_outside_repositories_and_invalid_paths_are_null() {
        let temp = Temp::new();
        let plain = temp.dir("plain/child");
        // The temp root lies in no repository of this user.
        assert_eq!(one(&plain), None);
        for path in [
            "",
            "relative/path",
            "/a/../b",
            "/a/./b",
            "/nonexistent/kelpmux",
        ] {
            assert_eq!(one(path), None, "{path:?}");
        }
        let file = temp.write("plain/file", "x");
        assert_eq!(one(&file.to_string_lossy()), None);
        let too_many = vec![plain.clone(); MAX_PATHS + 1];
        assert!(lookup(&too_many).is_err());
        assert_eq!(lookup(&vec![plain; MAX_PATHS]).unwrap().len(), MAX_PATHS);
    }

    #[test]
    fn an_oversize_config_keeps_the_repository_without_a_remote() {
        let temp = Temp::new();
        let mut config = String::from(ORIGIN_HTTPS);
        config.push_str(&format!("# {}\n", "x".repeat(CONFIG_LIMIT as usize)));
        let root = temp.repo("big", "ref: refs/heads/main\n", &config);
        let repo = one(&root).unwrap();
        assert_eq!(repo.branch.as_deref(), Some("main"));
        assert_eq!(repo.github, None);
    }

    #[test]
    fn symlinked_git_metadata_is_ignored() {
        let temp = Temp::new();
        let real = temp.repo("real", "ref: refs/heads/main\n", ORIGIN_HTTPS);
        temp.dir("linked");
        std::os::unix::fs::symlink(format!("{real}/.git"), temp.0.join("linked/.git")).unwrap();
        assert_eq!(one(&temp.0.join("linked").to_string_lossy()), None);
        // A symlinked HEAD is not read either.
        temp.dir("head/.git");
        temp.write("head/.git/config", ORIGIN_HTTPS);
        std::os::unix::fs::symlink(format!("{real}/.git/HEAD"), temp.0.join("head/.git/HEAD"))
            .unwrap();
        assert_eq!(one(&temp.0.join("head").to_string_lossy()), None);
    }

    #[test]
    fn the_cache_answers_repeated_and_sibling_paths_alike() {
        let temp = Temp::new();
        let root = temp.repo("c", "ref: refs/heads/main\n", ORIGIN_HTTPS);
        let a = temp.dir("c/a");
        let b = temp.dir("c/b");
        let found = lookup(&[a.clone(), b, a, root.clone()]).unwrap();
        assert!(found.iter().all(|r| r.as_ref().unwrap().root == root));
    }

    #[test]
    fn the_foreground_program_is_its_group_leader_name_only() {
        let temp = Temp::new();
        let stat = |pid: u32, comm: &str, group: i32, foreground: i32| {
            format!(
                "{pid} ({comm}) S 1 {group} {group} 34816 {foreground} 0 0 0 0 0 0 0 0 0 20 0 1 0 100 0 0\n"
            )
        };
        temp.write("proc/10/stat", &stat(10, "bash", 10, 10));
        assert_eq!(foreground_program(&temp.0.join("proc"), 10), None);
        temp.write("proc/11/stat", &stat(11, "bash", 11, 20));
        temp.write("proc/20/stat", &stat(20, "vim", 20, 20));
        temp.write("proc/20/cmdline", "/usr/bin/vim\0secret.txt\0");
        assert_eq!(
            foreground_program(&temp.0.join("proc"), 11).as_deref(),
            Some("vim")
        );
        // Without a readable command line, the kernel's name.
        temp.write("proc/12/stat", &stat(12, "bash", 12, 30));
        temp.write("proc/30/stat", &stat(30, "htop", 30, 30));
        assert_eq!(
            foreground_program(&temp.0.join("proc"), 12).as_deref(),
            Some("htop")
        );
        assert_eq!(foreground_program(&temp.0.join("proc"), 99), None);
    }
}
