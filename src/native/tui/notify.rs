//! Desktop notifications through the terminal application the TUI runs in:
//! `OSC 9` (iTerm2, kitty, WezTerm, Ghostty) and `OSC 777 ; notify`
//! (urxvt, foot, WezTerm, Ghostty). The TUI writes them on the controlling
//! client's terminal; the terminal application decides how to show them.
use crate::native::ui::DesktopNotifications;

/// Longest title or body written, in characters.
pub const MAX_TEXT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Osc9,
    Osc777,
}

/// What the attaching client's environment says about its terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Terminal {
    /// The protocol `Auto` uses, when the terminal is recognised.
    pub detected: Option<Protocol>,
    /// Running inside tmux: sequences need tmux's passthrough.
    pub tmux: bool,
}

impl Terminal {
    pub fn from_env() -> Self {
        Self::detect(|name| std::env::var(name).ok())
    }

    /// Recognise the terminal from `TERM_PROGRAM`, `LC_TERMINAL`, `TERM` and
    /// the variables kitty, WezTerm and Ghostty set. Over SSH usually only
    /// `TERM` (and `LC_*`) arrive, which still identifies kitty, Ghostty,
    /// foot, urxvt and iTerm2.
    pub fn detect(var: impl Fn(&str) -> Option<String>) -> Self {
        let set = |name: &str| var(name).is_some_and(|value| !value.is_empty());
        let term = var("TERM").unwrap_or_default();
        let program = var("TERM_PROGRAM").unwrap_or_default();
        let detected = if matches!(program.as_str(), "iTerm.app" | "WezTerm" | "ghostty")
            || var("LC_TERMINAL").is_some_and(|value| value == "iTerm2")
            || set("KITTY_WINDOW_ID")
            || set("WEZTERM_PANE")
            || set("WEZTERM_EXECUTABLE")
            || set("GHOSTTY_RESOURCES_DIR")
            || set("GHOSTTY_BIN_DIR")
            || matches!(term.as_str(), "xterm-kitty" | "xterm-ghostty" | "wezterm")
        {
            Some(Protocol::Osc9)
        } else if term.starts_with("foot") || term.starts_with("rxvt-unicode") {
            Some(Protocol::Osc777)
        } else {
            None
        };
        Self {
            detected,
            tmux: set("TMUX"),
        }
    }

    /// The protocol a setting resolves to here. `Auto` stays off inside
    /// tmux: the outer terminal is unknown there, and the variables tmux
    /// inherited may name an earlier one.
    pub fn protocol(&self, setting: DesktopNotifications) -> Option<Protocol> {
        match setting {
            DesktopNotifications::Auto if self.tmux => None,
            DesktopNotifications::Auto => self.detected,
            DesktopNotifications::Osc9 => Some(Protocol::Osc9),
            DesktopNotifications::Osc777 => Some(Protocol::Osc777),
            DesktopNotifications::Off => None,
        }
    }

    /// The bytes to write for a notification, or `None` when off. Inside
    /// tmux an explicit choice is wrapped in tmux's passthrough, which needs
    /// `allow-passthrough on`.
    pub fn sequence(
        &self,
        setting: DesktopNotifications,
        title: &str,
        body: &str,
    ) -> Option<String> {
        let sequence = build(self.protocol(setting)?, title, body);
        Some(if self.tmux {
            passthrough(&sequence)
        } else {
            sequence
        })
    }
}

/// Text safe inside an OSC string: no control characters (ESC and BEL
/// included), no `;` where it separates fields, at most `MAX_TEXT`
/// characters.
pub fn sanitize(text: &str, separators: bool) -> String {
    text.chars()
        .filter(|c| !c.is_control() && !(separators && *c == ';'))
        .take(MAX_TEXT)
        .collect::<String>()
        .trim()
        .to_owned()
}

pub fn build(protocol: Protocol, title: &str, body: &str) -> String {
    match protocol {
        // One message: the title leads it, which also keeps a leading
        // number from reading as a ConEmu-style `OSC 9 ; n` subcommand.
        Protocol::Osc9 => format!(
            "\x1b]9;{}\x07",
            sanitize(&format!("{}: {}", sanitize(title, false), body), false)
        ),
        Protocol::Osc777 => format!(
            "\x1b]777;notify;{};{}\x07",
            sanitize(title, true),
            sanitize(body, true)
        ),
    }
}

