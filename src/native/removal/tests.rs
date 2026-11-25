// The safe-removal check against real repositories made with the test's own
// Git (a bare "remote" and clones), and the removal guards on disposable
// folders. No network: the remote is a local bare repository.
use super::*;
use std::{
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kelpmux-removal-{}-{}",
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

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new(web_git::git_binary().expect("git is needed for these tests"))
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// A project folder with one checkout, `repo`, cloned from a bare remote
/// and pushed: nothing at risk.
fn project(temp: &Temp) -> (PathBuf, PathBuf) {
    let remote = temp.0.join("remote.git");
    fs::create_dir(&remote).unwrap();
    git(&remote, &["init", "--bare", "-q", "-b", "main"]);
    let folder = temp.0.join("root/projects/p");
    fs::create_dir_all(&folder).unwrap();
    git(&folder, &["clone", "-q", remote.to_str().unwrap(), "repo"]);
    let repo = folder.join("repo");
    git(&repo, &["checkout", "-q", "-b", "main"]);
    fs::write(repo.join("README"), "hello\n").unwrap();
    fs::write(repo.join(".gitignore"), "build/\n.env\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "first"]);
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    (folder, repo)
}

fn guards(temp: &Temp) -> Guards {
    let root = temp.0.join("root");
    Guards {
        roots: vec![root.clone()],
        protected: vec![
            root.clone(),
            root.join("projects"),
            root.join("scratch"),
            temp.0.join("home"),
        ],
    }
}

fn kinds(check: &Check) -> Vec<(Severity, &'static str)> {
    check.items.iter().map(|i| (i.severity, i.kind)).collect()
}

#[test]
fn a_pushed_clean_checkout_holds_nothing_at_risk() {
    let temp = Temp::new();
    let (folder, _) = project(&temp);
    let check = check(&folder, Mode::Delete, &guards(&temp));
    assert!(check.exists);
    assert_eq!(check.checkouts.len(), 1);
    assert_eq!(check.checkouts[0].error, None, "{check:?}");
    assert!(check.items.is_empty(), "{:?}", check.items);
    assert!(check.allowed());
    let json = check.to_json("p");
    assert_eq!(json["allowed"], true);
    assert_eq!(json["fingerprint"].as_str().unwrap().len(), 16);
}

#[test]
fn every_git_case_is_found_and_blocks_a_delete_but_not_an_archive() {
    let temp = Temp::new();
    let (folder, repo) = project(&temp);
    // Uncommitted, untracked, ignored.
    fs::write(repo.join("README"), "changed\n").unwrap();
    fs::write(repo.join("notes.txt"), "new\n").unwrap();
    fs::write(repo.join(".env"), "SECRET=1\n").unwrap();
    let check_delete = check(&folder, Mode::Delete, &guards(&temp));
    let found = kinds(&check_delete);
    assert!(
        found.contains(&(Severity::Blocker, "uncommitted")),
        "{found:?}"
    );
    assert!(
        found.contains(&(Severity::Blocker, "untracked")),
        "{found:?}"
    );
    assert!(found.contains(&(Severity::Warning, "ignored")), "{found:?}");
    assert!(!check_delete.allowed());
    let check_archive = check(&folder, Mode::Archive, &guards(&temp));
    assert!(check_archive.allowed(), "{:?}", check_archive.items);
    assert!(check_archive.items.iter().any(|i| i.kind == "uncommitted"
        && i.severity == Severity::Warning
        && i.message.contains("on no remote")));
    // A stash.
    git(&repo, &["stash", "-q", "-u"]);
    let found = check(&folder, Mode::Delete, &guards(&temp));
    assert!(
        kinds(&found).contains(&(Severity::Blocker, "stash")),
        "{:?}",
        found.items
    );
    assert_eq!(found.checkouts[0].stashes, 1);
    assert_eq!(found.checkouts[0].uncommitted, 0);
    git(&repo, &["stash", "drop", "-q"]);
    fs::remove_file(repo.join(".env")).unwrap();
    // An unpushed commit on the upstream branch.
    fs::write(repo.join("more"), "x\n").unwrap();
    git(&repo, &["add", "more"]);
    git(&repo, &["commit", "-q", "-m", "local"]);
    let found = check(&folder, Mode::Delete, &guards(&temp));
    assert_eq!(
        kinds(&found),
        vec![(Severity::Blocker, "unpushed")],
        "{:?}",
        found.items
    );
    assert!(found.items[0].message.contains("branch main: 1 commit"));
    git(&repo, &["push", "-q"]);
    // A branch without upstream whose commits are pushed: a warning only.
    git(&repo, &["branch", "side"]);
    let found = check(&folder, Mode::Delete, &guards(&temp));
    assert_eq!(
        kinds(&found),
        vec![(Severity::Warning, "no_upstream")],
        "{:?}",
        found.items
    );
    assert!(found.allowed());
    // A local branch with its own commit.
    git(&repo, &["checkout", "-q", "side"]);
    fs::write(repo.join("side"), "x\n").unwrap();
    git(&repo, &["add", "side"]);
    git(&repo, &["commit", "-q", "-m", "side"]);
    git(&repo, &["checkout", "-q", "main"]);
    let found = check(&folder, Mode::Delete, &guards(&temp));
    assert_eq!(
        kinds(&found),
        vec![(Severity::Blocker, "unpushed")],
        "{:?}",
        found.items
    );
    assert_eq!(found.checkouts[0].branches[0].name, "side");
    assert_eq!(found.checkouts[0].branches[0].upstream, None);
    git(&repo, &["branch", "-q", "-D", "side"]);
    // A detached HEAD with a commit on no branch.
    git(&repo, &["checkout", "-q", "--detach"]);
    fs::write(repo.join("detached"), "x\n").unwrap();
    git(&repo, &["add", "detached"]);
    git(&repo, &["commit", "-q", "-m", "detached"]);
    let found = check(&folder, Mode::Delete, &guards(&temp));
    assert_eq!(
        kinds(&found),
        vec![(Severity::Blocker, "detached")],
        "{:?}",
        found.items
    );
    assert!(found.checkouts[0].detached);
}

#[test]
fn files_outside_git_nested_repositories_and_changes_are_reported() {
    let temp = Temp::new();
    let (folder, _) = project(&temp);
    fs::write(folder.join("notes.md"), "12345").unwrap();
    fs::create_dir(folder.join("docs")).unwrap();
    fs::write(folder.join("docs/a.txt"), "abc").unwrap();
    std::os::unix::fs::symlink("/etc", folder.join("link")).unwrap();
    let first = check(&folder, Mode::Delete, &guards(&temp));
    assert_eq!(first.other_files.files, 3, "{first:?}");
    assert_eq!(first.other_files.bytes, 8);
    assert_eq!(kinds(&first), vec![(Severity::Warning, "other_files")]);
    assert!(first.allowed());
    let archived = check(&folder, Mode::Archive, &guards(&temp));
    assert_eq!(kinds(&archived), vec![(Severity::Info, "other_files")]);
    // The same tree gives the same fingerprint; a byte more does not.
    assert_eq!(
        check(&folder, Mode::Delete, &guards(&temp)).fingerprint(),
        first.fingerprint()
    );
    fs::write(folder.join("notes.md"), "123456").unwrap();
    assert_ne!(
        check(&folder, Mode::Delete, &guards(&temp)).fingerprint(),
        first.fingerprint()
    );
    // A hidden repository discovery does not inspect is a blocker.
    let hidden = folder.join(".hidden");
    fs::create_dir(&hidden).unwrap();
    git(&hidden, &["init", "-q"]);
    let found = check(&folder, Mode::Delete, &guards(&temp));
    assert!(
        kinds(&found).contains(&(Severity::Blocker, "nested_repository")),
        "{:?}",
        found.items
    );
    assert!(found.items.iter().any(|i| i.path.ends_with(".hidden")));
    let found = check(&folder, Mode::Archive, &guards(&temp));
    assert!(kinds(&found).contains(&(Severity::Warning, "nested_repository")));
    // A missing folder has nothing to remove.
    let missing = check(&folder.join("nope"), Mode::Delete, &guards(&temp));
    assert!(!missing.exists);
    assert_eq!(kinds(&missing), vec![(Severity::Info, "missing")]);
}

#[test]
fn removal_guards_refuse_roots_protected_folders_links_and_outsiders() {
    let temp = Temp::new();
    let guards = guards(&temp);
    let root = temp.0.join("root");
    for path in [
        root.join("projects"),
        root.join("scratch/s"),
        temp.0.join("home"),
    ] {
        fs::create_dir_all(&path).unwrap();
    }
    // The root itself, the projects base, a folder containing the Scratch
    // folder, a folder outside the roots.
    for (folder, expected) in [
        (root.clone(), "not inside the file roots"),
        (root.join("projects"), "never removes"),
        (temp.0.join("home"), "not inside the file roots"),
    ] {
        let error = guard(&folder, &guards).unwrap_err();
        assert!(error.contains(expected), "{folder:?}: {error}");
    }
    let mut more = guards.clone();
    more.protected.push(root.join("projects/p/inner"));
    fs::create_dir_all(root.join("projects/p/inner")).unwrap();
    assert!(
        guard(&root.join("projects/p"), &more)
            .unwrap_err()
            .contains("never removes")
    );
    // A link to a folder is refused, even when its target is removable.
    std::os::unix::fs::symlink(root.join("projects/p"), root.join("projects/link")).unwrap();
    assert!(
        guard(&root.join("projects/link"), &guards)
            .unwrap_err()
            .contains("symbolic link")
    );
    // A file is not a folder.
    fs::write(root.join("projects/file"), "x").unwrap();
    assert!(
        guard(&root.join("projects/file"), &guards)
            .unwrap_err()
            .contains("not a folder")
    );
    let check = check(&root.join("projects"), Mode::Delete, &guards);
    assert_eq!(kinds(&check), vec![(Severity::Blocker, "guard")]);
}

#[test]
fn removal_renames_first_and_never_follows_links() {
    let temp = Temp::new();
    let guards = guards(&temp);
    let projects = temp.0.join("root/projects");
    let outside = temp.0.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("keep"), "precious").unwrap();
    let folder = projects.join("p");
    fs::create_dir_all(folder.join("a/b")).unwrap();
    fs::write(folder.join("a/b/file"), "x").unwrap();
    std::os::unix::fs::symlink(&outside, folder.join("a/to-outside")).unwrap();
    std::os::unix::fs::symlink(outside.join("keep"), folder.join("keep-link")).unwrap();
    // A read-only subfolder still goes.
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(folder.join("a/b"), fs::Permissions::from_mode(0o500)).unwrap();
    let removed = remove(&folder, &guards);
    if removed.is_err() {
        fs::set_permissions(folder.join("a/b"), fs::Permissions::from_mode(0o700)).ok();
    }
    removed.unwrap();
    assert!(fs::symlink_metadata(&folder).is_err());
    assert_eq!(
        fs::read_to_string(outside.join("keep")).unwrap(),
        "precious"
    );
    // Nothing is left beside it.
    let left: Vec<_> = fs::read_dir(&projects)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert!(left.is_empty(), "{left:?}");
    // A guarded folder is never renamed.
    assert!(remove(&projects, &guards).is_err());
    assert!(projects.is_dir());
}

#[test]
fn mount_points_are_read_with_their_escapes() {
    // `/proc/self/mountinfo` always lists `/`.
    assert!(mount_points().iter().any(|m| m == Path::new("/")));
    assert_eq!(size_text(999), "999 B");
    assert_eq!(size_text(1_500_000), "1.5 MB");
}

/// A bare remote with one pushed commit, and its path.
fn remote(temp: &Temp, name: &str) -> PathBuf {
    let bare = temp.0.join(format!("{name}.git"));
    fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "--bare", "-q", "-b", "main"]);
    let seed = temp.0.join(format!("{name}-seed"));
    fs::create_dir(&seed).unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    fs::write(seed.join("file"), name).unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-q", "-m", "seed"]);
    git(&seed, &["push", "-q", bare.to_str().unwrap(), "main"]);
    bare
}

