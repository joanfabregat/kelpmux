//! File downloads over the pane stream (capability `file-downloads`, an
//! addition to `files`): a regular file's bytes, or files and folders as one
//! gzip-compressed tar archive, streamed to the client in acknowledged
//! chunks, the reverse of uploads.
//!
//! A download runs on its own worker thread, which reads the files and
//! produces chunks into a bounded channel; the event loop takes a chunk only
//! while the client has fewer than [`MAX_CHUNKS_IN_FLIGHT`] unacknowledged
//! ones and its output queue has room (see `poll_downloads` in the server),
//! so a slow client stalls its download rather than filling the server's
//! memory or output queue. Nothing is built whole in memory or on disk.
//!
//! The same care as the other file operations applies: every selected path
//! must lie inside a file root (the primary or an extra root) and is opened
//! beneath that root (`outside_root` otherwise, a root itself is not
//! downloaded); a single file is opened
//! like `file_read` opens it (the type checked first, never a device or
//! FIFO); an archive walks its folders through directory descriptors
//! opened with `O_NOFOLLOW`, stores symlinks as links without following
//! them, opens each file with `O_NOFOLLOW`, never enters another file
//! system (an opened folder must be the listed one, on the walk's device),
//! and skips devices, FIFOs and sockets. Depth, entry count, input bytes,
//! the names a walk lists and the names it holds are bounded, and a walk
//! stops at its next name once cancelled: an archive over a bound is refused with
//! `too_large` before its first byte when the planning walk sees it, and
//! fails the same way if the tree grows past it while streaming.
//!
//! Archive format: POSIX ustar headers, with a pax extended header for any
//! name or link target longer than 100 bytes or not ASCII, a size of 8 GiB
//! or more, or a modification time before 1970 or after 2242. Entries carry
//! permission bits (`mode & 0o777`) and the modification time, owner and
//! group 0 without names: the archive does not disclose the server's
//! account names, and an extracting user owns what they extract.
use super::files::{self, Code, Error, Outcome};
use flate2::{Compression, write::GzEncoder};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{CStr, OsStr},
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, OwnedFd, RawFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
};

/// Bytes per chunk (decoded). The base64 record is about 342 KiB, like an
/// upload chunk, so a chunk never delays pane updates for long.
pub const CHUNK_BYTES: usize = 256 * 1024;
/// Chunks sent and not yet acknowledged, per download.
pub const MAX_CHUNKS_IN_FLIGHT: usize = 8;
/// Unfinished downloads per connection.
pub const MAX_DOWNLOADS: usize = 2;
/// Download threads of the whole server (including cancelled downloads
/// whose thread has not returned yet).
pub const MAX_SERVER_DOWNLOADS: usize = 8;
/// Most entries (files, folders and symlinks) in one archive.
pub const MAX_ARCHIVE_ENTRIES: usize = 100_000;
/// Most input bytes (the sum of the archived files' sizes) in one archive.
pub const MAX_ARCHIVE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// Most names one walk of an archive lists (the planning walk and the
/// archive walk count separately): its entries, but also the special files
/// it skips and the names it could not examine, which the entry bound does
/// not count. Like the scan bound of `file_list`, it bounds a walk's time.
pub const MAX_ARCHIVE_SCAN: usize = 2 * MAX_ARCHIVE_ENTRIES;
/// How long a download may wait for its client (no acknowledgement and no
/// event taken into the client's output queue while a chunk is
/// unacknowledged or an event waits for room) before the server stops it
/// with an `io` error. A client that leaves a download behind (it gave up
/// on the start, or stopped reading) cannot hold its slot and thread for
/// longer than this.
pub const STALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10 * 60);
/// gzip level: miniz level 2 keeps one stream above gigabit rates on one
/// core for most of the size gain of higher levels (measured by Crabinet,
/// which streams ZIP archives the same way).
const LEVEL: u32 = 2;
/// Chunks a download thread may produce ahead of the event loop.
const AHEAD: usize = 2;
const BLOCK: usize = 512;
/// Largest value of an 11-digit octal header field.
const OCTAL_MAX: u64 = 0o777_7777_7777;

/// Download bounds, advertised in Welcome `file_limits.downloads`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub chunk_bytes: usize,
    pub max_chunks_in_flight: usize,
    pub max_downloads: usize,
    pub max_archive_entries: usize,
    pub max_archive_bytes: u64,
}
pub const LIMITS: Limits = Limits {
    chunk_bytes: CHUNK_BYTES,
    max_chunks_in_flight: MAX_CHUNKS_IN_FLIGHT,
    max_downloads: MAX_DOWNLOADS,
    max_archive_entries: MAX_ARCHIVE_ENTRIES,
    max_archive_bytes: MAX_ARCHIVE_BYTES,
};

/// The archive format of a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Archive {
    /// A gzip-compressed POSIX tar archive.
    #[serde(rename = "tar.gz")]
    TarGz,
}

/// What a download thread produces, in this order: `Started` or `Failed`;
/// then chunks; then `Done` or `Failed`.
#[derive(Debug)]
pub enum Piece {
    Started {
        name: String,
        size: Option<u64>,
        archive: Option<Archive>,
        entries: Option<u64>,
        input_bytes: Option<u64>,
    },
    Chunk {
        data: Vec<u8>,
        /// Archives: input bytes read so far.
        input: Option<u64>,
    },
    Done {
        size: u64,
        hash: String,
        skipped: u64,
        changed: u64,
    },
    Failed(Error),
}

