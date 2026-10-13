//! Native composition and ANSI damage output. Header hit targets are 3 columns
//! each, aligned to the right edge: grid=6, capped=9, fullscreen=6.
use super::{engine::safe_text, protocol::*, workspace::Workspace};
use std::{collections::BTreeMap, fmt::Write};
use unicode_width::UnicodeWidthChar;
const PALETTE: [(u8, u8); 9] = [
    (39, 24),
    (42, 29),
    (141, 60),
    (214, 94),
    (80, 30),
    (205, 89),
    (185, 58),
    (209, 95),
    (111, 67),
];
const SPINNER: [char; 8] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧'];

pub fn compose(
    workspace: &Workspace,
    screens: &BTreeMap<PaneId, Screen>,
    controller: bool,
    notice: Option<&str>,
    spinner: usize,
) -> Frame {
    let mut frame = Frame {
        cols: workspace.cols,
        rows: workspace.rows,
        cells: vec![Cell::default(); usize::from(workspace.cols) * usize::from(workspace.rows)],
        cursor: None,
        panes: workspace.layout(),
        active: workspace.active,
        reading: workspace.reading,
        controller,
        help: workspace.help,
        notice: notice.map(safe_text),
    };
    for view in &mut frame.panes {
        if let Some(screen) = screens.get(&view.pane.id) {
            view.modes = screen.modes;
        }
    }
    for view in frame.panes.clone() {
        let selected = view.pane.id == workspace.active;
        let covered_source = matches!(workspace.reading, ReadingMode::Capped(id) if id == view.pane.id)
            && !view.overlay;
        fill(&mut frame, view.rect, Color::Default);
        if covered_source {
            fill(&mut frame, view.body, Color::Rgb(0, 0, 0));
        } else if let Some(screen) = screens.get(&view.pane.id) {
            for y in 0..view.body.rows.min(screen.rows) {
                for x in 0..view.body.cols.min(screen.cols) {
                    if let Some(cell) = screen
                        .cells
                        .get(usize::from(y) * usize::from(screen.cols) + usize::from(x))
                    {
                        let mut cell = cell.clone();
                        if cell.width == 2 && x + 1 >= view.body.cols {
                            cell.text = " ".into();
                            cell.width = 1;
                        }
                        put(
                            &mut frame,
                            view.body.x.saturating_add(x),
                            view.body.y.saturating_add(y),
                            cell,
                        );
                    }
                }
            }
            if selected && !workspace.help {
                if let Some(cursor) = screen
                    .cursor
                    .filter(|c| c.position.x < view.body.cols && c.position.y < view.body.rows)
                {
                    frame.cursor = Some(Cursor {
                        position: Point {
                            x: view.body.x + cursor.position.x,
                            y: view.body.y + cursor.position.y,
                        },
                        visible: cursor.visible && controller,
                    });
                }
            }
        }
        header(&mut frame, &view, selected, workspace.reading, spinner);
    }
    if frame.rows > 0 {
        let y = frame.rows - 1;
        fill(
            &mut frame,
            Rect {
                x: 0,
                y,
                cols: workspace.cols,
                rows: 1,
            },
            Color::Indexed(234),
        );
        let label = if controller {
            "^B prefix · 1–4 focus · r read · f full · d detach"
        } else {
            "Observer · Ctrl-B t take control"
        };
        let mut text = label.to_owned();
        if let Some(notice) = notice {
            text.push_str(" · ");
            text.push_str(&safe_text(notice));
        }
        let width = frame.cols.saturating_sub(3);
        write_text(
            &mut frame,
            0,
            y,
            width,
            &text,
            Color::Indexed(250),
            Color::Indexed(234),
            false,
        );
        write_text(
            &mut frame,
            width,
            y,
            3,
            " ? ",
            Color::Indexed(231),
            Color::Indexed(24),
            true,
        );
    }
    if workspace.help {
        let cols = frame.cols.saturating_sub(4).min(76);
        let rows = frame.rows.saturating_sub(2).min(18);
        let rect = Rect {
            x: (frame.cols - cols) / 2,
            y: (frame.rows.saturating_sub(1) - rows) / 2,
            cols,
            rows,
        };
        fill(&mut frame, rect, Color::Indexed(236));
        write_text(
            &mut frame,
            rect.x,
            rect.y,
            cols,
            " Kelpmux keys",
            Color::Indexed(231),
            Color::Indexed(24),
            true,
        );
        if cols >= 3 {
            write_text(
                &mut frame,
                rect.x + cols - 3,
                rect.y,
                3,
                " × ",
                Color::Indexed(231),
                Color::Indexed(88),
                true,
            );
        }
        let lines = [
            "Ctrl-B, then:",
            "1–4       Focus a pane",
            "r         Reading view (120 columns)",
            "f         Full width reading",
            "Esc       Return to grid",
            "g         Cycle grid",
            "d         Detach",
            "t         Take control",
            "?         Toggle this help",
            "Wheel     Scroll history",
            "Drag      Select text",
            "y         Copy selection",
        ];
        for (i, line) in lines
            .iter()
            .enumerate()
            .take(usize::from(rows.saturating_sub(2)))
        {
            write_text(
                &mut frame,
                rect.x + 2,
                rect.y + 2 + i as u16,
                cols.saturating_sub(4),
                line,
                Color::Indexed(252),
                Color::Indexed(236),
                false,
            );
        }
        frame.cursor = None;
    }
    repair_wide_edges(&mut frame);
    frame
}
fn header(
    frame: &mut Frame,
    view: &PaneView,
    selected: bool,
    reading: ReadingMode,
    spinner: usize,
) {
    if view.rect.rows == 0 {
        return;
    }
    let (bright, muted) = PALETTE[usize::from(view.pane.palette) % PALETTE.len()];
    let bg = Color::Indexed(if selected { bright } else { muted });
    let fg = Color::Indexed(if selected { 16 } else { 252 });
    fill(
        frame,
        Rect {
            rows: 1,
            ..view.rect
        },
        bg,
    );
    let badge = format!(
        " {} {} ",
        if selected { "▶" } else { " " },
        view.pane.number
    );
    let badge_width = badge.chars().count() as u16;
    write_text(
        frame,
        view.rect.x,
        view.rect.y,
        view.rect.cols.min(badge_width),
        &badge,
        Color::Indexed(if selected { 231 } else { 117 }),
        Color::Indexed(if selected { 24 } else { 17 }),
        true,
    );
    let buttons: &[(&str, u8, u8)] = match reading {
        ReadingMode::Fullscreen(id) if id == view.pane.id => &[(" ▣ ", 16, 39), (" × ", 231, 88)],
        ReadingMode::Capped(id) if id == view.pane.id && view.overlay => {
            &[(" ↙ ", 231, 24), (" ⛶ ", 16, 39), (" × ", 231, 88)]
        }
        _ => &[(" ↗ ", 231, 24), (" ⛶ ", 16, 39)],
    };
    let button_width = buttons.len() as u16 * 3;
    let title_width = view.rect.cols.saturating_sub(badge_width + button_width);
    let marker = if view.pane.status == crate::model::Status::Working {
        SPINNER[spinner % 8].to_string()
    } else {
        view.pane.status.marker().to_owned()
    };
    let title = format!(" {} {}", marker, safe_text(&view.pane.title));
    write_text(
        frame,
        view.rect.x + badge_width,
        view.rect.y,
        title_width,
        &title,
        fg,
        bg,
        false,
    );
    for (i, (label, fg, bg)) in buttons.iter().enumerate() {
        let offset = view.rect.cols.saturating_sub(button_width) + i as u16 * 3;
        write_text(
            frame,
            view.rect.x + offset,
            view.rect.y,
            view.rect.cols.saturating_sub(offset).min(3),
            label,
            Color::Indexed(*fg),
            Color::Indexed(*bg),
            true,
        );
    }
}
fn put(frame: &mut Frame, x: u16, y: u16, cell: Cell) {
    if x < frame.cols && y < frame.rows {
        frame.cells[usize::from(y) * usize::from(frame.cols) + usize::from(x)] = cell;
    }
}
fn fill(frame: &mut Frame, rect: Rect, bg: Color) {
    for y in rect.y..rect.y.saturating_add(rect.rows).min(frame.rows) {
        for x in rect.x..rect.x.saturating_add(rect.cols).min(frame.cols) {
            put(
                frame,
                x,
                y,
                Cell {
                    bg,
                    ..Cell::default()
                },
            );
        }
    }
}
fn char_width(c: char) -> u16 {
    c.width().unwrap_or(0) as u16
}
fn write_text(
    frame: &mut Frame,
    x: u16,
    y: u16,
    width: u16,
    text: &str,
    fg: Color,
    bg: Color,
    bold: bool,
) {
    let mut offset = 0u16;
    let mut last: Option<usize> = None;
    for c in safe_text(text).chars() {
        let cw = char_width(c);
        if cw == 0 {
            if let Some(index) = last {
                if let Some(cell) = frame.cells.get_mut(index) {
                    cell.text.push(c);
                }
            }
            continue;
        }
        if offset.saturating_add(cw) > width
            || x.saturating_add(offset).saturating_add(cw) > frame.cols
        {
            break;
        }
        let cell = Cell {
            text: c.to_string(),
            width: cw as u8,
            fg,
            bg,
            bold,
            ..Cell::default()
        };
        put(frame, x + offset, y, cell.clone());
        last = Some(usize::from(y) * usize::from(frame.cols) + usize::from(x + offset));
        if cw == 2 {
            put(
                frame,
                x + offset + 1,
                y,
                Cell {
                    text: String::new(),
                    width: 0,
                    ..cell
                },
            );
        }
        offset += cw;
    }
}

