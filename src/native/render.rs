//! Native grid/tab composition and ANSI damage output. Grid header controls
//! occupy six columns; tab navigation uses shared geometry from workspace.
use super::{engine::safe_text, protocol::*, ui::PALETTE, workspace::Workspace};
use std::{collections::BTreeMap, fmt::Write};
use unicode_width::UnicodeWidthChar;
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
    if workspace.reading != ReadingMode::Grid {
        draw_tabs(&mut frame, workspace, spinner);
    } else {
        draw_grid_separators(&mut frame, workspace);
    }
    for view in frame.panes.clone() {
        let selected = view.pane.id == workspace.active;
        fill(&mut frame, view.rect, Color::Default);
        if let Some(screen) = screens.get(&view.pane.id) {
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
        if view.overlay && Some(view.pane.id) == workspace.ui.scratch {
            fill(
                &mut frame,
                Rect {
                    rows: 1,
                    ..view.rect
                },
                Color::Indexed(24),
            );
            write_text(
                &mut frame,
                view.rect.x,
                view.rect.y,
                view.rect.cols - 3,
                " Scratch terminal",
                Color::Indexed(231),
                Color::Indexed(24),
                true,
            );
            write_text(
                &mut frame,
                view.rect.x + view.rect.cols - 3,
                view.rect.y,
                3,
                " × ",
                Color::Indexed(231),
                Color::Indexed(88),
                true,
            );
        } else if workspace.reading == ReadingMode::Grid {
            header(&mut frame, &view, selected, workspace.reading, spinner);
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
        let width = frame.cols.saturating_sub(27);
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
            "Config",
            Color::Indexed(231),
            Color::Indexed(60),
            true,
        );
        write_text(
            &mut frame,
            width + 7,
            y,
            7,
            if workspace.reading == ReadingMode::Grid {
                " Tabs "
            } else {
                " Grid "
            },
            Color::Indexed(231),
            Color::Indexed(24),
            true,
        );
        write_text(
            &mut frame,
            width + 14,
            y,
            3,
            if matches!(workspace.reading, ReadingMode::Fullscreen(_)) {
                " ▣ "
            } else {
                " ⛶ "
            },
            Color::Indexed(231),
            Color::Indexed(30),
            true,
        );
        write_text(
            &mut frame,
            width + 17,
            y,
            7,
            "Layout ",
            Color::Indexed(231),
            Color::Indexed(24),
            true,
        );
        write_text(
            &mut frame,
            width + 24,
            y,
            3,
            if workspace.ui.notifications.iter().any(|item| !item.read) {
                " ! "
            } else {
                " ? "
            },
            Color::Indexed(231),
            Color::Indexed(24),
            true,
        );
    }
    if !workspace.ui.vitals.is_empty() && notice.is_none() {
        draw_vitals(&mut frame, &workspace.ui.vitals);
    }
    if workspace.help {
        let cols = frame.cols.saturating_sub(4).min(76);
        let rows = frame.rows.saturating_sub(2).min(30);
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
            "1–9       Select a tab (also Alt+1–9)".into(),
            format!("{}  Tabs / grid (also Alt+Tab / Alt+0)", key("z", "read")),
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
                "{} {}  Text width +10 / -10",
                key("+", "read-wider"),
                key("-", "read-narrower")
            ),
            format!("{}  Settings (tab placement)", key(",", "config")),
            format!("{}  Detach", key("d", "detach")),
            format!("{}  Take control", key("tT", "takeover")),
            format!("{}  Toggle help", key("?", "help")),
            format!("{}  Scroll: arrows/PgUp/PgDn; q leaves", key("[", "scroll")),
            format!("{}  Copy selection (also drag release)", key("y", "copy")),
            "Wheel/drag Scroll/select; Shift for mouse-aware apps".into(),
            "Click a tab number to choose its color".into(),
            format!(
                "{} / {} / {}  Overview / notifications / recovery",
                key("o", "overview"),
                key("b", "notifications"),
                key("r", "recovery")
            ),
            format!(
                "{} / {}  Next attention / next finished",
                key("N", "next-attention"),
                key("P", "next-finished")
            ),
            format!("{}  Persistent scratch terminal popup", key("c", "scratch")),
            format!(
                "{}  Search; Tab scope; Ctrl-Y copies result",
                key("/", "search")
            ),
            format!(
                "{}  Keyboard selection; arrows, y copies",
                key(" ", "selection")
            ),
            format!(
                "{} {} {} {}  Resize grid boundaries",
                key("H", "grid-narrower"),
                key("L", "grid-wider"),
                key("K", "grid-shorter"),
                key("J", "grid-taller")
            ),
            "Drag      Pane headers swap; separators resize".into(),
        ];
        for (i, line) in lines
            .iter()
            .enumerate()
            .skip(usize::from(workspace.ui.help_offset))
            .take(usize::from(rows.saturating_sub(2)))
        {
            write_text(
                &mut frame,
                rect.x + 2,
                rect.y + 2 + i as u16 - workspace.ui.help_offset,
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

fn draw_grid_separators(frame: &mut Frame, workspace: &Workspace) {
    let views = workspace.grid_layout();
    let columns: std::collections::BTreeSet<_> = views
        .iter()
        .filter(|view| view.rect.x > 0)
        .map(|view| view.rect.x - 1)
        .collect();
    let rows: std::collections::BTreeSet<_> = views
        .iter()
        .filter(|view| view.rect.y > 0)
        .map(|view| view.rect.y - 1)
        .collect();
    for y in 0..frame.rows.saturating_sub(1) {
        for x in 0..frame.cols {
            let glyph = match (columns.contains(&x), rows.contains(&y)) {
                (true, true) => "┼",
                (true, false) => "│",
                (false, true) => "─",
                _ => continue,
            };
            put(
                frame,
                x,
                y,
                Cell {
                    text: glyph.into(),
                    fg: Color::Indexed(245),
                    ..Cell::default()
                },
            );
        }
    }
}
fn header(
    frame: &mut Frame,
    view: &PaneView,
    selected: bool,
    _reading: ReadingMode,
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
    let buttons: &[(&str, u8, u8)] = &[(" ↗ ", 231, 24), (" ⛶ ", 16, 39)];
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
        DialogKind::Overview => " Workspace overview",
        DialogKind::Notifications => " Notifications",
        DialogKind::Recovery => {
            if workspace.ui.recovery_loading {
                " Recovery · loading…"
            } else {
                " Recovery"
            }
        }
        DialogKind::Search => " Search history",
        DialogKind::Move => " Move active pane · choose destination",
        DialogKind::Config => " Settings · tab position",
        DialogKind::CloseConfirm => " Close this tab?",
        DialogKind::Color => " Tab color · arrows, Enter or click",
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
    if dialog.kind == DialogKind::Color {
        let current = workspace
            .panes
            .iter()
            .find(|pane| Some(pane.id) == dialog.source)
            .map(|pane| usize::from(pane.palette));
        for (index, rect) in super::workspace::color_cards(bounds)
            .into_iter()
            .enumerate()
        {
            let (bright, muted) = PALETTE[index];
            let bg = Color::Indexed(muted);
            fill(frame, rect, bg);
            let label = format!(
                "{} {}",
                if current == Some(index) { "✓" } else { " " },
                super::ui::COLOR_NAMES[index]
            );
            write_text(
                frame,
                rect.x + 1,
                rect.y + rect.rows / 2,
                rect.cols.saturating_sub(2),
                &label,
                Color::Indexed(231),
                bg,
                true,
            );
            outline(
                frame,
                rect,
                Color::Indexed(if dialog.selection == index {
                    231
                } else {
                    bright
                }),
                bg,
            );
        }
    } else if dialog.kind == DialogKind::CloseConfirm {
        write_text(
            frame,
            bounds.x + 2,
            bounds.y + 2,
            bounds.cols - 4,
            "Closing stops this terminal and its running jobs.",
            Color::Indexed(231),
            Color::Indexed(236),
            true,
        );
        if let Some(pane) = workspace
            .panes
            .iter()
            .find(|pane| Some(pane.id) == dialog.source)
        {
            write_text(
                frame,
                bounds.x + 2,
                bounds.y + 3,
                bounds.cols - 4,
                &format!("{}  {}", pane.number, pane.title),
                Color::Indexed(250),
                Color::Indexed(236),
                false,
            );
        }
        for (index, rect) in super::workspace::close_buttons(bounds)
            .into_iter()
            .enumerate()
        {
            let selected = index == dialog.selection;
            let bg = Color::Indexed(match (index, selected) {
                (0, true) => 27,
                (0, false) => 24,
                (_, true) => 196,
                _ => 88,
            });
            fill(frame, rect, bg);
            let label = if index == 0 { "Cancel" } else { "Close tab" };
            let label = format!("{} {label}", if selected { "›" } else { " " });
            write_text(
                frame,
                rect.x + 1,
                rect.y + rect.rows / 2,
                rect.cols.saturating_sub(2),
                &label,
                Color::Indexed(231),
                bg,
                true,
            );
        }
    } else if dialog.kind == DialogKind::Config {
        let labels: Vec<String> = {
            use super::ui::TabPosition;
            [
                TabPosition::Auto,
                TabPosition::Top,
                TabPosition::Left,
                TabPosition::Bottom,
            ]
            .iter()
            .map(|position| {
                format!(
                    "{} {:?}",
                    if *position == workspace.ui.config.tab_position {
                        "●"
                    } else {
                        "○"
                    },
                    position
                )
            })
            .chain(std::iter::once(format!(
                "Notification sound: {}",
                if workspace.ui.config.notification_sound {
                    "on"
                } else {
                    "off"
                }
            )))
            .chain(std::iter::once(format!(
                "Wrap at reading width: {}",
                if workspace.ui.config.wrap_lines {
                    "on"
                } else {
                    "off"
                }
            )))
            .collect()
        };
        for (index, label) in labels.iter().enumerate() {
            let rect = Rect {
                x: bounds.x,
                y: bounds.y + 2 + index as u16,
                cols: bounds.cols,
                rows: 1,
            };
            let bg = Color::Indexed(if index == dialog.selection { 24 } else { 236 });
            fill(frame, rect, bg);
            write_text(
                frame,
                rect.x + 1,
                rect.y,
                rect.cols - 2,
                label,
                Color::Indexed(231),
                bg,
                true,
            );
        }
    } else if matches!(
        dialog.kind,
        DialogKind::Overview
            | DialogKind::Notifications
            | DialogKind::Recovery
            | DialogKind::Search
    ) {
        draw_dashboard(frame, workspace, dialog, bounds);
    } else if dialog.kind == DialogKind::Grid {
        let occupied = workspace
            .panes
            .iter()
            .filter(|p| !workspace.ui.unused.contains(&p.id))
            .filter(|p| Some(p.id) != workspace.ui.scratch)
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
            if let Some(pane) = workspace
                .panes
                .iter()
                .filter(|pane| Some(pane.id) != workspace.ui.scratch)
                .nth(index)
            {
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
    let mut output = String::from("\x1b]8;;\x1b\\\x1b[?25l");
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
            let mut hyperlink: Option<&str> = None;
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
                let target = cell.hyperlink.as_deref().filter(|uri| valid_hyperlink(uri));
                if target != hyperlink {
                    if hyperlink.is_some() {
                        output.push_str("\x1b]8;;\x1b\\");
                    }
                    if let Some(uri) = target {
                        let _ = write!(output, "\x1b]8;;{uri}\x1b\\");
                    }
                    hyperlink = target;
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
            if hyperlink.is_some() {
                output.push_str("\x1b]8;;\x1b\\");
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

fn draw_tabs(frame: &mut Frame, workspace: &Workspace, spinner: usize) {
    use super::{
        ui::TabPosition,
        workspace::{TabTarget, tab_targets},
    };
    let position = workspace
        .ui
        .config
        .tab_position
        .resolve(frame.cols, frame.rows);
    let area = match position {
        TabPosition::Left => Rect {
            x: 0,
            y: 0,
            cols: (frame.cols / 5).clamp(18, 28),
            rows: frame.rows - 1,
        },
        TabPosition::Bottom => Rect {
            x: 0,
            y: frame.rows - 2,
            cols: frame.cols,
            rows: 1,
        },
        _ => Rect {
            x: 0,
            y: 0,
            cols: frame.cols,
            rows: 1,
        },
    };
    fill(frame, area, Color::Indexed(234));
    let panes: Vec<_> = workspace
        .panes
        .iter()
        .filter(|pane| Some(pane.id) != workspace.ui.scratch)
        .collect();
    let active_id = if workspace.ui.scratch_visible {
        workspace.ui.scratch_return.unwrap_or(workspace.active)
    } else {
        workspace.active
    };
    let active = panes
        .iter()
        .position(|pane| pane.id == active_id)
        .unwrap_or(0);
    for (target, rect) in tab_targets(
        frame.cols,
        frame.rows,
        &workspace.ui.config,
        panes.len(),
        active,
    ) {
        match target {
            TabTarget::New | TabTarget::Wrap => {
                let wrapping = workspace.ui.config.wrap_lines
                    && !matches!(workspace.reading, ReadingMode::Fullscreen(_));
                let label = if target == TabTarget::Wrap {
                    if wrapping {
                        " Wrap: on "
                    } else {
                        " Wrap: off "
                    }
                } else if position == TabPosition::Left {
                    " + New tab "
                } else {
                    " + "
                };
                let enabled = target != TabTarget::New || panes.len() < 9;
                let bg = Color::Indexed(if enabled { 24 } else { 236 });
                fill(frame, rect, bg);
                write_text(
                    frame,
                    rect.x,
                    rect.y,
                    rect.cols,
                    label,
                    Color::Indexed(if enabled { 231 } else { 245 }),
                    bg,
                    true,
                );
            }
            TabTarget::Previous | TabTarget::Next => {
                let label = if position == TabPosition::Left && target == TabTarget::Previous {
                    " ↑ "
                } else if position == TabPosition::Left {
                    " ↓ "
                } else if target == TabTarget::Previous {
                    " ‹ "
                } else {
                    " › "
                };
                write_text(
                    frame,
                    rect.x,
                    rect.y,
                    rect.cols,
                    label,
                    Color::Indexed(231),
                    Color::Indexed(24),
                    true,
                );
            }
            TabTarget::Pane(index) => {
                if let Some(pane) = panes.get(index) {
                    let selected = pane.id == active_id;
                    let count = if position == TabPosition::Bottom {
                        format!(" {}/{} ", index + 1, panes.len())
                    } else {
                        String::new()
                    };
                    draw_tab(frame, workspace, pane, rect, selected, &count, spinner);
                }
            }
        }
    }
    let body = super::workspace::tab_body(
        frame.cols,
        frame.rows,
        &workspace.ui.config,
        matches!(workspace.reading, ReadingMode::Fullscreen(_)),
        workspace.ui.reading_size,
    );
    let x = body.x + body.cols + 1;
    if x < frame.cols {
        for y in body.y..body.y + body.rows {
            write_text(
                frame,
                x,
                y,
                1,
                if y == body.y { "+" } else { "│" },
                Color::Indexed(245),
                Color::Default,
                y == body.y,
            );
        }
    }
}

fn outline(frame: &mut Frame, rect: Rect, fg: Color, bg: Color) {
    if rect.cols < 2 || rect.rows == 0 {
        return;
    }
    for y in rect.y..rect.y + rect.rows {
        write_text(frame, rect.x, y, 1, "│", fg, bg, false);
        write_text(frame, rect.x + rect.cols - 1, y, 1, "│", fg, bg, false);
    }
    if rect.rows >= 3 {
        for (y, left, right) in [(rect.y, "┌", "┐"), (rect.y + rect.rows - 1, "└", "┘")] {
            write_text(frame, rect.x, y, 1, left, fg, bg, false);
            for x in rect.x + 1..rect.x + rect.cols - 1 {
                write_text(frame, x, y, 1, "─", fg, bg, false);
            }
            write_text(frame, rect.x + rect.cols - 1, y, 1, right, fg, bg, false);
        }
    }
}
fn draw_tab(
    frame: &mut Frame,
    workspace: &Workspace,
    pane: &PaneInfo,
    rect: Rect,
    selected: bool,
    count: &str,
    spinner: usize,
) {
    let (bright, muted) = PALETTE[usize::from(pane.palette) % PALETTE.len()];
    let bg = Color::Indexed(234);
    fill(frame, rect, bg);
    let number = super::workspace::tab_number(rect);
    write_text(
        frame,
        number.x,
        number.y,
        number.cols,
        &format!(" {} ", pane.number),
        Color::Indexed(if selected { 226 } else { 117 }),
        Color::Indexed(muted),
        true,
    );
    let marker = if pane.status == crate::model::Status::Working {
        SPINNER[spinner % 8].to_string()
    } else {
        pane.status.marker().to_owned()
    };
    let marker = format!(
        "{}{}",
        if workspace
            .ui
            .notifications
            .iter()
            .any(|item| item.pane == pane.id && !item.read)
        {
            "● "
        } else {
            ""
        },
        marker
    );
    let marker_x = number.x + number.cols + 2;
    let marker_width: u16 = marker.chars().map(char_width).sum();
    let close = super::workspace::tab_close(rect);
    write_text(
        frame,
        marker_x,
        number.y,
        close.x.saturating_sub(marker_x),
        &marker,
        Color::Indexed(bright),
        bg,
        true,
    );
    let count_width = count.len() as u16;
    let (title_x, title_y, title_width) = if rect.rows >= 4 {
        (rect.x + 2, rect.y + 2, rect.cols.saturating_sub(4))
    } else {
        let x = marker_x + marker_width + 1;
        (
            x,
            number.y,
            close.x.saturating_sub(count_width).saturating_sub(x),
        )
    };
    let title = marquee_title(
        &pane.title,
        title_width,
        workspace.ui.title_ticks.get(&pane.id).copied().unwrap_or(0),
    );
    write_text(
        frame,
        title_x,
        title_y,
        title_width,
        &title,
        Color::Indexed(231),
        bg,
        false,
    );
    if count_width > 0 {
        write_text(
            frame,
            close.x.saturating_sub(count_width),
            number.y,
            count_width,
            count,
            Color::Indexed(231),
            bg,
            true,
        );
    }
    write_text(
        frame,
        close.x,
        close.y,
        close.cols,
        " × ",
        Color::Indexed(231),
        Color::Indexed(88),
        true,
    );
    if selected {
        outline(frame, rect, Color::Indexed(bright), bg);
    } else {
        for y in rect.y..rect.y + rect.rows {
            write_text(frame, rect.x, y, 1, "│", Color::Indexed(bright), bg, false);
        }
    }
}

fn marquee_title(title: &str, width: u16, ticks: u16) -> String {
    let title = safe_text(title);
    let chars: Vec<_> = title.chars().take(512).collect();
    let total: u16 = chars.iter().map(|&c| char_width(c)).sum();
    let overflow = total.saturating_sub(width);
    let travel = u32::from(overflow) * 3;
    let elapsed = u32::from(ticks);
    let offset = if width == 0 || overflow == 0 || elapsed <= 12 || elapsed > travel + 28 {
        0
    } else {
        ((elapsed - 12) / 3).min(u32::from(overflow)) as u16
    };
    let mut skipped = 0;
    chars
        .into_iter()
        .skip_while(|&c| {
            if skipped < offset {
                skipped += char_width(c);
                true
            } else {
                false
            }
        })
        .collect()
}
fn draw_vitals(frame: &mut Frame, segments: &[(String, u8)]) {
    let start = if frame.controller { 0 } else { 10 };
    let end = frame.cols.saturating_sub(super::ui::FOOTER_COLS);
    let available = end.saturating_sub(start);
    let selected: Vec<_> = if available < 50 {
        segments
            .iter()
            .enumerate()
            .filter(|(index, _)| *index < 2 || *index == segments.len() - 1)
            .map(|(_, segment)| segment)
            .collect()
    } else {
        segments.iter().collect()
    };
    let total: u16 = selected
        .iter()
        .map(|(text, _)| text.chars().map(char_width).sum::<u16>() + 1)
        .sum();
    let mut x = start + available.saturating_sub(total);
    fill(
        frame,
        Rect {
            x: start,
            y: frame.rows - 1,
            cols: available,
            rows: 1,
        },
        Color::Indexed(234),
    );
    for (text, color) in selected {
        if x >= end {
            break;
        }
        let width = text.chars().map(char_width).sum::<u16>();
        write_text(
            frame,
            x,
            frame.rows - 1,
            (end - x).min(width),
            text,
            Color::Indexed(*color),
            Color::Indexed(234),
            false,
        );
        x = x.saturating_add(width + 1);
    }
}
fn draw_dashboard(
    frame: &mut Frame,
    workspace: &Workspace,
    dialog: super::ui::Dialog,
    bounds: Rect,
) {
    use super::ui::DialogKind;
    let start_row = if dialog.kind == DialogKind::Search {
        4
    } else {
        2
    };
    if dialog.kind == DialogKind::Search {
        write_text(
            frame,
            bounds.x + 1,
            bounds.y + 2,
            bounds.cols - 2,
            &format!("Find: {}", workspace.ui.search_query),
            Color::Indexed(231),
            Color::Indexed(236),
            true,
        );
        write_text(
            frame,
            bounds.x + 1,
            bounds.y + 3,
            bounds.cols - 2,
            if workspace.ui.search_all {
                "All panes · Tab toggles · Ctrl-Y copies"
            } else {
                "This pane · Tab toggles · Ctrl-Y copies"
            },
            Color::Indexed(117),
            Color::Indexed(236),
            false,
        );
    }
    let labels: Vec<String> = match dialog.kind {
        DialogKind::Overview => workspace
            .panes
            .iter()
            .filter(|pane| Some(pane.id) != workspace.ui.scratch)
            .map(|pane| {
                format!(
                    "{} {} {} · {} · session {} · {} · {} · {}{}",
                    pane.number,
                    pane.status.marker(),
                    pane.title,
                    pane.status.text(),
                    pane.session_id.as_deref().unwrap_or("unknown"),
                    pane.cwd,
                    if pane.last_activity == 0 {
                        "activity unknown".into()
                    } else {
                        format!("{}s ago", crate::now().saturating_sub(pane.last_activity))
                    },
                    pane.observation.source.label(),
                    if pane.observation.stale(crate::now()) {
                        " (stale/unknown check)"
                    } else {
                        ""
                    }
                )
            })
            .collect(),
        DialogKind::Notifications => workspace
            .ui
            .notifications
            .iter()
            .map(|item| {
                format!(
                    "{} {} {} · {} · pane {}",
                    if item.read { "○" } else { "●" },
                    item.status.marker(),
                    item.title,
                    item.workspace,
                    item.pane
                )
            })
            .collect(),
        DialogKind::Recovery => workspace
            .ui
            .recovery
            .iter()
            .map(|item| {
                format!(
                    "{} · {} · {} · session {} · Enter/y copies resume",
                    item.workspace,
                    item.entry.cwd,
                    item.entry.name,
                    item.entry.session_id.as_deref().unwrap_or("unknown")
                )
            })
            .collect(),
        DialogKind::Search => workspace
            .ui
            .search_hits
            .iter()
            .map(|hit| format!("{}:{} {}", hit.pane, hit.line, hit.text))
            .collect(),
        _ => vec![],
    };
    let visible = usize::from(bounds.rows.saturating_sub(start_row));
    let offset = dialog.selection.saturating_sub(visible.saturating_sub(1));
    if labels.is_empty() && visible > 0 {
        write_text(
            frame,
            bounds.x + 1,
            bounds.y + start_row,
            bounds.cols - 2,
            "No matching entries",
            Color::Indexed(245),
            Color::Indexed(236),
            false,
        );
    }
    for (index, label) in labels.iter().enumerate().skip(offset).take(visible) {
        let rect = Rect {
            x: bounds.x,
            y: bounds.y + start_row + (index - offset) as u16,
            cols: bounds.cols,
            rows: 1,
        };
        let bg = Color::Indexed(if index == dialog.selection { 24 } else { 236 });
        fill(frame, rect, bg);
        write_text(
            frame,
            rect.x + 1,
            rect.y,
            rect.cols - 2,
            label,
            Color::Indexed(231),
            bg,
            index == dialog.selection,
        );
    }
    frame.cursor = None;
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
                (ReadingMode::Capped(1), true, 6),
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
