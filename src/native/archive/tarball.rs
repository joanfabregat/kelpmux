//! The archive format: a gzip-compressed tar of a project folder's contents
//! (member paths relative to the folder, GNU long names), written and read
//! in this process, streaming, with no shell and no `tar` program.
//!
//! [`pack`] never follows a symbolic link (it stores the link), skips
//! sockets, FIFOs and devices (reported), keeps permission bits and
//! modification times, stores hard links as separate files, and refuses to
//! cross into another file system. [`unpack`] accepts only folders,
//! regular files and symbolic links with relative paths made of plain
//! names (no absolute path, no `..`), never writes through a symbolic link
//! (every parent must be a real folder it created) and never replaces a
//! file; it is meant for a fresh, private folder that is renamed into
//! place afterwards.
use super::{Cancelled, Progress};
use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
};

type Result<T> = std::result::Result<T, String>;

/// CRC32C (Castagnoli), as Cloud Storage computes it.
#[derive(Clone, Copy, Default)]
pub struct Crc32c(u32);
const fn crc32c_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x82F6_3B78
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}
static CRC32C: [u32; 256] = crc32c_table();
impl Crc32c {
    pub fn update(&mut self, bytes: &[u8]) {
        let mut crc = !self.0;
        for byte in bytes {
            crc = CRC32C[((crc ^ u32::from(*byte)) & 0xff) as usize] ^ (crc >> 8);
        }
        self.0 = !crc;
    }
    pub fn value(self) -> u32 {
        self.0
    }
}
/// The CRC32C of `bytes`.
pub fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = Crc32c::default();
    crc.update(bytes);
    crc.value()
}
/// A CRC32C as Cloud Storage writes it: base64 of its four big-endian bytes.
pub fn crc32c_base64(value: u32) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(value.to_be_bytes())
}

/// The checksums and length of a byte stream.
pub struct Digests {
    sha256: ring::digest::Context,
    crc32c: Crc32c,
    length: u64,
}
impl Default for Digests {
    fn default() -> Self {
        Self {
            sha256: ring::digest::Context::new(&ring::digest::SHA256),
            crc32c: Crc32c::default(),
            length: 0,
        }
    }
}
impl Digests {
    pub fn update(&mut self, bytes: &[u8]) {
        self.sha256.update(bytes);
        self.crc32c.update(bytes);
        self.length += bytes.len() as u64;
    }
    /// (lowercase hexadecimal SHA-256, CRC32C, length).
    pub fn finish(self) -> (String, u32, u64) {
        let sha = self.sha256.finish();
        (
            sha.as_ref().iter().map(|b| format!("{b:02x}")).collect(),
            self.crc32c.value(),
            self.length,
        )
    }
}

/// A writer that digests what passes through.
struct Digesting<W> {
    inner: W,
    digests: Digests,
}
impl<W: Write> Write for Digesting<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(bytes)?;
        self.digests.update(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// A reader of exactly `remaining` bytes: an earlier end is an error (the
/// file shrank while it was read), so a member never gets fewer bytes than
/// its header says.
struct Exact<'a, R> {
    inner: R,
    remaining: u64,
    progress: &'a Progress,
}
impl<R: Read> Read for Exact<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        if self.progress.cancelled() {
            return Err(io::Error::other(Cancelled));
        }
        let want = buffer
            .len()
            .min(self.remaining.min(usize::MAX as u64) as usize);
        let n = self.inner.read(&mut buffer[..want])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "a file shrank while it was archived",
            ));
        }
        self.remaining -= n as u64;
        self.progress.add_bytes(n as u64);
        Ok(n)
    }
}

/// What [`pack`] wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Packed {
    /// Folders, files and links stored.
    pub entries: u64,
    /// Bytes of the regular files stored.
    pub bytes: u64,
    /// Sockets, FIFOs and devices skipped (relative paths).
    pub skipped: Vec<String>,
    /// The tar.gz: lowercase hexadecimal SHA-256, CRC32C and size.
    pub sha256: String,
    pub crc32c: u32,
    pub size: u64,
}

fn io_error(path: &Path, error: impl std::fmt::Display) -> String {
    format!("{}: {error}", path.display())
}

fn cancelled(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.downcast_ref::<Cancelled>().is_some())
}