/// Emit only changed horizontal runs; all text is sanitized before reaching the
/// host terminal. CUP per run avoids delayed autowrap at the last column.
pub fn terminal_diff(previous: Option<&Frame>, frame: &Frame) -> Vec<u8> {
    let previous = previous.filter(|p| {
        p.cols == frame.cols && p.rows == frame.rows && p.cells.len() == frame.cells.len()
    });
    if previous.is_some_and(|p| p.cells == frame.cells && p.cursor == frame.cursor) {
        return vec![];
    }
    let mut output = String::from("\x1b[?25l");
    if previous.is_none() {
        output.push_str("\x1b[0m\x1b[2J");
    }
    for y in 0..frame.rows {
        let mut x = 0u16;
        while x < frame.cols {
            let index = usize::from(y) * usize::from(frame.cols) + usize::from(x);
            let Some(cell) = frame.cells.get(index) else {
                break;
            };
            if cell.width == 0 || previous.is_some_and(|p| p.cells[index] == *cell) {
                x += 1;
                continue;
            }
            let _ = write!(output, "\x1b[{};{}H", y + 1, x + 1);
            loop {
                let index = usize::from(y) * usize::from(frame.cols) + usize::from(x);
                let Some(cell) = frame.cells.get(index) else {
                    break;
                };
                if cell.width == 0 {
                    x += 1;
                    if x == frame.cols {
                        break;
                    }
                    continue;
                }
                if previous.is_some_and(|p| p.cells[index] == *cell) {
                    break;
                }
                output.push_str("\x1b[0m");
                sgr_color(&mut output, cell.fg, true);
                sgr_color(&mut output, cell.bg, false);
                if cell.bold {
                    output.push_str("\x1b[1m");
                }
                if cell.italic {
                    output.push_str("\x1b[3m");
                }
                if cell.underline {
                    output.push_str("\x1b[4m");
                }
                let text = safe_text(&cell.text);
                output.push_str(if text.is_empty() { " " } else { &text });
                x = x.saturating_add(u16::from(cell.width.max(1)));
                if x >= frame.cols {
                    break;
                }
            }
        }
    }
    output.push_str("\x1b[0m");
    if let Some(cursor) = frame
        .cursor
        .filter(|c| c.position.x < frame.cols && c.position.y < frame.rows)
    {
        let _ = write!(
            output,
            "\x1b[{};{}H",
            cursor.position.y + 1,
            cursor.position.x + 1
        );
        if cursor.visible {
            output.push_str("\x1b[?25h");
        }
    }
    output.into_bytes()
}
fn sgr_color(output: &mut String, color: Color, foreground: bool) {
    let prefix = if foreground { 38 } else { 48 };
    match color {
        Color::Default => {}
        Color::Indexed(i) => {
            let _ = write!(output, "\x1b[{prefix};5;{i}m");
        }
        Color::Rgb(r, g, b) => {
            let _ = write!(output, "\x1b[{prefix};2;{r};{g};{b}m");
        }
    }
}

