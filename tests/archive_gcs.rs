//! Archive, verification and restore against a local stand-in for the Cloud
//! Storage JSON API (resumable uploads, object metadata, listing, media
//! downloads), with a fixed token: no network, no Google account. The
//! stand-in can fail a chunk once, report a wrong CRC32C, or corrupt a
//! download.
use kelpmux::native::{
    archive::{self, Context, Job, gcs, tarball},
    removal::Mode,
    user_config,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

const TOKEN: &str = "test-token";
const BUCKET: &str = "test-bucket";

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kelpmux-archive-gcs-{}-{}",
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

struct Session {
    name: String,
    crc32c: String,
    metadata: Value,
    data: Vec<u8>,
}

#[derive(Default)]
struct State {
    objects: HashMap<String, (Vec<u8>, Value)>,
    sessions: HashMap<u64, Session>,
    next: u64,
    /// Answer the next chunk after the first with HTTP 503, once.
    fail_chunk: bool,
    /// Report a wrong CRC32C for the objects stored.
    lie_crc: bool,
    /// Flip a byte of downloads.
    corrupt: bool,
    chunks: usize,
    /// Requests without the token where one is needed.
    unauthorized: usize,
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            out.push(u8::from_str_radix(&text[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

fn query(target: &str) -> HashMap<String, String> {
    target
        .split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_owned(), decode(v)))
        .collect()
}

fn resource(name: &str, bytes: &[u8], metadata: &Value, generation: u64, lie: bool) -> Value {
    let mut crc = tarball::crc32c(bytes);
    if lie {
        crc ^= 1;
    }
    json!({
        "name": name,
        "bucket": BUCKET,
        "size": bytes.len().to_string(),
        "crc32c": tarball::crc32c_base64(crc),
        "generation": generation.to_string(),
        "updated": format!("2026-11-24T10:00:{:02}Z", generation % 60),
        "metadata": metadata,
    })
}

struct Fake {
    base: String,
    state: Arc<Mutex<State>>,
}
impl Fake {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        let origin = base.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = shared.clone();
                let origin = origin.clone();
                thread::spawn(move || serve(stream, &state, &origin));
            }
        });
        Self { base, state }
    }
    fn store(&self) -> Arc<gcs::Gcs> {
        Arc::new(gcs::Gcs::local(
            &self.base,
            gcs::TokenSource::Fixed(TOKEN.into()),
        ))
    }
    fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self.state.lock().unwrap())
    }
}

