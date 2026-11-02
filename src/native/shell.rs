//! Workspace-local Claude hyperlink capability; Codex launches unchanged.
use crate::{Result, journal};
use std::{
    fs::{self, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Command,
};
/// Workspace-local startup files preserve the user's configuration and install
/// an interactive function. No global shell or Codex configuration is modified.
pub fn integrate_shell(command: &mut Command, state: &Path, shell: &Path) -> Result<()> {
    let name = shell.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if !matches!(name, "bash" | "zsh") {
        return Ok(());
    }
    let dir = state.join("shell-integration");
    journal::private_dir(&dir)?;
    let write = |name: &str, text: &str| -> Result<()> {
        use std::io::Write;
        let temp = dir.join(format!("{name}-{}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)?;
        file.write_all(text.as_bytes())?;
        fs::rename(temp, dir.join(name))?;
        Ok(())
    };
    if name == "bash" {
        write(
            "bashrc",
            r#"if [ -r "$HOME/.bashrc" ]; then . "$HOME/.bashrc"; fi
if ! declare -F claude >/dev/null; then
function claude {
  if [ -t 1 ]; then FORCE_HYPERLINK="${FORCE_HYPERLINK-1}" command claude "$@";
  else command claude "$@"; fi
}
fi
"#,
        )?;
        command.arg("--rcfile").arg(dir.join("bashrc"));
    } else {
        let original = std::env::var_os("ZDOTDIR").map(PathBuf::from);
        command
            .env(
                "KELPMUX_USER_ZDOTDIR",
                original.as_deref().unwrap_or_else(|| Path::new("")),
            )
            .env("KELPMUX_INTEGRATION_DIR", &dir)
            .env("ZDOTDIR", &dir);
        write(
            ".zshenv",
            r#"if [[ -n $KELPMUX_USER_ZDOTDIR ]]; then export ZDOTDIR=$KELPMUX_USER_ZDOTDIR; else unset ZDOTDIR; fi
if [[ -r ${ZDOTDIR:-$HOME}/.zshenv ]]; then source "${ZDOTDIR:-$HOME}/.zshenv"; fi
export KELPMUX_USER_ZDOTDIR=${ZDOTDIR-}
export ZDOTDIR=$KELPMUX_INTEGRATION_DIR
"#,
        )?;
        write(
            ".zshrc",
            r#"if [[ -n $KELPMUX_USER_ZDOTDIR ]]; then export ZDOTDIR=$KELPMUX_USER_ZDOTDIR; else unset ZDOTDIR; fi
if [[ -r ${ZDOTDIR:-$HOME}/.zshrc ]]; then source "${ZDOTDIR:-$HOME}/.zshrc"; fi
if (( ! ${+functions[claude]} )); then
function claude {
  if [[ -t 1 ]]; then FORCE_HYPERLINK="${FORCE_HYPERLINK-1}" command claude "$@";
  else command claude "$@"; fi
}
fi
"#,
        )?;
    }
    Ok(())
}
