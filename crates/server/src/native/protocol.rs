//! The local socket envelope, its framing and the data types shared by stream
//! clients. Every record is a four-byte big-endian length and a UTF-8 JSON
//! payload of at most `MAX_MESSAGE` bytes. A client sends
//! `ClientMessage::Stream` requests; the server answers with
//! `ServerMessage::Stream` events, or `ServerMessage::Error` for a record it
//! cannot accept (such as a message of the composed protocol 58 that 0.3
//! removed), after which it closes the connection. A connection that enabled
//! `binary-screens` also receives binary screen records, whose payload starts
//! with `kelpclient_protocol::screens::RECORD_MARKER` (never a JSON byte); read
//! them with `read_server_record`.
use crate::{Result, model::Status};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub use kelpclient_protocol::types::*;
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;
pub const MAX_INPUT: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseKind {
    Press,
    Release,
    Move,
    WheelUp,
    WheelDown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MouseEvent {
    pub position: Point,
    pub kind: MouseKind,
    pub button: u8,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}
/// A record sent to the server: `{"Stream":{"type":…}}`, or the one-shot
/// administration request `"Diagnostics"` on a connection without Hello.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    Stream(super::stream::Request),
    /// Same-UID diagnostics without a stream session; stream clients send
    /// the `diagnostics` request instead.
    Diagnostics,
}
/// A record sent by the server: `{"Stream":{"type":…}}`, `{"Error":"…"}` or
/// the reply to `ClientMessage::Diagnostics`. Stream events stay unboxed so
/// clients can match `ServerMessage::Stream(event)` directly.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    Stream(super::stream::Event),
    /// The server could not accept a record and closes the connection.
    Error(String),
    Diagnostics(super::diagnostics::Report),
}

/// An immutable search preview. Coordinates address these rows, never a later
/// history page whose relative line numbers may have shifted with new output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneSearchMatch {
    pub source: String,
    pub first_line: i32,
    pub lines: Vec<String>,
    pub start: Point,
    pub end: Point,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    #[serde(default)]
    pub origin: Option<NotificationOrigin>,
    pub id: u64,
    pub pane: PaneId,
    pub title: String,
    pub status: Status,
    pub at: u64,
    pub read: bool,
    pub workspace: String,
    /// A terminal bell accompanied this typed agent notification and was
    /// merged into it (`bell-merge`): no separate bell record exists.
    /// Omitted from the wire when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bell: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationOrigin {
    pub run: String,
    pub pid: u32,
}

pub fn write_message<T: Serialize>(writer: &mut impl Write, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_MESSAGE {
        return Err("native protocol message is too large".into());
    }
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}
pub fn read_message<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<Option<T>> {
    match read_frame(reader)? {
        Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        None => Ok(None),
    }
}
/// One framed payload, unparsed; `None` at end of stream.
pub fn read_frame(reader: &mut impl Read) -> Result<Option<Vec<u8>>> {
    let mut header = [0u8; 4];
    let count = loop {
        match reader.read(&mut header[..1]) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            result => break result?,
        }
    };
    if count == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut header[1..])?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_MESSAGE {
        return Err("invalid native protocol message length".into());
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(Some(bytes))
}
/// A server record on a connection that may have enabled `binary-screens`.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum ServerRecord {
    Message(ServerMessage),
    /// A binary screen record; decode it with `kelpclient_protocol::screens::decode`.
    Screens(Vec<u8>),
}
pub fn read_server_record(reader: &mut impl Read) -> Result<Option<ServerRecord>> {
    let Some(bytes) = read_frame(reader)? else {
        return Ok(None);
    };
    if kelpclient_protocol::screens::is_binary_record(&bytes) {
        return Ok(Some(ServerRecord::Screens(bytes)));
    }
    Ok(Some(ServerRecord::Message(serde_json::from_slice(&bytes)?)))
}