fn respond(stream: &mut TcpStream, code: u16, headers: &[(&str, String)], body: &[u8]) {
    let mut head = format!(
        "HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn serve(mut stream: TcpStream, state: &Mutex<State>, origin: &str) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.is_empty() {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let target = parts.next().unwrap_or("").to_owned();
    let mut headers = HashMap::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.insert(name.trim().to_lowercase(), value.trim().to_owned());
        }
    }
    let length: usize = headers
        .get("content-length")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).unwrap();
    let path = target.split('?').next().unwrap_or("").to_owned();
    let q = query(&target);
    let authorized =
        headers.get("authorization").map(String::as_str) == Some(&format!("Bearer {TOKEN}"));
    let object_prefix = format!("/storage/v1/b/{BUCKET}/o/");
    let upload = format!("/upload/storage/v1/b/{BUCKET}/o");
    let mut state = state.lock().unwrap();
    let json_headers = [("Content-Type", "application/json".to_owned())];
    // An upload session's requests carry no token; everything else must.
    if !(method == "PUT" || (method == "DELETE" && q.contains_key("upload_id"))) && !authorized {
        state.unauthorized += 1;
        return respond(
            &mut stream,
            401,
            &json_headers,
            br#"{"error":{"message":"no token"}}"#,
        );
    }
    match (method.as_str(), path.as_str()) {
        ("POST", p)
            if p == upload && q.get("uploadType").map(String::as_str) == Some("resumable") =>
        {
            let resource: Value = serde_json::from_slice(&body).unwrap();
            let name = resource["name"].as_str().unwrap().to_owned();
            if q.get("ifGenerationMatch").map(String::as_str) == Some("0")
                && state.objects.contains_key(&name)
            {
                return respond(&mut stream, 412, &json_headers, b"{}");
            }
            state.next += 1;
            let id = state.next;
            state.sessions.insert(
                id,
                Session {
                    name,
                    crc32c: resource["crc32c"].as_str().unwrap_or("").to_owned(),
                    metadata: resource["metadata"].clone(),
                    data: Vec::new(),
                },
            );
            respond(
                &mut stream,
                200,
                &[(
                    "Location",
                    format!("{origin}{upload}?uploadType=resumable&upload_id={id}"),
                )],
                b"",
            )
        }
        ("POST", p) if p == upload => {
            let name = q["name"].clone();
            if state.objects.contains_key(&name) {
                return respond(&mut stream, 412, &json_headers, b"{}");
            }
            state.next += 1;
            let value = resource(&name, &body, &json!({}), state.next, false);
            state.objects.insert(name, (body, value.clone()));
            respond(
                &mut stream,
                200,
                &json_headers,
                &serde_json::to_vec(&value).unwrap(),
            )
        }
        ("PUT", p) if p == upload => {
            let id: u64 = q["upload_id"].parse().unwrap();
            let range = headers.get("content-range").cloned().unwrap_or_default();
            let (spec, total) = range
                .strip_prefix("bytes ")
                .unwrap()
                .split_once('/')
                .unwrap();
            let total: usize = total.parse().unwrap();
            if spec != "*" {
                state.chunks += 1;
                if state.fail_chunk && state.chunks == 2 {
                    state.fail_chunk = false;
                    return respond(&mut stream, 503, &json_headers, b"{}");
                }
                let (first, _) = spec.split_once('-').unwrap();
                let first: usize = first.parse().unwrap();
                let session = state.sessions.get_mut(&id).unwrap();
                assert_eq!(first, session.data.len(), "chunks arrive in order");
                session.data.extend_from_slice(&body);
            }
            let session = state.sessions.get(&id).unwrap();
            let have = session.data.len();
            if have < total {
                let mut extra = vec![];
                if have > 0 {
                    extra.push(("Range", format!("bytes=0-{}", have - 1)));
                }
                return respond(&mut stream, 308, &extra, b"");
            }
            // Complete: Cloud Storage checks the CRC32C given at the start.
            let data = session.data.clone();
            if session.crc32c != tarball::crc32c_base64(tarball::crc32c(&data)) {
                return respond(
                    &mut stream,
                    400,
                    &json_headers,
                    br#"{"error":{"message":"crc32c mismatch"}}"#,
                );
            }
            let (name, metadata) = (session.name.clone(), session.metadata.clone());
            state.next += 1;
            let lie = state.lie_crc;
            let value = resource(&name, &data, &metadata, state.next, lie);
            state.objects.insert(name, (data, value.clone()));
            respond(
                &mut stream,
                200,
                &json_headers,
                &serde_json::to_vec(&value).unwrap(),
            )
        }
        ("DELETE", p) if p == upload => respond(&mut stream, 499, &[], b""),
        ("GET", p) if p == format!("/storage/v1/b/{BUCKET}/o") => {
            let prefix = q.get("prefix").cloned().unwrap_or_default();
            let items: Vec<Value> = state
                .objects
                .iter()
                .filter(|(name, _)| name.starts_with(&prefix))
                .map(|(_, (_, value))| value.clone())
                .collect();
            respond(
                &mut stream,
                200,
                &json_headers,
                &serde_json::to_vec(&json!({"items": items})).unwrap(),
            )
        }
        ("GET", p) if p.starts_with(&object_prefix) => {
            let name = decode(&p[object_prefix.len()..]);
            let Some((bytes, value)) = state.objects.get(&name).cloned() else {
                return respond(
                    &mut stream,
                    404,
                    &json_headers,
                    br#"{"error":{"message":"No such object"}}"#,
                );
            };
            if q.get("alt").map(String::as_str) == Some("media") {
                let mut bytes = bytes;
                if state.corrupt {
                    let middle = bytes.len() / 2;
                    bytes[middle] ^= 0xff;
                }
                respond(&mut stream, 200, &[], &bytes)
            } else {
                respond(
                    &mut stream,
                    200,
                    &json_headers,
                    &serde_json::to_vec(&value).unwrap(),
                )
            }
        }
        ("DELETE", p) if p.starts_with(&object_prefix) => {
            let name = decode(&p[object_prefix.len()..]);
            match state.objects.remove(&name) {
                Some(_) => respond(&mut stream, 204, &[], b""),
                None => respond(&mut stream, 404, &json_headers, b"{}"),
            }
        }
        _ => respond(&mut stream, 400, &json_headers, b"{}"),
    }
}

