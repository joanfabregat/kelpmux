//! File operations on the stream protocol (capability `files`).
use base64::{Engine, engine::general_purpose::STANDARD};
use kelpserver::native::{
    files::{self, Code, Encoding, Entry, Kind},
    protocol::*,
    server::{ServerConfig, serve},
    stream::{self, Event, Request},
};
use std::{
    fs,
    os::unix::{
        fs::{MetadataExt, PermissionsExt, symlink},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

struct Daemon {
    root: PathBuf,
    thread: Option<thread::JoinHandle<kelpserver::Result<()>>>,
}
impl Daemon {
    /// A server whose file root is the test's folder.
    fn start() -> Self {
        Self::start_with(|root| Some(root.to_path_buf()))
    }
    /// A server whose file root is `root(test folder)` (`None`: the home
    /// directory, the default).
    fn start_with(files_root: impl FnOnce(&Path) -> Option<PathBuf>) -> Self {
        Self::start_roots(|root| (files_root(root), Vec::new()))
    }
    /// A server whose file root and extra roots are `roots(test folder)`;
    /// `roots` may also create folders in the test folder first.
    fn start_roots(roots: impl FnOnce(&Path) -> (Option<PathBuf>, Vec<PathBuf>)) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "kelpserver-files-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        // Replies spell paths from the canonical root.
        let root = fs::canonicalize(&root).unwrap();
        let state = root.join("state");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(root.join("work")).unwrap();
        let (files_root, extra_roots) = roots(&root);
        let config = ServerConfig {
            socket: root.join("control.sock"),
            state_dir: state,
            shell: "/bin/sh".into(),
            cwd: root.clone(),
            cols: 80,
            rows: 24,
            uploads: Default::default(),
            files: files::Settings {
                trash_dir: Some(root.join("data/Trash")),
                root: files_root,
                extra_roots,
            },
        };
        let handle = thread::spawn(move || serve(config));
        let daemon = Self {
            root,
            thread: Some(handle),
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(daemon.root.join("control.sock")).is_err() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        daemon
    }
    /// The test's working directory.
    fn work(&self) -> PathBuf {
        self.root.join("work")
    }
    fn trash(&self) -> PathBuf {
        self.root.join("data/Trash")
    }
    fn peer(&self) -> Peer {
        let socket = UnixStream::connect(self.root.join("control.sock")).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let mut peer = Peer {
            socket,
            welcome: None,
            next_request: 0,
        };
        peer.send(Request::Hello {
            version: stream::VERSION,
        });
        loop {
            let event = peer.next();
            if matches!(event, Event::Welcome { .. }) {
                peer.welcome = Some(event);
                return peer;
            }
        }
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.thread.as_ref().is_some_and(|t| !t.is_finished()) {
            if Instant::now() >= deadline {
                if !thread::panicking() {
                    panic!("server did not stop");
                }
                return;
            }
            let stopped = (|| -> Option<()> {
                let mut s = UnixStream::connect(self.root.join("control.sock")).ok()?;
                s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                for request in [
                    Request::Hello {
                        version: stream::VERSION,
                    },
                    Request::Shutdown {
                        request: "drop".into(),
                    },
                ] {
                    write_message(&mut s, &ClientMessage::Stream(request)).ok()?;
                }
                loop {
                    if let ServerMessage::Stream(Event::Done { .. }) =
                        read_message::<ServerMessage>(&mut s).ok()??
                    {
                        return Some(());
                    }
                }
            })();
            if stopped.is_none() {
                thread::sleep(Duration::from_millis(100));
                continue;
            }
            let wait = Instant::now() + Duration::from_secs(5);
            while self.thread.as_ref().is_some_and(|t| !t.is_finished()) && Instant::now() < wait {
                thread::sleep(Duration::from_millis(10));
            }
        }
        if let Some(t) = self.thread.take() {
            assert!(t.join().unwrap().is_ok());
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Peer {
    socket: UnixStream,
    welcome: Option<Event>,
    next_request: u64,
}
impl Peer {
    fn send(&mut self, request: Request) {
        write_message(&mut self.socket, &ClientMessage::Stream(request)).unwrap();
    }
    fn next(&mut self) -> Event {
        loop {
            match read_message(&mut self.socket).unwrap().unwrap() {
                ServerMessage::Stream(Event::Update(_)) => continue,
                ServerMessage::Stream(event) => return event,
                other => panic!("non-stream message {other:?}"),
            }
        }
    }
    fn id(&mut self) -> String {
        self.next_request += 1;
        format!("f{}", self.next_request)
    }
    /// Send a request built for a fresh identifier and return its reply.
    fn call(&mut self, build: impl FnOnce(String) -> Request) -> Event {
        let id = self.id();
        self.send(build(id.clone()));
        loop {
            let event = self.next();
            if event.correlation() == Some(id.as_str()) {
                return event;
            }
        }
    }
    fn list(&mut self, path: &Path) -> (Vec<Entry>, bool) {
        match self.call(|request| Request::FileList {
            request,
            path: text(path),
        }) {
            Event::FileList {
                path: listed,
                entries,
                truncated,
                ..
            } => {
                assert_eq!(listed, text(path));
                (entries, truncated)
            }
            other => panic!("{other:?}"),
        }
    }
    fn stat(&mut self, path: &str) -> Result<(String, Entry), files::Error> {
        match self.call(|request| Request::FileStat {
            request,
            path: path.into(),
        }) {
            Event::FileStat { path, entry, .. } => Ok((path, entry)),
            Event::FileError { error, .. } => Err(error),
            other => panic!("{other:?}"),
        }
    }
    /// `(hash, binary, bytes)`.
    fn read(
        &mut self,
        path: &Path,
        max_bytes: Option<u64>,
    ) -> Result<(String, bool, Vec<u8>), files::Error> {
        match self.call(|request| Request::FileRead {
            request,
            path: text(path),
            max_bytes,
        }) {
            Event::FileContent {
                hash,
                binary,
                encoding,
                content,
                size,
                ..
            } => {
                let bytes = match encoding {
                    Encoding::Utf8 => content.into_bytes(),
                    Encoding::Base64 => STANDARD.decode(content).unwrap(),
                };
                assert_eq!(size, bytes.len() as u64);
                Ok((hash, binary, bytes))
            }
            Event::FileError { error, .. } => Err(error),
            other => panic!("{other:?}"),
        }
    }
    fn write(
        &mut self,
        path: &Path,
        content: &str,
        base_hash: Option<&str>,
        create: bool,
    ) -> Result<String, files::Error> {
        match self.call(|request| Request::FileWrite {
            request,
            path: text(path),
            content: content.into(),
            encoding: Encoding::Utf8,
            base_hash: base_hash.map(str::to_owned),
            create,
        }) {
            Event::FileWritten {
                hash,
                size,
                created,
                ..
            } => {
                assert_eq!(size, content.len() as u64);
                assert_eq!(created, create);
                Ok(hash)
            }
            Event::FileError { error, .. } => Err(error),
            other => panic!("{other:?}"),
        }
    }
    fn done(&mut self, build: impl FnOnce(String) -> Request) -> Result<(), files::Error> {
        match self.call(build) {
            Event::Done { .. } => Ok(()),
            Event::FileError { error, .. } => Err(error),
            other => panic!("{other:?}"),
        }
    }
    fn mkdir(&mut self, path: &Path) -> Result<(), files::Error> {
        self.done(|request| Request::FileMkdir {
            request,
            path: text(path),
        })
    }
    fn rename(&mut self, from: &Path, to: &Path, overwrite: bool) -> Result<(), files::Error> {
        self.done(|request| Request::FileRename {
            request,
            from: text(from),
            to: text(to),
            overwrite,
        })
    }
    /// `(removed with their trash paths, failed with their codes)`.
    fn remove(&mut self, trash: bool, paths: &[&Path]) -> Removal {
        let paths: Vec<String> = paths.iter().map(|path| text(path)).collect();
        let event = self.call(|request| {
            if trash {
                Request::FileTrash { request, paths }
            } else {
                Request::FileDelete { request, paths }
            }
        });
        match event {
            Event::FileRemoved {
                removed, failed, ..
            } => (
                removed
                    .into_iter()
                    .map(|item| (item.path, item.trashed_as))
                    .collect(),
                failed
                    .into_iter()
                    .map(|item| (item.path, item.error.code))
                    .collect(),
            ),
            other => panic!("{other:?}"),
        }
    }
}
fn text(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}
/// Removed paths with their trash paths, and failed paths with their codes.
type Removal = (Vec<(String, Option<String>)>, Vec<(String, Code)>);
/// SHA-256 of `abc` (FIPS 180-2 test vector).
const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().mode() & 0o7777
}

#[test]
fn files_capability_and_limits_are_advertised() {
    let daemon = Daemon::start();
    let peer = daemon.peer();
    match peer.welcome.clone().unwrap() {
        Event::Welcome {
            capabilities,
            file_limits,
            ..
        } => {
            assert!(capabilities.iter().any(|c| c == "files"));
            let limits = file_limits.unwrap();
            assert_eq!(
                limits,
                files::Limits {
                    root: Some(text(&daemon.root)),
                    roots: Some(vec![files::NamedRoot {
                        path: text(&daemon.root),
                        label: text(Path::new(daemon.root.file_name().unwrap())),
                    }]),
                    ..files::LIMITS
                }
            );
            assert_eq!(limits.max_read_bytes, 8 * 1024 * 1024);
            assert_eq!(limits.max_list_entries, 5000);
            assert_eq!(limits.max_in_flight, 4);
            // A base64 read of the largest file fits one record.
            assert!((limits.max_read_bytes as usize).div_ceil(3) * 4 + 4096 < MAX_MESSAGE);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn list_stat_read_and_write_round_trip() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let work = daemon.work();
    fs::write(work.join("b.txt"), "abc").unwrap();
    fs::write(work.join("a.sh"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(work.join("a.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir(work.join("zdir")).unwrap();
    fs::create_dir(work.join("adir")).unwrap();
    symlink("zdir", work.join("link")).unwrap();
    UnixListener::bind(work.join("sock")).unwrap();

    let (entries, truncated) = peer.list(&work);
    assert!(!truncated);
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    // Directories first, then everything else, each by name; the symlink to
    // a directory is listed as a symlink.
    assert_eq!(names, ["adir", "zdir", "a.sh", "b.txt", "link", "sock"]);
    let by = |name: &str| entries.iter().find(|e| e.name == name).unwrap().clone();
    assert_eq!(by("zdir").kind, Kind::Dir);
    assert_eq!(by("b.txt").kind, Kind::File);
    assert_eq!(by("b.txt").size, 3);
    assert_eq!(by("a.sh").mode, 0o755);
    assert_ne!(by("a.sh").mode & 0o111, 0);
    assert_eq!(by("link").kind, Kind::Symlink);
    assert_eq!(by("link").target.as_deref(), Some("zdir"));
    assert_eq!(by("sock").kind, Kind::Other);
    let modified = fs::metadata(work.join("b.txt")).unwrap().mtime();
    assert_eq!(by("b.txt").modified, modified);

    // Stat describes the link itself, and normalises the path.
    let (path, entry) = peer.stat(&format!("{}//./link/", text(&work))).unwrap();
    assert_eq!(path, text(&work.join("link")));
    assert_eq!(entry.kind, Kind::Symlink);
    // The root itself can be stat'ed; `/` is outside it.
    let (path, entry) = peer.stat(&text(&daemon.root)).unwrap();
    assert_eq!((path, entry.kind), (text(&daemon.root), Kind::Dir));
    assert_eq!(peer.stat("/").unwrap_err().code, Code::OutsideRoot);
    assert_eq!(
        peer.stat(&text(&work.join("missing"))).unwrap_err().code,
        Code::NotFound
    );

    // Read: the hash is SHA-256 of the content.
    let (hash, binary, bytes) = peer.read(&work.join("b.txt"), None).unwrap();
    assert_eq!(
        (hash.as_str(), binary, bytes.as_slice()),
        (ABC, false, &b"abc"[..])
    );
    // Listing through a symlinked directory and reading through a link work.
    fs::write(work.join("zdir/inner.txt"), "abc").unwrap();
    assert_eq!(peer.list(&work.join("link")).0[0].name, "inner.txt");
    assert_eq!(
        peer.read(&work.join("link/inner.txt"), None).unwrap().0,
        ABC
    );

    // Write with the read hash, then read back the new content.
    let new = peer
        .write(&work.join("b.txt"), "hello\n", Some(&hash), false)
        .unwrap();
    assert_eq!(fs::read_to_string(work.join("b.txt")).unwrap(), "hello\n");
    let (again, _, bytes) = peer.read(&work.join("b.txt"), None).unwrap();
    assert_eq!((again, bytes), (new.clone(), b"hello\n".to_vec()));
    // Without base_hash a write is unconditional.
    peer.write(&work.join("b.txt"), "abc", None, false).unwrap();
    assert_eq!(peer.read(&work.join("b.txt"), None).unwrap().0, ABC);

    // Directories and non-regular files have no content; writes need an
    // existing file unless `create`.
    assert_eq!(
        peer.read(&work.join("zdir"), None).unwrap_err().code,
        Code::IsADirectory
    );
    assert_eq!(
        peer.read(&work.join("sock"), None).unwrap_err().code,
        Code::NotAFile
    );
    assert_eq!(
        peer.write(&work.join("nope.txt"), "x", None, false)
            .unwrap_err()
            .code,
        Code::NotFound
    );
    assert_eq!(
        peer.write(&work.join("zdir"), "x", None, false)
            .unwrap_err()
            .code,
        Code::IsADirectory
    );
}

#[test]
fn a_stale_base_hash_is_a_conflict_carrying_the_current_hash() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let file = daemon.work().join("doc.txt");
    fs::write(&file, "first").unwrap();
    let (stale, _, _) = peer.read(&file, None).unwrap();
    fs::write(&file, "abc").unwrap();
    let error = peer.write(&file, "mine", Some(&stale), false).unwrap_err();
    assert_eq!(error.code, Code::Conflict);
    assert_eq!(error.current_hash.as_deref(), Some(ABC));
    assert_eq!(error.path.as_deref(), Some(text(&file).as_str()));
    assert_eq!(fs::read_to_string(&file).unwrap(), "abc");
    // The current hash, in any case, wins.
    peer.write(&file, "mine", Some(&ABC.to_uppercase()), false)
        .unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "mine");
    // A file now larger than any read is a conflict, without hashing it.
    let large = vec![b'a'; files::MAX_READ_BYTES as usize + 1];
    fs::write(&file, &large).unwrap();
    let error = peer.write(&file, "mine", Some(ABC), false).unwrap_err();
    assert_eq!(error.code, Code::Conflict);
    assert_eq!(error.current_hash, None);
    assert_eq!(error.size, Some(large.len() as u64));
    assert_eq!(fs::metadata(&file).unwrap().len(), large.len() as u64);
}

#[test]
fn writes_refuse_a_read_only_file_and_keep_the_group() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let work = daemon.work();
    let file = work.join("locked.txt");
    fs::write(&file, "keep").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o444)).unwrap();
    // Like a shell's `>`, which root may still use.
    if unsafe { libc::geteuid() } != 0 {
        let error = peer.write(&file, "lost", None, false).unwrap_err();
        assert_eq!(error.code, Code::PermissionDenied);
        assert_eq!(fs::read_to_string(&file).unwrap(), "keep");
        assert_eq!(mode(&file), 0o444);
    }
    // The group of a replaced file is kept when the user may set it: root
    // any group, others a supplementary group of theirs.
    let grouped = work.join("grouped.txt");
    fs::write(&grouped, "old").unwrap();
    let egid = unsafe { libc::getegid() };
    let target = if unsafe { libc::geteuid() } == 0 {
        Some(4242)
    } else {
        let mut groups = vec![0 as libc::gid_t; 256];
        let count = unsafe { libc::getgroups(groups.len() as libc::c_int, groups.as_mut_ptr()) };
        groups.truncate(count.max(0) as usize);
        groups.into_iter().find(|gid| *gid != egid)
    };
    let changed = target
        .filter(|gid| std::os::unix::fs::chown(&grouped, None, Some(*gid)).is_ok())
        .is_some();
    peer.write(&grouped, "new", None, false).unwrap();
    assert_eq!(fs::read_to_string(&grouped).unwrap(), "new");
    let expected = if changed { target.unwrap() } else { egid };
    assert_eq!(fs::metadata(&grouped).unwrap().gid(), expected);
}

#[test]
fn reading_a_fifo_never_opens_it() {
    use std::{
        os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let fifo = daemon.work().join("fifo");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    // A writer opening a FIFO blocks until a reader opens it: a read that
    // opened the FIFO before refusing it would release the writer.
    let released = Arc::new(AtomicBool::new(false));
    let writer = {
        let (fifo, released) = (fifo.clone(), released.clone());
        thread::spawn(move || {
            let file = fs::OpenOptions::new().write(true).open(&fifo).unwrap();
            released.store(true, Ordering::SeqCst);
            drop(file);
        })
    };
    thread::sleep(Duration::from_millis(100));
    assert_eq!(peer.read(&fifo, None).unwrap_err().code, Code::NotAFile);
    thread::sleep(Duration::from_millis(200));
    assert!(!released.load(Ordering::SeqCst));
    // Release the writer.
    let reader = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&fifo)
        .unwrap();
    writer.join().unwrap();
    drop(reader);
    assert!(released.load(Ordering::SeqCst));
}

#[test]
fn writes_are_atomic_replacements_keeping_the_mode() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let work = daemon.work();
    let file = work.join("script.sh");
    fs::write(&file, "old").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o750)).unwrap();
    let before = fs::metadata(&file).unwrap().ino();
    peer.write(&file, "new", None, false).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "new");
    assert_eq!(mode(&file), 0o750);
    // A replacement is a new inode renamed into place, with no temporary
    // file left behind.
    assert_ne!(fs::metadata(&file).unwrap().ino(), before);
    let names: Vec<_> = fs::read_dir(&work)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["script.sh"]);
    // Set-id bits are not carried over.
    fs::set_permissions(&file, fs::Permissions::from_mode(0o4755)).unwrap();
    peer.write(&file, "newer", None, false).unwrap();
    assert_eq!(mode(&file), 0o755);
}

