//! File uploads over the pane stream (capability `uploads`).
//!
//! A client that drops files onto a terminal sends them in batches: one
//! `upload_begin` declares the files and their sizes, sequential
//! `upload_chunk`s carry base64 data, and `upload_finish` publishes the files
//! and returns their absolute paths, which the client pastes into the pane.
//! The files land on the host where the server and its shells run.
//!
//! Every batch is a new private (0700) directory under the upload root
//! (`$XDG_CACHE_HOME/kelpserver/uploads` by default) or under a client-chosen
//! existing directory owned by the server's user inside a file root
//! (`files::Root`: the primary or an extra root, resolved beneath that root
//! like every file operation). The
//! upload root itself is server configuration and need not lie inside the
//! file root. Files are written to private
//! temporary names (0600, `O_EXCL`, `O_NOFOLLOW`), synced and renamed into
//! place without overwriting. All file operations are relative to directory
//! descriptors opened without following a symlink at the final component.
//!
//! A batch belongs to its connection: a disconnect, a cancel or any failure
//! removes it with its partial files. Completed batches are recorded in a
//! registry in the server's state directory (path, device and inode) and are
//! deleted after the retention period, at startup and hourly. Only registered
//! batch directories whose device and inode still match are ever deleted;
//! deletion never follows symlinks.
use crate::Result;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::{CStr, CString, OsStr},
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

/// Most files in one batch.
pub const MAX_FILES: usize = 64;
/// Largest declared file.
pub const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;
/// Largest declared batch (sum of its file sizes).
pub const MAX_BATCH_BYTES: u64 = 1024 * 1024 * 1024;
/// Largest decoded `upload_chunk` payload. Base64 makes a chunk record about
/// 342 KiB, far below the 16 MiB record limit and short enough not to delay
/// pane updates queued behind it.
pub const MAX_CHUNK_BYTES: usize = 256 * 1024;
/// Chunks a client may send before the `upload_progress` acknowledging the
/// oldest one arrives. Every chunk produces one acknowledgement; the server
/// queues at most 32 reliable messages per connection, so a client ignoring
/// the window risks disconnection as a slow peer.
pub const MAX_CHUNKS_IN_FLIGHT: usize = 8;
/// Unfinished batches per connection.
pub const MAX_BATCHES: usize = 4;
/// Unfinished batches of the whole server.
pub const MAX_SERVER_BATCHES: usize = 16;
/// Longest stored file name, in bytes (the extension is kept).
pub const MAX_NAME_BYTES: usize = 200;
/// Days a completed batch is kept by default.
pub const DEFAULT_RETENTION_DAYS: u32 = 7;
/// Longest accepted retention.
pub const MAX_RETENTION_DAYS: u32 = 3650;
/// Registered batches (completed and unfinished) the server tracks.
const MAX_REGISTERED: usize = 4096;
const MAX_REGISTRY_BYTES: u64 = 4 * 1024 * 1024;
const SWEEP_INTERVAL: Duration = Duration::from_secs(3600);
/// Recently ended batches whose in-flight chunks are dropped silently.
const MAX_ENDED: usize = 64;
const REGISTRY: &str = "uploads.json";

/// Upload bounds advertised in Welcome `upload_limits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub max_files: usize,
    pub max_file_bytes: u64,
    pub max_batch_bytes: u64,
    pub max_chunk_bytes: usize,
    pub max_chunks_in_flight: usize,
    pub max_batches: usize,
}
pub const LIMITS: Limits = Limits {
    max_files: MAX_FILES,
    max_file_bytes: MAX_FILE_BYTES,
    max_batch_bytes: MAX_BATCH_BYTES,
    max_chunk_bytes: MAX_CHUNK_BYTES,
    max_chunks_in_flight: MAX_CHUNKS_IN_FLIGHT,
    max_batches: MAX_BATCHES,
};

/// One declared file of `upload_begin`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSpec {
    /// The client's file name; only a sanitized base name is used.
    pub name: String,
    /// Exact size in bytes.
    pub size: u64,
}

