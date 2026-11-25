//! UI-independent pane streams, the only protocol of the local socket. SSH
//! transports these framed messages unchanged.
//!
//! Send `Hello { version: 1 }` first, then `Subscribe` (replacement set, at most
//! `MAX_SUBSCRIPTIONS` unique current panes, advertised in Welcome `limits`). Welcome's random server epoch scopes all pane IDs. A new
//! epoch invalidates every saved baseline/lease, even if numerical IDs repeat.
//! Every Update includes current bounded metadata, notifications and structured vitals.
//! A snapshot starts revision 1; patches advance exactly one revision and require
//! the exact delivered baseline. Geometry changes, explicit Resync and backpressure
//! produce replacement snapshots. Clients must never apply a patch to another epoch.
//!
//! Claim/Resize/Input/Paste/Mouse address one pane. Claim is exclusive, explicit
//! takeover may replace an owner, and release/disconnect restores the server's
//! base pane size (`--size`). Leases have connection lifetime (no time-based expiry).
//!
//! At most 100,000 cells are subscribed per connection. A later resize exceeding
//! that aggregate budget disconnects that subscriber rather than accumulating state.
//! Server retains one in-flight packet and one replaceable aggregate update; excess
//! reliable responses disconnect a slow peer. File replies (`files`) are the
//! exception: they wait in a per-connection queue, bounded by the
//! connection's operation slots and at most 256 immediate refusals, and enter
//! the output queue as it drains. Notifications are the latest 128
//! retained records, not an exactly-once event log; acknowledgement is explicit.
//!
//! The session API (capabilities `pane-lifecycle`, `closed-sessions`,
//! `pane-history`, `pane-scroll`, `notification-ack-all`, `host-status`,
//! `server-lifecycle` and `diagnostics`) is additive. Its events are sent only
//! to a connection that asked for them, either by opting in (`session_events`,
//! `host_status`) or by sending a correlated request, so v1 clients that
//! reject unknown events never receive one. Clients must check the Welcome
//! capabilities before sending a new request: an older server disconnects on
//! an unknown request type. Step 2 of ADR 0002 added `startup-policy`,
//! `recovery-signal`, `spawn-shell`, `unused-shell`, `stream-limits`,
//! `notification-run` and `spawned-by` the same way, and decision 151a added
//! `uploads` (see `super::uploads`), and ADR 0003 section 3 added `files`
//! (see `super::files`), with `file-downloads` (see `super::downloads`).
//! Device/MCP capabilities remain deferred.
//!
//! `binary-screens` (ADR 0003 section 2): after `binary_screens {enabled:
//! true}`, each Update to that connection has `binary_screens: true`, empty
//! `snapshots`/`patches`, and is immediately followed by exactly one binary
//! record (`kelpclient_protocol::screens`) carrying its screens with the same
//! revision rules. Both records form one replaceable packet, so the
//! backpressure replacement above never separates them. Other connections
//! never receive a binary record.
use super::protocol::{MouseEvent, Notification, PaneId, PaneInfo, PaneSearchMatch, Screen};
use crate::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const VERSION: u16 = 1;
/// Subscribed panes per connection (advertised in Welcome `limits`). Every
/// pane of a grid shown on one terminal fits: 64 leaves headroom over 24- to
/// 36-pane grids. The per-pane cost is one retained screen baseline, which
/// `MAX_SUBSCRIBED_CELLS` bounds in aggregate.
pub const MAX_SUBSCRIPTIONS: usize = 64;
/// Pane geometry bounds for stream leases and the server base size (`--size`).
pub fn validate_size(cols: u16, rows: u16) -> Result<()> {
    if !(2..=400).contains(&cols) || !(1..=160).contains(&rows) {
        return Err("pane size must be 2–400 columns and 1–160 rows".into());
    }
    Ok(())
}
/// Bound both retained screen baselines and aggregate update serialization
/// (advertised in Welcome `limits`).
///
/// A backpressured update carries every subscribed screen as a snapshot in
/// one record, and records are capped at 16 MiB. A cell serializes to about
/// 100 JSON bytes with default colours and up to about 160 with RGB colours
/// and an underline colour, so 100,000 cells (at most about 15 MiB) is the
/// largest budget that always fits one record. A retained baseline cell costs
/// about 100 bytes of memory, so a connection keeps at most about 10 MB. The
/// budget does not limit grids: the panes displayed on one terminal together
/// cover at most that terminal, and the largest native pane is 400 × 160 =
/// 64,000 cells. Hidden panes are unsubscribed (see "Parking").
pub const MAX_SUBSCRIBED_CELLS: usize = 100_000;
/// Advertised stream bounds, so clients need not hardcode them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// Most panes in one `subscribe`.
    pub subscriptions: usize,
    /// Most aggregate screen cells (columns × rows) of the subscribed panes.
    pub subscribed_cells: usize,
}
pub const LIMITS: Limits = Limits {
    subscriptions: MAX_SUBSCRIPTIONS,
    subscribed_cells: MAX_SUBSCRIBED_CELLS,
};
/// Which panes the server creates when it starts (Welcome `startup_panes`).
/// Since 0.3 a server always starts without panes and sends `None`; the
/// client that starts it creates its layout with `spawn`. `--startup-panes
/// none` stays accepted. The other variants exist only so that a 0.3 client
/// can decode the Welcome of an older or newer server: they are never sent by
/// a 0.3 server and `--startup-panes` rejects them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupPanes {
    /// No pane: a stream client creates its own layout with `spawn`.
    #[default]
    None,
    /// 0.2 servers only: the server created the composed UI's saved layout.
    Layout,
    /// Any other value, from a server newer than this client.
    #[serde(other)]
    Unknown,
}
impl std::str::FromStr for StartupPanes {
    type Err = Box<dyn std::error::Error + Send + Sync>;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "none" => Ok(Self::None),
            "layout" => Err(
                "--startup-panes layout was removed in KelpServer 0.3: the server \
                 starts without panes and clients create them with spawn"
                    .into(),
            ),
            _ => Err("--startup-panes must be none".into()),
        }
    }
}
impl StartupPanes {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Layout => "layout",
            Self::Unknown => "unknown",
        }
    }
}
pub const CAPABILITIES: &[&str] = &[
    "pane-snapshots",
    "pane-patches",
    "pane-leases",
    "metadata",
    "notifications",
    "vitals",
    "hyperlinks",
    "program-status",
    "controller-theme",
    "clipboard-write",
    "pane-lifecycle",
    "closed-sessions",
    "pane-history",
    "pane-scroll",
    "notification-ack-all",
    "host-status",
    "server-lifecycle",
    "startup-policy",
    "recovery-signal",
    "spawn-shell",
    "unused-shell",
    "stream-limits",
    "notification-run",
    "spawned-by",
    "diagnostics",
    "uploads",
    "binary-screens",
    "files",
    "bell-merge",
    "file-downloads",
];
/// Aggregate context budget of one multi-pane `search` reply.
pub const MAX_SEARCH_CONTEXT: usize = 4 * 1024 * 1024;
/// Longest `history` page.
pub const MAX_HISTORY_LINES: usize = 200;

