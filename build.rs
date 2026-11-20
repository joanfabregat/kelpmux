//! Embeds the browser client's built assets (`web/dist`) in the `kelpmux`
//! executable. A checkout without `web/dist` still builds and runs the TUI;
//! `kelpmux web-relay` then refuses to start with an explicit error.
use std::{
    env, fs,
    path::{Path, PathBuf},
};
fn visit(base: &Path, path: &Path, entries: &mut Vec<(String, PathBuf)>) {
    let Ok(directory) = fs::read_dir(path) else {
        return;
    };
    for entry in directory.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit(base, &path, entries);
        } else if path.is_file() {
            entries.push((
                format!("/{}", path.strip_prefix(base).unwrap().to_string_lossy()),
                path,
            ));
        }
    }
}
fn main() {
    println!("cargo:rerun-if-changed=web/dist");
    let base = Path::new("web/dist");
    let mut entries = Vec::new();
    visit(base, base, &mut entries);
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut source = String::from("pub static ASSETS: &[(&str, &[u8])] = &[\n");
    for (url, path) in entries {
        source.push_str(&format!(
            "({url:?}, include_bytes!({:?})),\n",
            fs::canonicalize(path).unwrap()
        ));
    }
    source.push_str("];\n");
    fs::write(
        PathBuf::from(env::var("OUT_DIR").unwrap()).join("web_assets.rs"),
        source,
    )
    .unwrap();
}
