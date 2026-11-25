//! File downloads on the stream protocol (capability `file-downloads`).
use base64::{Engine, engine::general_purpose::STANDARD};
use kelpserver::native::{
    downloads::{self, Archive},
    files::{self, Code},
    protocol::*,
    server::{ServerConfig, serve},
    stream::{self, Event, Request},
};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    os::unix::{
        ffi::OsStrExt,
        fs::{PermissionsExt, symlink},
        net::UnixStream,
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
    fn start() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "kelpserver-downloads-{}-{}",
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
        fs::create_dir(root.join("extra")).unwrap();
        let config = ServerConfig {
            socket: root.join("control.sock"),
            state_dir: state,
            shell: "/bin/sh".into(),
            cwd: root.clone(),
            cols: 80,
            rows: 24,
            uploads: Default::default(),
            // The file root is the test's working folder, with `extra` as
            // an extra root.
            files: files::Settings {
                trash_dir: Some(root.join("data/Trash")),
                root: Some(root.join("work")),
                extra_roots: vec![root.join("extra")],
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
    fn work(&self) -> PathBuf {
        self.root.join("work")
    }
    fn extra(&self) -> PathBuf {
        self.root.join("extra")
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
/// A finished download: its start, its bytes and its end.
struct Downloaded {
    name: String,
    size: Option<u64>,
    archive: Option<Archive>,
    entries: Option<u64>,
    input_bytes: Option<u64>,
    bytes: Vec<u8>,
    hash: String,
    skipped: u64,
    changed: u64,
}
impl Peer {
    fn send(&mut self, request: Request) {
        write_message(&mut self.socket, &ClientMessage::Stream(request)).unwrap();
    }
    /// The next stream event, updates included.
    fn any(&mut self) -> Event {
        match read_message(&mut self.socket).unwrap().unwrap() {
            ServerMessage::Stream(event) => event,
            other => panic!("non-stream message {other:?}"),
        }
    }
    fn next(&mut self) -> Event {
        loop {
            match self.any() {
                Event::Update(_) => continue,
                event => return event,
            }
        }
    }
    fn id(&mut self) -> String {
        self.next_request += 1;
        format!("d{}", self.next_request)
    }
    /// Begin a download: `Ok(download)` once started, or its error.
    fn begin(
        &mut self,
        paths: &[&Path],
        archive: Option<Archive>,
    ) -> Result<(u64, Event), files::Error> {
        let request = self.id();
        self.send(Request::FileDownloadBegin {
            request: request.clone(),
            paths: paths.iter().map(|path| text(path)).collect(),
            archive,
        });
        loop {
            let event = self.next();
            if event.correlation() != Some(request.as_str()) {
                continue;
            }
            return match event {
                Event::FileDownloadStarted { download, .. } => Ok((download, event)),
                Event::FileError {
                    error, download, ..
                } => {
                    assert_eq!(
                        download, None,
                        "an error before the start names no download"
                    );
                    Err(error)
                }
                other => panic!("{other:?}"),
            };
        }
    }
    /// A whole download, acknowledging every chunk.
    fn download(
        &mut self,
        paths: &[&Path],
        archive: Option<Archive>,
    ) -> Result<Downloaded, files::Error> {
        let (download, started) = self.begin(paths, archive)?;
        let Event::FileDownloadStarted {
            name,
            size,
            archive,
            entries,
            input_bytes,
            ..
        } = started
        else {
            unreachable!()
        };
        let mut bytes = Vec::new();
        loop {
            match self.next() {
                Event::FileDownloadChunk {
                    download: d,
                    offset,
                    data,
                    ..
                } if d == download => {
                    assert_eq!(offset, bytes.len() as u64);
                    let data = STANDARD.decode(data).unwrap();
                    assert!(!data.is_empty() && data.len() <= downloads::CHUNK_BYTES);
                    bytes.extend_from_slice(&data);
                    self.send(Request::FileDownloadAck {
                        download,
                        offset: bytes.len() as u64,
                    });
                }
                Event::FileDownloadDone {
                    download: d,
                    size: total,
                    hash,
                    skipped,
                    changed,
                    ..
                } if d == download => {
                    assert_eq!(total, bytes.len() as u64);
                    return Ok(Downloaded {
                        name,
                        size,
                        archive,
                        entries,
                        input_bytes,
                        bytes,
                        hash,
                        skipped,
                        changed,
                    });
                }
                Event::FileError {
                    error,
                    download: Some(d),
                    ..
                } if d == download => return Err(error),
                _ => {}
            }
        }
    }
    /// A file's SHA-256 through `file_read`.
    fn hash(&mut self, path: &Path) -> String {
        let request = self.id();
        self.send(Request::FileRead {
            request: request.clone(),
            path: text(path),
            max_bytes: None,
        });
        loop {
            match self.next() {
                Event::FileContent {
                    request: r, hash, ..
                } if r == request => return hash,
                _ => {}
            }
        }
    }
    fn cancel(&mut self, download: u64) {
        let request = self.id();
        self.send(Request::FileDownloadCancel {
            request: request.clone(),
            download,
        });
        loop {
            match self.next() {
                Event::Done { request: r } if r == request => return,
                Event::FileDownloadChunk { download: d, .. } if d == download => {}
                Event::FileDownloadDone { download: d, .. } if d == download => {
                    panic!("a cancelled download finished")
                }
                _ => {}
            }
        }
    }
}
fn text(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}
/// Bytes that do not compress well, so archives stay large.
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

/// One archive member as a test reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Member {
    kind: u8,
    mode: u32,
    mtime: i64,
    data: Vec<u8>,
    link: Vec<u8>,
}
/// Decode a tar.gz: gunzip with flate2, then read ustar blocks and pax
/// extended headers by hand. Members by path, in archive order.
fn untar(gz: &[u8]) -> Vec<(Vec<u8>, Member)> {
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(gz)
        .read_to_end(&mut tar)
        .unwrap();
    assert_eq!(tar.len() % 512, 0);
    let field = |block: &[u8], range: std::ops::Range<usize>| -> Vec<u8> {
        let bytes = &block[range];
        bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())].to_vec()
    };
    let number = |block: &[u8], range: std::ops::Range<usize>| -> u64 {
        let text = String::from_utf8(field(block, range)).unwrap();
        u64::from_str_radix(text.trim(), 8).unwrap()
    };
    let mut members = Vec::new();
    let mut pax: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    let mut at = 0;
    let mut zeros = 0;
    while at < tar.len() {
        let block = &tar[at..at + 512];
        at += 512;
        if block.iter().all(|&b| b == 0) {
            zeros += 1;
            continue;
        }
        assert_eq!(zeros, 0, "data after the end of the archive");
        assert_eq!(&block[257..263], b"ustar\0");
        let mut copy = block.to_vec();
        copy[148..156].copy_from_slice(b"        ");
        assert_eq!(
            number(block, 148..156),
            copy.iter().map(|&b| u64::from(b)).sum::<u64>(),
            "checksum"
        );
        assert_eq!(number(block, 108..116), 0, "uid");
        assert_eq!(number(block, 116..124), 0, "gid");
        let kind = block[156];
        let mut size = number(block, 124..136);
        if let Some(value) = pax.get(b"size".as_slice()) {
            size = String::from_utf8(value.clone()).unwrap().parse().unwrap();
        }
        let data = tar[at..at + size as usize].to_vec();
        at += (size as usize).div_ceil(512) * 512;
        if kind == b'x' {
            let mut rest = data.as_slice();
            while !rest.is_empty() {
                let space = rest.iter().position(|&b| b == b' ').unwrap();
                let length: usize = std::str::from_utf8(&rest[..space])
                    .unwrap()
                    .parse()
                    .unwrap();
                let record = &rest[space + 1..length - 1];
                assert_eq!(rest[length - 1], b'\n');
                let equals = record.iter().position(|&b| b == b'=').unwrap();
                pax.insert(record[..equals].to_vec(), record[equals + 1..].to_vec());
                rest = &rest[length..];
            }
            continue;
        }
        let name = pax
            .remove(b"path".as_slice())
            .unwrap_or_else(|| field(block, 0..100));
        let link = pax
            .remove(b"linkpath".as_slice())
            .unwrap_or_else(|| field(block, 157..257));
        let mtime = pax
            .remove(b"mtime".as_slice())
            .map(|value| String::from_utf8(value).unwrap().parse().unwrap())
            .unwrap_or(number(block, 136..148) as i64);
        pax.clear();
        members.push((
            name,
            Member {
                kind,
                mode: number(block, 100..108) as u32,
                mtime,
                data,
                link,
            },
        ));
    }
    assert!(zeros >= 2, "the archive ends with two zero blocks");
    members
}

#[test]
fn download_capability_and_limits_are_advertised() {
    let daemon = Daemon::start();
    let peer = daemon.peer();
    match peer.welcome.clone().unwrap() {
        Event::Welcome {
            capabilities,
            file_limits,
            ..
        } => {
            assert!(capabilities.iter().any(|c| c == "file-downloads"));
            let limits = file_limits.unwrap().downloads.unwrap();
            assert_eq!(limits, downloads::LIMITS);
            assert_eq!(limits.chunk_bytes, 256 * 1024);
            assert_eq!(limits.max_chunks_in_flight, 8);
            // A window of base64 chunks stays far below the output queue.
            assert!(
                limits.max_chunks_in_flight * limits.chunk_bytes.div_ceil(3) * 4 < MAX_MESSAGE / 4
            );
        }
        other => panic!("{other:?}"),
    }
    // Older clients ignore the new field: the limits still decode without it.
    let old: files::Limits = serde_json::from_value(serde_json::json!({
        "max_read_bytes": 1, "max_write_bytes": 1, "max_list_entries": 1, "max_paths": 1,
        "max_in_flight": 1, "max_path_bytes": 1, "max_depth": 1
    }))
    .unwrap();
    assert_eq!(old.downloads, None);
}

#[test]
fn a_single_file_downloads_as_itself() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let path = daemon.work().join("photo.bin");
    let content = noise(600_000, 7);
    fs::write(&path, &content).unwrap();
    let got = peer.download(&[&path], None).unwrap();
    assert_eq!(got.name, "photo.bin");
    assert_eq!(got.size, Some(600_000));
    assert_eq!(got.archive, None);
    assert_eq!(got.bytes, content);
    assert_eq!(got.hash, peer.hash(&path));
    assert_eq!((got.skipped, got.changed), (0, 0));
    // An empty file has no chunk.
    let empty = daemon.work().join("empty");
    fs::write(&empty, b"").unwrap();
    let got = peer.download(&[&empty], None).unwrap();
    assert!(got.bytes.is_empty());
    assert_eq!(got.hash, peer.hash(&empty));
    // A symlink to a file downloads the file, as a read follows it.
    let link = daemon.work().join("link");
    symlink(&path, &link).unwrap();
    assert_eq!(peer.download(&[&link], None).unwrap().bytes, content);
    // A folder needs an archive; a FIFO is never opened; several paths need
    // an archive too.
    let folder = daemon.work().join("folder");
    fs::create_dir(&folder).unwrap();
    assert_eq!(
        peer.download(&[&folder], None).err().unwrap().code,
        Code::IsADirectory
    );
    let fifo = daemon.work().join("fifo");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert_eq!(
        peer.download(&[&fifo], None).err().unwrap().code,
        Code::NotAFile
    );
    assert_eq!(
        peer.download(&[&path, &empty], None).err().unwrap().code,
        Code::InvalidRequest
    );
    assert_eq!(
        peer.download(&[&daemon.work().join("missing")], None)
            .err()
            .unwrap()
            .code,
        Code::NotFound
    );
    let error = peer.download(&[Path::new("relative")], None).err().unwrap();
    assert_eq!(
        (error.code, error.path.as_deref()),
        (Code::InvalidPath, Some("relative"))
    );
    assert_eq!(
        peer.download(&[Path::new("/")], Some(Archive::TarGz))
            .err()
            .unwrap()
            .code,
        Code::OutsideRoot
    );
}

