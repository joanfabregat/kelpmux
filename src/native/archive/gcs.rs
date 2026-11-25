//! Google Cloud Storage through its JSON API, with the machine's own Google
//! identity: no client library, no key file.
//!
//! # The token
//!
//! An OAuth access token comes from the GCE metadata server
//! (`http://metadata.google.internal`, plain HTTP and link-local by design;
//! its answer must carry `Metadata-Flavor: Google`), or from
//! `archive.token_command`, a program run like `github.token_command`
//! (no shell, no arguments, bounded output, error output discarded). It is
//! kept in this process's memory until shortly before it expires (at most
//! [`TOKEN_TTL`]) and sent only to `storage.googleapis.com` over HTTPS: never
//! to the browser, a file, a log or an error message.
//!
//! # Requests
//!
//! Requests go only to the API's origin (and the metadata host for the
//! token), never through a proxy from the environment and never following
//! a redirect. A resumable upload's session URI must be on the API's own
//! upload path, or the upload is refused before any byte is sent to it.
//! Uploads go in [`CHUNK`] pieces read from a file (bounded memory), each
//! retried after the server says how much it has; downloads stream to a
//! file. Responses read into memory are bounded.
use super::{Cancelled, Progress, tarball};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, String>;

/// The JSON API's origin.
pub const API: &str = "https://storage.googleapis.com";
/// The metadata server's token endpoint.
pub const METADATA_TOKEN: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
/// Bytes per upload request: a multiple of 256 KiB, as GCS requires.
pub const CHUNK: usize = 8 * 1024 * 1024;
/// Longest a token is kept, even when it lives longer.
pub const TOKEN_TTL: Duration = Duration::from_secs(45 * 60);
/// Largest JSON answer read.
const MAX_JSON: u64 = 4 * 1024 * 1024;
/// Tries of one upload chunk or request before giving up.
const TRIES: u32 = 6;
/// Most archives listed for a project.
pub const MAX_LISTED: usize = 1000;

/// Where the token comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenSource {
    /// The metadata server at this URL ([`METADATA_TOKEN`]).
    Metadata(String),
    /// `archive.token_command`.
    Command(PathBuf),
    /// A fixed token (tests).
    Fixed(String),
}
impl TokenSource {
    /// What the browser is told about it.
    pub fn label(&self) -> String {
        match self {
            Self::Metadata(_) => "the machine's service account (GCE metadata server)".into(),
            Self::Command(path) => path.display().to_string(),
            Self::Fixed(_) => "a fixed token".into(),
        }
    }
}

/// An object's metadata, as the API gives it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Object {
    pub name: String,
    pub size: u64,
    /// Base64 of the big-endian CRC32C.
    pub crc32c: String,
    pub generation: String,
    pub updated: String,
    /// The `sha256` of its custom metadata, when set.
    pub sha256: Option<String>,
    pub metadata: serde_json::Map<String, Value>,
}
impl Object {
    fn parse(value: &Value) -> Result<Self> {
        let text = |key: &str| value[key].as_str().unwrap_or("").to_owned();
        let name = text("name");
        if name.is_empty() {
            return Err("Cloud Storage answered without an object name".into());
        }
        let metadata = value["metadata"].as_object().cloned().unwrap_or_default();
        Ok(Self {
            name,
            size: text("size")
                .parse()
                .map_err(|_| "Cloud Storage answered without an object size")?,
            crc32c: text("crc32c"),
            generation: text("generation"),
            updated: text("updated"),
            sha256: metadata
                .get("sha256")
                .and_then(Value::as_str)
                .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                .map(str::to_lowercase),
            metadata,
        })
    }
}

/// Percent-encodes a path segment or query value: everything but
/// unreserved characters.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// A Cloud Storage client.
pub struct Gcs {
    agent: ureq::Agent,
    /// For the metadata server only: plain HTTP.
    metadata: ureq::Agent,
    api: String,
    source: TokenSource,
    token: Mutex<Option<(String, Instant)>>,
}

fn agent(https_only: bool) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(https_only)
        .max_redirects(0)
        // Never a proxy from the environment: the token goes to Google
        // directly.
        .proxy(None)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(120)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .timeout_per_write(Some(Duration::from_secs(60)))
        .build()
        .into()
}