/// Server configuration of uploads (`--upload-dir`, `--upload-retention-days`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// Root of the batch directories. `None` is `$XDG_CACHE_HOME/kelpserver/uploads`
    /// (or `~/.cache/kelpserver/uploads`).
    pub dir: Option<PathBuf>,
    /// Days completed batches are kept. `None` is seven days; zero keeps them.
    pub retention_days: Option<u32>,
}

/// The default upload root.
pub fn default_dir() -> Result<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .filter(|path| path.is_absolute())
        .map(|cache| cache.join("kelpserver/uploads"))
        .ok_or_else(|| "neither XDG_CACHE_HOME nor HOME is an absolute path".into())
}

/// A safe base name: no directory, NUL, control or bidirectional formatting
/// characters, no leading dot (so never `.` or `..` or a hidden file), at
/// most `MAX_NAME_BYTES` bytes with the extension kept.
pub fn sanitize_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim_start();
    if cleaned.is_empty() {
        return "upload".into();
    }
    bounded(cleaned)
}
/// The stem and extension (with its dot) of a name; extensions are short and
/// never the whole name.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(index) if index > 0 && name.len() - index <= 32 => name.split_at(index),
        _ => (name, ""),
    }
}
fn bounded(name: &str) -> String {
    if name.len() <= MAX_NAME_BYTES {
        return name.into();
    }
    let (stem, extension) = split_extension(name);
    let mut keep = MAX_NAME_BYTES - extension.len();
    while !stem.is_char_boundary(keep) {
        keep -= 1;
    }
    format!("{}{extension}", &stem[..keep])
}
fn unique_name(name: String, taken: &BTreeSet<String>) -> String {
    if !taken.contains(&name) {
        return name;
    }
    let (stem, extension) = split_extension(&name);
    (2..)
        .map(|n| {
            let suffix = format!("-{n}{extension}");
            let mut keep = stem.len().min(MAX_NAME_BYTES - suffix.len());
            while !stem.is_char_boundary(keep) {
                keep -= 1;
            }
            format!("{}{suffix}", &stem[..keep])
        })
        .find(|candidate| !taken.contains(candidate))
        .unwrap()
}

