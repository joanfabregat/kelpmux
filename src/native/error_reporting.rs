//! Optional, off-by-default error reporting to a GlitchTip (Sentry-compatible)
//! install. Errors and panics only: no tracing, sessions, breadcrumbs or user
//! data. Without a DSN nothing is initialised and nothing is sent.
//!
//! - [`RELEASE`], the release name embedded at build time (see `build.rs`),
//!   identical to the one the web client reports and uploads source maps for.
//! - [`Dsn`] and [`Settings`]: validated settings, from the web relay's
//!   `config.json` (`glitchtip_dsn`, `error_environment`) or, for the TUI and
//!   `web-bridge`, from `KELPMUX_GLITCHTIP_DSN` and `KELPMUX_ERROR_ENVIRONMENT`.
//! - [`init`]: the Rust reporter for the relay, bridge and TUI (component tags
//!   `relay`, `bridge`, `tui`), built on the official `sentry` crate with its
//!   in-process `ureq` + `rustls` transport. It reports panics, plus the few
//!   errors a component passes to [`report_error`]. The browser reports with
//!   the Sentry browser SDK from the relay's `/connection.json`.
//!
//! Privacy, as in the browser: no breadcrumbs, user, request, extra data,
//! server name or host name; `before_send` ([`scrub`]) enforces it on every
//! event and keeps only the `component` tag and the OS and architecture.
use crate::Result;
use sentry::protocol::{Context, DeviceContext, Event, Level, OsContext};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Once},
    time::{Duration, Instant},
};

/// The longest a flush blocks: on a normal exit, after a TUI panic once the
/// terminal is restored, and when the relay or bridge shuts down.
pub const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);
/// [`report_error`] sends at most one event per kind in this interval.
pub const ERROR_INTERVAL: Duration = Duration::from_secs(60);

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

/// Keeps reporting on; dropping it flushes pending events (at most
/// [`FLUSH_TIMEOUT`]) and turns reporting off.
#[must_use = "reporting stops when the guard is dropped"]
pub struct Guard {
    client: Arc<sentry::Client>,
}
impl Guard {
    /// Waits at most [`FLUSH_TIMEOUT`] for pending events to be sent.
    pub fn flush(&self) -> bool {
        self.client.flush(Some(FLUSH_TIMEOUT))
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.client.close(Some(FLUSH_TIMEOUT));
        sentry::Hub::main().bind_client(None);
    }
}

/// Starts reporting for `component` when `settings` has a DSN. Without one
/// nothing happens: no SDK, no thread, no network.
pub fn init(settings: Option<Settings>, component: Component) -> Option<Guard> {
    init_with_transport(settings, component, Arc::new(UreqTransport))
}

/// [`init`] from `KELPMUX_GLITCHTIP_DSN` and `KELPMUX_ERROR_ENVIRONMENT`, for
/// the TUI and `web-bridge`. Invalid values turn reporting off with a warning
/// on stderr (before any terminal mode change) rather than refusing to run.
pub fn init_from_env(component: Component) -> Option<Guard> {
    match Settings::from_env() {
        Ok(settings) => init(settings, component),
        Err(error) => {
            eprintln!("kelpmux: error reporting disabled: {error}");
            None
        }
    }
}

/// [`init`] with another transport; tests record events with it.
#[doc(hidden)]
pub fn init_with_transport(
    settings: Option<Settings>,
    component: Component,
    transport: Arc<dyn sentry::TransportFactory>,
) -> Option<Guard> {
    let options = client_options(&settings?, component, transport)?;
    // Not `sentry::init`: its defaults read SENTRY_*, SSL_VERIFY and proxy
    // variables. The options here are complete.
    let client = Arc::new(sentry::Client::from(options));
    sentry::Hub::main().bind_client(Some(client.clone()));
    install_panic_hook();
    Some(Guard { client })
}