#[test]
fn downloads_stay_inside_the_root() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let work = daemon.work();
    let outside = daemon.root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret.txt"), "secret").unwrap();
    fs::create_dir(work.join("project")).unwrap();
    fs::write(work.join("project/a.txt"), "a").unwrap();
    symlink(&outside, work.join("project/out")).unwrap();
    symlink(outside.join("secret.txt"), work.join("secret-link")).unwrap();
    symlink("/etc", work.join("etc")).unwrap();
    let code = |peer: &mut Peer, paths: &[&Path], archive| {
        peer.download(paths, archive).err().unwrap().code
    };
    // Outside, directly or through a link, as a file or an archive.
    for archive in [None, Some(Archive::TarGz)] {
        assert_eq!(
            code(&mut peer, &[&outside.join("secret.txt")], archive),
            Code::OutsideRoot
        );
        assert_eq!(
            code(&mut peer, &[&work.join("project/out/secret.txt")], archive),
            Code::OutsideRoot
        );
        assert_eq!(
            code(&mut peer, &[&work.join("etc/passwd")], archive),
            Code::OutsideRoot
        );
    }
    assert_eq!(
        code(&mut peer, &[&work.join("secret-link")], None),
        Code::OutsideRoot
    );
    assert_eq!(
        code(&mut peer, &[&outside], Some(Archive::TarGz)),
        Code::OutsideRoot
    );
    // One path outside refuses the whole selection.
    assert_eq!(
        code(
            &mut peer,
            &[&work.join("project"), &outside],
            Some(Archive::TarGz)
        ),
        Code::OutsideRoot
    );
    // The root itself is not downloaded; its entries are.
    assert_eq!(
        code(&mut peer, &[&work], Some(Archive::TarGz)),
        Code::InvalidRequest
    );
    // An archive stores a link out as a link, never its target; selected
    // itself, a link is stored the same way.
    let got = peer
        .download(
            &[&work.join("project"), &work.join("secret-link")],
            Some(Archive::TarGz),
        )
        .unwrap();
    let members: BTreeMap<String, Member> = untar(&got.bytes)
        .into_iter()
        .map(|(name, member)| (String::from_utf8(name).unwrap(), member))
        .collect();
    let link = members.get("project/out").unwrap();
    assert_eq!(
        (link.kind, link.link.clone()),
        (b'2', text(&outside).into_bytes())
    );
    assert_eq!(members.get("secret-link").unwrap().kind, b'2');
    assert!(members.values().all(|member| member.data != b"secret"));
}

