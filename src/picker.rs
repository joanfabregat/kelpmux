//! A short-lived card dialog inside tmux's own popup, never an agent terminal.
use crate::{Result, workspace::GridChoice};
use std::io::{self, Write};

struct Terminal(libc::termios);
impl Terminal {
    fn raw() -> Result<Self> {
        let mut term = std::mem::MaybeUninit::uninit();
        // SAFETY: stdin is the popup PTY; libc initializes the supplied termios.
        if unsafe { libc::tcgetattr(0, term.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        let saved = unsafe { term.assume_init() };
        let mut raw = saved;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        print!("\x1b[?25l\x1b[?1003h\x1b[?1006h");
        io::stdout().flush()?;
        Ok(Self(saved))
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        print!("\x1b[0m\x1b[?1003l\x1b[?1006l\x1b[?25h");
        let _ = io::stdout().flush();
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.0) };
    }
}

pub const WIDTH: usize = 68;
const CARD_WIDTH: usize = 21;
const CARD_HEIGHT: usize = 6;

fn draw(choices: &[GridChoice], selected: usize) -> Result<()> {
    print!(
        "\x1b[0m\x1b[48;5;235m\x1b[2J\x1b[H\x1b[38;5;255m Kelpmux grid\x1b[1;65H\x1b[48;5;88m × \x1b[48;5;235m"
    );
    for (index, choice) in choices.iter().enumerate() {
        let x = 2 + index % 3 * 22;
        let y = 3 + index / 3 * 7;
        let (fg, bg) = if choice.blocked.is_some() {
            (244, 236)
        } else if index == selected {
            (16, 39)
        } else if choice.current {
            (255, 24)
        } else {
            (252, 237)
        };
        for row in 0..CARD_HEIGHT {
            print!(
                "\x1b[{};{}H\x1b[38;5;{fg};48;5;{bg}m{}",
                y + row,
                x,
                " ".repeat(CARD_WIDTH)
            );
        }
        print!(
            "\x1b[{y};{}H{}  {} cols x {} rows",
            x + 1,
            index + 1,
            choice.grid.cols,
            choice.grid.rows
        );
        for (row, line) in choice.drawing(true).iter().enumerate() {
            print!("\x1b[{};{}H{line}", y + 2 + row, x + 3);
        }
        if choice.current {
            print!("\x1b[{};{}H● current", y + 5, x + 1);
        }
        if choice.blocked.is_some() {
            print!("\x1b[{};{}HUnavailable", y + 5, x + 1);
        }
    }
    let note = choices[selected]
        .blocked
        .as_deref()
        .unwrap_or("Click / number / arrows + Enter · Esc closes");
    print!("\x1b[16;1H\x1b[38;5;252;48;5;235m{note:<68}\x1b[0m");
    io::stdout().flush()?;
    Ok(())
}

fn byte(timeout: i32) -> Result<Option<u8>> {
    let mut fd = libc::pollfd {
        fd: 0,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one initialized pollfd; no pointers retained by libc.
    let ready = unsafe { libc::poll(&mut fd, 1, timeout) };
    if ready < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(None);
        }
        return Err(error.into());
    }
    if ready == 0 {
        return Ok(None);
    }
    let mut b = [0];
    // Read the same fd we poll; Stdin buffering could hide the rest of an
    // escape/mouse packet from poll and turn a click into an Escape cancel.
    let count = unsafe { libc::read(0, b.as_mut_ptr().cast(), 1) };
    if count < 0 {
        return Err(io::Error::last_os_error().into());
    }
    if count == 0 {
        return Err("picker input closed".into());
    }
    Ok(Some(b[0]))
}

pub fn choose(choices: &[GridChoice]) -> Result<Option<usize>> {
    if choices.len() != 6 {
        return Err("picker requires six grid presets".into());
    }
    let _terminal = Terminal::raw()?;
    let mut selected = choices.iter().position(|c| c.current).unwrap_or(0);
    draw(choices, selected)?;
    loop {
        let Some(key) = byte(120_000)? else {
            return Ok(None);
        };
        let mut apply = false;
        match key {
            b'1'..=b'6' => {
                selected = (key - b'1') as usize;
                apply = true;
            }
            b'\r' | b'\n' => apply = true,
            b'q' | 3 => return Ok(None),
            27 => {
                if byte(80)? != Some(b'[') {
                    return Ok(None);
                }
                let mut seq = Vec::new();
                while seq.len() < 40 {
                    let Some(b) = byte(80)? else {
                        break;
                    };
                    seq.push(b);
                    if b.is_ascii_alphabetic() || b == b'~' {
                        break;
                    }
                }
                match seq.as_slice() {
                    b"A" => selected = selected.saturating_sub(3),
                    b"B" => selected = (selected + 3).min(5),
                    b"C" => selected = (selected + 1).min(5),
                    b"D" => selected = selected.saturating_sub(1),
                    _ if seq.first() == Some(&b'<') => {
                        let message = String::from_utf8_lossy(&seq);
                        let values: Vec<_> = message[1..message.len() - 1]
                            .split(';')
                            .filter_map(|s| s.parse::<usize>().ok())
                            .collect();
                        if values.len() == 3 {
                            let (button, x, y) = (values[0], values[1], values[2]);
                            if button & 64 != 0 {
                                continue;
                            }
                            if y == 1 && x >= 65 && button == 0 {
                                return Ok(None);
                            }
                            if (2..=66).contains(&x) && (3..=15).contains(&y) {
                                let col = (x - 2) / 22;
                                let row = (y - 3) / 7;
                                if col < 3
                                    && row < 2
                                    && (x - 2) % 22 < CARD_WIDTH
                                    && (y - 3) % 7 < CARD_HEIGHT
                                {
                                    selected = row * 3 + col;
                                    apply = button == 0 && seq.last() == Some(&b'M');
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if apply && choices[selected].blocked.is_none() {
            return Ok(Some(selected));
        }
        draw(choices, selected)?;
    }
}
