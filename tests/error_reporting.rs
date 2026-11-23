//! Error reporting end to end, with a recording transport instead of HTTPS:
//! a panic in another thread and rate-limited errors each produce exactly the
//! expected events, scrubbed, and nothing is captured once the guard is gone;
//! the real HTTPS transport then starts and stops within the flush bound.
//! Its own test binary, because the client and panic hook are process-wide.
use kelpmux::native::error_reporting::{
    Component, FLUSH_TIMEOUT, RELEASE, Settings, init, init_with_transport, report_error,
};
use sentry::{Envelope, Transport, protocol::Event};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Recorder(Mutex<Vec<Event<'static>>>);
impl Transport for Recorder {
    fn send_envelope(&self, envelope: Envelope) {
        if let Some(event) = envelope.event() {
            self.0.lock().unwrap().push(event.clone());
        }
    }
}
impl Recorder {
    fn take(&self) -> Vec<Event<'static>> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

fn assert_scrubbed(event: &Event<'static>) {
    assert_eq!(event.release.as_deref(), Some(RELEASE));
    assert_eq!(event.environment.as_deref(), Some("ci"));
    assert_eq!(
        event.tags.get("component").map(String::as_str),
        Some("bridge")
    );
    assert_eq!(event.tags.len(), 1);
    assert!(event.breadcrumbs.is_empty());
    assert!(event.user.is_none() && event.request.is_none());
    assert!(event.server_name.is_none());
    assert!(event.extra.is_empty());
}

#[test]
fn panics_and_errors_are_reported_once_and_scrubbed() {
    let recorder = Arc::new(Recorder::default());
    let settings = Settings::new(Some("https://key@errors.example.com/9"), Some("ci")).unwrap();
    let guard =
        init_with_transport(settings, Component::Bridge, Arc::new(recorder.clone())).unwrap();

    sentry::add_breadcrumb(sentry::Breadcrumb {
        message: Some("typed secret".into()),
        ..Default::default()
    });
    let result = std::thread::spawn(|| panic!("reporting test panic")).join();
    assert!(result.is_err());
    assert!(guard.flush());
    let events = recorder.take();
    assert_eq!(events.len(), 1, "{events:?}");
    let panic = &events[0];
    assert_scrubbed(panic);
    assert_eq!(panic.level, sentry::Level::Fatal);
    let exception = &panic.exception.values[0];
    assert_eq!(exception.ty, "panic");
    assert_eq!(exception.value.as_deref(), Some("reporting test panic"));
    assert!(exception.stacktrace.is_some());

    assert!(report_error(
        "test.connect",
        "Connection refused (os error 111)"
    ));
    assert!(!report_error(
        "test.connect",
        "Connection refused (os error 111)"
    ));
    assert!(report_error(
        "test.handshake",
        "SSH server host key does not match"
    ));
    assert!(guard.flush());
    let events = recorder.take();
    assert_eq!(events.len(), 2, "{events:?}");
    for event in &events {
        assert_scrubbed(event);
        assert_eq!(event.level, sentry::Level::Error);
    }
    assert_eq!(
        events[0].message.as_deref(),
        Some("test.connect: Connection refused (os error 111)")
    );

    drop(guard);
    assert!(sentry::Hub::main().client().is_none());
    assert!(!report_error("test.other", "after shutdown"));
    let _ = std::thread::spawn(|| panic!("after shutdown")).join();
    assert!(recorder.take().is_empty());

    // The real HTTPS transport starts and shuts down within the flush bound
    // when nothing is pending (nothing is sent: no event was captured).
    let settings = Settings::new(Some("https://key@errors.invalid/9"), None).unwrap();
    let started = std::time::Instant::now();
    let guard = init(settings, Component::Tui).unwrap();
    assert!(sentry::Hub::main().client().is_some());
    drop(guard);
    assert!(sentry::Hub::main().client().is_none());
    assert!(started.elapsed() < FLUSH_TIMEOUT);
}
