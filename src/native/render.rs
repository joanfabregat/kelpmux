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
    if workspace.reading != ReadingMode::Grid || workspace.active == 0 {
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
    if workspace.active == 0 && !workspace.ui.scratch_visible {
        let body = super::workspace::tab_body(
            workspace.cols,
            workspace.rows,
            &workspace.ui.config,
            false,
            None,
        );
        write_text(
            &mut frame,
            body.x,
            body.y + 2,
            body.cols,
            "All tabs are parked. Ctrl-B u restores a tab; Ctrl-B Enter creates one.",
            Color::Indexed(250),
            Color::Default,
            false,
        );
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
            Color::Indexed(super::ui::FOOTER_BG),
        );
        let label = if controller {
            format!(
                "^{}: {} tabs/grid · Enter new · p park · {} detach",
                char::from(b'A' + workspace.ui.config.prefix - 1),
                binding_label(&workspace.ui.config, "z", "read"),
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
        let width = frame.cols.saturating_sub(super::ui::FOOTER_COLS);
        write_text(
            &mut frame,
            super::ui::FOOTER_COLS,
            y,
            width,
            &text,
            Color::Indexed(250),
            Color::Indexed(super::ui::FOOTER_BG),
            false,
        );
        write_text(
            &mut frame,
            0,
            y,
            8,
            " Config ",
            Color::Indexed(231),
            Color::Indexed(24),
            true,
        );
        fill(
            &mut frame,
            Rect {
                x: 8,
                y,
                cols: 6,
                rows: 1,
            },
            Color::Indexed(67),
        );
        write_text(
            &mut frame,
            8,
            y,
            6,
            if workspace.active == 0 {
                " New "
            } else if workspace.reading == ReadingMode::Grid {
                " Tabs "
            } else {
                " Grid "
            },
            Color::Indexed(231),
            Color::Indexed(67),
            true,
        );
        write_text(
            &mut frame,
            14,
            y,
            3,
            if workspace.ui.notifications.iter().any(|item| !item.read) {
                " ! "
            } else {
                " ? "
            },
            Color::Indexed(231),
            Color::Indexed(
                if workspace.ui.notifications.iter().any(|item| !item.read) {
                    160
                } else {
                    94
                },
            ),
            true,
        );
    }
    if !workspace.ui.vitals.is_empty() && notice.is_none() {
        draw_vitals(&mut frame, &workspace.ui.vitals);
    }
    if workspace.help || workspace.ui.dialog.is_some() || workspace.ui.scratch_visible {
        let keep =
            if workspace.ui.scratch_visible && !workspace.help && workspace.ui.dialog.is_none() {
                Some(super::workspace::scratch_rect(frame.cols, frame.rows))
            } else {
                None
            };
        dim_background(&mut frame, keep);
    }
    if workspace.help {
        draw_help(&mut frame, workspace);
    }
    if let Some(dialog) = workspace.ui.dialog {
        draw_dialog(&mut frame, workspace, dialog, spinner);
    }
    repair_wide_edges(&mut frame);
    frame
}

fn help_entries(config: &super::ui::UiConfig) -> Vec<(String, &'static str)> {
    let key = |default, action| binding_label(config, default, action);
    vec![
        ("1–9".into(), "Select tab (also Alt+1–9)"),
        (key("z", "read"), "Tabs / grid (also Alt+Tab)"),
        (key("g/e", "grid"), "Automatic grid view"),
        (key("f", "fullscreen"), "Toggle reading-width limit"),
        ("Enter".into(), "New tab"),
        ("p".into(), "Park tab (confirm)"),
        ("u".into(), "Show parked tabs"),
        (key("s", "move"), "Move pane to a destination"),
        (key("a", "agents"), "Agent picker"),
        (key("!", "attention"), "Agents needing attention"),
        (key("n", "finished"), "Finished agents"),
        (
            format!(
                "{} {} {} {}",
                key("h", "focus-left"),
                key("j", "focus-down"),
                key("k", "focus-up"),
                key("l", "focus-right")
            ),
            "Focus left/down/up/right",
        ),
        (
            format!("{} / {}", key("+", "read-wider"), key("-", "read-narrower")),
            "Text width +10 / −10 cols",
        ),
        (key(",", "config"), "Settings"),
        (key("d", "detach"), "Detach; keep jobs running"),
        (key("tT", "takeover"), "Take control"),
        (key("?", "help"), "Toggle help"),
        (key("[", "scroll"), "Scroll mode; arrows/PgUp/PgDn"),
        (key("y", "copy"), "Copy selection"),
        ("Wheel / drag".into(), "Scroll/select; Shift for apps"),
        ("Tab number".into(), "Choose tab color (click)"),
        (key("o", "overview"), "Workspace overview"),
        (key("b", "notifications"), "Notifications"),
        (key("r", "recovery"), "Recovery journal"),
        (
            format!(
                "{} / {}",
                key("N", "next-attention"),
                key("P", "next-finished")
            ),
            "Next attention / finished",
        ),
        (key("c", "scratch"), "Scratch terminal popup"),
        (key("/", "search"), "Search; Tab scope; Ctrl-Y copy"),
        (key("Space", "selection"), "Select with arrows; y copies"),
        ("Header drag".into(), "Swap panes; grid auto-balances"),
    ]
}
fn draw_help(frame: &mut Frame, workspace: &Workspace) {
    let rect = super::input::help_bounds(frame.cols, frame.rows);
    fill(frame, rect, Color::Indexed(236));
    let title = format!(
        " Help · Ctrl-{}, then key",
        char::from(b'A' + workspace.ui.config.prefix - 1)
    );
    let header_bg = Color::Indexed(if workspace.ui.notifications.iter().any(|n| !n.read) {
        160
    } else {
        94
    });
    fill(frame, Rect { rows: 1, ..rect }, header_bg);
    write_text(
        frame,
        rect.x + 1,
        rect.y,
        rect.cols.saturating_sub(5),
        &title,
        Color::Indexed(231),
        header_bg,
        true,
    );
    write_text(
        frame,
        rect.x + rect.cols - 3,
        rect.y,
        3,
        " × ",
        Color::Indexed(231),
        Color::Indexed(88),
        true,
    );
    let key_width = (rect.cols / 3).clamp(10, 22);
    let action_x = rect.x + key_width + 3;
    let action_width = rect.cols.saturating_sub(key_width + 4);
    write_text(
        frame,
        rect.x + 1,
        rect.y + 1,
        key_width,
        "Key",
        Color::Indexed(117),
        Color::Indexed(236),
        true,
    );
    write_text(
        frame,
        action_x,
        rect.y + 1,
        action_width,
        "Action",
        Color::Indexed(117),
        Color::Indexed(236),
        true,
    );
    let entries = help_entries(&workspace.ui.config);
    let visible = usize::from(rect.rows.saturating_sub(3));
    let offset = usize::from(workspace.ui.help_offset).min(entries.len().saturating_sub(visible));
    for (index, (key, action)) in entries.iter().enumerate().skip(offset).take(visible) {
        let y = rect.y + 2 + (index - offset) as u16;
        let bg = Color::Indexed(if index % 2 == 0 { 235 } else { 237 });
        fill(
            frame,
            Rect {
                x: rect.x,
                y,
                cols: rect.cols,
                rows: 1,
            },
            bg,
        );
        write_text(
            frame,
            rect.x + 1,
            y,
            key_width,
            key,
            Color::Indexed(117),
            bg,
            true,
        );
        write_text(
            frame,
            rect.x + key_width + 1,
            y,
            1,
            "│",
            Color::Indexed(242),
            bg,
            false,
        );
        write_text(
            frame,
            action_x,
            y,
            action_width,
            action,
            Color::Indexed(252),
            bg,
            false,
        );
    }
    let hint = format!(
        " ↑↓ scroll · Esc close · {}–{}/{}",
        offset + 1,
        (offset + visible).min(entries.len()),
        entries.len()
    );
    write_text(
        frame,
        rect.x,
        rect.y + rect.rows - 1,
        rect.cols,
        &hint,
        Color::Indexed(250),
        Color::Indexed(236),
        false,
    );
    frame.cursor = None;
}
fn draw_grid_separators(frame: &mut Frame, workspace: &Workspace) {
    for view in workspace.grid_layout().iter().filter(|v| !v.pane.parked) {
        if view.rect.y > 0 {
            write_text(
                frame,
                0,
                view.rect.y - 1,
                frame.cols,
                &"─".repeat(usize::from(frame.cols)),
                Color::Indexed(245),
                Color::Default,
                false,
            );
        }
        if view.rect.x > 0 {
            for y in view.rect.y..view.rect.y + view.rect.rows {
                write_text(
                    frame,
                    view.rect.x - 1,
                    y,
                    1,
                    "▎",
                    Color::Indexed(245),
                    Color::Default,
                    false,
                );
            }
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
    let activity = format!(" {marker} ");
    write_text(
        frame,
        view.rect.x + badge_width,
        view.rect.y,
        title_width.min(3),
        &activity,
        Color::Indexed(16),
        Color::Indexed(bright),
        true,
    );
    write_text(
        frame,
        view.rect.x + badge_width + 3,
        view.rect.y,
        title_width.saturating_sub(3),
        &format!(" {}", safe_text(&view.pane.title)),
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
fn draw_dialog(
    frame: &mut Frame,
    workspace: &Workspace,
    dialog: super::ui::Dialog,
    spinner: usize,
) {
    use super::ui::{AgentFilter, DialogKind};
    let bounds = super::workspace::dialog_bounds_for(frame.cols, frame.rows, dialog.kind);
    fill(frame, bounds, Color::Indexed(236));
    let header_bg = Color::Indexed(
        if matches!(
            dialog.kind,
            DialogKind::CloseConfirm | DialogKind::ParkConfirm | DialogKind::RestoreConfirm
        ) {
            workspace
                .panes
                .iter()
                .find(|p| Some(p.id) == dialog.source)
                .map(|p| PALETTE[usize::from(p.palette) % PALETTE.len()].1)
                .unwrap_or(24)
        } else {
            24
        },
    );
    fill(frame, Rect { rows: 1, ..bounds }, header_bg);
    let title = match dialog.kind {
        DialogKind::Parked => " Parked tabs · select to restore",
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
        DialogKind::Config => " Settings",
        DialogKind::CloseConfirm => " Close this tab?",
        DialogKind::ParkConfirm => " ⊟ Park this tab?",
        DialogKind::RestoreConfirm => " ↥ Restore this tab?",
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
        header_bg,
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
    } else if matches!(
        dialog.kind,
        DialogKind::CloseConfirm | DialogKind::ParkConfirm | DialogKind::RestoreConfirm
    ) {
        write_text(
            frame,
            bounds.x + 2,
            bounds.y + 2,
            bounds.cols - 4,
            match dialog.kind {
                DialogKind::ParkConfirm => "Hides the tab; its jobs keep running.",
                DialogKind::RestoreConfirm => "Returns this tab to the tab bar.",
                _ => "Stops this terminal and its jobs.",
            },
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
                &format!(
                    "{}  {}",
                    if pane.parked {
                        format!("#{}", pane.id)
                    } else {
                        pane.number.to_string()
                    },
                    pane.title
                ),
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
                (_, true) if dialog.kind == DialogKind::CloseConfirm => 196,
                _ if dialog.kind == DialogKind::CloseConfirm => 88,
                (_, true) if dialog.kind == DialogKind::ParkConfirm => 136,
                _ if dialog.kind == DialogKind::ParkConfirm => 58,
                (_, true) => 35,
                _ => 29,
            });
            fill(frame, rect, bg);
            let label = if index == 0 {
                "Cancel"
            } else {
                match dialog.kind {
                    DialogKind::ParkConfirm => "⊟ Park tab",
                    DialogKind::RestoreConfirm => "↥ Restore tab",
                    _ => "Close tab",
                }
            };
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
        use super::ui::TabPosition;
        let positions = [
            TabPosition::Auto,
            TabPosition::Top,
            TabPosition::Left,
            TabPosition::Bottom,
        ];
        let (step, height) = super::workspace::config_geometry(bounds);
        for (group, label) in ["Tab placement", "Reading width", "Notifications"]
            .iter()
            .enumerate()
        {
            fill(
                frame,
                Rect {
                    x: bounds.x + 1,
                    y: bounds.y + 1 + group as u16 * step,
                    cols: bounds.cols - 2,
                    rows: height + 1,
                },
                Color::Indexed(235),
            );
            write_text(
                frame,
                bounds.x + 2,
                bounds.y + 1 + group as u16 * step,
                bounds.cols - 4,
                label,
                Color::Indexed(117),
                Color::Indexed(235),
                true,
            );
        }
        let wrap = workspace.ui.config.wrap_lines
            && !matches!(workspace.reading, ReadingMode::Fullscreen(_));
        for (index, value, rect) in super::workspace::config_targets(bounds) {
            let (checked, label) = match index {
                0..=3 => (
                    positions[index] == workspace.ui.config.tab_position,
                    if rect.cols < 9 && index == 3 {
                        "Btm".into()
                    } else {
                        format!("{:?}", positions[index])
                    },
                ),
                4 => (
                    value == Some(wrap),
                    if value == Some(true) { "On" } else { "Off" }.to_owned(),
                ),
                _ => (
                    value == Some(workspace.ui.config.notification_sound),
                    if value == Some(true) { "On" } else { "Off" }.to_owned(),
                ),
            };
            let label = format!("{} {label}", if checked { "●" } else { "○" });
            let bg = Color::Indexed(if index == dialog.selection && (index < 4 || checked) {
                24
            } else if checked {
                29
            } else {
                238
            });
            fill(frame, rect, bg);
            write_text(
                frame,
                rect.x + 1,
                rect.y + rect.rows / 2,
                rect.cols.saturating_sub(1),
                &label,
                Color::Indexed(231),
                bg,
                true,
            );
        }
        let inner = bounds.cols - 4;
        write_text(
            frame,
            bounds.x + 2,
            bounds.y + 2 + step + height / 2,
            inner / 2 - 1,
            &format!(
                "Limit: {} cols",
                workspace.ui.config.remembered_read_widths[super::ui::geometry_bucket(frame.cols)]
                    .unwrap_or(workspace.ui.config.read_width)
            ),
            Color::Indexed(250),
            Color::Indexed(235),
            false,
        );
        write_text(
            frame,
            bounds.x + 2,
            bounds.y + 2 + 2 * step + height / 2,
            inner / 2 - 1,
            "Sound",
            Color::Indexed(250),
            Color::Indexed(235),
            false,
        );
    } else if dialog.kind == DialogKind::Parked {
        draw_parked(frame, workspace, dialog, bounds, spinner);
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
fn dim_color(color: Color, foreground: bool) -> Color {
    let (r, g, b) = match color {
        Color::Default => {
            if foreground {
                (208, 208, 208)
            } else {
                (16, 16, 16)
            }
        }
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(index) => {
            const ANSI: [(u8, u8, u8); 16] = [
                (0, 0, 0),
                (128, 0, 0),
                (0, 128, 0),
                (128, 128, 0),
                (0, 0, 128),
                (128, 0, 128),
                (0, 128, 128),
                (192, 192, 192),
                (128, 128, 128),
                (255, 0, 0),
                (0, 255, 0),
                (255, 255, 0),
                (0, 0, 255),
                (255, 0, 255),
                (0, 255, 255),
                (255, 255, 255),
            ];
            if index < 16 {
                ANSI[usize::from(index)]
            } else if index >= 232 {
                let v = 8 + (index - 232) * 10;
                (v, v, v)
            } else {
                let n = index - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                (level(n / 36), level(n / 6 % 6), level(n % 6))
            }
        }
    };
    let darken = |v: u8| (u16::from(v) * 42 / 100) as u8;
    Color::Rgb(darken(r), darken(g), darken(b))
}
fn dim_background(frame: &mut Frame, keep: Option<Rect>) {
    for (index, cell) in frame.cells.iter_mut().enumerate() {
        let point = Point {
            x: (index % usize::from(frame.cols)) as u16,
            y: (index / usize::from(frame.cols)) as u16,
        };
        if keep.is_some_and(|rect| rect.contains(point)) {
            continue;
        }
        cell.fg = dim_color(cell.fg, true);
        cell.bg = dim_color(cell.bg, false);
        cell.bold = false;
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
            y: frame.rows - 3,
            cols: frame.cols,
            rows: 2,
        },
        _ => Rect {
            x: 0,
            y: 0,
            cols: frame.cols,
            rows: 2,
        },
    };
    fill(frame, area, Color::Indexed(234));
    let panes: Vec<_> = workspace
        .panes
        .iter()
        .filter(|pane| !pane.parked && Some(pane.id) != workspace.ui.scratch)
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
            TabTarget::Parked => {
                let label = format!(
                    " ⊟ Parked {} ",
                    workspace.panes.iter().filter(|p| p.parked).count()
                );
                fill(frame, rect, Color::Indexed(58));
                write_text(
                    frame,
                    rect.x,
                    rect.y + rect.rows / 2,
                    rect.cols,
                    &label,
                    Color::Indexed(231),
                    Color::Indexed(58),
                    true,
                );
            }
            TabTarget::New => {
                let label = if position == TabPosition::Left {
                    " + New tab "
                } else {
                    " + "
                };
                let enabled = workspace
                    .panes
                    .iter()
                    .filter(|p| Some(p.id) != workspace.ui.scratch)
                    .count()
                    < 9;
                let bg = Color::Indexed(if enabled { 238 } else { 236 });
                fill(frame, rect, bg);
                write_text(
                    frame,
                    rect.x,
                    rect.y + rect.rows / 2,
                    rect.cols,
                    label,
                    Color::Indexed(if enabled { 231 } else { 245 }),
                    bg,
                    true,
                );
            }
            TabTarget::Previous | TabTarget::Next => {
                fill(frame, rect, Color::Indexed(67));
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
                    rect.y + rect.rows / 2,
                    rect.cols,
                    label,
                    Color::Indexed(231),
                    Color::Indexed(67),
                    true,
                );
            }
            TabTarget::Pane(index) => {
                if let Some(pane) = panes.get(index) {
                    let selected = pane.id == active_id;
                    let count = if position == TabPosition::Bottom
                        && panes.len() * 18 + 3 > usize::from(frame.cols)
                        && frame.cols.saturating_sub(9) < 36
                    {
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
    let active = panes.iter().find(|pane| pane.id == active_id);
    let (bright, muted) = active
        .map(|pane| PALETTE[usize::from(pane.palette) % PALETTE.len()])
        .unwrap_or(PALETTE[0]);
    if position == TabPosition::Left {
        let x = body.x - 1;
        for y in 0..frame.rows - 1 {
            write_text(
                frame,
                x,
                y,
                1,
                "▎",
                Color::Indexed(muted),
                Color::Default,
                false,
            );
        }
    }
    let wrapping =
        workspace.ui.config.wrap_lines && !matches!(workspace.reading, ReadingMode::Fullscreen(_));
    let x = body.x + body.cols;
    if x < frame.cols
        && super::workspace::reading_limit_effective(frame.cols, frame.rows, &workspace.ui.config)
    {
        for y in body.y..body.y + body.rows {
            write_text(
                frame,
                x,
                y,
                1,
                if y == body.y {
                    if wrapping { "+" } else { "−" }
                } else {
                    "▎"
                },
                Color::Indexed(bright),
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
    let bg = Color::Indexed(if selected { muted } else { 234 });
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
        " {}{} ",
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
    let park = super::workspace::tab_park(rect);
    write_text(
        frame,
        marker_x,
        number.y,
        close.x.saturating_sub(marker_x),
        &marker,
        Color::Indexed(16),
        Color::Indexed(bright),
        true,
    );
    let count_width = count.len() as u16;
    let (title_x, title_y, title_width) = if rect.rows >= 2 {
        (
            rect.x + 2,
            rect.y + 1,
            rect.cols.saturating_sub(4 + count_width),
        )
    } else {
        let x = marker_x + marker_width + 1;
        (
            x,
            number.y,
            park.x.saturating_sub(count_width).saturating_sub(x),
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
            if rect.rows >= 2 {
                rect.x + rect.cols.saturating_sub(2 + count_width)
            } else {
                park.x.saturating_sub(count_width)
            },
            if rect.rows >= 2 { rect.y + 1 } else { number.y },
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
    write_text(
        frame,
        park.x,
        park.y,
        park.cols,
        " ⊟ ",
        Color::Indexed(231),
        Color::Indexed(58),
        true,
    );
    for y in rect.y..rect.y + rect.rows {
        write_text(frame, rect.x, y, 1, "▎", Color::Indexed(bright), bg, false);
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
    let start = (super::ui::FOOTER_COLS + if frame.controller { 0 } else { 10 }).min(frame.cols);
    let end = frame.cols;
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
        Color::Indexed(super::ui::FOOTER_BG),
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
            Color::Indexed(super::ui::FOOTER_BG),
            false,
        );
        x = x.saturating_add(width + 1);
    }
}
fn draw_parked(
    frame: &mut Frame,
    workspace: &Workspace,
    dialog: super::ui::Dialog,
    bounds: Rect,
    spinner: usize,
) {
    let status_x = bounds.x + bounds.cols.saturating_sub(22);
    let restore_x = bounds.x + bounds.cols.saturating_sub(5);
    let title_x = bounds.x + 9;
    for (x, width, label) in [
        (bounds.x + 2, 6, "Tab"),
        (title_x, status_x.saturating_sub(title_x), "Name"),
        (status_x, 16, "Status"),
        (restore_x, 3, " ↥ "),
    ] {
        write_text(
            frame,
            x,
            bounds.y + 1,
            width,
            label,
            Color::Indexed(250),
            Color::Indexed(236),
            true,
        );
    }
    let panes: Vec<_> = workspace.panes.iter().filter(|p| p.parked).collect();
    if panes.is_empty() {
        write_text(
            frame,
            bounds.x + 2,
            bounds.y + 2,
            bounds.cols.saturating_sub(4),
            "No parked tabs",
            Color::Indexed(250),
            Color::Indexed(236),
            false,
        );
    }
    for (index, rect) in super::workspace::parked_cards(bounds, dialog.selection, panes.len()) {
        let pane = panes[index];
        let y = rect.y + 1;
        let selected = index == dialog.selection;
        let (bright, muted) = PALETTE[usize::from(pane.palette) % PALETTE.len()];
        let bg = Color::Indexed(if selected { muted } else { 234 });
        fill(frame, rect, bg);
        for row in rect.y..rect.y + rect.rows {
            write_text(
                frame,
                bounds.x,
                row,
                1,
                "▎",
                Color::Indexed(bright),
                bg,
                false,
            );
        }
        write_text(
            frame,
            bounds.x + 1,
            y,
            4,
            &format!(" {} ", pane.id),
            Color::Indexed(if selected { 226 } else { 117 }),
            bg,
            true,
        );
        let marker = if pane.status == crate::model::Status::Working {
            SPINNER[spinner % SPINNER.len()].to_string()
        } else {
            pane.status.marker().to_owned()
        };
        write_text(
            frame,
            bounds.x + 6,
            y,
            3,
            &format!(" {marker} "),
            Color::Indexed(16),
            Color::Indexed(bright),
            true,
        );
        let title_width = status_x.saturating_sub(title_x + 1);
        let title = marquee_title(
            &pane.title,
            title_width,
            workspace.ui.title_ticks.get(&pane.id).copied().unwrap_or(0),
        );
        write_text(
            frame,
            title_x,
            y,
            title_width,
            &title,
            Color::Indexed(231),
            bg,
            false,
        );
        write_text(
            frame,
            status_x,
            y,
            16,
            pane.status.text(),
            Color::Indexed(bright),
            bg,
            false,
        );
        write_text(
            frame,
            restore_x,
            y,
            3,
            " ↥ ",
            Color::Indexed(231),
            Color::Indexed(29),
            true,
        );
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
                assert_eq!(frame.cells[5].bg, Color::Indexed(39));
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