/// An answer read in full (bounded).
struct Answer {
    code: u16,
    location: Option<String>,
    range: Option<String>,
    body: Vec<u8>,
}
impl Answer {
    fn json(&self) -> Result<Value> {
        serde_json::from_slice(&self.body)
            .map_err(|_| "Cloud Storage sent an invalid answer".into())
    }
    /// An error that says what the API said, without echoing a token.
    fn error(&self, what: &str) -> String {
        let message = self
            .json()
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
            .unwrap_or_default();
        let message: String = message
            .chars()
            .filter(|c| !c.is_control())
            .take(300)
            .collect();
        match self.code {
            401 => format!(
                "{what}: Cloud Storage refused the token (HTTP 401); check the machine's identity or archive.token_command"
            ),
            403 => format!(
                "{what}: permission denied (HTTP 403){}{message}; the identity needs the Storage Object User role on the bucket",
                if message.is_empty() { "" } else { ": " }
            ),
            404 => format!(
                "{what}: not found (HTTP 404){}{message}",
                if message.is_empty() { "" } else { ": " }
            ),
            code => format!(
                "{what}: Cloud Storage returned HTTP {code}{}{message}",
                if message.is_empty() { "" } else { ": " }
            ),
        }
    }
}

fn read_answer(mut response: ureq::http::Response<ureq::Body>) -> Result<Answer> {
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let code = response.status().as_u16();
    let location = header("location");
    let range = header("range");
    let mut body = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_JSON + 1)
        .read_to_end(&mut body)
        .map_err(|_| "the Cloud Storage answer was interrupted")?;
    if body.len() as u64 > MAX_JSON {
        return Err("the Cloud Storage answer exceeds its size limit".into());
    }
    Ok(Answer {
        code,
        location,
        range,
        body,
    })
}

fn transient(code: u16) -> bool {
    matches!(code, 408 | 429 | 500 | 502 | 503 | 504)
}

