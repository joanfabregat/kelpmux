//! Bounded, pane-local OSC 7501 status. No prompts, messages or RPC interception.
use crate::{
    model::{Agent, Kind, Observation, Status, StatusSource},
    process::Process,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::collections::BTreeMap;

pub const REPLY: &[u8] =
    b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";
const LIMIT: usize = 4096;
const RECORDS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Working,
    Blocked,
    Done,
    Error,
    Clear,
}
impl State {
    fn status(self) -> Status {
        match self {
            Self::Idle => Status::Idle,
            Self::Working => Status::Working,
            Self::Blocked => Status::InputNeeded,
            Self::Done => Status::TurnFinished,
            Self::Error => Status::Failed,
            Self::Clear => Status::Unknown,
        }
    }
    fn priority(self) -> u8 {
        match self {
            Self::Blocked => 5,
            Self::Working => 4,
            Self::Error => 3,
            Self::Done => 2,
            Self::Idle => 1,
            Self::Clear => 0,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub state: State,
    pub id: String,
    pub app: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Query,
    Report(Report),
    Reset,
    Prompt,
}

fn segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.+-".contains(&c))
}
fn text(value: &str, encoded: usize, decoded: usize) -> bool {
    if value.len() > encoded {
        return false;
    }
    let mut padded = value.to_owned();
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    STANDARD
        .decode(padded)
        .ok()
        .filter(|s| s.len() <= decoded)
        .and_then(|s| String::from_utf8(s).ok())
        .is_some_and(|s| !s.chars().any(char::is_control))
}
fn report(body: &[u8]) -> Option<Event> {
    let body = std::str::from_utf8(body).ok()?.strip_prefix("7501;")?;
    if body.trim() == "?" {
        return Some(Event::Query);
    }
    let mut fields = BTreeMap::new();
    for pair in body.split(':') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key.len() > 16 {
            return None;
        }
        if key.is_empty()
            || !key.bytes().all(|c| c.is_ascii_lowercase())
            || !value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_.,+/=-".contains(&c))
        {
            continue;
        }
        if (key == "msg" && !text(value, 2732, 2048))
            || (key == "title" && !text(value, 256, 192))
            || (key == "app" && value.len() > 32)
            || (key == "id" && value.len() > 128)
        {
            return None;
        }
        // Validate free text, then discard it; it must never enter the journal.
        if matches!(key, "state" | "id" | "app") {
            fields.insert(key, value);
        }
    }
    let state = match *fields.get("state")? {
        "idle" => State::Idle,
        "working" => State::Working,
        "blocked" => State::Blocked,
        "done" => State::Done,
        "error" => State::Error,
        "clear" => State::Clear,
        _ => return None,
    };
    let id = fields.get("id").copied().unwrap_or("");
    if fields.contains_key("id") && (id.split('/').count() > 8 || !id.split('/').all(segment)) {
        return None;
    }
    Some(Event::Report(Report {
        state,
        id: id.into(),
        app: fields
            .get("app")
            .filter(|s| segment(s))
            .map(|s| (*s).into()),
    }))
}

