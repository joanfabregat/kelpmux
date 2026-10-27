//! Native grid/tab composition and ANSI damage output. Grid header controls
//! share tab styling; navigation and controls use workspace geometry.
use super::{engine::safe_text, protocol::*, ui::PALETTE, workspace::Workspace};
use std::{collections::BTreeMap, fmt::Write};
use unicode_width::UnicodeWidthChar;

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
    if workspace.reading == ReadingMode::Grid && workspace.active != 0 {
        draw_grid_separators(&mut frame, workspace);
        let (_, count) = workspace.grid_page();
        for rect in workspace.grid_slots().into_iter().skip(count) {
            let bg = theme_surface(workspace, 8);
            fill(&mut frame, rect, bg);
            write_text(
                &mut frame,
                rect.x + rect.cols.saturating_sub(8) / 2,
                rect.y + rect.rows / 2,
                rect.cols.min(8),
                "Inactive",
                theme_surface(workspace, 50),
                bg,
                false,
            );
        }
    } else if !matches!(workspace.reading, ReadingMode::Fullscreen(_)) {
        draw_tabs(&mut frame, workspace, spinner);
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
                        if workspace.reading == ReadingMode::Grid && !selected && !view.overlay {
                            cell.fg = inactive_color(cell.fg, true, workspace.ui.terminal_theme);
                            cell.bg = inactive_color(cell.bg, false, workspace.ui.terminal_theme);
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
        } else if workspace.reading == ReadingMode::Grid
            || matches!(workspace.reading, ReadingMode::Fullscreen(id) if id == view.pane.id)
        {
            header(&mut frame, workspace, &view, selected, spinner);
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
    if frame.rows > 0 && !frame.sidebar_status() {
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
                "Obs · Ctrl-{} {} take control",
                char::from(b'A' + workspace.ui.config.prefix - 1),
                binding_label(&workspace.ui.config, "tT", "takeover")
            )
        };
        let mut text = label;
        if let Some(notice) = notice {
            text.push_str(" · ");
            text.push_str(&safe_text(notice));
        }
        let parked_count = workspace.panes.iter().filter(|pane| pane.parked).count();
        let help = super::workspace::help_button(frame.rows, parked_count);
        let [config, mode, _, _] = frame.controls();
        let footer_cols = frame.footer_cols();
        if workspace.ui.show_dev_badge {
            draw_dev_badge(&mut frame, help.x + help.cols, y);
        }
        let width = frame.cols.saturating_sub(footer_cols);
        write_text(
            &mut frame,
            footer_cols,
            y,
            width,
            &text,
            Color::Indexed(250),
            Color::Indexed(super::ui::FOOTER_BG),
            false,
        );
        write_text(
            &mut frame,
            config.x,
            config.y,
            config.cols,
            super::ui::CONFIG_BUTTON_LABEL,
            Color::Indexed(231),
            Color::Indexed(24),
            true,
        );
        fill(&mut frame, mode, Color::Indexed(67));
        write_text(
            &mut frame,
            mode.x,
            mode.y,
            mode.cols,
            if workspace.active == 0 {
                " + New "
            } else if workspace.reading == ReadingMode::Grid {
                " ▦ Tabs "
            } else {
                " ▦ Grid "
            },
            Color::Indexed(231),
            Color::Indexed(67),
            true,
        );
        write_text(
            &mut frame,
            help.x,
            help.y,
            help.cols,
            if super::ui::has_unread_notifications(&workspace.ui, &workspace.panes) {
                " ⚠︎ "
            } else {
                " ? "
            },
            Color::Indexed(231),
            Color::Indexed(
                if super::ui::has_unread_notifications(&workspace.ui, &workspace.panes) {
                    160
                } else {
                    94
                },
            ),
            true,
        );
        let parked = super::workspace::parked_button(frame.rows, parked_count);
        fill(&mut frame, parked, Color::Indexed(super::ui::PARKED_BG));
        write_text(
            &mut frame,
            parked.x,
            parked.y,
            parked.cols,
            &super::workspace::parked_label(parked_count),
            Color::Indexed(231),
            Color::Indexed(super::ui::PARKED_BG),
            true,
        );
    }
    if frame.sidebar_status() {
        draw_sidebar_status(&mut frame, workspace, notice);
    } else if !workspace.ui.vitals.is_empty() && notice.is_none() {
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
        (key("f", "fullscreen"), "Focus tab / return to grid"),
        ("Enter".into(), "New tab"),
        ("Tab / Shift+Tab".into(), "Next / previous tab"),
        ("PgUp / PgDn".into(), "Previous / next grid page"),
        ("p".into(), "Park tab (confirm)"),
        ("u".into(), "Show parked tabs"),
        (key("s", "move"), "Move pane to a destination"),
        (
            format!("{} / {}", key("a", "agents"), key("i", "agent-link")),
            "Agents / link conversation",
        ),
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
        " ? Help · Ctrl-{}, then key",
        char::from(b'A' + workspace.ui.config.prefix - 1)
    );
    let header_bg = Color::Indexed(
        if super::ui::has_unread_notifications(&workspace.ui, &workspace.panes) {
            160
        } else {
            94
        },
    );
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
    for rect in workspace.grid_slots() {
        let selected = frame
            .panes
            .iter()
            .any(|view| view.rect == rect && view.pane.id == workspace.active);
        let right = rect.x + rect.cols;
        if right < frame.cols {
            for y in rect.y..rect.y + rect.rows {
                write_text(
                    frame,
                    right,
                    y,
                    1,
                    "│",
                    tab_accent(workspace, selected),
                    Color::Default,
                    false,
                );
            }
        }
    }
}
fn header(
    frame: &mut Frame,
    workspace: &Workspace,
    view: &PaneView,
    selected: bool,
    spinner: usize,
) {
    if view.rect.rows == 0 {
        return;
    }
    draw_tab(
        frame,
        workspace,
        &view.pane,
        Rect {
            rows: 1,
            ..view.rect
        },
        selected,
        "",
        spinner,
    );
}

fn inactive_color(color: Color, foreground: bool, theme: [Option<[u8; 3]>; 2]) -> Color {
    if color == Color::Default && !foreground {
        return Color::Default;
    }
    let (r, g, b) = if color == Color::Default {
        theme[usize::from(!foreground)]
            .map(|[r, g, b]| (r, g, b))
            .unwrap_or_else(|| color_rgb(color, foreground))
    } else {
        color_rgb(color, foreground)
    };
    let backdrop = theme[1].unwrap_or([0, 0, 0]);
    let blend = |value, base| ((u16::from(value) * 3 + u16::from(base)) / 4) as u8;
    Color::Rgb(
        blend(r, backdrop[0]),
        blend(g, backdrop[1]),
        blend(b, backdrop[2]),
    )
}

