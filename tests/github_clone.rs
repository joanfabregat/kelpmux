// Cloning through the hardened runner of web_github against a local
// repository served over HTTP (Git's "dumb" protocol), no network: the
// user's own Git configuration applies (here an extra header standing in
// for a credential helper) but its hooks never run, the remote stays the
// URL given, nothing is left behind on failure or cancel, and Git never
// prompts. Its credential helper never sees the API tokens of the
// bridge's environment.
use kelpmux::native::web_github::{CloneRequest, clone_into};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kelpmux-clone-{}-{}",
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

/// A HOME whose `.gitconfig` sends an extra header and sets a hooks folder
/// with a `post-checkout` hook that leaves a marker; set once, before any
/// test runs Git.
fn home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let home = std::env::temp_dir().join(format!("kelpmux-clone-home-{}", std::process::id()));
        let hooks = home.join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("post-checkout");
        fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", home.join("hook-ran").display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            home.join(".gitconfig"),
            format!(
                "[http]\n\textraHeader = X-KelpMux-Test: from-user-config\n[core]\n\thooksPath = {}\n[credential]\n\thelper = \"!f() {{ env > {}; }}; f\"\n",
                hooks.display(),
                home.join("helper-env").display()
            ),
        )
        .unwrap();
        // SAFETY: before any test thread starts a process (they all wait
        // on this `OnceLock`).
        unsafe {
            std::env::set_var("HOME", &home);
            // A token meant for the API, which Git must never see.
            std::env::set_var("GH_TOKEN", "sentinel-gh-token");
            std::env::set_var("GITHUB_TOKEN", "sentinel-github-token");
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        home
    })
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
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
    assert!(status.success(), "git {args:?}");
}

/// Serves `root` over HTTP; with `auth`, every request gets 401. Records
/// each request's `X-KelpMux-Test` header.
fn serve(root: PathBuf, auth: bool) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = seen.clone();
    thread::spawn(move || {
        for socket in listener.incoming() {
            let Ok(mut socket) = socket else { return };
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut head = vec![];
            while !head.ends_with(b"\r\n\r\n") {
                let mut b = [0; 1];
                if socket.read_exact(&mut b).is_err() {
                    break;
                }
                head.push(b[0]);
            }
            let head = String::from_utf8_lossy(&head).into_owned();
            let path = head.split(' ').nth(1).unwrap_or("/").to_owned();
            let header = head
                .lines()
                .find_map(|l| {
                    l.split_once(':')
                        .filter(|(n, _)| n.eq_ignore_ascii_case("x-kelpmux-test"))
                        .map(|(_, v)| v.trim().to_owned())
                })
                .unwrap_or_default();
            log.lock().unwrap().push(header);
            let file = root.join(path.split('?').next().unwrap().trim_start_matches('/'));
            let (status, body) = if auth {
                (
                    "401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"test\"",
                    vec![],
                )
            } else if file.starts_with(&root) && file.is_file() {
                (
                    "200 OK\r\nContent-Type: text/plain",
                    fs::read(&file).unwrap(),
                )
            } else {
                ("404 Not Found", vec![])
            };
            let _ = socket.write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            );
            let _ = socket.write_all(&body);
        }
    });
    (format!("http://{addr}"), seen)
}

/// A served bare repository `hello.git` with one commit.
fn upstream(temp: &Temp) -> PathBuf {
    let work = temp.0.join("work");
    fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    fs::write(work.join("README.md"), "hello\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "First"]);
    let served = temp.0.join("served");
    fs::create_dir_all(&served).unwrap();
    git(&served, &["clone", "-q", "--bare", "../work", "hello.git"]);
    git(&served.join("hello.git"), &["update-server-info"]);
    served
}

fn request<'a>(
    url: &'a str,
    folder: &'a Path,
    cancel: &'a AtomicBool,
    progress: &'a Mutex<String>,
) -> CloneRequest<'a> {
    CloneRequest {
        url,
        folder,
        schemes: &["http"],
        ssh_auth_sock: None,
        timeout: Duration::from_secs(60),
        cancel,
        progress,
    }
}

