//! `kelpmux attach`: the terminal loop of the stream TUI. It owns
//! the TTY and the stream connection; the controller owns all UI state.
use super::super::{
    cli::{
        Channel, ESCAPE_TIMEOUT, INTERRUPTED, RESIZED, RawTty, TerminalWriter, backend_size,
        fitted_mouse, paint, queue_events, tty_size,
    },
    connection::response,
    input::{Decoder, Router},
    protocol::*,
    stream::{self, Event, Request},
};
use super::{
    controller::{Controller, Options},
    state::{self, Store},
};
use crate::Result;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    collections::VecDeque,
    io::Write,
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::Path,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

/// How often UI state is written and checked for other TUIs' writes.
const FLUSH_INTERVAL: Duration = Duration::from_millis(500);
const PAINT_INTERVAL: Duration = Duration::from_millis(16);

/// Opens the workspace's persisted TUI state, migrating the composed
/// server's settings and tab layout once.
pub fn open_store(state: &Path) -> Result<Store> {
    crate::journal::private_dir(state)?;
    let mut store = Store::open(state)?;
    if !store.document.migrated {
        if store.document.settings.is_none() {
            if let Some(settings) = state::legacy_settings(state) {
                store.set_settings(settings);
            }
        }
        if store.document.layout.is_empty() {
            store.set_layout(state::legacy_layout(state));
        }
    }
    Ok(store)
}

fn send(channel: &mut Channel, requests: Vec<Request>) -> Result<()> {
    for request in requests {
        channel.send(&ClientMessage::Stream(request))?;
    }
    Ok(())
}