#[test]
fn a_folder_downloads_as_a_tar_gz_archive() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let project = daemon.work().join("project");
    fs::create_dir_all(project.join("src/deep")).unwrap();
    fs::create_dir(project.join("empty")).unwrap();
    let big = noise(700_000, 3);
    fs::write(project.join("src/big.bin"), &big).unwrap();
    fs::write(project.join("src/deep/a.txt"), "alpha\n").unwrap();
    fs::write(project.join("run.sh"), "#!/bin/sh\necho hi\n").unwrap();
    fs::set_permissions(project.join("run.sh"), fs::Permissions::from_mode(0o750)).unwrap();
    let long = "n".repeat(150);
    fs::write(project.join(&long), "long name").unwrap();
    fs::write(project.join("café ☕.txt"), "unicode").unwrap();
    // Symlinks are stored as links, never followed: a link to a secret
    // outside the folder and a link to a folder.
    fs::write(daemon.work().join("secret"), "outside").unwrap();
    symlink(daemon.work().join("secret"), project.join("secret-link")).unwrap();
    symlink("src", project.join("src-link")).unwrap();
    let long_target = format!("/{}", "t".repeat(200));
    symlink(&long_target, project.join("long-link")).unwrap();
    // A FIFO is skipped (and never opened).
    let fifo = project.join("fifo");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);

    let got = peer.download(&[&project], Some(Archive::TarGz)).unwrap();
    assert_eq!(got.name, "project.tar.gz");
    assert_eq!(got.archive, Some(Archive::TarGz));
    assert_eq!(got.size, None);
    assert_eq!(got.skipped, 1, "the FIFO");
    assert_eq!(got.changed, 0);
    // Folders project, src, src/deep, empty; six files; three links.
    assert_eq!(got.entries, Some(4 + 5 + 3));
    let inputs = big.len() + 6 + 18 + 9 + "unicode".len();
    assert_eq!(got.input_bytes, Some(inputs as u64));
    // The hash covers the bytes as sent.
    let archive = daemon.work().join("project.tar.gz");
    fs::write(&archive, &got.bytes).unwrap();
    assert_eq!(got.hash, peer.hash(&archive));

    let members = untar(&got.bytes);
    let names: Vec<String> = members
        .iter()
        .map(|(name, _)| String::from_utf8(name.clone()).unwrap())
        .collect();
    // Byte order within each folder, a folder's tree right after it.
    assert_eq!(
        names,
        [
            "project/".to_owned(),
            "project/café ☕.txt".into(),
            "project/empty/".into(),
            "project/long-link".into(),
            format!("project/{long}"),
            "project/run.sh".into(),
            "project/secret-link".into(),
            "project/src/".into(),
            "project/src/big.bin".into(),
            "project/src/deep/".into(),
            "project/src/deep/a.txt".into(),
            "project/src-link".into(),
        ]
    );
    let members: BTreeMap<String, Member> = members
        .into_iter()
        .map(|(name, member)| (String::from_utf8(name).unwrap(), member))
        .collect();
    let member = |name: &str| members.get(name).unwrap_or_else(|| panic!("{name}"));
    assert_eq!(member("project/src/big.bin").data, big);
    assert_eq!(member("project/src/deep/a.txt").data, b"alpha\n");
    assert_eq!(member("project/café ☕.txt").data, b"unicode");
    assert_eq!(member(&format!("project/{long}")).data, b"long name");
    let script = member("project/run.sh");
    assert_eq!((script.kind, script.mode), (b'0', 0o750));
    let modified = fs::metadata(project.join("run.sh"))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert_eq!(script.mtime, modified);
    assert_eq!(member("project/empty/").kind, b'5');
    let link = member("project/secret-link");
    assert_eq!(link.kind, b'2');
    assert_eq!(link.link, text(&daemon.work().join("secret")).as_bytes());
    assert!(link.data.is_empty());
    assert_eq!(member("project/src-link").link, b"src");
    assert_eq!(member("project/long-link").link, long_target.as_bytes());
    assert!(!members.keys().any(|name| name.contains("fifo")));
}