/// Client-chosen correlation identifiers: 1–128 ASCII letters, digits, `-` or `_`.
pub fn valid_request(request: &str) -> bool {
    !request.is_empty()
        && request.len() <= 128
        && request
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Hello {
        version: u16,
    },
    Subscribe {
        panes: Vec<PaneId>,
    },
    /// Opt in to application clipboard writes for panes owned by this connection.
    Clipboard {
        enabled: bool,
    },
    Claim {
        pane: PaneId,
        takeover: bool,
    },
    Release {
        pane: PaneId,
    },
    Resize {
        pane: PaneId,
        cols: u16,
        rows: u16,
    },
    Theme {
        pane: PaneId,
        foreground: [u8; 3],
        background: [u8; 3],
    },
    Input {
        pane: PaneId,
        bytes: Vec<u8>,
    },
    Paste {
        pane: PaneId,
        bytes: Vec<u8>,
    },
    Mouse {
        pane: PaneId,
        event: MouseEvent,
    },
    Resync {
        pane: PaneId,
    },
    Acknowledge {
        notification: u64,
    },
    /// `notification-ack-all`: mark every retained notification read.
    AcknowledgeAll,
    /// `pane-lifecycle`: receive lifecycle events and per-pane session data.
    SessionEvents {
        enabled: bool,
    },
    /// `pane-lifecycle`: start a pane. Without `cwd`, the server's launch
    /// directory is used; an empty `argv` opens an interactive shell. A retry
    /// with the same `request`, directory and arguments returns the same pane.
    Spawn {
        request: String,
        #[serde(default)]
        cwd: Option<PathBuf>,
        #[serde(default)]
        argv: Vec<String>,
        /// `spawn-shell`: an absolute path to an executable file to run
        /// instead of the server's shell. Omitted when absent, so a request
        /// without it stays valid for servers without `spawn-shell`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shell: Option<PathBuf>,
    },
    /// `recovery-signal`: clear the unclean-shutdown signal for every client.
    DismissRecovery {
        request: String,
    },
    /// `pane-lifecycle`: terminate a pane's process group and remove the pane.
    /// Refused while another connection holds the pane's lease.
    Close {
        request: String,
        pane: PaneId,
    },
    /// `closed-sessions`: list restorable records of closed panes.
    ClosedSessions {
        request: String,
    },
    /// `closed-sessions`: reopen a listed record. `rerun` repeats a plain
    /// program; agent records always resume their conversation if known.
    Restore {
        request: String,
        record: String,
        #[serde(default)]
        rerun: bool,
    },
    /// `pane-history`: plain-text rows; negative lines are scrollback.
    History {
        request: String,
        pane: PaneId,
        start: i32,
        limit: usize,
    },
    /// `pane-history`: literal case-insensitive search. An empty pane list
    /// searches every pane in creation order.
    Search {
        request: String,
        #[serde(default)]
        panes: Vec<PaneId>,
        query: String,
        limit: usize,
    },
    /// `pane-scroll`: move the leased pane's viewport through its scrollback.
    Scroll {
        pane: PaneId,
        lines: i32,
    },
    /// `pane-scroll`: show a primary-history line (as returned by `history`
    /// or a search match's `first_line + start.y`) in the leased pane.
    Reveal {
        pane: PaneId,
        line: i32,
    },
    /// `binary-screens`: receive this connection's pane screens as binary
    /// records (`kelpclient_protocol::screens`) after each `update` instead
    /// of `Update.snapshots` and `Update.patches`.
    BinaryScreens {
        enabled: bool,
    },
    /// `host-status`: include the host process/disk snapshot in updates.
    HostStatus {
        enabled: bool,
    },
    /// `server-lifecycle`: stop the server and every pane process.
    Shutdown {
        request: String,
    },
    /// `diagnostics`: bounded server metrics, without terminal contents.
    Diagnostics {
        request: String,
    },
    /// `uploads`: declare a batch of files. `dest` is an existing absolute
    /// directory owned by the server's user; the batch gets its own private
    /// directory inside it (or inside the server's upload directory).
    UploadBegin {
        request: String,
        files: Vec<super::uploads::FileSpec>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dest: Option<PathBuf>,
    },
    /// `uploads`: the next base64 bytes of a batch. Files are sent in order
    /// and each file's chunks at strictly sequential offsets.
    UploadChunk {
        batch: u64,
        file_index: usize,
        offset: u64,
        data: String,
    },
    /// `uploads`: publish a complete batch.
    UploadFinish {
        request: String,
        batch: u64,
    },
    /// `uploads`: remove a batch and its files.
    UploadCancel {
        request: String,
        batch: u64,
    },
    /// `files`: a directory's entries (`file_list` reply). Paths are
    /// absolute or start with `~/` (see `super::files::normalize`).
    FileList {
        request: String,
        path: String,
    },
    /// `files`: one entry, not following a final symlink (`file_stat` reply).
    FileStat {
        request: String,
        path: String,
    },
    /// `files`: a regular file's content (`file_content` reply), at most
    /// `max_bytes` (default and cap: `file_limits.max_read_bytes`).
    FileRead {
        request: String,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<u64>,
    },
    /// `files`: replace a file atomically, or create it with `create`
    /// (`file_written` reply). With `base_hash`, a file whose current
    /// SHA-256 differs is not written (`file_error` `conflict`).
    FileWrite {
        request: String,
        path: String,
        content: String,
        #[serde(default)]
        encoding: super::files::Encoding,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_hash: Option<String>,
        #[serde(default)]
        create: bool,
    },
    /// `files`: create one directory (`done` reply).
    FileMkdir {
        request: String,
        path: String,
    },
    /// `files`: rename or move within one file system (`done` reply). An
    /// existing `to` is replaced only with `overwrite`.
    FileRename {
        request: String,
        from: String,
        to: String,
        #[serde(default)]
        overwrite: bool,
    },
    /// `files`: move entries to the freedesktop home trash (`file_removed`).
    FileTrash {
        request: String,
        paths: Vec<String>,
    },
    /// `files`: delete entries permanently, directories recursively, never
    /// following a symlink (`file_removed`).
    FileDelete {
        request: String,
        paths: Vec<String>,
    },
    /// `file-downloads`: send one regular file's bytes, or files and folders
    /// as one archive (`file_download_started`, then chunks).
    FileDownloadBegin {
        request: String,
        paths: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        archive: Option<super::downloads::Archive>,
    },
    /// `file-downloads`: the client received the download's bytes up to
    /// `offset` (the end of the oldest unacknowledged chunk).
    FileDownloadAck {
        download: u64,
        offset: u64,
    },
    /// `file-downloads`: stop a download (`done` reply).
    FileDownloadCancel {
        request: String,
        download: u64,
    },
}
impl Request {
    /// The correlation identifier echoed by this request's reply and errors.
    pub fn correlation(&self) -> Option<&str> {
        match self {
            Self::Spawn { request, .. }
            | Self::Close { request, .. }
            | Self::ClosedSessions { request }
            | Self::Restore { request, .. }
            | Self::History { request, .. }
            | Self::Search { request, .. }
            | Self::DismissRecovery { request }
            | Self::Shutdown { request }
            | Self::Diagnostics { request }
            | Self::UploadBegin { request, .. }
            | Self::UploadFinish { request, .. }
            | Self::UploadCancel { request, .. }
            | Self::FileList { request, .. }
            | Self::FileStat { request, .. }
            | Self::FileRead { request, .. }
            | Self::FileWrite { request, .. }
            | Self::FileMkdir { request, .. }
            | Self::FileRename { request, .. }
            | Self::FileTrash { request, .. }
            | Self::FileDelete { request, .. }
            | Self::FileDownloadBegin { request, .. }
            | Self::FileDownloadCancel { request, .. } => Some(request),
            _ => None,
        }
    }
}