fn os_error() -> std::io::Error {
    std::io::Error::last_os_error()
}
fn cstring(name: &OsStr) -> Result<CString> {
    Ok(CString::new(name.as_bytes()).map_err(|_| "path contains NUL")?)
}
fn check(result: libc::c_int) -> std::io::Result<libc::c_int> {
    if result < 0 {
        Err(os_error())
    } else {
        Ok(result)
    }
}
const DIRECTORY: libc::c_int =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
/// Open a directory; a symlink at the final component fails (`ELOOP`).
fn open_dir(path: &Path) -> std::io::Result<OwnedFd> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let fd = check(unsafe { libc::open(path.as_ptr(), DIRECTORY) })?;
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
fn open_dir_at(dir: RawFd, name: &CStr) -> std::io::Result<OwnedFd> {
    let fd = check(unsafe { libc::openat(dir, name.as_ptr(), DIRECTORY) })?;
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
fn stat(fd: RawFd) -> std::io::Result<libc::stat> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    check(unsafe { libc::fstat(fd, &mut st) })?;
    Ok(st)
}
fn owned_directory(fd: &OwnedFd) -> Result<libc::stat> {
    let st = stat(fd.as_raw_fd())?;
    if st.st_mode & libc::S_IFMT != libc::S_IFDIR || st.st_uid != unsafe { libc::geteuid() } {
        return Err("not a directory owned by this user".into());
    }
    Ok(st)
}
/// An existing directory owned by this user, opened without following a
/// symlink at the final component.
fn owned_directory_at(path: &Path) -> Option<OwnedFd> {
    let fd = open_dir(path).ok()?;
    owned_directory(&fd).ok()?;
    Some(fd)
}
fn random_hex() -> Result<String> {
    let mut bytes = [0u8; 8];
    if unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), 0) } != bytes.len() as isize
    {
        return Err(os_error().into());
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Remove every entry of a directory without following symlinks.
fn clear_directory(dir: RawFd, depth: usize) -> Result<()> {
    if depth > 32 {
        return Err("upload directory is nested too deeply".into());
    }
    let mut names = Vec::new();
    {
        let copy = check(unsafe { libc::fcntl(dir, libc::F_DUPFD_CLOEXEC, 0) })?;
        let stream = unsafe { libc::fdopendir(copy) };
        if stream.is_null() {
            let error = os_error();
            unsafe { libc::close(copy) };
            return Err(error.into());
        }
        unsafe { libc::rewinddir(stream) };
        loop {
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            if name.to_bytes() != b"." && name.to_bytes() != b".." {
                names.push(name.to_owned());
            }
        }
        unsafe { libc::closedir(stream) };
    }
    for name in names {
        if unsafe { libc::unlinkat(dir, name.as_ptr(), 0) } == 0 {
            continue;
        }
        let error = os_error();
        match error.raw_os_error() {
            Some(libc::EISDIR) | Some(libc::EPERM) => {
                let child = open_dir_at(dir, &name)?;
                clear_directory(child.as_raw_fd(), depth + 1)?;
                check(unsafe { libc::unlinkat(dir, name.as_ptr(), libc::AT_REMOVEDIR) })?;
            }
            Some(libc::ENOENT) => {}
            _ => return Err(error.into()),
        }
    }
    Ok(())
}
/// Delete a batch directory if it is still the registered one. `Ok(false)`
/// means it is gone or no longer ours (missing, replaced, a symlink); nothing
/// was deleted then.
fn remove_batch(path: &Path, dev: u64, ino: u64) -> Result<bool> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Ok(false);
    };
    let name = cstring(name)?;
    let not_ours = |error: &std::io::Error| {
        matches!(
            error.raw_os_error(),
            Some(libc::ENOENT | libc::ELOOP | libc::ENOTDIR)
        )
    };
    let parent = match open_dir(parent) {
        Ok(fd) => fd,
        Err(error) if not_ours(&error) => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let dir = match open_dir_at(parent.as_raw_fd(), &name) {
        Ok(fd) => fd,
        Err(error) if not_ours(&error) => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let st = stat(dir.as_raw_fd())?;
    if st.st_dev != dev || st.st_ino != ino || st.st_uid != unsafe { libc::geteuid() } {
        return Ok(false);
    }
    clear_directory(dir.as_raw_fd(), 0)?;
    drop(dir);
    check(unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) })?;
    Ok(true)
}

