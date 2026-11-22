//! Kelp TUI and legacy tmux client, using the public terminal foundation.
pub use kelpserver::{Result, discovery, journal, label, now, process, session_id, shell_quote};
pub mod model {
    pub use kelpclient::model::*;
}
pub mod native {
    // KelpServer modules KelpMux uses: the session server's stream, workspace
    // lifecycle, closed-session history, server identity and the SSH bridge.
    // KelpServer's composed TUI (protocol 58 frames, `ui`, `render`,
    // `workspace`, its controller and `Action` handling) is not used: the
    // stream TUI renders locally with the modules below, moved or copied
    // here by ADR 0002 step 2, so KelpServer can drop them in step 3.
    pub use kelpserver::native::{
        bridge, connection, diagnostics, engine, history, hooks, lifecycle, recovery, reference,
        stream, templates, upgrade, vitals,
    };
    pub mod cli;
    pub mod hover;
    pub mod input;
    pub mod protocol;
    pub mod render;
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
