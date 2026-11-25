//! Project status, archive, restore and delete (ADR 0006).
//!
//! A project's `status` is `active`, `paused` (shown dimmed, nothing else)
//! or `archived`: its folder was packed as a tar.gz ([`tarball`]), uploaded
//! to Cloud Storage ([`gcs`]) with verified checksums, and removed from the
//! machine after the safe-removal check ([`removal`]); the project stays in
//! the configuration with its archive record, so it can be restored.
//!
//! # Archiving
//!
//! 1. The safe-removal check runs (Git findings are warnings: the archive
//!    keeps `.git`), and its fingerprint must equal the one the user saw.
//! 2. The folder is packed into a private temporary file, with its SHA-256
//!    and CRC32C computed while it is written.
//! 3. It is uploaded (resumable) as a new object
//!    `<prefix>/<project id>/<UTC time>.tar.gz` that must not exist, with
//!    the project's id, name, folder, KelpMux's version and the SHA-256 as
//!    metadata; Cloud Storage checks the CRC32C given at the start.
//! 4. Verification: the stored object's size and CRC32C, read back from the
//!    API, must equal the local ones, and its metadata the SHA-256.
//! 5. The check runs again and must give the same fingerprint, and no
//!    entry of the folder may have changed since packing began (status
//!    change times, entry and byte counts).
//! 6. The configuration records `status = "archived"` and the archive;
//!    only then is the folder removed ([`removal::remove`]). Any failure
//!    before keeps the folder, and a failure to remove it puts the status
//!    back.
//!
//! # Restoring
//!
//! The chosen object (the recorded one by default) is downloaded to a
//! private temporary file; its CRC32C must equal the API's, and its SHA-256
//! the object's metadata (and the record's, for the recorded object). It is
//! unpacked into a fresh private folder beside the destination, which is
//! then renamed into place (never over an existing folder): the original
//! folder, or a new one inside `projects.base`. The status becomes active.
//!
//! # Jobs
//!
//! Archive, restore and delete run as jobs: in the foreground for `kelpmux
//! project`, or as a detached `kelpmux project` process the web bridge
//! starts, so closing the browser does not stop one. At most one job per
//! project runs (an exclusive `flock` on `<state>/archive/<id>.lock`); its
//! state is `<state>/archive/<id>.json`, rewritten atomically as it
//! progresses, and a cancel request is the file `<id>.cancel`. Removal,
//! once started, cannot be cancelled.
pub mod gcs;
pub mod tarball;

use super::{
    removal::{self, Mode},
    user_config::{self, ArchiveRecord, Config, ProjectChange, ProjectStatus},
};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, OpenOptionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, String>;

/// The error of a cancelled operation.
#[derive(Debug)]
pub struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}
impl std::error::Error for Cancelled {}

/// A job's counters and its cancel flag, shared with the code doing the
/// work.
#[derive(Default)]
pub struct Progress {
    bytes: AtomicU64,
    files: AtomicU64,
    cancel: AtomicBool,
}
impl Progress {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub fn add_bytes(&self, n: u64) {
        self.bytes.fetch_add(n, Ordering::Relaxed);
    }
    pub fn set_bytes(&self, n: u64) {
        self.bytes.store(n, Ordering::Relaxed);
    }
    pub fn add_file(&self) {
        self.files.fetch_add(1, Ordering::Relaxed);
    }
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }
    pub fn files(&self) -> u64 {
        self.files.load(Ordering::Relaxed)
    }
    fn reset(&self) {
        self.bytes.store(0, Ordering::Relaxed);
        self.files.store(0, Ordering::Relaxed);
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` of Unix seconds (UTC).
pub fn utc(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// What the jobs and operations need: where the configuration, the job
/// states and the temporary archives are, and (tests) a store to use
/// instead of the configured one.
#[derive(Clone)]
pub struct Context {
    pub config_path: PathBuf,
    pub env: user_config::Env,
    /// Job states and locks.
    pub state: PathBuf,
    /// Temporary archives.
    pub cache: PathBuf,
    pub store: Option<Arc<gcs::Gcs>>,
}

fn xdg(variable: &str, fallback: &str, home: &Path) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(fallback))
}

impl Context {
    /// This user's: `config.toml`, `$XDG_STATE_HOME/kelpmux/archive` and
    /// `$XDG_CACHE_HOME/kelpmux/archive`.
    pub fn current() -> Result<Self> {
        let env = user_config::Env::current().map_err(|e| e.to_string())?;
        Ok(Self {
            config_path: user_config::default_path().map_err(|e| e.to_string())?,
            state: xdg("XDG_STATE_HOME", ".local/state", &env.home)
                .join("kelpmux")
                .join("archive"),
            cache: xdg("XDG_CACHE_HOME", ".cache", &env.home)
                .join("kelpmux")
                .join("archive"),
            env,
            store: None,
        })
    }
    /// The configuration in the file, which must be valid.
    pub fn config(&self) -> Result<Config> {
        let text = user_config::read(&self.config_path)
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        user_config::validate(&text, &self.env).map_err(|errors| {
            format!(
                "the user configuration is invalid ({}); fix it with `kelpmux config check`",
                errors
                    .iter()
                    .map(|e| e.display(&self.config_path))
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        })
    }
    fn project(&self, config: &Config, id: &str) -> Result<user_config::Project> {
        config
            .project(id)
            .cloned()
            .ok_or_else(|| format!("no project has the id `{id}`"))
    }
    fn guards(&self, config: &Config, id: &str) -> removal::Guards {
        let mut guards = removal::Guards::for_project(config, id, &self.env.home);
        guards.protected.push(self.state.clone());
        guards.protected.push(self.cache.clone());
        guards
    }
    /// The store and where archives go: `(store, bucket, prefix)`.
    fn store(&self, config: &Config) -> Result<(Arc<gcs::Gcs>, String, String)> {
        let settings = &config.archive;
        let bucket = settings.bucket.clone().ok_or(
            "archiving is off: set `archive.backend = \"gcs\"` and `archive.bucket` in config.toml",
        )?;
        let store = match &self.store {
            Some(store) => store.clone(),
            None => Arc::new(gcs::Gcs::new(token_source(settings))),
        };
        Ok((store, bucket, settings.prefix.clone()))
    }
    fn change(&self, change: &ProjectChange) -> Result<()> {
        match user_config::update_projects(&self.config_path, change, &self.env)
            .map_err(|e| e.to_string())?
        {
            Ok(()) => Ok(()),
            Err(errors) => Err(format!(
                "the configuration was not changed: {}",
                errors
                    .iter()
                    .map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join("; ")
            )),
        }
    }
    fn private_dir(path: &Path) -> Result<()> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Where the token comes from, for these settings.