// Overlay edges may bisect a wide glyph in a background pane. Repair both
// halves so a leading glyph cannot paint into the overlay and no orphan spacer
// can prevent a later diff from clearing that column.
fn repair_wide_edges(frame: &mut Frame) {
    for y in 0..usize::from(frame.rows) {
        for x in 0..usize::from(frame.cols) {
            let i = y * usize::from(frame.cols) + x;
            let invalid = if frame.cells[i].width == 2 {
                x + 1 == usize::from(frame.cols) || frame.cells[i + 1].width != 0
            } else if frame.cells[i].width == 0 {
                x == 0 || frame.cells[i - 1].width != 2
            } else {
                false
            };
            if invalid {
                frame.cells[i].text = " ".into();
                frame.cells[i].width = 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::Status, native::engine::Terminal};
    fn view(cols: u16, overlay: bool) -> PaneView {
        PaneView {
            pane: PaneInfo {
                id: 1,
                number: 1,
                pid: 1,
                cwd: String::new(),
                title: "題名 \x1b[2J #{literal}".into(),
                palette: 0,
                status: Status::Idle,
                session_id: None,
                dead: false,
            },
            rect: Rect {
                x: 0,
                y: 0,
                cols,
                rows: 4,
            },
            body: Rect {
                x: 0,
                y: 1,
                cols,
                rows: 3,
            },
            overlay,
            modes: InputModes::default(),
        }
    }
    #[test]
    fn header_palette_buttons_title_sanitization_and_wide_clipping() {
        for cols in [40, 80, 120, 240, 400] {
            for (reading, overlay, buttons) in [
                (ReadingMode::Grid, false, 6),
                (ReadingMode::Capped(1), true, 9),
                (ReadingMode::Fullscreen(1), true, 6),
            ] {
                let workspace = Workspace::new(cols, 10).unwrap();
                let mut frame = compose(&workspace, &BTreeMap::new(), true, None, 0);
                header(&mut frame, &view(cols, overlay), true, reading, 0);
                assert_eq!(frame.cells[0].bg, Color::Indexed(24));
                assert_eq!(frame.cells[5].bg, Color::Indexed(39));
                let button_start = usize::from(cols - buttons);
                assert_eq!(frame.cells[button_start].text, " ");
                assert_eq!(frame.cells[usize::from(cols) - 1].text, " ");
                let text = frame.cells[..usize::from(cols)]
                    .iter()
                    .map(|c| c.text.as_str())
                    .collect::<String>();
                assert!(text.contains("#{literal}"));
                assert!(!text.contains('\x1b'));
                let mut display = Terminal::new(cols, 10);
                display.feed(&terminal_diff(None, &frame));
                assert_eq!(display.snapshot().cells, frame.cells);
                header(&mut frame, &view(cols, overlay), false, reading, 0);
                assert_eq!(frame.cells[5].bg, Color::Indexed(24));
            }
        }
    }
    #[test]
    fn bisected_background_wide_cells_are_erased() {
        let workspace = Workspace::new(40, 10).unwrap();
        let mut frame = compose(&workspace, &BTreeMap::new(), true, None, 0);
        frame.cells[2].text = "界".into();
        frame.cells[2].width = 2;
        frame.cells[4].width = 0;
        frame.cells[4].text.clear();
        repair_wide_edges(&mut frame);
        assert_eq!(frame.cells[2].width, 1);
        assert_eq!(frame.cells[2].text, " ");
        assert_eq!(frame.cells[4].width, 1);
    }
}
