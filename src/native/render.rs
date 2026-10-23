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
        } else if workspace.reading == ReadingMode::Grid {
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
        let footer_cols = help.x + help.cols;
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
            if workspace.ui.notifications.iter().any(|item| !item.read) {
                " ⚠︎ "
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
        let right = view.rect.x + view.rect.cols;
        if right < frame.cols {
            for y in view.rect.y..view.rect.y + view.rect.rows {
                write_text(
                    frame,
                    right,
                    y,
                    1,
                    "│",
                    tab_accent(workspace, view.pane.id == workspace.active),
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
        DialogKind::Overview => " Workspace overview",
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
        use super::ui::TabPosition;
        let positions = [
            TabPosition::Auto,
            TabPosition::Top,
            TabPosition::Left,
            TabPosition::Bottom,
        ];
        let (step, height) = super::workspace::config_geometry(bounds);
        for (group, label) in ["Tab placement", "Notifications"].iter().enumerate() {
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
                _ => (
                    value == Some(workspace.ui.config.notification_sound),
                    if value == Some(true) { "On" } else { "Off" }.to_owned(),
                ),
            };
            let label = format!("{} {label}", if checked { "●" } else { "○" });
            let bg = Color::Indexed(if index >= 4 && index == dialog.selection && checked {
                24
            } else if index >= 4 && checked {
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
                Color::Indexed(if index < 4 && index == dialog.selection {
                    226
                } else {
                    231
                }),
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
fn color_rgb(color: Color, foreground: bool) -> (u8, u8, u8) {
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
            cols: super::workspace::sidebar_width(frame.cols),
            rows: frame.rows,
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
    fill(
        frame,
        area,
        if position == TabPosition::Left {
            tab_surface(workspace)
        } else {
            Color::Indexed(234)
        },
    );
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
        active,
        workspace.panes.iter().filter(|pane| pane.parked).count(),
    ) {
        match target {
            TabTarget::Parked => {
                // The shared target is rendered with the footer in every view.
            }
            TabTarget::New => {
                let label = if position == TabPosition::Left {
                    " + New tab "
                } else {
                    "  +  "
                };
                let sidebar = position == TabPosition::Left;
                let bg = if sidebar {
                    tab_surface(workspace)
                } else {
                    Color::Indexed(234)
                };
                fill(frame, rect, bg);
                if sidebar {
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
                }
                let inset = if position == TabPosition::Left {
                    rect.cols
                        .saturating_sub(label.chars().map(char_width).sum())
                        / 2
                } else {
                    0
                };
                write_text(
                    frame,
                    rect.x + inset,
                    rect.y + rect.rows / 2,
                    rect.cols - inset,
                    label,
                    if sidebar {
                        Color::Indexed(if tab_text(workspace, false) == Color::Indexed(232) {
                            24
                        } else {
                            75
                        })
                    } else {
                        Color::Indexed(250)
                    },
                    bg,
                    !sidebar,
                );
            }
            TabTarget::Previous | TabTarget::Next => {
                let bg = if position == TabPosition::Left {
                    Color::Indexed(67)
                } else {
                    Color::Indexed(234)
                };
                fill(frame, rect, bg);
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
                    bg,
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
                    if let Some(previous) = previous_tab {
                        if position == TabPosition::Left {
                            // Use the bottom edge of the folder row without
                            // replacing its text or adding another tab row.
                            let y = previous.y + previous.rows - 1;
                            for x in previous.x + 1..previous.x + previous.cols - 1 {
                                let cell = &mut frame.cells[usize::from(y * frame.cols + x)];
                                cell.underline = true;
                                if cell.text == " " {
                                    cell.fg = Color::Indexed(243);
                                }
                            }
                        } else {
                            let bg = frame.cells[usize::from(rect.y * frame.cols + rect.x)].bg;
                            for y in rect.y..rect.y + rect.rows {
                                write_text(
                                    frame,
                                    rect.x,
                                    y,
                                    1,
                                    "│",
                                    Color::Indexed(243),
                                    bg,
                                    false,
                                );
                            }
                        }
                    }
                    previous_tab = Some(rect);
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
    let accent = tab_accent(workspace, true);
    if position == TabPosition::Left {
        let x = body.x - 1;
        for y in 0..frame.rows {
            write_text(frame, x, y, 1, "▎", accent, Color::Default, false);
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
    let grid = workspace.reading == ReadingMode::Grid;
    let horizontal = !grid
        && workspace
            .ui
            .config
            .tab_position
            .resolve(frame.cols, frame.rows)
            != super::ui::TabPosition::Left;
    let accent = tab_accent(workspace, selected);
    let bg = if grid {
        Color::Default
    } else if horizontal {
        Color::Indexed(if selected { 250 } else { 234 })
    } else if selected {
        tab_accent(workspace, true)
    } else {
        tab_background(workspace, selected)
    };
    let text_color = if horizontal {
        Color::Indexed(if selected { 232 } else { 250 })
    } else if selected && !grid {
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
    if grid {
        write_text(
            frame,
            rect.x,
            rect.y,
            rect.cols,
            &"─".repeat(usize::from(rect.cols)),
            accent,
            bg,
            false,
        );
    }
    let number = super::workspace::tab_number_for(rect, pane.number);
    write_text(
        frame,
        number.x,
        number.y,
        number.cols,
        &format!(" {} ", pane.number),
        if horizontal {
            text_color
        } else if selected {
            Color::Indexed(226)
        } else {
            tab_text(workspace, false)
        },
        bg,
        true,
    );
    let unread = workspace
        .ui
        .notifications
        .iter()
        .any(|item| item.pane == pane.id && !item.read);
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
            Color::Indexed(if horizontal && selected { 172 } else { 226 }),
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
            if horizontal {
                Color::Indexed(if selected { 232 } else { 250 })
            } else {
                Color::Indexed(231)
            },
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
        write_text(
            frame,
            rect.x,
            y,
            1,
            if grid {
                "─"
            } else if horizontal {
                " "
            } else {
                "▌"
            },
            accent,
            bg,
            false,
        );
    }
    if pane.status == crate::model::Status::Working {
        activity_ball(
            frame,
            track,
            if horizontal {
                Color::Indexed(if selected { 232 } else { 250 })
            } else if selected {
                Color::Indexed(226)
            } else {
                tab_text(workspace, false)
            },
            spinner,
        );
    }
}

fn activity_ball(frame: &mut Frame, track: Rect, foreground: Color, tick: usize) {
    if track.cols == 0 {
        return;
    }
    let index = usize::from(track.y) * usize::from(frame.cols) + usize::from(track.x);
    if let Some(cell) = frame.cells.get_mut(index) {
        cell.text = ["⣤", "⠶", "⠛", "⠶"][(tick / 2) % 4].into();
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

fn draw_sidebar_status(frame: &mut Frame, workspace: &Workspace, notice: Option<&str>) {
    let rows = super::workspace::sidebar_status_rows(frame.rows);
    let width = super::workspace::sidebar_width(frame.cols) - 1;
    let top = frame.rows - rows;
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
        write_text(frame, 2, top, width - 4, " Observer ", muted, bg, false);
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
            width - 2,
            "Observer",
            muted,
            bg,
            false,
        );
    } else {
        let value_width = width.saturating_sub(9);
        let metrics = sidebar_metrics(&workspace.ui.vitals);
        let capacity = usize::from(metric_end - metric_top);
        let visible: Vec<_> = metrics.iter().take(capacity).collect();
        for (index, metric) in visible.into_iter().enumerate() {
            let y = metric_top + index as u16;
            write_text(frame, 1, y, 6, metric.label, muted, bg, false);
            if metric.label == "Disks" {
                let total = metric
                    .values
                    .iter()
                    .map(|(value, _)| value.chars().map(char_width).sum::<u16>())
                    .sum::<u16>()
                    + metric.values.len().saturating_sub(1) as u16;
                let mut x = width - 1 - total.min(value_width);
                for (value, color) in &metric.values {
                    let size = value.chars().map(char_width).sum::<u16>();
                    if x + size > width - 1 {
                        write_text(
                            frame,
                            x,
                            y,
                            width.saturating_sub(x + 1),
                            "…",
                            muted,
                            bg,
                            false,
                        );
                        break;
                    }
                    write_text(frame, x, y, size, value, Color::Indexed(*color), bg, false);
                    x += size + 1;
                }
                continue;
            }
            let (value, color) = &metric.values[0];
            if matches!(metric.label, "CPU" | "Memory" | "Swap") {
                if let Some((bar, percent)) = value.split_once(' ') {
                    let display = format!("{bar} {percent:>4}");
                    write_text(
                        frame,
                        width - 7,
                        y,
                        6,
                        &display,
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
    let [config, mode, parked, help] = frame.controls();
    let unread = workspace.ui.notifications.iter().any(|item| !item.read);
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
            activity_ball(
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
    let query = if ui.status_query.is_empty() {
        "/ search · c CPU · m memory · p PID · r/w I/O".to_string()
    } else {
        format!("Search: {}", safe_text(&ui.status_query))
    };
    write_text(
        frame,
        x,
        tabs_y + 1,
        width,
        &query,
        Color::Indexed(245),
        bg,
        false,
    );
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
    let bar_width = rect.cols.saturating_sub(8);
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
        6,
        label,
        Color::Indexed(color),
        bg,
        false,
    );
    if bar_width > 0 {
        let start = rect.x + 6;
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
    let mut details = if two_columns {
        vec![
            (
                format!("Tasks {} ({} running)", data.tasks, data.running),
                250,
            ),
            (format!("Load {}", data.load), 250),
        ]
    } else {
        vec![
            (
                format!(
                    "Tasks {} ({} running) · Load {}",
                    data.tasks, data.running, data.load
                ),
                250,
            ),
            (clock_uptime.clone(), 39),
        ]
    };
    if two_columns {
        if status::summary_detail_rows(bounds) == 6 {
            details.push((clock, 39));
            details.push((uptime, 39));
        } else {
            details.push((clock_uptime, 39));
        }
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
    let disks: Vec<_> = data
        .disks
        .iter()
        .map(|disk| {
            (
                format!(
                    "{} {} {}% ({}/{})",
                    safe_text(&disk.mount),
                    super::vitals::pie_glyph(disk.percent),
                    disk.percent,
                    status::bytes(disk.available),
                    status::bytes(disk.total),
                ),
                if disk.percent >= 90 { 196 } else { 222 },
            )
        })
        .collect();
    if !two_columns {
        details.extend(disks.iter().cloned());
    }
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
        details.len()
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
    if two_columns {
        for (index, (text, color)) in disks
            .iter()
            .take(usize::from(height.saturating_sub(lower_y)) * 2)
            .enumerate()
        {
            write_text(
                frame,
                x + (index % 2) as u16 * (meter_width + meter_gap),
                y + lower_y + (index / 2) as u16,
                meter_width,
                text,
                Color::Indexed(*color),
                Color::Indexed(236),
                false,
            );
        }
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
                assert_eq!(frame.cells[0].bg, Color::Default);
                assert_eq!(frame.cells[5].bg, Color::Default);
                let button_start = usize::from(cols - 10);
                assert_eq!(frame.cells[button_start].text, "─");
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
                assert_eq!(frame.cells[5].bg, Color::Default);
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