pub fn token_source(settings: &user_config::ArchiveSettings) -> gcs::TokenSource {
    match &settings.token_command {
        Some(path) => gcs::TokenSource::Command(path.clone()),
        None => gcs::TokenSource::Metadata(gcs::METADATA_TOKEN.into()),
    }
}

/// `archive_status`: where archives go and where the token comes from,
/// never the token. No network.
pub fn status(config: &Config) -> Value {
    let settings = &config.archive;
    let source = token_source(settings);
    json!({
        "configured": settings.bucket.is_some(),
        "backend": settings.bucket.as_ref().map(|_| "gcs"),
        "bucket": settings.bucket,
        "prefix": settings.prefix,
        "token_source": if settings.token_command.is_some() { "command" } else { "metadata" },
        "token_label": source.label(),
        "error": if settings.bucket.is_none() {
            Some("archiving is off: set `archive.backend = \"gcs\"` and `archive.bucket` in config.toml")
        } else {
            None
        },
    })
}

/// A project id as given to an operation.
fn valid_id(id: &str) -> Result<()> {
    if user_config::id_problem(id).is_some() {
        return Err("invalid project id".into());
    }
    Ok(())
}

/// `project_check_removal` / `kelpmux project check-removal`.
pub fn check(ctx: &Context, id: &str, mode: Mode) -> Result<Value> {
    valid_id(id)?;
    let config = ctx.config()?;
    let project = ctx.project(&config, id)?;
    let check = removal::check(&project.folder.path, mode, &ctx.guards(&config, id));
    Ok(check.to_json(id))
}

/// Sets a project's status to active or paused.
pub fn set_status(ctx: &Context, id: &str, status: ProjectStatus) -> Result<()> {
    valid_id(id)?;
    let config = ctx.config()?;
    let project = ctx.project(&config, id)?;
    if status == ProjectStatus::Archived {
        return Err("a project becomes archived by archiving it".into());
    }
    if project.status == ProjectStatus::Archived && !project.folder.path.is_dir() {
        return Err("this project is archived: restore it first".into());
    }
    ctx.change(&ProjectChange::Status {
        id: id.into(),
        status,
        archive: None,
        folder: None,
    })
}

/// One job: its state file, the lock that makes it the only one of its
/// project, and the thread that writes its progress and watches for a
/// cancel request.
pub struct Job {
    project: String,
    id: String,
    kind: &'static str,
    file: PathBuf,
    cancel_file: PathBuf,
    progress: Arc<Progress>,
    state: Arc<Mutex<Value>>,
    done: Arc<AtomicBool>,
    /// Held until the job finishes (or is dropped).
    lock: Mutex<Option<File>>,
}

const LOCK_BUSY: &str = "another archive, restore or delete of this project is running";

fn try_lock(path: &Path) -> Result<Option<File>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    // SAFETY: a valid descriptor.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(Some(file))
    } else {
        Ok(None)
    }
}

