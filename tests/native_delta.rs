use kelpmux::native::{delta, protocol::*, ui::UiState};
fn frame(cols: u16, rows: u16) -> Frame {
    Frame {
        cols,
        rows,
        cells: vec![Cell::default(); usize::from(cols) * usize::from(rows)],
        cursor: None,
        panes: vec![],
        active: 1,
        reading: ReadingMode::Grid,
        controller: true,
        help: false,
        notice: None,
        ui: UiState::default(),
    }
}
#[test]
fn metadata_and_patches_accept_large_workspaces() {
    let mut ws = kelpmux::native::workspace::Workspace::new(80, 24).unwrap();
    ws.panes = (1..=1000)
        .map(|id| PaneInfo {
            id,
            ..Default::default()
        })
        .collect();
    ws.active = 1000;
    ws.ui.config.pane_colors = (1..=1000).map(|id| (id, 0)).collect();
    ws.ui.config.validate().unwrap();
    let old = kelpmux::native::render::compose(&ws, &Default::default(), true, None, 0);
    let mut new = old.clone();
    new.cells[100].text = "x".into();
    let patch = delta::diff(&old, &new, 1).unwrap();
    assert_eq!(delta::apply(&old, 1, &patch).unwrap(), new);
}

#[test]
fn single_cell_large_screen_patch_replays_with_a_strong_wire_size_reduction() {
    let old = frame(400, 160);
    let mut new = old.clone();
    new.cells[63999].text = "x".into();
    new.cells[63999].fg = Color::Rgb(12, 34, 56);
    new.cursor = Some(Cursor {
        position: Point { x: 399, y: 159 },
        visible: true,
    });
    let patch = delta::diff(&old, &new, 1).unwrap();
    assert_eq!(patch.changes.len(), 1);
    assert_eq!(delta::apply(&old, 1, &patch).unwrap(), new);
    let full = serde_json::to_vec(&new).unwrap().len();
    let reduced = serde_json::to_vec(&patch).unwrap().len();
    assert!(reduced * 1000 < full, "full={full},patch={reduced}");
    println!(
        "400x160 one-cell full={full} bytes patch={reduced} bytes reduction={}x",
        full / reduced
    );
}
#[test]
fn patches_preserve_wide_glyph_styles_and_metadata_and_reject_revision_gaps() {
    let old = frame(80, 24);
    let mut new = old.clone();
    new.cells[0] = Cell {
        text: "界".into(),
        width: 2,
        fg: Color::Indexed(42),
        bg: Color::Rgb(3, 4, 5),
        bold: true,
        italic: true,
        underline: true,
        underline_color: Some(Color::Indexed(238)),
        hyperlink: Some("https://example.com/wide".into()),
    };
    new.cells[1] = Cell {
        text: String::new(),
        width: 0,
        ..Cell::default()
    };
    new.cells[2].text = "e\u{301}".into();
    new.notice = Some("updated".into());
    new.help = true;
    let patch = delta::diff(&old, &new, 5).unwrap();
    assert_eq!(delta::apply(&old, 5, &patch).unwrap(), new);
    assert!(delta::apply(&old, 4, &patch).is_err());
    assert!(delta::apply(&old, 6, &patch).is_err());
    let mut corrupt = patch.clone();
    corrupt.revision += 1;
    assert!(delta::apply(&old, 5, &corrupt).is_err());
    assert!(delta::diff(&old, &new, u64::MAX).is_none());
}
#[test]
fn malformed_patch_bounds_cells_and_inlined_frames_are_refused() {
    let old = frame(80, 24);
    let mut new = old.clone();
    new.cells[0].text = "x".into();
    let good = delta::diff(&old, &new, 1).unwrap();
    for variant in 0..6 {
        let mut patch = good.clone();
        match variant {
            0 => patch.changes.push(patch.changes[0].clone()),
            1 => patch.changes[0].0 = 1920,
            2 => patch.changes[0].1.text = "\x1b[31m".into(),
            3 => patch.changes[0].1.text = "界".into(),
            4 => patch.frame.cells.push(Cell::default()),
            5 => patch.frame.cols = 81,
            _ => unreachable!(),
        };
        assert!(delta::apply(&old, 1, &patch).is_err(), "variant{variant}");
    }
}
#[test]
fn resize_and_large_changes_use_full_frames_and_metadata_only_updates_still_replay() {
    let old = frame(40, 10);
    let mut resized = frame(80, 24);
    assert!(delta::diff(&old, &resized, 1).is_none());
    resized = old.clone();
    for cell in &mut resized.cells {
        cell.text = "x".into();
    }
    assert!(delta::diff(&old, &resized, 1).is_none());
    let mut metadata = old.clone();
    metadata.controller = false;
    metadata.notice = Some("observer".into());
    let patch = delta::diff(&old, &metadata, 8).unwrap();
    assert!(patch.changes.is_empty());
    assert_eq!(delta::apply(&old, 8, &patch).unwrap(), metadata);
}
#[test]
fn many_sequential_updates_replay_and_geometry_reordering_requires_full() {
    let mut client = frame(40, 10);
    let mut revision = 1;
    for index in 0..100 {
        let mut next = client.clone();
        next.cells[index * 3].text = ((b'a' + (index % 26) as u8) as char).to_string();
        next.cells[index * 3].fg = Color::Indexed(index as u8);
        next.notice = Some(format!("update {index}"));
        let patch = delta::diff(&client, &next, revision).unwrap();
        client = delta::apply(&client, revision, &patch).unwrap();
        revision = patch.revision;
        assert_eq!(client, next);
    }
    let mut changed = client.clone();
    changed.panes.push(PaneView {
        pane: PaneInfo {
            id: 1,
            ..PaneInfo::default()
        },
        rect: Rect {
            x: 0,
            y: 0,
            cols: 40,
            rows: 9,
        },
        body: Rect {
            x: 0,
            y: 1,
            cols: 40,
            rows: 8,
        },
        overlay: false,
        modes: InputModes::default(),
    });
    assert!(delta::diff(&client, &changed, revision).is_none());
    let mut moved = changed.clone();
    moved.panes[0].body.rows = 7;
    assert!(delta::diff(&changed, &moved, revision).is_none());
}
