// GitHub mocked by a local HTTP server and token programs written as
// shell scripts; no network. Clones are tested in tests/github_clone.rs.
use super::*;
use std::{
    io::Write,
    net::TcpListener,
    sync::{atomic::AtomicU64, mpsc},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kelpmux-github-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    /// An executable shell script.
    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn token_of(auth: &GitHubAuth) -> Result<String> {
    read_token(auth)
}

type Route = Box<dyn Fn(&str) -> (u16, Vec<(String, String)>, Value) + Send + Sync>;
/// Each request's path and Authorization header.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// A stand-in for GitHub's API: answers each request with `route(path)`,
/// records each request's path and Authorization header.
fn fake(route: Route) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: Seen = Arc::default();
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
            let auth = head
                .lines()
                .find_map(|l| {
                    l.split_once(':')
                        .filter(|(n, _)| n.eq_ignore_ascii_case("authorization"))
                        .map(|(_, v)| v.trim().to_owned())
                })
                .unwrap_or_default();
            log.lock().unwrap().push((path.clone(), auth));
            let (code, headers, body) = route(&path);
            let body = serde_json::to_vec(&body).unwrap();
            let mut response = format!(
                "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            );
            for (name, value) in headers {
                response += &format!("{name}: {value}\r\n");
            }
            response += "\r\n";
            let _ = socket.write_all(response.as_bytes());
            let _ = socket.write_all(&body);
        }
    });
    (format!("http://{addr}"), seen)
}

fn h(name: &str, value: &str) -> (String, String) {
    (name.into(), value.into())
}

fn repo_json(owner: &str, name: &str, id: u64) -> Value {
    json!({
        "id": id,
        "name": name,
        "owner": {"login": owner},
        "private": id.is_multiple_of(2),
        "size": id,
        "default_branch": "main",
        "description": "A\u{7}repo",
        "pushed_at": "2026-11-20T10:00:00Z",
        "clone_url": "https://evil.example/x.git",
    })
}

/// The API of these tests: `/user`, two pages of repositories, facts.
fn github_api() -> Route {
    Box::new(move |path: &str| {
        let rate = vec![
            h("X-RateLimit-Limit", "5000"),
            h("X-RateLimit-Remaining", "4990"),
            h("X-RateLimit-Reset", "1900000000"),
        ];
        match path {
            "/user" => {
                let mut headers = rate;
                headers.push(h("X-OAuth-Scopes", "repo, read:org"));
                (
                    200,
                    headers,
                    json!({"login": "octocat", "name": "The Octocat"}),
                )
            }
            p if p.starts_with("/user/repos?") && p.ends_with("page=1") => {
                let mut rows: Vec<Value> = (1..=99)
                    .map(|i| repo_json("octocat", &format!("r{i:03}"), i))
                    .collect();
                // Skipped, not fatal.
                rows.push(json!({"id": 0, "name": "../x", "owner": {"login": "o"}}));
                (
                    200,
                    vec![h(
                        "Link",
                        "<https://api.github.com/user/repos?page=2>; rel=\"next\"",
                    )],
                    json!(rows),
                )
            }
            p if p.starts_with("/user/repos?") && p.ends_with("page=2") => {
                (200, vec![], json!([repo_json("acme", "Tool", 500)]))
            }
            "/repos/octocat/hello" => (
                200,
                vec![],
                json!({"id": 7, "name": "hello", "owner": {"login": "octocat"}, "visibility": "internal", "default_branch": "trunk", "size": 12}),
            ),
            p if p.starts_with("/repos/octocat/hello/pulls?") => (
                200,
                vec![h(
                    "Link",
                    "<https://api.github.com/repositories/7/pulls?state=open&per_page=1&page=2>; rel=\"next\", <https://api.github.com/repositories/7/pulls?state=open&per_page=1&page=14>; rel=\"last\"",
                )],
                json!([{"number": 1}]),
            ),
            p if p.starts_with("/repos/octocat/limited") => (
                403,
                vec![h("X-RateLimit-Remaining", "0"), h("X-RateLimit-Reset", "1")],
                json!({"message": "rate limited"}),
            ),
            _ => (404, vec![], json!({"message": "Not Found"})),
        }
    })
}

