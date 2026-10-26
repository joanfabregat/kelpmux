//! Client-local hover feedback. Geometry is shared with click routing; probing
//! an action never sends it or mutates the live router/server.
use super::{input::Router, protocol::*, ui::DialogKind, workspace as ws};

pub fn target(frame: &Frame, point: Point) -> Option<Rect> {
    if !frame.controller || point.x >= frame.cols || point.y >= frame.rows {
        return None;
    }
    let close = |bounds: Rect| Rect {
        x: bounds.x + bounds.cols.saturating_sub(3),
        y: bounds.y,
        cols: 3,
        rows: 1,
    };
    let mut regions = Vec::new();
    if let Some(dialog) = frame.ui.dialog {
        let bounds = ws::dialog_bounds_for(frame.cols, frame.rows, dialog.kind);
        regions.push(close(bounds));
        match dialog.kind {
            DialogKind::Config => regions.extend(
                ws::config_targets(bounds)
                    .into_iter()
                    .map(|(_, _, rect)| rect),
            ),
            DialogKind::CloseConfirm | DialogKind::ParkConfirm | DialogKind::RestoreConfirm => {
                regions.extend(ws::close_buttons(bounds))
            }
            DialogKind::Color => regions.extend(ws::color_cards(bounds)),
            DialogKind::Grid => regions.extend(ws::grid_cards(bounds)),
            DialogKind::Move => regions.extend(ws::move_cards(
                bounds,
                frame.ui.grid_shape.0,
                frame.ui.grid_shape.1,
            )),
            DialogKind::Parked => {
                let count = frame.panes.iter().filter(|view| view.pane.parked).count();
                for (_, rect) in ws::parked_cards(bounds, dialog.selection, count) {
                    regions.extend(ws::parked_controls(rect));
                    regions.push(rect);
                }
            }
            DialogKind::Status => {
                let y = bounds.y + 1 + super::status::summary_height(bounds, &frame.ui.status);
                regions.push(Rect {
                    x: bounds.x + 1,
                    y,
                    cols: 9,
                    rows: 1,
                });
                regions.push(Rect {
                    x: bounds.x + 10,
                    y,
                    cols: 6.min(bounds.cols.saturating_sub(11)),
                    rows: 1,
                });
                let mut x = bounds.x + 1;
                for (_, width, sort) in
                    super::status::columns(bounds.cols - 2, frame.ui.status_view, &frame.ui.status)
                {
                    if sort.is_some() {
                        regions.push(Rect {
                            x,
                            y: super::status::table_y(bounds, &frame.ui.status),
                            cols: width,
                            rows: 1,
                        });
                    }
                    x += width;
                }
            }
            _ => {
                let first = bounds.y
                    + if dialog.kind == DialogKind::Search {
                        4
                    } else {
                        2
                    };
                if point.y >= first && point.y < bounds.y + bounds.rows.saturating_sub(1) {
                    regions.push(Rect {
                        x: bounds.x + 1,
                        y: point.y,
                        cols: bounds.cols - 2,
                        rows: 1,
                    });
                }
            }
        }
    } else if frame.help {
        regions.push(close(super::input::help_bounds(frame.cols, frame.rows)));
        regions.push(frame.controls()[3]);
    } else if frame.ui.scratch_visible {
        if let Some(view) = frame
            .panes
            .iter()
            .find(|view| Some(view.pane.id) == frame.ui.scratch)
        {
            regions.push(close(view.rect));
        }
    } else {
        regions.extend(frame.controls());
        if super::status::metric_target(frame, point).is_some() {
            let (x, cols) = if frame.sidebar_status() {
                (0, ws::sidebar_width(frame.cols) - 1)
            } else {
                (
                    frame.footer_cols(),
                    frame.cols.saturating_sub(frame.footer_cols()),
                )
            };
            regions.push(Rect {
                x,
                y: point.y,
                cols,
                rows: 1,
            });
        }
        if frame.reading != ReadingMode::Grid || frame.active == 0 {
            let panes: Vec<_> = frame
                .panes
                .iter()
                .filter(|view| !view.pane.parked && Some(view.pane.id) != frame.ui.scratch)
                .collect();
            let active = panes
                .iter()
                .position(|view| view.pane.id == frame.active)
                .unwrap_or(0);
            for (kind, rect) in ws::tab_targets(
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
            ) {
                if let ws::TabTarget::Pane(_) = kind {
                    regions.push(ws::tab_close(rect));
                    regions.push(ws::tab_park(rect));
                }
                regions.push(rect);
            }
        } else {
            for view in frame.panes.iter().filter(|view| {
                !view.pane.parked && !view.overlay && view.rect.cols > 0 && view.rect.rows > 0
            }) {
                let header = Rect {
                    rows: 1,
                    ..view.rect
                };
                regions.extend(ws::grid_controls(header));
                regions.push(header);
            }
        }
    }
    let rect = regions
        .into_iter()
        .find(|rect| rect.cols > 0 && rect.rows > 0 && rect.contains(point))?;
    let click = InputEvent::Mouse(MouseEvent {
        position: point,
        kind: MouseKind::Press,
        button: 0,
        shift: false,
        alt: false,
        ctrl: false,
    });
    let actionable = Router::default()
        .route(click, frame)
        .iter()
        .any(|message| matches!(message, ClientMessage::Action(_)));
    actionable.then_some(rect)
}

pub fn highlight(cell: &mut Cell, theme: [Option<[u8; 3]>; 2]) {
    let background = if cell.bg == Color::Default {
        theme[1].map_or(Color::Default, |[r, g, b]| Color::Rgb(r, g, b))
    } else {
        cell.bg
    };
    let (r, g, b) = super::render::color_rgb(background, false);
    let light = u16::from(r) * 3 + u16::from(g) * 6 + u16::from(b) >= 1600;
    let [r, g, b] = [r, g, b].map(|v| {
        if light {
            v.saturating_sub(24)
        } else {
            v.saturating_add(24)
        }
    });
    cell.bg = Color::Rgb(r, g, b);
    cell.bold = true;
}
