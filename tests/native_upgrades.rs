//! Upgrade selection authenticates the live listener and pins its exact executable.
use kelpmux::native::{
    lifecycle,
    protocol::*,
    upgrade::{self, Identity},
};
use std::{
    fs,
    os::unix::{
        fs::{MetadataExt, PermissionsExt, symlink},
        net::UnixStream,
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "upgrade-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        Self { root }
    }
    fn installed(&self) -> (PathBuf, PathBuf, Identity) {
        let versions = self.root.join("versions");
        let path = versions.join("old/kelpmux");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::copy("/bin/true", &path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let identity = Identity {
            pid: std::process::id(),
            birth: 1,
            device: metadata.dev(),
            inode: metadata.ino(),
            executable: path.clone(),
            version: "0.0.1".into(),
            protocol: 3,
        };
        (versions, path, identity)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn exact_current_binary_needs_no_switch_and_old_installed_binary_is_pinned() {
    let fixture = Fixture::new();
    let (versions, path, identity) = fixture.installed();
    assert!(
        upgrade::matching_binary(&identity, &path, &versions)
            .unwrap()
            .is_none()
    );
    let file = upgrade::matching_binary(&identity, &std::env::current_exe().unwrap(), &versions)
        .unwrap()
        .unwrap();
    let retained_inode = file.metadata().unwrap().ino();
    fs::rename(&path, path.with_extension("old")).unwrap();
    fs::copy("/bin/false", &path).unwrap();
    assert_eq!(file.metadata().unwrap().ino(), retained_inode);
    assert!(
        upgrade::matching_binary(&identity, &std::env::current_exe().unwrap(), &versions).is_err()
    );
}
#[test]
fn client_selection_rejects_symlinks_foreign_roots_and_writable_files() {
    let fixture = Fixture::new();
    let (versions, path, mut identity) = fixture.installed();
    let current = std::env::current_exe().unwrap();
    assert!(
        upgrade::matching_binary(&identity, &current, &fixture.root.join("untrusted")).is_err()
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(upgrade::matching_binary(&identity, &current, &versions).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    let linked = versions.join("link");
    symlink(path.parent().unwrap(), &linked).unwrap();
    identity.executable = linked.join("kelpmux");
    assert!(upgrade::matching_binary(&identity, &current, &versions).is_err());
}
#[test]
fn peer_identity_ignores_pid_hint_but_rejects_stale_birth_and_unsafe_metadata() {
    let fixture = Fixture::new();
    let (stream, _other) = UnixStream::pair().unwrap();
    fs::write(fixture.root.join("server.pid"), "999999").unwrap();
    let original = upgrade::identify(&stream, &fixture.root).unwrap();
    assert_eq!(original.pid, std::process::id());
    upgrade::save(&fixture.root).unwrap();
    let saved = upgrade::identify(&stream, &fixture.root).unwrap();
    assert_eq!(saved.protocol, VERSION);
    let mut stale = saved;
    stale.birth += 1;
    let path = fixture.root.join("server.identity.json");
    fs::write(&path, serde_json::to_vec(&stale).unwrap()).unwrap();
    assert!(
        upgrade::identify(&stream, &fixture.root)
            .unwrap_err()
            .to_string()
            .contains("stale")
    );
    upgrade::save(&fixture.root).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(upgrade::identify(&stream, &fixture.root).is_err());
    fs::remove_file(&path).unwrap();
    symlink("server.pid", &path).unwrap();
    assert!(upgrade::identify(&stream, &fixture.root).is_err());
}
struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn new_client_uses_running_installed_client_without_restarting_workspace() {
    for installation in ["versions/v-retained", "builds/aaaaaaa"] {
        retained_client_survives_selection_change(installation);
    }
}
fn retained_client_survives_selection_change(installation: &str) {
    let fixture = Fixture::new();
    let home = fixture.root.join("home");
    let installed = home
        .join(".local/share/kelpmux")
        .join(installation)
        .join("kelpmux");
    fs::create_dir_all(installed.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_kelpmux"), &installed).unwrap();
    fs::set_permissions(&installed, fs::Permissions::from_mode(0o700)).unwrap();
    let state = fixture.root.join("state");
    lifecycle::private_state(&state).unwrap();
    let socket = state.join("control.sock");
    let mut server = Server(
        Command::new(&installed)
            .args(["server", "--socket"])
            .arg(&socket)
            .arg("--state-dir")
            .arg(&state)
            .args(["--shell", "/bin/sh", "--cwd"])
            .arg(&fixture.root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let before = loop {
        if let Ok(ServerMessage::State { panes, .. }) = lifecycle::inspect(&socket) {
            break panes;
        }
        assert!(Instant::now() < deadline, "server did not start");
        assert!(server.0.try_wait().unwrap().is_none(), "server exited");
        thread::sleep(Duration::from_millis(10));
    };
    for verb in ["attach", "ssh-entry"] {
        let output = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
            .args([verb, "--socket"])
            .arg(&socket)
            .arg("--state-dir")
            .arg(&state)
            .env("HOME", &home)
            .env_remove("XDG_DATA_HOME")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("using retained client"), "{error}");
        assert!(
            error.contains("requires stdin and stdout to be TTYs"),
            "{error}"
        );
    }
    // A populated main workspace must not supply identity/config for a custom socket.
    let xdg_state = fixture.root.join("xdg-state");
    let main_state = xdg_state.join("kelpmux/workspaces/main");
    lifecycle::private_state(&main_state).unwrap();
    upgrade::save(&main_state).unwrap();
    let identity_path = main_state.join("server.identity.json");
    let mut unrelated: Identity =
        serde_json::from_slice(&fs::read(&identity_path).unwrap()).unwrap();
    unrelated.birth += 1;
    fs::write(&identity_path, serde_json::to_vec(&unrelated).unwrap()).unwrap();
    fs::write(
        main_state.join("workspace.json"),
        b"invalid unrelated configuration",
    )
    .unwrap();
    let custom_attach = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
        .args(["attach", "--socket"])
        .arg(&socket)
        .env("HOME", &home)
        .env("XDG_STATE_HOME", &xdg_state)
        .env_remove("XDG_DATA_HOME")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&custom_attach.stderr)
            .contains("requires stdin and stdout to be TTYs")
    );
    let status = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
        .args(["status", "--socket"])
        .arg(&socket)
        .env("XDG_STATE_HOME", &xdg_state)
        .output()
        .unwrap();
    assert!(status.status.success());
    assert!(String::from_utf8_lossy(&status.stderr).contains(&format!("protocol {VERSION}")));
    assert!(matches!(
        serde_json::from_slice::<ServerMessage>(&status.stdout).unwrap(),
        ServerMessage::State { .. }
    ));
    let explicit_workspace = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
        .args(["status", "--socket"])
        .arg(&socket)
        .args(["--workspace", "main"])
        .env("XDG_STATE_HOME", &xdg_state)
        .output()
        .unwrap();
    assert!(
        !explicit_workspace.status.success(),
        "explicit workspace must remain authoritative"
    );
    // Authenticate legacy servers through their live socket and retained inode.
    fs::remove_file(state.join("server.identity.json")).unwrap();
    let legacy = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
        .args(["attach", "--socket"])
        .arg(&socket)
        .arg("--state-dir")
        .arg(&state)
        .env("HOME", &home)
        .env_remove("XDG_DATA_HOME")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&legacy.stderr).contains("unknown (legacy server)"));
    assert!(server.0.try_wait().unwrap().is_none());
    let ServerMessage::State { panes: after, .. } = lifecycle::inspect(&socket).unwrap() else {
        panic!()
    };
    assert_eq!(before[0].pid, after[0].pid);
    fs::remove_file(&installed).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kelpmux"))
        .args(["attach", "--socket"])
        .arg(&socket)
        .arg("--state-dir")
        .arg(&state)
        .env("HOME", &home)
        .env_remove("XDG_DATA_HOME")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(server.0.try_wait().unwrap().is_none());
}
#[test]
fn stale_pid_without_live_socket_never_selects_an_executable() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join("server.pid"),
        std::process::id().to_string(),
    )
    .unwrap();
    assert!(
        upgrade::attach_matching(&fixture.root.join("absent.sock"), &fixture.root, false).is_err()
    );
}
