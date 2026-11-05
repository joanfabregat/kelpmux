use crate::Result;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct Process {
    pub pid: u32,
    pub parent: u32,
    pub group: i32,
    pub foreground: i32,
    pub tty: i64,
    pub start: u64,
    pub state: char,
    pub command: String,
}

pub fn parse_stat(pid: u32, text: &str) -> Result<Process> {
    let open = text.find('(').ok_or("invalid process stat")?;
    let close = text.rfind(')').ok_or("invalid process stat")?;
    let fields: Vec<_> = text[close + 1..].split_whitespace().collect();
    if fields.len() < 20 {
        return Err("short process stat".into());
    }
    Ok(Process {
        pid,
        command: text[open + 1..close].to_owned(),
        state: fields[0].chars().next().ok_or("missing state")?,
        parent: fields[1].parse()?,
        group: fields[2].parse()?,
        tty: fields[4].parse()?,
        foreground: fields[5].parse()?,
        start: fields[19].parse()?,
    })
}

pub fn read(root: &Path, pid: u32) -> Result<Process> {
    parse_stat(
        pid,
        &fs::read_to_string(root.join(pid.to_string()).join("stat"))?,
    )
}

pub fn snapshot(root: &Path) -> Result<BTreeMap<u32, Process>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() {
            if let Ok(proc) = read(root, pid) {
                result.insert(pid, proc);
            }
        }
    }
    Ok(result)
}

pub fn descendants(tree: &BTreeMap<u32, Process>, root: u32) -> BTreeSet<u32> {
    let mut result = BTreeSet::from([root]);
    loop {
        let before = result.len();
        for p in tree.values() {
            if result.contains(&p.parent) {
                result.insert(p.pid);
            }
        }
        if before == result.len() {
            return result;
        }
    }
}

pub fn args(root: &Path, pid: u32) -> Vec<String> {
    fs::read(root.join(pid.to_string()).join("cmdline"))
        .unwrap_or_default()
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

pub fn environment(root: &Path, pid: u32, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    fs::read(root.join(pid.to_string()).join("environ"))
        .ok()?
        .split(|b| *b == 0)
        .find_map(|s| {
            std::str::from_utf8(s)
                .ok()?
                .strip_prefix(&prefix)
                .map(str::to_owned)
        })
}

pub fn links(root: &Path, pid: u32) -> Vec<PathBuf> {
    fs::read_dir(root.join(pid.to_string()).join("fd"))
        .into_iter()
        .flatten()
        .filter_map(|e| fs::read_link(e.ok()?.path()).ok())
        .collect()
}

pub fn idle(tree: &BTreeMap<u32, Process>, pid: u32, allow_stopped: bool) -> bool {
    let Some(p) = tree.get(&pid) else {
        return false;
    };
    ["bash", "zsh", "fish", "sh", "dash", "ksh"].contains(&p.command.as_str())
        && p.tty != 0
        && p.group > 0
        && p.group == p.foreground
        && (p.state == 'S' || (allow_stopped && p.state == 'T'))
        && descendants(tree, pid).len() == 1
}