#[test]
fn a_selection_downloads_as_one_archive() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let work = daemon.work();
    fs::create_dir(work.join("docs")).unwrap();
    fs::write(work.join("docs/a.md"), "a").unwrap();
    fs::write(work.join("notes.txt"), "notes").unwrap();
    symlink("notes.txt", work.join("alias")).unwrap();
    let got = peer
        .download(
            &[
                &work.join("docs"),
                &work.join("notes.txt"),
                &work.join("alias"),
            ],
            Some(Archive::TarGz),
        )
        .unwrap();
    assert_eq!(got.name, "work.tar.gz");
    let names: Vec<Vec<u8>> = untar(&got.bytes)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        names,
        [
            b"docs/".to_vec(),
            b"docs/a.md".to_vec(),
            b"notes.txt".to_vec(),
            b"alias".to_vec()
        ]
    );
    // A single file as an archive, and two selected entries with one name.
    let got = peer
        .download(&[&work.join("notes.txt")], Some(Archive::TarGz))
        .unwrap();
    assert_eq!(got.name, "notes.txt.tar.gz");
    assert_eq!(untar(&got.bytes)[0].1.data, b"notes");
    fs::create_dir(work.join("other")).unwrap();
    fs::write(work.join("other/notes.txt"), "other").unwrap();
    assert_eq!(
        peer.download(
            &[&work.join("notes.txt"), &work.join("other/notes.txt")],
            Some(Archive::TarGz)
        )
        .err()
        .unwrap()
        .code,
        Code::InvalidRequest
    );
}