/// Packs the contents of `folder` as a tar.gz into `out` (see the module
/// documentation). `progress` counts the bytes of files read.
pub fn pack(folder: &Path, out: File, progress: &Progress) -> Result<Packed> {
    let root = fs::symlink_metadata(folder).map_err(|e| io_error(folder, e))?;
    if !root.is_dir() {
        return Err(format!("{} is not a folder", folder.display()));
    }
    let device = root.dev();
    let writer = Digesting {
        inner: io::BufWriter::with_capacity(256 * 1024, out),
        digests: Digests::default(),
    };
    let gzip = flate2::write::GzEncoder::new(writer, flate2::Compression::default());
    let mut builder = tar::Builder::new(gzip);
    let mut packed = Packed::default();
    // The folder itself, first: its mode (and time), so a restore gives the
    // folder back as private as it was. Not counted as an entry.
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_mode(root.mode() & 0o777);
    header.set_mtime(u64::try_from(root.mtime()).unwrap_or(0));
    header.set_uid(u64::from(root.uid()));
    header.set_gid(u64::from(root.gid()));
    header.set_size(0);
    builder
        .append_data(&mut header, "./", io::empty())
        .map_err(|e| e.to_string())?;
    // Depth first, names sorted, so the same tree gives the same archive.
    let mut stack: Vec<PathBuf> = vec![PathBuf::new()];
    while let Some(relative) = stack.pop() {
        let dir = folder.join(&relative);
        let mut names: Vec<std::ffi::OsString> = fs::read_dir(&dir)
            .map_err(|e| io_error(&dir, e))?
            .map(|entry| entry.map(|e| e.file_name()))
            .collect::<io::Result<_>>()
            .map_err(|e| io_error(&dir, e))?;
        names.sort();
        let mut subfolders = Vec::new();
        for name in names {
            if progress.cancelled() {
                return Err(Cancelled.to_string());
            }
            let member = relative.join(&name);
            let path = folder.join(&member);
            let meta = fs::symlink_metadata(&path).map_err(|e| io_error(&path, e))?;
            let kind = meta.file_type();
            let mut header = tar::Header::new_gnu();
            header.set_mode(meta.mode() & 0o7777);
            header.set_mtime(u64::try_from(meta.mtime()).unwrap_or(0));
            header.set_uid(u64::from(meta.uid()));
            header.set_gid(u64::from(meta.gid()));
            if kind.is_dir() {
                if meta.dev() != device {
                    return Err(format!(
                        "{} is another file system (a mount point); KelpMux does not archive across mount points: unmount it first",
                        path.display()
                    ));
                }
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                builder
                    .append_data(&mut header, &member, io::empty())
                    .map_err(|e| io_error(&path, e))?;
                subfolders.push(member);
            } else if kind.is_symlink() {
                let target = fs::read_link(&path).map_err(|e| io_error(&path, e))?;
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                builder
                    .append_link(&mut header, &member, &target)
                    .map_err(|e| io_error(&path, e))?;
            } else if kind.is_file() {
                // O_NOFOLLOW: a file replaced by a link since the listing
                // is refused; O_NONBLOCK: never waits on a FIFO.
                let file = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
                    .open(&path)
                    .map_err(|e| io_error(&path, e))?;
                let opened = file.metadata().map_err(|e| io_error(&path, e))?;
                if !opened.is_file() || opened.ino() != meta.ino() || opened.dev() != meta.dev() {
                    return Err(format!("{} changed while it was archived", path.display()));
                }
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(opened.len());
                let reader = Exact {
                    inner: file,
                    remaining: opened.len(),
                    progress,
                };
                builder
                    .append_data(&mut header, &member, reader)
                    .map_err(|e| {
                        if cancelled(&e) {
                            Cancelled.to_string()
                        } else {
                            io_error(&path, e)
                        }
                    })?;
                packed.bytes += opened.len();
                progress.add_file();
            } else {
                // Sockets, FIFOs, devices (and anything else): not archived.
                packed.skipped.push(member.to_string_lossy().into_owned());
                continue;
            }
            packed.entries += 1;
        }
        // Popped in name order.
        stack.extend(subfolders.into_iter().rev());
    }
    let gzip = builder.into_inner().map_err(|e| e.to_string())?;
    let mut writer = gzip.finish().map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    let file = writer
        .inner
        .into_inner()
        .map_err(|e| e.error().to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    let (sha256, crc32c, size) = writer.digests.finish();
    packed.sha256 = sha256;
    packed.crc32c = crc32c;
    packed.size = size;
    Ok(packed)
}

/// A member path made safe: relative, plain names only (`.` parts
/// dropped). `None` for the archive's root (`.` or empty).
pub fn member_path(path: &Path) -> Result<Option<PathBuf>> {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => clean.push(name),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!(
                    "the archive member {} contains `..`",
                    path.display()
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "the archive member {} is an absolute path",
                    path.display()
                ));
            }
        }
    }
    if clean.as_os_str().len() > 4096 {
        return Err("an archive member's path is too long".into());
    }
    Ok((!clean.as_os_str().is_empty()).then_some(clean))
}