fn client_options(
    settings: &Settings,
    component: Component,
    transport: Arc<dyn sentry::TransportFactory>,
) -> Option<sentry::ClientOptions> {
    // `ClientOptions` is non-exhaustive: start from the defaults and set
    // every option that matters here explicitly.
    let mut options = sentry::ClientOptions::default();
    options.dsn = Some(settings.dsn.as_str().parse().ok()?);
    options.release = Some(RELEASE.into());
    options.environment = settings.environment.clone().map(Into::into);
    options.server_name = None;
    options.max_breadcrumbs = 0;
    options.attach_stacktrace = false;
    options.send_default_pii = false;
    options.accept_invalid_certs = false;
    options.auto_session_tracking = false;
    options.debug = false;
    // Only stack-trace processing (in-app frames); panics are captured by
    // this module's own hook, see `install_panic_hook`.
    options.default_integrations = false;
    options.integrations = vec![Arc::new(
        sentry::integrations::backtrace::ProcessStacktraceIntegration,
    )];
    options.in_app_include = vec!["kelpmux", "kelpserver", "kelpclient"];
    options.before_breadcrumb = Some(Arc::new(|_| None));
    options.before_send = Some(Arc::new(move |event| scrub(event, component)));
    options.shutdown_timeout = FLUSH_TIMEOUT;
    options.transport = Some(transport);
    Some(options)
}

/// `before_send`: drops anything that could identify the user, host or
/// session, whatever produced the event, and sets the `component` tag.
pub fn scrub(mut event: Event<'static>, component: Component) -> Option<Event<'static>> {
    event.user = None;
    event.request = None;
    event.server_name = None;
    event.breadcrumbs = Default::default();
    event.extra.clear();
    event.modules.clear();
    event.tags.clear();
    event
        .tags
        .insert("component".into(), component.tag().into());
    event.contexts.clear();
    event.contexts.insert(
        "os".into(),
        Context::from(OsContext {
            name: Some(std::env::consts::OS.into()),
            ..Default::default()
        }),
    );
    event.contexts.insert(
        "device".into(),
        Context::from(DeviceContext {
            arch: Some(std::env::consts::ARCH.into()),
            ..Default::default()
        }),
    );
    Some(event)
}

/// Captures panics while a client is bound, then runs the previous hook, so
/// the panic message is printed exactly as before. Nothing is flushed here:
/// a TUI must first leave raw mode, which happens while the panic unwinds;
/// the [`Guard`] then flushes when it is dropped.
fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let next = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let hub = sentry::Hub::main();
            if hub.client().is_some() {
                let event = sentry::integrations::panic::PanicIntegration::new()
                    .event_from_panic_info(info);
                hub.capture_event(event);
            }
            next(info);
        }));
    });
}

