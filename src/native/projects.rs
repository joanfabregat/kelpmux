//! Which project each terminal belongs to, shared by every browser.
//!
//! A terminal is linked to a project when it is created in that project, or
//! when it is moved to another one; it is never inferred from its folder.
//! Terminals without a link belong to the built-in Scratch project. Links
//! live in the workspace's state directory, in `terminal-projects.json`,
//! keyed by the terminal's identity on the server: `<run epoch>:<pane>`,
//! which is also the key of the closed-terminal record KelpServer keeps when
//! it closes, so a closed terminal keeps its project.
//!
//! Several bridges (one per browser connection) write it: each change takes
//! a `flock` on `.terminal-projects.json.lock` beside it, reads the file,
//! writes a temporary file, fsyncs it and renames it over the file. The
//! file keeps the newest [`MAX_LINKS`] links.
use super::user_config;
use crate::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// The file's name in the workspace's state directory.
pub const FILE_NAME: &str = "terminal-projects.json";
/// Most links kept; the oldest go first.
pub const MAX_LINKS: usize = 4096;
/// Largest accepted file.
const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
struct Link {
    project: String,
    /// When the link was made (Unix seconds), to keep the newest.
    at: u64,
}

#[derive(Serialize, Deserialize, Default, Debug)]
struct Stored {
    version: u32,
    links: BTreeMap<String, Link>,
}

/// A workspace's terminal → project links.
#[derive(Debug, Clone)]
pub struct Links {
    path: PathBuf,
}
impl Links {
    /// The links of the workspace whose state directory is `state`.
    pub fn at(state: &Path) -> Self {
        Self {
            path: state.join(FILE_NAME),
        }
    }
    /// A terminal's key: its server run and pane.
    pub fn key(epoch: &str, pane: u64) -> String {
        format!("{epoch}:{pane}")
    }
    fn load(&self) -> Stored {
        use std::io::Read;
        let Ok(file) = std::fs::File::open(&self.path) else {
            return Stored::default();
        };
        let mut bytes = Vec::new();
        if file.take(MAX_BYTES).read_to_end(&mut bytes).is_err() {
            return Stored::default();
        }
        serde_json::from_slice(&bytes).unwrap_or_default()
    }
    /// Every link: terminal key → project id.
    pub fn all(&self) -> BTreeMap<String, String> {
        self.load()
            .links
            .into_iter()
            .map(|(key, link)| (key, link.project))
            .collect()
    }
    /// The project ids links still name.
    pub fn projects(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.all().into_values().collect();
        ids.sort();
        ids.dedup();
        ids
    }
    /// The live terminals of run `epoch` with a project: pane → project id.
    pub fn panes(&self, epoch: &str) -> BTreeMap<u64, String> {
        let prefix = format!("{epoch}:");
        self.all()
            .into_iter()
            .filter_map(|(key, project)| {
                let pane = key.strip_prefix(&prefix)?.parse().ok()?;
                Some((pane, project))
            })
            .collect()
    }
    /// Changes the links under the lock and saves them.
    fn change(&self, edit: impl FnOnce(&mut BTreeMap<String, Link>)) -> Result<()> {
        if let Some(parent) = self.path.parent()
            && !parent.is_dir()
        {
            return Err("the workspace state directory does not exist".into());
        }
        let _lock = user_config::WriteLock::acquire(&self.path)?;
        let mut stored = self.load();
        edit(&mut stored.links);
        if stored.links.len() > MAX_LINKS {
            let mut ages: Vec<(u64, String)> = stored
                .links
                .iter()
                .map(|(key, link)| (link.at, key.clone()))
                .collect();
            ages.sort();
            let excess = stored.links.len() - MAX_LINKS;
            for (_, key) in ages.into_iter().take(excess) {
                stored.links.remove(&key);
            }
        }
        stored.version = 1;
        user_config::replace(&self.path, &serde_json::to_string(&stored)?)
    }
    /// Links terminal `key` to `project`; `None` (Scratch) removes its link.
    pub fn set(&self, key: &str, project: Option<&str>) -> Result<()> {
        self.change(|links| match project {
            Some(project) => {
                links.insert(
                    key.to_owned(),
                    Link {
                        project: project.to_owned(),
                        at: crate::now(),
                    },
                );
            }
            None => {
                links.remove(key);
            }
        })
    }
    /// Gives terminal `to` the project of `from` (a closed terminal reopened
    /// in a new one), unless `to` already has one.
    pub fn inherit(&self, from: &str, to: &str) -> Result<()> {
        let Some(project) = self.all().get(from).cloned() else {
            return Ok(());
        };
        self.change(|links| {
            links.entry(to.to_owned()).or_insert(Link {
                project,
                at: crate::now(),
            });
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "kelpmux-projects-{name}-{}-{}",
            std::process::id(),
            crate::now()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn links_are_set_moved_to_scratch_and_inherited() {
        let dir = temp("links");
        let links = Links::at(&dir);
        assert!(links.all().is_empty());
        links.set(&Links::key("run", 3), Some("kelp")).unwrap();
        links.set(&Links::key("run", 4), Some("web")).unwrap();
        links.set(&Links::key("old", 9), Some("kelp")).unwrap();
        assert_eq!(
            links.panes("run"),
            BTreeMap::from([(3, "kelp".to_owned()), (4, "web".to_owned())])
        );
        links.set("run:4", None).unwrap();
        assert_eq!(links.panes("run").len(), 1);
        // A closed terminal reopened keeps its project; an existing link stays.
        links.inherit("old:9", "run:7").unwrap();
        links.inherit("run:3", "run:7").unwrap();
        links.inherit("missing", "run:8").unwrap();
        assert_eq!(links.panes("run").get(&7).unwrap(), "kelp");
        assert!(!links.panes("run").contains_key(&8));
        assert_eq!(links.projects(), ["kelp"]);
        // A damaged file reads as no links rather than failing.
        fs::write(dir.join(FILE_NAME), "{").unwrap();
        assert!(links.all().is_empty());
        links.set("run:1", Some("x")).unwrap();
        assert_eq!(links.all().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_writers_all_land_and_the_oldest_links_go() {
        let dir = temp("concurrent");
        let threads: Vec<_> = (0..8)
            .map(|n| {
                let links = Links::at(&dir);
                std::thread::spawn(move || {
                    for pane in 0..10 {
                        links
                            .set(&Links::key("run", n * 100 + pane), Some("p"))
                            .unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(Links::at(&dir).all().len(), 80);
        let _ = fs::remove_dir_all(&dir);
    }
}