#[test]
fn create_refuses_an_existing_entry() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let work = daemon.work();
    let file = work.join("new.txt");
    peer.write(&file, "abc", None, true).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "abc");
    // The new file is not private: 0666 less the server's umask.
    assert_ne!(mode(&file) & 0o600, 0);
    let error = peer.write(&file, "other", None, true).unwrap_err();
    assert_eq!(error.code, Code::Exists);
    assert_eq!(fs::read_to_string(&file).unwrap(), "abc");
    // A dangling symlink also occupies the name.
    symlink("nowhere", work.join("dangling")).unwrap();
    assert_eq!(
        peer.write(&work.join("dangling"), "x", None, true)
            .unwrap_err()
            .code,
        Code::Exists
    );
    assert!(!work.join("nowhere").exists());
    // base_hash names existing content; it cannot go with create.
    assert_eq!(
        peer.write(&work.join("other.txt"), "x", Some(ABC), true)
            .unwrap_err()
            .code,
        Code::InvalidRequest
    );
    assert_eq!(
        peer.write(&work.join("missing/x.txt"), "x", None, true)
            .unwrap_err()
            .code,
        Code::NotFound
    );
}

#[test]
fn mkdir_creates_one_directory() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let dir = daemon.work().join("made");
    peer.mkdir(&dir).unwrap();
    assert!(fs::symlink_metadata(&dir).unwrap().is_dir());
    assert_eq!(peer.mkdir(&dir).unwrap_err().code, Code::Exists);
    assert_eq!(
        peer.mkdir(&daemon.work().join("a/b")).unwrap_err().code,
        Code::NotFound
    );
    assert_eq!(
        peer.mkdir(Path::new("/")).unwrap_err().code,
        Code::OutsideRoot
    );
    assert_eq!(
        peer.mkdir(&daemon.root).unwrap_err().code,
        Code::InvalidRequest
    );
}

