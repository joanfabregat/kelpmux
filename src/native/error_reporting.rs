//! Optional, off-by-default error reporting to a GlitchTip (Sentry-compatible)
//! install. Errors and panics only: no tracing, sessions, breadcrumbs or user
//! data. Without a DSN nothing is initialised and nothing is sent.
//!
//! What is here today:
//! - [`RELEASE`], the release name embedded at build time (see `build.rs`),
//!   identical to the one the web client reports and uploads source maps for.
//! - [`Dsn`] and [`Settings`]: validated settings, from the web relay's
//!   `config.json` (`glitchtip_dsn`, `error_environment`) or, for the TUI and
//!   `web-bridge`, from `KELPMUX_GLITCHTIP_DSN` and `KELPMUX_ERROR_ENVIRONMENT`.
//!
//! The relay already hands its DSN to the browser, which reports with the
//! Sentry browser SDK. The Rust reporter for the relay, bridge and TUI
//! (component tags `relay`, `bridge`, `tui`) is pending a choice of
//! implementation; it will start from [`Settings`] and tag events with
//! [`Component::tag`]. Nothing in this module sends anything.
use crate::Result;

/// `kelpmux@<version>+<12-digit git sha>`, or `kelpmux@<version>` without Git
/// information, unless the build set `KELPMUX_RELEASE`.
pub const RELEASE: &str = env!("KELPMUX_RELEASE");

/// The `component` tag of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    Web,
    Relay,
    Bridge,
    Tui,
}
impl Component {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Relay => "relay",
            Self::Bridge => "bridge",
            Self::Tui => "tui",
        }
    }
}

/// A public Sentry/GlitchTip DSN: `https://PUBLIC_KEY@HOST[:PORT][/PATH]/PROJECT_ID`.
/// It is handed to browsers, so a DSN with a secret key is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dsn {
    value: String,
    origin: String,
}
impl Dsn {
    pub fn parse(value: &str) -> Result<Self> {
        const INVALID: &str = "invalid GlitchTip DSN: expected https://KEY@HOST/PROJECT_ID";
        if value.len() > 512 || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(INVALID.into());
        }
        let rest = value.strip_prefix("https://").ok_or(INVALID)?;
        if rest.contains(['?', '#']) {
            return Err(INVALID.into());
        }
        let (authority, path) = rest.split_once('/').ok_or(INVALID)?;
        let (key, host) = authority.split_once('@').ok_or(INVALID)?;
        if key.contains(':') {
            return Err("GlitchTip DSN must not contain a secret key".into());
        }
        if key.is_empty() || !key.bytes().all(|c| c.is_ascii_alphanumeric()) {
            return Err(INVALID.into());
        }
        // `[IPv6]` or a name, then an optional `:PORT`.
        let split = if host.starts_with('[') {
            host.find(']').map_or(host.len(), |end| end + 1)
        } else {
            host.find(':').unwrap_or(host.len())
        };
        let (name, port) = match &host[split..] {
            "" => (host, None),
            rest => (&host[..split], Some(rest.strip_prefix(':').unwrap_or(rest))),
        };
        let name_ok = if let Some(ip) = name.strip_prefix('[').and_then(|n| n.strip_suffix(']')) {
            ip.parse::<std::net::Ipv6Addr>().is_ok()
        } else {
            !name.is_empty()
                && name.len() <= 253
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
                && !name.starts_with(['.', '-'])
        };
        let port_ok = port
            .is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.parse::<u16>().is_ok_and(|n| n > 0));
        let project = path.rsplit('/').next().unwrap_or("");
        let prefix_ok = path
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/-_.".contains(&c))
            && !path.split('/').any(|s| s == "." || s == "..");
        if !name_ok
            || !port_ok
            || !prefix_ok
            || project.is_empty()
            || !project.bytes().all(|c| c.is_ascii_digit())
        {
            return Err(INVALID.into());
        }
        Ok(Self {
            value: value.to_owned(),
            origin: format!("https://{host}"),
        })
    }
    pub fn as_str(&self) -> &str {
        &self.value
    }
    /// `https://HOST[:PORT]`, the only origin events are sent to.
    pub fn origin(&self) -> &str {
        &self.origin
    }
}

/// Checks an environment name: 1–64 letters, digits, `.`, `_` or `-`.
pub fn validate_environment(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        return Err("invalid error environment: use 1-64 letters, digits, '.', '_' or '-'".into());
    }
    Ok(())
}