/// Sets a path's modification time without following a link.
fn set_mtime(path: &Path, seconds: u64) -> Result<()> {
    let c = CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let time = libc::timespec {
        tv_sec: seconds.min(i64::MAX as u64) as libc::time_t,
        tv_nsec: 0,
    };
    let times = [
        libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT,
        },
        time,
    ];
    // SAFETY: a NUL-terminated path and two timespecs.
    let result = unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            c.as_ptr(),
            times.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(io_error(path, io::Error::last_os_error()));
    }
    Ok(())
}

/// What [`unpack`] wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unpacked {
    /// The folder's own mode, when the archive holds it (`./`).
    pub root_mode: Option<u32>,
    pub entries: u64,
    pub bytes: u64,
}

/// Checks that every folder between `dest` and `path` is a real folder
/// (never a link), creating missing ones privately.
fn parents(dest: &Path, member: &Path) -> Result<()> {
    let mut current = dest.to_path_buf();
    let Some(parent) = member.parent() else {
        return Ok(());
    };
    for component in parent.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(format!(
                    "the archive puts a member below {}, which is not a folder",
                    current.display()
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&current)
                    .map_err(|e| io_error(&current, e))?;
            }
            Err(e) => return Err(io_error(&current, e)),
        }
    }
    Ok(())
}

/// The most an [`unpack`] may produce: entries (folders, files, links) and
/// bytes of regular files. Past either it stops: a damaged or hostile
/// archive cannot fill the disk.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub entries: u64,
    pub bytes: u64,
}
impl Limits {
    /// What the decompressed stream may hold besides file content: a
    /// header and padding per entry, long names, and the end blocks.
    fn stream(&self) -> u64 {
        self.bytes
            .saturating_add(self.entries.saturating_mul(8 * 1024))
            .saturating_add(1024 * 1024)
    }
}