fn leftovers(folder: &Path) -> Vec<String> {
    fs::read_dir(folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".kelpmux-clone"))
        .collect()
}

#[test]
fn clones_with_the_users_git_configuration_but_never_its_hooks() {
    let home = home();
    let temp = Temp::new();
    let (base, seen) = serve(upstream(&temp), false);
    let project = temp.0.join("project");
    fs::create_dir_all(&project).unwrap();
    let url = format!("{base}/hello.git");
    let cancel = AtomicBool::new(false);
    let progress = Mutex::new(String::new());
    let path = clone_into(&request(&url, &project, &cancel, &progress)).unwrap();
    assert_eq!(path, project.join("hello"));
    assert_eq!(
        fs::read_to_string(path.join("README.md")).unwrap(),
        "hello\n"
    );
    // The remote is the URL given; nothing about credentials is saved.
    let config = fs::read_to_string(path.join(".git/config")).unwrap();
    assert!(config.contains(&format!("url = {url}")), "{config}");
    assert!(!config.contains("credential") && !config.contains("extraHeader"));
    // The user's configuration applied; its hooks did not run.
    assert!(seen.lock().unwrap().iter().any(|h| h == "from-user-config"));
    assert!(!home.join("hook-ran").exists());
    assert!(leftovers(&project).is_empty());

    // An existing folder is never cloned over.
    let error = clone_into(&request(&url, &project, &cancel, &progress))
        .unwrap_err()
        .to_string();
    assert!(error.contains("already exists"), "{error}");
}

#[test]
fn refusals_failures_and_cancels_leave_nothing_behind() {
    let home = home();
    let temp = Temp::new();
    let (base, _) = serve(upstream(&temp), true);
    let project = temp.0.join("project");
    fs::create_dir_all(&project).unwrap();
    let url = format!("{base}/hello.git");
    let progress = Mutex::new(String::new());
    // Credentials required and none given: Git fails without prompting.
    let cancel = AtomicBool::new(false);
    let error = clone_into(&request(&url, &project, &cancel, &progress))
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("Git"), "{error}");
    // The user's credential helper ran without the API tokens of the
    // bridge's environment, and with Git LFS downloads off.
    let seen = fs::read_to_string(home.join("helper-env")).unwrap();
    assert!(seen.contains("HOME="), "the helper ran: {seen}");
    assert!(!seen.contains("sentinel"), "a GitHub token reached Git");
    assert!(seen.contains("GIT_LFS_SKIP_SMUDGE=1"), "{seen}");
    assert!(seen.contains("GIT_TERMINAL_PROMPT=0"));
    assert!(!project.join("hello").exists());
    assert!(leftovers(&project).is_empty());
    let cancel = AtomicBool::new(true);
    let error = clone_into(&request(&url, &project, &cancel, &progress))
        .unwrap_err()
        .to_string();
    assert_eq!(error, "cancelled");
    assert!(!project.join("hello").exists());
    assert!(leftovers(&project).is_empty());
    // Only the transports allowed, no credentials in the URL, a real
    // project folder.
    for bad in [
        "file:///srv/hello.git".to_owned(),
        format!("{}/hello.git", base.replace("http://", "http://user:pw@")),
        "ext::sh -c touch% /tmp/pwned".to_owned(),
        "https://github.com/o/hello.git".to_owned(),
    ] {
        assert!(
            clone_into(&request(&bad, &project, &cancel, &progress)).is_err(),
            "{bad}"
        );
    }
    let missing = project.join("missing");
    let error = clone_into(&request(&url, &missing, &cancel, &progress))
        .unwrap_err()
        .to_string();
    assert!(error.contains("is missing"), "{error}");
}