#[test]
fn an_archive_over_its_bounds_is_refused_before_it_starts() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let mut deep = daemon.work().join("deep");
    // `deep` and 64 folders below it: 65 levels, one too many.
    for _ in 0..files::MAX_DEPTH {
        deep.push("d");
    }
    fs::create_dir_all(&deep).unwrap();
    let error = peer
        .download(&[&daemon.work().join("deep")], Some(Archive::TarGz))
        .err()
        .unwrap();
    assert_eq!(error.code, Code::TooLarge);
    assert!(error.message.contains("nested"), "{}", error.message);
    // One level less is fine.
    let got = peer
        .download(&[&daemon.work().join("deep/d")], Some(Archive::TarGz))
        .unwrap();
    assert_eq!(got.entries, Some(files::MAX_DEPTH as u64));
}

#[test]
fn unacknowledged_chunks_stop_at_the_window() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let path = daemon.work().join("big");
    let content = noise(5 * 1024 * 1024, 11);
    fs::write(&path, &content).unwrap();
    let (download, _) = peer.begin(&[&path], None).unwrap();
    let window = downloads::MAX_CHUNKS_IN_FLIGHT;
    let mut bytes = Vec::new();
    let chunk = |peer: &mut Peer, bytes: &mut Vec<u8>| match peer.next() {
        Event::FileDownloadChunk {
            download: d,
            offset,
            data,
            ..
        } => {
            assert_eq!((d, offset), (download, bytes.len() as u64));
            bytes.extend(STANDARD.decode(data).unwrap());
        }
        other => panic!("{other:?}"),
    };
    for _ in 0..window {
        chunk(&mut peer, &mut bytes);
    }
    // Nothing more arrives without an acknowledgement.
    peer.socket
        .set_read_timeout(Some(Duration::from_millis(600)))
        .unwrap();
    loop {
        match read_message::<ServerMessage>(&mut peer.socket) {
            Ok(Some(ServerMessage::Stream(Event::Update(_)))) => continue,
            Ok(other) => panic!("{other:?}"),
            Err(_) => break,
        }
    }
    peer.socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    // One acknowledgement, one more chunk.
    peer.send(Request::FileDownloadAck {
        download,
        offset: downloads::CHUNK_BYTES as u64,
    });
    chunk(&mut peer, &mut bytes);
    assert_eq!(bytes, content[..bytes.len()]);
    // An acknowledgement out of order fails the download.
    peer.send(Request::FileDownloadAck {
        download,
        offset: 5,
    });
    loop {
        match peer.next() {
            Event::FileDownloadChunk { .. } => {}
            Event::FileError {
                error,
                download: Some(d),
                ..
            } => {
                assert_eq!((d, error.code), (download, Code::InvalidRequest));
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    // The connection stays usable.
    assert_eq!(peer.download(&[&path], None).unwrap().bytes, content);
}

#[test]
fn a_cancelled_download_sends_nothing_more() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let path = daemon.work().join("big");
    fs::write(&path, noise(8 * 1024 * 1024, 5)).unwrap();
    let (download, _) = peer.begin(&[&path], None).unwrap();
    peer.cancel(download);
    // Late acknowledgements and a second cancel are harmless.
    peer.send(Request::FileDownloadAck {
        download,
        offset: downloads::CHUNK_BYTES as u64,
    });
    peer.cancel(download);
    // Its slot comes back once its thread returns, which it does promptly
    // (but not necessarily before the next request): the connection's
    // whole allowance works again.
    let mut started = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while started.len() < downloads::MAX_DOWNLOADS {
        match peer.begin(&[&path], None) {
            Ok((download, _)) => started.push(download),
            Err(error) if error.code == Code::Busy && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("{error:?}"),
        }
    }
    // Over the allowance: busy.
    assert_eq!(peer.begin(&[&path], None).err().unwrap().code, Code::Busy);
    for download in started {
        peer.cancel(download);
    }
}

#[test]
fn cancelling_and_restarting_stays_within_the_allowance() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let path = daemon.work().join("big");
    fs::write(&path, noise(4 * 1024 * 1024, 17)).unwrap();
    for _ in 0..20 {
        let mut started = Vec::new();
        // A cancelled download's thread holds its slot until it returns,
        // which it does promptly: the slot comes back.
        let deadline = Instant::now() + Duration::from_secs(10);
        while started.len() < downloads::MAX_DOWNLOADS {
            match peer.begin(&[&path], None) {
                Ok((download, _)) => started.push(download),
                Err(error) if error.code == Code::Busy && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("{error:?}"),
            }
        }
        assert_eq!(peer.begin(&[&path], None).err().unwrap().code, Code::Busy);
        for download in started {
            peer.cancel(download);
        }
    }
    // The connection's allowance is whole again.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match peer.download(&[&path], None) {
            Ok(got) => break assert_eq!(got.bytes.len(), 4 * 1024 * 1024),
            Err(error) if error.code == Code::Busy && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("{error:?}"),
        }
    }
}