/// Wrap a sequence in tmux's DCS passthrough, doubling its escapes.
pub fn passthrough(sequence: &str) -> String {
    format!("\x1bPtmux;{}\x1b\\", sequence.replace('\x1b', "\x1b\x1b"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn env(pairs: &[(&str, &str)]) -> Terminal {
        let vars: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Terminal::detect(|name| vars.get(name).cloned())
    }

    #[test]
    fn builds_osc_9_and_777() {
        assert_eq!(
            build(Protocol::Osc9, "KelpMux", "2 Finished · build"),
            "\x1b]9;KelpMux: 2 Finished · build\x07"
        );
        assert_eq!(
            build(Protocol::Osc777, "KelpMux", "2 Finished · build"),
            "\x1b]777;notify;KelpMux;2 Finished · build\x07"
        );
    }

    #[test]
    fn sanitizes_controls_separators_and_length() {
        let hostile = "a\x1b]0;x\x07b\x1b\\c;d\u{9c}e\n";
        assert_eq!(sanitize(hostile, false), "a]0;xb\\c;de");
        assert_eq!(sanitize(hostile, true), "a]0xb\\cde");
        let sequence = build(Protocol::Osc777, "T;i\x07t", hostile);
        assert_eq!(sequence, "\x1b]777;notify;Tit;a]0xb\\cde\x07");
        // Exactly one ESC and one BEL: nothing can end the string early.
        for sequence in [sequence, build(Protocol::Osc9, "T", hostile)] {
            assert_eq!(sequence.matches('\x1b').count(), 1);
            assert_eq!(sequence.matches('\x07').count(), 1);
        }
        let long = "é".repeat(500);
        assert_eq!(sanitize(&long, false).chars().count(), MAX_TEXT);
        let message = build(Protocol::Osc9, "KelpMux", &long);
        assert_eq!(message.chars().count(), "\x1b]9;\x07".len() + MAX_TEXT);
    }

    #[test]
    fn auto_detects_known_terminals() {
        for (pairs, expected) in [
            (vec![("TERM_PROGRAM", "iTerm.app")], Some(Protocol::Osc9)),
            (vec![("LC_TERMINAL", "iTerm2")], Some(Protocol::Osc9)),
            (vec![("TERM_PROGRAM", "WezTerm")], Some(Protocol::Osc9)),
            (vec![("WEZTERM_PANE", "3")], Some(Protocol::Osc9)),
            (vec![("TERM_PROGRAM", "ghostty")], Some(Protocol::Osc9)),
            (vec![("TERM", "xterm-ghostty")], Some(Protocol::Osc9)),
            (vec![("GHOSTTY_RESOURCES_DIR", "/x")], Some(Protocol::Osc9)),
            (vec![("TERM", "xterm-kitty")], Some(Protocol::Osc9)),
            (vec![("KITTY_WINDOW_ID", "1")], Some(Protocol::Osc9)),
            (vec![("TERM", "foot")], Some(Protocol::Osc777)),
            (vec![("TERM", "foot-extra")], Some(Protocol::Osc777)),
            (
                vec![("TERM", "rxvt-unicode-256color")],
                Some(Protocol::Osc777),
            ),
            (vec![("TERM", "xterm-256color")], None),
            (vec![("TERM_PROGRAM", "Apple_Terminal")], None),
            (vec![("KITTY_WINDOW_ID", "")], None),
            (vec![], None),
        ] {
            let terminal = env(&pairs);
            assert_eq!(terminal.detected, expected, "{pairs:?}");
            assert!(!terminal.tmux);
            assert_eq!(terminal.protocol(DesktopNotifications::Auto), expected);
            assert_eq!(terminal.protocol(DesktopNotifications::Off), None);
        }
        let unknown = env(&[("TERM", "xterm-256color")]);
        assert_eq!(unknown.sequence(DesktopNotifications::Auto, "T", "b"), None);
        assert_eq!(
            unknown.protocol(DesktopNotifications::Osc777),
            Some(Protocol::Osc777)
        );
    }

    #[test]
    fn tmux_turns_auto_off_and_wraps_explicit_choices() {
        let terminal = env(&[
            ("TMUX", "/tmp/tmux-1000/default,1,0"),
            ("TERM_PROGRAM", "tmux"),
            ("KITTY_WINDOW_ID", "1"),
        ]);
        assert!(terminal.tmux);
        assert_eq!(terminal.protocol(DesktopNotifications::Auto), None);
        assert_eq!(
            terminal
                .sequence(DesktopNotifications::Osc9, "KelpMux", "1 Bell · sh")
                .unwrap(),
            "\x1bPtmux;\x1b\x1b]9;KelpMux: 1 Bell · sh\x07\x1b\\"
        );
    }
}