#[test]
fn rename_refuses_to_overwrite_unless_asked() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let work = daemon.work();
    fs::write(work.join("a.txt"), "a").unwrap();
    fs::write(work.join("b.txt"), "b").unwrap();
    fs::create_dir(work.join("dir")).unwrap();
    peer.rename(&work.join("a.txt"), &work.join("dir/a.txt"), false)
        .unwrap();
    assert_eq!(fs::read_to_string(work.join("dir/a.txt")).unwrap(), "a");
    assert!(!work.join("a.txt").exists());
    let error = peer
        .rename(&work.join("b.txt"), &work.join("dir/a.txt"), false)
        .unwrap_err();
    assert_eq!(error.code, Code::Exists);
    assert_eq!(
        error.path.as_deref(),
        Some(text(&work.join("dir/a.txt")).as_str())
    );
    assert_eq!(fs::read_to_string(work.join("b.txt")).unwrap(), "b");
    assert_eq!(fs::read_to_string(work.join("dir/a.txt")).unwrap(), "a");
    peer.rename(&work.join("b.txt"), &work.join("dir/a.txt"), true)
        .unwrap();
    assert_eq!(fs::read_to_string(work.join("dir/a.txt")).unwrap(), "b");
    // A directory moves whole; a symlink moves as a link.
    peer.rename(&work.join("dir"), &work.join("moved"), false)
        .unwrap();
    assert!(work.join("moved/a.txt").exists());
    symlink("moved", work.join("link")).unwrap();
    peer.rename(&work.join("link"), &work.join("link2"), false)
        .unwrap();
    assert_eq!(
        fs::read_link(work.join("link2")).unwrap(),
        Path::new("moved")
    );
    assert!(work.join("moved/a.txt").exists());
    assert_eq!(
        peer.rename(&work.join("missing"), &work.join("x"), false)
            .unwrap_err()
            .code,
        Code::NotFound
    );
}

