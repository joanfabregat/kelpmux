//! Static hosting and a bounded, fixed-destination SSH byte relay. No workspace
//! socket, key enrollment, account switch, SSH termination or command execution.
//! Moved from KelpServer (ADR 0002 step 2): the relay serves KelpMux's web UI,
//! so `kelpmux web-relay` uses this copy.
use crate::Result;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tungstenite::{
    Message, accept_hdr_with_config,
    handshake::server::{ErrorResponse, Request, Response},
    protocol::WebSocketConfig,
};
pub type Assets = &'static [(&'static str, &'static [u8])];
#[cfg(test)]
const ASSETS: Assets = &[];
const BUFFER: usize = 1024 * 1024;

fn pinned_key(key: &str) -> Result<Vec<u8>> {
    let encoded = key
        .strip_prefix("ssh-ed25519 ")
        .ok_or("Ed25519 host key required")?;
    let blob = STANDARD.decode(encoded)?;
    // RFC 8709: string "ssh-ed25519", string containing the 32-byte key.
    if blob.len() != 51 || &blob[..19] != b"\0\0\0\x0bssh-ed25519\0\0\0\x20" {
        return Err("invalid Ed25519 host key".into());
    }
    Ok(blob)
}

/// Inspect only the initial plaintext server handshake on the actual forwarded
/// connection. This enforces the public-key pin; OpenSSH in the browser verifies
/// possession of its private key using the exchange signature. The relay never
/// derives session keys, decrypts traffic or authenticates on the user's behalf.
struct HostKeyGate {
    expected: Vec<u8>,
    pending: Vec<u8>,
    banner: bool,
    banner_bytes: usize,
    packets: usize,
    checked: bool,
}
impl HostKeyGate {
    fn new(key: &str) -> Result<Self> {
        Ok(Self {
            expected: pinned_key(key)?,
            pending: Vec::new(),
            banner: false,
            banner_bytes: 0,
            packets: 0,
            checked: false,
        })
    }
    fn receive(&mut self, bytes: &[u8]) -> Result<Vec<u8>> {
        if self.checked {
            return Ok(bytes.to_vec());
        }
        if self.pending.len() + bytes.len() > 131072 {
            return Err("SSH handshake buffer limit".into());
        }
        self.pending.extend_from_slice(bytes);
        let mut forward = Vec::new();
        loop {
            if !self.banner {
                let Some(end) = self.pending.iter().position(|b| *b == b'\n') else {
                    if self.banner_bytes + self.pending.len() > 8192 {
                        return Err("SSH banner limit".into());
                    }
                    break;
                };
                self.banner_bytes += end + 1;
                if self.banner_bytes > 8192 {
                    return Err("SSH banner limit".into());
                }
                let line = &self.pending[..=end];
                if line.starts_with(b"SSH-") {
                    if !line.starts_with(b"SSH-2.0-") || line.len() > 255 {
                        return Err("unsupported SSH identification".into());
                    }
                    self.banner = true;
                }
                forward.extend(self.pending.drain(..=end));
                continue;
            }
            if self.pending.len() < 5 {
                break;
            }
            let length = u32::from_be_bytes(self.pending[..4].try_into().unwrap()) as usize;
            let padding = self.pending[4] as usize;
            if !(12..=65536).contains(&length)
                || (length + 4) % 8 != 0
                || padding < 4
                || length < padding + 2
            {
                return Err("invalid SSH handshake packet".into());
            }
            if self.pending.len() < length + 4 {
                break;
            }
            self.packets += 1;
            if self.packets > 32 {
                return Err("SSH handshake packet limit".into());
            }
            let payload = &self.pending[5..4 + length - padding];
            match payload[0] {
                // KEXINIT, IGNORE and DEBUG do not establish server identity.
                20 | 2 | 4 => {}
                // KEX_ECDH_REPLY: the first SSH string is the host public key.
                // Browser OpenSSH is restricted to curve25519-sha256 below.
                31 => {
                    if payload.len() < 5 {
                        return Err("truncated SSH host key".into());
                    }
                    let size = u32::from_be_bytes(payload[1..5].try_into().unwrap()) as usize;
                    if size != self.expected.len()
                        || payload.get(5..5 + size) != Some(self.expected.as_slice())
                    {
                        return Err("SSH server host key does not match relay configuration".into());
                    }
                    self.checked = true;
                }
                _ => return Err("SSH handshake ended before host-key verification".into()),
            }
            forward.extend(self.pending.drain(..length + 4));
            if self.checked {
                forward.append(&mut self.pending);
                break;
            }
        }
        Ok(forward)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub listen: SocketAddr,
    pub origin: String,
    pub ssh_target: String,
    pub ssh_host_key: String,
    pub username: String,
    pub bridge_command: String,
    pub workspace: String,
    #[serde(default)]
    pub dev: bool,
    /// Optional public GlitchTip DSN. Unset means no error reporting: the
    /// browser is not told about one and loads no reporting code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glitchtip_dsn: Option<String>,
    /// Optional environment name for reported errors (e.g. `preprod`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_environment: Option<String>,
}
impl Config {
    /// Validated error-reporting settings; `None` when no DSN is configured.
    pub fn error_reporting(&self) -> Result<Option<super::error_reporting::Settings>> {
        super::error_reporting::Settings::new(
            self.glitchtip_dsn.as_deref(),
            self.error_environment.as_deref(),
        )
    }
    pub fn validate(&self) -> Result<()> {
        if !self.enabled {
            return Err("web relay is disabled in configuration".into());
        }
        if !self.origin.starts_with("https://")
            || self.origin[8..].is_empty()
            || self.origin[8..].contains('/')
            || self.origin.chars().any(char::is_control)
        {
            return Err("web origin must be an exact HTTPS origin".into());
        }
        if !self.ssh_host_key.starts_with("ssh-ed25519 ")
            || self.ssh_host_key.len() > 1024
            || self.ssh_host_key.chars().any(char::is_control)
        {
            return Err("an operator-pinned Ed25519 SSH host key is required".into());
        }
        pinned_key(&self.ssh_host_key)?;
        if self.username.is_empty()
            || self.username.len() > 64
            || !self
                .username
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            return Err("invalid configured SSH user".into());
        }
        if self.bridge_command.is_empty()
            || self.bridge_command.len() > 4096
            || self.bridge_command.chars().any(char::is_control)
        {
            return Err("invalid bridge command".into());
        }
        super::lifecycle::workspace_path(&self.workspace)?;
        self.error_reporting()?;
        Ok(())
    }
    /// The bootstrap document the browser reads from `/connection.json`.
    /// `errors` is present only when a DSN is configured.
    fn connection(&self) -> Result<serde_json::Value> {
        let mut value = json!({"origin":self.origin,"rpId":self.origin.trim_start_matches("https://").split(':').next().unwrap_or(""),"hostPublic":self.ssh_host_key,"username":self.username,"command":self.bridge_command,"workspace":self.workspace,"dev":self.dev});
        if let Some(settings) = self.error_reporting()? {
            let mut errors = json!({"dsn": settings.dsn.as_str()});
            if let Some(environment) = settings.environment {
                errors["environment"] = environment.into();
            }
            value["errors"] = errors;
        }
        Ok(value)
    }
    /// The Content-Security-Policy of every static response. `connect-src`
    /// adds the GlitchTip origin only when a DSN is configured.
    fn content_security_policy(&self) -> String {
        let reporting = match self.error_reporting() {
            Ok(Some(settings)) => format!(" {}", settings.dsn.origin()),
            _ => String::new(),
        };
        format!(
            "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; connect-src 'self'{reporting}; worker-src 'self'; img-src 'self' blob: data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"
        )
    }
}
#[derive(Default)]
struct Budget {
    // Window start, requests, active HTTP handlers, active SSH relays.
    peers: BTreeMap<IpAddr, (Instant, usize, usize, usize)>,
    ssh: usize,
}
impl Budget {
    fn claim(&mut self, ip: IpAddr) -> bool {
        let now = Instant::now();
        self.peers.retain(|_, (at, _, active, _)| {
            *active > 0 || now.duration_since(*at) < Duration::from_secs(60)
        });
        if self.peers.len() >= 256 && !self.peers.contains_key(&ip) {
            return false;
        }
        let peer = self.peers.entry(ip).or_insert((now, 0, 0, 0));
        if now.duration_since(peer.0) >= Duration::from_secs(60) {
            peer.0 = now;
            peer.1 = 0;
        }
        if peer.1 >= 512 || peer.2 >= 64 {
            return false;
        }
        peer.1 += 1;
        peer.2 += 1;
        true
    }
    fn release(&mut self, ip: IpAddr) {
        if let Some(peer) = self.peers.get_mut(&ip) {
            peer.2 = peer.2.saturating_sub(1);
        }
    }
    fn claim_ssh(&mut self, ip: IpAddr) -> bool {
        let Some(peer) = self.peers.get_mut(&ip) else {
            return false;
        };
        if self.ssh >= 32 || peer.3 >= 8 {
            return false;
        }
        self.ssh += 1;
        peer.3 += 1;
        true
    }
    fn release_ssh(&mut self, ip: IpAddr) {
        if let Some(peer) = self.peers.get_mut(&ip) {
            peer.3 = peer.3.saturating_sub(1);
        }
        self.ssh = self.ssh.saturating_sub(1);
    }
}
struct SshPermit<'a> {
    budget: &'a Mutex<Budget>,
    ip: IpAddr,
}
impl Drop for SshPermit<'_> {
    fn drop(&mut self) {
        self.budget.lock().unwrap().release_ssh(self.ip);
    }
}
fn response(
    stream: &mut TcpStream,
    csp: &str,
    status: &str,
    content_type: &str,
    body: &[u8],
    head: bool,
) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-cache\r\nCross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Embedder-Policy: require-corp\r\nCross-Origin-Resource-Policy: same-origin\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: {csp}\r\n\r\n",
        body.len()
    )?;
    if !head {
        stream.write_all(body)?;
    }
    Ok(())
}
fn serve(
    mut socket: TcpStream,
    config: &Config,
    target: SocketAddr,
    budget: &Mutex<Budget>,
    assets: Assets,
) -> Result<()> {
    socket.set_read_timeout(Some(Duration::from_secs(3)))?;
    socket.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut peek = [0u8; 16384];
    let started = Instant::now();
    let count = loop {
        let n = socket.peek(&mut peek)?;
        if n == 0 {
            return Ok(());
        }
        if peek[..n].windows(4).any(|p| p == b"\r\n\r\n") {
            break n;
        }
        if n == peek.len() || started.elapsed() > Duration::from_secs(3) {
            return Err("HTTP header limit".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let header = std::str::from_utf8(&peek[..count])?;
    let line = header.lines().next().ok_or("missing request")?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    if path == "/ssh" && method == "GET" {
        let expected = config.origin.clone();
        #[allow(clippy::result_large_err)]
        // Required by tungstenite’s handshake callback signature.
        let accept = move |request: &Request,
                           response: Response|
              -> std::result::Result<Response, ErrorResponse> {
            if request.uri().path() != "/ssh"
                || request.uri().query().is_some()
                || request.headers().get_all("origin").iter().count() != 1
                || request
                    .headers()
                    .get("origin")
                    .and_then(|v| v.to_str().ok())
                    != Some(expected.as_str())
            {
                return Err(tungstenite::http::Response::builder()
                    .status(403)
                    .body(Some("Origin denied".into()))
                    .unwrap());
            }
            Ok(response)
        };
        let limits = WebSocketConfig::default()
            .max_message_size(Some(65536))
            .max_frame_size(Some(65536))
            .max_write_buffer_size(BUFFER)
            .write_buffer_size(0);
        let mut ws = accept_hdr_with_config(socket, accept, Some(limits))?;
        let ip = ws.get_ref().peer_addr()?.ip();
        if !budget.lock().unwrap().claim_ssh(ip) {
            return Err("SSH connection budget exceeded".into());
        }
        let _permit = SshPermit { budget, ip };
        let mut tcp = TcpStream::connect_timeout(&target, Duration::from_secs(5))?;
        let mut host_key = HostKeyGate::new(&config.ssh_host_key)?;
        let handshake_started = Instant::now();
        tcp.set_nonblocking(true)?;
        tcp.set_nodelay(true)?;
        ws.get_mut().set_nonblocking(true)?;
        // Terminal updates are small and latency-sensitive in both directions.
        // Disabling Nagle only on the SSH leg leaves browser-bound updates
        // waiting for TCP acknowledgements at the reverse proxy.
        ws.get_mut().set_nodelay(true)?;
        let mut pending = Vec::new();
        let mut buffer = [0u8; 32768];
        let mut progress = Instant::now();
        loop {
            for _ in 0..8 {
                match ws.read() {
                    Ok(Message::Binary(data)) => {
                        if pending.len() + data.len() > BUFFER {
                            return Err("relay buffer limit".into());
                        }
                        pending.extend_from_slice(&data);
                        progress = Instant::now();
                    }
                    Ok(Message::Close(_)) => return Ok(()),
                    Ok(Message::Ping(_) | Message::Pong(_)) => {}
                    Ok(_) => return Err("binary SSH frames required".into()),
                    Err(tungstenite::Error::Io(e))
                        if e.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        break;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            if !pending.is_empty() {
                match tcp.write(&pending) {
                    Ok(0) => return Ok(()),
                    Ok(n) => {
                        pending.drain(..n);
                        progress = Instant::now();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.into()),
                }
            }
            for _ in 0..8 {
                match tcp.read(&mut buffer) {
                    Ok(0) => return Ok(()),
                    Ok(n) => {
                        let verified = host_key.receive(&buffer[..n])?;
                        if verified.is_empty() {
                            continue;
                        }
                        match ws.send(Message::Binary(verified.into())) {
                            Ok(()) => {}
                            Err(tungstenite::Error::Io(e))
                                if e.kind() == std::io::ErrorKind::WouldBlock => {}
                            Err(e) => return Err(e.into()),
                        }
                        progress = Instant::now();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(e.into()),
                }
            }
            let ws_blocked = match ws.flush() {
                Ok(()) => false,
                Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    true
                }
                Err(e) => return Err(e.into()),
            };
            if progress.elapsed() > Duration::from_secs(180) {
                return Ok(());
            }
            if !host_key.checked && handshake_started.elapsed() > Duration::from_secs(15) {
                return Err("SSH host-key verification timed out".into());
            }
            // Wake as soon as either leg has input instead of imposing a
            // 10 ms sleep on every interactive exchange. Only wait for writes
            // when backpressure actually left data queued.
            use std::os::fd::AsRawFd;
            let mut fds = [
                libc::pollfd {
                    fd: ws.get_ref().as_raw_fd(),
                    events: libc::POLLIN | if ws_blocked { libc::POLLOUT } else { 0 },
                    revents: 0,
                },
                libc::pollfd {
                    fd: tcp.as_raw_fd(),
                    events: libc::POLLIN | if pending.is_empty() { 0 } else { libc::POLLOUT },
                    revents: 0,
                },
            ];
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, 10) };
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error.into());
                }
            }
        }
    }
    // Consume the request header before closing a static response. Dropping a
    // TCP socket with unread request bytes causes a reset, which can truncate
    // a large body at an HTTP/2 reverse proxy even after a 200 status arrived.
    let header_length = peek[..count]
        .windows(4)
        .position(|p| p == b"\r\n\r\n")
        .ok_or("missing HTTP header terminator")?
        + 4;
    socket.read_exact(&mut vec![0u8; header_length])?;
    let csp = config.content_security_policy();
    if method != "GET" && method != "HEAD" {
        return response(
            &mut socket,
            &csp,
            "405 Method Not Allowed",
            "text/plain",
            b"Read-only static service",
            false,
        );
    }
    if path == "/connection.json" {
        return response(
            &mut socket,
            &csp,
            "200 OK",
            "application/json",
            &serde_json::to_vec(&config.connection()?)?,
            method == "HEAD",
        );
    }
    let path = path.split('?').next().unwrap_or(path);
    let path = if path == "/" { "/index.html" } else { path };
    if let Some((_, body)) = assets.iter().find(|(name, _)| *name == path) {
        response(
            &mut socket,
            &csp,
            "200 OK",
            content_type(path),
            body,
            method == "HEAD",
        )
    } else {
        response(
            &mut socket,
            &csp,
            "404 Not Found",
            "text/plain",
            b"Not found",
            method == "HEAD",
        )
    }
}
/// Media type of an embedded asset. Every response also carries
/// `X-Content-Type-Options: nosniff`, so browsers rely on this value: the
/// web manifest and icons must not be served as `application/octet-stream`.
fn content_type(path: &str) -> &'static str {
    let extension = path.rsplit_once('.').map_or("", |(_, extension)| extension);
    match extension {
        "html" => "text/html; charset=utf-8",
        "svg" => "image/svg+xml",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "wasm" => "application/wasm",
        "woff2" => "font/woff2",
        "png" => "image/png",
        "webmanifest" => "application/manifest+json",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}
