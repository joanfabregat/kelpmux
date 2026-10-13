//! Stateful Alacritty adapter. This module owns no PTY or event loop.
use super::protocol::*;
use crate::Result;
use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    index::{Column, Point as TermPoint, Side},
    selection::{Selection, SelectionType},
    term::{
        Config, Term, TermMode, cell::Flags, color::Colors, point_to_viewport, viewport_to_point,
    },
    vte::ansi::{Color as AnsiColor, Processor, Rgb},
};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub enum Effect {
    WritePty(Vec<u8>),
    Title(String),
    Bell,
    Clipboard(String),
}

// Callbacks run synchronously inside the parser, never block on consumers, and
// cannot grow a queue without limit when an application floods OSC/DSR requests.
#[derive(Clone, Default)]
struct Listener(Arc<Mutex<Vec<Event>>>);
impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        if matches!(
            event,
            Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange
        ) {
            return;
        }
        if let Ok(mut queue) = self.0.try_lock() {
            if queue.len() < 256 {
                queue.push(event);
            }
        }
    }
}
struct Size {
    cols: usize,
    rows: usize,
}
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Terminal {
    term: Term<Listener>,
    parser: Processor,
    listener: Listener,
}
impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Self {
        let listener = Listener::default();
        let size = Size {
            cols: usize::from(cols.max(2)),
            rows: usize::from(rows.max(1)),
        };
        Self {
            term: Term::new(Config::default(), &size, listener.clone()),
            parser: Processor::new(),
            listener,
        }
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Effect> {
        let mut effects = Vec::new();
        for chunk in bytes
            .split_inclusive(|byte| *byte == 0x07 || *byte == b'\\')
            .flat_map(|part| part.chunks(4096))
        {
            self.parser.advance(&mut self.term, chunk);
            let events = self
                .listener
                .0
                .lock()
                .map(|mut q| std::mem::take(&mut *q))
                .unwrap_or_default();
            for event in events {
                let effect = match event {
                    Event::PtyWrite(text) => Some(Effect::WritePty(text.into_bytes())),
                    Event::Title(text) => Some(Effect::Title(safe_text(&text))),
                    Event::ResetTitle => Some(Effect::Title(String::new())),
                    Event::Bell => Some(Effect::Bell),
                    Event::ClipboardStore(_, text) => Some(Effect::Clipboard(text)),
                    Event::ColorRequest(index, format) => {
                        let rgb = self.term.colors()[index].unwrap_or_else(|| default_rgb(index));
                        Some(Effect::WritePty(format(rgb).into_bytes()))
                    }
                    Event::TextAreaSizeRequest(format) => Some(Effect::WritePty(
                        format(WindowSize {
                            num_lines: self.term.screen_lines() as u16,
                            num_cols: self.term.columns() as u16,
                            // Native composition has no pixel geometry to report.
                            cell_width: 0,
                            cell_height: 0,
                        })
                        .into_bytes(),
                    )),
                    // Clipboard reads deliberately remain disabled by Config::osc52.
                    _ => None,
                };
                if let Some(effect) = effect {
                    effects.push(effect);
                }
            }
        }
        effects
    }
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.term.resize(Size {
            cols: usize::from(cols.max(2)),
            rows: usize::from(rows.max(1)),
        });
    }
    pub fn snapshot(&self) -> Screen {
        let cols = self.term.columns() as u16;
        let rows = self.term.screen_lines() as u16;
        let content = self.term.renderable_content();
        let mut cells = vec![Cell::default(); usize::from(cols) * usize::from(rows)];
        for indexed in content.display_iter {
            let Some(point) = point_to_viewport(content.display_offset, indexed.point) else {
                continue;
            };
            if point.line >= usize::from(rows) || point.column.0 >= usize::from(cols) {
                continue;
            }
            let source = indexed.cell;
            let flags = source.flags;
            let mut fg = color(source.fg, content.colors);
            let mut bg = color(source.bg, content.colors);
            if flags.contains(Flags::INVERSE) {
                // Defaults are role-dependent in ANSI: resolve before swapping.
                if fg == Color::Default {
                    fg = rgb_color(default_rgb(256));
                }
                if bg == Color::Default {
                    bg = rgb_color(default_rgb(257));
                }
                std::mem::swap(&mut fg, &mut bg);
            }
            if content.selection.as_ref().is_some_and(|selection| {
                selection.contains_cell(&indexed, content.cursor.point, content.cursor.shape)
            }) {
                fg = Color::Rgb(0, 0, 0);
                bg = Color::Rgb(180, 200, 225);
            }
            let spacer = flags.contains(Flags::WIDE_CHAR_SPACER);
            let mut text = if spacer {
                String::new()
            } else if flags.intersects(Flags::HIDDEN | Flags::LEADING_WIDE_CHAR_SPACER) {
                " ".into()
            } else {
                safe_text(&source.c.to_string())
            };
            if !spacer && !flags.contains(Flags::HIDDEN) {
                if let Some(extra) = source.zerowidth() {
                    text.extend(extra.iter().copied().filter(|c| !c.is_control()));
                }
            }
            cells[point.line * usize::from(cols) + point.column.0] = Cell {
                text,
                width: if spacer {
                    0
                } else if flags.contains(Flags::WIDE_CHAR) {
                    2
                } else {
                    1
                },
                fg,
                bg,
                bold: flags.contains(Flags::BOLD),
                italic: flags.contains(Flags::ITALIC),
                underline: flags.intersects(Flags::ALL_UNDERLINES),
            };
        }
        let mode = content.mode;
        let cursor = point_to_viewport(content.display_offset, content.cursor.point)
            .filter(|p| p.line < usize::from(rows) && p.column.0 < usize::from(cols))
            .map(|p| Cursor {
                position: Point {
                    x: p.column.0 as u16,
                    y: p.line as u16,
                },
                visible: mode.contains(TermMode::SHOW_CURSOR) && content.display_offset == 0,
            });
        Screen {
            cols,
            rows,
            cells,
            cursor,
            modes: InputModes {
                application_cursor: mode.contains(TermMode::APP_CURSOR),
                application_keypad: mode.contains(TermMode::APP_KEYPAD),
                bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
                mouse_click: mode.contains(TermMode::MOUSE_REPORT_CLICK),
                mouse_drag: mode.contains(TermMode::MOUSE_DRAG),
                mouse_motion: mode.contains(TermMode::MOUSE_MOTION),
                mouse_sgr: mode.contains(TermMode::SGR_MOUSE),
                focus: mode.contains(TermMode::FOCUS_IN_OUT),
            },
        }
    }
    pub fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }
    pub fn select(&mut self, start: Point, end: Point) -> Result<String> {
        let valid = |p: Point| {
            usize::from(p.x) < self.term.columns() && usize::from(p.y) < self.term.screen_lines()
        };
        if !valid(start) || !valid(end) {
            return Err("selection is outside the terminal viewport".into());
        }
        let point = |p: Point| {
            viewport_to_point(
                self.term.grid().display_offset(),
                TermPoint::new(usize::from(p.y), Column(usize::from(p.x))),
            )
        };
        let (start, end) = if (start.y, start.x) <= (end.y, end.x) {
            (start, end)
        } else {
            (end, start)
        };
        let mut selection = Selection::new(SelectionType::Simple, point(start), Side::Left);
        selection.update(point(end), Side::Right);
        self.term.selection = Some(selection);
        Ok(self.selection().unwrap_or_default())
    }
    pub fn selection(&self) -> Option<String> {
        self.term.selection_to_string()
    }
}

