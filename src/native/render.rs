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
        ui: workspace.ui.clone(),
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
            if selected && !workspace.help && workspace.ui.dialog.is_none() {
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
        if view.overlay && matches!(workspace.reading, ReadingMode::Capped(_)) {
            write_text(
                &mut frame,
                view.rect.x + view.rect.cols - 1,
                view.rect.y + view.rect.rows - 1,
                1,
                "◢",
                Color::Indexed(231),
                Color::Indexed(24),
                true,
            );
        }
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
            format!(
                "^{}: {} read · {} grid · {} detach",
                char::from(b'A' + workspace.ui.config.prefix - 1),
                binding_label(&workspace.ui.config, "z", "read"),
                binding_label(&workspace.ui.config, "g", "grid"),
                binding_label(&workspace.ui.config, "d", "detach")
            )
        } else {
            format!(
                "Observer · Ctrl-{} {} take control",
                char::from(b'A' + workspace.ui.config.prefix - 1),
                binding_label(&workspace.ui.config, "tT", "takeover")
            )
        };
        let mut text = label;
        if let Some(notice) = notice {
            text.push_str(" · ");
            text.push_str(&safe_text(notice));
        }
        let width = frame.cols.saturating_sub(10);
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
            7,
            " Grid ",
            Color::Indexed(231),
            Color::Indexed(24),
            true,
        );
        write_text(
            &mut frame,
            width + 7,
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
        let key = |default, action| binding_label(&workspace.ui.config, default, action);
        let lines = [
            format!(
                "Ctrl-{}, then:",
                char::from(b'A' + workspace.ui.config.prefix - 1)
            ),
            "1–9       Read a numbered pane (also Alt+1–9)".into(),
            format!(
                "{}  Reading / grid (also Alt+Tab / Alt+0)",
                key("z", "read")
            ),
            format!("{}  Full width / capped reading", key("f", "fullscreen")),
            format!(
                "{}  Grid picker (occupied panes protected)",
                key("g", "grid")
            ),
            format!(
                "{}, N  Move active pane to numbered destination",
                key("s", "move")
            ),
            format!(
                "{} / {} / {}  Agents / attention / finished",
                key("a", "agents"),
                key("!", "attention"),
                key("n", "finished")
            ),
            format!(
                "{} {} {} {}  Focus left/down/up/right",
                key("h", "focus-left"),
                key("j", "focus-down"),
                key("k", "focus-up"),
                key("l", "focus-right")
            ),
            format!(
                "{} {} {} {}  Read width +10/-10; height +2/-2",
                key("+", "read-wider"),
                key("-", "read-narrower"),
                key(">", "read-taller"),
                key("<", "read-shorter")
            ),
            format!("{}  Detach", key("d", "detach")),
            format!("{}  Take control", key("tT", "takeover")),
            format!("{}  Toggle help", key("?", "help")),
            format!("{}  Scroll: arrows/PgUp/PgDn; q leaves", key("[", "scroll")),
            format!("{}  Copy selection (also drag release)", key("y", "copy")),
            "Wheel/drag Scroll/select; Shift for mouse-aware apps".into(),
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
    if let Some(dialog) = workspace.ui.dialog {
        draw_dialog(&mut frame, workspace, dialog);
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
    let fg = Color::Indexed(if selected { 231 } else { 245 });
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
        Color::Indexed(if selected { 226 } else { 117 }),
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
fn draw_dialog(frame: &mut Frame, workspace: &Workspace, dialog: super::ui::Dialog) {
    use super::ui::{AgentFilter, DialogKind};
    let bounds = super::workspace::dialog_bounds(frame.cols, frame.rows);
    fill(frame, bounds, Color::Indexed(236));
    let title = match dialog.kind {
        DialogKind::Grid => " Grid · arrows, Enter or click",
        DialogKind::Move => " Move active pane · choose destination",
        DialogKind::Agents => match dialog.filter {
            AgentFilter::All => " Agents",
            AgentFilter::Attention => " Agents needing attention",
            AgentFilter::Finished => " Finished agents",
        },
    };
    write_text(
        frame,
        bounds.x,
        bounds.y,
        bounds.cols,
        title,
        Color::Indexed(231),
        Color::Indexed(24),
        true,
    );
    write_text(
        frame,
        bounds.x + bounds.cols - 3,
        bounds.y,
        3,
        " × ",
        Color::Indexed(231),
        Color::Indexed(88),
        true,
    );
    if dialog.kind == DialogKind::Grid {
        let occupied = workspace
            .panes
            .iter()
            .filter(|p| !workspace.ui.unused.contains(&p.id))
            .count();
        for (index, (rect, &(cols, rows))) in super::workspace::grid_cards(bounds)
            .into_iter()
            .zip(super::workspace::GRIDS.iter())
            .enumerate()
        {
            let enabled = usize::from(cols) * usize::from(rows) >= occupied;
            let bg = Color::Indexed(if index == dialog.selection && enabled {
                24
            } else {
                236
            });
            let fg = Color::Indexed(if enabled { 231 } else { 240 });
            fill(frame, rect, bg);
            let label = if rect.cols < 16 {
                format!("{}×{}", cols, rows)
            } else {
                format!("{} cols × {} rows", cols, rows)
            };
            write_text(
                frame,
                rect.x + 1,
                rect.y,
                rect.cols.saturating_sub(2),
                &label,
                fg,
                bg,
                true,
            );
            let drawing = if cols == 1 {
                "┌────┐"
            } else if cols == 2 {
                "┌──┬──┐"
            } else {
                "┌─┬─┬─┐"
            };
            if rect.rows > 1 {
                write_text(
                    frame,
                    rect.x + 1,
                    rect.y + 1,
                    rect.cols.saturating_sub(2),
                    drawing,
                    fg,
                    bg,
                    false,
                );
            }
            for row in 0..u16::from(rows) * 2 {
                let y = rect.y + 2 + row;
                if y < rect.y + rect.rows {
                    let line = if row == u16::from(rows) * 2 - 1 {
                        if cols == 1 {
                            "└────┘"
                        } else if cols == 2 {
                            "└──┴──┘"
                        } else {
                            "└─┴─┴─┘"
                        }
                    } else if row % 2 == 1 {
                        if cols == 1 {
                            "├────┤"
                        } else if cols == 2 {
                            "├──┼──┤"
                        } else {
                            "├─┼─┼─┤"
                        }
                    } else if cols == 1 {
                        "│    │"
                    } else if cols == 2 {
                        "│  │  │"
                    } else {
                        "│ │ │ │"
                    };
                    write_text(
                        frame,
                        rect.x + 1,
                        y,
                        rect.cols.saturating_sub(2),
                        line,
                        fg,
                        bg,
                        false,
                    );
                }
            }
        }
    } else if dialog.kind == DialogKind::Move {
        for (index, rect) in
            super::workspace::move_cards(bounds, workspace.grid_cols, workspace.grid_rows)
                .into_iter()
                .enumerate()
        {
            if let Some(pane) = workspace.panes.get(index) {
                let bg = Color::Indexed(if index == dialog.selection { 24 } else { 236 });
                fill(frame, rect, bg);
                let label = format!(
                    " {} {}",
                    pane.number,
                    if Some(pane.id) == dialog.source {
                        "← source"
                    } else {
                        ""
                    }
                );
                write_text(
                    frame,
                    rect.x + 1,
                    rect.y,
                    rect.cols.saturating_sub(2),
                    &label,
                    Color::Indexed(226),
                    bg,
                    true,
                );
                if rect.rows > 1 {
                    write_text(
                        frame,
                        rect.x + 1,
                        rect.y + 1,
                        rect.cols.saturating_sub(2),
                        &pane.title,
                        Color::Indexed(231),
                        bg,
                        false,
                    );
                }
            }
        }
    } else {
        let entries = workspace.dialog_entries();
        if entries.is_empty() {
            write_text(
                frame,
                bounds.x + 1,
                bounds.y + 2,
                bounds.cols - 2,
                "No matching agents",
                Color::Indexed(245),
                Color::Indexed(236),
                false,
            );
        }
        let visible = usize::from(bounds.rows.saturating_sub(2));
        let offset = dialog.selection.saturating_sub(visible.saturating_sub(1));
        for (index, id) in entries.iter().enumerate().skip(offset).take(visible) {
            if let Some(pane) = workspace.panes.iter().find(|p| p.id == *id) {
                let rect = Rect {
                    x: bounds.x,
                    y: bounds.y + 2 + (index - offset) as u16,
                    cols: bounds.cols,
                    rows: 1,
                };
                let bg = Color::Indexed(if index == dialog.selection { 24 } else { 236 });
                fill(frame, rect, bg);
                let label = format!(
                    " {} {} {} · {} · session {}",
                    pane.number,
                    pane.status.marker(),
                    pane.title,
                    pane.status.text(),
                    pane.session_id.as_deref().unwrap_or("unknown")
                );
                write_text(
                    frame,
                    rect.x + 1,
                    rect.y,
                    rect.cols - 2,
                    &label,
                    Color::Indexed(231),
                    bg,
                    index == dialog.selection,
                );
            }
        }
    }
    frame.cursor = None;
}
fn binding_label(config: &super::ui::UiConfig, defaults: &str, action: &str) -> String {
    let mut keys: Vec<String> = defaults
        .chars()
        .map(|c| c.to_string())
        .filter(|key| config.bindings.get(key).is_none_or(|value| value == action))
        .collect();
    for (key, value) in &config.bindings {
        if value == action && !keys.contains(key) {
            keys.push(key.clone());
        }
    }
    if keys.is_empty() {
        "unbound".into()
    } else {
        keys.join("/")
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
// Position, clipping and style are independent inputs to this drawing primitive.
#[allow(clippy::too_many_arguments)]
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
                ..Default::default()
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