/// Writes `value` to `path` atomically (temporary file, rename).
fn write_json(path: &Path, value: &Value) -> Result<()> {
    let temp = path.with_extension(format!("tmp{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    drop(file);
    fs::rename(&temp, path).map_err(|e| e.to_string())
}

impl Job {
    /// Starts a job of `kind` for `project`, or fails when one runs.
    pub fn start(
        ctx: &Context,
        project: &str,
        kind: &'static str,
        id: Option<&str>,
    ) -> Result<Arc<Self>> {
        valid_id(project)?;
        Context::private_dir(&ctx.state)?;
        let lock = try_lock(&ctx.state.join(format!("{project}.lock")))?.ok_or(LOCK_BUSY)?;
        let id = match id {
            Some(id) if id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()) => {
                id.to_owned()
            }
            Some(_) => return Err("invalid job id".into()),
            None => removal::random_hex()?,
        };
        let cancel_file = ctx.state.join(format!("{project}.cancel"));
        let _ = fs::remove_file(&cancel_file);
        let now = crate::now();
        let job = Arc::new(Self {
            project: project.into(),
            id: id.clone(),
            kind,
            file: ctx.state.join(format!("{project}.json")),
            cancel_file,
            progress: Arc::new(Progress::default()),
            state: Arc::new(Mutex::new(json!({
                "id": id,
                "project": project,
                "kind": kind,
                "state": "running",
                "phase": "starting",
                "message": "Starting",
                "bytes_done": 0,
                "bytes_total": null,
                "files_done": 0,
                "started_at": now,
                "updated_at": now,
                "error": null,
                "result": null,
                "cancellable": true,
                "pid": std::process::id(),
            }))),
            done: Arc::new(AtomicBool::new(false)),
            lock: Mutex::new(Some(lock)),
        });
        job.write();
        let watcher = Arc::downgrade(&job);
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(250));
                let Some(watcher) = watcher.upgrade() else {
                    return;
                };
                if watcher.done.load(Ordering::Acquire) {
                    return;
                }
                if let Ok(text) = fs::read_to_string(&watcher.cancel_file)
                    && text.trim() == watcher.id
                    && watcher.cancellable()
                {
                    watcher.progress.cancel();
                }
                watcher.write();
            }
        });
        Ok(job)
    }
    pub fn progress(&self) -> &Progress {
        &self.progress
    }
    fn cancellable(&self) -> bool {
        self.state
            .lock()
            .map(|s| s["cancellable"] == true)
            .unwrap_or(false)
    }
    /// The state as written, with the counters.
    fn snapshot(&self) -> Value {
        let mut state = self.state.lock().map(|s| s.clone()).unwrap_or(Value::Null);
        state["bytes_done"] = json!(self.progress.bytes());
        state["files_done"] = json!(self.progress.files());
        state["updated_at"] = json!(crate::now());
        state
    }
    fn write(&self) {
        let _ = write_json(&self.file, &self.snapshot());
    }
    /// Enters a phase: its message, the bytes it will count (when known),
    /// and whether it may still be cancelled.
    pub fn phase(&self, phase: &str, message: &str, total: Option<u64>, cancellable: bool) {
        self.progress.reset();
        if let Ok(mut state) = self.state.lock() {
            state["phase"] = json!(phase);
            state["message"] = json!(message);
            state["bytes_total"] = json!(total);
            state["cancellable"] = json!(cancellable);
        }
        self.write();
    }
    /// Ends the job with its result; returns the final state.
    pub fn finish(&self, result: &Result<Value>) -> Value {
        self.done.store(true, Ordering::Release);
        if let Ok(mut state) = self.state.lock() {
            state["cancellable"] = json!(false);
            match result {
                Ok(value) => {
                    state["state"] = json!("done");
                    state["phase"] = json!("done");
                    state["message"] = json!("Done");
                    state["result"] = value.clone();
                }
                Err(error) => {
                    state["state"] = json!(if self.progress.cancelled() {
                        "cancelled"
                    } else {
                        "failed"
                    });
                    state["error"] = json!(error);
                }
            }
        }
        let _ = fs::remove_file(&self.cancel_file);
        let snapshot = self.snapshot();
        let _ = write_json(&self.file, &snapshot);
        // Another job of the project may start now.
        if let Ok(mut lock) = self.lock.lock() {
            lock.take();
        }
        snapshot
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn kind(&self) -> &str {
        self.kind
    }
    pub fn project(&self) -> &str {
        &self.project
    }
}

/// Every project's last job (`archive_jobs`, `kelpmux project jobs`). A
/// job whose process ended without finishing shows as failed.
pub fn jobs(ctx: &Context) -> Value {
    let mut jobs = Vec::new();
    if let Ok(entries) = fs::read_dir(&ctx.state) {
        for entry in entries.flatten().take(1024) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(project) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if valid_id(project).is_err() {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.is_file() || meta.len() > 256 * 1024 {
                continue;
            }
            let Ok(mut job) = fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| serde_json::from_slice::<Value>(&b).map_err(|e| e.to_string()))
            else {
                continue;
            };
            if job["state"] == "running"
                && let Ok(Some(_free)) = try_lock(&ctx.state.join(format!("{project}.lock")))
            {
                job["state"] = json!("failed");
                job["cancellable"] = json!(false);
                job["error"] = json!("the job stopped before it finished (its process ended)");
            }
            jobs.push(job);
        }
    }
    jobs.sort_by_key(|j| std::cmp::Reverse(j["started_at"].as_u64().unwrap_or(0)));
    json!({ "jobs": jobs })
}

