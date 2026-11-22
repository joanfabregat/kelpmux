//! Kelp TUI and legacy tmux client, using the public terminal foundation.
pub use kelpserver::{Result, discovery, journal, label, now, process, session_id, shell_quote};
pub mod model {
    pub use kelpclient::model::*;
}
pub mod native {
    // Modules defined below shadow KelpServer's modules of the same name. They
    // were moved here by ADR 0002 step 2 (TUI rendering and layout, the Status
    // dialog's presentation and the web relay); KelpServer drops its copies
    // in step 3. The composed protocol's data types (`protocol`, `ui`) still
    // come from KelpServer until then.
    pub use kelpserver::native::*;
    pub mod cli;
    pub mod hover;
    pub mod input;
    pub mod render;
    pub mod status;
    pub mod tui;
    pub mod web_assets;
    pub mod web_bridge;
    pub mod web_relay;
    pub mod web_repos;
    pub mod workspace;
}
pub mod picker;
pub mod tmux;
pub mod workspace;
