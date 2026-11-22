//! Kelp TUI and legacy tmux client, using the public terminal foundation.
pub use kelpserver::{Result, discovery, journal, label, now, process, session_id, shell_quote};
pub mod model {
    pub use kelpclient::model::*;
}
pub mod native {
    // KelpServer modules KelpMux uses, all part of the generic session
    // server (ADR 0002): the stream and its session API, the same-user
    // connection, server lifecycle and upgrade checks, closed-session
    // history, recovery records, hooks, templates, vitals, diagnostics, the
    // terminal emulator, the stream reference client and the SSH bridge.
    // Nothing from KelpServer's composed UI (protocol 58 frames, `Action`,
    // `ui`, `render`, `workspace`, its controller) is used: the TUI renders
    // locally with the modules below, and administration (`ctl`, `stop`,
    // the web bridge) speaks the stream session API (`session`).
    pub use kelpserver::native::{
        bridge, connection, diagnostics, engine, history, hooks, lifecycle, recovery, reference,
        stream, templates, upgrade, vitals,
    };
    pub mod cli;
    pub mod hover;
    pub mod input;
    pub mod protocol;
    pub mod render;
    pub mod session;
    pub mod status;
    pub mod tui;
    pub mod ui;
    pub mod web_assets;
    pub mod web_bridge;
    pub mod web_relay;
    pub mod web_repos;
    pub mod workspace;
}
pub mod picker;
pub mod tmux;
pub mod workspace;
