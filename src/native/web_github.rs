//! GitHub in the web client, with the GitHub access this machine already
//! has: the repositories the user's token can see, read-only facts about
//! the GitHub repositories a project's checkouts come from, and cloning a
//! repository into a project with the user's own Git credentials.
//!
//! Its repository picker, facts and clone runner are adapted from
//! KelpHub's GitHub support, published here by its author, without
//! KelpHub's GitHub App, workspaces, database or tenants: KelpMux has one
//! user, whose bridge runs this module.
//!
//! # The token
//!
//! `[github]` in `config.toml` says where the token comes from
//! (`user_config::GitHubAuth`): `gh auth token`, `GH_TOKEN`/`GITHUB_TOKEN`
//! in the bridge's environment, or a program's first line of output. A
//! program runs without a shell or arguments, in its own session, killed
//! after [`TOKEN_COMMAND_TIMEOUT`], its output bounded and its error output
//! discarded (it could hold a secret). The token stays in this process's
//! memory for [`TOKEN_TTL`] at most, and is sent only to GitHub's API: never
//! to the browser, a file, a log, an error message or a Git process.
//!
//! # What crosses to the browser
//!
//! Every GitHub request is made here, as the bridge's user, never by the
//! browser. The browser gets public metadata only: the signed-in login,
//! the token's scopes and rate limit, repository names, visibility,
//! descriptions, sizes and counts.
//!
//! # Cloning
//!
//! [`clone_into`] runs Git with the user's own configuration and
//! credentials (SSH agent, credential helper), so nothing is injected and
//! nothing is written but the checkout; the remote stays the URL given.
//! Only the URL's own transport (`https` or `ssh`) is allowed, hooks,
//! templates, fsmonitor and submodules are off, objects are checked, Git
//! cannot prompt, and it is killed (with its process group) at its
//! timeout, on cancel or when the bridge dies. The clone is made in a
//! hidden sibling folder and renamed into place only when complete, never
//! over an existing folder.
use super::{
    user_config::{GitHubAuth, GitHubSettings, github_name},
    web_git,
};
use crate::Result;

type Error = Box<dyn std::error::Error + Send + Sync>;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    os::unix::{fs::MetadataExt, fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const API: &str = "https://api.github.com";
const API_VERSION: &str = "2026-03-10";
/// Largest GitHub response.
const MAX: usize = 2 * 1024 * 1024;
/// Repositories listed, at most (20 pages of 100).
const REPOSITORY_LIMIT: usize = 2000;
/// How long the repository list is reused unless refreshed.
const REPOSITORIES_TTL: Duration = Duration::from_secs(300);
/// How long a repository's facts are reused.
const STATE_TTL: Duration = Duration::from_secs(300);
/// Most repositories one `github_state` request names, and fetches.
pub const MAX_STATE_REPOS: usize = 64;
const STATE_FETCHES: usize = 16;
/// A `github_state` request answers with what it has by then.
const STATE_BUDGET: Duration = Duration::from_secs(10);
/// One facts fetch per bridge starts at most this often; a request in
/// between is answered from the cache. Each repository is fetched again
/// only after [`STATE_TTL`].
const STATE_ROUND_INTERVAL: Duration = Duration::from_secs(10);
/// Repositories one bridge fetches facts for per hour, at most (two API
/// requests each), so a page that loops on `github_state` cannot spend
/// the token's rate limit.
const STATE_HOURLY: u32 = 500;
const HOUR: Duration = Duration::from_secs(3600);
/// How long a token is kept in memory.
pub const TOKEN_TTL: Duration = Duration::from_secs(300);
/// How long a failed token source is left alone (Test retries at once).
const TOKEN_RETRY: Duration = Duration::from_secs(30);
/// How long a token program may run, and how much it may print.
pub const TOKEN_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const TOKEN_OUTPUT: usize = 8192;
/// How long GitHub is left alone after a rate limit without a reset time.
const BACKOFF: Duration = Duration::from_secs(300);
/// Longest a clone may run.
pub const CLONE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Repositories larger than this (GitHub's `size`, in KiB) are refused.
pub const MAX_CLONE_KIB: u64 = 5 * 1024 * 1024;
/// Clones one bridge runs at a time, and finished ones it remembers.
const MAX_CLONES: usize = 2;
const KEPT_CLONES: usize = 8;
/// Where `gh` is looked for after the bridge's `PATH`.
const GH_BINARIES: &[&str] = &["/usr/bin/gh", "/usr/local/bin/gh"];
/// The transports the web client clones over.
pub const CLONE_SCHEMES: &[&str] = &["https", "ssh"];

fn now() -> u64 {
    crate::now()
}

/// A random 128-bit identifier, as 32 hexadecimal digits.
fn identifier() -> Result<String> {
    let mut bytes = [0u8; 16];
    // SAFETY: writes at most `bytes.len()` bytes into `bytes`.
    let n = unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), 0) };
    if n != bytes.len() as isize {
        return Err("no randomness available".into());
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// A token as GitHub issues them: printable ASCII without spaces.
fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() < 4096 && token.bytes().all(|c| c.is_ascii_graphic())
}

/// What the browser is told about where the token comes from.
fn describe(auth: &GitHubAuth) -> (&'static str, String) {
    match auth {
        GitHubAuth::Gh => ("gh", "GitHub CLI (gh auth token)".into()),
        GitHubAuth::Env => ("env", "GH_TOKEN or GITHUB_TOKEN".into()),
        GitHubAuth::Command(path) => ("command", path.display().to_string()),
    }
}

/// `gh`: in the bridge's `PATH` (absolute entries only), else
/// [`GH_BINARIES`].
fn gh_binary() -> Option<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join("gh"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
        .into_iter()
        .chain(GH_BINARIES.iter().map(PathBuf::from))
        .find(|path| executable(path))
}

