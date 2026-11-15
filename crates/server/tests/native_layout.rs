use kelpserver::native::{
    lifecycle::TabLayout,
    protocol::*,
    server::{ServerConfig, serve},
};
use serde_json::json;
use std::{
    fs,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
struct Daemon {
    root: PathBuf,
    thread: Option<thread::JoinHandle<kelpserver::Result<()>>>,
}
impl Daemon {
    /// Start a workspace whose saved tab layout contains the given panes.
    fn start(root: PathBuf, panes: serde_json::Value) -> Self {
        let state = root.join("state");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        let count = panes.as_array().unwrap().len();
        let layout = state.join("tab-layout.json");
        fs::write(
            &layout,
            serde_json::to_vec(&json!({
                "template": null,
                "cwd": root,
                "shell": "/bin/sh",
                "panes": panes,
            }))
            .unwrap(),
        )
        .unwrap();
        fs::set_permissions(&layout, fs::Permissions::from_mode(0o600)).unwrap();
        let config = ServerConfig {
            initial_weights: Default::default(),
            template: None,
            socket: root.join("control.sock"),
            state_dir: state,
            shell: "/bin/sh".into(),
            cwd: root.clone(),
            cols: 100,
            rows: 30,
            ui: Default::default(),
            initial_grid: (1, 1),
            initial_tabs: Some(count),
        };
        let handle = thread::spawn(move || serve(config));
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(root.join("control.sock")).is_err() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        Self {
            root,
            thread: Some(handle),
        }
    }
    fn panes(&self) -> Vec<PaneInfo> {
        let mut s = UnixStream::connect(self.root.join("control.sock")).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write_message(&mut s, &ClientMessage::Inspect).unwrap();
        match read_message(&mut s).unwrap().unwrap() {
            ServerMessage::State { panes, .. } => panes,
            other => panic!("{other:?}"),
        }
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        if let Ok(mut s) = UnixStream::connect(self.root.join("control.sock")) {
            let _ = write_message(&mut s, &ClientMessage::Command(Action::Shutdown));
        }
        if let Some(t) = self.thread.take() {
            assert!(t.join().unwrap().is_ok());
        }
        fs::remove_dir_all(&self.root).unwrap();
    }
}
fn root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("kelpmux-{name}-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    root.canonicalize().unwrap()
}
fn process_cwd(pid: u32) -> PathBuf {
    fs::read_link(format!("/proc/{pid}/cwd")).unwrap()
}

#[test]
fn saved_tabs_in_missing_folders_start_in_the_workspace_folder() {
    let root = root("layout-missing-cwd");
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    let daemon = Daemon::start(
        root.clone(),
        json!([
            {"name": null, "cwd": root.join("moved"), "shell": "/bin/sh"},
            {"name": "code", "cwd": root.join("deleted/deeper"), "shell": "/bin/sh"},
            {"name": null, "cwd": project, "shell": "/bin/sh"},
        ]),
    );
    let panes = daemon.panes();
    assert_eq!(panes.len(), 3);
    for (pane, folder) in panes.iter().zip([&root, &root, &project]) {
        assert_eq!(Path::new(&pane.cwd), folder.as_path());
        assert_eq!(&process_cwd(pane.pid), folder);
    }
    assert_eq!(panes[1].title, "code");
    // The next start reuses the folders the shells actually received.
    let saved = TabLayout::load(&root.join("state/tab-layout.json")).unwrap();
    let folders: Vec<_> = saved.panes.iter().map(|pane| pane.cwd.clone()).collect();
    assert_eq!(folders, [root.clone(), root.clone(), project]);
    assert_eq!(saved.panes[1].name.as_deref(), Some("code"));
    assert!(saved.panes[0].name.is_none());
}

#[test]
fn unstartable_saved_tabs_are_skipped_but_one_shell_remains() {
    let root = root("layout-bad-shell");
    let daemon = Daemon::start(
        root.clone(),
        json!([
            {"name": "a", "cwd": root, "shell": root.join("missing-shell")},
            {"name": "b", "cwd": root.join("moved"), "shell": root.join("missing-shell")},
        ]),
    );
    let panes = daemon.panes();
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0].title, "shell");
    assert_eq!(process_cwd(panes[0].pid), root);
}
