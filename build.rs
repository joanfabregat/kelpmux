//! Embeds the browser client's built assets (`web/dist`) in the `kelpmux`
//! executable. A checkout without `web/dist` still builds and runs the TUI;
//! `kelpmux web-relay` then refuses to start with an explicit error.
//!
//! Also sets `KELPMUX_RELEASE`, the error-reporting release name, computed
//! exactly as `web/scripts/release.mjs` computes it for the web build.
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};
fn visit(base: &Path, path: &Path, entries: &mut Vec<(String, PathBuf)>) {
    let Ok(directory) = fs::read_dir(path) else {
        return;
    };
    for entry in directory.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit(base, &path, entries);
        } else if path.is_file() && path.extension().is_none_or(|e| e != "map") {
            // Source maps are for deploy-time upload only, never served.
            entries.push((
                format!("/{}", path.strip_prefix(base).unwrap().to_string_lossy()),
                path,
            ));
        }
    }
}
fn valid_release(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._@+-".contains(&c))
}
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(["-c", "safe.directory=*"])
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_owned())
}
/// `KELPMUX_RELEASE` if set, else `kelpmux@VERSION+SHA12` with the commit from
/// `KELPMUX_GIT_SHA` or Git, else `kelpmux@VERSION`.
fn release() -> String {
    println!("cargo:rerun-if-env-changed=KELPMUX_RELEASE");
    println!("cargo:rerun-if-env-changed=KELPMUX_GIT_SHA");
    if let Some(name) = env::var("KELPMUX_RELEASE").ok().filter(|n| !n.is_empty()) {
        assert!(valid_release(&name), "invalid KELPMUX_RELEASE {name:?}");
        return name;
    }
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let sha = match env::var("KELPMUX_GIT_SHA").ok().filter(|s| !s.is_empty()) {
        Some(sha) => Some(sha),
        None => {
            // Rebuild when HEAD moves (commit, checkout, reset).
            if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
                for file in ["HEAD", "logs/HEAD"] {
                    let path = Path::new(&dir).join(file);
                    if path.exists() {
                        println!("cargo:rerun-if-changed={}", path.display());
                    }
                }
            }
            git(&["rev-parse", "HEAD"])
        }
    };
    match sha {
        Some(sha) => {
            let sha = sha.to_ascii_lowercase();
            assert!(
                sha.len() >= 12 && sha.bytes().all(|c| c.is_ascii_hexdigit()),
                "invalid KELPMUX_GIT_SHA {sha:?}"
            );
            format!("kelpmux@{version}+{}", &sha[..12])
        }
        None => format!("kelpmux@{version}"),
    }
}
fn main() {
    println!("cargo:rerun-if-changed=web/dist");
    let release = release();
    // The web build records the release its source maps belong to; a
    // different value here means the two builds would report different
    // releases, and uploaded maps would not match browser events.
    if let Ok(web) = fs::read_to_string("web/sourcemaps/RELEASE") {
        println!("cargo:rerun-if-changed=web/sourcemaps/RELEASE");
        if web.trim() != release {
            println!(
                "cargo:warning=web client built for release {} but kelpmux for {release}; set KELPMUX_RELEASE for both builds",
                web.trim()
            );
        }
    }
    println!("cargo:rustc-env=KELPMUX_RELEASE={release}");
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