/// Runs a token program (argv only, no shell) in its own session, killed
/// after `timeout`; returns its first line of output. Its error output is
/// discarded, and failures say only how it ended.
pub(crate) fn run_token_command(
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<String> {
    let name = if args.is_empty() {
        format!("`{}`", program.display())
    } else {
        "`gh auth token`".into()
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    // A program just written can be briefly busy (`ETXTBSY`) while another
    // thread's child still holds it open.
    let mut tries = 0;
    let mut child = loop {
        match command.spawn() {
            Ok(child) => break child,
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && tries < 20 => {
                tries += 1;
                thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return Err(format!("{name} could not be started").into()),
        }
    };
    let stdout = child.stdout.take().ok_or("token output unavailable")?;
    let reader = thread::spawn(move || {
        let mut bytes = vec![];
        let mut stdout = stdout;
        let mut chunk = [0u8; 4096];
        // Reads past the limit (discarding) so the program never blocks.
        while let Ok(n) = stdout.read(&mut chunk) {
            if n == 0 {
                break;
            }
            let take = n.min((TOKEN_OUTPUT + 1).saturating_sub(bytes.len()));
            bytes.extend_from_slice(&chunk[..take]);
        }
        bytes
    });
    let until = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= until {
            // SAFETY: the child leads its own process group.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let mut bytes = reader.join().map_err(|_| "token output unavailable")?;
    let Some(status) = status else {
        return Err(format!("{name} did not finish within {} seconds", timeout.as_secs()).into());
    };
    if !status.success() {
        bytes.fill(0);
        return Err(match status.code() {
            Some(code) => format!("{name} failed (exit status {code})"),
            None => format!("{name} was killed"),
        }
        .into());
    }
    if bytes.len() > TOKEN_OUTPUT {
        bytes.fill(0);
        return Err(format!("{name} printed more than {TOKEN_OUTPUT} bytes").into());
    }
    let token = String::from_utf8_lossy(&bytes)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_owned();
    bytes.fill(0);
    if !valid_token(&token) {
        return Err(format!("{name} printed no token on its first line").into());
    }
    Ok(token)
}

/// Reads the token from its source. Errors never contain it.
pub fn read_token(auth: &GitHubAuth) -> Result<String> {
    match auth {
        GitHubAuth::Gh => {
            let gh = gh_binary().ok_or(
                "the GitHub CLI (gh) is not installed where the bridge looks (its PATH, /usr/bin, /usr/local/bin)",
            )?;
            run_token_command(
                &gh,
                &["auth", "token", "--hostname", "github.com"],
                TOKEN_COMMAND_TIMEOUT,
            )
            .map_err(|e| format!("{e}; sign in on the server with `gh auth login`").into())
        }
        GitHubAuth::Env => ["GH_TOKEN", "GITHUB_TOKEN"]
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|t| !t.trim().is_empty()))
            .map(|t| t.trim().to_owned())
            .filter(|t| valid_token(t))
            .ok_or_else(|| {
                "neither GH_TOKEN nor GITHUB_TOKEN holds a token in the web bridge's environment"
                    .into()
            }),
        GitHubAuth::Command(path) => {
            if !executable(path) {
                return Err(format!(
                    "`{}` (github.token_command) is not an executable file",
                    path.display()
                )
                .into());
            }
            run_token_command(path, &[], TOKEN_COMMAND_TIMEOUT)
        }
    }
}

/// A GitHub API answer.
struct Response {
    code: u16,
    body: Value,
    /// The `Link` header, for counting and following pages.
    link: Option<String>,
    /// `X-OAuth-Scopes` (classic tokens only).
    scopes: Option<String>,
    /// `x-ratelimit-limit`, `-remaining` and `-reset`.
    rate: Option<(u64, u64, u64)>,
    /// A rate limit: `x-ratelimit-remaining` is 0 or `retry-after` is set;
    /// the Unix time to wait until, when given.
    limited: Option<Option<u64>>,
}

/// GitHub rejected the token (HTTP 401).
#[derive(Debug)]
struct Rejected;
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GitHub rejected the token; check where it comes from (github.auth or github.token_command) and sign in again")
    }
}
impl std::error::Error for Rejected {}

/// GitHub's rate limit, until a Unix time when it said.
#[derive(Debug)]
struct Limited(Option<u64>);
impl std::fmt::Display for Limited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GitHub's rate limit was reached; try again later")
    }
}
impl std::error::Error for Limited {}