/// Unpacks a tar.gz made by [`pack`] into `dest`, an empty folder of its
/// own (see the module documentation), within `limits`. `progress` counts
/// the bytes of files written.
pub fn unpack(archive: File, dest: &Path, limits: Limits, progress: &Progress) -> Result<Unpacked> {
    let reader = io::BufReader::with_capacity(256 * 1024, archive);
    // The decompressed stream is bounded too, so a long name cannot take
    // the memory either; a stream cut there reads as damaged.
    let decoder = flate2::read::GzDecoder::new(reader).take(limits.stream());
    let mut archive = tar::Archive::new(decoder);
    let mut unpacked = Unpacked::default();
    let past =
        |what: &str| format!("the archive holds more {what} than expected; nothing was restored");
    // Folder modes and times are set last, deepest first, so read-only
    // folders can be filled and their times survive.
    let mut folders: Vec<(PathBuf, u32, u64)> = Vec::new();
    for entry in archive.entries().map_err(|e| e.to_string())? {
        if progress.cancelled() {
            return Err(Cancelled.to_string());
        }
        let mut entry = entry.map_err(|e| format!("the archive is damaged: {e}"))?;
        let raw = entry.path().map_err(|e| e.to_string())?.into_owned();
        let Some(member) = member_path(&raw)? else {
            if entry.header().entry_type() == tar::EntryType::Directory {
                unpacked.root_mode = entry.header().mode().ok().map(|m| m & 0o777);
            }
            continue;
        };
        if unpacked.entries >= limits.entries {
            return Err(past("entries"));
        }
        let header = entry.header();
        let mode = header.mode().map_err(|e| e.to_string())? & 0o777;
        let mtime = header.mtime().map_err(|e| e.to_string())?;
        let kind = header.entry_type();
        parents(dest, &member)?;
        let path = dest.join(&member);
        match kind {
            tar::EntryType::Directory => {
                match fs::symlink_metadata(&path) {
                    Ok(meta) if meta.is_dir() => {}
                    Ok(_) => {
                        return Err(format!("{} appears twice in the archive", member.display()));
                    }
                    Err(_) => fs::DirBuilder::new()
                        .mode(0o700)
                        .create(&path)
                        .map_err(|e| io_error(&path, e))?,
                }
                folders.push((path, mode, mtime));
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                    .open(&path)
                    .map_err(|e| io_error(&path, e))?;
                let mut buffer = vec![0u8; 64 * 1024];
                loop {
                    if progress.cancelled() {
                        return Err(Cancelled.to_string());
                    }
                    let n = entry
                        .read(&mut buffer)
                        .map_err(|e| format!("the archive is damaged: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    if unpacked.bytes.saturating_add(n as u64) > limits.bytes {
                        return Err(past("bytes"));
                    }
                    file.write_all(&buffer[..n])
                        .map_err(|e| io_error(&path, e))?;
                    unpacked.bytes += n as u64;
                    progress.add_bytes(n as u64);
                }
                file.set_permissions(fs::Permissions::from_mode(mode))
                    .map_err(|e| io_error(&path, e))?;
                drop(file);
                set_mtime(&path, mtime)?;
                progress.add_file();
            }
            tar::EntryType::Symlink => {
                let target = entry
                    .link_name()
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("the link {} has no target", member.display()))?
                    .into_owned();
                std::os::unix::fs::symlink(&target, &path).map_err(|e| io_error(&path, e))?;
                set_mtime(&path, mtime)?;
            }
            other => {
                return Err(format!(
                    "the archive member {} is of a kind KelpMux does not restore ({other:?})",
                    member.display()
                ));
            }
        }
        unpacked.entries += 1;
    }
    // A stream cut at the bound can end on an entry boundary, which tar
    // reads as the end of the archive.
    if archive.into_inner().limit() == 0 {
        return Err(past("data"));
    }
    folders.sort_by_key(|a| std::cmp::Reverse(a.0.components().count()));
    for (path, mode, mtime) in folders {
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))
            .map_err(|e| io_error(&path, e))?;
        set_mtime(&path, mtime)?;
    }
    Ok(unpacked)
}

/// What a tree holds, to see whether it changed: entries (folders, files,
/// links; special files skipped), bytes of regular files, and whether an
/// entry's status (the folder's own included) changed at or after `since`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub entries: u64,
    pub bytes: u64,
    pub changed_since: bool,
}

/// The status change time of `meta`, as (seconds, nanoseconds).
fn ctime(meta: &fs::Metadata) -> (i64, i64) {
    (meta.ctime(), meta.ctime_nsec())
}

/// Now, as a status change time to compare with [`tally`]'s.
pub fn ctime_now() -> (i64, i64) {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: writes one timespec. CLOCK_REALTIME_COARSE is what the
    // kernel stamps inode times with, so nothing changed later can carry
    // an earlier time.
    unsafe { libc::clock_gettime(libc::CLOCK_REALTIME_COARSE, &mut now) };
    (now.tv_sec, now.tv_nsec)
}

/// Walks `folder` (not following links, on its own file system) for a
/// [`Tally`].
pub fn tally(folder: &Path, since: (i64, i64)) -> Result<Tally> {
    let root = fs::symlink_metadata(folder).map_err(|e| io_error(folder, e))?;
    let device = root.dev();
    let mut tally = Tally {
        changed_since: ctime(&root) >= since,
        ..Tally::default()
    };
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| io_error(&dir, e))? {
            let entry = entry.map_err(|e| io_error(&dir, e))?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).map_err(|e| io_error(&path, e))?;
            let kind = meta.file_type();
            if ctime(&meta) >= since {
                tally.changed_since = true;
            }
            if kind.is_dir() {
                if meta.dev() != device {
                    return Err(format!("{} is a mount point", path.display()));
                }
                tally.entries += 1;
                stack.push(path);
            } else if kind.is_file() {
                tally.entries += 1;
                tally.bytes += meta.len();
            } else if kind.is_symlink() {
                tally.entries += 1;
            }
        }
    }
    Ok(tally)
}