/// Session facts about a live pane, independent of any client's presentation.
/// Pane IDs increase monotonically and are never reused within one epoch, so
/// they also give creation order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSession {
    pub pane: PaneId,
    /// Unix seconds.
    pub created_at: u64,
    /// The directory the shell was started in; `PaneInfo::cwd` is the current one.
    pub launch_cwd: String,
    pub shell: String,
    /// Arguments given to `spawn`; empty for an interactive shell or another origin.
    pub argv: Vec<String>,
    /// The command line typed into the shell at startup, if any.
    pub startup: Option<String>,
    /// The latest OSC title set by the application, unmodified by naming rules.
    pub terminal_title: Option<String>,
    /// `unused-shell`: the pane's shell has no child process, no pending or
    /// recent input (two idle observations and a 200 ms cooldown), and no
    /// other process on its terminal. Sampled every 500 ms while a
    /// connection has `session_events` enabled; leases do not affect it.
    #[serde(default)]
    pub unused_shell: bool,
    /// `spawned-by`: the Welcome `client` ID of the stream connection whose
    /// `spawn` or `restore` created the pane, within this epoch. Absent for
    /// replacement shells the server creates itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    /// The pane's process exited.
    Exited,
    /// A client closed the pane.
    Closed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// An explicit shutdown request.
    Shutdown,
    /// The last pane exited and no other pane keeps the server alive.
    LastPaneExited,
}
/// A restorable record of a closed pane. `key` identifies it for `restore`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedSession {
    #[serde(flatten)]
    pub record: super::history::Entry,
    /// The agent resume command `restore` runs, if the record is an agent.
    pub resume: Option<String>,
    /// The program `restore` with `rerun` runs, if the record is a program.
    pub rerun: Option<String>,
}
impl From<super::history::Entry> for ClosedSession {
    fn from(record: super::history::Entry) -> Self {
        Self {
            resume: record.command(),
            rerun: record.previous_command(),
            record,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneSearchResult {
    pub pane: PaneId,
    pub hits: Vec<PaneSearchMatch>,
    pub truncated: bool,
}

/// Pane screens and patches are protocol types, re-exported here.
pub use super::protocol::{PanePatch, PaneSnapshot};

/// Check a full pane screen before allocating a renderer's backing surface.
pub fn validate(snapshot: &PaneSnapshot) -> Result<()> {
    let screen = &snapshot.screen;
    if snapshot.pane == 0
        || snapshot.revision == 0
        || !(1..=400).contains(&screen.cols)
        || !(1..=160).contains(&screen.rows)
        || screen.cells.len() != usize::from(screen.cols) * usize::from(screen.rows)
        || screen.cursor.is_some_and(|cursor| {
            cursor.position.x >= screen.cols || cursor.position.y >= screen.rows
        })
        || screen.cells.iter().enumerate().any(|(index, cell)| {
            !kelpclient_protocol::cell_valid(cell)
                || cell.width == 2
                    && index % usize::from(screen.cols) + 1 == usize::from(screen.cols)
        })
    {
        return Err("invalid pane snapshot".into());
    }
    Ok(())
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub pane: PaneId,
    pub owner: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Update {
    pub epoch: String,
    pub subscribed: Vec<PaneId>,
    pub snapshots: Vec<PaneSnapshot>,
    pub patches: Vec<PanePatch>,
    pub panes: Vec<PaneInfo>,
    pub leases: Vec<Lease>,
    pub notifications: Vec<Notification>,
    pub vitals: super::vitals::Vitals,
    /// Only for connections that enabled `session_events`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<PaneSession>,
    /// Only for connections that enabled `host_status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<Box<super::status::Snapshot>>,
    /// `binary-screens`: this update's screens follow it as exactly one binary
    /// record, and `snapshots` and `patches` are empty. Omitted when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub binary_screens: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Welcome {
        version: u16,
        client: u64,
        epoch: String,
        capabilities: Vec<String>,
        /// The server's package version (`server-lifecycle`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server_version: Option<String>,
        /// The server's startup policy (`startup-policy`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        startup_panes: Option<StartupPanes>,
        /// The previous run ended uncleanly and no client has shown or
        /// dismissed recovery yet (`recovery-signal`). Omitted when false.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery_pending: bool,
        /// Subscription bounds (`stream-limits`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limits: Option<Limits>,
        /// The current run in notification origins (`notification-run`): a
        /// notification belongs to a live pane when `origin.run` equals it,
        /// `origin.pid` equals the pane's `pid` and its `pane` the pane ID.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notification_run: Option<String>,
        /// Upload bounds and the flow-control window (`uploads`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        upload_limits: Option<super::uploads::Limits>,
        /// File operation bounds (`files`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_limits: Option<super::files::Limits>,
    },
    Update(Update),
    Lease {
        pane: PaneId,
        owner: Option<u64>,
    },
    Clipboard {
        pane: PaneId,
        text: String,
    },
    Error {
        message: String,
        /// The failed request's correlation identifier, when it had one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<String>,
    },
    /// Opt-in lifecycle events. A pending update built before the event is
    /// discarded, so the next update for that connection follows the event.
    PaneCreated {
        session: PaneSession,
    },
    PaneExited {
        pane: PaneId,
        at: u64,
        code: Option<i32>,
        signal: Option<i32>,
    },
    PaneClosed {
        pane: PaneId,
        at: u64,
        reason: CloseReason,
        /// The closed-session record created for this pane, if any.
        record: Option<String>,
    },
    ServerStopping {
        reason: StopReason,
    },
    /// Replies to correlated requests; sent only to the requesting connection.
    Spawned {
        request: String,
        pane: PaneId,
    },
    Done {
        request: String,
    },
    ClosedSessions {
        request: String,
        records: Vec<ClosedSession>,
    },
    Restored {
        request: String,
        record: String,
        pane: PaneId,
        /// The conversation was already open in this pane; nothing was started.
        existing: bool,
    },
    History {
        request: String,
        pane: PaneId,
        lines: Vec<(i32, String)>,
    },
    Search {
        request: String,
        results: Vec<PaneSearchResult>,
        truncated: bool,
    },
    Diagnostics {
        request: String,
        report: super::diagnostics::Report,
    },
    /// `uploads`: the batch was created in `dir`.
    UploadStarted {
        request: String,
        batch: u64,
        dir: PathBuf,
    },
    /// `uploads`: one chunk was written; `received` bytes of the file so far.
    /// Every accepted chunk gets exactly one, in order.
    UploadProgress {
        batch: u64,
        file_index: usize,
        received: u64,
    },
    /// `uploads`: the batch's files, in request order, as absolute paths.
    UploadDone {
        request: String,
        batch: u64,
        paths: Vec<PathBuf>,
    },
    /// `uploads`: a chunk failed; the batch and its files were removed.
    UploadFailed {
        batch: u64,
        message: String,
    },
    /// `files`: the entries of `path` (normalised), directories first, then
    /// by name. `truncated` when entries were left out by the bounds;
    /// `skipped` counts names that are not UTF-8.
    FileList {
        request: String,
        path: String,
        entries: Vec<super::files::Entry>,
        truncated: bool,
        skipped: usize,
    },
    /// `files`: one entry (`lstat`).
    FileStat {
        request: String,
        path: String,
        entry: super::files::Entry,
    },
    /// `files`: a regular file's whole content and its SHA-256 (`hash`,
    /// lowercase hex), the `base_hash` of a later `file_write`.
    FileContent {
        request: String,
        path: String,
        size: u64,
        modified: i64,
        mode: u32,
        hash: String,
        /// The bytes hold NUL or are not UTF-8.
        binary: bool,
        encoding: super::files::Encoding,
        content: String,
    },
    /// `files`: the file now holds the written content.
    FileWritten {
        request: String,
        path: String,
        hash: String,
        size: u64,
        modified: i64,
        created: bool,
    },
    /// `files`: the outcome of `file_trash` or `file_delete`, per path.
    FileRemoved {
        request: String,
        removed: Vec<super::files::Removed>,
        failed: Vec<super::files::Failure>,
    },
    /// `files`: a file request failed; nothing was changed.
    FileError {
        request: String,
        error: super::files::Error,
        /// `file-downloads`: the download that failed after it started.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        download: Option<u64>,
    },
    /// `file-downloads`: the download began. `size` is a single file's
    /// size when it was opened; an archive has `archive`, and the entries
    /// and input bytes the planning walk found.
    FileDownloadStarted {
        request: String,
        download: u64,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        archive: Option<super::downloads::Archive>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entries: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_bytes: Option<u64>,
    },
    /// `file-downloads`: the next base64 bytes, at `offset`. Archives carry
    /// the input bytes read so far (`input`), for progress.
    FileDownloadChunk {
        download: u64,
        offset: u64,
        data: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<u64>,
    },
    /// `file-downloads`: every byte was sent: `size` bytes whose SHA-256 is
    /// `hash`. `skipped` entries were left out of an archive (special
    /// files, unreadable entries, mount points' contents); `changed` files
    /// changed while they were read.
    FileDownloadDone {
        request: String,
        download: u64,
        size: u64,
        hash: String,
        skipped: u64,
        changed: u64,
    },
}

impl Event {
    /// The correlation identifier of a reply, or of an error of a correlated
    /// request. Unsolicited events have none.
    pub fn correlation(&self) -> Option<&str> {
        match self {
            Self::Spawned { request, .. }
            | Self::Done { request }
            | Self::ClosedSessions { request, .. }
            | Self::Restored { request, .. }
            | Self::History { request, .. }
            | Self::Search { request, .. }
            | Self::Diagnostics { request, .. }
            | Self::UploadStarted { request, .. }
            | Self::UploadDone { request, .. }
            | Self::FileList { request, .. }
            | Self::FileStat { request, .. }
            | Self::FileContent { request, .. }
            | Self::FileWritten { request, .. }
            | Self::FileRemoved { request, .. }
            | Self::FileError { request, .. }
            | Self::FileDownloadStarted { request, .. }
            | Self::FileDownloadDone { request, .. } => Some(request),
            Self::Error { request, .. } => request.as_deref(),
            _ => None,
        }
    }
}

/// Apply only to the exact baseline. A full snapshot starts a fresh revision chain.
pub fn apply(snapshot: &PaneSnapshot, patch: &PanePatch) -> Result<PaneSnapshot> {
    validate(snapshot)?;
    if patch.pane != snapshot.pane
        || patch.base != snapshot.revision
        || patch.base.checked_add(1) != Some(patch.revision)
        || patch.changes.len() > snapshot.screen.cells.len()
    {
        return Err("invalid pane patch baseline".into());
    }
    if patch.cursor.is_some_and(|cursor| {
        cursor.position.x >= snapshot.screen.cols || cursor.position.y >= snapshot.screen.rows
    }) {
        return Err("invalid pane patch cursor".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for (index, cell) in &patch.changes {
        if *index >= snapshot.screen.cells.len()
            || !seen.insert(*index)
            || !kelpclient_protocol::cell_valid(cell)
        {
            return Err("invalid pane patch cell".into());
        }
    }
    let mut next = snapshot.clone();
    for (index, cell) in &patch.changes {
        next.screen.cells[*index] = cell.clone();
    }
    next.revision = patch.revision;
    next.screen.cursor = patch.cursor;
    next.screen.modes = patch.modes;
    validate(&next)?;
    Ok(next)
}

pub fn diff(old: &PaneSnapshot, screen: &Screen) -> Option<PanePatch> {
    if old.screen.cols != screen.cols
        || old.screen.rows != screen.rows
        || old.screen.cells.len() != screen.cells.len()
    {
        return None;
    }
    let changes: Vec<_> = old
        .screen
        .cells
        .iter()
        .zip(&screen.cells)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(index, (_, cell))| (index, cell.clone()))
        .collect();
    // A sparse patch is useful; dense changes are cheaper and simpler as snapshots.
    if changes.len() > screen.cells.len() / 4 {
        return None;
    }
    Some(PanePatch {
        pane: old.pane,
        base: old.revision,
        revision: old.revision.checked_add(1)?,
        cursor: screen.cursor,
        modes: screen.modes,
        changes,
    })
}
