//! Bounded metadata-only recovery across private named workspace directories.
use super::{
    protocol::RecoveryItem,
    tracking::{self, RecoveryEntry},
};
use crate::{Result, journal::RETENTION};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
const MAX_WORKSPACES: usize = 64;
const MAX_METADATA_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PER_WORKSPACE: usize = 256;
const MAX_ITEMS: usize = MAX_WORKSPACES * MAX_PER_WORKSPACE;
fn private(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    for ancestor in path.ancestors() {
        if fs::symlink_metadata(ancestor).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return false;
        }
    }
    fs::symlink_metadata(path).is_ok_and(|meta| {
        meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0
    })
}
fn workspace_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    (!name.is_empty()
        && name.len() <= 48
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
    .then(|| name.into())
}
fn readable_journal(state: &Path) -> bool {
    let root = state.join("native");
    if !private(&root) {
        return false;
    }
    for name in ["registry.json", "journal.jsonl", "registry.lock"] {
        match fs::symlink_metadata(root.join(name)) {
            Ok(meta)
                if !meta.is_file()
                    || meta.uid() != unsafe { libc::geteuid() }
                    || meta.mode() & 0o077 != 0
                    || meta.len() > MAX_METADATA_BYTES =>
            {
                return false;
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return false,
            _ => (),
        }
    }
    true
}
fn entries(state: &Path, time: u64) -> Vec<RecoveryEntry> {
    if !private(state) || !readable_journal(state) {
        return vec![];
    }
    let Ok(mut found) = tracking::recovery_at(state, time) else {
        return vec![];
    };
    // Keep current registry agents even when their last scan predates retention.
    // Live is a retained observation, never a claim that a vanished host PID survived.
    if let Ok(store) = crate::journal::Store::open_nonblocking(&state.join("native")) {
        for agent in store
            .registry
            .agents
            .values()
            .filter(|agent| agent.ended_at.is_none())
        {
            if !found.iter().any(|entry| {
                entry.pid == agent.pid
                    && entry.process_start == agent.process_start
                    && entry.session_id == agent.session_id
            }) {
                found.push(agent.clone().into());
            }
        }
    }
    found.retain(|entry| {
        entry.ended_at.is_none()
            || entry.ended_at.unwrap_or(entry.last_seen) >= time.saturating_sub(RETENTION)
    });
    found.sort_by(|a, b| {
        a.ended_at
            .is_some()
            .cmp(&b.ended_at.is_some())
            .then(b.last_seen.cmp(&a.last_seen))
    });
    found.into_iter().take(MAX_PER_WORKSPACE).collect()
}
pub fn collect(state: &Path) -> Result<Vec<RecoveryItem>> {
    let root = state
        .parent()
        .filter(|parent| parent.file_name().is_some_and(|name| name == "workspaces"));
    collect_at(state, root, crate::now())
}
pub fn collect_from(state: &Path, workspace_root: &Path) -> Result<Vec<RecoveryItem>> {
    collect_at(state, Some(workspace_root), crate::now())
}
pub fn collect_cancellable(
    state: &Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Vec<RecoveryItem>> {
    let root = state
        .parent()
        .filter(|parent| parent.file_name().is_some_and(|name| name == "workspaces"));
    collect_inner(state, root, crate::now(), Some(cancel))
}
/// Deterministic retention; a custom current state does not implicitly inspect sibling directories.
pub fn collect_at(
    state: &Path,
    workspace_root: Option<&Path>,
    time: u64,
) -> Result<Vec<RecoveryItem>> {
    collect_inner(state, workspace_root, time, None)
}
fn collect_inner(
    state: &Path,
    workspace_root: Option<&Path>,
    time: u64,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<RecoveryItem>> {
    if !private(state) {
        return Err("recovery requires an owned private workspace directory".into());
    }
    let mut states = Vec::<(String, PathBuf)>::new();
    states.push((
        workspace_name(state).unwrap_or_else(|| "current".into()),
        state.to_path_buf(),
    ));
    if let Some(root) = workspace_root.filter(|root| private(root)) {
        if let Ok(directories) = fs::read_dir(root) {
            for directory in directories.take(MAX_WORKSPACES * 4).flatten() {
                let path = directory.path();
                if path == state || !private(&path) {
                    continue;
                }
                if let Some(name) = workspace_name(&path) {
                    states.push((name, path));
                    if states.len() >= MAX_WORKSPACES {
                        break;
                    }
                }
            }
        }
    }
    states.sort_by(|a, b| a.0.cmp(&b.0));
    let mut result = BTreeMap::new();
    for (workspace, state) in states {
        if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed)) {
            break;
        }
        for entry in entries(&state, time) {
            let key = (
                workspace.clone(),
                entry.kind.as_str().to_owned(),
                entry.pid,
                entry.process_start,
                entry.session_id.clone(),
            );
            result.insert(
                key,
                RecoveryItem {
                    workspace: workspace.clone(),
                    entry,
                },
            );
            if result.len() >= MAX_ITEMS {
                break;
            }
        }
    }
    let mut items: Vec<_> = result.into_values().collect();
    items.sort_by(|a, b| {
        a.workspace
            .cmp(&b.workspace)
            .then(a.entry.cwd.cmp(&b.entry.cwd))
            .then(b.entry.last_seen.cmp(&a.entry.last_seen))
            .then(a.entry.pid.cmp(&b.entry.pid))
    });
    Ok(items)
}
