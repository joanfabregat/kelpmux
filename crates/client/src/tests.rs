use super::*;
#[test]
fn history_layout_preserves_wide_combining_and_joined_glyph_columns() {
    let s = history_screen(&["A界e\u{301}👩\u{200d}💻Z".into()], 12, 2).unwrap();
    assert_eq!(s.cells.len(), 24);
    assert_eq!(s.cells[1].text, "界");
    assert_eq!(s.cells[1].width, 2);
    assert_eq!(s.cells[2].width, 0);
    assert_eq!(s.cells[3].text, "e\u{301}");
    assert_eq!(s.cells[4].text, "👩\u{200d}💻");
    assert_eq!(s.cells[4].width, 2);
    assert_eq!(s.cells[6].text, "Z");
    assert!(history_screen(&[], 401, 1).is_err());
}
fn screen() -> PaneSnapshot {
    PaneSnapshot {
        pane: 1,
        revision: 1,
        screen: Screen {
            cols: 3,
            rows: 2,
            cells: vec![Cell::default(); 6],
            cursor: None,
            modes: InputModes::default(),
        },
    }
}
#[test]
fn invalid_patches_are_transactional() {
    let s = screen();
    let p = PanePatch {
        pane: 1,
        base: 1,
        revision: 2,
        cursor: None,
        modes: InputModes::default(),
        changes: vec![(7, Cell::default())],
    };
    assert!(apply(&s, &p).is_err());
    assert_eq!(s.revision, 1);
    let mut duplicate = p.clone();
    duplicate.changes = vec![(1, Cell::default()), (1, Cell::default())];
    assert!(apply(&s, &duplicate).is_err());
    let mut stale = p.clone();
    stale.base = 9;
    stale.changes.clear();
    assert!(apply(&s, &stale).is_err());
}
#[test]
fn fragmented_stream_is_bounded_and_epoch_checked() {
    let mut c = Client::default();
    let event = serde_json::json!({"Stream":{"type":"welcome","version":1,"client":1,"epoch":"first","capabilities":[]}});
    let payload = serde_json::to_vec(&event).unwrap();
    let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
    frame.extend(payload);
    for (i, byte) in frame.iter().enumerate() {
        let got = c.receive(&[*byte]).unwrap();
        assert_eq!(got.len(), usize::from(i == frame.len() - 1));
    }
    assert_eq!(c.epoch, "first");
    assert!(c.accept(&event).is_err());
    assert!(
        c.accept(&serde_json::json!({"Stream":{"type":"update","epoch":"other"}}))
            .is_err()
    );
    assert!(
        Client::default()
            .receive(&[0xff, 0xff, 0xff, 0xff])
            .is_err()
    );
}
#[test]
fn selection_preserves_wide_and_combining_cells() {
    let mut c = Client::default();
    let mut s = screen();
    s.screen.cells[0].text = "a\u{301}".into();
    s.screen.cells[1].text = "界".into();
    s.screen.cells[1].width = 2;
    s.screen.cells[2].text = "".into();
    s.screen.cells[2].width = 0;
    c.screens.insert(1, s);
    assert_eq!(
        c.selection(1, Point { x: 0, y: 0 }, Point { x: 2, y: 0 })
            .unwrap(),
        "a\u{301}界"
    );
    assert!(
        c.selection(1, Point { x: 0, y: 0 }, Point { x: 3, y: 0 })
            .is_err()
    );
}