#[test]
fn a_disconnect_stops_its_downloads() {
    let daemon = Daemon::start();
    let path = daemon.work().join("big");
    fs::write(&path, noise(8 * 1024 * 1024, 9)).unwrap();
    // Fill the server's download threads from connections that then leave
    // mid-download.
    let per = downloads::MAX_DOWNLOADS;
    for _ in 0..downloads::MAX_SERVER_DOWNLOADS / per {
        let mut peer = daemon.peer();
        for _ in 0..per {
            peer.begin(&[&path], None).unwrap();
        }
        drop(peer);
    }
    // Their threads stop, so a new connection can download again.
    let mut peer = daemon.peer();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match peer.download(&[&path], None) {
            Ok(got) => {
                assert_eq!(got.bytes.len(), 8 * 1024 * 1024);
                break;
            }
            Err(error) if error.code == Code::Busy && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => panic!("{error:?}"),
        }
    }
}

#[test]
fn a_large_download_streams_while_screens_keep_updating() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let path = daemon.work().join("large");
    let content = noise(50 * 1024 * 1024, 13);
    fs::write(&path, &content).unwrap();
    // A pane that keeps printing, subscribed by this connection.
    let request = peer.id();
    peer.send(Request::Spawn {
        request: request.clone(),
        cwd: None,
        argv: vec![
            "sh".into(),
            "-c".into(),
            "while :; do date +%s%N; sleep 0.02; done".into(),
        ],
        shell: None,
    });
    let pane = loop {
        match peer.next() {
            Event::Spawned { request: r, pane } if r == request => break pane,
            _ => {}
        }
    };
    peer.send(Request::Subscribe { panes: vec![pane] });
    let (download, _) = peer.begin(&[&path], None).unwrap();
    let started = Instant::now();
    let mut bytes = Vec::new();
    let mut screens = 0;
    let mut chunks = 0;
    loop {
        match peer.any() {
            Event::Update(update) => {
                if !update.snapshots.is_empty() || !update.patches.is_empty() {
                    screens += 1;
                }
            }
            Event::FileDownloadChunk {
                download: d,
                offset,
                data,
                ..
            } if d == download => {
                assert_eq!(offset, bytes.len() as u64);
                bytes.extend(STANDARD.decode(data).unwrap());
                chunks += 1;
                // A slow reader: the server must wait, not disconnect.
                if chunks % 16 == 0 {
                    thread::sleep(Duration::from_millis(40));
                }
                peer.send(Request::FileDownloadAck {
                    download,
                    offset: bytes.len() as u64,
                });
            }
            Event::FileDownloadDone {
                download: d, size, ..
            } if d == download => {
                assert_eq!(size, content.len() as u64);
                break;
            }
            Event::FileError { error, .. } => panic!("{error:?}"),
            _ => {}
        }
    }
    assert!(bytes == content, "the downloaded bytes differ");
    let elapsed = started.elapsed();
    // Screens kept arriving while the download ran (every 33 ms at most
    // while the pane prints; at least a few over the download).
    assert!(
        screens >= 3,
        "{screens} screen updates during a {elapsed:?} download"
    );
}