#[test]
fn trash_writes_freedesktop_info_and_numbers_collisions() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let work = daemon.work();
    fs::create_dir(work.join("one")).unwrap();
    fs::create_dir(work.join("two")).unwrap();
    fs::write(work.join("one/a.txt"), "1").unwrap();
    fs::write(work.join("two/a.txt"), "2").unwrap();
    fs::create_dir(work.join("my dir")).unwrap();
    fs::write(work.join("my dir/inner"), "x").unwrap();
    let (removed, failed) = peer.remove(
        true,
        &[
            &work.join("one/a.txt"),
            &work.join("two/a.txt"),
            &work.join("my dir"),
            &work.join("missing"),
        ],
    );
    assert_eq!(failed, [(text(&work.join("missing")), Code::NotFound)]);
    let trash = daemon.trash();
    assert_eq!(
        removed,
        [
            (
                text(&work.join("one/a.txt")),
                Some(text(&trash.join("files/a.txt")))
            ),
            (
                text(&work.join("two/a.txt")),
                Some(text(&trash.join("files/a.2.txt")))
            ),
            (
                text(&work.join("my dir")),
                Some(text(&trash.join("files/my dir")))
            ),
        ]
    );
    assert!(!work.join("one/a.txt").exists());
    assert_eq!(fs::read_to_string(trash.join("files/a.txt")).unwrap(), "1");
    assert_eq!(
        fs::read_to_string(trash.join("files/a.2.txt")).unwrap(),
        "2"
    );
    assert_eq!(
        fs::read_to_string(trash.join("files/my dir/inner")).unwrap(),
        "x"
    );
    for (name, original) in [
        ("a.txt", work.join("one/a.txt")),
        ("a.2.txt", work.join("two/a.txt")),
        ("my dir", work.join("my dir")),
    ] {
        let info = fs::read_to_string(trash.join(format!("info/{name}.trashinfo"))).unwrap();
        let lines: Vec<_> = info.lines().collect();
        assert_eq!(lines.len(), 3, "{info}");
        assert_eq!(lines[0], "[Trash Info]");
        assert_eq!(
            lines[1],
            format!("Path={}", text(&original).replace(' ', "%20"))
        );
        let date = lines[2].strip_prefix("DeletionDate=").unwrap();
        assert_eq!(date.len(), 19, "{date}");
        assert!(date.as_bytes()[10] == b'T' && date.as_bytes()[4] == b'-');
    }
    assert_eq!(mode(&trash), 0o700);
    assert_eq!(mode(&trash.join("files")), 0o700);
    assert_eq!(mode(&trash.join("info")), 0o700);
    // An orphaned payload without info keeps its name: the next is numbered.
    fs::write(trash.join("files/b.txt"), "orphan").unwrap();
    fs::write(work.join("b.txt"), "b").unwrap();
    let (removed, _) = peer.remove(true, &[&work.join("b.txt")]);
    assert_eq!(removed[0].1, Some(text(&trash.join("files/b.2.txt"))));
    assert!(!trash.join("info/b.txt.trashinfo").exists());
    assert_eq!(
        fs::read_to_string(trash.join("files/b.txt")).unwrap(),
        "orphan"
    );
}

#[test]
fn trashing_a_symlink_moves_the_link_only() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let work = daemon.work();
    fs::create_dir(work.join("target")).unwrap();
    fs::write(work.join("target/keep"), "k").unwrap();
    symlink(work.join("target"), work.join("link")).unwrap();
    let (removed, failed) = peer.remove(true, &[&work.join("link")]);
    assert!(failed.is_empty());
    assert_eq!(removed.len(), 1);
    let moved = daemon.trash().join("files/link");
    assert!(
        fs::symlink_metadata(&moved)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(work.join("target/keep")).unwrap(), "k");
}

#[test]
fn delete_is_recursive_and_never_follows_symlinks() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let work = daemon.work();
    let outside = daemon.root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("precious"), "p").unwrap();
    fs::create_dir_all(work.join("tree/a/b/c")).unwrap();
    fs::write(work.join("tree/a/b/c/file"), "x").unwrap();
    fs::write(work.join("tree/top"), "x").unwrap();
    symlink(&outside, work.join("tree/a/escape")).unwrap();
    symlink(&outside, work.join("dirlink")).unwrap();
    fs::write(work.join("plain"), "x").unwrap();
    let (removed, failed) = peer.remove(
        false,
        &[
            &work.join("tree"),
            &work.join("dirlink"),
            &work.join("plain"),
            &work.join("missing"),
        ],
    );
    assert_eq!(
        removed,
        [
            (text(&work.join("tree")), None),
            (text(&work.join("dirlink")), None),
            (text(&work.join("plain")), None),
        ]
    );
    assert_eq!(failed, [(text(&work.join("missing")), Code::NotFound)]);
    assert_eq!(fs::read_dir(&work).unwrap().count(), 0);
    // Neither the link inside the tree nor the link to a directory was
    // followed.
    assert_eq!(fs::read_to_string(outside.join("precious")).unwrap(), "p");
    // Nothing went to the trash.
    assert!(!daemon.trash().exists());
}