pub fn run(mut socket: UnixStream, state: &Path, takeover: bool) -> Result<()> {
    if unsafe { libc::isatty(0) } != 1 || unsafe { libc::isatty(1) } != 1 {
        return Err("attach requires stdin and stdout to be TTYs".into());
    }
    write_message(
        &mut socket,
        &ClientMessage::Stream(Request::Hello {
            version: stream::VERSION,
        }),
    )?;
    let welcome = match response(&mut socket)? {
        ServerMessage::Stream(welcome @ Event::Welcome { .. }) => welcome,
        ServerMessage::Stream(Event::Error { message, .. }) | ServerMessage::Error(message) => {
            return Err(format!("stream attach failed: {message}").into());
        }
        _ => {
            return Err(
                "the workspace server does not offer the pane stream; restart the workspace with a current server".into(),
            );
        }
    };
    let size = tty_size()?;
    let (cols, rows) = backend_size(size);
    // The workspace's own template and shell, as `kelpmux start` resolves
    // them: the TUI creates the tabs of a server started without panes.
    let (template, shell) = state::workspace_template(state);
    let default_shell = shell.or_else(|| {
        Some(
            std::env::var_os("SHELL")
                .map(|shell| shell.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/bin/bash".into()),
        )
    });
    let options = Options {
        takeover,
        workspace: state
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        dev_badge: std::env::var_os("KELPMUX_DEV_BADGE").is_some_and(|value| value == "1"),
        default_shell,
        template,
    };
    let mut controller = Controller::new(cols, rows, open_store(state)?, options)?;
    controller.welcome(welcome)?;
    let mut channel = Channel::new(socket)?;
    send(&mut channel, controller.take_requests())?;
    channel.flush()?;
    // Connection and handshake failures precede raw and alternate-screen mode.
    let tty = RawTty::enter()?;
    let result = run_loop(&mut controller, &mut channel, size);
    let _ = controller.store.flush();
    drop(tty);
    if let Some(reason) = &controller.stopped {
        eprintln!("kelpmux: {reason}");
    }
    result
}

fn run_loop(
    controller: &mut Controller,
    channel: &mut Channel,
    mut size: (u16, u16),
) -> Result<()> {
    let mut decoder = Decoder::default();
    let mut router = Router::default();
    let mut frame: Option<Frame> = None;
    let mut painted: Option<Frame> = None;
    let mut escape = None;
    let mut stdout = TerminalWriter;
    let mut pending = VecDeque::new();
    let mut keyboard_mode = 1;
    let mut pointer = None;
    let mut hovered = None;
    let mut hover_dirty = false;
    let mut painted_at = Instant::now() - Duration::from_secs(1);
    let mut flushed_at = Instant::now();
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) {
            return Ok(());
        }
        if RESIZED.swap(false, Ordering::Relaxed) {
            size = tty_size()?;
            painted = None;
            pointer = None;
            hovered = None;
            let (cols, rows) = backend_size(size);
            controller.resize(cols, rows)?;
        }
        for message in channel.receive()? {
            match message {
                ServerMessage::Stream(event) => controller.event(event)?,
                ServerMessage::Error(error) => controller.notice = Some(error),
                _ => {}
            }
        }
        if channel.eof {
            controller
                .stopped
                .get_or_insert_with(|| "the workspace server disconnected".into());
            return Ok(());
        }
        if controller.stopped.is_some() {
            return Ok(());
        }
        if escape.is_some_and(|time: Instant| time.elapsed() >= ESCAPE_TIMEOUT) {
            queue_events(&mut pending, decoder.flush())?;
            escape = None;
        }
        if controller.synced() {
            if frame.is_none() || controller.dirty {
                frame = Some(controller.frame());
            }
            while let Some(mut event) = pending.pop_front() {
                if matches!(&event, InputEvent::Bytes(bytes) if bytes == b"\x1b[I") {
                    // Focusing this terminal takes control, as in the
                    // composed TUI; the report still reaches the application.
                    controller.take_control();
                    frame = Some(controller.frame());
                }
                let current = frame.as_ref().expect("frame composed above");
                if let InputEvent::Mouse(mouse) = &mut event {
                    let Some(point) = fitted_mouse(current, size, mouse.position) else {
                        pointer = None;
                        hover_dirty |= hovered.take().is_some();
                        continue;
                    };
                    mouse.position = point;
                    if mouse.kind == MouseKind::Move {
                        pointer = (mouse.button == 3).then_some(point);
                    } else if pointer.is_some() {
                        pointer = Some(point);
                    }
                    let next =
                        pointer.and_then(|point| super::super::hover::target(current, point));
                    hover_dirty |= next != hovered;
                    hovered = next;
                } else if matches!(&event, InputEvent::Bytes(bytes) if bytes == b"\x1b[O") {
                    pointer = None;
                    hover_dirty |= hovered.take().is_some();
                }
                let messages = router.route(event, current);
                if !messages.is_empty() && controller.notice.take().is_some() {
                    controller.dirty = true;
                }
                for message in messages {
                    controller.message(message);
                }
                if controller.dirty {
                    // Later events route against the updated screen.
                    frame = Some(controller.frame());
                }
            }
            if let Some(current) = frame.as_ref() {
                for message in router.tick(current) {
                    controller.message(message);
                }
            }
        }
        controller.tick(Instant::now());
        if controller.detached {
            return Ok(());
        }
        send(channel, controller.take_requests())?;
        for text in std::mem::take(&mut controller.clipboard) {
            if text.len() <= MAX_INPUT {
                write!(stdout, "\x1b]52;c;{}\x07", STANDARD.encode(text.as_bytes()))?;
            }
        }
        if std::mem::take(&mut controller.bell) {
            stdout.write_all(b"\x07")?;
        }
        if controller.synced()
            && (controller.dirty || hover_dirty)
            && painted_at.elapsed() >= PAINT_INTERVAL
        {
            let next = controller.frame();
            hovered = pointer.and_then(|point| super::super::hover::target(&next, point));
            let mode = if next.controller && !next.help && next.ui.dialog.is_none() {
                next.panes
                    .iter()
                    .find(|p| p.pane.id == next.active)
                    .map_or(1, |p| p.modes.keyboard_flags | 1)
            } else {
                1
            };
            if mode != keyboard_mode {
                write!(stdout, "\x1b[={mode}u")?;
                keyboard_mode = mode;
            }
            paint(&mut stdout, &mut painted, &next, size, None, hovered)?;
            frame = Some(next);
            controller.dirty = false;
            hover_dirty = false;
            painted_at = Instant::now();
        }
        if flushed_at.elapsed() >= FLUSH_INTERVAL {
            // Save this TUI's changes and pick up other TUIs'; a reload
            // marks the controller dirty, so the next pass repaints.
            if let Err(error) = controller.exchange_state() {
                controller.notice = Some(format!("TUI state: {error}"));
            }
            flushed_at = Instant::now();
        }
        channel.flush()?;
        let mut polls = [
            libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: channel.socket.as_raw_fd(),
                events: libc::POLLIN
                    | if channel.output.is_empty() {
                        0
                    } else {
                        libc::POLLOUT
                    },
                revents: 0,
            },
        ];
        let result = unsafe { libc::poll(polls.as_mut_ptr(), 2, 10) };
        if result < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(std::io::Error::last_os_error().into());
        }
        if polls[0].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            let mut bytes = [0; 8192];
            let count = unsafe { libc::read(0, bytes.as_mut_ptr().cast(), bytes.len()) };
            if count == 0 {
                return Ok(());
            }
            if count < 0 {
                let error = std::io::Error::last_os_error();
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) {
                    continue;
                }
                return Err(error.into());
            }
            queue_events(&mut pending, decoder.feed(&bytes[..count as usize]))?;
            escape = Some(Instant::now());
        }
    }
}
