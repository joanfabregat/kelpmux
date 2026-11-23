//! The TUI (ADR 0002 step 2): KelpMux's terminal interface as a client of
//! KelpServer's pane stream and session API. It renders locally with the
//! renderer moved from KelpServer (`native::render`, `native::workspace`) and
//! owns its UI state: tabs, order, names, colours, parking, focus history,
//! dialogs, grid geometry, scratch and settings. The state is persisted per
//! workspace by `state`. `kelpmux attach` and `ssh-entry` run it; the
//! composed-protocol TUI it replaced was removed.
pub mod attach;
pub mod controller;
pub mod notify;
pub mod state;
#[cfg(test)]
mod tests;
