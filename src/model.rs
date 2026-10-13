use serde::{Deserialize, Serialize};

pub const WORKING_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
    InputNeeded,
    TurnFinished,
    Idle,
    Failed,
    Exited,
    #[default]
    Unknown,
}

impl Status {
    pub fn text(self) -> &'static str {
        match self {
            Self::Working => "WORKING",
            Self::InputNeeded => "INPUT NEEDED",
            Self::TurnFinished => "TURN FINISHED",
            Self::Idle => "IDLE",
            Self::Failed => "FAILED",
            Self::Exited => "EXITED",
            Self::Unknown => "UNKNOWN",
        }
    }
    pub fn marker(self) -> &'static str {
        match self {
            Self::Working => WORKING_FRAMES[0],
            Self::InputNeeded => "!",
            Self::TurnFinished => "✓",
            Self::Idle => "○",
            Self::Failed => "×",
            Self::Exited => "■",
            Self::Unknown => "?",
        }
    }
    pub fn from_live(value: &str) -> Self {
        match value {
            "working" | "running" | "busy" | "active" => Self::Working,
            "waiting" | "waiting_for_input" | "needs_input" | "input_needed" | "permission" => {
                Self::InputNeeded
            }
            "idle" => Self::Idle,
            "completed" | "turn_finished" => Self::TurnFinished,
            "error" | "failed" | "systemError" => Self::Failed,
            _ => Self::Unknown,
        }
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
            || self.ended_at != other.ended_at
    }
}