fn commit(repo: &Path, file: &str) {
    fs::write(repo.join(file), file).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-q", "-m", file]);
}

#[test]
fn a_submodule_with_a_local_commit_blocks_a_delete() {
    let temp = Temp::new();
    let (folder, repo) = project(&temp);
    let library = remote(&temp, "library");
    git(
        &repo,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            library.to_str().unwrap(),
            "lib",
        ],
    );
    git(&repo, &["commit", "-q", "-m", "submodule"]);
    git(&repo, &["push", "-q"]);
    let clean = check(&folder, Mode::Delete, &guards(&temp));
    assert!(clean.allowed(), "{:?}", clean.items);
    assert!(
        clean
            .checkouts
            .iter()
            .any(|c| c.kind == "submodule" && c.error.is_none()),
        "{:?}",
        clean.checkouts
    );
    // A commit in the submodule that no remote has.
    let lib = repo.join("lib");
    commit(&lib, "local");
    let found = check(&folder, Mode::Delete, &guards(&temp));
    assert!(!found.allowed(), "{:?}", found.items);
    assert!(
        found.items.iter().any(|i| i.severity == Severity::Blocker
            && i.path.ends_with("/lib")
            && (i.kind == "unpushed" || i.kind == "detached")),
        "{:?}",
        found.items
    );
    // Not checked out any more: its repository in .git/modules is reported.
    git(&repo, &["submodule", "deinit", "-q", "-f", "lib"]);
    let found = check(&folder, Mode::Delete, &guards(&temp));
    assert!(
        found.items.iter().any(|i| i.kind == "unchecked"
            && i.severity == Severity::Blocker
            && i.path.contains("modules")),
        "{:?}",
        found.items
    );
}