/// Per-kind limit of [`ERROR_INTERVAL`] for [`report_error`].
#[derive(Default)]
struct RateLimit(BTreeMap<&'static str, Instant>);
impl RateLimit {
    fn allow(&mut self, kind: &'static str, now: Instant) -> bool {
        match self.0.get(kind) {
            Some(last) if now.saturating_duration_since(*last) < ERROR_INTERVAL => false,
            _ => {
                self.0.insert(kind, now);
                true
            }
        }
    }
}

/// Reports an error message of level `error`, at most once per `kind` per
/// [`ERROR_INTERVAL`]. A no-op unless [`init`] enabled reporting. `detail`
/// must hold no user data: no terminal content, paths, titles or commands.
pub fn report_error(kind: &'static str, detail: &str) -> bool {
    static LIMIT: Mutex<RateLimit> = Mutex::new(RateLimit(BTreeMap::new()));
    let hub = sentry::Hub::main();
    if hub.client().is_none() {
        return false;
    }
    let allowed = LIMIT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .allow(kind, Instant::now());
    if !allowed {
        return false;
    }
    hub.capture_event(Event {
        message: Some(format!("{kind}: {detail}")),
        level: Level::Error,
        fingerprint: vec![kind.into()].into(),
        ..Default::default()
    });
    true
}

/// The HTTPS transport: `ureq` with `rustls`, `ring` and the bundled WebPKI
/// roots. Certificates are always verified and requests time out.
struct UreqTransport;
impl sentry::TransportFactory for UreqTransport {
    fn create_transport_with_options(
        &self,
        options: sentry::TransportOptions,
    ) -> Arc<dyn sentry::Transport> {
        use ureq::tls::{TlsConfig, TlsProvider};
        let agent = ureq::Agent::config_builder()
            .tls_config(TlsConfig::builder().provider(TlsProvider::Rustls).build())
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .new_agent();
        Arc::new(
            sentry::transports::UreqHttpTransportOptions::from(options)
                .with_agent(agent)
                .build(),
        )
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
    #[test]
    fn nothing_starts_without_a_dsn() {
        // This test binary never binds a client (tests/error_reporting.rs
        // does, in its own process).
        assert!(init(None, Component::Tui).is_none());
        assert!(sentry::Hub::main().client().is_none());
        assert!(!report_error("test.kind", "ignored"));
    }
    #[test]
    fn options_disable_everything_but_errors() {
        let settings = Settings::new(Some("https://k@g.example.com/3"), Some("dev"))
            .unwrap()
            .unwrap();
        let options = client_options(&settings, Component::Relay, Arc::new(UreqTransport)).unwrap();
        let dsn = options.dsn.unwrap();
        assert_eq!(
            (dsn.public_key(), dsn.secret_key(), dsn.host(), dsn.port()),
            ("k", None, "g.example.com", 443)
        );
        assert_eq!(dsn.project_id().to_string(), "3");
        assert_eq!(options.release.as_deref(), Some(RELEASE));
        assert_eq!(options.environment.as_deref(), Some("dev"));
        assert_eq!(options.server_name, None);
        assert_eq!(options.max_breadcrumbs, 0);
        assert!(!options.send_default_pii && !options.attach_stacktrace);
        assert!(!options.accept_invalid_certs && !options.auto_session_tracking);
        assert!(!options.default_integrations && !options.debug);
        assert_eq!(options.integrations.len(), 1);
        assert_eq!(options.shutdown_timeout, FLUSH_TIMEOUT);
        let crumb = sentry::protocol::Breadcrumb::default();
        assert!((options.before_breadcrumb.unwrap())(crumb).is_none());
    }
    #[test]
    fn before_send_scrubs_identifying_data() {
        use sentry::protocol::{Breadcrumb, Request, User};
        let mut event = Event {
            message: Some("boom".into()),
            server_name: Some("host.internal".into()),
            user: Some(User {
                username: Some("alice".into()),
                ..Default::default()
            }),
            request: Some(Request {
                url: "https://example.com/?q=secret".parse().ok(),
                ..Default::default()
            }),
            breadcrumbs: vec![Breadcrumb::default()].into(),
            ..Default::default()
        };
        event.extra.insert("cwd".into(), "/home/alice".into());
        event.tags.insert("title".into(), "secret".into());
        event.contexts.insert(
            "os".into(),
            Context::from(OsContext {
                name: Some("Linux".into()),
                kernel_version: Some("6.0 host build".into()),
                ..Default::default()
            }),
        );
        let event = scrub(event, Component::Bridge).unwrap();
        assert_eq!(event.message.as_deref(), Some("boom"));
        assert!(event.user.is_none() && event.request.is_none() && event.server_name.is_none());
        assert!(event.breadcrumbs.is_empty() && event.extra.is_empty());
        assert_eq!(
            event.tags.into_iter().collect::<Vec<_>>(),
            [("component".to_owned(), "bridge".to_owned())]
        );
        let json = serde_json::to_string(&event.contexts).unwrap();
        assert!(!json.contains("host build"), "{json}");
        assert!(json.contains(std::env::consts::ARCH), "{json}");
        assert_eq!(event.contexts.len(), 2);
    }
    #[test]
    fn errors_are_limited_to_one_per_kind_per_minute() {
        let mut limit = RateLimit::default();
        let start = Instant::now();
        assert!(limit.allow("a", start));
        assert!(!limit.allow("a", start + Duration::from_secs(59)));
        assert!(limit.allow("b", start + Duration::from_secs(1)));
        assert!(limit.allow("a", start + ERROR_INTERVAL));
        assert!(!limit.allow("a", start + ERROR_INTERVAL + Duration::from_secs(1)));
    }
}