/// A project `kelp` in `<temp>/root/projects/kelp` with archiving to the
/// stand-in, and the context of the operations.
fn setup(temp: &Temp, fake: &Fake) -> (Context, PathBuf) {
    let root = temp.0.join("root");
    let folder = root.join("projects/kelp");
    fs::create_dir_all(folder.join("src")).unwrap();
    fs::create_dir_all(temp.0.join("home")).unwrap();
    fs::write(folder.join("README"), "hello\n").unwrap();
    fs::write(folder.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    std::os::unix::fs::symlink("../README", folder.join("src/readme-link")).unwrap();
    // Incompressible data: the archive takes two upload chunks.
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let data: Vec<u8> = (0..(gcs::CHUNK + gcs::CHUNK / 4))
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    fs::write(folder.join("data.bin"), data).unwrap();
    let config = temp.0.join("config/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        format!(
            "[files]\nroot = \"{root}\"\n[projects]\nbase = \"{root}/projects\"\n[scratch]\nfolder = \"{root}/scratch\"\n[archive]\nbackend = \"gcs\"\nbucket = \"{BUCKET}\"\n\n[[project]]\nid = \"kelp\"\nname = \"Kelp\"\nfolder = \"{root}/projects/kelp\"\n",
            root = root.display()
        ),
    )
    .unwrap();
    let ctx = Context {
        config_path: config,
        env: user_config::Env {
            home: temp.0.join("home"),
            check_dirs: true,
        },
        state: temp.0.join("state"),
        cache: temp.0.join("cache"),
        store: Some(fake.store()),
    };
    (ctx, folder)
}

fn fingerprint(ctx: &Context, mode: Mode) -> String {
    archive::check(ctx, "kelp", mode).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Every file of a tree: relative path → content, or link target.
fn contents(folder: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let relative = path.strip_prefix(folder).unwrap().to_path_buf();
            if meta.file_type().is_symlink() {
                found.push((
                    relative,
                    fs::read_link(&path)
                        .unwrap()
                        .into_os_string()
                        .into_encoded_bytes(),
                ));
            } else if meta.is_dir() {
                found.push((relative, b"/".to_vec()));
                stack.push(path);
            } else {
                found.push((relative, fs::read(&path).unwrap()));
            }
        }
    }
    found.sort();
    found
}

fn config(ctx: &Context) -> user_config::Config {
    ctx.config().unwrap()
}

#[test]
fn archive_uploads_verifies_and_removes_then_restore_brings_it_back() {
    let temp = Temp::new();
    let fake = Fake::start();
    let (ctx, folder) = setup(&temp, &fake);
    let before = contents(&folder);
    // A private project folder stays private.
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o710)).unwrap();
    }
    // A failed chunk is resumed where the server says it stopped.
    fake.with(|s| s.fail_chunk = true);
    let job = Job::start(&ctx, "kelp", "archive", None).unwrap();
    let result = archive::archive(&ctx, &job, "kelp", &fingerprint(&ctx, Mode::Archive));
    job.finish(&result);
    let result = result.unwrap();
    assert!(
        fs::symlink_metadata(&folder).is_err(),
        "the folder was removed"
    );
    assert_eq!(
        fs::read_dir(folder.parent().unwrap()).unwrap().count(),
        0,
        "nothing left beside it"
    );
    assert!(fake.with(|s| s.chunks) >= 3, "two chunks and a retry");
    assert_eq!(fake.with(|s| s.unauthorized), 0);
    let project = config(&ctx).project("kelp").cloned().unwrap();
    assert_eq!(project.status, user_config::ProjectStatus::Archived);
    let record = project.archive.clone().unwrap();
    assert!(
        record.entries.is_some() && record.unpacked_bytes.is_some(),
        "{record:?}"
    );
    assert_eq!(result["archive"]["object"], record.object.as_str());
    assert!(record.object.starts_with("kelpmux/kelp/") && record.object.ends_with("Z.tar.gz"));
    // The stored object carries its SHA-256 and the project.
    let stored = fake.with(|s| s.objects[&record.object].1.clone());
    assert_eq!(stored["metadata"]["sha256"], record.sha256.as_str());
    assert_eq!(stored["metadata"]["kelpmux-project"], "kelp");
    assert_eq!(stored["metadata"]["kelpmux-name"], "Kelp");
    // The temporary archive is gone.
    assert_eq!(fs::read_dir(&ctx.cache).unwrap().count(), 0);
    let listed = archive::list(&ctx, "kelp").unwrap();
    assert_eq!(listed["archives"][0]["object"], record.object.as_str());
    assert_eq!(listed["archives"][0]["recorded"], true);
    // A checking change of the status is refused while archived.
    assert!(archive::set_status(&ctx, "kelp", user_config::ProjectStatus::Paused).is_err());
    let job = Job::start(&ctx, "kelp", "restore", None).unwrap();
    let restored = archive::restore(&ctx, &job, "kelp", None, None);
    job.finish(&restored);
    restored.unwrap();
    assert_eq!(contents(&folder), before);
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(fs::metadata(&folder).unwrap().mode() & 0o777, 0o710);
    }
    let project = config(&ctx).project("kelp").cloned().unwrap();
    assert_eq!(project.status, user_config::ProjectStatus::Active);
    assert_eq!(project.archive, Some(record));
    let leftovers: Vec<_> = fs::read_dir(folder.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(leftovers.len(), 1, "{leftovers:?}");
    // The original folder exists now: a restore needs a new one, inside
    // the projects folder.
    let job = Job::start(&ctx, "kelp", "restore", None).unwrap();
    let error = archive::restore(&ctx, &job, "kelp", None, None).unwrap_err();
    assert!(error.contains("exists"), "{error}");
    let error = archive::restore(&ctx, &job, "kelp", None, Some("/etc/kelp")).unwrap_err();
    assert!(error.contains("inside the projects folder"), "{error}");
    drop(job);
}

