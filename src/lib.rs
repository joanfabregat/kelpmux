//! Kelpmux coordinates tmux workspaces without owning agent processes.
pub mod discovery;
pub mod journal;
pub mod model;
pub mod native;
pub mod picker;
pub mod process;
pub mod tmux;
pub mod workspace;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Escape shell arguments for optional tmux command bindings.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Keep tmux format expansion and control characters out of user-facing labels.
pub fn label(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect::<String>()
        .replace('#', "##")
}

pub fn session_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
}