/// A running download, as the event loop holds it. Dropping it cancels the
/// download: the thread stops at its next read or chunk.
pub struct Job {
    pub receiver: mpsc::Receiver<Piece>,
    cancel: Arc<AtomicBool>,
    alive: Thread,
}
impl Job {
    /// The download's thread, which may outlive the job once cancelled.
    pub fn thread(&self) -> Thread {
        self.alive.clone()
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
/// Whether a download's thread is still running.
#[derive(Clone)]
pub struct Thread(Arc<AtomicBool>);
impl Thread {
    pub fn running(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    #[cfg(test)]
    pub(super) fn from_flag(running: Arc<AtomicBool>) -> Self {
        Self(running)
    }
}
/// Counts a download thread from its start until it returns.
struct Running(Arc<AtomicUsize>, Arc<AtomicBool>);
impl Drop for Running {
    fn drop(&mut self) {
        self.1.store(false, Ordering::Release);
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Start a download on its own thread; `threads` counts running download
/// threads. `root` is the server's file root (`None` refuses the download
/// with `outside_root`).
pub fn start(
    paths: Vec<String>,
    archive: Option<Archive>,
    root: Option<files::Root>,
    threads: &Arc<AtomicUsize>,
) -> io::Result<Job> {
    let (sender, receiver) = mpsc::sync_channel(AHEAD);
    let cancel = Arc::new(AtomicBool::new(false));
    let alive = Arc::new(AtomicBool::new(true));
    threads.fetch_add(1, Ordering::Relaxed);
    let running = Running(threads.clone(), alive.clone());
    let flag = cancel.clone();
    std::thread::Builder::new()
        .name("kelpserver-download".into())
        .spawn(move || {
            let _running = running;
            run(paths, archive, root, Output::new(sender, flag));
        })?;
    Ok(Job {
        receiver,
        cancel,
        alive: Thread(alive),
    })
}

fn cancelled() -> io::Error {
    // Not `Interrupted`, which `write_all` retries.
    io::Error::new(io::ErrorKind::ConnectionAborted, "download cancelled")
}
fn is_cancelled(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::ConnectionAborted && error.to_string() == "download cancelled"
}

/// The bytes sent: hashed, counted and cut into chunks for the channel.
struct Output {
    sender: mpsc::SyncSender<Piece>,
    cancel: Arc<AtomicBool>,
    buffer: Vec<u8>,
    size: u64,
    hash: super::sha256::Sha256,
    /// Archives: input bytes read so far, reported with each chunk.
    input: Option<Arc<AtomicU64>>,
}
impl Output {
    fn new(sender: mpsc::SyncSender<Piece>, cancel: Arc<AtomicBool>) -> Self {
        Self {
            sender,
            cancel,
            buffer: Vec::with_capacity(CHUNK_BYTES),
            size: 0,
            hash: Default::default(),
            input: None,
        }
    }
    fn stopped(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    fn send(&self, piece: Piece) -> io::Result<()> {
        if self.stopped() {
            return Err(cancelled());
        }
        // Blocks while the event loop is `AHEAD` chunks behind; fails once
        // the download is dropped.
        self.sender.send(piece).map_err(|_| cancelled())
    }
    fn emit(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let data = std::mem::replace(&mut self.buffer, Vec::with_capacity(CHUNK_BYTES));
        self.hash.update(&data);
        self.size += data.len() as u64;
        let input = self.input.as_ref().map(|n| n.load(Ordering::Relaxed));
        self.send(Piece::Chunk { data, input })
    }
    /// The last chunk; the total size and hash.
    fn finish(mut self) -> io::Result<(u64, String, Self)> {
        self.emit()?;
        let hash = std::mem::take(&mut self.hash).hex();
        Ok((self.size, hash, self))
    }
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.stopped() {
            return Err(cancelled());
        }
        let n = bytes.len().min(CHUNK_BYTES - self.buffer.len());
        self.buffer.extend_from_slice(&bytes[..n]);
        if self.buffer.len() == CHUNK_BYTES {
            self.emit()?;
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        // Chunks are whole until the end; `finish` sends the last one.
        Ok(())
    }
}

fn run(paths: Vec<String>, archive: Option<Archive>, root: Option<files::Root>, output: Output) {
    let sender = output.sender.clone();
    let cancel = output.cancel.clone();
    let result = match &root {
        Some(root) => download(paths, archive, root, output),
        None => Err(files::no_root()),
    };
    let failure = match result {
        Ok(done) => {
            let _ = sender.send(done);
            return;
        }
        Err(error) => error,
    };
    if !cancel.load(Ordering::Relaxed) {
        let _ = sender.send(Piece::Failed(failure));
    }
}
fn download(
    paths: Vec<String>,
    archive: Option<Archive>,
    root: &files::Root,
    output: Output,
) -> Outcome<Piece> {
    if paths.is_empty() || paths.len() > files::MAX_PATHS {
        return Err(Error::new(
            Code::InvalidRequest,
            format!("name 1 to {} paths", files::MAX_PATHS),
        ));
    }
    let mut normalized = Vec::with_capacity(paths.len());
    for path in &paths {
        let at = |mut error: Error| {
            error.path = Some(path.clone());
            error
        };
        let checked = root.request_path(path).map_err(at)?;
        if root.relative(&checked)?.as_os_str().is_empty() {
            return Err(at(Error::new(
                Code::InvalidRequest,
                "a file root itself is not downloaded; select the entries inside it",
            )));
        }
        normalized.push(checked);
    }
    match archive {
        None if normalized.len() == 1 => raw(root, &normalized[0], output),
        None => Err(Error::new(
            Code::InvalidRequest,
            "several paths download only as an archive",
        )),
        Some(Archive::TarGz) => tar_gz(root, &normalized, output, BOUNDS),
    }
}
/// Map an output failure: a cancelled download, or an I/O error at `path`.
fn output_error(error: io::Error, path: &Path) -> Error {
    if is_cancelled(&error) {
        cancelled_error()
    } else {
        files::io_error(error, path)
    }
}

/// One regular file, as itself.
fn raw(root: &files::Root, path: &Path, mut output: Output) -> Outcome<Piece> {
    // Opened like `file_read` opens it: beneath the root, the type first.
    let (fd, st) = files::open_regular(root, path, "download").map_err(|mut error| {
        if error.code == Code::IsADirectory {
            error.message = "a folder downloads only as an archive".into();
        }
        error
    })?;
    output
        .send(Piece::Started {
            name: name_text(path),
            size: Some(st.st_size as u64),
            archive: None,
            entries: None,
            input_bytes: None,
        })
        .map_err(|e| output_error(e, path))?;
    let mut file = File::from(fd);
    let mut buffer = vec![0u8; CHUNK_BYTES];
    loop {
        let n = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(files::io_error(error, path)),
        };
        output
            .write_all(&buffer[..n])
            .map_err(|e| output_error(e, path))?;
    }
    let (size, hash, _) = output.finish().map_err(|e| output_error(e, path))?;
    Ok(Piece::Done {
        size,
        hash,
        skipped: 0,
        changed: 0,
    })
}
fn name_text(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".into())
}

/// One selected entry of an archive: its parent directory and `lstat`.
struct Root {
    path: PathBuf,
    location: files::Location,
    st: libc::stat,
}
/// What the planning walk found.
#[derive(Default)]
struct Plan {
    entries: u64,
    bytes: u64,
}
/// The bounds of one archive; tests use smaller ones.
#[derive(Debug, Clone, Copy)]
struct Bounds {
    /// Files, folders and symlinks archived.
    entries: u64,
    /// Input bytes: the sum of the archived files' sizes.
    bytes: u64,
    /// Names one walk lists.
    scan: u64,
    /// Names listed and not yet taken, across every open level of a walk:
    /// what a walk holds in memory, whatever its depth.
    held: u64,
}
const BOUNDS: Bounds = Bounds {
    entries: MAX_ARCHIVE_ENTRIES as u64,
    bytes: MAX_ARCHIVE_BYTES,
    scan: MAX_ARCHIVE_SCAN as u64,
    held: MAX_ARCHIVE_ENTRIES as u64,
};
fn too_many_entries(bounds: &Bounds, path: &Path) -> Error {
    Error::new(
        Code::TooLarge,
        format!(
            "the archive would hold more than {} entries",
            bounds.entries
        ),
    )
    .at(path)
}
fn too_many_names(bounds: &Bounds, path: &Path) -> Error {
    Error::new(
        Code::TooLarge,
        format!(
            "the folders hold more than {} names (special files included)",
            bounds.scan
        ),
    )
    .at(path)
}
fn too_many_bytes(bounds: &Bounds, path: &Path) -> Error {
    Error::new(
        Code::TooLarge,
        format!("the archive would read more than {} bytes", bounds.bytes),
    )
    .at(path)
}
fn too_deep(path: &Path) -> Error {
    Error::new(
        Code::TooLarge,
        format!("folders are nested deeper than {} levels", files::MAX_DEPTH),
    )
    .at(path)
}
fn cancelled_error() -> Error {
    Error::new(Code::Io, "download cancelled")
}
fn is_denied(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::EACCES | libc::EPERM))
}
fn is_gone(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR))
}
/// One walk of the selected trees (the planning walk or the archive walk):
/// its bounds, the download's cancel flag, the names it listed and the names
/// it holds.
struct Walk {
    bounds: Bounds,
    cancel: Arc<AtomicBool>,
    scanned: u64,
    held: u64,
}
impl Walk {
    fn new(bounds: Bounds, cancel: Arc<AtomicBool>) -> Self {
        Self {
            bounds,
            cancel,
            scanned: 0,
            held: 0,
        }
    }
    /// A directory's names in byte order, refusing one that would pass the
    /// entry bound (with `entries` counted so far), the held-names bound or
    /// the scan bound. Each name is held until [`Walk::take`].
    fn list(&mut self, dir: RawFd, entries: u64, path: &Path) -> Outcome<Vec<std::ffi::CString>> {
        let entry_room = self
            .bounds
            .entries
            .saturating_sub(entries)
            .min(self.bounds.held.saturating_sub(self.held));
        let scan_room = self.bounds.scan.saturating_sub(self.scanned);
        let room = entry_room.min(scan_room);
        let limit = usize::try_from(room)
            .unwrap_or(usize::MAX)
            .saturating_add(1);
        let (mut names, more) = files::names(dir, limit).map_err(|e| files::io_error(e, path))?;
        let listed = names.len() as u64;
        if more || listed > room {
            return Err(if scan_room < entry_room {
                too_many_names(&self.bounds, path)
            } else {
                too_many_entries(&self.bounds, path)
            });
        }
        self.scanned += listed;
        self.held += listed;
        names.sort();
        Ok(names)
    }
    /// One listed name is taken for processing; stop if cancelled.
    fn take(&mut self) -> Outcome<()> {
        self.held = self.held.saturating_sub(1);
        if self.cancel.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }
        Ok(())
    }
}
/// An entry's `lstat`, without triggering an automount.
fn lstat_at(dir: RawFd, name: &CStr) -> io::Result<libc::stat> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    files::check(unsafe {
        libc::fstatat(
            dir,
            name.as_ptr(),
            &mut st,
            libc::AT_SYMLINK_NOFOLLOW | libc::AT_NO_AUTOMOUNT,
        )
    })?;
    Ok(st)
}
fn open_dir_at(dir: RawFd, name: &CStr) -> io::Result<OwnedFd> {
    files::open_at(dir, name, files::DIRECTORY | libc::O_NOFOLLOW, 0)
}
/// Whether a folder opened by name is the one its listing described, on
/// the walk's file system: a mount point (whose `lstat` may describe the
/// covered folder) or an entry replaced meanwhile is not entered.
fn same_folder(listed: &libc::stat, opened: &libc::stat, dev: libc::dev_t) -> bool {
    files::is_dir(opened)
        && opened.st_dev == listed.st_dev
        && opened.st_ino == listed.st_ino
        && opened.st_dev == dev
}
/// Open a listed folder to walk it: `None` for a mount point or an entry
/// that is no longer that folder.
fn enter(
    dir: RawFd,
    name: &CStr,
    listed: &libc::stat,
    dev: libc::dev_t,
) -> io::Result<Option<OwnedFd>> {
    let child = open_dir_at(dir, name)?;
    let opened = files::stat_fd(child.as_raw_fd())?;
    Ok(same_folder(listed, &opened, dev).then_some(child))
}
/// The selected entries, checked.
fn roots(root: &files::Root, paths: &[PathBuf]) -> Outcome<Vec<Root>> {
    let mut roots = Vec::with_capacity(paths.len());
    let mut names = std::collections::BTreeSet::new();
    for path in paths {
        let location = files::locate(root, path)?;
        let st = lstat_at(location.dir.as_raw_fd(), &location.name)
            .map_err(|e| files::io_error(e, path))?;
        if !names.insert(location.name.clone()) {
            return Err(Error::new(
                Code::InvalidRequest,
                "two selected entries have the same name",
            )
            .at(path));
        }
        if !(files::is_regular(&st) || files::is_dir(&st) || files::is_symlink(&st)) {
            return Err(Error::new(Code::NotAFile, "not a file, folder or symlink").at(path));
        }
        roots.push(Root {
            path: path.clone(),
            location,
            st,
        });
    }
    Ok(roots)
}
/// Count what an archive of the selected entries would hold, within the
/// bounds.
fn plan(roots: &[Root], walk: &mut Walk) -> Outcome<Plan> {
    let mut plan = Plan::default();
    for root in roots {
        plan_entry(
            root.location.dir.as_raw_fd(),
            &root.location.name,
            &root.st,
            0,
            root.st.st_dev,
            &root.path,
            &mut plan,
            walk,
        )?;
    }
    Ok(plan)
}
fn plan_tree(
    dir: RawFd,
    depth: usize,
    dev: libc::dev_t,
    path: &Path,
    plan: &mut Plan,
    walk: &mut Walk,
) -> Outcome<()> {
    if depth > files::MAX_DEPTH {
        return Err(too_deep(path));
    }
    for name in walk.list(dir, plan.entries, path)? {
        walk.take()?;
        let child = path.join(OsStr::from_bytes(name.to_bytes()));
        let Ok(st) = lstat_at(dir, &name) else {
            continue;
        };
        plan_entry(dir, &name, &st, depth, dev, &child, plan, walk)?;
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn plan_entry(
    dir: RawFd,
    name: &CStr,
    st: &libc::stat,
    depth: usize,
    dev: libc::dev_t,
    path: &Path,
    plan: &mut Plan,
    walk: &mut Walk,
) -> Outcome<()> {
    let regular = files::is_regular(st);
    if !(regular || files::is_dir(st) || files::is_symlink(st)) {
        return Ok(());
    }
    plan.entries += 1;
    if plan.entries > walk.bounds.entries {
        return Err(too_many_entries(&walk.bounds, path));
    }
    if regular {
        plan.bytes += st.st_size as u64;
        if plan.bytes > walk.bounds.bytes {
            return Err(too_many_bytes(&walk.bounds, path));
        }
    } else if files::is_dir(st) && st.st_dev == dev {
        if let Ok(Some(child)) = enter(dir, name, st, dev) {
            plan_tree(child.as_raw_fd(), depth + 1, dev, path, plan, walk)?;
        }
    }
    Ok(())
}

/// Files and folders as one gzip-compressed tar archive.
fn tar_gz(
    root: &files::Root,
    paths: &[PathBuf],
    mut output: Output,
    bounds: Bounds,
) -> Outcome<Piece> {
    let roots = roots(root, paths)?;
    let plan = plan(&roots, &mut Walk::new(bounds, output.cancel.clone()))?;
    let name = match &roots[..] {
        [root] => format!("{}.tar.gz", name_text(&root.path)),
        _ => {
            let parent = roots[0].path.parent();
            if parent.is_some_and(|parent| roots.iter().all(|r| r.path.parent() == Some(parent)))
                && parent.and_then(Path::file_name).is_some()
            {
                format!("{}.tar.gz", name_text(parent.unwrap()))
            } else {
                "download.tar.gz".into()
            }
        }
    };
    let input = Arc::new(AtomicU64::new(0));
    output.input = Some(input.clone());
    let first = &roots[0].path;
    output
        .send(Piece::Started {
            name,
            size: None,
            archive: Some(Archive::TarGz),
            entries: Some(plan.entries),
            input_bytes: Some(plan.bytes),
        })
        .map_err(|e| output_error(e, first))?;
    let walk = Walk::new(bounds, output.cancel.clone());
    let mut tar = Tar {
        out: GzEncoder::new(output, Compression::new(LEVEL)),
        walk,
        input,
        entries: 0,
        bytes: 0,
        skipped: 0,
        changed: 0,
        buffer: vec![0u8; CHUNK_BYTES],
    };
    for root in &roots {
        let name = root.location.name.to_bytes().to_vec();
        tar.entry(
            root.location.dir.as_raw_fd(),
            &root.location.name,
            &root.st,
            &name,
            0,
            root.st.st_dev,
            &root.path,
        )?;
    }
    // The end of archive: two zero blocks.
    tar.out
        .write_all(&[0u8; 2 * BLOCK])
        .map_err(|e| output_error(e, first))?;
    let Tar {
        out,
        skipped,
        changed,
        ..
    } = tar;
    let output = out.finish().map_err(|e| output_error(e, first))?;
    let (size, hash, _) = output.finish().map_err(|e| output_error(e, first))?;
    Ok(Piece::Done {
        size,
        hash,
        skipped,
        changed,
    })
}

struct Tar {
    out: GzEncoder<Output>,
    walk: Walk,
    input: Arc<AtomicU64>,
    entries: u64,
    bytes: u64,
    /// Entries left out: devices, FIFOs, sockets, what could not be opened
    /// for lack of permission, and the contents of mount points.
    skipped: u64,
    /// Files that changed size or modification time while being read.
    changed: u64,
    buffer: Vec<u8>,
}
impl Tar {
    fn write(&mut self, bytes: &[u8], path: &Path) -> Outcome<()> {
        self.out.write_all(bytes).map_err(|e| output_error(e, path))
    }
    fn count(&mut self, path: &Path) -> Outcome<()> {
        self.entries += 1;
        if self.entries > self.walk.bounds.entries {
            return Err(too_many_entries(&self.walk.bounds, path));
        }
        Ok(())
    }
    fn header(&mut self, header: Header<'_>, path: &Path) -> Outcome<()> {
        let mut blocks = Vec::with_capacity(4 * BLOCK);
        header.encode(&mut blocks);
        self.write(&blocks, path)
    }
    /// Archive one entry as `archived` (a folder with its tree).
    #[allow(clippy::too_many_arguments)]
    fn entry(
        &mut self,
        dir: RawFd,
        name: &CStr,
        st: &libc::stat,
        archived: &[u8],
        depth: usize,
        dev: libc::dev_t,
        path: &Path,
    ) -> Outcome<()> {
        let mode = st.st_mode & 0o777;
        if files::is_dir(st) {
            self.count(path)?;
            let mut folder = archived.to_vec();
            folder.push(b'/');
            self.header(
                Header {
                    name: &folder,
                    kind: b'5',
                    mode,
                    size: 0,
                    mtime: st.st_mtime,
                    link: b"",
                },
                path,
            )?;
            if st.st_dev != dev {
                // A mount point: stored as an empty folder, like
                // `tar --one-file-system`.
                self.skipped += 1;
                return Ok(());
            }
            let child = match enter(dir, name, st, dev) {
                Ok(Some(child)) => child,
                // A mount point whose `lstat` described the covered folder,
                // or a folder replaced since it was listed: stored empty.
                Ok(None) => {
                    self.skipped += 1;
                    return Ok(());
                }
                Err(error) if is_denied(&error) => {
                    self.skipped += 1;
                    return Ok(());
                }
                Err(error) if is_gone(&error) => return Ok(()),
                Err(error) => return Err(files::io_error(error, path)),
            };
            self.tree(child.as_raw_fd(), &folder, depth + 1, dev, path)
        } else if files::is_symlink(st) {
            let Some(target) = read_link(dir, name) else {
                // Removed meanwhile.
                return Ok(());
            };
            self.count(path)?;
            self.header(
                Header {
                    name: archived,
                    kind: b'2',
                    mode,
                    size: 0,
                    mtime: st.st_mtime,
                    link: &target,
                },
                path,
            )
        } else if files::is_regular(st) {
            self.file(dir, name, archived, path)
        } else {
            self.skipped += 1;
            Ok(())
        }
    }
    fn tree(
        &mut self,
        dir: RawFd,
        prefix: &[u8],
        depth: usize,
        dev: libc::dev_t,
        path: &Path,
    ) -> Outcome<()> {
        if depth > files::MAX_DEPTH {
            return Err(too_deep(path));
        }
        for name in self.walk.list(dir, self.entries, path)? {
            self.walk.take()?;
            let child = path.join(OsStr::from_bytes(name.to_bytes()));
            let st = match lstat_at(dir, &name) {
                Ok(st) => st,
                Err(error) if is_gone(&error) => continue,
                Err(error) if is_denied(&error) => {
                    self.skipped += 1;
                    continue;
                }
                Err(error) => return Err(files::io_error(error, &child)),
            };
            let mut archived = prefix.to_vec();
            archived.extend_from_slice(name.to_bytes());
            self.entry(dir, &name, &st, &archived, depth, dev, &child)?;
        }
        Ok(())
    }
    /// A regular file: its size when opened is the entry's size. A file
    /// that shrinks while read is padded with zero bytes, one that grows is
    /// cut at that size; either counts as `changed`, as does a change of its
    /// modification time.
    fn file(&mut self, dir: RawFd, name: &CStr, archived: &[u8], path: &Path) -> Outcome<()> {
        let fd = match files::open_at(
            dir,
            name,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY,
            0,
        ) {
            Ok(fd) => fd,
            Err(error) if is_denied(&error) => {
                self.skipped += 1;
                return Ok(());
            }
            Err(error) if is_gone(&error) => return Ok(()),
            // Replaced by a symlink after the listing (ELOOP), or by
            // something that cannot be opened.
            Err(_) => {
                self.skipped += 1;
                return Ok(());
            }
        };
        let st = files::stat_fd(fd.as_raw_fd()).map_err(|e| files::io_error(e, path))?;
        if !files::is_regular(&st) {
            self.skipped += 1;
            return Ok(());
        }
        self.count(path)?;
        let size = st.st_size as u64;
        self.bytes += size;
        if self.bytes > self.walk.bounds.bytes {
            return Err(too_many_bytes(&self.walk.bounds, path));
        }
        self.header(
            Header {
                name: archived,
                kind: b'0',
                mode: st.st_mode & 0o777,
                size,
                mtime: st.st_mtime,
                link: b"",
            },
            path,
        )?;
        let mut file = File::from(fd);
        let mut remaining = size;
        let mut buffer = std::mem::take(&mut self.buffer);
        let result = (|| -> Outcome<()> {
            while remaining > 0 {
                let want = buffer
                    .len()
                    .min(usize::try_from(remaining).unwrap_or(usize::MAX));
                let n = match file.read(&mut buffer[..want]) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(files::io_error(error, path)),
                };
                self.write(&buffer[..n], path)?;
                remaining -= n as u64;
                self.input.fetch_add(n as u64, Ordering::Relaxed);
            }
            Ok(())
        })();
        self.buffer = buffer;
        result?;
        let mut changed = remaining > 0;
        while remaining > 0 {
            let n = remaining.min(self.buffer.len() as u64) as usize;
            let zeros = vec![0u8; n];
            self.write(&zeros, path)?;
            remaining -= n as u64;
        }
        let padding = (BLOCK - (size % BLOCK as u64) as usize) % BLOCK;
        self.write(&[0u8; BLOCK][..padding], path)?;
        if let Ok(now) = files::stat_fd(file.as_raw_fd()) {
            changed |= (now.st_size, now.st_mtime, now.st_mtime_nsec)
                != (st.st_size, st.st_mtime, st.st_mtime_nsec);
        }
        self.changed += u64::from(changed);
        Ok(())
    }
}
/// A symlink's target as bytes, not followed.
fn read_link(dir: RawFd, name: &CStr) -> Option<Vec<u8>> {
    let mut buffer = vec![0u8; files::MAX_PATH_BYTES + 1];
    let length =
        unsafe { libc::readlinkat(dir, name.as_ptr(), buffer.as_mut_ptr().cast(), buffer.len()) };
    if length < 0 {
        return None;
    }
    buffer.truncate(length as usize);
    Some(buffer)
}

/// One tar entry's header.
struct Header<'a> {
    name: &'a [u8],
    /// `0` file, `2` symlink, `5` folder (`x` for a pax header).
    kind: u8,
    mode: u32,
    size: u64,
    mtime: i64,
    link: &'a [u8],
}
impl Header<'_> {
    /// The ustar header block, preceded by a pax extended header (and its
    /// records) when a field does not fit ustar.
    fn encode(&self, out: &mut Vec<u8>) {
        let plain = |bytes: &[u8]| bytes.len() <= NAME_FIELD && bytes.iter().all(|&b| printable(b));
        let mut records = Vec::new();
        // Pax values are UTF-8 unless the header says otherwise.
        if std::str::from_utf8(self.name).is_err() || std::str::from_utf8(self.link).is_err() {
            pax_record(&mut records, b"hdrcharset", b"BINARY");
        }
        let (name, prefix) = if plain(self.name) {
            (self.name.to_vec(), Vec::new())
        } else {
            pax_record(&mut records, b"path", self.name);
            fallback(self.name, NAME_FIELD, PREFIX_FIELD)
        };
        let link = if plain(self.link) {
            self.link.to_vec()
        } else {
            pax_record(&mut records, b"linkpath", self.link);
            fallback(self.link, NAME_FIELD, 0).0
        };
        if self.size > OCTAL_MAX {
            pax_record(&mut records, b"size", self.size.to_string().as_bytes());
        }
        let mtime = u64::try_from(self.mtime)
            .ok()
            .filter(|mtime| *mtime <= OCTAL_MAX);
        if mtime.is_none() {
            pax_record(&mut records, b"mtime", self.mtime.to_string().as_bytes());
        }
        if !records.is_empty() {
            let mut pax = b"PaxHeader/".to_vec();
            let room = NAME_FIELD - pax.len();
            pax.extend_from_slice(&fallback(self.name, room, 0).0);
            block(
                out,
                &pax,
                b"",
                b'x',
                0o644,
                records.len() as u64,
                mtime.unwrap_or(0),
                b"",
            );
            out.extend_from_slice(&records);
            out.resize(out.len().div_ceil(BLOCK) * BLOCK, 0);
        }
        block(
            out,
            &name,
            &prefix,
            self.kind,
            self.mode,
            if self.size > OCTAL_MAX { 0 } else { self.size },
            mtime.unwrap_or(0),
            &link,
        );
    }
}
/// A pax record: `<length> <key>=<value>\n`, the length counting itself.
fn pax_record(out: &mut Vec<u8>, key: &[u8], value: &[u8]) {
    let rest = 1 + key.len() + 1 + value.len() + 1;
    let mut length = rest + 1;
    while length != rest + length.to_string().len() {
        length = rest + length.to_string().len();
    }
    out.extend_from_slice(length.to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(key);
    out.push(b'=');
    out.extend_from_slice(value);
    out.push(b'\n');
}
/// The ustar `name` and `linkname` fields' size.
const NAME_FIELD: usize = 100;
/// The ustar `prefix` field's size.
const PREFIX_FIELD: usize = 155;
fn printable(b: u8) -> bool {
    (0x20..0x7f).contains(&b)
}
/// A path's components as a relative path: printable ASCII only (others as
/// `_`), without empty, `.` or `..` components, so never absolute; a
/// trailing `/` (a folder) is kept.
fn relative(path: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(path.len());
    for part in path.split(|&b| b == b'/') {
        if part.is_empty() || part == b"." || part == b".." {
            continue;
        }
        if !out.is_empty() {
            out.push(b'/');
        }
        out.extend(part.iter().map(|&b| if printable(b) { b } else { b'_' }));
    }
    if path.ends_with(b"/") && !out.is_empty() {
        out.push(b'/');
    }
    out
}
/// The ustar `(name, prefix)` fields of a path that also has a pax record,
/// for readers that ignore pax (those that know it use the record). The
/// value is never absolute, has no `.` or `..` component and is never
/// empty: the whole path split on a `/` into `prefix` (at most
/// `prefix_max` bytes) and `name` (at most `max`) when it fits, otherwise
/// the last components that fit `max` bytes (the end of the last component
/// when it alone is longer), with an empty prefix.
fn fallback(path: &[u8], max: usize, prefix_max: usize) -> (Vec<u8>, Vec<u8>) {
    let path = relative(path);
    if path.len() <= max {
        return (non_empty(path), Vec::new());
    }
    // A split on a `/` that leaves a name (not the trailing `/` of a folder).
    let split = path[..path.len() - 1]
        .iter()
        .enumerate()
        .filter(|&(_, &b)| b == b'/')
        .map(|(at, _)| at)
        .find(|&at| at <= prefix_max && path.len() - at - 1 <= max);
    if let Some(at) = split {
        return (path[at + 1..].to_vec(), path[..at].to_vec());
    }
    // The last components that fit, starting after a `/`.
    let tail = (path.len() - max..path.len() - 1)
        .find(|&at| path[at - 1] == b'/')
        .map(|at| path[at..].to_vec());
    let name = match tail {
        Some(tail) => tail,
        None => relative(&path[path.len() - max..]),
    };
    (non_empty(name), Vec::new())
}
fn non_empty(name: Vec<u8>) -> Vec<u8> {
    if name.is_empty() || name == b"/" {
        b"_".to_vec()
    } else {
        name
    }
}
/// One ustar header block.
#[allow(clippy::too_many_arguments)]
fn block(
    out: &mut Vec<u8>,
    name: &[u8],
    prefix: &[u8],
    kind: u8,
    mode: u32,
    size: u64,
    mtime: u64,
    link: &[u8],
) {
    let mut header = [0u8; BLOCK];
    header[..name.len()].copy_from_slice(name);
    octal(&mut header[100..108], u64::from(mode));
    octal(&mut header[108..116], 0);
    octal(&mut header[116..124], 0);
    octal(&mut header[124..136], size);
    octal(&mut header[136..148], mtime);
    header[156] = kind;
    header[157..157 + link.len()].copy_from_slice(link);
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    header[345..345 + prefix.len()].copy_from_slice(prefix);
    // The checksum counts its own field as eight spaces.
    header[148..156].copy_from_slice(b"        ");
    let sum: u32 = header.iter().map(|&b| u32::from(b)).sum();
    header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    out.extend_from_slice(&header);
}
/// A zero-padded octal number filling all but the field's last byte (NUL).
fn octal(field: &mut [u8], value: u64) {
    let digits = field.len() - 1;
    let text = format!("{value:0digits$o}");
    field[..digits].copy_from_slice(&text.as_bytes()[text.len() - digits..]);
    field[digits] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pax_records_count_their_own_length() {
        let mut out = Vec::new();
        pax_record(&mut out, b"path", b"a");
        assert_eq!(out, b"9 path=a\n");
        let mut out = Vec::new();
        let value = vec![b'x'; 93];
        pax_record(&mut out, b"path", &value);
        // 1 + 4 + 1 + 93 + 1 = 100 bytes besides the length: 103 in all.
        assert_eq!(out.len(), 103);
        assert!(out.starts_with(b"103 path="));
    }
    #[test]
    fn headers_are_ustar_with_a_valid_checksum() {
        let mut out = Vec::new();
        Header {
            name: b"dir/",
            kind: b'5',
            mode: 0o755,
            size: 0,
            mtime: 1_795_300_000,
            link: b"",
        }
        .encode(&mut out);
        assert_eq!(out.len(), BLOCK);
        assert_eq!(&out[..4], b"dir/");
        assert_eq!(&out[100..108], b"0000755\0");
        assert_eq!(&out[257..263], b"ustar\0");
        assert_eq!(out[156], b'5');
        let stored = u32::from_str_radix(std::str::from_utf8(&out[148..154]).unwrap(), 8).unwrap();
        let mut copy = out.clone();
        copy[148..156].copy_from_slice(b"        ");
        assert_eq!(stored, copy.iter().map(|&b| u32::from(b)).sum::<u32>());
        // A long or non-ASCII name gets a pax header first.
        let mut out = Vec::new();
        Header {
            name: "é".repeat(60).as_bytes(),
            kind: b'0',
            mode: 0o644,
            size: 3,
            mtime: -5,
            link: b"",
        }
        .encode(&mut out);
        assert_eq!(out.len(), 3 * BLOCK);
        assert_eq!(out[156], b'x');
        let records = &out[BLOCK..2 * BLOCK];
        let text = String::from_utf8_lossy(records);
        assert!(text.contains(&format!("path={}\n", "é".repeat(60))));
        assert!(text.contains("mtime=-5\n"));
        assert_eq!(out[2 * BLOCK + 156], b'0');
    }
    #[test]
    fn octal_fields_are_zero_padded() {
        let mut field = [0xffu8; 12];
        octal(&mut field, 5);
        assert_eq!(&field, b"00000000005\0");
        octal(&mut field, OCTAL_MAX);
        assert_eq!(&field, b"77777777777\0");
    }
    /// A field's bytes up to its first NUL.
    fn field(block: &[u8], range: std::ops::Range<usize>) -> &[u8] {
        let bytes = &block[range];
        &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())]
    }
    /// What a reader that ignores pax would extract: `prefix/name`.
    fn ustar_path(block: &[u8]) -> Vec<u8> {
        let (name, prefix) = (field(block, 0..100), field(block, 345..500));
        if prefix.is_empty() {
            name.to_vec()
        } else {
            [prefix, b"/", name].concat()
        }
    }
    fn assert_safe(value: &[u8], what: &str) {
        assert!(!value.is_empty(), "{what} is empty");
        assert!(!value.starts_with(b"/"), "{what} is absolute: {value:?}");
        for part in value.split(|&b| b == b'/') {
            assert!(
                part != b".." && part != b".",
                "{what} has a dot component: {:?}",
                String::from_utf8_lossy(value)
            );
        }
        assert!(value.iter().all(|&b| printable(b)), "{what} is not ASCII");
    }
    #[test]
    fn ustar_fallback_names_are_relative_without_dot_components() {
        let long = |head: &[u8], tail: usize| {
            let mut name = head.to_vec();
            name.extend(std::iter::repeat_n(b'z', tail));
            name
        };
        let mut names = vec![
            // The last 100 bytes would start with `../`.
            long(b"r/x../", 97),
            // The last 100 bytes would start with `/`, with a short and a
            // long middle component (no `prefix` split fits the latter).
            [b"r/".as_slice(), &[b'q'; 40], b"/", &[b'y'; 99]].concat(),
            [b"r/".as_slice(), &[b'q'; 200], b"/", &[b'y'; 99]].concat(),
            [b"r/".as_slice(), &[b'q'; 200], b"/", &[b'y'; 99], b"/"].concat(),
            // One component longer than the name field.
            [b"r/".as_slice(), &[b'.'; 200]].concat(),
            [b"r/".as_slice(), &[b'.'; 101], b"/"].concat(),
            // Not UTF-8, and dots after a component boundary.
            [b"r/\xff\xfe/".as_slice(), &[b'.'; 2], b"/", &[b'w'; 120]].concat(),
        ];
        names.push(long(b"", 300));
        for name in &names {
            for link in [b"".as_slice(), name] {
                let mut out = Vec::new();
                Header {
                    name,
                    kind: if link.is_empty() { b'0' } else { b'2' },
                    mode: 0o644,
                    size: 0,
                    mtime: 0,
                    link,
                }
                .encode(&mut out);
                let blocks: Vec<&[u8]> = out.chunks(BLOCK).collect();
                let (pax, header) = (blocks[0], blocks[blocks.len() - 1]);
                assert_eq!(pax[156], b'x');
                let pax_name = field(pax, 0..100);
                assert!(pax_name.starts_with(b"PaxHeader/"));
                assert_safe(&pax_name[b"PaxHeader/".len()..], "the pax header's name");
                assert_safe(field(header, 0..100), "the name field");
                let prefix = field(header, 345..500);
                if !prefix.is_empty() {
                    assert_safe(prefix, "the prefix field");
                }
                assert_safe(&ustar_path(header), "the ustar path");
                if !link.is_empty() {
                    assert_safe(field(header, 157..257), "the link field");
                }
                // The pax record keeps the exact name.
                let records = &out[BLOCK..out.len() - BLOCK];
                let mut record = b"path=".to_vec();
                record.extend_from_slice(name);
                record.push(b'\n');
                assert!(records.windows(record.len()).any(|w| w == record));
                // Non-UTF-8 values are flagged as binary.
                let binary = std::str::from_utf8(name).is_err();
                let flag = b"hdrcharset=BINARY\n";
                assert_eq!(records.windows(flag.len()).any(|w| w == flag), binary);
            }
        }
        // A path that fits `prefix` + `name` is split on a `/` and complete.
        let name = [b"r/".as_slice(), &[b'q'; 40], b"/", &[b'y'; 99]].concat();
        let mut out = Vec::new();
        Header {
            name: &name,
            kind: b'0',
            mode: 0o644,
            size: 0,
            mtime: 0,
            link: b"",
        }
        .encode(&mut out);
        assert_eq!(ustar_path(&out[out.len() - BLOCK..]), name);
        let name = long(b"r/x../", 97);
        assert_eq!(
            fallback(&name, 100, 155),
            (vec![b'z'; 97], b"r/x..".to_vec())
        );
        assert_eq!(fallback(b"/../..", 100, 155).0, b"_");
    }

    /// A fresh folder for a test.
    fn scratch(test: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "kelpserver-downloads-unit-{}-{test}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    fn small(scan: u64, held: u64) -> Bounds {
        Bounds {
            entries: 1000,
            bytes: 1 << 20,
            scan,
            held,
        }
    }
    fn plan_of(path: &Path, bounds: Bounds, cancelled: bool) -> (Outcome<Plan>, Walk) {
        let root = files::Root::new(path.parent()).unwrap();
        let roots = roots(&root, &[root.confine(path).unwrap()]).unwrap();
        let mut walk = Walk::new(bounds, Arc::new(AtomicBool::new(cancelled)));
        (plan(&roots, &mut walk), walk)
    }
    fn mkfifo(path: &Path) {
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    }
    #[test]
    fn a_cancelled_walk_stops_at_its_next_name() {
        let root = scratch("cancel");
        for dir in ["a", "b", "c"] {
            std::fs::create_dir(root.join(dir)).unwrap();
            for file in 0..50 {
                std::fs::write(root.join(dir).join(file.to_string()), "x").unwrap();
            }
        }
        let (result, walk) = plan_of(&root, BOUNDS, false);
        assert_eq!(result.unwrap().entries, 1 + 3 + 150);
        assert_eq!(walk.scanned, 3 + 150);
        assert_eq!(walk.held, 0);
        // Cancelled: the walk lists the top folder, then stops before its
        // first name, without entering a folder.
        let (result, walk) = plan_of(&root, BOUNDS, true);
        let error = result.err().unwrap();
        assert_eq!(error.message, "download cancelled");
        assert_eq!(walk.scanned, 3);
        // The archive walk stops the same way.
        let (sender, _receiver) = mpsc::sync_channel(AHEAD);
        let flag = Arc::new(AtomicBool::new(false));
        let output = Output::new(sender, flag.clone());
        let mut tar = Tar {
            out: GzEncoder::new(output, Compression::new(LEVEL)),
            walk: Walk::new(BOUNDS, flag.clone()),
            input: Default::default(),
            entries: 0,
            bytes: 0,
            skipped: 0,
            changed: 0,
            buffer: vec![0u8; 16],
        };
        let dir = files::open_at(
            libc::AT_FDCWD,
            &files::cstring(root.as_os_str().as_bytes()).unwrap(),
            files::DIRECTORY,
            0,
        )
        .unwrap();
        flag.store(true, Ordering::Relaxed);
        let error = tar
            .tree(
                dir.as_raw_fd(),
                b"r/",
                1,
                lstat_at(dir.as_raw_fd(), c".").unwrap().st_dev,
                &root,
            )
            .err()
            .unwrap();
        assert_eq!(error.message, "download cancelled");
        assert_eq!((tar.walk.scanned, tar.entries), (3, 0));
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn special_files_count_against_the_scan_budget() {
        let root = scratch("fifos");
        for n in 0..50 {
            mkfifo(&root.join(format!("fifo{n}")));
        }
        // FIFOs are no entries: the entry bound alone would let any number
        // through.
        let (result, _) = plan_of(&root, small(1000, 1000), false);
        assert_eq!(result.unwrap().entries, 1);
        let (result, walk) = plan_of(&root, small(20, 1000), false);
        let error = result.err().unwrap();
        assert_eq!(error.code, Code::TooLarge);
        assert!(error.message.contains("names"), "{}", error.message);
        assert!(walk.scanned <= 20);
        // Across folders, too.
        std::fs::create_dir(root.join("sub")).unwrap();
        for n in 0..30 {
            mkfifo(&root.join(format!("sub/fifo{n}")));
        }
        let (result, _) = plan_of(&root, small(60, 1000), false);
        assert_eq!(result.err().unwrap().code, Code::TooLarge);
        let (result, walk) = plan_of(&root, small(81, 1000), false);
        assert_eq!(result.unwrap().entries, 2);
        assert_eq!(walk.scanned, 81);
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn names_held_across_open_folders_are_bounded() {
        let root = scratch("held");
        // `a` is walked first, while the top folder still holds five names.
        std::fs::create_dir(root.join("a")).unwrap();
        for n in 0..5 {
            std::fs::write(root.join(format!("f{n}")), "x").unwrap();
            std::fs::write(root.join(format!("a/g{n}")), "x").unwrap();
        }
        let (result, walk) = plan_of(&root, small(1000, 10), false);
        assert_eq!(result.unwrap().entries, 1 + 6 + 5);
        assert_eq!(walk.held, 0);
        std::fs::write(root.join("a/g5"), "x").unwrap();
        let (result, _) = plan_of(&root, small(1000, 10), false);
        let error = result.err().unwrap();
        assert_eq!(error.code, Code::TooLarge);
        assert!(error.message.contains("entries"), "{}", error.message);
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn only_the_listed_folder_on_the_same_file_system_is_entered() {
        let mut listed: libc::stat = unsafe { std::mem::zeroed() };
        listed.st_mode = libc::S_IFDIR | 0o755;
        listed.st_dev = 7;
        listed.st_ino = 42;
        assert!(same_folder(&listed, &listed, 7));
        // Another file system (a mount point), or the walk's root elsewhere.
        let mut opened = listed;
        opened.st_dev = 8;
        assert!(!same_folder(&listed, &opened, 7));
        assert!(!same_folder(&listed, &listed, 8));
        // Another folder (replaced after the listing).
        let mut opened = listed;
        opened.st_ino = 43;
        assert!(!same_folder(&listed, &opened, 7));
        let mut opened = listed;
        opened.st_mode = libc::S_IFREG | 0o644;
        assert!(!same_folder(&listed, &opened, 7));
    }
}