/// Waits before try `n` (1, 2, 4… seconds, at most 16), or less when
/// cancelled.
fn backoff(n: u32, progress: Option<&Progress>) {
    let until = Instant::now() + Duration::from_secs(1 << n.min(4));
    while Instant::now() < until {
        if progress.is_some_and(Progress::cancelled) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

impl Gcs {
    /// The real API, with the token from `source`.
    pub fn new(source: TokenSource) -> Self {
        Self::with(API, source, true)
    }
    /// A local stand-in for the API (tests): plain HTTP allowed.
    pub fn local(base: &str, source: TokenSource) -> Self {
        Self::with(base, source, false)
    }
    fn with(api: &str, source: TokenSource, https_only: bool) -> Self {
        Self {
            agent: agent(https_only),
            metadata: agent(false),
            api: api.trim_end_matches('/').to_owned(),
            source,
            token: Mutex::new(None),
        }
    }
    pub fn source(&self) -> &TokenSource {
        &self.source
    }

    /// The token, from memory while fresh, else from its source. Errors
    /// never contain it.
    pub fn token(&self) -> Result<String> {
        if let Ok(cached) = self.token.lock()
            && let Some((token, until)) = cached.as_ref()
            && Instant::now() < *until
        {
            return Ok(token.clone());
        }
        let (token, lifetime) = match &self.source {
            TokenSource::Fixed(token) => (token.clone(), TOKEN_TTL),
            TokenSource::Command(path) => {
                let token = super::super::web_github::run_token_command(
                    path,
                    &[],
                    super::super::web_github::TOKEN_COMMAND_TIMEOUT,
                )
                .map_err(|e| format!("archive.token_command: {e}"))?;
                // Access tokens printed by a program live an hour; reread
                // well before.
                (token, Duration::from_secs(30 * 60))
            }
            TokenSource::Metadata(url) => self.metadata_token(url)?,
        };
        if let Ok(mut cached) = self.token.lock() {
            *cached = Some((token.clone(), Instant::now() + lifetime.min(TOKEN_TTL)));
        }
        Ok(token)
    }

    /// Forgets the token (after the API refused it).
    fn forget(&self) {
        if let Ok(mut cached) = self.token.lock() {
            *cached = None;
        }
    }

    fn metadata_token(&self, url: &str) -> Result<(String, Duration)> {
        let unavailable = "the GCE metadata server is unreachable: this is not a Google Cloud VM, or it blocks this process; set archive.token_command";
        let mut response = self
            .metadata
            .get(url)
            .header("Metadata-Flavor", "Google")
            .config()
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .call()
            .map_err(|_| unavailable)?;
        let flavor = response
            .headers()
            .get("metadata-flavor")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let code = response.status().as_u16();
        let mut body = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(64 * 1024)
            .read_to_end(&mut body)
            .map_err(|_| unavailable)?;
        if flavor.as_deref() != Some("Google") {
            return Err(unavailable.into());
        }
        if code != 200 {
            return Err(format!(
                "the GCE metadata server gave no token (HTTP {code}); does the VM have a service account with the cloud-platform or devstorage scope?"
            ));
        }
        let value: Value = serde_json::from_slice(&body)
            .map_err(|_| "the GCE metadata server sent an invalid token")?;
        body.fill(0);
        let token = value["access_token"]
            .as_str()
            .filter(|t| !t.is_empty() && t.len() < 4096 && t.bytes().all(|b| b.is_ascii_graphic()))
            .ok_or("the GCE metadata server sent an invalid token")?
            .to_owned();
        let expires = value["expires_in"].as_u64().unwrap_or(300);
        Ok((
            token,
            Duration::from_secs(expires.saturating_sub(60).max(10)),
        ))
    }

    /// A JSON API request with the token; retried on transient failures.
    fn call(
        &self,
        method: &str,
        url: &str,
        body: Option<(&str, &[u8])>,
        headers: &[(&str, String)],
        what: &str,
    ) -> Result<Answer> {
        let mut tries = 0;
        loop {
            tries += 1;
            let token = self.token()?;
            let builder = ureq::http::Request::builder()
                .method(method)
                .uri(url)
                .header("Authorization", format!("Bearer {token}"))
                .header("User-Agent", "KelpMux");
            let builder = headers
                .iter()
                .fold(builder, |b, (name, value)| b.header(*name, value));
            let result = match body {
                Some((kind, bytes)) => {
                    let request = builder
                        .header("Content-Type", kind)
                        .body(bytes.to_vec())
                        .map_err(|_| format!("{what}: invalid request"))?;
                    self.agent.run(request)
                }
                None => {
                    let request = builder
                        .body(())
                        .map_err(|_| format!("{what}: invalid request"))?;
                    self.agent.run(request)
                }
            };
            let answer = match result {
                Ok(response) => read_answer(response),
                Err(_) => Err(format!(
                    "{what}: Cloud Storage could not be reached; check the network"
                )),
            };
            match answer {
                Ok(answer) if answer.code == 401 && tries == 1 => {
                    // A token revoked or expired early: read it again once.
                    self.forget();
                }
                Ok(answer) if transient(answer.code) && tries < TRIES => backoff(tries, None),
                Err(_) if tries < TRIES => backoff(tries, None),
                Ok(answer) => {
                    if answer.code == 401 {
                        self.forget();
                    }
                    return Ok(answer);
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn object_url(&self, bucket: &str, name: &str) -> String {
        format!(
            "{}/storage/v1/b/{}/o/{}",
            self.api,
            encode(bucket),
            encode(name)
        )
    }

    /// An object's metadata; `None` when it does not exist.
    pub fn object(&self, bucket: &str, name: &str) -> Result<Option<Object>> {
        let answer = self.call(
            "GET",
            &self.object_url(bucket, name),
            None,
            &[],
            "Reading the archive's details",
        )?;
        match answer.code {
            200 => Ok(Some(Object::parse(&answer.json()?)?)),
            404 => Ok(None),
            _ => Err(answer.error("Reading the archive's details")),
        }
    }

    /// The objects whose names start with `prefix`, at most
    /// [`MAX_LISTED`].
    pub fn list(&self, bucket: &str, prefix: &str) -> Result<Vec<Object>> {
        let mut objects = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut url = format!(
                "{}/storage/v1/b/{}/o?prefix={}&maxResults=1000",
                self.api,
                encode(bucket),
                encode(prefix)
            );
            if let Some(page) = &page {
                url.push_str(&format!("&pageToken={}", encode(page)));
            }
            let answer = self.call("GET", &url, None, &[], "Listing the archives")?;
            if answer.code != 200 {
                return Err(answer.error("Listing the archives"));
            }
            let value = answer.json()?;
            for item in value["items"].as_array().into_iter().flatten() {
                if objects.len() >= MAX_LISTED {
                    return Ok(objects);
                }
                objects.push(Object::parse(item)?);
            }
            match value["nextPageToken"].as_str() {
                Some(next) if !next.is_empty() && next.len() < 4096 => page = Some(next.to_owned()),
                _ => return Ok(objects),
            }
        }
    }

    /// Deletes an object (one generation of it when given).
    pub fn delete(&self, bucket: &str, name: &str, generation: Option<&str>) -> Result<()> {
        let mut url = self.object_url(bucket, name);
        if let Some(generation) = generation {
            url.push_str(&format!("?generation={}", encode(generation)));
        }
        let answer = self.call("DELETE", &url, None, &[], "Deleting the object")?;
        if matches!(answer.code, 200 | 204) {
            Ok(())
        } else {
            Err(answer.error("Deleting the object"))
        }
    }

    /// Uploads a small object in one request, failing when it exists.
    pub fn put_small(&self, bucket: &str, name: &str, bytes: &[u8]) -> Result<Object> {
        let url = format!(
            "{}/upload/storage/v1/b/{}/o?uploadType=media&ifGenerationMatch=0&name={}",
            self.api,
            encode(bucket),
            encode(name)
        );
        let answer = self.call(
            "POST",
            &url,
            Some(("text/plain", bytes)),
            &[],
            "Writing a test object",
        )?;
        if answer.code != 200 {
            return Err(answer.error("Writing a test object"));
        }
        Object::parse(&answer.json()?)
    }

    /// Uploads `file` (`size` bytes, with this CRC32C) as a new object
    /// `name` with custom `metadata`, through a resumable upload in
    /// [`CHUNK`] pieces. The object must not exist yet; Cloud Storage
    /// itself checks the CRC32C given at the start against what it
    /// received. Returns the object as stored.
    #[allow(clippy::too_many_arguments)]
    pub fn upload(
        &self,
        bucket: &str,
        name: &str,
        mut file: File,
        size: u64,
        crc32c: u32,
        metadata: serde_json::Map<String, Value>,
        progress: &Progress,
    ) -> Result<Object> {
        let what = "Uploading the archive";
        let start = format!(
            "{}/upload/storage/v1/b/{}/o?uploadType=resumable&ifGenerationMatch=0",
            self.api,
            encode(bucket)
        );
        let resource = json!({
            "name": name,
            "contentType": "application/gzip",
            "crc32c": tarball::crc32c_base64(crc32c),
            "metadata": metadata,
        });
        let body = serde_json::to_vec(&resource).map_err(|e| e.to_string())?;
        let answer = self.call(
            "POST",
            &start,
            Some(("application/json; charset=UTF-8", &body)),
            &[
                ("X-Upload-Content-Type", "application/gzip".into()),
                ("X-Upload-Content-Length", size.to_string()),
            ],
            what,
        )?;
        if answer.code != 200 {
            return Err(answer.error(what));
        }
        let session = answer
            .location
            .ok_or("Cloud Storage started no upload session")?;
        // The session URI is a capability: only ever the API's own upload
        // path of this bucket.
        let expected = format!("{}/upload/storage/v1/b/{}/o?", self.api, encode(bucket));
        if !session.starts_with(&expected) || session.chars().any(|c| c.is_control() || c == ' ') {
            return Err(
                "Cloud Storage returned an unexpected upload session; nothing was sent".into(),
            );
        }
        let result = self.send_chunks(&session, &mut file, size, progress);
        if result.is_err() {
            // Ends the session; a failure to do so only leaves it to expire.
            let _ = self
                .agent
                .delete(&session)
                .header("Content-Length", "0")
                .call();
        }
        let object = result?;
        if object.name != name {
            return Err("Cloud Storage stored the archive under another name".into());
        }
        Ok(object)
    }

    fn send_chunks(
        &self,
        session: &str,
        file: &mut File,
        size: u64,
        progress: &Progress,
    ) -> Result<Object> {
        let what = "Uploading the archive";
        let mut offset = 0u64;
        let mut buffer = vec![0u8; CHUNK];
        let mut failures = 0u32;
        loop {
            if progress.cancelled() {
                return Err(Cancelled.to_string());
            }
            let length = (size - offset).min(CHUNK as u64) as usize;
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| e.to_string())?;
            file.read_exact(&mut buffer[..length])
                .map_err(|e| e.to_string())?;
            let range = if size == 0 {
                "bytes */0".to_owned()
            } else {
                format!("bytes {offset}-{}/{size}", offset + length as u64 - 1)
            };
            let sent = self
                .agent
                .put(session)
                .header("Content-Range", &range)
                .header("User-Agent", "KelpMux")
                .send(&buffer[..length]);
            let answer = match sent {
                Ok(response) => read_answer(response).ok(),
                Err(_) => None,
            };
            match answer {
                Some(answer) if matches!(answer.code, 200 | 201) => {
                    progress.set_bytes(size);
                    return Object::parse(&answer.json()?);
                }
                Some(answer) if answer.code == 308 => {
                    offset = committed(answer.range.as_deref())?;
                    progress.set_bytes(offset);
                    failures = 0;
                }
                Some(answer) if !transient(answer.code) => return Err(answer.error(what)),
                _ => {
                    failures += 1;
                    if failures >= TRIES {
                        return Err(format!(
                            "{what}: Cloud Storage kept failing; try again later"
                        ));
                    }
                    backoff(failures, Some(progress));
                    // Asks how much arrived, and resumes there.
                    match self.committed(session, size)? {
                        Ok(arrived) => offset = arrived,
                        Err(object) => {
                            progress.set_bytes(size);
                            return Ok(object);
                        }
                    }
                    progress.set_bytes(offset);
                }
            }
            if offset > size {
                return Err(format!(
                    "{what}: Cloud Storage reports more bytes than sent"
                ));
            }
        }
    }

    /// How many bytes of the upload the server has (`Ok`), or the object
    /// when the upload is complete (`Err`).
    fn committed(&self, session: &str, size: u64) -> Result<std::result::Result<u64, Object>> {
        let answer = self
            .agent
            .put(session)
            .header("Content-Range", format!("bytes */{size}"))
            .send_empty()
            .map_err(|_| "Uploading the archive: Cloud Storage could not be reached")
            .and_then(|r| read_answer(r).map_err(|_| "Uploading the archive: interrupted"))?;
        match answer.code {
            308 => Ok(Ok(committed(answer.range.as_deref())?)),
            200 | 201 => Ok(Err(Object::parse(&answer.json()?)?)),
            _ => Err(answer.error("Uploading the archive")),
        }
    }

    /// Downloads an object (this generation) into `out`, and returns its
    /// digests (SHA-256, CRC32C, length).
    pub fn download(
        &self,
        bucket: &str,
        name: &str,
        generation: &str,
        size: u64,
        out: &mut File,
        progress: &Progress,
    ) -> Result<(String, u32, u64)> {
        let what = "Downloading the archive";
        let url = format!(
            "{}?alt=media&generation={}",
            self.object_url(bucket, name),
            encode(generation)
        );
        let token = self.token()?;
        let mut response = self
            .agent
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "KelpMux")
            .header("Accept-Encoding", "identity")
            .call()
            .map_err(|_| format!("{what}: Cloud Storage could not be reached"))?;
        if response.status().as_u16() != 200 {
            let answer = read_answer(response)?;
            return Err(answer.error(what));
        }
        let mut digests = tarball::Digests::default();
        // Never more than the object's size (one byte more shows a
        // longer answer).
        let mut reader = response.body_mut().as_reader().take(size + 1);
        let mut total = 0u64;
        let mut buffer = vec![0u8; 256 * 1024];
        loop {
            if progress.cancelled() {
                return Err(Cancelled.to_string());
            }
            let n = reader
                .read(&mut buffer)
                .map_err(|_| format!("{what}: the download was interrupted"))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > size {
                return Err(format!(
                    "{what}: Cloud Storage sent more than the archive's {size} bytes"
                ));
            }
            out.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
            digests.update(&buffer[..n]);
            progress.add_bytes(n as u64);
        }
        out.sync_all().map_err(|e| e.to_string())?;
        Ok(digests.finish())
    }
}

/// The bytes committed, from a 308 answer's `Range: bytes=0-N` (none: 0).
fn committed(range: Option<&str>) -> Result<u64> {
    match range {
        None => Ok(0),
        Some(range) => range
            .strip_prefix("bytes=0-")
            .and_then(|last| last.trim().parse::<u64>().ok())
            .map(|last| last + 1)
            .ok_or_else(|| "Cloud Storage sent an invalid upload range".into()),
    }
}
