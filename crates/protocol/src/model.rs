use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Codex,
    Claude,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Working,
    WaitingBackground,
    InputNeeded,
    TurnFinished,
    Idle,
    Failed,
    Exited,
    #[default]
    Unknown,
}

impl Status {
    pub fn from_live(value: &str) -> Self {
        match value {
            "working" | "running" | "busy" | "active" => Self::Working,
            "waiting" | "waiting_for_input" | "needs_input" | "input_needed" | "permission" => {
                Self::InputNeeded
            }
            "idle" => Self::Idle,
            "waiting_background" => Self::WaitingBackground,
            "completed" | "turn_finished" => Self::TurnFinished,
            "error" | "failed" | "systemError" => Self::Failed,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatusSource {
    #[default]
    Unknown,
    Process,
    Hook,
    LiveRecord,
    AppServer,
    Transcript,
    Terminal,
}
/// A fresh check of historical evidence does not invent a fresh lifecycle event.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Observation {
    pub source: StatusSource,
    pub checked_at: u64,
    pub event_at: Option<u64>,
}
impl Observation {
    pub fn checked(source: StatusSource, checked_at: u64) -> Self {
        Self {
            source,
            checked_at,
            event_at: None,
        }
    }
    pub fn stale(&self, now: u64) -> bool {
        self.checked_at == 0 || now.saturating_sub(self.checked_at) > 10
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Agent {
    pub pane: String,
    pub window: String,
    pub pid: u32,
    pub process_start: u64,
    pub kind: Kind,
    pub session_id: Option<String>,
    pub name: String,
    pub cwd: String,
    pub status: Status,
    #[serde(default)]
    pub observation: Observation,
    /// Evidence used for identity, never inferred from an arbitrary newest transcript.
    pub source: String,
    pub first_seen: u64,
    pub last_seen: u64,
    pub ended_at: Option<u64>,
}

impl Agent {
    pub fn key(&self) -> String {
        format!("{}:{}", self.pid, self.process_start)
    }
    pub fn changed(&self, other: &Self) -> bool {
        self.pane != other.pane
            || self.window != other.window
            || self.session_id != other.session_id
            || self.name != other.name
            || self.cwd != other.cwd
            || self.status != other.status
            || self.observation.source != other.observation.source
            || self.observation.event_at != other.observation.event_at
            || self.ended_at != other.ended_at
    }
}
