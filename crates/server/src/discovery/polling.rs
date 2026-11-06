//! Shared polling lifecycle and statically registered agent adapters.
//! Adapters never launch/resume agents. The owner controls the polling interval.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub kind: Kind,
    pub id: String,
    pub name: String,
    pub status: Status,
}
#[derive(Debug, Clone)]
pub struct Link {
    pub pane: String,
    pub pid: u32,
    pub birth: u64,
    pub kind: Kind,
    pub session_id: String,
}
/// Implement this interface and register an adapter to add/replace a poller.
/// Errors affect only that observation; unavailable evidence stays UNKNOWN.
pub trait Adapter: Send {
    fn kind(&self) -> Kind;
    fn matches(&self, process: &Process, args: &[String]) -> bool;
    fn begin_cycle(&mut self) {}
    fn poll(
        &mut self,
        context: &Discovery,
        process: &Process,
        args: &[String],
        agent: &mut Agent,
    ) -> Result<()>;
    fn sessions(&mut self, _context: &Discovery) -> Result<Vec<Session>> {
        Ok(vec![])
    }
}
struct Claude;
impl Adapter for Claude {
    fn kind(&self) -> Kind {
        Kind::Claude
    }
    fn matches(&self, process: &Process, args: &[String]) -> bool {
        super::classify(process, args) == Some(self.kind())
    }
    fn poll(
        &mut self,
        context: &Discovery,
        process: &Process,
        args: &[String],
        agent: &mut Agent,
    ) -> Result<()> {
        super::claude::enrich(context, process, args, agent);
        Ok(())
    }
}
#[derive(Default)]
struct Codex(super::codex::Daemon);
impl Adapter for Codex {
    fn kind(&self) -> Kind {
        Kind::Codex
    }
    fn matches(&self, process: &Process, args: &[String]) -> bool {
        super::classify(process, args) == Some(self.kind())
    }
    fn begin_cycle(&mut self) {
        self.0 = Default::default();
    }
    fn poll(
        &mut self,
        context: &Discovery,
        process: &Process,
        args: &[String],
        agent: &mut Agent,
    ) -> Result<()> {
        super::codex::enrich(context, &mut self.0, process, args, agent);
        Ok(())
    }
    fn sessions(&mut self, context: &Discovery) -> Result<Vec<Session>> {
        self.0.sessions(context)
    }
}
pub fn builtins() -> Vec<Box<dyn Adapter>> {
    vec![Box::new(Codex::default()), Box::new(Claude)]
}
pub struct Cycle {
    pub agents: Vec<Agent>,
    pub tree: BTreeMap<u32, Process>,
}
pub struct Poller {
    context: Discovery,
    adapters: Vec<Box<dyn Adapter>>,
}
impl Poller {
    pub fn new(context: Discovery) -> Self {
        Self::with_adapters(context, builtins())
    }
    pub fn with_adapters(context: Discovery, adapters: Vec<Box<dyn Adapter>>) -> Self {
        Self { context, adapters }
    }
    pub fn poll(&mut self, panes: &[Pane], links: &[Link], time: u64) -> Result<Cycle> {
        for adapter in &mut self.adapters {
            adapter.begin_cycle();
        }
        let tree = process::snapshot(&self.context.proc_root)?;
        let agents = self
            .context
            .scan_tree(panes, time, &tree, &mut self.adapters, links)?;
        Ok(Cycle { agents, tree })
    }
    pub fn sessions(&mut self, kind: Kind) -> Result<Vec<Session>> {
        let mut sessions = Vec::new();
        for adapter in self
            .adapters
            .iter_mut()
            .filter(|adapter| adapter.kind() == kind)
        {
            sessions.extend(adapter.sessions(&self.context)?);
        }
        sessions.retain(|session| session.kind == kind && crate::session_id(&session.id));
        sessions.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        let mut seen = std::collections::BTreeSet::new();
        sessions.retain(|session| seen.insert(session.id.clone()));
        sessions.truncate(128);
        for session in &mut sessions {
            session.name = session
                .name
                .chars()
                .filter(|c| !c.is_control())
                .take(128)
                .collect();
        }
        Ok(sessions)
    }
}