fn tab_surface(workspace: &Workspace) -> Color {
    theme_surface(workspace, 14)
}
fn theme_surface(workspace: &Workspace, offset: u8) -> Color {
    workspace.ui.terminal_theme[1].map_or_else(
        || match offset {
            8 => Color::Indexed(234),
            14 => Color::Indexed(super::ui::TAB_BG),
            24 => Color::Indexed(238),
            32 => Color::Indexed(240),
            50 => Color::Indexed(244),
            _ => Color::Indexed(250),
        },
        |rgb| {
            let light = u16::from(rgb[0]) * 3 + u16::from(rgb[1]) * 6 + u16::from(rgb[2]) >= 1280;
            let [r, g, b] = rgb.map(|value| {
                if light {
                    value.saturating_sub(offset)
                } else {
                    value.saturating_add(offset)
                }
            });
            Color::Rgb(r, g, b)
        },
    )
}
fn tab_accent(workspace: &Workspace, selected: bool) -> Color {
    theme_surface(workspace, if selected { 88 } else { 50 })
}
fn tab_background(workspace: &Workspace, selected: bool) -> Color {
    theme_surface(workspace, if selected { 32 } else { 14 })
}
fn tab_text(workspace: &Workspace, _selected: bool) -> Color {
    if workspace.ui.terminal_theme[1]
        .is_some_and(|[r, g, b]| u16::from(r) * 3 + u16::from(g) * 6 + u16::from(b) >= 1280)
    {
        Color::Indexed(232)
    } else {
        Color::Indexed(231)
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
            DialogKind::Parked | DialogKind::ParkConfirm | DialogKind::RestoreConfirm
        ) {
            super::ui::PARKED_BG
        } else if dialog.kind == DialogKind::CloseConfirm {
            88
        } else {
            24
        },
    );
    fill(frame, Rect { rows: 1, ..bounds }, header_bg);
    let title = match dialog.kind {
        DialogKind::Status => " System status · live",
        DialogKind::Parked => " ⊟ Parked tabs · select to restore",
        DialogKind::Grid => " ▦ Grid · arrows, Enter or click",
        DialogKind::Overview => " Workspace overview · l links conversation",
        DialogKind::AgentLink => " Link agent conversation · Enter or click",
        DialogKind::Notifications => " ⚠︎ Notifications",
        DialogKind::Recovery => {
            if workspace.ui.recovery_loading {
                " Recovery · loading…"
            } else {
                " Recovery"
            }
        }
        DialogKind::Search => " Search history",
        DialogKind::Move => " Move active pane · choose destination",
        DialogKind::Config => " ⚙︎ Settings",
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
    if dialog.kind == DialogKind::Status {
        draw_status(frame, workspace, bounds);
    } else if dialog.kind == DialogKind::Color {
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
                _ if dialog.kind == DialogKind::RestoreConfirm => super::ui::RESTORE_BG,
                _ => super::ui::PARKED_BG,
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
        let (_, height) = super::workspace::config_geometry(bounds);
        {
            fill(
                frame,
                Rect {
                    x: bounds.x + 1,
                    y: bounds.y + 1,
                    cols: bounds.cols - 2,
                    rows: height + 1,
                },
                Color::Indexed(235),
            );
            write_text(
                frame,
                bounds.x + 2,
                bounds.y + 1,
                bounds.cols - 4,
                "Notifications",
                Color::Indexed(117),
                Color::Indexed(235),
                true,
            );
        }
        for (index, value, rect) in super::workspace::config_targets(bounds) {
            let checked = value == Some(workspace.ui.config.notification_sound);
            let label = if value == Some(true) { "On" } else { "Off" };
            let label = format!("{} {label}", if checked { "●" } else { "○" });
            let bg = Color::Indexed(if index == dialog.selection && checked {
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
            bounds.y + 2 + height / 2,
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
            | DialogKind::AgentLink
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
pub(super) fn color_rgb(color: Color, foreground: bool) -> (u8, u8, u8) {
    match color {
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
    }
}
fn dim_color(color: Color, foreground: bool) -> Color {
    let (r, g, b) = color_rgb(color, foreground);
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
pub(super) fn write_text(
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
                    match cell.underline_color {
                        Some(Color::Indexed(i)) => {
                            let _ = write!(output, "\x1b[58;5;{i}m");
                        }
                        Some(Color::Rgb(r, g, b)) => {
                            let _ = write!(output, "\x1b[58;2;{r};{g};{b}m");
                        }
                        _ => {}
                    }
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
    use super::workspace::{TabTarget, tab_targets};
    let area = Rect {
        x: 0,
        y: 0,
        cols: frame.sidebar_width(),
        rows: frame.rows,
    };
    fill(frame, area, tab_surface(workspace));
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
    let mut previous_tab: Option<Rect> = None;
    for (target, rect) in tab_targets(
        frame.cols,
        frame.rows,
        &workspace.ui.config,
        panes.len(),
        workspace
            .ui
            .tab_scroll_anchor
            .unwrap_or(active)
            .min(panes.len().saturating_sub(1)),
        workspace.panes.iter().filter(|pane| pane.parked).count(),
    ) {
        match target {
            TabTarget::Parked => {}
            TabTarget::New => {
                let label = " + New tab ";
                let bg = tab_surface(workspace);
                fill(frame, rect, bg);
                write_text(
                    frame,
                    rect.x + 1,
                    rect.y + rect.rows / 2,
                    rect.cols - 2,
                    &"─".repeat(usize::from(rect.cols - 2)),
                    tab_accent(workspace, false),
                    bg,
                    false,
                );
                let inset = rect
                    .cols
                    .saturating_sub(label.chars().map(char_width).sum())
                    / 2;
                write_text(
                    frame,
                    rect.x + inset,
                    rect.y + rect.rows / 2,
                    rect.cols - inset,
                    label,
                    Color::Indexed(if tab_text(workspace, false) == Color::Indexed(232) {
                        24
                    } else {
                        75
                    }),
                    bg,
                    false,
                );
            }
            TabTarget::Previous | TabTarget::Next => {
                let bg = Color::Indexed(67);
                fill(frame, rect, bg);
                let label = if target == TabTarget::Previous {
                    " ↑ "
                } else {
                    " ↓ "
                };
                write_text(
                    frame,
                    rect.x,
                    rect.y + rect.rows / 2,
                    rect.cols,
                    label,
                    Color::Indexed(231),
                    bg,
                    true,
                );
            }
            TabTarget::Pane(index) => {
                if let Some(pane) = panes.get(index) {
                    draw_tab(
                        frame,
                        workspace,
                        pane,
                        rect,
                        pane.id == active_id,
                        "",
                        spinner,
                    );
                    if let Some(previous) = previous_tab {
                        let y = previous.y + previous.rows - 1;
                        for x in previous.x + 1..previous.x + previous.cols - 1 {
                            let cell = &mut frame.cells[usize::from(y * frame.cols + x)];
                            cell.underline = true;
                            cell.underline_color = Some(theme_surface(workspace, 24));
                        }
                    }
                    previous_tab = Some(rect);
                }
            }
        }
    }
    let x = frame.sidebar_width() - 1;
    let accent = theme_surface(workspace, 24);
    for y in 0..frame.rows {
        write_text(frame, x, y, 1, "▎", accent, Color::Default, false);
    }
    write_text(
        frame,
        x,
        frame.rows / 2,
        1,
        "⋮",
        accent,
        Color::Default,
        false,
    );
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
    let grid = matches!(
        workspace.reading,
        ReadingMode::Grid | ReadingMode::Fullscreen(_)
    );
    let accent = tab_accent(workspace, selected);
    let bg = tab_background(workspace, selected);
    let text_color = if selected && !grid {
        let (r, g, b) = color_rgb(bg, false);
        Color::Indexed(
            if u16::from(r) * 3 + u16::from(g) * 6 + u16::from(b) >= 1280 {
                232
            } else {
                231
            },
        )
    } else {
        tab_text(workspace, selected)
    };
    fill(frame, rect, bg);
    let number = super::workspace::tab_number_for(rect, pane.number);
    write_text(
        frame,
        number.x,
        number.y,
        number.cols,
        &format!(" {} ", pane.number),
        if selected {
            Color::Indexed(226)
        } else {
            Color::Indexed(if tab_text(workspace, false) == Color::Indexed(232) {
                25
            } else {
                110
            })
        },
        bg,
        true,
    );
    let unread = workspace.ui.notifications.iter().any(|item| {
        !item.read && super::activity::matches(item, pane, &workspace.ui.notification_run)
    });
    let close = super::workspace::tab_close(rect);
    let park = super::workspace::tab_park(rect);
    let count_width = count.len() as u16;
    let (title_x, title_y, title_width) = if rect.rows >= 2 {
        (
            rect.x + 2,
            rect.y + 1,
            rect.cols.saturating_sub(4 + count_width),
        )
    } else {
        let x = number.x + number.cols + 2;
        (
            x,
            number.y,
            park.x.saturating_sub(count_width).saturating_sub(x),
        )
    };
    let track = if rect.rows >= 2 {
        Rect {
            x: number.x + number.cols + 1,
            y: rect.y,
            cols: park.x.saturating_sub(number.x + number.cols + 1).min(1),
            rows: 1,
        }
    } else {
        Rect {
            x: title_x,
            y: title_y,
            cols: if title_width >= 3 { 1 } else { 0 },
            rows: 1,
        }
    };
    let (title_x, title_width) = if rect.rows == 1 {
        let occupied = track.cols + u16::from(track.cols > 0);
        (title_x + occupied, title_width.saturating_sub(occupied))
    } else {
        (title_x, title_width)
    };
    let bell_width = if unread { 2.min(title_width) } else { 0 };
    if grid && title_width > 0 {
        write_text(
            frame,
            title_x.saturating_sub(1),
            title_y,
            1,
            " ",
            text_color,
            bg,
            false,
        );
    }
    if unread {
        write_text(
            frame,
            title_x,
            title_y,
            bell_width,
            &format!("{} ", super::ui::ALERT_ICON),
            Color::Indexed(226),
            bg,
            true,
        );
    }
    let empty_shell = pane.kind.is_none() && pane.title.eq_ignore_ascii_case("shell");
    let text_color = if empty_shell {
        Color::Indexed(if text_color == Color::Indexed(232) {
            240
        } else {
            245
        })
    } else {
        text_color
    };
    let title = marquee_title(
        if empty_shell { "shell" } else { &pane.title },
        title_width.saturating_sub(bell_width),
        workspace.ui.title_ticks.get(&pane.id).copied().unwrap_or(0),
    );
    write_text(
        frame,
        title_x + bell_width,
        title_y,
        title_width.saturating_sub(bell_width),
        &title,
        text_color,
        bg,
        false,
    );
    if grid {
        let width: u16 = title.chars().map(char_width).sum();
        if width < title_width.saturating_sub(bell_width) {
            write_text(
                frame,
                title_x + bell_width + width,
                title_y,
                1,
                " ",
                text_color,
                bg,
                false,
            );
        }
    }
    if empty_shell {
        let width = title
            .chars()
            .map(char_width)
            .sum::<u16>()
            .min(title_width.saturating_sub(bell_width));
        for x in title_x + bell_width..title_x + bell_width + width {
            let index = usize::from(title_y) * usize::from(frame.cols) + usize::from(x);
            if let Some(cell) = frame.cells.get_mut(index) {
                cell.italic = true;
            }
        }
    }
    if rect.rows >= 2 {
        let folder = std::path::Path::new(&pane.cwd)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&pane.cwd);
        write_text(
            frame,
            if rect.rows >= 3 {
                rect.x + 2
            } else {
                track.x + track.cols + 1
            },
            if rect.rows >= 3 { rect.y + 2 } else { rect.y },
            if rect.rows >= 3 {
                rect.cols.saturating_sub(4)
            } else {
                park.x.saturating_sub(track.x + track.cols + 2)
            },
            &safe_text(folder),
            if text_color == Color::Indexed(232) {
                Color::Indexed(240)
            } else {
                Color::Indexed(if selected { 250 } else { 245 })
            },
            bg,
            false,
        );
    }
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
    let light_controls = if grid {
        tab_text(workspace, false) == Color::Indexed(232)
    } else {
        let (r, g, b) = color_rgb(bg, false);
        u16::from(r) * 3 + u16::from(g) * 6 + u16::from(b) >= 1280
    };
    write_text(
        frame,
        close.x,
        close.y,
        close.cols,
        " × ",
        Color::Indexed(if light_controls { 160 } else { 203 }),
        bg,
        true,
    );
    write_text(
        frame,
        park.x,
        park.y,
        park.cols,
        " ⊟ ",
        Color::Indexed(if light_controls { 91 } else { 141 }),
        bg,
        true,
    );
    for y in rect.y..rect.y + rect.rows {
        write_text(frame, rect.x, y, 1, " ", accent, bg, false);
    }
    if pane.status == crate::model::Status::Working {
        activity_spinner(
            frame,
            track,
            if selected {
                Color::Indexed(226)
            } else {
                tab_text(workspace, false)
            },
            spinner,
        );
    }
}

fn activity_spinner(frame: &mut Frame, track: Rect, foreground: Color, tick: usize) {
    if track.cols == 0 {
        return;
    }
    let index = usize::from(track.y) * usize::from(frame.cols) + usize::from(track.x);
    if let Some(cell) = frame.cells.get_mut(index) {
        cell.text = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][tick % 10].into();
        cell.fg = foreground;
        cell.width = 1;
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
struct SidebarMetric {
    label: &'static str,
    values: Vec<(String, u8)>,
}
fn sidebar_metrics(segments: &[(String, u8)]) -> Vec<SidebarMetric> {
    let metric = |label, value, color| SidebarMetric {
        label,
        values: vec![(value, color)],
    };
    let mut metrics = Vec::new();
    let mut disks = Vec::new();
    for (text, color) in segments {
        if let Some(value) = text.strip_prefix("cpu ") {
            metrics.push(metric("CPU", value.to_string(), *color));
        } else if let Some(value) = text.strip_prefix("mem ") {
            metrics.push(metric("Memory", value.to_string(), *color));
        } else if let Some(value) = text.strip_prefix("swap ") {
            metrics.push(metric("Swap", value.to_string(), *color));
        } else if let Some((label, value)) = text
            .strip_prefix('C')
            .map(|value| ("CPU", value))
            .or_else(|| text.strip_prefix('M').map(|value| ("Memory", value)))
        {
            if let Ok(percent) = value.parse::<u8>() {
                metrics.push(metric(
                    label,
                    format!(
                        "{} {}%",
                        super::vitals::bar_glyph(percent),
                        percent.min(100)
                    ),
                    *color,
                ));
            }
        } else if text == "disk" {
            continue;
        } else if text.starts_with(['●', '◕', '◑', '◔', '○']) || text.starts_with("disk ")
        {
            disks.push((
                text.strip_prefix("disk ").unwrap_or(text).to_string(),
                *color,
            ));
        }
    }
    if !disks.is_empty() {
        metrics.push(SidebarMetric {
            label: "Disks",
            values: disks,
        });
    }
    metrics
}

fn draw_dev_badge(frame: &mut Frame, x: u16, y: u16) {
    write_text(
        frame,
        x,
        y,
        5,
        " DEV ",
        Color::Indexed(203),
        Color::Indexed(240),
        true,
    );
}

fn sidebar_dev_watermark(frame: &Frame) -> Rect {
    use super::workspace::{TabTarget, sidebar_status_rows, tab_targets};
    let panes: Vec<_> = frame
        .panes
        .iter()
        .filter(|view| !view.pane.parked && Some(view.pane.id) != frame.ui.scratch)
        .collect();
    let active_id = if frame.ui.scratch_visible {
        frame.ui.scratch_return.unwrap_or(frame.active)
    } else {
        frame.active
    };
    let active = panes
        .iter()
        .position(|view| view.pane.id == active_id)
        .unwrap_or(0);
    let top = frame.rows - sidebar_status_rows(frame.rows);
    let occupied = tab_targets(
        frame.cols,
        frame.rows,
        &frame.ui.config,
        panes.len(),
        frame
            .ui
            .tab_scroll_anchor
            .unwrap_or(active)
            .min(panes.len().saturating_sub(1)),
        frame.panes.iter().filter(|view| view.pane.parked).count(),
    )
    .into_iter()
    .filter(|(target, _)| *target != TabTarget::Parked)
    .map(|(_, rect)| rect.y + rect.rows)
    .max()
    .unwrap_or(0)
    .min(top);
    let width = frame.sidebar_width() - 1;
    let free = top - occupied;
    if free == 0 {
        let cols = if sidebar_status_rows(frame.rows) == 3 {
            3
        } else {
            7
        };
        return Rect {
            x: if cols == 3 {
                5
            } else {
                width.saturating_sub(cols) / 2
            },
            y: top,
            cols,
            rows: 1,
        };
    }
    let rows = if free >= 3 { 3 } else { 1 };
    Rect {
        x: 1,
        y: occupied + (free - rows) / 2,
        cols: width - 2,
        rows,
    }
}

fn metric_bar_background(frame: &mut Frame, area: Rect, background: Color) {
    for y in area.y..area.y + area.rows {
        for x in area.x..area.x + area.cols {
            let cell = &mut frame.cells[usize::from(y * frame.cols + x)];
            if matches!(
                cell.text.as_str(),
                "▁" | "▂" | "▃" | "▄" | "▅" | "▆" | "▇" | "█"
            ) {
                cell.bg = background;
            }
        }
    }
}

fn draw_sidebar_disks(
    frame: &mut Frame,
    rect: Rect,
    disks: &[(String, u8)],
    colors: (Color, Color),
) {
    let (muted, bg) = colors;
    let count = disks.len().min(2);
    if count == 0 || rect.rows == 0 {
        return;
    }
    let labels: Vec<_> = disks
        .iter()
        .take(count)
        .map(|(mount, _)| {
            let mount = safe_text(mount);
            if mount == "/" || !mount.starts_with('/') {
                mount
            } else {
                format!(
                    "/{}",
                    mount
                        .rsplit('/')
                        .find(|part| !part.is_empty())
                        .unwrap_or("?")
                )
            }
        })
        .collect();
    // Keep mount prefixes even at the minimum width. The pie already denotes
    // usage, so the tightest variant can omit redundant percent signs.
    let entries = (0..3)
        .map(|mode| {
            let entries: Vec<_> = disks
                .iter()
                .zip(&labels)
                .map(|((_, percent), label)| {
                    let percent = (*percent).min(100);
                    let text = format!(
                        "{label}{}{}{percent}{}",
                        if mode == 0 { " " } else { "" },
                        super::vitals::pie_glyph(percent),
                        if mode < 2 { "%" } else { "" }
                    );
                    (text, if percent >= 90 { 196 } else { 222 })
                })
                .collect();
            let width = entries
                .iter()
                .map(|(text, _)| text.chars().map(char_width).sum::<u16>())
                .sum::<u16>()
                + count.saturating_sub(1) as u16;
            (entries, width)
        })
        .find(|(_, width)| *width <= rect.cols)
        .unwrap_or_else(|| (vec![("Disks…".into(), 222)], 6));
    if entries.1 + 6 <= rect.cols {
        write_text(frame, rect.x, rect.y, 5, "Disks", muted, bg, false);
    }
    let mut x = rect.x + rect.cols.saturating_sub(entries.1);
    for (text, color) in entries.0 {
        let width = text.chars().map(char_width).sum::<u16>();
        write_text(
            frame,
            x,
            rect.y,
            width.min((rect.x + rect.cols).saturating_sub(x)),
            &text,
            Color::Indexed(color),
            bg,
            false,
        );
        x += width + 1;
    }
}

pub(super) fn sidebar_disk_region(frame: &Frame) -> Option<Rect> {
    if !frame.sidebar_status() {
        return None;
    }
    let metrics = sidebar_metrics(&frame.ui.vitals);
    let index = metrics.iter().position(|metric| metric.label == "Disks");
    if index.is_none() && frame.ui.disk_readings.is_empty() {
        return None;
    }
    let height = super::workspace::sidebar_status_rows(frame.rows);
    let top = frame.rows - height + u16::from(height > 3);
    let end = frame.rows - if height > 3 { 3 } else { 2 };
    let y = top + index.unwrap_or(metrics.len()) as u16;
    (y < end).then(|| Rect {
        x: 0,
        y,
        cols: frame.sidebar_width() - 1,
        rows: 1,
    })
}

fn draw_sidebar_status(frame: &mut Frame, workspace: &Workspace, notice: Option<&str>) {
    let rows = super::workspace::sidebar_status_rows(frame.rows);
    let width = frame.sidebar_width() - 1;
    let top = frame.rows - rows;
    let watermark = if workspace.ui.show_dev_badge {
        sidebar_dev_watermark(frame)
    } else {
        Rect::default()
    };
    let bg = tab_surface(workspace);
    fill(
        frame,
        Rect {
            x: 0,
            y: top,
            cols: width,
            rows,
        },
        bg,
    );
    let foreground = tab_text(workspace, false);
    let muted = Color::Indexed(if foreground == Color::Indexed(232) {
        243
    } else {
        245
    });
    let value_color = if foreground == Color::Indexed(232) {
        foreground
    } else {
        Color::Indexed(250)
    };
    let metric_top = if rows > 3 { top + 1 } else { top };
    let metric_end = if rows > 3 {
        frame.rows - 3
    } else {
        frame.rows - 2
    };
    let compact_dev = workspace.ui.show_dev_badge && rows == 3 && watermark.y == top;
    if rows > 3 {
        for y in [top, frame.rows - 3] {
            write_text(
                frame,
                1,
                y,
                width - 2,
                &"─".repeat(usize::from(width - 2)),
                tab_accent(workspace, false),
                bg,
                false,
            );
        }
    }
    if !frame.controller && rows > 3 {
        write_text(
            frame,
            2,
            top,
            width.saturating_sub(4),
            if workspace.ui.show_dev_badge && watermark.y == top {
                "Obs"
            } else {
                " Observer "
            },
            muted,
            bg,
            false,
        );
    }
    if let Some(notice) = notice {
        let mut y = metric_top;
        let mut line = String::new();
        for word in safe_text(notice).split_whitespace() {
            let size = line.chars().map(char_width).sum::<u16>()
                + 1
                + word.chars().map(char_width).sum::<u16>();
            if !line.is_empty() && size > width - 2 {
                write_text(frame, 1, y, width - 2, &line, value_color, bg, false);
                y += 1;
                line.clear();
                if y >= metric_end {
                    break;
                }
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if y < metric_end {
            write_text(frame, 1, y, width - 2, &line, value_color, bg, false);
        }
    } else if !frame.controller && rows == 3 {
        write_text(
            frame,
            1,
            metric_top,
            width.saturating_sub(2),
            if workspace.ui.show_dev_badge && watermark.y == top {
                "Obs"
            } else {
                "Observer"
            },
            muted,
            bg,
            false,
        );
    } else {
        let value_width = width.saturating_sub(9);
        let mut metrics = sidebar_metrics(&workspace.ui.vitals);
        if !workspace.ui.disk_readings.is_empty()
            && !metrics.iter().any(|metric| metric.label == "Disks")
        {
            metrics.push(SidebarMetric {
                label: "Disks",
                values: vec![],
            });
        }
        let capacity = usize::from(metric_end - metric_top);
        let visible: Vec<_> = metrics.iter().take(capacity).collect();
        let mut y = metric_top;
        for metric in visible {
            if y >= metric_end {
                break;
            }
            if metric.label == "Disks" {
                let fallback: Vec<_> = metric
                    .values
                    .iter()
                    .enumerate()
                    .filter_map(|(index, (value, _))| {
                        let percent = value
                            .split_whitespace()
                            .last()?
                            .strip_suffix('%')?
                            .parse::<u8>()
                            .ok()?;
                        Some((format!("D{}", index + 1), percent))
                    })
                    .collect();
                let disks = if workspace.ui.disk_readings.is_empty() {
                    &fallback
                } else {
                    &workspace.ui.disk_readings
                };
                draw_sidebar_disks(
                    frame,
                    Rect {
                        x: 1,
                        y,
                        cols: width.saturating_sub(2),
                        rows: 1,
                    },
                    disks,
                    (muted, bg),
                );
                y += 1;
                continue;
            }
            let row_y = y;
            y += 1;
            let y = row_y;
            let label = if compact_dev {
                match metric.label {
                    "Memory" => "RAM",
                    "Swap" => "SWP",
                    "Disks" => "DSK",
                    label => label,
                }
            } else {
                metric.label
            };
            write_text(frame, 1, y, 6, label, muted, bg, false);
            let (value, color) = &metric.values[0];
            if matches!(metric.label, "CPU" | "Memory" | "Swap") {
                if let Some((_, percent)) = value.split_once(' ') {
                    let percent = percent.trim();
                    let load = percent
                        .strip_suffix('%')
                        .and_then(|value| value.parse::<u16>().ok())
                        .unwrap_or(0)
                        .min(100);
                    let start = if compact_dev { 11 } else { 10 };
                    let percent_x = width - if compact_dev { 4 } else { 5 };
                    let length = percent_x.saturating_sub(start + 1);
                    let filled = (load * length + 50) / 100;
                    for index in 0..length {
                        write_text(
                            frame,
                            start + index,
                            y,
                            1,
                            if index < filled { "▰" } else { "▱" },
                            if index < filled {
                                Color::Indexed(*color)
                            } else {
                                muted
                            },
                            bg,
                            false,
                        );
                    }
                    write_text(
                        frame,
                        percent_x,
                        y,
                        4,
                        &format!("{percent:>4}"),
                        Color::Indexed(*color),
                        bg,
                        false,
                    );
                    continue;
                }
            }
            let value = safe_text(value);
            let mut display = String::new();
            let mut size = 0;
            for c in value.chars() {
                let next = char_width(c);
                if size + next > value_width {
                    while size >= value_width {
                        if let Some(last) = display.pop() {
                            size -= char_width(last);
                        } else {
                            break;
                        }
                    }
                    display.push('…');
                    size += 1;
                    break;
                }
                display.push(c);
                size += next;
            }
            write_text(
                frame,
                width - 1 - size,
                y,
                size,
                &display,
                Color::Indexed(*color),
                bg,
                false,
            );
        }
    }
    if workspace.ui.show_dev_badge {
        let panel = theme_surface(workspace, 24);
        fill(frame, watermark, panel);
        let label = if watermark.cols == 3 {
            "DEV"
        } else {
            " D E V "
        };
        let label_width = label.len() as u16;
        write_text(
            frame,
            watermark.x + watermark.cols.saturating_sub(label_width) / 2,
            watermark.y + watermark.rows / 2,
            label_width,
            label,
            Color::Indexed(if foreground == Color::Indexed(232) {
                124
            } else {
                203
            }),
            panel,
            true,
        );
    }
    let [config, mode, parked, help] = frame.controls();
    let unread = super::ui::has_unread_notifications(&workspace.ui, &workspace.panes);
    let parked_count = workspace.panes.iter().filter(|pane| pane.parked).count();
    for (rect, label, color) in [
        (
            config,
            if config.cols >= 10 {
                " ⚙︎ Config "
            } else {
                "⚙︎ Config"
            }
            .into(),
            24,
        ),
        (
            mode,
            if workspace.active == 0 {
                if mode.cols >= 8 { " + New " } else { " + " }
            } else if mode.cols >= 8 {
                " ▦ Grid "
            } else {
                " ▦ "
            }
            .into(),
            67,
        ),
        (
            parked,
            {
                let label = super::workspace::parked_label(parked_count);
                if label.chars().map(char_width).sum::<u16>() <= parked.cols {
                    label
                } else if label.trim().chars().map(char_width).sum::<u16>() <= parked.cols {
                    label.trim().into()
                } else if parked_count > 0 {
                    format!(" ⊟ ({parked_count}) ")
                } else {
                    "⊟ Parked".into()
                }
            },
            super::ui::PARKED_BG,
        ),
        (
            help,
            if unread {
                if help.cols >= 8 {
                    "⚠︎ Alerts"
                } else {
                    " ⚠︎ "
                }
            } else if help.cols >= 8 {
                " ? Help "
            } else {
                " ? "
            }
            .into(),
            if unread { 160 } else { 94 },
        ),
    ] {
        let label_width = label.chars().map(char_width).sum::<u16>().min(rect.cols);
        let button_bg = Color::Indexed(color);
        fill(frame, rect, button_bg);
        let inset = (rect.cols - label_width) / 2;
        write_text(
            frame,
            rect.x + inset,
            rect.y,
            rect.cols - inset,
            &label,
            Color::Indexed(231),
            button_bg,
            false,
        );
    }
}
fn draw_vitals(frame: &mut Frame, segments: &[(String, u8)]) {
    let segments: Vec<_> = segments
        .iter()
        .filter(|(text, _)| !super::vitals::is_clock_segment(text))
        .cloned()
        .collect();
    let start = (frame.footer_cols() + if frame.controller { 0 } else { 10 }).min(frame.cols);
    let end = frame.cols;
    let available = end.saturating_sub(start);
    let selected: Vec<_> = if available < 50 {
        let mut remaining = available.saturating_sub(
            segments
                .last()
                .map_or(0, |(text, _)| text.chars().count() as u16),
        );
        let mut chosen = Vec::new();
        for segment in segments
            .iter()
            .take(2)
            .take(segments.len().saturating_sub(1))
        {
            let cost = segment.0.chars().count() as u16 + 1;
            if cost <= remaining {
                chosen.push(segment);
                remaining -= cost;
            }
        }
        chosen.extend(segments.last());
        chosen
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
    metric_bar_background(
        frame,
        Rect {
            x: start,
            y: frame.rows - 1,
            cols: available,
            rows: 1,
        },
        Color::Indexed(244),
    );
}
fn draw_parked(
    frame: &mut Frame,
    workspace: &Workspace,
    dialog: super::ui::Dialog,
    bounds: Rect,
    spinner: usize,
) {
    let [restore, close] = super::workspace::parked_controls(bounds);
    let panes: Vec<_> = workspace.panes.iter().filter(|p| p.parked).collect();
    if panes.is_empty() {
        let middle = bounds.y + bounds.rows / 2;
        for (y, label, color, bold) in [
            (middle.saturating_sub(2), "⊟", super::ui::PARKED_BG, true),
            (middle, "No parked tabs", 231, true),
            (middle + 1, "Park tabs to keep jobs running.", 250, false),
            (bounds.y + bounds.rows - 2, "Esc to close", 245, false),
        ] {
            let width = bounds.cols.saturating_sub(4);
            let text_width: u16 = label.chars().map(char_width).sum();
            write_text(
                frame,
                bounds.x + 2 + width.saturating_sub(text_width) / 2,
                y,
                width,
                label,
                Color::Indexed(color),
                Color::Indexed(236),
                bold,
            );
        }
        return;
    }
    let title_x = bounds.x + 8;
    for (x, width, label) in [
        (bounds.x + 2, 6, "Tab"),
        (title_x, restore.x.saturating_sub(title_x + 1), "Name"),
        (restore.x, 3, " ↥ "),
        (close.x, 3, " × "),
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
    for (index, rect) in super::workspace::parked_cards(bounds, dialog.selection, panes.len()) {
        let pane = panes[index];
        let y = rect.y + 1;
        let selected = index == dialog.selection;
        let accent = tab_accent(workspace, selected);
        let bg = tab_background(workspace, selected);
        fill(frame, rect, bg);
        for row in rect.y..rect.y + rect.rows {
            write_text(frame, bounds.x, row, 1, "▌", accent, bg, false);
        }
        write_text(
            frame,
            bounds.x + 1,
            y,
            4,
            &format!(" {} ", pane.id),
            if selected {
                Color::Indexed(226)
            } else {
                tab_text(workspace, false)
            },
            bg,
            true,
        );
        let title_width = restore.x.saturating_sub(title_x + 1);
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
            tab_text(workspace, selected),
            bg,
            false,
        );
        let [restore, close] = super::workspace::parked_controls(rect);
        for (control, label, color) in [(restore, " ↥ ", super::ui::RESTORE_BG), (close, " × ", 88)]
        {
            fill(frame, control, Color::Indexed(color));
            write_text(
                frame,
                control.x,
                y,
                control.cols,
                label,
                Color::Indexed(231),
                Color::Indexed(color),
                true,
            );
        }
        if pane.status == crate::model::Status::Working {
            activity_spinner(
                frame,
                Rect {
                    x: rect.x + 6,
                    y,
                    cols: 1,
                    rows: 1,
                },
                if selected {
                    Color::Indexed(226)
                } else {
                    tab_text(workspace, false)
                },
                spinner,
            );
        }
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
    if dialog.kind == DialogKind::Notifications {
        let buttons = super::ui::notification_buttons(bounds);
        for (rect, label) in buttons.into_iter().zip([
            if workspace.ui.notification_history {
                " h Hide history "
            } else {
                " h Show history "
            },
            " a Mark all read ",
        ]) {
            write_text(
                frame,
                rect.x,
                rect.y,
                rect.cols,
                label,
                Color::Indexed(117),
                Color::Indexed(237),
                false,
            );
        }
    }
    let labels: Vec<String> = match dialog.kind {
        DialogKind::AgentLink => {
            if workspace.ui.agent_link_loading {
                vec!["Loading running conversations…".into()]
            } else if let Some(error) = &workspace.ui.agent_link_error {
                vec![
                    "Automatic discovery / clear explicit link".into(),
                    error.clone(),
                ]
            } else {
                std::iter::once("Automatic discovery / clear explicit link".into())
                    .chain(workspace.ui.agent_sessions.iter().map(|session| {
                        format!(
                            "{} · {} · {}",
                            session.name,
                            session.status.text(),
                            session.id
                        )
                    }))
                    .collect()
            }
        }
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
        DialogKind::Notifications => {
            super::ui::visible_notifications(&workspace.ui, &workspace.panes)
                .map(|item| {
                    let target = workspace.panes.iter().find(|pane| {
                        super::activity::matches(item, pane, &workspace.ui.notification_run)
                    });
                    let location = target.map_or_else(
                        || format!("historical · former pane {}", item.pane),
                        |pane| {
                            format!(
                                "{}tab {}",
                                if pane.parked { "parked " } else { "" },
                                pane.number
                            )
                        },
                    );
                    format!(
                        "{} {} {} · {} · {} · {}s ago",
                        if item.read { "○" } else { "●" },
                        if item.title.starts_with("Bell · ") {
                            super::ui::ALERT_ICON
                        } else {
                            item.status.marker()
                        },
                        location,
                        item.title,
                        item.workspace,
                        crate::now().saturating_sub(item.at)
                    )
                })
                .collect()
        }
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
            if dialog.kind == DialogKind::Notifications && !workspace.ui.notification_history {
                "No unread alerts"
            } else {
                "No matching entries"
            },
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
fn draw_status(frame: &mut Frame, workspace: &Workspace, bounds: Rect) {
    use super::status::{self, View};
    let ui = &workspace.ui;
    let data = &ui.status;
    let bg = Color::Indexed(236);
    let width = bounds.cols.saturating_sub(2);
    let x = bounds.x + 1;
    let summary_height = status::summary_height(bounds, data);
    let tabs_y = bounds.y + 1 + summary_height;
    let main = ui.status_view == View::Main;
    write_text(
        frame,
        x,
        tabs_y,
        width,
        if main {
            "[ Main ]   I/O"
        } else {
            "  Main   [ I/O ]"
        },
        Color::Indexed(117),
        bg,
        true,
    );
    let mut query = if ui.status_query.is_empty() {
        "/ search · c CPU · m memory · p PID · r/w I/O".to_string()
    } else {
        format!("Search: {}", safe_text(&ui.status_query))
    };
    let tasks = format!("Tasks {} ({} running)", data.tasks, data.running);
    let tasks_width = tasks.chars().map(char_width).sum::<u16>();
    let show_tasks = data.ready
        && width >= 50
        && (ui.status_query.is_empty()
            || query.chars().map(char_width).sum::<u16>() + tasks_width + 2 <= width);
    if show_tasks && query.chars().map(char_width).sum::<u16>() + tasks_width + 2 > width {
        query = "/ search".into();
    }
    write_text(
        frame,
        x,
        tabs_y + 1,
        if show_tasks {
            width - tasks_width - 2
        } else {
            width
        },
        &query,
        Color::Indexed(245),
        bg,
        false,
    );
    if show_tasks {
        write_text(
            frame,
            x + width - tasks_width,
            tabs_y + 1,
            tasks_width,
            &tasks,
            Color::Indexed(250),
            bg,
            false,
        );
    }
    draw_status_summary(frame, workspace, bounds, summary_height);
    let table_y = status::table_y(bounds, data);
    fill(
        frame,
        Rect {
            x,
            y: table_y,
            cols: width,
            rows: 1,
        },
        Color::Indexed(239),
    );
    let columns = status::columns(width, ui.status_view, data);
    let mut col_x = x;
    for (label, size, sort) in &columns {
        let text = if *sort == Some(ui.status_sort) {
            format!("{}{}", label, if ui.status_descending { "↓" } else { "↑" })
        } else {
            label.to_string()
        };
        let available = if *size == 0 {
            x + width - col_x
        } else {
            (*size).min((x + width).saturating_sub(col_x))
        };
        write_text(
            frame,
            col_x,
            table_y,
            available,
            &text,
            Color::Indexed(117),
            Color::Indexed(239),
            true,
        );
        col_x += available;
    }
    let rows = status::processes(data, &ui.status_query, ui.status_sort, ui.status_descending);
    let capacity = usize::from((bounds.y + bounds.rows - 1).saturating_sub(table_y + 1));
    let offset = ui
        .status_offset
        .min(rows.len().saturating_sub(capacity.max(1)));
    for (index, process) in rows.iter().skip(offset).take(capacity).enumerate() {
        let y = table_y + 1 + index as u16;
        let row_bg = if index % 2 == 0 {
            bg
        } else {
            Color::Indexed(235)
        };
        fill(
            frame,
            Rect {
                x,
                y,
                cols: width,
                rows: 1,
            },
            row_bg,
        );
        let mut col_x = x;
        for (label, size, _) in &columns {
            let text = match *label {
                "PID" => process.pid.to_string(),
                "USER" => process.user.clone(),
                "CPU%" => status::percent(process.cpu),
                "MEM%" => status::percent(Some(process.memory)),
                "RES" => status::bytes(process.resident),
                "TIME" => format!("{}:{:02}", process.runtime / 60, process.runtime % 60),
                "S" => process.state.clone(),
                "READ/s" => status::rate(process.read),
                "WRITE/s" => status::rate(process.write),
                _ => safe_text(&process.command),
            };
            let available = if *size == 0 {
                (x + width).saturating_sub(col_x)
            } else {
                (*size).min((x + width).saturating_sub(col_x))
            };
            let color = match *label {
                "CPU%" => 114,
                "MEM%" | "RES" => 141,
                "READ/s" | "WRITE/s" => 222,
                _ => 250,
            };
            write_text(
                frame,
                col_x,
                y,
                available,
                &text,
                Color::Indexed(color),
                row_bg,
                false,
            );
            col_x += available;
        }
    }
    if rows.is_empty() && capacity > 0 {
        write_text(
            frame,
            x,
            table_y + 1,
            width,
            if data.ready {
                "No matching processes"
            } else {
                "Waiting for first sample…"
            },
            Color::Indexed(245),
            bg,
            false,
        );
    }
    let footer = format!(
        "Esc close · Tab view · {}–{}/{}",
        if rows.is_empty() { 0 } else { offset + 1 },
        (offset + capacity).min(rows.len()),
        rows.len()
    );
    write_text(
        frame,
        x,
        bounds.y + bounds.rows - 1,
        width,
        &footer,
        Color::Indexed(245),
        bg,
        false,
    );
}
fn draw_status_meter(
    frame: &mut Frame,
    rect: Rect,
    label: &str,
    percent: u8,
    detail: &str,
    color: u8,
) {
    let bg = Color::Indexed(236);
    let label_width = if label.starts_with('/') {
        (label.chars().map(char_width).sum::<u16>() + 1).min(rect.cols / 3)
    } else {
        6
    };
    let bar_width = rect.cols.saturating_sub(label_width + 2);
    let value = if detail.is_empty() || detail.chars().map(char_width).sum::<u16>() + 5 > bar_width
    {
        format!("{percent:>3}%")
    } else {
        format!("{percent:>3}% {detail}")
    };
    let value_width = value.chars().map(char_width).sum::<u16>();
    write_text(
        frame,
        rect.x,
        rect.y,
        label_width,
        label,
        Color::Indexed(color),
        bg,
        false,
    );
    if bar_width > 0 {
        let start = rect.x + label_width;
        write_text(frame, start, rect.y, 1, "[", Color::Indexed(245), bg, false);
        let filled = (u32::from(bar_width) * u32::from(percent.min(100)) + 50) / 100;
        for index in 0..bar_width {
            let active = u32::from(index) < filled;
            write_text(
                frame,
                start + 1 + index,
                rect.y,
                1,
                if active { "|" } else { " " },
                Color::Indexed(if active { color } else { 245 }),
                bg,
                false,
            );
        }
        write_text(
            frame,
            start + 1 + bar_width,
            rect.y,
            1,
            "]",
            Color::Indexed(245),
            bg,
            false,
        );
    }
    write_text(
        frame,
        rect.x + rect.cols.saturating_sub(value_width + 1),
        rect.y,
        bar_width.min(value_width),
        &value,
        Color::Indexed(color),
        bg,
        false,
    );
}

fn draw_status_summary(frame: &mut Frame, workspace: &Workspace, bounds: Rect, height: u16) {
    use super::status::{self, View};
    let data = &workspace.ui.status;
    let x = bounds.x + 1;
    let y = bounds.y + 1;
    let width = bounds.cols.saturating_sub(2);
    let two_columns = status::summary_columns(bounds) == 3;
    let meter_width = status::summary_column_width(bounds);
    let meter_gap = if two_columns { 2 } else { 0 };
    if !data.ready {
        write_text(
            frame,
            x,
            y,
            width,
            "Collecting system metrics…",
            Color::Indexed(250),
            Color::Indexed(236),
            false,
        );
        return;
    }
    let clock = workspace
        .ui
        .vitals
        .iter()
        .filter(|(s, _)| s.contains(':') || (s.len() == 10 && s.as_bytes().get(4) == Some(&b'-')))
        .map(|(s, _)| s.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let uptime = format!(
        "Uptime {}d {:02}:{:02}",
        data.uptime / 86400,
        data.uptime / 3600 % 24,
        data.uptime / 60 % 60
    );
    let clock_uptime = if clock.is_empty() {
        uptime.clone()
    } else {
        format!("{clock} · {uptime}")
    };
    let mut details = vec![(format!("Load {}", data.load), 250)];
    if two_columns {
        if status::summary_detail_rows(bounds) == 5 {
            details.push((clock, 39));
            details.push((uptime, 39));
        } else {
            details.push((clock_uptime, 39));
        }
    } else {
        details.push((clock_uptime, 39));
    }
    details.push((
        format!(
            "Network ↓ {}  ↑ {}",
            status::rate(data.network.map(|v| v.0)),
            status::rate(data.network.map(|v| v.1))
        ),
        73,
    ));
    details.push((
        format!(
            "Disk I/O R {}  W {}",
            status::rate(data.disk_io.map(|v| v.0)),
            status::rate(data.disk_io.map(|v| v.1))
        ),
        222,
    ));
    let disks = &data.disks;
    let percent =
        |used: u64, total: u64| (u128::from(used) * 100 / u128::from(total.max(1))).min(100) as u8;
    let ram = percent(data.memory_used, data.memory_total);
    let swap = percent(data.swap_used, data.swap_total);
    let ram_detail = format!(
        "{}/{}",
        status::bytes(data.memory_used),
        status::bytes(data.memory_total)
    );
    let swap_detail = format!(
        "{}/{}",
        status::bytes(data.swap_used),
        status::bytes(data.swap_total)
    );
    if height == 1 {
        let rect = Rect {
            x,
            y,
            cols: width,
            rows: 1,
        };
        if workspace.ui.status_view == View::Io {
            let (text, color) = details
                .iter()
                .find(|(text, _)| text.starts_with("Disk I/O"))
                .unwrap();
            write_text(
                frame,
                x,
                y,
                width,
                text,
                Color::Indexed(*color),
                Color::Indexed(236),
                false,
            );
        } else if workspace.ui.status_sort == status::Sort::Memory {
            draw_status_meter(frame, rect, "RAM", ram, &ram_detail, 141);
        } else {
            let cpu = data.cpus.iter().map(|p| u64::from(*p)).sum::<u64>()
                / data.cpus.len().max(1) as u64;
            draw_status_meter(frame, rect, "CPU", cpu as u8, "", 114);
        }
        return;
    }
    let lower_rows = if two_columns {
        disks.len().div_ceil(2)
    } else {
        details.len() + disks.len()
    }
    .min(usize::from(u16::MAX)) as u16;
    let memory_rows = if two_columns { 1 } else { 2 };
    let cpu_columns = if two_columns { 2 } else { 1 };
    let cpu_slots = (usize::from(height.saturating_sub(lower_rows + memory_rows).max(1))
        * cpu_columns)
        .min(data.cpus.len());
    let cpu_rows = cpu_slots.div_ceil(cpu_columns) as u16;
    for index in 0..cpu_slots {
        let grouped = index + 1 == cpu_slots && data.cpus.len() > cpu_slots;
        let (label, cpu, detail) = if grouped {
            let remaining = &data.cpus[index..];
            (
                "CPU…".into(),
                (remaining.iter().map(|p| u64::from(*p)).sum::<u64>() / remaining.len() as u64)
                    as u8,
                format!("{} cores", remaining.len()),
            )
        } else {
            (format!("CPU{index}"), data.cpus[index], String::new())
        };
        draw_status_meter(
            frame,
            Rect {
                x: x + (index % cpu_columns) as u16 * (meter_width + meter_gap),
                y: y + (index / cpu_columns) as u16,
                cols: meter_width,
                rows: 1,
            },
            &label,
            cpu,
            &detail,
            114,
        );
    }
    for (row, label, value, detail, color) in [
        (0, "RAM", ram, ram_detail.as_str(), 141),
        (1, "Swap", swap, swap_detail.as_str(), 73),
    ] {
        let meter_y = cpu_rows + if two_columns { 0 } else { row };
        if meter_y < height {
            draw_status_meter(
                frame,
                Rect {
                    x: x + if two_columns {
                        row * (meter_width + meter_gap)
                    } else {
                        0
                    },
                    y: y + meter_y,
                    cols: meter_width,
                    rows: 1,
                },
                label,
                value,
                detail,
                color,
            );
        }
    }
    let lower_y = cpu_rows + memory_rows;
    let disk_y = lower_y + if two_columns { 0 } else { details.len() as u16 };
    let disk_columns = if two_columns { 2 } else { 1 };
    for (index, disk) in disks
        .iter()
        .take(usize::from(height.saturating_sub(disk_y)) * disk_columns)
        .enumerate()
    {
        draw_status_meter(
            frame,
            Rect {
                x: x + (index % disk_columns) as u16 * (meter_width + meter_gap),
                y: y + disk_y + (index / disk_columns) as u16,
                cols: meter_width,
                rows: 1,
            },
            &safe_text(&disk.mount),
            disk.percent,
            &format!(
                "({}/{})",
                status::bytes(disk.available),
                status::bytes(disk.total)
            ),
            if disk.percent >= 90 { 196 } else { 222 },
        );
    }
    let (detail_x, detail_y, detail_width, capacity) = if two_columns {
        (x + 2 * (meter_width + meter_gap), y, meter_width, height)
    } else {
        (x, y + lower_y, width, height.saturating_sub(lower_y))
    };
    for (index, (text, color)) in details.iter().take(usize::from(capacity)).enumerate() {
        write_text(
            frame,
            detail_x,
            detail_y + index as u16,
            detail_width,
            text,
            Color::Indexed(*color),
            Color::Indexed(236),
            false,
        );
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
            for (reading, overlay) in [
                (ReadingMode::Grid, false),
                (ReadingMode::Capped(1), true),
                (ReadingMode::Fullscreen(1), true),
            ] {
                let workspace = Workspace::new(cols, 10).unwrap();
                let mut frame = compose(&workspace, &BTreeMap::new(), true, None, 0);
                header(&mut frame, &workspace, &view(cols, overlay), true, 0);
                assert_eq!(frame.cells[0].bg, Color::Indexed(240));
                assert_eq!(frame.cells[5].bg, Color::Indexed(240));
                let button_start = usize::from(cols - 10);
                assert_eq!(frame.cells[button_start].text, " ");
                assert_eq!(frame.cells[usize::from(cols) - 1].text, " ");
                let text = frame.cells[..usize::from(cols)]
                    .iter()
                    .map(|c| c.text.as_str())
                    .collect::<String>();
                assert!(text.contains("#{literal}"));
                assert!(!text.contains('\x1b'));
                if reading == ReadingMode::Grid && !overlay {
                    assert!(!text.contains('↗') && !text.contains('▶'));
                    assert!(!text.contains('⛶'));
                }
                let mut display = Terminal::new(cols, 10);
                display.feed(&terminal_diff(None, &frame));
                assert_eq!(display.snapshot().cells, frame.cells);
                header(&mut frame, &workspace, &view(cols, overlay), false, 0);
                assert_eq!(frame.cells[5].bg, Color::Indexed(236));
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