#[test]
fn writes_refuse_a_symlink_as_the_final_component() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let work = daemon.work();
    fs::write(work.join("real.txt"), "real").unwrap();
    symlink("real.txt", work.join("alias.txt")).unwrap();
    let error = peer
        .write(&work.join("alias.txt"), "changed", None, false)
        .unwrap_err();
    assert_eq!(error.code, Code::Symlink);
    assert_eq!(fs::read_to_string(work.join("real.txt")).unwrap(), "real");
    assert!(
        fs::symlink_metadata(work.join("alias.txt"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    // Reading follows it, like a shell.
    assert_eq!(peer.read(&work.join("alias.txt"), None).unwrap().2, b"real");
}

#[test]
fn size_entry_and_path_limits_are_enforced() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let work = daemon.work();
    fs::write(work.join("small"), "0123456789abc").unwrap();
    let error = peer.read(&work.join("small"), Some(10)).unwrap_err();
    assert_eq!((error.code, error.size), (Code::TooLarge, Some(13)));
    assert_eq!(
        peer.read(&work.join("small"), Some(13)).unwrap().2.len(),
        13
    );
    let big = work.join("big");
    fs::write(&big, vec![b'a'; files::MAX_READ_BYTES as usize + 1]).unwrap();
    let error = peer.read(&big, Some(u64::MAX)).unwrap_err();
    assert_eq!(error.code, Code::TooLarge);
    fs::remove_file(&big).unwrap();
    // The largest text read fits one record.
    fs::write(&big, vec![b'a'; files::MAX_READ_BYTES as usize]).unwrap();
    assert_eq!(
        peer.read(&big, None).unwrap().2.len() as u64,
        files::MAX_READ_BYTES
    );
    let content = "a".repeat(files::MAX_WRITE_BYTES as usize + 1);
    assert_eq!(
        peer.write(&work.join("small"), &content, None, false)
            .unwrap_err()
            .code,
        Code::TooLarge
    );
    assert_eq!(
        fs::read_to_string(work.join("small")).unwrap(),
        "0123456789abc"
    );

    let many = work.join("many");
    fs::create_dir(&many).unwrap();
    for n in 0..=files::MAX_LIST_ENTRIES {
        fs::write(many.join(format!("{n:05}")), "").unwrap();
    }
    fs::create_dir(many.join("zz-dir")).unwrap();
    let (entries, truncated) = peer.list(&many);
    assert!(truncated);
    assert_eq!(entries.len(), files::MAX_LIST_ENTRIES);
    // Directories sort first, so a truncated listing keeps them; then the
    // first names in order, whatever order the directory returned them in.
    assert_eq!(entries[0].name, "zz-dir");
    assert_eq!(entries[1].name, "00000");
    assert_eq!(
        entries[files::MAX_LIST_ENTRIES - 1].name,
        format!("{:05}", files::MAX_LIST_ENTRIES - 2)
    );

    let paths: Vec<String> = (0..=files::MAX_PATHS)
        .map(|n| text(&work.join(n.to_string())))
        .collect();
    match peer.call(|request| Request::FileDelete { request, paths }) {
        Event::FileError { error, .. } => assert_eq!(error.code, Code::InvalidRequest),
        other => panic!("{other:?}"),
    }
    match peer.call(|request| Request::FileTrash {
        request,
        paths: vec![],
    }) {
        Event::FileError { error, .. } => assert_eq!(error.code, Code::InvalidRequest),
        other => panic!("{other:?}"),
    }
    // One invalid path refuses the whole request before anything is removed.
    match peer.call(|request| Request::FileDelete {
        request,
        paths: vec![text(&work.join("small")), "relative".into()],
    }) {
        Event::FileError { error, .. } => {
            assert_eq!(error.code, Code::InvalidPath);
            assert_eq!(error.path.as_deref(), Some("relative"));
        }
        other => panic!("{other:?}"),
    }
    assert!(work.join("small").exists());
}

#[test]
fn binary_files_round_trip_as_base64() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let file = daemon.work().join("blob.bin");
    let bytes: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
    fs::write(&file, &bytes).unwrap();
    let (hash, binary, read) = peer.read(&file, None).unwrap();
    assert!(binary);
    assert_eq!(read, bytes);
    let replacement: Vec<u8> = vec![0, 159, 146, 150, 255];
    match peer.call(|request| Request::FileWrite {
        request,
        path: text(&file),
        content: STANDARD.encode(&replacement),
        encoding: Encoding::Base64,
        base_hash: Some(hash),
        create: false,
    }) {
        Event::FileWritten { size, .. } => assert_eq!(size, 5),
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(&file).unwrap(), replacement);
    match peer.call(|request| Request::FileWrite {
        request,
        path: text(&file),
        content: "not base64!".into(),
        encoding: Encoding::Base64,
        base_hash: None,
        create: false,
    }) {
        Event::FileError { error, .. } => assert_eq!(error.code, Code::InvalidContent),
        other => panic!("{other:?}"),
    }
    // Invalid UTF-8 without NUL is binary too.
    fs::write(&file, [b'a', 0xff, b'b']).unwrap();
    let (_, binary, read) = peer.read(&file, None).unwrap();
    assert!(binary);
    assert_eq!(read, [b'a', 0xff, b'b']);
}

#[test]
fn home_relative_and_invalid_paths() {
    // The default root: the home directory, which `~` names.
    let Some(home) = files::home().and_then(|home| fs::canonicalize(home).ok()) else {
        return;
    };
    let daemon = Daemon::start_with(|_| None);
    let mut peer = daemon.peer();
    match peer.welcome.clone().unwrap() {
        Event::Welcome { file_limits, .. } => {
            let limits = file_limits.unwrap();
            assert_eq!(limits.root, Some(text(&home)));
            // The home directory is labelled as such.
            assert_eq!(
                limits.roots,
                Some(vec![files::NamedRoot {
                    path: text(&home),
                    label: "Home".into(),
                }])
            );
        }
        other => panic!("{other:?}"),
    }
    let (path, entry) = peer.stat("~").unwrap();
    assert_eq!(Path::new(&path), home);
    assert_eq!(entry.kind, Kind::Dir);
    let (path, _) = peer.stat("~/").unwrap();
    assert_eq!(Path::new(&path), home);
    match peer.call(|request| Request::FileList {
        request,
        path: "~".into(),
    }) {
        Event::FileList { path, .. } => assert_eq!(Path::new(&path), home),
        other => panic!("{other:?}"),
    }
    // Outside the home directory, nothing is reachable.
    if home != Path::new("/") {
        assert_eq!(peer.stat("/").unwrap_err().code, Code::OutsideRoot);
    }
    for bad in ["relative/x", "/a/../b", "~someone/x", ""] {
        let error = peer.stat(bad).unwrap_err();
        assert_eq!(error.code, Code::InvalidPath, "{bad:?}");
        assert_eq!(error.path.as_deref(), Some(bad));
    }
}