/// Validated reporting settings. `None` everywhere means reporting is off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub dsn: Dsn,
    pub environment: Option<String>,
}
impl Settings {
    /// Settings from an optional DSN and environment. No DSN means off; an
    /// environment without a DSN is checked but has no effect.
    pub fn new(dsn: Option<&str>, environment: Option<&str>) -> Result<Option<Self>> {
        if let Some(environment) = environment {
            validate_environment(environment)?;
        }
        let Some(dsn) = dsn else {
            return Ok(None);
        };
        Ok(Some(Self {
            dsn: Dsn::parse(dsn)?,
            environment: environment.map(str::to_owned),
        }))
    }
    /// Settings for the TUI and `web-bridge`: `KELPMUX_GLITCHTIP_DSN` and
    /// `KELPMUX_ERROR_ENVIRONMENT`, empty values counting as unset.
    pub fn from_env() -> Result<Option<Self>> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }
    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<Self>> {
        let get = |name| lookup(name).filter(|value| !value.is_empty());
        let dsn = get("KELPMUX_GLITCHTIP_DSN");
        let environment = get("KELPMUX_ERROR_ENVIRONMENT");
        Self::new(dsn.as_deref(), environment.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_names_this_build() {
        let rest = RELEASE.strip_prefix("kelpmux@").unwrap_or(RELEASE);
        assert!(!RELEASE.is_empty() && RELEASE.len() <= 200);
        if std::env::var_os("KELPMUX_RELEASE").is_none() {
            assert!(rest.starts_with(env!("CARGO_PKG_VERSION")), "{RELEASE}");
        }
    }
    #[test]
    fn accepts_public_dsns_and_derives_their_origin() {
        for (dsn, origin) in [
            (
                "https://abc123@glitchtip.example.com/7",
                "https://glitchtip.example.com",
            ),
            (
                "https://abc@errors.example.com:8443/sub/path/42",
                "https://errors.example.com:8443",
            ),
            ("https://abc@[::1]:9000/1", "https://[::1]:9000"),
        ] {
            let parsed = Dsn::parse(dsn).unwrap();
            assert_eq!(parsed.as_str(), dsn);
            assert_eq!(parsed.origin(), origin);
        }
    }
    #[test]
    fn rejects_invalid_or_secret_dsns() {
        for dsn in [
            "",
            "http://abc@glitchtip.example.com/7",
            "https://glitchtip.example.com/7",
            "https://abc:secret@glitchtip.example.com/7",
            "https://abc@glitchtip.example.com/",
            "https://abc@glitchtip.example.com/seven",
            "https://abc@glitchtip.example.com",
            "https://abc@glitchtip.example.com/7?x=1",
            "https://abc@glitchtip.example.com/7#x",
            "https://abc@glitchtip example.com/7",
            "https://abc@glitchtip.example.com:0/7",
            "https://abc@glitchtip.example.com:99999/7",
            "https://abc@/7",
            "https://a-b@glitchtip.example.com/7",
            "https://abc@glitchtip.example.com/../7",
            "https://abc@glitchtip.example.com/7\n",
            "https://abc@'unsafe-inline'/7",
        ] {
            assert!(Dsn::parse(dsn).is_err(), "{dsn:?}");
        }
        assert!(Dsn::parse(&format!("https://k@{}.com/1", "a".repeat(600))).is_err());
    }
    #[test]
    fn settings_are_off_without_a_dsn_and_check_the_environment() {
        assert_eq!(Settings::new(None, None).unwrap(), None);
        assert_eq!(Settings::new(None, Some("production")).unwrap(), None);
        assert!(Settings::new(None, Some("bad env")).is_err());
        let settings = Settings::new(Some("https://k@g.example.com/1"), Some("preprod"))
            .unwrap()
            .unwrap();
        assert_eq!(settings.environment.as_deref(), Some("preprod"));
        assert!(Settings::new(Some("https://k@g.example.com/1"), Some("")).is_err());
        assert!(Settings::new(Some("https://k@g.example.com/1"), Some(&"x".repeat(65))).is_err());
        assert!(Settings::new(Some("nope"), None).is_err());
    }
    #[test]
    fn environment_variables_configure_the_tui_and_bridge() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_string())
            }
        };
        assert_eq!(Settings::from_lookup(env(&[])).unwrap(), None);
        assert_eq!(
            Settings::from_lookup(env(&[("KELPMUX_GLITCHTIP_DSN", "")])).unwrap(),
            None
        );
        let settings = Settings::from_lookup(env(&[
            ("KELPMUX_GLITCHTIP_DSN", "https://k@g.example.com/3"),
            ("KELPMUX_ERROR_ENVIRONMENT", "dev"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(settings.dsn.origin(), "https://g.example.com");
        assert_eq!(settings.environment.as_deref(), Some("dev"));
        assert!(Settings::from_lookup(env(&[("KELPMUX_GLITCHTIP_DSN", "http://k@g/1")])).is_err());
        assert_eq!(Component::Tui.tag(), "tui");
    }
}
