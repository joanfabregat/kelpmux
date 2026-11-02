use super::*;
pub(super) fn enrich(context: &Discovery, proc: &Process, args: &[String], agent: &mut Agent) {
    let file = context
        .claude_home
        .join("sessions")
        .join(format!("{}.json", proc.pid));
    if let Some(value) = read_json(&file) {
        let start = value.get("procStart").and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .or_else(|| v.as_u64())
        });
        if value.get("pid").and_then(Value::as_u64) == Some(proc.pid as u64)
            && start == Some(proc.start)
        {
            agent.session_id =
                string(&value, &["sessionId", "session_id"]).filter(|s| session_id(s));
            if let Some(name) = string(&value, &["name"]) {
                agent.name = name;
            }
            if let Some(status) = string(&value, &["status"]) {
                agent.status = Status::from_live(&status);
                agent.observation.source = crate::model::StatusSource::LiveRecord;
            }
            agent.source = "claude-live-record".into();
        }
    }
    if agent.session_id.is_none() {
        agent.session_id = resume_id(args, true);
        if agent.session_id.is_some() {
            agent.source = "resume-argument".into();
        }
    }
}
