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

/// Session markers that an agent, a multiplexer or a parent KelpMux pane
/// leaves in the environment of the process that started the server. Panes
/// are new top-level terminals, so they must not inherit them: Claude Code
/// would otherwise treat a fresh session as a child of the one that started
/// the server (and turn off transcript saving), and agents would attach to
/// the wrong tmux pane or messaging socket. User configuration such as API
/// keys, `CLAUDE_CONFIG_DIR` and `CODEX_HOME` is kept.
pub fn inherited_session_marker(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    matches!(
        name,
        "CLAUDECODE"
            | "CLAUDE_PID"
            | "CLAUDE_EFFORT"
            | "CODEX_SESSION_ID"
            | "CODEX_THREAD_ID"
            | "CODEX_TOOL_CALL_ID"
            | "CODEX_CI"
            | "TMUX"
            | "TMUX_PANE"
            | "STY"
    ) || name.starts_with("CLAUDE_CODE_")
}

/// Removes every inherited session marker from a pane's command.
pub fn remove_session_markers(command: &mut Command) {
    for (name, _) in std::env::vars_os() {
        if inherited_session_marker(&name) {
            command.env_remove(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::inherited_session_marker;
    use std::ffi::OsStr;

    #[test]
    fn agent_and_multiplexer_session_markers_are_not_inherited() {
        for marker in [
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_MESSAGING_SOCKET",
            "CLAUDE_PID",
            "CLAUDE_EFFORT",
            "CODEX_SESSION_ID",
            "CODEX_THREAD_ID",
            "CODEX_TOOL_CALL_ID",
            "CODEX_CI",
            "TMUX",
            "TMUX_PANE",
        ] {
            assert!(inherited_session_marker(OsStr::new(marker)), "{marker}");
        }
        for kept in [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "CLAUDE_CONFIG_DIR",
            "CLAUDE_CODE",
            "CODEX_HOME",
            "CODEX_API_KEY",
            "TMUXINATOR_CONFIG",
            "PATH",
            "HOME",
        ] {
            assert!(!inherited_session_marker(OsStr::new(kept)), "{kept}");
        }
    }
}