/// The GitHub REST API: GET only, fixed headers, no redirects, a 15-second
/// limit and bounded responses.
#[derive(Clone)]
pub struct Client {
    agent: ureq::Agent,
    base: String,
}
impl Client {
    pub fn new() -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .https_only(true)
                .max_redirects(0)
                // Never a proxy from the environment: the token goes to
                // GitHub directly.
                .proxy(None)
                .http_status_as_error(false)
                .timeout_global(Some(Duration::from_secs(15)))
                .build()
                .into(),
            base: API.into(),
        }
    }
    /// A client of a local stand-in for the API (tests).
    pub fn local(base: &str) -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .max_redirects(0)
                // Never a proxy from the environment: the token goes to
                // GitHub directly.
                .proxy(None)
                .http_status_as_error(false)
                .timeout_global(Some(Duration::from_secs(5)))
                .build()
                .into(),
            base: base.into(),
        }
    }
    fn get(&self, path: &str, token: &str) -> Result<Response> {
        let mut response = self
            .agent
            .get(format!("{}{path}", self.base))
            .header("User-Agent", "KelpMux")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .header("Authorization", format!("Bearer {token}"))
            .call()
            .map_err(|_| "GitHub request failed; check the network and try again")?;
        let code = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let number = |name: &str| header(name).and_then(|v| v.trim().parse::<u64>().ok());
        let link = header("link");
        let scopes = header("x-oauth-scopes");
        let remaining = number("x-ratelimit-remaining");
        let reset = number("x-ratelimit-reset");
        let retry = number("retry-after");
        let rate = match (number("x-ratelimit-limit"), remaining, reset) {
            (Some(limit), Some(remaining), Some(reset)) => Some((limit, remaining, reset)),
            _ => None,
        };
        let limited = if matches!(code, 403 | 429) && (remaining == Some(0) || retry.is_some()) {
            Some(retry.map(|r| now() + r).or(reset))
        } else {
            None
        };
        let mut bytes = vec![];
        response
            .body_mut()
            .as_reader()
            .take(MAX as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "GitHub response interrupted")?;
        if bytes.len() > MAX {
            return Err("GitHub response exceeds its size limit".into());
        }
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).map_err(|_| "invalid GitHub response")?
        };
        Ok(Response {
            code,
            body,
            link,
            scopes,
            rate,
            limited,
        })
    }
    /// A successful request's answer. Errors never echo the response.
    fn call(&self, path: &str, token: &str) -> Result<Response> {
        let response = self.get(path, token)?;
        if let Some(until) = response.limited {
            return Err(Limited(until).into());
        }
        if response.code == 401 {
            return Err(Rejected.into());
        }
        if !(200..300).contains(&response.code) {
            return Err(format!("GitHub returned HTTP {}", response.code).into());
        }
        Ok(response)
    }
}
impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

/// Who the token signs in as: `/user`, with the token's scopes and rate
/// limit from the answer's headers.
fn identity(client: &Client, token: &str) -> Result<Value> {
    let response = client.call("/user", token)?;
    let login = response.body["login"]
        .as_str()
        .filter(|s| github_name(s))
        .ok_or("invalid GitHub login")?;
    let name: String = response.body["name"]
        .as_str()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(100)
        .collect();
    // Fine-grained and App tokens send no scopes header: `null`.
    let scopes = response.scopes.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_graphic()))
            .take(32)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    });
    Ok(json!({
        "login": login,
        "name": name,
        "scopes": scopes,
        "rate": rate_json(response.rate),
        "checked_at": now(),
    }))
}

fn rate_json(rate: Option<(u64, u64, u64)>) -> Value {
    rate.map_or(
        Value::Null,
        |(limit, remaining, reset)| json!({"limit": limit, "remaining": remaining, "reset": reset}),
    )
}

/// The public metadata of a repository object from the GitHub API, with
/// clone URLs built here (never GitHub's own fields).
fn repository(value: &Value) -> Result<Value> {
    let name = value["name"]
        .as_str()
        .filter(|s| github_name(s))
        .ok_or("invalid GitHub repository name")?;
    let owner = value["owner"]["login"]
        .as_str()
        .filter(|s| github_name(s))
        .ok_or("invalid GitHub repository owner")?;
    let id = value["id"]
        .as_u64()
        .filter(|id| *id > 0)
        .ok_or("invalid GitHub repository ID")?;
    let text = |key: &str, limit: usize| -> String {
        value[key]
            .as_str()
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_control())
            .take(limit)
            .collect()
    };
    Ok(json!({
        "id": id,
        "name": name,
        "full_name": format!("{owner}/{name}"),
        "ssh_url": format!("git@github.com:{owner}/{name}.git"),
        "https_url": format!("https://github.com/{owner}/{name}.git"),
        "private": value["private"] == true,
        "archived": value["archived"] == true,
        "fork": value["fork"] == true,
        "default_branch": text("default_branch", 255),
        "description": text("description", 240),
        "size_kib": value["size"].as_u64().unwrap_or(0),
        "pushed_at": text("pushed_at", 32),
    }))
}

/// The repositories the token can see (owned, collaborator and
/// organization member), at most [`REPOSITORY_LIMIT`]; and whether there
/// were more.
fn list_repositories(client: &Client, token: &str) -> Result<(Vec<Value>, bool)> {
    let mut repos: Vec<Value> = vec![];
    let mut seen = std::collections::BTreeSet::new();
    for page in 1..=(REPOSITORY_LIMIT / 100) {
        let response = client.call(
            &format!(
                "/user/repos?affiliation=owner,collaborator,organization_member&sort=full_name&per_page=100&page={page}"
            ),
            token,
        )?;
        let rows = response
            .body
            .as_array()
            .ok_or("GitHub repositories missing")?;
        for row in rows {
            // A repository GitHub describes oddly is skipped, not fatal.
            if let Ok(repo) = repository(row)
                && seen.insert(repo["id"].as_u64())
            {
                repos.push(repo);
            }
        }
        let more = response
            .link
            .as_deref()
            .is_some_and(|l| l.contains("rel=\"next\""));
        if rows.len() < 100 && !more {
            repos.sort_by_key(|r| r["full_name"].as_str().unwrap_or("").to_lowercase());
            return Ok((repos, false));
        }
    }
    repos.sort_by_key(|r| r["full_name"].as_str().unwrap_or("").to_lowercase());
    Ok((repos, true))
}

/// The number of items of a list requested with `per_page=1`, from its
/// `Link` header (the last page's number) and its length.
fn counted(link: Option<&str>, listed: usize) -> u64 {
    link.and_then(|link| {
        link.split(',')
            .find(|part| part.contains("rel=\"last\""))
            .and_then(|part| {
                let url = part.split(';').next()?.trim().trim_matches(['<', '>']);
                let query = url.split_once('?')?.1;
                query
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("page="))
                    .and_then(|page| page.parse().ok())
            })
    })
    .unwrap_or(listed as u64)
}