/// One registered batch directory (`<state>/uploads.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registered {
    pub path: PathBuf,
    pub dev: u64,
    pub ino: u64,
    /// Unix seconds.
    pub created_at: u64,
    /// Unix seconds; absent while the batch is unfinished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<u64>,
}
fn load_registry(path: &Path) -> Result<Vec<Registered>> {
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err("upload registry must be a private regular file owned by this user".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_REGISTRY_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_REGISTRY_BYTES {
        return Err("upload registry is too large".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

struct Upload {
    name: String,
    size: u64,
    received: u64,
    temp: CString,
}
struct Batch {
    client: u64,
    path: PathBuf,
    dir: OwnedFd,
    dev: u64,
    ino: u64,
    files: Vec<Upload>,
    /// The file being written; files are sent in order.
    current: Option<(usize, File)>,
}
impl Batch {
    /// The index of the first file still missing bytes.
    fn expected(&self) -> Option<usize> {
        self.files.iter().position(|file| file.received < file.size)
    }
    fn create(&self, index: usize) -> Result<File> {
        let fd = check(unsafe {
            libc::openat(
                self.dir.as_raw_fd(),
                self.files[index].temp.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600 as libc::c_uint,
            )
        })?;
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    fn write(&mut self, index: usize, offset: u64, data: &str) -> Result<u64> {
        if self.expected() != Some(index) {
            return Err("upload chunk is out of order: files are sent in order".into());
        }
        let file = &self.files[index];
        if offset != file.received {
            return Err(format!(
                "upload chunk offset {offset} does not continue file {index} at {}",
                file.received
            )
            .into());
        }
        if data.is_empty() || data.len() > MAX_CHUNK_BYTES.div_ceil(3) * 4 {
            return Err("upload chunk must hold 1 to max_chunk_bytes bytes".into());
        }
        let bytes = STANDARD
            .decode(data)
            .map_err(|_| "upload chunk data is not valid base64")?;
        if bytes.is_empty() || bytes.len() > MAX_CHUNK_BYTES {
            return Err("upload chunk must hold 1 to max_chunk_bytes bytes".into());
        }
        if file.received + bytes.len() as u64 > file.size {
            return Err(format!("upload exceeds the declared size of file {index}").into());
        }
        if self.current.as_ref().is_none_or(|(open, _)| *open != index) {
            self.current = Some((index, self.create(index)?));
        }
        let (_, handle) = self.current.as_mut().unwrap();
        handle.write_all(&bytes)?;
        let file = &mut self.files[index];
        file.received += bytes.len() as u64;
        if file.received == file.size {
            let (_, handle) = self.current.take().unwrap();
            handle.sync_all()?;
        }
        Ok(file.received)
    }
    /// Sync, then publish every file under its final name without replacing
    /// anything.
    fn publish(&mut self) -> Result<Vec<PathBuf>> {
        if let Some(index) = self.expected() {
            let file = &self.files[index];
            return Err(format!(
                "upload is incomplete: file {index} has {} of {} bytes",
                file.received, file.size
            )
            .into());
        }
        for index in 0..self.files.len() {
            if self.files[index].size == 0 {
                self.create(index)?.sync_all()?;
            }
        }
        let dir = self.dir.as_raw_fd();
        for file in &self.files {
            let target = cstring(OsStr::new(&file.name))?;
            let renamed = unsafe {
                libc::renameat2(
                    dir,
                    file.temp.as_ptr(),
                    dir,
                    target.as_ptr(),
                    libc::RENAME_NOREPLACE as _,
                )
            };
            if renamed != 0 {
                let error = os_error();
                if !matches!(error.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS)) {
                    return Err(error.into());
                }
                // A file system without RENAME_NOREPLACE: a hard link never replaces either.
                check(unsafe { libc::linkat(dir, file.temp.as_ptr(), dir, target.as_ptr(), 0) })?;
                check(unsafe { libc::unlinkat(dir, file.temp.as_ptr(), 0) })?;
            }
        }
        check(unsafe { libc::fsync(dir) })?;
        Ok(self
            .files
            .iter()
            .map(|file| self.path.join(&file.name))
            .collect())
    }
}

/// The outcome of one `upload_chunk`.
#[derive(Debug, PartialEq, Eq)]
pub enum Chunk {
    /// Bytes received so far for the file (`upload_progress`).
    Accepted(u64),
    /// A chunk still in flight when its batch ended; dropped silently.
    Ignored,
    /// The batch failed and was removed (`upload_failed`).
    Failed(String),
}

/// The server's uploads: unfinished batches per connection and the registry.
pub struct Uploads {
    root: Option<PathBuf>,
    retention_days: u32,
    registry_path: PathBuf,
    registry: Vec<Registered>,
    batches: BTreeMap<u64, Batch>,
    /// Batches that ended (cancelled or failed), with their connection.
    ended: VecDeque<(u64, u64)>,
    next: u64,
    swept: Instant,
}
impl Uploads {
    /// Load the registry from the state directory. A damaged registry is
    /// reported and replaced; the batches it named are then kept forever
    /// rather than risk deleting anything else.
    pub fn new(state_dir: &Path, settings: &Settings) -> Result<Self> {
        let retention_days = settings.retention_days.unwrap_or(DEFAULT_RETENTION_DAYS);
        if retention_days > MAX_RETENTION_DAYS {
            return Err("upload retention must be at most 3650 days".into());
        }
        if settings.dir.as_ref().is_some_and(|dir| !dir.is_absolute()) {
            return Err("upload directory must be an absolute path".into());
        }
        let registry_path = state_dir.join(REGISTRY);
        let registry = load_registry(&registry_path).unwrap_or_else(|error| {
            eprintln!("kelpserver: ignoring the upload registry: {error}");
            Vec::new()
        });
        Ok(Self {
            root: settings.dir.clone(),
            retention_days,
            registry_path,
            registry,
            batches: BTreeMap::new(),
            ended: VecDeque::new(),
            next: 1,
            swept: Instant::now(),
        })
    }
    fn save(&self) -> Result<()> {
        crate::journal::atomic(&self.registry_path, &self.registry)
    }
    /// Delete expired completed batches and unfinished batches no connection
    /// is writing (left by a previous run). Called at startup and hourly.
    pub fn sweep(&mut self, now: u64) {
        self.swept = Instant::now();
        let before = self.registry.clone();
        let retention = u64::from(self.retention_days) * 86_400;
        let active: BTreeSet<(u64, u64)> = self
            .batches
            .values()
            .map(|batch| (batch.dev, batch.ino))
            .collect();
        self.registry.retain(|entry| {
            let expired = match entry.completed_at {
                Some(at) => self.retention_days > 0 && now.saturating_sub(at) >= retention,
                None => !active.contains(&(entry.dev, entry.ino)),
            };
            if !expired {
                return true;
            }
            match remove_batch(&entry.path, entry.dev, entry.ino) {
                Ok(_) => false,
                Err(error) => {
                    eprintln!(
                        "kelpserver: could not remove upload batch {}: {error}",
                        entry.path.display()
                    );
                    true
                }
            }
        });
        if self.registry != before {
            if let Err(error) = self.save() {
                eprintln!("kelpserver: upload registry: {error}");
            }
        }
    }
    /// Sweep if the last sweep is an hour old.
    pub fn sweep_due(&mut self) {
        if self.swept.elapsed() >= SWEEP_INTERVAL {
            self.sweep(crate::now());
        }
    }
    /// Start a batch for a connection; returns its ID and directory. A
    /// `dest` must lie inside `files_root`, the server's file root.
    pub fn begin(
        &mut self,
        client: u64,
        files: &[FileSpec],
        dest: Option<&Path>,
        files_root: Option<&super::files::Root>,
    ) -> Result<(u64, PathBuf)> {
        if files.is_empty() || files.len() > MAX_FILES {
            return Err(format!("an upload holds 1 to {MAX_FILES} files").into());
        }
        if files.iter().any(|file| file.size > MAX_FILE_BYTES) {
            return Err(format!("an uploaded file is limited to {MAX_FILE_BYTES} bytes").into());
        }
        let total: u64 = files.iter().map(|file| file.size).sum();
        if total > MAX_BATCH_BYTES {
            return Err(format!("an upload is limited to {MAX_BATCH_BYTES} bytes").into());
        }
        if files.iter().any(|file| file.name.len() > 4096) {
            return Err("upload file name is too long".into());
        }
        if self.batches.values().filter(|b| b.client == client).count() >= MAX_BATCHES {
            return Err(format!("at most {MAX_BATCHES} unfinished uploads per connection").into());
        }
        if self.batches.len() >= MAX_SERVER_BATCHES {
            return Err("too many unfinished uploads on this server".into());
        }
        if self.registry.len() >= MAX_REGISTERED {
            return Err("too many retained upload batches; lower the upload retention".into());
        }
        let (root_path, root) = match dest {
            Some(dest) => {
                if !dest.is_absolute()
                    || dest
                        .components()
                        .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
                {
                    return Err("upload destination must be an absolute normalized path".into());
                }
                // A client-chosen destination is confined like every file
                // operation: inside a file root, opened beneath that root.
                let files_root = files_root.ok_or_else(|| super::files::no_root().message)?;
                let dest = files_root.confine(dest).map_err(|_| {
                    let roots: Vec<_> = files_root
                        .paths()
                        .map(|path| path.display().to_string())
                        .collect();
                    format!(
                        "upload destination is outside the file root{} {}",
                        if roots.len() > 1 { "s" } else { "" },
                        roots.join(", ")
                    )
                })?;
                let fd = super::files::open_path(
                    files_root,
                    &dest,
                    libc::O_RDONLY | libc::O_DIRECTORY,
                    false,
                )
                .ok()
                .filter(|fd| owned_directory(fd).is_ok())
                .ok_or(
                    "upload destination must be an existing directory owned by this user, \
                     not a symlink",
                )?;
                (dest, fd)
            }
            None => {
                let (path, private) = match &self.root {
                    Some(dir) => (dir.clone(), false),
                    None => (default_dir()?, true),
                };
                fs::create_dir_all(&path)?;
                let fd = owned_directory_at(&path).ok_or(
                    "upload directory must be a directory owned by this user, not a symlink",
                )?;
                if private {
                    check(unsafe { libc::fchmod(fd.as_raw_fd(), 0o700) })?;
                }
                (path, fd)
            }
        };
        let mut space: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatvfs(root.as_raw_fd(), &mut space) } == 0
            && (space.f_bavail as u64).saturating_mul(space.f_frsize as u64) < total
        {
            return Err("not enough free space for this upload".into());
        }
        let mut created = None;
        for _ in 0..8 {
            let name = format!("{}-{}", crate::now(), random_hex()?);
            let cname = cstring(OsStr::new(&name))?;
            if unsafe { libc::mkdirat(root.as_raw_fd(), cname.as_ptr(), 0o700) } == 0 {
                created = Some((name, cname));
                break;
            }
            let error = os_error();
            if error.raw_os_error() != Some(libc::EEXIST) {
                return Err(error.into());
            }
        }
        let (name, cname) = created.ok_or("could not create a unique upload directory")?;
        let path = root_path.join(&name);
        let dir = open_dir_at(root.as_raw_fd(), &cname)?;
        let st = owned_directory(&dir)?;
        check(unsafe { libc::fchmod(dir.as_raw_fd(), 0o700) })?;
        let mut taken = BTreeSet::new();
        let uploads = files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let name = unique_name(sanitize_name(&file.name), &taken);
                taken.insert(name.clone());
                Upload {
                    name,
                    size: file.size,
                    received: 0,
                    temp: CString::new(format!(".kelpserver-upload-{index}.part")).unwrap(),
                }
            })
            .collect();
        self.registry.push(Registered {
            path: path.clone(),
            dev: st.st_dev,
            ino: st.st_ino,
            created_at: crate::now(),
            completed_at: None,
        });
        if let Err(error) = self.save() {
            self.registry.pop();
            let _ = check(unsafe {
                libc::unlinkat(root.as_raw_fd(), cname.as_ptr(), libc::AT_REMOVEDIR)
            });
            return Err(error);
        }
        let id = self.next;
        self.next += 1;
        self.batches.insert(
            id,
            Batch {
                client,
                path: path.clone(),
                dir,
                dev: st.st_dev,
                ino: st.st_ino,
                files: uploads,
                current: None,
            },
        );
        Ok((id, path))
    }
    /// Remove an unfinished batch, its files and its registry entry.
    fn abort(&mut self, id: u64) {
        let Some(batch) = self.batches.remove(&id) else {
            return;
        };
        if self.ended.len() >= MAX_ENDED {
            self.ended.pop_front();
        }
        self.ended.push_back((batch.client, id));
        let Batch {
            path,
            dev,
            ino,
            dir,
            current,
            ..
        } = batch;
        drop(current);
        drop(dir);
        match remove_batch(&path, dev, ino) {
            Ok(_) => {
                self.registry
                    .retain(|entry| entry.dev != dev || entry.ino != ino || entry.path != path);
                if let Err(error) = self.save() {
                    eprintln!("kelpserver: upload registry: {error}");
                }
            }
            // The entry stays; the next sweep retries.
            Err(error) => eprintln!(
                "kelpserver: could not remove upload batch {}: {error}",
                path.display()
            ),
        }
    }
    fn owned(&self, client: u64, id: u64) -> bool {
        self.batches
            .get(&id)
            .is_some_and(|batch| batch.client == client)
    }
    fn ended(&self, client: u64, id: u64) -> bool {
        self.ended.contains(&(client, id))
    }
    pub fn chunk(&mut self, client: u64, id: u64, index: usize, offset: u64, data: &str) -> Chunk {
        if !self.owned(client, id) {
            return if self.ended(client, id) {
                Chunk::Ignored
            } else {
                Chunk::Failed("unknown upload batch".into())
            };
        }
        let batch = self.batches.get_mut(&id).unwrap();
        if index >= batch.files.len() {
            self.abort(id);
            return Chunk::Failed("upload chunk names an unknown file".into());
        }
        match batch.write(index, offset, data) {
            Ok(received) => Chunk::Accepted(received),
            Err(error) => {
                self.abort(id);
                Chunk::Failed(error.to_string())
            }
        }
    }
    /// Publish a complete batch. Any failure removes the batch.
    pub fn finish(&mut self, client: u64, id: u64) -> Result<Vec<PathBuf>> {
        if !self.owned(client, id) {
            return Err("unknown upload batch".into());
        }
        let result = self.batches.get_mut(&id).unwrap().publish();
        let paths = match result {
            Ok(paths) => paths,
            Err(error) => {
                self.abort(id);
                return Err(error);
            }
        };
        let batch = self.batches.remove(&id).unwrap();
        if let Some(entry) = self.registry.iter_mut().find(|entry| {
            entry.dev == batch.dev && entry.ino == batch.ino && entry.path == batch.path
        }) {
            entry.completed_at = Some(crate::now());
        }
        if let Err(error) = self.save() {
            eprintln!("kelpserver: upload registry: {error}");
        }
        Ok(paths)
    }
    /// Remove a batch of this connection. Cancelling a batch that already
    /// ended (failed or cancelled) succeeds.
    pub fn cancel(&mut self, client: u64, id: u64) -> Result<()> {
        if self.owned(client, id) {
            self.abort(id);
            Ok(())
        } else if self.ended(client, id) {
            Ok(())
        } else {
            Err("unknown upload batch".into())
        }
    }
    /// A connection ended: remove its unfinished batches.
    pub fn disconnect(&mut self, client: u64) {
        let ids: Vec<u64> = self
            .batches
            .iter()
            .filter(|(_, batch)| batch.client == client)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.abort(id);
        }
        self.ended.retain(|(owner, _)| *owner != client);
    }
}
impl Drop for Uploads {
    fn drop(&mut self) {
        let ids: Vec<u64> = self.batches.keys().copied().collect();
        for id in ids {
            self.abort(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_are_sanitized_and_bounded() {
        assert_eq!(sanitize_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_name("C:\\Users\\me\\photo.png"), "photo.png");
        assert_eq!(sanitize_name(".."), "upload");
        assert_eq!(sanitize_name("."), "upload");
        assert_eq!(sanitize_name(""), "upload");
        assert_eq!(sanitize_name(".bashrc"), "bashrc");
        assert_eq!(sanitize_name("a\0b\u{7}c\u{202e}d.txt"), "abcd.txt");
        let long = format!("{}.jpeg", "é".repeat(300));
        let bounded = sanitize_name(&long);
        assert!(bounded.len() <= MAX_NAME_BYTES);
        assert!(bounded.ends_with(".jpeg"));
        let mut taken = BTreeSet::new();
        taken.insert("a.txt".to_owned());
        taken.insert("a-2.txt".to_owned());
        assert_eq!(unique_name("a.txt".into(), &taken), "a-3.txt");
    }
}