pub fn run(path: &Path) -> Result<()> {
    run_with_assets(path, &[])
}

/// Caller supplies embedded, immutable assets; the generic relay contains no UI.
pub fn run_with_assets(path: &Path, assets: Assets) -> Result<()> {
    let bytes = fs::read(path)?;
    if bytes.len() > 65536 {
        return Err("web configuration too large".into());
    }
    let config: Config = serde_json::from_slice(&bytes)?;
    config.validate()?;
    let target = config
        .ssh_target
        .to_socket_addrs()?
        .next()
        .ok_or("SSH target did not resolve")?;
    let listener = TcpListener::bind(config.listen)?;
    let config = Arc::new(config);
    let clients = Arc::new(AtomicUsize::new(0));
    let budget = Arc::new(Mutex::new(Budget::default()));
    eprintln!(
        "Kelpmux static web + SSH byte relay: {} (workspace {})",
        config.origin, config.workspace
    );
    // The relay's own errors and panics will be reported from here with
    // these settings (component `relay`) once the Rust reporter lands; today
    // only the browser reports (see super::error_reporting).
    if let Some(settings) = config.error_reporting()? {
        eprintln!(
            "Browser error reporting to {} (release {})",
            settings.dsn.origin(),
            super::error_reporting::RELEASE
        );
    }
    for socket in listener.incoming() {
        let Ok(socket) = socket else {
            continue;
        };
        let ip = socket.peer_addr()?.ip();
        if clients.fetch_add(1, Ordering::Relaxed) >= 128 {
            clients.fetch_sub(1, Ordering::Relaxed);
            continue;
        }
        if !budget.lock().unwrap().claim(ip) {
            clients.fetch_sub(1, Ordering::Relaxed);
            continue;
        }
        let config = config.clone();
        let clients = clients.clone();
        let budget = budget.clone();
        std::thread::spawn(move || {
            let _ = serve(socket, &config, target, &budget, assets);
            budget.lock().unwrap().release(ip);
            clients.fetch_sub(1, Ordering::Relaxed);
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn asset_bursts_do_not_consume_ssh_session_quota() {
        let ip = "127.0.0.1".parse().unwrap();
        let mut budget = Budget::default();
        for _ in 0..48 {
            assert!(budget.claim(ip));
        }
        for _ in 0..8 {
            assert!(budget.claim_ssh(ip));
        }
        assert!(!budget.claim_ssh(ip));
        assert!(budget.claim(ip));
        budget.release_ssh(ip);
        assert!(budget.claim_ssh(ip));
        for i in 2..=4 {
            let ip = format!("127.0.0.{i}").parse().unwrap();
            assert!(budget.claim(ip));
            for _ in 0..8 {
                assert!(budget.claim_ssh(ip));
            }
        }
        let ip = "127.0.0.5".parse().unwrap();
        assert!(budget.claim(ip));
        assert!(!budget.claim_ssh(ip));
    }
    fn packet(payload: &[u8]) -> Vec<u8> {
        let mut padding = 4;
        while (5 + payload.len() + padding) % 8 != 0 {
            padding += 1;
        }
        let mut bytes = ((1 + payload.len() + padding) as u32)
            .to_be_bytes()
            .to_vec();
        bytes.push(padding as u8);
        bytes.extend_from_slice(payload);
        bytes.resize(bytes.len() + padding, 0);
        bytes
    }
    fn exchange(key: &[u8]) -> Vec<u8> {
        let mut bytes = b"SSH-2.0-test\r\n".to_vec();
        bytes.extend(packet(&[20, 0]));
        let mut reply = vec![31];
        reply.extend((key.len() as u32).to_be_bytes());
        reply.extend(key);
        bytes.extend(packet(&reply));
        bytes.extend(b"encrypted bytes");
        bytes
    }
    #[test]
    fn relay_checks_each_actual_handshake_even_when_fragmented() {
        let config = config();
        let key = pinned_key(&config.ssh_host_key).unwrap();
        let wire = exchange(&key);
        for size in 1..=wire.len() {
            let mut gate = HostKeyGate::new(&config.ssh_host_key).unwrap();
            let mut forwarded = Vec::new();
            for chunk in wire.chunks(size) {
                forwarded.extend(gate.receive(chunk).unwrap());
            }
            assert!(gate.checked);
            assert_eq!(forwarded, wire);
        }
        let mut wrong = key.clone();
        wrong[50] ^= 1;
        let mut gate = HostKeyGate::new(&config.ssh_host_key).unwrap();
        assert!(gate.receive(&exchange(&wrong)).is_err());
        assert!(!gate.checked);
        // A successful previous connection never authorizes the next one.
        let mut fresh = HostKeyGate::new(&config.ssh_host_key).unwrap();
        let mut premature = b"SSH-2.0-test\r\n".to_vec();
        premature.extend(packet(&[21]));
        assert!(fresh.receive(&premature).is_err());
    }
    #[test]
    fn host_key_gate_rejects_malformed_and_unbounded_handshakes() {
        let key = config().ssh_host_key;
        assert!(pinned_key("ssh-ed25519 YWJj").is_err());
        let mut gate = HostKeyGate::new(&key).unwrap();
        assert!(gate.receive(&vec![b'x'; 8193]).is_err());
        let mut gate = HostKeyGate::new(&key).unwrap();
        let mut wire = b"SSH-2.0-test\r\n".to_vec();
        wire.extend([255; 5]);
        assert!(gate.receive(&wire).is_err());
        let mut gate = HostKeyGate::new(&key).unwrap();
        let mut wire = b"SSH-2.0-test\r\n".to_vec();
        wire.extend(packet(&[31, 255, 255, 255, 255]));
        assert!(gate.receive(&wire).is_err());
    }
    fn config() -> Config {
        Config {
            enabled: true,
            listen: "127.0.0.1:0".parse().unwrap(),
            origin: "https://terminal.example.com".into(),
            ssh_target: "127.0.0.1:22".into(),
            ssh_host_key:
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHFN8js0LQln1YTEyrBp7QhcwqZrIc7yVnNEzFkyaTbV"
                    .into(),
            username: "test".into(),
            bridge_command: "/usr/bin/kelpmux web-bridge --workspace dev".into(),
            workspace: "dev".into(),
            dev: true,
            glitchtip_dsn: None,
            error_environment: None,
        }
    }
    fn reporting_config() -> Config {
        Config {
            glitchtip_dsn: Some("https://abc123@errors.example.com/7".into()),
            error_environment: Some("preprod".into()),
            ..config()
        }
    }
    #[test]
    fn interactive_relay_round_trip_probe() {
        use tungstenite::client::IntoClientRequest;
        let wire = exchange(&pinned_key(&config().ssh_host_key).unwrap());
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        let target_address = target.local_addr().unwrap();
        let expected = wire.len();
        let echo = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut socket, _) = target.accept().unwrap();
                socket.set_nodelay(true).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                socket.write_all(&wire).unwrap();
                let mut byte = [0u8; 1];
                while socket.read_exact(&mut byte).is_ok() {
                    if socket.write_all(&byte).is_err() {
                        break;
                    }
                }
            }
        });
        let mut direct = TcpStream::connect(target_address).unwrap();
        direct.set_nodelay(true).unwrap();
        direct
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        direct.read_exact(&mut vec![0; expected]).unwrap();
        let mut direct_times = Vec::new();
        for n in 0..24u8 {
            let at = Instant::now();
            direct.write_all(&[n]).unwrap();
            let mut received = [0];
            direct.read_exact(&mut received).unwrap();
            assert_eq!(received, [n]);
            direct_times.push(at.elapsed().as_secs_f64() * 1000.0);
        }
        drop(direct);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let relay = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            let mut budget = Budget::default();
            assert!(budget.claim(socket.peer_addr().unwrap().ip()));
            let _ = serve(
                socket,
                &config(),
                target_address,
                &Mutex::new(budget),
                ASSETS,
            );
        });
        let mut request = format!("ws://{address}/ssh").into_client_request().unwrap();
        request
            .headers_mut()
            .insert("Origin", config().origin.parse().unwrap());
        let socket = TcpStream::connect(address).unwrap();
        socket.set_nodelay(true).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (mut ws, _) = tungstenite::client(request, socket).unwrap();
        let mut received = 0;
        while received < expected {
            if let Message::Binary(bytes) = ws.read().unwrap() {
                received += bytes.len();
            }
        }
        let mut relay_times = Vec::new();
        for n in 0..24u8 {
            let at = Instant::now();
            ws.send(Message::Binary(vec![n].into())).unwrap();
            assert_eq!(ws.read().unwrap().into_data().as_ref(), &[n]);
            relay_times.push(at.elapsed().as_secs_f64() * 1000.0);
        }
        drop(ws);
        relay.join().unwrap();
        echo.join().unwrap();
        direct_times.sort_by(f64::total_cmp);
        relay_times.sort_by(f64::total_cmp);
        println!(
            "Local transport RTT, 24 samples: direct TCP median={:.3}ms p95={:.3}ms; WebSocket relay median={:.3}ms p95={:.3}ms (excludes VPN/TLS/WASSH/application)",
            direct_times[12], direct_times[22], relay_times[12], relay_times[22]
        );
    }
    fn http(request: &[u8]) -> Vec<u8> {
        http_with(config(), request)
    }
    fn http_with(config: Config, request: &[u8]) -> Vec<u8> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            serve(
                socket,
                &config,
                "127.0.0.1:22".parse().unwrap(),
                &Mutex::new(Budget::default()),
                ASSETS,
            )
            .unwrap();
        });
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client.write_all(request).unwrap();
        let mut response = Vec::new();
        client
            .read_to_end(&mut response)
            .expect("static response must end with EOF, not a TCP reset");
        worker.join().unwrap();
        response
    }
    #[test]
    fn static_response_consumes_headers_and_delivers_complete_body() {
        let response = http(b"GET /connection.json HTTP/1.1\r\nHost: terminal.example.com\r\n\r\n");
        let split = response.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
        let headers = std::str::from_utf8(&response[..split]).unwrap();
        let length: usize = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(length, response.len() - split);
        let json: serde_json::Value = serde_json::from_slice(&response[split..]).unwrap();
        assert_eq!(json["workspace"], "dev");
        if !ASSETS.is_empty() {
            let response =
                http(b"GET /client-core.wasm HTTP/1.1\r\nHost: terminal.example.com\r\n\r\n");
            assert!(response.len() > 65536);
        }
    }
    #[test]
    fn assets_carry_their_media_types_under_nosniff() {
        for (path, expected) in [
            ("/index.html", "text/html; charset=utf-8"),
            ("/icon-192.png", "image/png"),
            ("/manifest.webmanifest", "application/manifest+json"),
            ("/connection-hints.json", "application/json"),
            ("/client-core.wasm", "application/wasm"),
            ("/assets/app.js", "text/javascript; charset=utf-8"),
            ("/favicon.svg", "image/svg+xml"),
            ("/unknown.bin", "application/octet-stream"),
            ("/no-extension", "application/octet-stream"),
        ] {
            assert_eq!(content_type(path), expected, "{path}");
        }
        let response = http(b"GET /connection.json HTTP/1.1\r\nHost: terminal.example.com\r\n\r\n");
        let headers = String::from_utf8_lossy(&response);
        assert!(headers.contains("X-Content-Type-Options: nosniff"));
        assert!(headers.contains("Content-Type: application/json"));
    }
    #[test]
    fn public_service_never_enrolls_keys_or_accepts_mutations() {
        let response = http(
            b"POST /api/enroll HTTP/1.1\r\nHost: terminal.example.com\r\nContent-Length: 0\r\n\r\n",
        );
        assert!(response.starts_with(b"HTTP/1.1 405"));
        let response = http(
            b"GET /ssh?target=other.example:22 HTTP/1.1\r\nHost: terminal.example.com\r\n\r\n",
        );
        assert!(response.starts_with(b"HTTP/1.1 404"));
    }
    #[test]
    fn relay_is_explicitly_enabled_and_requires_pinned_https_identity() {
        let mut c = config();
        c.enabled = false;
        assert!(c.validate().is_err());
        c = config();
        c.origin = "http://terminal.example.com".into();
        assert!(c.validate().is_err());
        c = config();
        c.ssh_host_key = "".into();
        assert!(c.validate().is_err());
    }
    fn split(response: &[u8]) -> (String, serde_json::Value) {
        let at = response.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
        (
            String::from_utf8_lossy(&response[..at]).into_owned(),
            serde_json::from_slice(&response[at..]).unwrap_or_default(),
        )
    }
    #[test]
    fn error_reporting_settings_parse_from_configuration() {
        let base = serde_json::to_value(config()).unwrap();
        // Absent: the existing configuration format, reporting off.
        assert!(base.get("glitchtip_dsn").is_none());
        let parsed: Config = serde_json::from_value(base.clone()).unwrap();
        assert!(parsed.validate().is_ok());
        assert_eq!(parsed.error_reporting().unwrap(), None);
        // Present.
        let mut value = base.clone();
        value["glitchtip_dsn"] = "https://abc123@errors.example.com/7".into();
        value["error_environment"] = "preprod".into();
        let parsed: Config = serde_json::from_value(value).unwrap();
        assert!(parsed.validate().is_ok());
        let settings = parsed.error_reporting().unwrap().unwrap();
        assert_eq!(settings.dsn.origin(), "https://errors.example.com");
        assert_eq!(settings.environment.as_deref(), Some("preprod"));
        // Invalid DSNs and environments stop the relay at startup.
        for (dsn, environment) in [
            (Some("http://abc@errors.example.com/7"), None),
            (Some("https://abc:secret@errors.example.com/7"), None),
            (Some("not a dsn"), None),
            (Some("https://abc@errors.example.com/7"), Some("pre prod")),
            (None, Some("")),
        ] {
            let mut c = config();
            c.glitchtip_dsn = dsn.map(Into::into);
            c.error_environment = environment.map(Into::into);
            assert!(c.validate().is_err(), "{dsn:?} {environment:?}");
        }
        // The shipped example lists both fields, unset.
        let example: Config =
            serde_json::from_str(include_str!("../../examples/web-relay.json")).unwrap();
        assert_eq!(example.error_reporting().unwrap(), None);
        // Wrong types are configuration errors too.
        let mut value = base;
        value["glitchtip_dsn"] = 7.into();
        assert!(serde_json::from_value::<Config>(value).is_err());
    }
    #[test]
    fn browser_gets_the_dsn_and_csp_origin_only_when_configured() {
        let request = b"GET /connection.json HTTP/1.1\r\nHost: terminal.example.com\r\n\r\n";
        let (headers, json) = split(&http(request));
        assert!(json.get("errors").is_none(), "{json}");
        assert!(!headers.contains("errors.example.com"));
        assert!(headers.contains("; connect-src 'self'; worker-src"));
        let (headers, json) = split(&http_with(reporting_config(), request));
        assert_eq!(
            json["errors"],
            json!({"dsn": "https://abc123@errors.example.com/7", "environment": "preprod"})
        );
        assert_eq!(json["workspace"], "dev");
        assert!(headers.contains("; connect-src 'self' https://errors.example.com; worker-src"));
        // Every static response carries the same policy.
        let (headers, _) = split(&http_with(
            reporting_config(),
            b"GET /missing HTTP/1.1\r\nHost: terminal.example.com\r\n\r\n",
        ));
        assert!(headers.contains("connect-src 'self' https://errors.example.com;"));
        let mut no_environment = reporting_config();
        no_environment.error_environment = None;
        let (_, json) = split(&http_with(no_environment, request));
        assert_eq!(
            json["errors"],
            json!({"dsn": "https://abc123@errors.example.com/7"})
        );
    }
}
