//! Frozen TUI and legacy tmux client, using the public terminal foundation.
pub use kelpserver::{Result, discovery, journal, label, now, process, session_id, shell_quote};
pub mod model {
    pub use kelpclient::model::*;
}
pub mod native {
    pub use kelpserver::native::*;
    pub mod cli;
    pub mod hover;
    pub mod input;
}
pub mod picker;
pub mod tmux;
pub mod workspace;