#[derive(Default)]
enum Lex {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc {
        body: Vec<u8>,
        escaped: bool,
        overflow: bool,
    },
    String(bool),
}
#[derive(Default)]
pub struct Parser {
    state: Lex,
}
impl Parser {
    fn escape(byte: u8) -> Lex {
        match byte {
            b']' => Lex::Osc {
                body: vec![],
                escaped: false,
                overflow: false,
            },
            b'[' => Lex::Csi,
            b'P' | b'_' | b'^' | b'X' => Lex::String(false),
            0x1b => Lex::Escape,
            _ => Lex::Ground,
        }
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        let mut events = vec![];
        for &byte in bytes {
            let mut event = None;
            self.state = match std::mem::take(&mut self.state) {
                Lex::Ground => {
                    if byte == 0x1b {
                        Lex::Escape
                    } else {
                        Lex::Ground
                    }
                }
                Lex::Escape => {
                    if byte == b'c' {
                        event = Some(Event::Reset);
                    }
                    Self::escape(byte)
                }
                Lex::Csi => {
                    if byte == 0x1b {
                        Lex::Escape
                    } else if (0x40..=0x7e).contains(&byte) || matches!(byte, 0x18 | 0x1a) {
                        Lex::Ground
                    } else {
                        Lex::Csi
                    }
                }
                Lex::String(escaped) => {
                    if escaped && byte == b'\\' || matches!(byte, 0x18 | 0x1a) {
                        Lex::Ground
                    } else {
                        Lex::String(byte == 0x1b)
                    }
                }
                Lex::Osc {
                    mut body,
                    escaped,
                    mut overflow,
                } => {
                    if (escaped && byte == b'\\') || byte == 7 {
                        if !overflow {
                            event = if body == b"133;A" || body.starts_with(b"133;A;") {
                                Some(Event::Prompt)
                            } else {
                                report(&body)
                            };
                        }
                        Lex::Ground
                    } else if matches!(byte, 0x18 | 0x1a) {
                        Lex::Ground
                    } else if escaped {
                        Self::escape(byte)
                    } else if byte == 0x1b {
                        Lex::Osc {
                            body,
                            escaped: true,
                            overflow,
                        }
                    } else {
                        if body.len() < LIMIT - 4 {
                            body.push(byte);
                        } else {
                            overflow = true;
                        }
                        Lex::Osc {
                            body,
                            escaped: false,
                            overflow,
                        }
                    }
                }
            };
            if let Some(event) = event {
                if events.len() < 256 {
                    events.push(event);
                }
            }
        }
        events
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Records {
    entries: BTreeMap<String, (Report, u64)>,
    sequence: u64,
}
impl Records {
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::Reset => self.entries.clear(),
            Event::Prompt => self
                .entries
                .retain(|_, (r, _)| !matches!(r.state, State::Working | State::Blocked)),
            Event::Report(r) if r.state == State::Clear => self.entries.retain(|id, _| {
                !(r.id.is_empty()
                    || *id == r.id
                    || id
                        .strip_prefix(&r.id)
                        .is_some_and(|tail| tail.starts_with('/')))
            }),
            Event::Report(r) => {
                self.sequence = self.sequence.saturating_add(1);
                if !self.entries.contains_key(&r.id) && self.entries.len() == RECORDS {
                    if let Some(id) = self
                        .entries
                        .iter()
                        .min_by_key(|(_, (_, at))| at)
                        .map(|(id, _)| id.clone())
                    {
                        self.entries.remove(&id);
                    }
                }
                self.entries
                    .insert(r.id.clone(), (r.clone(), self.sequence));
            }
            Event::Query => (),
        }
    }
    pub fn status(&self, kind: Kind) -> Option<Status> {
        self.entries
            .iter()
            .filter_map(|(id, (record, _))| {
                let mut inherited = id.as_str();
                let app = loop {
                    if let Some(app) = self
                        .entries
                        .get(inherited)
                        .and_then(|(r, _)| r.app.as_deref())
                    {
                        break Some(app);
                    }
                    if inherited.is_empty() {
                        break None;
                    }
                    inherited = inherited.rsplit_once('/').map_or("", |(parent, _)| parent);
                };
                let matches = match kind {
                    Kind::Codex => app == Some("codex"),
                    Kind::Claude => app == Some("claude-code"),
                };
                matches.then_some(record.state)
            })
            .max_by_key(|state| state.priority())
            .map(State::status)
    }
}

/// Bind reports to the foreground job's PID/birth, never a folder or daemon.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub owner: (u32, u64),
    pub agent: Option<(u32, u64)>,
    pub records: Records,
    pub event_at: u64,
}
impl Snapshot {
    pub fn apply(&self, agent: &mut Agent, tree: &BTreeMap<u32, Process>, time: u64) -> bool {
        if self
            .agent
            .is_some_and(|identity| identity != (agent.pid, agent.process_start))
        {
            return false;
        }
        let Some(process) = tree
            .get(&agent.pid)
            .filter(|p| p.start == agent.process_start)
        else {
            return false;
        };
        if !tree
            .get(&self.owner.0)
            .is_some_and(|p| p.start == self.owner.1)
            || process.group != self.owner.0 as i32
            || process.foreground != process.group
            || process.state == 'T'
        {
            return false;
        }
        let Some(status) = self.records.status(agent.kind) else {
            return false;
        };
        agent.status = status;
        agent.observation = Observation {
            source: StatusSource::Terminal,
            checked_at: time,
            event_at: Some(self.event_at),
        };
        true
    }
}
