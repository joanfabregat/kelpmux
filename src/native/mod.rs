//! Owned Linux multiplexer. The legacy tmux adapter remains independent.
pub mod cli;
pub mod delta;
pub mod engine;
pub mod hooks;
pub mod input;
pub mod lifecycle;
pub mod protocol;
pub mod render;
pub mod server;
pub mod tracking;
pub mod ui;
pub mod vitals;
pub mod workspace;