/// A repository's facts: default branch, visibility, archived, last push
/// and open pull requests (when the token may list them). A repository
/// the token cannot see is `{"visible": false}`.
fn repository_state(client: &Client, token: &str, slug: &str) -> Result<Value> {
    let response = client.get(&format!("/repos/{slug}"), token)?;
    if let Some(until) = response.limited {
        return Err(Limited(until).into());
    }
    if response.code == 401 {
        return Err(Rejected.into());
    }
    if matches!(response.code, 403 | 404) {
        return Ok(json!({"full_name": slug, "visible": false}));
    }
    if !(200..300).contains(&response.code) {
        return Err(format!("GitHub returned HTTP {}", response.code).into());
    }
    let repo = repository(&response.body)?;
    let visibility = match response.body["visibility"].as_str() {
        Some(v @ ("public" | "private" | "internal")) => v.to_owned(),
        _ if repo["private"] == true => "private".into(),
        _ => "public".into(),
    };
    let mut state = json!({
        "full_name": repo["full_name"],
        "visible": true,
        "default_branch": repo["default_branch"],
        "visibility": visibility,
        "archived": repo["archived"],
        "pushed_at": repo["pushed_at"],
        "size_kib": repo["size_kib"],
        "open_pulls": null,
    });
    let response = client.get(&format!("/repos/{slug}/pulls?state=open&per_page=1"), token)?;
    if let Some(until) = response.limited {
        return Err(Limited(until).into());
    }
    if (200..300).contains(&response.code) {
        let listed = response.body.as_array().map_or(0, Vec::len);
        state["open_pulls"] = json!(counted(response.link.as_deref(), listed));
    }
    Ok(state)
}

/// One clone this bridge started.
struct CloneJob {
    id: u64,
    project: String,
    /// `owner/name` for a github.com URL, else the folder name.
    full_name: String,
    url: String,
    path: PathBuf,
    state: &'static str,
    progress: Arc<Mutex<String>>,
    error: Option<String>,
    cancel: Arc<AtomicBool>,
}
impl CloneJob {
    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "project": self.project,
            "full_name": self.full_name,
            "url": self.url,
            "path": self.path,
            "state": self.state,
            "progress": self.progress.lock().map(|p| p.clone()).unwrap_or_default(),
            "error": self.error,
        })
    }
}

#[derive(Default)]
struct State {
    /// The token, when it was read, and the source it came from.
    token: Option<(String, Instant, GitHubAuth)>,
    /// The last failure to read it, and when.
    token_error: Option<(String, Instant, GitHubAuth)>,
    /// `/user`'s answer for the cached token.
    identity: Option<Value>,
    busy: bool,
    error: Option<String>,
    repos: Option<Vec<Value>>,
    truncated: bool,
    listed: Option<Instant>,
    clones: Vec<CloneJob>,
    next_clone: u64,
    /// Lowercase `owner/name` → (fetched, facts).
    facts: HashMap<String, (Instant, Value)>,
    fetching: bool,
    backoff: Option<Instant>,
    /// When the last facts fetch started.
    last_round: Option<Instant>,
    /// The current hour of facts fetches: when it began, how many so far.
    hour: Option<Instant>,
    fetched: u32,
}
impl State {
    /// Forgets the token and everything learned with it.
    fn sign_out(&mut self) {
        self.token = None;
        self.identity = None;
        self.repos = None;
        self.listed = None;
        self.facts.clear();
    }
}

/// Reads the `[github]` settings in effect.
pub type Settings = Arc<dyn Fn() -> Option<GitHubSettings> + Send + Sync>;

struct Inner {
    settings: Settings,
    client: Client,
    state: Mutex<State>,
}

/// The GitHub integration of one bridge.
#[derive(Clone)]
pub struct GitHub(Arc<Inner>);

type Reply = Box<dyn FnOnce(std::result::Result<Value, String>) + Send>;

