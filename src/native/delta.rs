//! Revisioned frame updates; implementation supplied by the phase-two transport worker.
use super::protocol::{Frame,Cell};
use serde::{Serialize,Deserialize};
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct Patch { pub base:u64,pub revision:u64,pub frame:Frame,pub changes:Vec<(usize,Cell)> }