#[test]
fn concurrent_operations_all_complete() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let work = daemon.work();
    fs::write(work.join("x"), "abc").unwrap();
    // More requests than the per-connection bound: each gets a reply, a
    // content or `busy`, and the server keeps serving.
    let ids: Vec<String> = (0..12).map(|_| peer.id()).collect();
    for id in &ids {
        peer.send(Request::FileRead {
            request: id.clone(),
            path: text(&work.join("x")),
            max_bytes: None,
        });
    }
    let mut pending: std::collections::BTreeSet<String> = ids.into_iter().collect();
    let mut contents = 0;
    while !pending.is_empty() {
        match peer.next() {
            Event::FileContent { request, hash, .. } => {
                assert_eq!(hash, ABC);
                assert!(pending.remove(&request));
                contents += 1;
            }
            Event::FileError { request, error, .. } => {
                assert_eq!(error.code, Code::Busy);
                assert!(pending.remove(&request));
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(contents >= files::MAX_IN_FLIGHT);
    assert_eq!(peer.read(&work.join("x"), None).unwrap().0, ABC);
}

#[test]
fn concurrent_largest_reads_all_arrive() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let work = daemon.work();
    // Binary content travels as base64: each reply is about 11.2 MiB, so
    // several together exceed one connection's output queue and must wait
    // for it to drain instead of disconnecting the client.
    let bytes: Vec<u8> = (0..=255u8)
        .cycle()
        .take(files::MAX_READ_BYTES as usize)
        .collect();
    fs::write(work.join("blob"), &bytes).unwrap();
    let ids: Vec<String> = (0..files::MAX_IN_FLIGHT).map(|_| peer.id()).collect();
    for id in &ids {
        peer.send(Request::FileRead {
            request: id.clone(),
            path: text(&work.join("blob")),
            max_bytes: None,
        });
    }
    let mut pending: std::collections::BTreeSet<String> = ids.into_iter().collect();
    while !pending.is_empty() {
        match peer.next() {
            Event::FileContent {
                request,
                encoding,
                content,
                ..
            } => {
                assert_eq!(encoding, Encoding::Base64);
                assert_eq!(STANDARD.decode(content).unwrap(), bytes);
                assert!(pending.remove(&request));
            }
            other => panic!("{other:?}"),
        }
    }
    fs::write(work.join("x"), "abc").unwrap();
    assert_eq!(peer.read(&work.join("x"), None).unwrap().0, ABC);
}

#[test]
fn a_burst_of_refused_requests_keeps_the_connection() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let work = daemon.work();
    // Far more requests than run at once: the refusals (`busy`) must not
    // overflow the output queue, which would disconnect the client.
    // One write, so the server reads them all in one pass.
    let ids: Vec<String> = (0..64).map(|_| peer.id()).collect();
    let mut burst = Vec::new();
    for id in &ids {
        let request = Request::FileStat {
            request: id.clone(),
            path: text(&work),
        };
        write_message(&mut burst, &ClientMessage::Stream(request)).unwrap();
    }
    std::io::Write::write_all(&mut peer.socket, &burst).unwrap();
    let mut pending: std::collections::BTreeSet<String> = ids.into_iter().collect();
    let mut busy = 0;
    while !pending.is_empty() {
        match peer.next() {
            Event::FileStat { request, .. } => assert!(pending.remove(&request)),
            Event::FileError { request, error, .. } => {
                assert_eq!(error.code, Code::Busy);
                assert!(pending.remove(&request));
                busy += 1;
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(busy > 0);
    let (_, entry) = peer.stat(&text(&work)).unwrap();
    assert_eq!(entry.kind, Kind::Dir);
}

/// A server confined to `work`, with a sibling folder `outside` holding
/// `secret.txt`.
fn confined() -> (Daemon, PathBuf, PathBuf) {
    let daemon = Daemon::start_with(|root| Some(root.join("work")));
    let work = daemon.work();
    let outside = daemon.root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret.txt"), "secret").unwrap();
    (daemon, work, outside)
}
fn code_of(event: Event) -> Code {
    match event {
        Event::FileError { error, .. } => error.code,
        other => panic!("{other:?}"),
    }
}

#[test]
fn every_operation_outside_the_root_is_refused() {
    let (daemon, work, outside) = confined();
    let mut peer = daemon.peer();
    match peer.welcome.clone().unwrap() {
        Event::Welcome { file_limits, .. } => {
            assert_eq!(file_limits.unwrap().root, Some(text(&work)));
        }
        other => panic!("{other:?}"),
    }
    fs::write(work.join("a.txt"), "a").unwrap();
    let secret = outside.join("secret.txt");
    // A sibling whose name starts like the root's is outside too.
    let sibling = daemon.root.join("workx");
    fs::create_dir(&sibling).unwrap();
    fs::write(sibling.join("b.txt"), "b").unwrap();
    for path in [
        &outside,
        &secret,
        &sibling,
        &sibling.join("b.txt"),
        &daemon.root,
    ] {
        let error = peer.stat(&text(path)).unwrap_err();
        assert_eq!(error.code, Code::OutsideRoot, "{path:?}");
        assert_eq!(error.path.as_deref(), Some(text(path).as_str()));
    }
    assert_eq!(
        code_of(peer.call(|request| Request::FileList {
            request,
            path: text(&outside),
        })),
        Code::OutsideRoot
    );
    assert_eq!(
        peer.read(&secret, None).unwrap_err().code,
        Code::OutsideRoot
    );
    assert_eq!(
        peer.write(&secret, "changed", None, false)
            .unwrap_err()
            .code,
        Code::OutsideRoot
    );
    assert_eq!(
        peer.write(&outside.join("new.txt"), "x", None, true)
            .unwrap_err()
            .code,
        Code::OutsideRoot
    );
    assert_eq!(
        peer.mkdir(&outside.join("d")).unwrap_err().code,
        Code::OutsideRoot
    );
    // Neither end of a rename may be outside.
    assert_eq!(
        peer.rename(&work.join("a.txt"), &outside.join("a.txt"), false)
            .unwrap_err()
            .code,
        Code::OutsideRoot
    );
    assert_eq!(
        peer.rename(&secret, &work.join("secret.txt"), false)
            .unwrap_err()
            .code,
        Code::OutsideRoot
    );
    // Trash and delete check every path first: one outside refuses all.
    for trash in [true, false] {
        let paths = vec![text(&work.join("a.txt")), text(&secret)];
        let event = peer.call(|request| {
            if trash {
                Request::FileTrash { request, paths }
            } else {
                Request::FileDelete { request, paths }
            }
        });
        assert_eq!(code_of(event), Code::OutsideRoot);
    }
    assert!(work.join("a.txt").exists());
    assert_eq!(fs::read_to_string(&secret).unwrap(), "secret");
    assert!(!outside.join("new.txt").exists() && !outside.join("d").exists());
    // `..` is never accepted, and `~` (the home directory) is outside here.
    let error = peer
        .stat(&format!("{}/../outside/secret.txt", text(&work)))
        .unwrap_err();
    assert_eq!(error.code, Code::InvalidPath);
    if files::home().is_some_and(|home| !work.starts_with(home)) {
        assert_eq!(peer.stat("~").unwrap_err().code, Code::OutsideRoot);
    }
}

#[test]
fn the_root_can_be_browsed_but_not_changed() {
    let (daemon, work, _) = confined();
    let mut peer = daemon.peer();
    fs::write(work.join("a.txt"), "a").unwrap();
    let (entries, _) = peer.list(&work);
    assert_eq!(entries.len(), 1);
    let (path, entry) = peer.stat(&text(&work)).unwrap();
    assert_eq!(
        (path, entry.kind, entry.name),
        (text(&work), Kind::Dir, "work".into())
    );
    for trash in [true, false] {
        let (removed, failed) = peer.remove(trash, &[&work]);
        assert!(removed.is_empty());
        assert_eq!(failed, [(text(&work), Code::InvalidRequest)]);
    }
    assert_eq!(
        peer.rename(&work, &work.join("moved"), false)
            .unwrap_err()
            .code,
        Code::InvalidRequest
    );
    assert_eq!(
        peer.rename(&work.join("a.txt"), &work, true)
            .unwrap_err()
            .code,
        Code::InvalidRequest
    );
    assert_eq!(
        peer.write(&work, "x", None, false).unwrap_err().code,
        Code::InvalidRequest
    );
    assert!(work.join("a.txt").exists());
    // A symlink to the root inside it is only a link: removing it keeps the
    // root and its contents.
    symlink(&work, work.join("self")).unwrap();
    let (removed, failed) = peer.remove(false, &[&work.join("self")]);
    assert_eq!((removed.len(), failed.len()), (1, 0));
    assert!(work.join("a.txt").exists());
}

#[test]
fn symlinks_out_of_the_root_are_listed_but_never_followed() {
    let (daemon, work, outside) = confined();
    let mut peer = daemon.peer();
    // Links out: absolute and relative, to a folder and to a file, and to
    // /etc.
    symlink(&outside, work.join("out")).unwrap();
    symlink("../outside", work.join("rel-out")).unwrap();
    symlink(outside.join("secret.txt"), work.join("secret-link")).unwrap();
    symlink("/etc", work.join("etc")).unwrap();
    // Links in: absolute and relative.
    fs::create_dir(work.join("dir")).unwrap();
    fs::write(work.join("dir/f.txt"), "inside").unwrap();
    symlink(work.join("dir"), work.join("in")).unwrap();
    symlink("dir/f.txt", work.join("file-in")).unwrap();
    symlink("../dir", work.join("dir/up")).unwrap();

    // Listed and stat'ed as links, with their targets.
    let (entries, _) = peer.list(&work);
    let target = |name: &str| {
        let entry = entries.iter().find(|entry| entry.name == name).unwrap();
        assert_eq!(entry.kind, Kind::Symlink, "{name}");
        entry.target.clone().unwrap()
    };
    assert_eq!(target("out"), text(&outside));
    assert_eq!(target("rel-out"), "../outside");
    assert_eq!(target("etc"), "/etc");
    let (_, entry) = peer.stat(&text(&work.join("etc"))).unwrap();
    assert_eq!(
        (entry.kind, entry.target.as_deref()),
        (Kind::Symlink, Some("/etc"))
    );

    // Nothing is reached through them.
    for link in ["out", "rel-out"] {
        let through = work.join(link);
        let listed = peer.call(|request| Request::FileList {
            request,
            path: text(&through),
        });
        assert_eq!(code_of(listed), Code::OutsideRoot, "{link}");
        let secret = through.join("secret.txt");
        assert_eq!(
            peer.stat(&text(&secret)).unwrap_err().code,
            Code::OutsideRoot
        );
        assert_eq!(
            peer.read(&secret, None).unwrap_err().code,
            Code::OutsideRoot
        );
        assert_eq!(
            peer.write(&secret, "x", None, false).unwrap_err().code,
            Code::OutsideRoot
        );
        assert_eq!(
            peer.write(&through.join("new.txt"), "x", None, true)
                .unwrap_err()
                .code,
            Code::OutsideRoot
        );
        assert_eq!(
            peer.mkdir(&through.join("d")).unwrap_err().code,
            Code::OutsideRoot
        );
        assert_eq!(
            peer.rename(&work.join("dir/f.txt"), &through.join("f.txt"), false)
                .unwrap_err()
                .code,
            Code::OutsideRoot
        );
        let (removed, failed) = peer.remove(false, &[&secret]);
        assert!(removed.is_empty());
        assert_eq!(failed, [(text(&secret), Code::OutsideRoot)]);
    }
    assert_eq!(
        peer.read(&work.join("secret-link"), None).unwrap_err().code,
        Code::OutsideRoot
    );
    assert_eq!(
        peer.read(&work.join("etc/passwd"), None).unwrap_err().code,
        Code::OutsideRoot
    );
    // Writing to a final symlink is refused as before, wherever it points.
    assert_eq!(
        peer.write(&work.join("secret-link"), "x", None, false)
            .unwrap_err()
            .code,
        Code::Symlink
    );
    assert_eq!(
        fs::read_to_string(outside.join("secret.txt")).unwrap(),
        "secret"
    );
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    assert!(work.join("dir/f.txt").exists());

    // Links that stay inside work like folders and files.
    assert_eq!(
        peer.read(&work.join("in/f.txt"), None).unwrap().2,
        b"inside"
    );
    assert_eq!(peer.read(&work.join("file-in"), None).unwrap().2, b"inside");
    assert_eq!(
        peer.read(&work.join("in/up/up/f.txt"), None).unwrap().2,
        b"inside"
    );
    assert_eq!(peer.list(&work.join("in")).0.len(), 2);
    peer.write(&work.join("in/g.txt"), "g", None, true).unwrap();
    assert_eq!(fs::read_to_string(work.join("dir/g.txt")).unwrap(), "g");
    // Deleting or trashing a link out removes the link only.
    let (removed, _) = peer.remove(true, &[&work.join("out")]);
    assert_eq!(removed.len(), 1);
    let (removed, _) = peer.remove(false, &[&work.join("rel-out")]);
    assert_eq!(removed.len(), 1);
    assert!(outside.join("secret.txt").exists());
}

/// A server confined to `work`, with the extra roots `extra`, `a/data` and
/// `b/data` (same name), and configured extra roots that are skipped: one
/// missing, a file, one inside `work`, one inside `extra`, a repeat of
/// `extra` spelled through a symlink, and the test folder (which contains
/// `work`). `outside` holds `secret.txt`.
fn with_extras() -> (Daemon, PathBuf, PathBuf, PathBuf) {
    let daemon = Daemon::start_roots(|root| {
        for dir in ["extra/sub", "a/data", "b/data", "work/inner", "outside"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(root.join("outside/secret.txt"), "secret").unwrap();
        fs::write(root.join("a-file"), "x").unwrap();
        symlink(root.join("extra"), root.join("extra-link")).unwrap();
        (
            Some(root.join("work")),
            vec![
                root.join("extra"),
                root.join("missing"),
                root.join("a-file"),
                root.join("work/inner"),
                root.join("extra/sub"),
                root.join("extra-link"),
                root.to_path_buf(),
                root.join("a/data"),
                root.join("b/data"),
            ],
        )
    });
    let (work, extra, outside) = (
        daemon.work(),
        daemon.root.join("extra"),
        daemon.root.join("outside"),
    );
    (daemon, work, extra, outside)
}

#[test]
fn extra_roots_are_advertised_without_skipped_ones() {
    let (daemon, work, extra, _) = with_extras();
    let peer = daemon.peer();
    let Event::Welcome { file_limits, .. } = peer.welcome.clone().unwrap() else {
        unreachable!()
    };
    let limits = file_limits.unwrap();
    // `root` stays the primary root, for older clients.
    assert_eq!(limits.root, Some(text(&work)));
    let named = |path: &Path, label: &str| files::NamedRoot {
        path: text(path),
        label: label.into(),
    };
    let (a, b) = (daemon.root.join("a/data"), daemon.root.join("b/data"));
    assert_eq!(
        limits.roots.unwrap(),
        [
            named(&work, "work"),
            named(&extra, "extra"),
            // The same name twice: each is labelled with its path.
            named(&a, &text(&a)),
            named(&b, &text(&b)),
        ]
    );
}

#[test]
fn every_operation_works_inside_an_extra_root() {
    let (daemon, work, extra, outside) = with_extras();
    let mut peer = daemon.peer();
    // In the extra root, as in the primary one.
    peer.write(&extra.join("a.txt"), "a", None, true).unwrap();
    assert_eq!(peer.read(&extra.join("a.txt"), None).unwrap().2, b"a");
    peer.mkdir(&extra.join("d")).unwrap();
    peer.rename(&extra.join("a.txt"), &extra.join("d/b.txt"), false)
        .unwrap();
    let (entries, _) = peer.list(&extra.join("d"));
    assert_eq!(entries.len(), 1);
    let (path, entry) = peer.stat(&text(&extra)).unwrap();
    assert_eq!((path, entry.kind), (text(&extra), Kind::Dir));
    // A skipped root's folder is reached only where another root holds it.
    let (path, _) = peer.stat(&text(&extra.join("sub"))).unwrap();
    assert_eq!(path, text(&extra.join("sub")));
    peer.write(&work.join("inner/c.txt"), "c", None, true)
        .unwrap();
    // The trash (outside every root here) takes entries of the primary root
    // only: trashing would move this one out of its root.
    let (removed, failed) = peer.remove(true, &[&extra.join("d/b.txt")]);
    assert!(removed.is_empty());
    assert_eq!(failed, [(text(&extra.join("d/b.txt")), Code::CrossRoot)]);
    assert!(extra.join("d/b.txt").exists());
    let (removed, failed) = peer.remove(true, &[&work.join("inner/c.txt")]);
    assert_eq!((removed.len(), failed.len()), (1, 0));
    let (removed, failed) = peer.remove(false, &[&extra.join("d")]);
    assert_eq!((removed.len(), failed.len()), (1, 0));
    assert!(!extra.join("d").exists());
    // Outside every root, and the skipped test folder, nothing.
    for path in [
        outside.join("secret.txt"),
        daemon.root.clone(),
        daemon.root.join("a-file"),
        daemon.root.join("missing"),
    ] {
        assert_eq!(
            peer.stat(&text(&path)).unwrap_err().code,
            Code::OutsideRoot,
            "{path:?}"
        );
    }
    // An extra root itself is browsed, never changed.
    for trash in [true, false] {
        let (removed, failed) = peer.remove(trash, &[&extra]);
        assert!(removed.is_empty());
        assert_eq!(failed, [(text(&extra), Code::InvalidRequest)]);
    }
    assert_eq!(
        peer.rename(&extra, &extra.join("moved"), false)
            .unwrap_err()
            .code,
        Code::InvalidRequest
    );
    assert!(extra.is_dir());
}

#[test]
fn renames_between_roots_are_refused() {
    let (daemon, work, extra, _) = with_extras();
    let mut peer = daemon.peer();
    fs::write(work.join("a.txt"), "a").unwrap();
    fs::write(extra.join("b.txt"), "b").unwrap();
    for (from, to) in [
        (work.join("a.txt"), extra.join("a.txt")),
        (extra.join("b.txt"), work.join("b.txt")),
        (extra.join("b.txt"), daemon.root.join("a/data/b.txt")),
    ] {
        for overwrite in [false, true] {
            let error = peer.rename(&from, &to, overwrite).unwrap_err();
            assert_eq!(error.code, Code::CrossRoot, "{from:?} -> {to:?}");
            assert_eq!(error.path.as_deref(), Some(text(&to).as_str()));
        }
    }
    assert!(work.join("a.txt").exists() && extra.join("b.txt").exists());
    assert!(!work.join("b.txt").exists() && !extra.join("a.txt").exists());
}

#[test]
fn symlinks_never_lead_from_one_root_into_another() {
    let (daemon, work, extra, outside) = with_extras();
    let mut peer = daemon.peer();
    fs::create_dir(work.join("dir")).unwrap();
    fs::write(work.join("dir/f.txt"), "primary").unwrap();
    fs::write(extra.join("g.txt"), "extra").unwrap();
    // From the extra root into the primary one (absolute and relative),
    // outside, and to /etc; from the primary root into the extra one.
    symlink(work.join("dir"), extra.join("to-work")).unwrap();
    symlink("../work/dir", extra.join("rel-work")).unwrap();
    symlink(work.join("dir/f.txt"), extra.join("work-file")).unwrap();
    symlink(&outside, extra.join("out")).unwrap();
    symlink("/etc", extra.join("etc")).unwrap();
    symlink(&extra, work.join("to-extra")).unwrap();
    symlink("../extra/g.txt", work.join("extra-file")).unwrap();
    // And one that stays inside the extra root.
    symlink("g.txt", extra.join("in")).unwrap();
    for link in [
        extra.join("to-work"),
        extra.join("rel-work"),
        extra.join("out"),
        work.join("to-extra"),
    ] {
        let listed = peer.call(|request| Request::FileList {
            request,
            path: text(&link),
        });
        assert_eq!(code_of(listed), Code::OutsideRoot, "{link:?}");
        for name in ["f.txt", "g.txt", "secret.txt"] {
            let through = link.join(name);
            assert_eq!(
                peer.read(&through, None).unwrap_err().code,
                Code::OutsideRoot,
                "{through:?}"
            );
        }
        assert_eq!(
            peer.write(&link.join("new.txt"), "x", None, true)
                .unwrap_err()
                .code,
            Code::OutsideRoot
        );
        assert_eq!(
            peer.mkdir(&link.join("d")).unwrap_err().code,
            Code::OutsideRoot
        );
    }
    for file in [
        extra.join("work-file"),
        extra.join("etc/passwd"),
        work.join("extra-file"),
    ] {
        assert_eq!(
            peer.read(&file, None).unwrap_err().code,
            Code::OutsideRoot,
            "{file:?}"
        );
    }
    // Listed as links; the link inside the extra root is followed.
    let (entries, _) = peer.list(&extra);
    let to_work = entries.iter().find(|e| e.name == "to-work").unwrap();
    assert_eq!(to_work.kind, Kind::Symlink);
    assert_eq!(peer.read(&extra.join("in"), None).unwrap().2, b"extra");
    assert!(!work.join("dir/new.txt").exists() && !work.join("dir/d").exists());
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
}
