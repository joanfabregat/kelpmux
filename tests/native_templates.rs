use kelpmux::native::templates::{self, Template, TemplatePane};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "km-template-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    fn template(&self) -> Template {
        Template {
            grid: (2, 1),
            panes: vec![TemplatePane {
                name: "code".into(),
                cwd: self.0.clone(),
                shell: Some(PathBuf::from("/bin/sh")),
            }],
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn template_loading_validates_capacity_names_paths_and_schema() {
    let fixture = Fixture::new();
    let template = fixture.template();
    let path = fixture.0.join("project.json");
    fs::write(&path, serde_json::to_vec(&template).unwrap()).unwrap();
    assert_eq!(Template::load(&path).unwrap(), template);
    for variant in 0..6 {
        let mut changed = template.clone();
        match variant {
            0 => changed.grid = (4, 4),
            1 => {
                changed.grid = (1, 1);
                changed.panes.push(changed.panes[0].clone());
            }
            2 => changed.panes[0].name = "bad\x1b".into(),
            3 => changed.panes[0].cwd = PathBuf::from("relative"),
            4 => changed.panes[0].cwd = PathBuf::from("/does/not/exist"),
            5 => changed.panes[0].shell = Some(PathBuf::from("/does/not/exist")),
            _ => unreachable!(),
        };
        assert!(changed.validate().is_err());
    }
    fs::write(
        &path,
        r#"{"grid":[1,1],"panes":[],"command":"codex --resume guessed"}"#,
    )
    .unwrap();
    assert!(Template::load(&path).is_err());
    fs::write(&path, vec![b' '; 65537]).unwrap();
    assert!(Template::load(&path).is_err());
}
#[test]
fn stored_templates_are_private_sorted_and_never_follow_symlinks() {
    let fixture = Fixture::new();
    assert!(templates::list(&fixture.0).unwrap().is_empty());
    let dir = fixture.0.join("templates");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    for name in ["zeta", "alpha"] {
        fs::write(
            dir.join(format!("{name}.json")),
            serde_json::to_vec(&fixture.template()).unwrap(),
        )
        .unwrap();
    }
    let listed = templates::list(&fixture.0).unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "zeta"]
    );
    std::os::unix::fs::symlink(dir.join("alpha.json"), dir.join("link.json")).unwrap();
    assert!(templates::list(&fixture.0).is_err());
    fs::remove_file(dir.join("link.json")).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(templates::list(&fixture.0).is_err());
}
#[test]
fn fifo_and_parent_directory_symlinks_fail_without_blocking() {
    let fixture = Fixture::new();
    let fifo = fixture.0.join("fifo.json");
    let raw = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
    assert!(Template::load(&fifo).is_err());
    let dir = fixture.0.join("real");
    fs::create_dir(&dir).unwrap();
    let path = dir.join("file.json");
    fs::write(&path, serde_json::to_vec(&fixture.template()).unwrap()).unwrap();
    let link = fixture.0.join("link");
    std::os::unix::fs::symlink(&dir, &link).unwrap();
    assert!(Template::load(&link.join("file.json")).is_err());
}