pub(crate) fn safe_text(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}
fn rgb_color(rgb: Rgb) -> Color {
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}
fn color(value: AnsiColor, colors: &Colors) -> Color {
    let index = match value {
        AnsiColor::Spec(rgb) => return rgb_color(rgb),
        AnsiColor::Indexed(i) => usize::from(i),
        AnsiColor::Named(n) => n as usize,
    };
    if let Some(rgb) = colors[index] {
        return rgb_color(rgb);
    }
    if index < 256 {
        Color::Indexed(index as u8)
    } else if index == 256 || index == 257 {
        Color::Default
    } else {
        rgb_color(default_rgb(index))
    }
}
fn default_rgb(index: usize) -> Rgb {
    const ANSI: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    let (r, g, b) = match index {
        0..=15 => ANSI[index],
        16..=231 => {
            let i = index - 16;
            let c = |v| if v == 0 { 0 } else { (55 + v * 40) as u8 };
            (c(i / 36), c(i / 6 % 6), c(i % 6))
        }
        232..=255 => {
            let v = (8 + (index - 232) * 10) as u8;
            (v, v, v)
        }
        257 => (0, 0, 0),
        259..=266 => {
            let (r, g, b) = ANSI[index - 259];
            (r / 2, g / 2, b / 2)
        }
        _ => (229, 229, 229),
    };
    Rgb { r, g, b }
}