/// Asks the running job of `project` to stop.
pub fn cancel(ctx: &Context, project: &str) -> Result<Value> {
    valid_id(project)?;
    let path = ctx.state.join(format!("{project}.json"));
    let job: Value = fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or("no job of this project is known")?;
    if job["state"] != "running" {
        return Err("this job is not running".into());
    }
    if job["cancellable"] != true {
        return Err("this job is removing the folder and can no longer be cancelled".into());
    }
    let id = job["id"].as_str().ok_or("invalid job state")?;
    write_cancel(&ctx.state.join(format!("{project}.cancel")), id)?;
    Ok(jobs(ctx))
}

fn write_cancel(path: &Path, id: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(id.as_bytes()).map_err(|e| e.to_string())
}

/// A private temporary file for an archive in `ctx.cache`, removed when
/// dropped.
struct TempFile(PathBuf);
impl TempFile {
    fn create(ctx: &Context, project: &str) -> Result<(Self, File)> {
        Context::private_dir(&ctx.cache)?;
        let path = ctx
            .cache
            .join(format!("{project}-{}.tar.gz", removal::random_hex()?));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok((Self(path), file))
    }
}
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Removes temporary archives of `project` a job that died left behind (the
/// caller holds the project's lock).
fn clean_cache(ctx: &Context, project: &str) {
    if let Ok(entries) = fs::read_dir(&ctx.cache) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(rest) = name.strip_prefix(&format!("{project}-"))
                && rest.len() == 16 + ".tar.gz".len()
                && rest.ends_with(".tar.gz")
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// Archives project `id` (see the module documentation). `fingerprint` is
/// the safe-removal check the user confirmed.
pub fn archive(ctx: &Context, job: &Job, id: &str, fingerprint: &str) -> Result<Value> {
    let progress = job.progress();
    let config = ctx.config()?;
    let project = ctx.project(&config, id)?;
    if project.status == ProjectStatus::Archived {
        return Err("this project is archived already".into());
    }
    let (store, bucket, prefix) = ctx.store(&config)?;
    clean_cache(ctx, id);
    let folder = project.folder.path.clone();
    let guards = ctx.guards(&config, id);
    job.phase("checking", "Checking what the folder holds", None, true);
    let before = removal::check(&folder, Mode::Archive, &guards);
    if !before.exists {
        return Err(format!(
            "{} does not exist: there is nothing to archive",
            folder.display()
        ));
    }
    if !before.allowed() {
        return Err(format!(
            "the safe-removal check found blockers: {}",
            before
                .items
                .iter()
                .filter(|i| i.severity == removal::Severity::Blocker)
                .map(|i| format!("{}: {}", i.path, i.message))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if before.fingerprint() != fingerprint {
        return Err("the folder changed since it was checked; check it again".into());
    }
    // Fails early, before packing, when there is no token.
    store.token()?;
    let since = tarball::ctime_now();
    let started = crate::now();
    let (temp, file) = TempFile::create(ctx, id)?;
    job.phase(
        "packing",
        "Packing the folder",
        Some(before.other_files.bytes),
        true,
    );
    let packed = tarball::pack(&folder, file, progress)?;
    if progress.cancelled() {
        return Err(Cancelled.to_string());
    }
    let object = format!(
        "{prefix}/{id}/{}.tar.gz",
        utc(started).replace(['-', ':'], "")
    );
    let mut metadata = serde_json::Map::new();
    for (key, value) in [
        ("kelpmux-project", id.to_owned()),
        ("kelpmux-name", project.name.clone()),
        ("kelpmux-folder", project.folder.written.clone()),
        ("kelpmux-version", env!("CARGO_PKG_VERSION").to_owned()),
        ("kelpmux-entries", packed.entries.to_string()),
        ("kelpmux-bytes", packed.bytes.to_string()),
        ("sha256", packed.sha256.clone()),
    ] {
        metadata.insert(key.into(), json!(value));
    }
    job.phase(
        "uploading",
        &format!("Uploading to gs://{bucket}/{object}"),
        Some(packed.size),
        true,
    );
    let upload = File::open(&temp.0).map_err(|e| e.to_string())?;
    let stored = store.upload(
        &bucket,
        &object,
        upload,
        packed.size,
        packed.crc32c,
        metadata,
        progress,
    )?;
    job.phase("verifying", "Verifying the stored archive", None, true);
    let crc = tarball::crc32c_base64(packed.crc32c);
    let verify = |object: &gcs::Object| -> Result<()> {
        if object.size != packed.size {
            return Err(format!(
                "the stored archive has {} bytes, not {}: the folder was kept",
                object.size, packed.size
            ));
        }
        if object.crc32c != crc {
            return Err(
                "the stored archive's CRC32C differs from the local one: the folder was kept"
                    .into(),
            );
        }
        Ok(())
    };
    verify(&stored)?;
    let fresh = store
        .object(&bucket, &object)?
        .ok_or("the stored archive cannot be read back: the folder was kept")?;
    verify(&fresh)?;
    if fresh.sha256.as_deref() != Some(packed.sha256.as_str()) {
        return Err("the stored archive's SHA-256 metadata differs: the folder was kept".into());
    }
    drop(temp);
    job.phase("checking", "Checking the folder again", None, true);
    let after = removal::check(&folder, Mode::Archive, &guards);
    if after.fingerprint() != before.fingerprint() || !after.allowed() {
        return Err(format!(
            "the folder changed while it was archived; it was kept, and the archive gs://{bucket}/{object} is not recorded"
        ));
    }
    let tally = tarball::tally(&folder, since)?;
    if tally.changed_since || tally.entries != packed.entries || tally.bytes != packed.bytes {
        return Err(format!(
            "the folder changed while it was archived; it was kept, and the archive gs://{bucket}/{object} is not recorded"
        ));
    }
    if progress.cancelled() {
        return Err(Cancelled.to_string());
    }
    let record = ArchiveRecord {
        backend: "gcs".into(),
        bucket: bucket.clone(),
        object: object.clone(),
        size: packed.size,
        sha256: packed.sha256.clone(),
        crc32c: crc,
        archived_at: utc(started),
        entries: Some(packed.entries),
        unpacked_bytes: Some(packed.bytes),
    };
    job.phase(
        "removing",
        "Removing the folder from this machine",
        None,
        false,
    );
    ctx.change(&ProjectChange::Status {
        id: id.into(),
        status: ProjectStatus::Archived,
        archive: Some(Some(record.clone())),
        folder: None,
    })?;
    if let Err(error) = removal::remove(&folder, &guards) {
        // The folder (or what is left of it) stays: so does the status.
        let _ = ctx.change(&ProjectChange::Status {
            id: id.into(),
            status: project.status,
            archive: None,
            folder: None,
        });
        return Err(format!(
            "the archive gs://{bucket}/{object} is verified, but removing the folder failed: {error}"
        ));
    }
    Ok(json!({
        "archive": record.to_json(),
        "entries": packed.entries,
        "bytes": packed.bytes,
        "skipped": packed.skipped,
        "warnings": before.items.iter().filter(|i| i.severity == removal::Severity::Warning).map(|i| format!("{}: {}", i.path, i.message)).collect::<Vec<_>>(),
    }))
}

/// Whether `object` may be restored for project `id`: under
/// `<prefix>/<id>/`, or the recorded one.
fn object_allowed(object: &str, id: &str, prefix: &str, record: Option<&ArchiveRecord>) -> bool {
    let plain = !object.is_empty()
        && object.len() <= 1024
        && !object.chars().any(char::is_control)
        && !object.split('/').any(|p| p == "..");
    plain
        && (object.starts_with(&format!("{prefix}/{id}/"))
            || record.is_some_and(|r| r.object == object))
}

/// `archive_list` / `kelpmux project archives`: the project's archives,
/// newest first, with which one is recorded.
pub fn list(ctx: &Context, id: &str) -> Result<Value> {
    valid_id(id)?;
    let config = ctx.config()?;
    let project = ctx.project(&config, id)?;
    let (store, bucket, prefix) = ctx.store(&config)?;
    let mut objects = store.list(&bucket, &format!("{prefix}/{id}/"))?;
    let record = project.archive.as_ref();
    if let Some(record) = record
        && record.bucket == bucket
        && !objects.iter().any(|o| o.name == record.object)
        && let Ok(Some(object)) = store.object(&bucket, &record.object)
    {
        objects.push(object);
    }
    objects.sort_by(|a, b| b.updated.cmp(&a.updated).then(b.name.cmp(&a.name)));
    Ok(json!({
        "bucket": bucket,
        "archives": objects.iter().filter(|o| o.name.ends_with(".tar.gz")).map(|o| json!({
            "object": o.name,
            "size": o.size,
            "updated": o.updated,
            "sha256": o.sha256,
            "crc32c": o.crc32c,
            "recorded": record.is_some_and(|r| r.bucket == bucket && r.object == o.name),
        })).collect::<Vec<_>>(),
    }))
}

/// Restores project `id` from `object` (default: the recorded archive) into
/// its folder, or into `folder` (as written, inside `projects.base`) when
/// given.
pub fn restore(
    ctx: &Context,
    job: &Job,
    id: &str,
    object: Option<&str>,
    folder: Option<&str>,
) -> Result<Value> {
    let progress = job.progress();
    let config = ctx.config()?;
    let project = ctx.project(&config, id)?;
    let (store, configured_bucket, prefix) = ctx.store(&config)?;
    clean_cache(ctx, id);
    let record = project.archive.as_ref();
    let (bucket, object) = match object {
        Some(object) => (configured_bucket.clone(), object.to_owned()),
        None => {
            let record = record.ok_or("this project has no recorded archive: choose one")?;
            (record.bucket.clone(), record.object.clone())
        }
    };
    if !object_allowed(&object, id, &prefix, record) {
        return Err("this object is not an archive of this project".into());
    }
    // Where it goes.
    let (target, written) = match folder.map(str::trim).filter(|f| !f.is_empty()) {
        None => (project.folder.path.clone(), None),
        Some(written) => {
            let path = user_config::expand(written, &ctx.env.home)
                .map_err(|m| format!("the folder {m}"))?;
            let base = &config.projects_base.path;
            if !path.starts_with(base) || path == *base {
                return Err(format!(
                    "a new folder must be inside the projects folder ({})",
                    config.projects_base.path.display()
                ));
            }
            (path, Some(written.to_owned()))
        }
    };
    if fs::symlink_metadata(&target).is_ok() {
        return Err(format!(
            "{} exists; restore into a new folder (inside {})",
            target.display(),
            config.projects_base.path.display()
        ));
    }
    let change = ProjectChange::Status {
        id: id.into(),
        status: ProjectStatus::Active,
        archive: None,
        folder: written.clone(),
    };
    // The change must be valid before anything is downloaded.
    let text = user_config::read(&ctx.config_path)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    user_config::edit_projects(
        &text,
        &change,
        &user_config::Env {
            check_dirs: false,
            ..ctx.env.clone()
        },
    )
    .map_err(|errors| {
        errors
            .iter()
            .map(|e| e.message.clone())
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    job.phase("listing", "Reading the archive's details", None, true);
    let details = store
        .object(&bucket, &object)?
        .ok_or_else(|| format!("gs://{bucket}/{object} does not exist"))?;
    let expected_sha = match (record, &details.sha256) {
        (Some(r), _) if r.object == object && r.bucket == bucket => Some(r.sha256.clone()),
        (_, Some(sha)) => Some(sha.clone()),
        _ => None,
    }
    .ok_or("the archive has no SHA-256 to verify it with")?;
    if let Some(sha) = &details.sha256
        && *sha != expected_sha
    {
        return Err("the archive's SHA-256 metadata differs from the recorded one".into());
    }
    let (temp, mut file) = TempFile::create(ctx, id)?;
    job.phase(
        "downloading",
        &format!("Downloading gs://{bucket}/{object}"),
        Some(details.size),
        true,
    );
    let (sha256, crc, size) = store.download(
        &bucket,
        &object,
        &details.generation,
        details.size,
        &mut file,
        progress,
    )?;
    if size != details.size
        || tarball::crc32c_base64(crc) != details.crc32c
        || sha256 != expected_sha
    {
        return Err(
            "the downloaded archive does not match its checksums; nothing was restored".into(),
        );
    }
    drop(file);
    let parent = target
        .parent()
        .ok_or("the folder has no parent")?
        .to_path_buf();
    fs::create_dir_all(&parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let limits = restore_limits(
        record.filter(|r| r.bucket == bucket && r.object == object),
        &details,
        free_space(&parent)?,
    )?;
    let staging = parent.join(format!(".kelpmux-restoring-{}", removal::random_hex()?));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&staging)
        .map_err(|e| format!("{}: {e}", staging.display()))?;
    /// Removes the staging folder unless it was renamed into place.
    struct Staging(Option<PathBuf>);
    impl Drop for Staging {
        fn drop(&mut self) {
            if let Some(path) = self.0.take() {
                let _ = fs::remove_dir_all(path);
            }
        }
    }
    let mut staged = Staging(Some(staging.clone()));
    job.phase("extracting", "Unpacking the archive", Some(size), true);
    let unpacked = tarball::unpack(
        File::open(&temp.0).map_err(|e| e.to_string())?,
        &staging,
        limits,
        progress,
    )?;
    if progress.cancelled() {
        return Err(Cancelled.to_string());
    }
    drop(temp);
    {
        use std::os::unix::fs::PermissionsExt;
        // The folder's own mode as archived; private when unknown.
        let mode = unpacked.root_mode.unwrap_or(0o700);
        fs::set_permissions(&staging, fs::Permissions::from_mode(mode))
            .map_err(|e| e.to_string())?;
    }
    job.phase("restoring", "Moving the folder into place", None, false);
    removal::rename_new(&staging, &target)?;
    staged.0 = None;
    ctx.change(&change).map_err(|e| {
        format!(
            "the folder was restored to {}, but the project's status could not be set: {e}",
            target.display()
        )
    })?;
    Ok(json!({
        "folder": target,
        "object": object,
        "bucket": bucket,
        "entries": unpacked.entries,
        "bytes": unpacked.bytes,
    }))
}

/// Space kept free on the destination's file system after a restore.
const RESERVE: u64 = 256 * 1024 * 1024;
/// Most entries of an archive that does not say how many it holds.
const MAX_ENTRIES: u64 = 10_000_000;

/// Bytes free for this user on the file system of `path`.
fn free_space(path: &Path) -> Result<u64> {
    let text = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| "invalid folder path")?;
    // SAFETY: a valid C string and an output struct.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(text.as_ptr(), &mut stat) } != 0 {
        return Err(format!(
            "{}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

/// What unpacking may produce. The counts recorded when the archive was
/// packed (the project's record for its own object, else the object's
/// metadata) bound it exactly; an archive without them gets at most 64
/// times its size plus 1 GiB. Either way the file system must keep
/// [`RESERVE`] free.
fn restore_limits(
    record: Option<&ArchiveRecord>,
    details: &gcs::Object,
    free: u64,
) -> Result<tarball::Limits> {
    let meta = |key: &str| {
        details
            .metadata
            .get(key)
            .and_then(Value::as_str)
            .and_then(|n| n.parse::<u64>().ok())
    };
    let entries = record
        .and_then(|r| r.entries)
        .or_else(|| meta("kelpmux-entries"))
        .unwrap_or(MAX_ENTRIES)
        .min(MAX_ENTRIES);
    let room = free.saturating_sub(RESERVE);
    let bytes = match record
        .and_then(|r| r.unpacked_bytes)
        .or_else(|| meta("kelpmux-bytes"))
    {
        Some(bytes) => {
            if bytes > room {
                return Err(format!(
                    "the archive unpacks to {}, and only {} can be used on the destination's disk (keeping {} free)",
                    removal::size_text(bytes),
                    removal::size_text(room),
                    removal::size_text(RESERVE)
                ));
            }
            bytes
        }
        None => details
            .size
            .saturating_mul(64)
            .saturating_add(1024 * 1024 * 1024)
            .min(room),
    };
    Ok(tarball::Limits { entries, bytes })
}

/// Deletes project `id`'s folder after the safe-removal check, whose
/// fingerprint the user confirmed, and with the project's name typed;
/// `remove` also removes the project from the configuration.
pub fn delete(
    ctx: &Context,
    job: &Job,
    id: &str,
    fingerprint: &str,
    name: &str,
    remove: bool,
) -> Result<Value> {
    let config = ctx.config()?;
    let project = ctx.project(&config, id)?;
    if name != project.name {
        return Err("the name typed is not the project's name".into());
    }
    let folder = project.folder.path.clone();
    let guards = ctx.guards(&config, id);
    job.phase("checking", "Checking what the folder holds", None, true);
    let check = removal::check(&folder, Mode::Delete, &guards);
    if check.fingerprint() != fingerprint {
        return Err("the folder changed since it was checked; check it again".into());
    }
    if !check.allowed() {
        return Err(format!(
            "the safe-removal check found blockers: {}",
            check
                .items
                .iter()
                .filter(|i| i.severity == removal::Severity::Blocker)
                .map(|i| format!("{}: {}", i.path, i.message))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if job.progress().cancelled() {
        return Err(Cancelled.to_string());
    }
    if check.exists {
        job.phase("removing", "Deleting the folder", None, false);
        removal::remove(&folder, &guards)?;
    }
    if remove {
        ctx.change(&ProjectChange::Delete { id: id.into() })?;
    }
    Ok(json!({
        "folder": folder,
        "deleted": check.exists,
        "removed": remove,
    }))
}

/// `archive_test` / `kelpmux project archive-test`: writes and deletes a
/// small object under `<prefix>/.kelpmux-test/`.
pub fn test(ctx: &Context) -> Result<Value> {
    let config = ctx.config()?;
    let (store, bucket, prefix) = ctx.store(&config)?;
    let started = Instant::now();
    let name = format!("{prefix}/.kelpmux-test/{}.txt", removal::random_hex()?);
    let body = b"KelpMux archive test; safe to delete.\n";
    let object = store.put_small(&bucket, &name, body)?;
    if object.crc32c != tarball::crc32c_base64(tarball::crc32c(body)) {
        let _ = store.delete(&bucket, &name, Some(&object.generation));
        return Err("Cloud Storage stored the test object with another checksum".into());
    }
    store.delete(&bucket, &name, Some(&object.generation))?;
    Ok(json!({
        "ok": true,
        "bucket": bucket,
        "object": name,
        "token_label": store.source().label(),
        "elapsed_ms": started.elapsed().as_millis() as u64,
    }))
}

/// Starts `kelpmux project <args> --job ID` as a detached process (its own
/// session: closing the browser or the SSH connection does not stop it),
/// and waits until it has taken the project's lock and written its state,
/// or failed to. Returns the jobs.
pub fn spawn(ctx: &Context, project: &str, args: &[String]) -> Result<Value> {
    valid_id(project)?;
    Context::private_dir(&ctx.state)?;
    // Refuses at once when a job runs (the process checks again).
    drop(try_lock(&ctx.state.join(format!("{project}.lock")))?.ok_or(LOCK_BUSY)?);
    let job = removal::random_hex()?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut command = Command::new(exe);
    command
        .arg("project")
        .args(args)
        .arg("--job")
        .arg(&job)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("the job could not start: {e}"))?;
    let state = ctx.state.join(format!("{project}.json"));
    let deadline = Instant::now() + Duration::from_secs(5);
    let written = || {
        fs::read(&state)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .is_some_and(|v| v["id"] == job.as_str())
    };
    let result = loop {
        // The exit status first, then the state: a job that fails quickly
        // has written its state by the time it exits.
        let exited = child.try_wait();
        if written() {
            break Ok(());
        }
        if let Ok(Some(status)) = exited {
            break Err(match status.code() {
                Some(75) => LOCK_BUSY.to_owned(),
                _ => "the job ended before it started; run `kelpmux project` in a terminal to see why".to_owned(),
            });
        }
        if Instant::now() >= deadline {
            break Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    };
    // Reaps it when it ends.
    thread::spawn(move || {
        let _ = child.wait();
    });
    result?;
    Ok(jobs(ctx))
}

/// The JSON a command prints: its value, or `{"error": …}`.
fn print(result: Result<Value>) -> crate::Result<()> {
    match result {
        Ok(value) => {
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
        Err(error) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({ "error": error }))?
            );
            std::process::exit(1);
        }
    }
}

const USAGE: &str = "usage: kelpmux project status ID [active|paused]
       kelpmux project check-removal ID [--mode archive|delete]
       kelpmux project archive ID --fingerprint F
       kelpmux project archives ID
       kelpmux project restore ID [--object OBJECT] [--folder FOLDER]
       kelpmux project delete ID --fingerprint F --name NAME [--remove]
       kelpmux project delete ID --keep-folder
       kelpmux project jobs | cancel ID | archive-test";

/// Runs a job in this process, printing its final state; exits 75 when
/// another job of the project runs.
fn run_job(
    ctx: &Context,
    project: &str,
    kind: &'static str,
    job: Option<&str>,
    work: impl FnOnce(&Job) -> Result<Value>,
) -> crate::Result<()> {
    let job = match Job::start(ctx, project, kind, job) {
        Ok(job) => job,
        Err(error) if error == LOCK_BUSY => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({ "error": error }))?
            );
            std::process::exit(75);
        }
        Err(error) => return print(Err(error)),
    };
    let result = work(&job);
    let state = job.finish(&result);
    match result {
        Ok(_) => print(Ok(state)),
        Err(_) => {
            println!("{}", serde_json::to_string_pretty(&state)?);
            std::process::exit(1);
        }
    }
}

/// `kelpmux project …`: JSON on standard output, for people and agents.
pub fn cli(args: &[String]) -> crate::Result<()> {
    let Some(verb) = args.first().map(String::as_str) else {
        return Err(USAGE.into());
    };
    if matches!(verb, "--help" | "-h" | "help") {
        println!("{USAGE}");
        return Ok(());
    }
    let mut positional = Vec::new();
    let mut options: std::collections::HashMap<&str, String> = Default::default();
    let mut flags = std::collections::HashSet::new();
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            name @ ("--mode" | "--fingerprint" | "--object" | "--folder" | "--name" | "--job") => {
                let value = rest.next().ok_or_else(|| format!("{name} needs a value"))?;
                options.insert(&name[2..], value.clone());
            }
            flag @ ("--remove" | "--keep-folder") => {
                flags.insert(&flag[2..]);
            }
            other if !other.starts_with("--") => positional.push(other.to_owned()),
            other => return Err(format!("unknown option {other}\n{USAGE}").into()),
        }
    }
    let ctx = Context::current()?;
    let id =
        || -> crate::Result<String> { positional.first().cloned().ok_or_else(|| USAGE.into()) };
    let option = |name: &str| options.get(name).cloned();
    let job = option("job");
    match verb {
        "status" => {
            let id = id()?;
            if let Some(status) = positional.get(1) {
                let status = ProjectStatus::parse(status)
                    .filter(|s| *s != ProjectStatus::Archived)
                    .ok_or("the status is active or paused")?;
                if let Err(error) = set_status(&ctx, &id, status) {
                    return print(Err(error));
                }
            }
            let config = ctx.config()?;
            let project = ctx.project(&config, &id)?;
            print(Ok(json!({
                "id": project.id,
                "name": project.name,
                "folder": project.folder.path,
                "missing": !project.folder.path.is_dir(),
                "status": project.status.as_str(),
                "archive": project.archive.as_ref().map(ArchiveRecord::to_json),
            })))
        }
        "check-removal" => {
            let mode = match option("mode") {
                None => Mode::Delete,
                Some(mode) => Mode::parse(&mode).ok_or("--mode is archive or delete")?,
            };
            print(check(&ctx, &id()?, mode))
        }
        "archive" => {
            let id = id()?;
            let fingerprint = option("fingerprint").ok_or(
                "--fingerprint required: run `kelpmux project check-removal ID --mode archive` first and pass its fingerprint",
            )?;
            run_job(&ctx, &id, "archive", job.as_deref(), |job| {
                archive(&ctx, job, &id, &fingerprint)
            })
        }
        "archives" => print(list(&ctx, &id()?)),
        "restore" => {
            let id = id()?;
            let (object, folder) = (option("object"), option("folder"));
            run_job(&ctx, &id, "restore", job.as_deref(), |job| {
                restore(&ctx, job, &id, object.as_deref(), folder.as_deref())
            })
        }
        "delete" if flags.contains("keep-folder") => {
            let id = id()?;
            valid_id(&id)?;
            print(
                ctx.change(&ProjectChange::Delete { id: id.clone() })
                    .map(|()| json!({ "removed": true, "folder_kept": true, "project": id })),
            )
        }
        "delete" => {
            let id = id()?;
            let fingerprint = option("fingerprint").ok_or(
                "--fingerprint required: run `kelpmux project check-removal ID` first and pass its fingerprint",
            )?;
            let name = option("name").ok_or("--name required: the project's exact name")?;
            let remove = flags.contains("remove");
            run_job(&ctx, &id, "delete", job.as_deref(), |job| {
                delete(&ctx, job, &id, &fingerprint, &name, remove)
            })
        }
        "jobs" => print(Ok(jobs(&ctx))),
        "cancel" => print(cancel(&ctx, &id()?)),
        "archive-test" => print(test(&ctx)),
        _ => Err(USAGE.into()),
    }
}

#[cfg(test)]
mod tests;
