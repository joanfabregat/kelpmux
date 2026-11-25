//! Kelp TUI and legacy tmux client, using the public terminal foundation.
pub use kelpserver::{Result, discovery, journal, now, process, session_id, shell_quote};

/// Keep tmux format expansion and control characters out of user-facing labels.
/// Copied from KelpServer's crate root (kelpserver afc5197, Apache-2.0, same
/// authors), which removed it in 0.3.0-beta.2.
pub fn label(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect::<String>()
        .replace('#', "##")
}
pub mod model {
    pub use kelpclient::model::*;

    // Status presentation. Copied from `kelpclient-protocol` 0.2.0
    // (`Status::text`, `Status::marker`, `WORKING_FRAMES`,
    // `StatusSource::label`; Apache-2.0, same authors), which removed it in
    // 0.3.0: clients own status labels and glyphs. Trait methods here,
    // because the types are the protocol crate's.

    pub const WORKING_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

    /// Labels and glyphs of an agent status.
    pub trait StatusLabel {
        fn text(self) -> &'static str;
        fn marker(self) -> &'static str;
    }
    impl StatusLabel for Status {
        fn text(self) -> &'static str {
            match self {
                Self::Working => "WORKING",
                Self::WaitingBackground => "WAITING IN BACKGROUND",
                Self::InputNeeded => "INPUT NEEDED",
                Self::TurnFinished => "TURN FINISHED",
                Self::Idle => "IDLE",
                Self::Failed => "FAILED",
                Self::Exited => "EXITED",
                Self::Unknown => "UNKNOWN",
            }
        }
        fn marker(self) -> &'static str {
            match self {
                Self::Working => WORKING_FRAMES[0],
                Self::WaitingBackground => "◌",
                Self::InputNeeded => "!",
                Self::TurnFinished => "✓",
                Self::Idle => "○",
                Self::Failed => "×",
                Self::Exited => "■",
                Self::Unknown => "?",
            }
        }
    }
    /// The label of the evidence behind a status.
    pub trait StatusSourceLabel {
        fn label(self) -> &'static str;
    }
    impl StatusSourceLabel for StatusSource {
        fn label(self) -> &'static str {
            match self {
                Self::Unknown => "evidence unknown",
                Self::Process => "process observation",
                Self::Hook => "lifecycle hook",
                Self::LiveRecord => "live record",
                Self::AppServer => "agent server",
                Self::Transcript => "transcript observation",
                Self::Terminal => "terminal program status",
            }
        }
    }
}
pub mod native {
    // KelpServer 0.3 modules KelpMux uses, all part of the generic session
    // server (ADR 0002): the stream and its session API, the same-user
    // connection, server lifecycle, recovery records, hooks, the diagnostics
    // report, the terminal emulator, the stream reference client and the SSH
    // bridge. KelpServer 0.3 has no composed UI any more; the TUI renders
    // locally with the modules below, administration (`ctl`, `stop`, the web
    // bridge) speaks the stream session API (`session`), `config` holds
    // KelpMux's part of `workspace.json`, and the web relay is KelpMux's own
    // (`web_relay`). Since KelpServer 0.3.0-beta.2 the startup template
    // format (`templates`), retained-client selection (`upgrade`), restore
    // choices (`history`) and the vitals presentation (`vitals`) are
    // KelpMux's too; those modules re-export the server's part.
    pub use kelpserver::native::{
        bridge, connection, diagnostics, engine, hooks, lifecycle, recovery, reference, stream,
        uploads,
    };
    pub mod cli;
    pub mod config;
    pub mod error_reporting;
    pub mod history;
    pub mod hover;
    pub mod input;
    pub mod projects;
    pub mod protocol;
    pub mod render;
    pub mod session;
    pub mod status;
    pub mod templates;
    pub mod tui;
    pub mod ui;
    pub mod upgrade;
    pub mod user_config;
    pub mod vitals;
    pub mod web_assets;
    pub mod web_bridge;
    pub mod web_git;
    pub mod web_relay;
    pub mod web_repos;
    pub mod workspace;
}
pub mod picker;
pub mod tmux;
pub mod workspace;