impl GitHub {
    pub fn new(settings: Settings) -> Self {
        Self::with(settings, Client::new())
    }
    /// With another API (tests).
    pub fn with(settings: Settings, client: Client) -> Self {
        Self(Arc::new(Inner {
            settings,
            client,
            state: Mutex::default(),
        }))
    }
    fn state(&self) -> Result<std::sync::MutexGuard<'_, State>> {
        self.0
            .state
            .lock()
            .map_err(|_| "GitHub state unavailable".into())
    }
    fn settings(&self) -> GitHubSettings {
        (self.0.settings)().unwrap_or_default()
    }

    /// The token, from memory while fresh, else from its source. A source
    /// that just failed is not run again for [`TOKEN_RETRY`] unless
    /// `retry`.
    fn token(&self, retry: bool) -> Result<String> {
        let auth = self.settings().auth.ok_or(
            "GitHub is not signed in: set `github.auth` or `github.token_command` in config.toml",
        )?;
        {
            let mut state = self.state()?;
            match &state.token {
                Some((token, at, from)) if *from == auth && at.elapsed() < TOKEN_TTL => {
                    return Ok(token.clone());
                }
                Some(_) => state.sign_out(),
                None => {}
            }
            if !retry
                && let Some((error, at, from)) = &state.token_error
                && *from == auth
                && at.elapsed() < TOKEN_RETRY
            {
                return Err(error.clone().into());
            }
        }
        // Outside the lock: the source may take seconds.
        let read = read_token(&auth);
        let mut state = self.state()?;
        match read {
            Ok(token) => {
                state.token = Some((token.clone(), Instant::now(), auth));
                state.token_error = None;
                Ok(token)
            }
            Err(error) => {
                state.sign_out();
                state.token_error = Some((error.to_string(), Instant::now(), auth));
                Err(error)
            }
        }
    }

    /// Notes what an API failure means: a rejected token is forgotten, a
    /// rate limit pauses facts.
    fn failed(&self, error: &Error) {
        if let Ok(mut state) = self.state() {
            if error.downcast_ref::<Rejected>().is_some() {
                state.sign_out();
            }
            if let Some(Limited(until)) = error.downcast_ref::<Limited>() {
                let wait = until
                    .map(|until| Duration::from_secs(until.saturating_sub(now()).clamp(1, 3600)))
                    .unwrap_or(BACKOFF);
                state.backoff = Some(Instant::now() + wait);
            }
        }
    }

    /// `github_status`: where the token comes from, who it signs in as,
    /// the clones of this bridge. Never the token. Checks the sign-in on
    /// its own thread when it is not known yet.
    pub fn status(&self) -> Result<Value> {
        let settings = self.settings();
        let check = {
            let state = self.state()?;
            settings.auth.is_some()
                && state.identity.is_none()
                && !state.busy
                && state.error.is_none()
                && state.token_error.is_none()
        };
        if check {
            return self.job(|github| github.check(false));
        }
        self.describe(&settings)
    }

    fn describe(&self, settings: &GitHubSettings) -> Result<Value> {
        let state = self.state()?;
        let (source, label) = settings
            .auth
            .as_ref()
            .map(describe)
            .map_or((Value::Null, Value::Null), |(s, l)| (json!(s), json!(l)));
        let identity = state.identity.clone().unwrap_or(Value::Null);
        let sock = settings
            .ssh_auth_sock
            .clone()
            .or_else(|| std::env::var_os("SSH_AUTH_SOCK").map(PathBuf::from));
        Ok(json!({
            "enabled": settings.auth.is_some(),
            "source": source,
            "source_label": label,
            "signed_in": !identity.is_null(),
            "login": identity["login"],
            "name": identity["name"],
            "scopes": identity["scopes"],
            "rate": identity["rate"],
            "checked_at": identity["checked_at"],
            "ssh_agent": {
                "configured": settings.ssh_auth_sock.is_some(),
                "available": sock.as_deref().is_some_and(|s| {
                    use std::os::unix::fs::FileTypeExt;
                    fs::metadata(s).is_ok_and(|m| m.file_type().is_socket())
                }),
            },
            "busy": state.busy,
            "error": state.error.clone().or_else(|| state.token_error.as_ref().map(|(e, _, _)| e.clone())),
            "repositories_ready": state.repos.is_some(),
            "repository_count": state.repos.as_ref().map(Vec::len),
            "repositories_truncated": state.truncated,
            "clones": state.clones.iter().map(CloneJob::json).collect::<Vec<_>>(),
        }))
    }

    /// Runs `work` on its own thread unless other work is running; the
    /// result shows in `status` (`busy`, `error`).
    fn job(&self, work: impl FnOnce(&Self) -> Result<()> + Send + 'static) -> Result<Value> {
        let mut state = self.state()?;
        if !state.busy {
            state.busy = true;
            state.error = None;
            let service = self.clone();
            thread::spawn(move || {
                let result = work(&service);
                if let Err(error) = &result {
                    service.failed(error);
                }
                if let Ok(mut state) = service.0.state.lock() {
                    state.busy = false;
                    if let Err(error) = result {
                        // Token failures show as `token_error` already.
                        if state.token_error.is_none() {
                            state.error = Some(error.to_string());
                        }
                    }
                }
            });
        }
        drop(state);
        self.describe(&self.settings())
    }

    /// Reads the token (again when `fresh`) and asks GitHub who it is.
    fn check(&self, fresh: bool) -> Result<()> {
        let token = self.token(fresh)?;
        let identity = identity(&self.0.client, &token)?;
        self.state()?.identity = Some(identity);
        Ok(())
    }

    /// `github_test`: forgets the token, reads it again and checks it.
    pub fn test(&self) -> Result<Value> {
        {
            let mut state = self.state()?;
            if !state.busy {
                state.sign_out();
                state.token_error = None;
                state.error = None;
            }
        }
        self.job(|github| github.check(true))
    }

    /// `github_repositories`: the status, and the repositories the token
    /// can see once listed (on the service's thread; `refresh` lists
    /// again).
    pub fn repositories(&self, refresh: bool) -> Result<Value> {
        self.settings().auth.ok_or(
            "GitHub is not signed in: set `github.auth` or `github.token_command` in config.toml",
        )?;
        let stale = {
            let state = self.state()?;
            refresh
                || state
                    .listed
                    .is_none_or(|at| at.elapsed() >= REPOSITORIES_TTL)
        };
        let mut status = if stale {
            self.job(|github| {
                let token = github.token(false)?;
                if github.state()?.identity.is_none() {
                    let identity = identity(&github.0.client, &token)?;
                    github.state()?.identity = Some(identity);
                }
                let (repos, truncated) = list_repositories(&github.0.client, &token)?;
                let mut state = github.state()?;
                state.repos = Some(repos);
                state.truncated = truncated;
                state.listed = Some(Instant::now());
                Ok(())
            })?
        } else {
            self.describe(&self.settings())?
        };
        status["repositories"] = json!(self.state()?.repos);
        Ok(status)
    }

    /// `github_clone`: clones `url` into `<folder>/<name of the URL>` of
    /// project `project`, on its own thread, with the user's own Git
    /// credentials.
    pub fn clone_repository(&self, project: &str, folder: &Path, url: &str) -> Result<Value> {
        let parsed = clone_url(url, CLONE_SCHEMES)?;
        check_folder(folder)?;
        let target = folder.join(&parsed.name);
        if fs::symlink_metadata(&target).is_ok() {
            return Err(format!("{} already exists", target.display()).into());
        }
        let slug = web_git::github_slug(url);
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(String::from("Starting")));
        let job_id = {
            let mut state = self.state()?;
            if state.clones.iter().filter(|c| c.state == "running").count() >= MAX_CLONES {
                return Err("other clones are running; wait for one to finish".into());
            }
            if state
                .clones
                .iter()
                .any(|c| c.state == "running" && c.path == target)
            {
                return Err("this repository is already being cloned there".into());
            }
            state.next_clone += 1;
            let id = state.next_clone;
            state.clones.push(CloneJob {
                id,
                project: project.to_owned(),
                full_name: slug.clone().unwrap_or_else(|| parsed.name.clone()),
                url: url.to_owned(),
                path: target.clone(),
                state: "running",
                progress: progress.clone(),
                error: None,
                cancel: cancel.clone(),
            });
            let finished = state.clones.len().saturating_sub(KEPT_CLONES);
            let mut dropped = 0;
            state.clones.retain(|c| {
                let old = dropped < finished && c.state != "running";
                dropped += usize::from(old);
                !old
            });
            id
        };
        let service = self.clone();
        let url = url.to_owned();
        let folder = folder.to_owned();
        let ssh_auth_sock = self.settings().ssh_auth_sock;
        thread::spawn(move || {
            let result = (|| -> Result<PathBuf> {
                // A GitHub repository's size, when the token can tell.
                if let Some(slug) = &slug
                    && service.settings().auth.is_some()
                {
                    set(&progress, "Checking the repository on GitHub");
                    if let Ok(token) = service.token(false)
                        && let Ok(facts) = repository_state(&service.0.client, &token, slug)
                        && let Some(size) = facts["size_kib"].as_u64()
                        && size > MAX_CLONE_KIB
                    {
                        return Err(format!(
                            "{slug} is {} GiB on GitHub; KelpMux clones repositories up to {} GiB (clone it in a terminal)",
                            size / (1024 * 1024),
                            MAX_CLONE_KIB / (1024 * 1024)
                        )
                        .into());
                    }
                }
                clone_into(&CloneRequest {
                    url: &url,
                    folder: &folder,
                    schemes: CLONE_SCHEMES,
                    ssh_auth_sock: ssh_auth_sock.as_deref(),
                    timeout: CLONE_TIMEOUT,
                    cancel: &cancel,
                    progress: &progress,
                })
            })();
            if let Ok(mut state) = service.0.state.lock()
                && let Some(job) = state.clones.iter_mut().find(|c| c.id == job_id)
            {
                match result {
                    Ok(path) => {
                        job.state = "done";
                        job.path = path;
                        set(&progress, "");
                    }
                    Err(error) => {
                        job.state = if cancel.load(Ordering::Acquire) {
                            "cancelled"
                        } else {
                            "failed"
                        };
                        job.error = Some(error.to_string());
                    }
                }
            }
        });
        self.describe(&self.settings())
    }

    /// `github_clone_cancel`: stops a running clone; its partial folder is
    /// removed.
    pub fn cancel_clone(&self, id: u64) -> Result<Value> {
        {
            let state = self.state()?;
            let job = state
                .clones
                .iter()
                .find(|c| c.id == id)
                .ok_or("this clone is not known")?;
            job.cancel.store(true, Ordering::Release);
        }
        self.describe(&self.settings())
    }

    /// `github_state`: facts about `slugs` (`owner/name`, at most
    /// [`MAX_STATE_REPOS`]) from the cache, fetching the missing or stale
    /// ones within [`STATE_BUDGET`] on its own thread unless a fetch is
    /// already running. `reply` gets `{"repos": {slug: facts}}`.
    pub fn state_request(&self, slugs: Vec<String>, reply: Reply) -> Result<()> {
        if slugs.len() > MAX_STATE_REPOS {
            return Err(format!("ask for at most {MAX_STATE_REPOS} repositories").into());
        }
        for slug in &slugs {
            let valid = slug
                .split_once('/')
                .is_some_and(|(o, n)| github_name(o) && github_name(n));
            if !valid {
                return Err("repositories are named owner/name".into());
            }
        }
        self.settings().auth.ok_or("GitHub is not signed in")?;
        {
            let mut state = self.state()?;
            if state.fetching
                || state
                    .last_round
                    .is_some_and(|at| at.elapsed() < STATE_ROUND_INTERVAL)
            {
                drop(state);
                reply(self.facts(&slugs, true).map_err(|e| e.to_string()));
                return Ok(());
            }
            state.fetching = true;
            state.last_round = Some(Instant::now());
        }
        let service = self.clone();
        thread::spawn(move || {
            let fetched = service.fetch_facts(&slugs);
            if let Ok(mut state) = service.0.state.lock() {
                state.fetching = false;
            }
            reply(match fetched {
                Ok(()) => service.facts(&slugs, false).map_err(|e| e.to_string()),
                Err(error) => Err(error.to_string()),
            });
        });
        Ok(())
    }

    /// The cached facts of `slugs`.
    fn facts(&self, slugs: &[String], pending: bool) -> Result<Value> {
        let state = self.state()?;
        let repos: serde_json::Map<String, Value> = slugs
            .iter()
            .filter_map(|slug| {
                let (_, facts) = state.facts.get(&slug.to_lowercase())?;
                Some((slug.clone(), facts.clone()))
            })
            .collect();
        Ok(json!({
            "repos": repos,
            "pending": pending || repos.len() < slugs.len(),
            "limited": state.backoff.is_some_and(|until| until > Instant::now()),
        }))
    }

    fn fetch_facts(&self, slugs: &[String]) -> Result<()> {
        let deadline = Instant::now() + STATE_BUDGET;
        let wanted: Vec<String> = {
            let state = self.state()?;
            if state.backoff.is_some_and(|until| until > Instant::now()) {
                return Ok(());
            }
            let mut seen = std::collections::HashSet::new();
            slugs
                .iter()
                .map(|s| s.to_lowercase())
                .filter(|s| seen.insert(s.clone()))
                .filter(|s| {
                    state
                        .facts
                        .get(s)
                        .is_none_or(|(at, _)| at.elapsed() >= STATE_TTL)
                })
                .take(STATE_FETCHES)
                .collect()
        };
        if wanted.is_empty() {
            return Ok(());
        }
        let token = self.token(false)?;
        for slug in wanted {
            if Instant::now() >= deadline {
                break;
            }
            {
                let mut state = self.state()?;
                let now = Instant::now();
                let start = match state.hour {
                    Some(start) if now.duration_since(start) < HOUR => start,
                    _ => {
                        state.hour = Some(now);
                        state.fetched = 0;
                        now
                    }
                };
                if state.fetched >= STATE_HOURLY {
                    state.backoff = Some(start + HOUR);
                    break;
                }
                state.fetched += 1;
            }
            match repository_state(&self.0.client, &token, &slug) {
                Ok(facts) => self.remember(&slug, facts)?,
                Err(error) => {
                    self.failed(&error);
                    if error.downcast_ref::<Limited>().is_some() {
                        break;
                    }
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    fn remember(&self, slug: &str, facts: Value) -> Result<()> {
        let mut state = self.state()?;
        if state.facts.len() >= 1024 {
            state.facts.retain(|_, (at, _)| at.elapsed() < STATE_TTL);
        }
        state
            .facts
            .insert(slug.to_lowercase(), (Instant::now(), facts));
        Ok(())
    }
}

fn set(progress: &Mutex<String>, text: &str) {
    if let Ok(mut p) = progress.lock() {
        *p = text.to_owned();
    }
}

/// A folder clones may go in: an existing directory owned by this user,
/// not a symbolic link.
fn check_folder(folder: &Path) -> Result<()> {
    let m = fs::symlink_metadata(folder)
        .map_err(|_| format!("the project folder {} is missing", folder.display()))?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || !folder.is_absolute() {
        return Err(format!(
            "the project folder {} is not a folder of your account",
            folder.display()
        )
        .into());
    }
    Ok(())
}

/// A clone URL, checked.
#[derive(Debug, PartialEq, Eq)]
pub struct CloneUrl {
    /// Git's name of its transport: `https`, `http` or `ssh`.
    pub scheme: &'static str,
    /// The folder it is cloned into: the last path segment without `.git`.
    pub name: String,
}

/// Checks a clone URL: `https://host[:port]/path` without credentials,
/// `ssh://[user@]host[:port]/path`, or `user@host:path` (SSH), with only
/// letters, digits and `/:._-~@`; and a transport in `schemes`. Its last
/// segment, without `.git`, names the folder.
pub fn clone_url(url: &str, schemes: &[&str]) -> Result<CloneUrl> {
    let invalid = || -> Error {
        "enter an HTTPS or SSH clone URL, such as git@github.com:owner/name.git".into()
    };
    if url.len() > 2048
        || url.starts_with('-')
        || !url
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/:._-~@".contains(&b))
    {
        return Err(invalid());
    }
    let (scheme, authority, path) = if let Some(rest) = url.strip_prefix("https://") {
        let (host, path) = rest.split_once('/').ok_or_else(invalid)?;
        if host.contains('@') {
            return Err("clone URLs must not hold credentials".into());
        }
        ("https", host, path)
    } else if let Some(rest) = url.strip_prefix("http://") {
        let (host, path) = rest.split_once('/').ok_or_else(invalid)?;
        if host.contains('@') {
            return Err("clone URLs must not hold credentials".into());
        }
        ("http", host, path)
    } else if let Some(rest) = url.strip_prefix("ssh://") {
        let (authority, path) = rest.split_once('/').ok_or_else(invalid)?;
        ("ssh", authority, path)
    } else if url.contains("://") || url.contains("::") {
        return Err(invalid());
    } else {
        // `user@host:path`: Git's scp-like SSH syntax.
        let (authority, path) = url.split_once(':').ok_or_else(invalid)?;
        if !authority.contains('@') || authority.contains('/') {
            return Err(invalid());
        }
        ("ssh", authority, path)
    };
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(user, host)| {
            if user.is_empty() || user.contains('@') || user.starts_with('-') {
                ""
            } else {
                host
            }
        });
    let host = if scheme == "ssh" && !url.starts_with("ssh://") {
        host
    } else {
        host.split_once(':').map_or(host, |(name, port)| {
            if port.parse::<u16>().is_ok_and(|p| p > 0) {
                name
            } else {
                ""
            }
        })
    };
    if host.is_empty()
        || host.starts_with(['-', '.'])
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-.".contains(&b))
        || path.split('/').any(|part| part == "..")
    {
        return Err(invalid());
    }
    let last = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let name = last.strip_suffix(".git").unwrap_or(last);
    if !github_name(name) || name.starts_with('.') {
        return Err("the URL does not end with a repository name".into());
    }
    if !schemes.contains(&scheme) {
        return Err(format!("{scheme} clone URLs are not accepted here").into());
    }
    Ok(CloneUrl {
        scheme,
        name: name.to_owned(),
    })
}

/// What [`clone_into`] clones, where, how.
pub struct CloneRequest<'a> {
    /// Checked by [`clone_url`] against `schemes`.
    pub url: &'a str,
    /// The existing folder the clone goes in.
    pub folder: &'a Path,
    /// The transports accepted: [`CLONE_SCHEMES`] for the web client.
    pub schemes: &'a [&'a str],
    /// `SSH_AUTH_SOCK` for Git's SSH, instead of the bridge's own.
    pub ssh_auth_sock: Option<&'a Path>,
    pub timeout: Duration,
    pub cancel: &'a AtomicBool,
    /// The last progress line Git printed.
    pub progress: &'a Mutex<String>,
}