#[test]
fn downloads_reach_extra_roots_without_crossing_between_roots() {
    let daemon = Daemon::start();
    let mut peer = daemon.peer();
    let (work, extra) = (daemon.work(), daemon.extra());
    fs::write(work.join("secret.txt"), "primary").unwrap();
    fs::create_dir(extra.join("project")).unwrap();
    fs::write(extra.join("project/a.txt"), "a").unwrap();
    symlink(&work, extra.join("project/to-work")).unwrap();
    symlink("../work", extra.join("rel-work")).unwrap();
    symlink(&extra, work.join("to-extra")).unwrap();
    // A file and a folder inside the extra root download as usual.
    let got = peer
        .download(&[&extra.join("project/a.txt")], None)
        .unwrap();
    assert_eq!(
        (got.name.as_str(), got.bytes.as_slice()),
        ("a.txt", &b"a"[..])
    );
    // A selection across two roots is still one archive.
    let got = peer
        .download(
            &[&extra.join("project"), &work.join("secret.txt")],
            Some(Archive::TarGz),
        )
        .unwrap();
    let members: BTreeMap<String, Member> = untar(&got.bytes)
        .into_iter()
        .map(|(name, member)| (String::from_utf8(name).unwrap(), member))
        .collect();
    assert_eq!(members.get("project/a.txt").unwrap().data, b"a");
    // The link into the primary root is stored as a link.
    assert_eq!(members.get("project/to-work").unwrap().kind, b'2');
    // Nothing is reached through a link from one root into another.
    for archive in [None, Some(Archive::TarGz)] {
        for path in [
            extra.join("project/to-work/secret.txt"),
            extra.join("rel-work/secret.txt"),
            work.join("to-extra/project/a.txt"),
        ] {
            assert_eq!(
                peer.download(&[&path], archive).err().unwrap().code,
                Code::OutsideRoot,
                "{path:?}"
            );
        }
    }
    // An extra root itself is not downloaded.
    assert_eq!(
        peer.download(&[&extra], Some(Archive::TarGz))
            .err()
            .unwrap()
            .code,
        Code::InvalidRequest
    );
}