#[test]
fn token_programs_give_their_first_line_and_failures_never_show_output() {
    let temp = Temp::new();
    let good = temp.script("good", "printf ' ghp_secret123 \\nsecond line\\n'");
    assert_eq!(
        token_of(&GitHubAuth::Command(good)).unwrap(),
        "ghp_secret123"
    );
    let failing = temp.script("failing", "echo ghp_leak; echo ghp_leak2 >&2; exit 3");
    let error = token_of(&GitHubAuth::Command(failing))
        .unwrap_err()
        .to_string();
    assert!(error.contains("exit status 3"), "{error}");
    assert!(!error.contains("ghp_leak"), "{error}");
    let empty = temp.script("empty", "echo");
    let error = token_of(&GitHubAuth::Command(empty))
        .unwrap_err()
        .to_string();
    assert!(error.contains("printed no token"), "{error}");
    let spaced = temp.script("spaced", "echo 'two words'");
    assert!(token_of(&GitHubAuth::Command(spaced)).is_err());
    let flood = temp.script("flood", "head -c 20000 /dev/zero | tr '\\0' a");
    let error = token_of(&GitHubAuth::Command(flood))
        .unwrap_err()
        .to_string();
    assert!(error.contains("more than"), "{error}");
    let missing = GitHubAuth::Command(temp.0.join("nope"));
    assert!(
        token_of(&missing)
            .unwrap_err()
            .to_string()
            .contains("not an executable")
    );
    // Killed with its children at the timeout.
    let slow = temp.script("slow", "sleep 30 & wait");
    let started = Instant::now();
    let error = run_token_command(&slow, &[], Duration::from_millis(300))
        .unwrap_err()
        .to_string();
    assert!(error.contains("did not finish"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn identity_reads_login_scopes_and_rate_and_errors_never_echo() {
    let (base, seen) = fake(github_api());
    let client = Client::local(&base);
    let me = identity(&client, "ghp_abc").unwrap();
    assert_eq!(me["login"], "octocat");
    assert_eq!(me["name"], "The Octocat");
    assert_eq!(me["scopes"], json!(["repo", "read:org"]));
    assert_eq!(
        me["rate"],
        json!({"limit": 5000, "remaining": 4990, "reset": 1900000000u64})
    );
    assert_eq!(
        seen.lock().unwrap()[0],
        ("/user".into(), "Bearer ghp_abc".into())
    );

    // A rejected token, a refusal and a rate limit: no body, no token.
    let (base, _) = fake(Box::new(|path: &str| match path {
        "/user" => (401, vec![], json!({"message": "Bad credentials ghp_abc"})),
        "/repos/a/b" => (
            429,
            vec![h("Retry-After", "60")],
            json!({"message": "slow down"}),
        ),
        _ => (500, vec![], json!({"message": "boom ghp_abc"})),
    }));
    let client = Client::local(&base);
    let error = identity(&client, "ghp_abc").unwrap_err();
    assert!(error.downcast_ref::<Rejected>().is_some());
    assert!(!error.to_string().contains("ghp_abc"));
    let error = repository_state(&client, "ghp_abc", "a/b").unwrap_err();
    assert!(
        error
            .downcast_ref::<Limited>()
            .is_some_and(|l| l.0.is_some())
    );
    let error = list_repositories(&client, "ghp_abc")
        .unwrap_err()
        .to_string();
    assert_eq!(error, "GitHub returned HTTP 500");
    // Fine-grained tokens send no scopes.
    let (base, _) = fake(Box::new(|_: &str| (200, vec![], json!({"login": "me"}))));
    let me = identity(&Client::local(&base), "github_pat_x").unwrap();
    assert_eq!(me["scopes"], Value::Null);
    assert_eq!(me["rate"], Value::Null);
}

#[test]
fn repositories_are_listed_across_pages_with_clone_urls_built_here() {
    let (base, seen) = fake(github_api());
    let (repos, truncated) = list_repositories(&Client::local(&base), "ghp_abc").unwrap();
    assert!(!truncated);
    assert_eq!(repos.len(), 100);
    // Sorted regardless of case: acme/Tool first.
    assert_eq!(repos[0]["full_name"], "acme/Tool");
    assert_eq!(repos[0]["ssh_url"], "git@github.com:acme/Tool.git");
    assert_eq!(repos[0]["https_url"], "https://github.com/acme/Tool.git");
    assert_eq!(repos[0]["description"], "Arepo");
    assert_eq!(repos[0]["size_kib"], 500);
    assert!(!repos.iter().any(|r| r.to_string().contains("evil.example")));
    let paths: Vec<String> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|(p, _)| p.clone())
        .collect();
    assert!(paths[0].contains("affiliation=owner,collaborator,organization_member"));
    assert_eq!(paths.len(), 2);
}

#[test]
fn facts_count_open_pulls_and_hide_what_the_token_cannot_see() {
    let (base, _) = fake(github_api());
    let client = Client::local(&base);
    let facts = repository_state(&client, "t", "octocat/hello").unwrap();
    assert_eq!(facts["visible"], true);
    assert_eq!(facts["visibility"], "internal");
    assert_eq!(facts["default_branch"], "trunk");
    assert_eq!(facts["open_pulls"], 14);
    let facts = repository_state(&client, "t", "octocat/secret").unwrap();
    assert_eq!(
        facts,
        json!({"full_name": "octocat/secret", "visible": false})
    );
    assert_eq!(counted(None, 0), 0);
    assert_eq!(counted(None, 1), 1);
    assert_eq!(
        counted(Some("<https://x/?state=open&page=3>; rel=\"last\""), 1),
        3
    );
}

fn settings(auth: Option<GitHubAuth>) -> Settings {
    let settings = GitHubSettings {
        auth,
        ssh_auth_sock: None,
    };
    Arc::new(move || Some(settings.clone()))
}

fn wait(github: &GitHub, done: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..200 {
        let status = github.status().unwrap();
        if done(&status) {
            return status;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out: {}", github.status().unwrap());
}

#[test]
fn the_service_signs_in_lists_and_keeps_the_token_to_itself() {
    let temp = Temp::new();
    let count = temp.0.join("count");
    let program = temp.script(
        "token",
        &format!("echo x >> '{}'; echo ghp_service_secret", count.display()),
    );
    let (base, seen) = fake(github_api());
    let github = GitHub::with(
        settings(Some(GitHubAuth::Command(program.clone()))),
        Client::local(&base),
    );
    // Not known yet: checked on the service's thread.
    let status = github.status().unwrap();
    assert_eq!(status["enabled"], true);
    assert_eq!(status["source"], "command");
    assert_eq!(status["source_label"], program.display().to_string());
    let status = wait(&github, |s| {
        s["busy"] == false && (s["signed_in"] == true || !s["error"].is_null())
    });
    assert_eq!(status["signed_in"], true, "{status}");
    assert_eq!(status["login"], "octocat");
    assert_eq!(status["scopes"], json!(["repo", "read:org"]));
    assert_eq!(status["rate"]["remaining"], 4990);
    github.repositories(false).unwrap();
    let listed = loop {
        let listed = github.repositories(false).unwrap();
        if listed["busy"] == false {
            break listed;
        }
        thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(
        listed["repositories"].as_array().unwrap().len(),
        100,
        "{listed}"
    );
    let (sender, receiver) = mpsc::channel();
    github
        .state_request(
            vec!["octocat/hello".into(), "octocat/gone".into()],
            Box::new(move |r| sender.send(r).unwrap()),
        )
        .unwrap();
    let facts = receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert_eq!(facts["repos"]["octocat/hello"]["open_pulls"], 14);
    assert_eq!(facts["repos"]["octocat/gone"]["visible"], false);
    // Another request within the round interval is answered from the
    // cache, without asking GitHub.
    let asked = |path: &str| {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p == path)
            .count()
    };
    let (sender, receiver) = mpsc::channel();
    github
        .state_request(
            vec!["octocat/hello".into(), "octocat/other".into()],
            Box::new(move |r| sender.send(r).unwrap()),
        )
        .unwrap();
    let cached = receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert_eq!(cached["pending"], true);
    assert_eq!(cached["repos"]["octocat/hello"]["open_pulls"], 14);
    assert_eq!(asked("/repos/octocat/other"), 0);
    // An hour's budget spent: nothing more is fetched until it ends.
    {
        let mut state = github.state().unwrap();
        state.last_round = None;
        state.hour = Some(Instant::now());
        state.fetched = STATE_HOURLY;
    }
    let (sender, receiver) = mpsc::channel();
    github
        .state_request(
            vec!["octocat/other".into()],
            Box::new(move |r| sender.send(r).unwrap()),
        )
        .unwrap();
    let spent = receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert_eq!(spent["limited"], true, "{spent}");
    assert_eq!(asked("/repos/octocat/other"), 0);
    github.state().unwrap().backoff = None;
    // The program ran once: the token is kept in memory.
    assert_eq!(fs::read_to_string(&count).unwrap().lines().count(), 1);
    // Every request carried it; no answer to the browser does.
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .all(|(_, auth)| auth == "Bearer ghp_service_secret")
    );
    for answer in [github.status().unwrap(), listed, facts] {
        assert!(!answer.to_string().contains("ghp_service_secret"));
    }
    // Test reads it again.
    github.test().unwrap();
    wait(&github, |s| s["busy"] == false && s["signed_in"] == true);
    assert_eq!(fs::read_to_string(&count).unwrap().lines().count(), 2);
    // Bad URLs and names are refused before any work.
    for (slugs, error) in [
        (vec!["../x".to_owned()], "owner/name"),
        (vec!["a/b".to_owned(); 65], "at most"),
    ] {
        let e = github
            .state_request(slugs, Box::new(|_| {}))
            .unwrap_err()
            .to_string();
        assert!(e.contains(error), "{e}");
    }
}

#[test]
fn a_rejected_token_signs_out_and_a_missing_source_explains_itself() {
    let temp = Temp::new();
    let program = temp.script("token", "echo ghp_old");
    let (base, _) = fake(Box::new(|_: &str| {
        (401, vec![], json!({"message": "Bad credentials"}))
    }));
    let github = GitHub::with(
        settings(Some(GitHubAuth::Command(program))),
        Client::local(&base),
    );
    github.status().unwrap();
    let status = wait(&github, |s| s["busy"] == false && !s["error"].is_null());
    assert_eq!(status["signed_in"], false);
    assert!(
        status["error"]
            .as_str()
            .unwrap()
            .contains("rejected the token"),
        "{status}"
    );
    assert!(github.state().unwrap().token.is_none());

    let failing = temp.script("failing", "exit 1");
    let github = GitHub::with(
        settings(Some(GitHubAuth::Command(failing))),
        Client::local(&base),
    );
    github.status().unwrap();
    let status = wait(&github, |s| s["busy"] == false && !s["error"].is_null());
    assert!(
        status["error"].as_str().unwrap().contains("exit status 1"),
        "{status}"
    );

    let off = GitHub::with(settings(None), Client::local(&base));
    let status = off.status().unwrap();
    assert_eq!(status["enabled"], false);
    assert_eq!(status["busy"], false);
    let error = off.repositories(false).unwrap_err().to_string();
    assert!(error.contains("github.token_command"), "{error}");
    assert!(
        off.state_request(vec!["a/b".into()], Box::new(|_| {}))
            .is_err()
    );
}

#[test]
fn clone_urls_are_https_or_ssh_without_credentials() {
    for (url, scheme, name) in [
        (
            "git@github.com:octocat/Hello-World.git",
            "ssh",
            "Hello-World",
        ),
        ("ssh://git@github.com/octocat/hello", "ssh", "hello"),
        ("ssh://git@host.example:2222/srv/repo.git/", "ssh", "repo"),
        ("https://github.com/octocat/hello.git", "https", "hello"),
        ("https://git.example.com:8443/a/b/c.d", "https", "c.d"),
    ] {
        let parsed = clone_url(url, CLONE_SCHEMES).unwrap();
        assert_eq!(
            (parsed.scheme, parsed.name.as_str()),
            (scheme, name),
            "{url}"
        );
    }
    for url in [
        "https://user:token@github.com/o/r.git",
        "https://ghp_x@github.com/o/r.git",
        "http://github.com/o/r.git",
        "file:///srv/r.git",
        "/srv/r.git",
        "ext::sh -c touch% /tmp/pwned",
        "fd::3",
        "-oProxyCommand=x@h:r",
        "git@-oProxyCommand:r",
        "ssh://-oProxyCommand@host/r.git",
        "-x@host:r",
        "github.com:o/r",
        "git@github.com:o/..",
        "git@github.com:o/.hidden",
        "git@github.com:o/r;rm",
        "https://github.com/o/r?x=1",
        "https://github.com:0/o/r",
        "https://github.com/",
        "",
    ] {
        assert!(clone_url(url, CLONE_SCHEMES).is_err(), "{url}");
    }
    assert!(clone_url("http://127.0.0.1:9/r.git", &["http"]).is_ok());
}

#[test]
fn git_keeps_the_users_environment_but_not_gits_own() {
    for name in [
        "HOME",
        "PATH",
        "SSH_AUTH_SOCK",
        "GIT_SSH_COMMAND",
        "GIT_SSH",
        "LANG",
    ] {
        assert!(kept(name), "{name}");
    }
    for name in [
        "GIT_DIR",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
        "GIT_WORK_TREE",
        "GIT_TEMPLATE_DIR",
        "SSH_ASKPASS",
        "DISPLAY",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "GITHUB_ENTERPRISE_TOKEN",
    ] {
        assert!(!kept(name), "{name}");
    }
}