#[test]
fn a_checksum_mismatch_keeps_the_folder_and_the_status() {
    let temp = Temp::new();
    let fake = Fake::start();
    let (ctx, folder) = setup(&temp, &fake);
    fake.with(|s| s.lie_crc = true);
    let job = Job::start(&ctx, "kelp", "archive", None).unwrap();
    let error =
        archive::archive(&ctx, &job, "kelp", &fingerprint(&ctx, Mode::Archive)).unwrap_err();
    assert!(error.contains("CRC32C"), "{error}");
    assert!(folder.join("data.bin").is_file());
    let project = config(&ctx).project("kelp").cloned().unwrap();
    assert_eq!(project.status, user_config::ProjectStatus::Active);
    assert_eq!(project.archive, None);
    // A stale fingerprint stops before anything is uploaded.
    fake.with(|s| s.lie_crc = false);
    let objects = fake.with(|s| s.objects.len());
    let error = archive::archive(&ctx, &job, "kelp", "0000000000000000").unwrap_err();
    assert!(error.contains("changed since it was checked"), "{error}");
    assert_eq!(fake.with(|s| s.objects.len()), objects);
    let state = job.finish(&Err(error));
    assert_eq!(state["state"], "failed");
}

#[test]
fn a_corrupt_download_restores_nothing_and_a_new_folder_works() {
    let temp = Temp::new();
    let fake = Fake::start();
    let (ctx, folder) = setup(&temp, &fake);
    let before = contents(&folder);
    let job = Job::start(&ctx, "kelp", "archive", None).unwrap();
    archive::archive(&ctx, &job, "kelp", &fingerprint(&ctx, Mode::Archive)).unwrap();
    drop(job);
    fake.with(|s| s.corrupt = true);
    let job = Job::start(&ctx, "kelp", "restore", None).unwrap();
    let error = archive::restore(&ctx, &job, "kelp", None, None).unwrap_err();
    assert!(error.contains("does not match"), "{error}");
    assert!(fs::symlink_metadata(&folder).is_err());
    assert_eq!(fs::read_dir(folder.parent().unwrap()).unwrap().count(), 0);
    fake.with(|s| s.corrupt = false);
    // Into a new folder under the projects folder; the project moves there.
    let new = folder.parent().unwrap().join("kelp-2");
    archive::restore(&ctx, &job, "kelp", None, Some(new.to_str().unwrap())).unwrap();
    assert_eq!(contents(&new), before);
    let project = config(&ctx).project("kelp").cloned().unwrap();
    assert_eq!(project.folder.path, new);
    // Another project's object is refused.
    let error =
        archive::restore(&ctx, &job, "kelp", Some("kelpmux/web/1.tar.gz"), Some("/x")).unwrap_err();
    assert!(error.contains("not an archive of this project"), "{error}");
}

#[test]
fn the_test_writes_and_deletes_a_small_object() {
    let temp = Temp::new();
    let fake = Fake::start();
    let (ctx, _) = setup(&temp, &fake);
    let result = archive::test(&ctx).unwrap();
    assert_eq!(result["ok"], true);
    assert!(
        result["object"]
            .as_str()
            .unwrap()
            .starts_with("kelpmux/.kelpmux-test/")
    );
    assert!(fake.with(|s| s.objects.is_empty()));
    let status = archive::status(&config(&ctx));
    assert_eq!(status["configured"], true);
    assert_eq!(status["token_source"], "metadata");
    assert!(status.to_string().find(TOKEN).is_none());
}