/// The last progress or error line of Git's standard error, printable.
fn last_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .split(['\r', '\n'])
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect()
}

/// Environment variables Git keeps from the bridge: everything but Git's
/// own (which could point it at another repository or configuration),
/// except how it runs SSH, graphical password prompts, and GitHub tokens
/// (`gh auth git-credential` then uses gh's stored login, and no token
/// meant for the API reaches a program the clone runs).
fn kept(name: &str) -> bool {
    if let Some(git) = name.strip_prefix("GIT_") {
        return matches!(git, "SSH" | "SSH_COMMAND" | "SSH_VARIANT");
    }
    !matches!(
        name,
        "SSH_ASKPASS"
            | "DISPLAY"
            | "WAYLAND_DISPLAY"
            | "GH_TOKEN"
            | "GITHUB_TOKEN"
            | "GH_ENTERPRISE_TOKEN"
            | "GITHUB_ENTERPRISE_TOKEN"
    )
}

/// Clones `url` into `<folder>/<name of the URL>` through the hardened
/// runner (see the module documentation), with the user's own Git
/// configuration and credentials: in a hidden sibling folder first,
/// renamed into place, never over an existing one. Returns the checkout.
pub fn clone_into(request: &CloneRequest) -> Result<PathBuf> {
    let CloneUrl { scheme, name } = clone_url(request.url, request.schemes)?;
    check_folder(request.folder)?;
    let target = request.folder.join(&name);
    if fs::symlink_metadata(&target).is_ok() {
        return Err(format!("{} already exists", target.display()).into());
    }
    let temp = request
        .folder
        .join(format!(".kelpmux-clone-{name}-{}", identifier()?));
    let mut command = Command::new(web_git::git_binary()?);
    command.env_clear();
    command.envs(std::env::vars_os().filter(|(name, _)| name.to_str().is_some_and(kept)));
    if let Some(sock) = request.ssh_auth_sock {
        command.env("SSH_AUTH_SOCK", sock);
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ALLOW_PROTOCOL", scheme)
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_LFS_SKIP_SMUDGE", "1");
    for setting in [
        "core.hooksPath=/dev/null".to_owned(),
        "core.fsmonitor=false".into(),
        "init.templateDir=".into(),
        "http.lowSpeedLimit=1000".into(),
        "http.lowSpeedTime=60".into(),
        "protocol.allow=never".into(),
        format!("protocol.{scheme}.allow=always"),
        "submodule.recurse=false".into(),
        "transfer.fsckObjects=true".into(),
        "gc.auto=0".into(),
        // No Git LFS download during the clone (`git lfs pull` later).
        "filter.lfs.process=".into(),
        "filter.lfs.smudge=".into(),
        "filter.lfs.required=false".into(),
    ] {
        command.arg("-c").arg(setting);
    }
    command
        .args(["clone", "--progress", "--no-recurse-submodules", "--"])
        .arg(request.url)
        .arg(&temp)
        .current_dir(request.folder)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // SAFETY: only async-signal-safe calls between fork and exec. Its own
    // session: no terminal to prompt on, and killed as a group.
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
        .map_err(|e| format!("Git could not start: {e}"))?;
    let mut stderr = child.stderr.take().ok_or("Git output unavailable")?;
    let shown = Arc::new(Mutex::new(Vec::<u8>::new()));
    let reader = {
        let shown = shown.clone();
        thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                if let Ok(mut tail) = shown.lock() {
                    tail.extend_from_slice(&chunk[..n]);
                    let excess = tail.len().saturating_sub(4096);
                    tail.drain(..excess);
                }
            }
        })
    };
    let deadline = Instant::now() + request.timeout;
    let mut stopped = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => stopped = Some("Git could not be watched"),
        }
        if request.cancel.load(Ordering::Acquire) {
            stopped = Some("cancelled");
        } else if Instant::now() >= deadline {
            stopped = Some("the clone took too long and was stopped");
        }
        if stopped.is_some() {
            // SAFETY: Git leads its own process group.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            let _ = child.wait();
            break None;
        }
        if let Ok(tail) = shown.lock() {
            set(request.progress, &last_line(&tail));
        }
        thread::sleep(Duration::from_millis(100));
    };
    let _ = reader.join();
    let tail = shown.lock().map(|t| last_line(&t)).unwrap_or_default();
    let result = match (status, stopped) {
        (_, Some(reason)) => Err(reason.to_owned()),
        (Some(status), None) if status.success() => Ok(()),
        (_, None) if tail.is_empty() => Err("Git failed".to_owned()),
        (_, None) => Err(format!("Git: {tail}")),
    };
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&temp);
        return Err(error.into());
    }
    if let Err(error) = rename_new(&temp, &target) {
        let _ = fs::remove_dir_all(&temp);
        return Err(error);
    }
    Ok(target)
}

/// Renames `from` to `to`, failing when `to` exists.
fn rename_new(from: &Path, to: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let from_c = std::ffi::CString::new(from.as_os_str().as_bytes())?;
    let to_c = std::ffi::CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both are NUL-terminated paths that outlive the call.
    let renamed = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from_c.as_ptr(),
            libc::AT_FDCWD,
            to_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if renamed == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EEXIST) => Err(format!("{} already exists", to.display()).into()),
        // A file system without RENAME_NOREPLACE: refused rather than
        // risking a rename over a folder created meanwhile.
        Some(libc::EINVAL | libc::ENOSYS) => Err(format!(
            "the file system of {} cannot rename without replacing; clone in a terminal",
            to.display()
        )
        .into()),
        _ => Err(error.into()),
    }
}

#[cfg(test)]
mod tests;