#[test]
fn a_repository_inside_an_ignored_folder_is_checked() {
    let temp = Temp::new();
    let (folder, repo) = project(&temp);
    fs::write(repo.join(".gitignore"), "build/\n.env\nvendor/\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "ignore vendor"]);
    git(&repo, &["push", "-q"]);
    let library = remote(&temp, "vendored");
    let vendor = repo.join("vendor");
    fs::create_dir(&vendor).unwrap();
    git(&vendor, &["clone", "-q", library.to_str().unwrap(), "dep"]);
    let dep = vendor.join("dep");
    let pushed = check(&folder, Mode::Delete, &guards(&temp));
    assert!(pushed.allowed(), "{:?}", pushed.items);
    assert!(
        pushed
            .checkouts
            .iter()
            .any(|c| c.kind == "nested" && c.path == dep)
    );
    commit(&dep, "local");
    let found = check(&folder, Mode::Delete, &guards(&temp));
    assert!(!found.allowed());
    assert!(
        found
            .items
            .iter()
            .any(|i| i.kind == "unpushed" && i.path == dep.to_string_lossy()),
        "{:?}",
        found.items
    );
    // An archive keeps it: a warning.
    let archived = check(&folder, Mode::Archive, &guards(&temp));
    assert!(archived.allowed(), "{:?}", archived.items);
}

#[test]
fn commits_only_on_a_tag_and_outside_worktrees_are_found() {
    let temp = Temp::new();
    let (folder, repo) = project(&temp);
    // A commit reachable from a tag only.
    git(&repo, &["checkout", "-q", "-b", "tmp"]);
    commit(&repo, "tagged");
    git(&repo, &["tag", "keep"]);
    git(&repo, &["checkout", "-q", "main"]);
    git(&repo, &["branch", "-q", "-D", "tmp"]);
    let found = check(&folder, Mode::Delete, &guards(&temp));
    assert_eq!(
        kinds(&found),
        vec![(Severity::Blocker, "other_refs")],
        "{:?}",
        found.items
    );
    assert_eq!(found.checkouts[0].other_refs_unpushed, 1);
    git(&repo, &["tag", "-d", "keep"]);
    // A linked worktree outside the folder.
    let outside = temp.0.join("elsewhere");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            outside.to_str().unwrap(),
        ],
    );
    let found = check(&folder, Mode::Delete, &guards(&temp));
    assert_eq!(
        kinds(&found),
        vec![(Severity::Blocker, "worktree")],
        "{:?}",
        found.items
    );
    assert_eq!(found.checkouts[0].outside_worktrees, vec![outside.clone()]);
    let archived = check(&folder, Mode::Archive, &guards(&temp));
    assert_eq!(kinds(&archived), vec![(Severity::Warning, "worktree")]);
    git(
        &repo,
        &["worktree", "remove", "-f", outside.to_str().unwrap()],
    );
    // A linked worktree inside the folder is inspected.
    let inside = folder.join("second");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "second",
            inside.to_str().unwrap(),
        ],
    );
    commit(&inside, "work");
    let found = check(&folder, Mode::Delete, &guards(&temp));
    assert!(
        found.checkouts.iter().any(|c| c.path == inside),
        "{:?}",
        found.checkouts
    );
    assert!(
        found
            .items
            .iter()
            .any(|i| i.kind == "unpushed" && i.message.contains("second")),
        "{:?}",
        found.items
    );
}
