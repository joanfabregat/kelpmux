//! One-shot client of KelpServer's stream session API, for same-user
//! administration (`kelpmux ctl`, `list`, `stop`) and the web bridge's
//! request-correlated operations. Each use opens its own stream connection,
//! sends `hello`, sends one or a few requests and waits for their correlated
//! replies; it never subscribes to screens and never keeps a lease after it
//! disconnects.
//!
//! KelpServer 0.3 has a similar `connection::Session` for its own `start`,
//! `stop` and `status`. KelpMux keeps this one because it needs more than
//! that type offers: opening on an already connected socket (the web bridge
//! checks the peer process on it before greeting), the 10 s bound that
//! `closed_sessions` needs (KelpServer's waits 6 s), `require` with a
//! user-facing message, the session-data opt-in (`sessions`), `claim` and
//! `release` for `ctl reclaim`, `wait_closed` after `shutdown`, request
//! identities unique per process, and an epoch check on updates.
use super::{
    protocol::*,
    stream::{self, Event, Request, Update},
};
use crate::Result;
use std::{
    os::unix::net::UnixStream,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

/// How long one exchange waits for its reply. `closed_sessions` waits for
/// the server's background collection, so it is not the 3 s handshake bound.
pub const TIMEOUT: Duration = Duration::from_secs(10);

pub struct Session {
    socket: UnixStream,
    /// This connection's Welcome `client` ID.
    pub client: u64,
    pub epoch: String,
    pub capabilities: Vec<String>,
    pub server_version: Option<String>,
}

impl Session {
    /// Greets the server on `socket` and reads its Welcome.
    pub fn open(mut socket: UnixStream) -> Result<Self> {
        socket.set_read_timeout(Some(TIMEOUT))?;
        socket.set_write_timeout(Some(TIMEOUT))?;
        write_message(
            &mut socket,
            &ClientMessage::Stream(Request::Hello {
                version: stream::VERSION,
            }),
        )?;
        let mut session = Self {
            socket,
            client: 0,
            epoch: String::new(),
            capabilities: Vec::new(),
            server_version: None,
        };
        match session.event()? {
            Event::Welcome {
                version,
                client,
                epoch,
                capabilities,
                server_version,
                ..
            } if version == stream::VERSION && client != 0 && !epoch.is_empty() => {
                session.client = client;
                session.epoch = epoch;
                session.capabilities = capabilities;
                session.server_version = server_version;
                Ok(session)
            }
            _ => Err("the terminal server did not welcome the stream".into()),
        }
    }
    pub fn has(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }
    /// Fails unless the server advertises `capability`: an older server
    /// disconnects a connection that sends an unknown request type.
    pub fn require(&self, capability: &str) -> Result<()> {
        if self.has(capability) {
            Ok(())
        } else {
            Err(format!(
                "this terminal server does not offer `{capability}`; restart the workspace with a current server"
            )
            .into())
        }
    }
    pub fn send(&mut self, request: Request) -> Result<()> {
        write_message(&mut self.socket, &ClientMessage::Stream(request))
    }
    /// The next stream event. A stream error without a correlation fails
    /// the exchange: this client sends one request at a time.
    fn event(&mut self) -> Result<Event> {
        match read_message::<ServerMessage>(&mut self.socket)? {
            Some(ServerMessage::Stream(Event::Error {
                message,
                request: None,
            }))
            | Some(ServerMessage::Error(message)) => Err(message.into()),
            Some(ServerMessage::Stream(event)) => Ok(event),
            Some(_) => Err("unexpected reply from the terminal server".into()),
            None => Err("the terminal server disconnected".into()),
        }
    }
    /// Sends a correlated request and returns its reply, skipping updates
    /// and other events that arrive first.
    pub fn call(&mut self, request: Request) -> Result<Event> {
        let id = request
            .correlation()
            .ok_or("request has no correlation")?
            .to_owned();
        if !stream::valid_request(&id) {
            return Err("request identity must be 1–128 letters, digits, - or _".into());
        }
        self.send(request)?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if Instant::now() > deadline {
                return Err("the terminal server did not answer in time".into());
            }
            let event = self.event()?;
            let echoed = match &event {
                Event::Error {
                    message,
                    request: Some(request),
                } if *request == id => return Err(message.clone().into()),
                Event::Spawned { request, .. }
                | Event::Done { request }
                | Event::ClosedSessions { request, .. }
                | Event::Restored { request, .. }
                | Event::History { request, .. }
                | Event::Search { request, .. } => request,
                _ => continue,
            };
            if *echoed == id {
                return Ok(event);
            }
        }
    }
    /// The next metadata update (pane list, leases, notifications).
    pub fn update(&mut self) -> Result<Update> {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() <= deadline {
            if let Event::Update(update) = self.event()? {
                if update.epoch != self.epoch {
                    return Err("terminal server epoch changed".into());
                }
                return Ok(update);
            }
        }
        Err("the terminal server sent no update".into())
    }
    /// The pane list with each pane's session facts: opts in to session
    /// events and waits for an update built after the opt-in.
    pub fn sessions(&mut self) -> Result<Update> {
        self.require("pane-lifecycle")?;
        self.send(Request::SessionEvents { enabled: true })?;
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() <= deadline {
            let update = self.update()?;
            // An update built before the opt-in has no session data.
            if update.panes.is_empty() || !update.sessions.is_empty() {
                return Ok(update);
            }
        }
        Err("the terminal server sent no session data".into())
    }
    /// Takes the pane's lease (input and size) from whichever client holds
    /// it. The lease ends with this connection.
    pub fn claim(&mut self, pane: PaneId) -> Result<()> {
        self.send(Request::Claim {
            pane,
            takeover: true,
        })?;
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() <= deadline {
            if let Event::Lease {
                pane: leased,
                owner,
            } = self.event()?
                && leased == pane
                && owner == Some(self.client)
            {
                return Ok(());
            }
        }
        Err("the terminal server did not grant the pane".into())
    }
    /// Gives a held lease back: the pane returns to the base geometry.
    pub fn release(&mut self, pane: PaneId) -> Result<()> {
        self.send(Request::Release { pane })?;
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() <= deadline {
            if let Event::Lease {
                pane: released,
                owner: None,
            } = self.event()?
                && released == pane
            {
                return Ok(());
            }
        }
        Err("the terminal server did not release the pane".into())
    }
    /// Waits until the server closes this connection, as it does when it
    /// stops; a timeout is not an error.
    pub fn wait_closed(&mut self, limit: Duration) {
        let deadline = Instant::now() + limit;
        let _ = self.socket.set_read_timeout(Some(limit));
        while Instant::now() < deadline {
            match read_message::<ServerMessage>(&mut self.socket) {
                Ok(Some(_)) => {}
                _ => return,
            }
        }
    }
}

/// A request identity unique within this process, for operations the user
/// does not retry (`close`, `history`, `shutdown`, …).
pub fn request_id(prefix: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
