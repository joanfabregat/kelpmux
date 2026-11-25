use super::*;
use serde_json::json;
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

#[test]
fn clipboard_events_are_bounded_ephemeral_messages() {
    let mut client = Client::default();
    assert!(
        client
            .accept(&json!({"Stream":{"type":"clipboard","pane":1,"text":"copied"}}))
            .is_ok()
    );
    assert!(client.screens.is_empty());
    for event in [
        json!({"Stream":{"type":"clipboard","pane":0,"text":"bad"}}),
        json!({"Stream":{"type":"clipboard","pane":1,"text":"x".repeat(65537)}}),
        json!({"Stream":{"type":"clipboard","pane":1,"text":false}}),
    ] {
        assert!(client.accept(&event).is_err());
    }
}

#[test]
fn upload_events_are_checked_before_their_paths_can_be_pasted() {
    let mut client = Client::default();
    for event in [
        json!({"Stream":{"type":"upload_started","request":"u-1","batch":1,"dir":"/home/me/.cache/kelpserver/uploads/1-ab"}}),
        json!({"Stream":{"type":"upload_progress","batch":1,"file_index":0,"received":262144}}),
        json!({"Stream":{"type":"upload_done","request":"u-2","batch":1,"paths":["/up/1-ab/a b.png","/up/1-ab/it's.txt"]}}),
        json!({"Stream":{"type":"upload_failed","batch":1,"message":"upload exceeds the declared size of file 0"}}),
        json!({"Stream":{"type":"done","request":"u-3"}}),
    ] {
        assert!(client.accept(&event).is_ok(), "{event}");
    }
    for event in [
        // A path with a newline would run a command once pasted.
        json!({"Stream":{"type":"upload_done","request":"u-2","batch":1,"paths":["/up/a\nrm -rf ~"]}}),
        json!({"Stream":{"type":"upload_done","request":"u-2","batch":1,"paths":["relative.txt"]}}),
        json!({"Stream":{"type":"upload_done","request":"u 2","batch":1,"paths":[]}}),
        json!({"Stream":{"type":"upload_started","request":"u-1","batch":-1,"dir":"/up"}}),
        json!({"Stream":{"type":"upload_started","request":"u-1","batch":1,"dir":"/up/\u{1b}[2J"}}),
        json!({"Stream":{"type":"upload_progress","batch":1,"file_index":0}}),
        json!({"Stream":{"type":"upload_failed","batch":1}}),
        json!({"Stream":{"type":"done","request":""}}),
    ] {
        assert!(client.accept(&event).is_err(), "{event}");
    }
}

#[test]
fn file_replies_reach_the_page_when_well_formed() {
    let hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let mut client = Client::default();
    for event in [
        json!({"Stream":{"type":"file_list","request":"file-1","path":"/home/me","entries":[
            {"name":"src","kind":"dir","size":4096,"modified":1795300000,"mode":493},
            {"name":"current","kind":"symlink","size":7,"modified":1795300000,"mode":511,"target":"release"},
            {"name":"line\nbreak.txt","kind":"file","size":0,"modified":-5,"mode":420}],
            "truncated":false,"skipped":1}}),
        json!({"Stream":{"type":"file_stat","request":"file-2","path":"/","entry":{"name":"/","kind":"dir","size":4096,"modified":1,"mode":493}}}),
        json!({"Stream":{"type":"file_content","request":"file-3","path":"/home/me/a.txt","size":0,"modified":1,"mode":420,"hash":hash,"binary":false,"encoding":"utf8","content":""}}),
        json!({"Stream":{"type":"file_content","request":"file-3","path":"/home/me/a.bin","size":1,"modified":1,"mode":420,"hash":hash,"binary":true,"encoding":"base64","content":"AA=="}}),
        json!({"Stream":{"type":"file_written","request":"file-4","path":"/home/me/a.txt","hash":hash,"size":3,"modified":2,"created":true}}),
        json!({"Stream":{"type":"file_removed","request":"file-5","removed":[{"path":"/home/me/old","trashed_as":"/home/me/.local/share/Trash/files/old"},{"path":"/home/me/gone"}],
            "failed":[{"path":"/home/me/build","error":{"code":"permission_denied","message":"denied","path":"/home/me/build"}}]}}),
        json!({"Stream":{"type":"file_error","request":"file-6","error":{"code":"conflict","message":"the file changed since it was read","path":"/home/me/a.txt","current_hash":hash}}}),
        json!({"Stream":{"type":"file_error","request":"file-7","error":{"code":"too_large","message":"too large","size":99999999}}}),
        // Codes are open: `outside_root` (the file root) passes like any.
        json!({"Stream":{"type":"file_error","request":"file-8","error":{"code":"outside_root","message":"the path is outside the file root","path":"/etc/passwd"}}}),
    ] {
        assert!(client.accept(&event).is_ok(), "{event}");
    }
    for event in [
        json!({"Stream":{"type":"file_list","request":"file 1","path":"/home/me","entries":[],"truncated":false,"skipped":0}}),
        json!({"Stream":{"type":"file_list","request":"file-1","path":"home/me","entries":[],"truncated":false,"skipped":0}}),
        json!({"Stream":{"type":"file_list","request":"file-1","path":"/home/me","entries":[{"name":"x","kind":"pipe","size":0,"modified":0,"mode":0}],"truncated":false,"skipped":0}}),
        json!({"Stream":{"type":"file_list","request":"file-1","path":"/home/me","entries":[{"name":"","kind":"file","size":0,"modified":0,"mode":0}],"truncated":false,"skipped":0}}),
        json!({"Stream":{"type":"file_list","request":"file-1","path":"/home/me","entries":[],"skipped":0}}),
        json!({"Stream":{"type":"file_stat","request":"file-2","path":"/x","entry":{}}}),
        json!({"Stream":{"type":"file_content","request":"file-3","path":"/a","size":0,"modified":1,"mode":420,"hash":"zz","binary":false,"encoding":"utf8","content":""}}),
        json!({"Stream":{"type":"file_content","request":"file-3","path":"/a","size":0,"modified":1,"mode":420,"hash":hash,"binary":false,"encoding":"utf16","content":""}}),
        json!({"Stream":{"type":"file_written","request":"file-4","path":"/a","hash":hash,"size":3,"modified":2}}),
        json!({"Stream":{"type":"file_removed","request":"file-5","removed":[{"path":"relative"}],"failed":[]}}),
        json!({"Stream":{"type":"file_removed","request":"file-5","removed":[],"failed":[{"path":"/a","error":{"code":"io"}}]}}),
        json!({"Stream":{"type":"file_error","request":"file-6","error":{"code":"conflict","message":"x","current_hash":"nothex"}}}),
        json!({"Stream":{"type":"file_error","request":"file-6"}}),
    ] {
        assert!(client.accept(&event).is_err(), "{event}");
    }
}

#[test]
fn download_events_are_checked() {
    let hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let mut client = Client::default();
    for event in [
        json!({"Stream":{"type":"file_download_started","request":"file-1","download":1,"name":"a.txt","size":3}}),
        json!({"Stream":{"type":"file_download_started","request":"file-2","download":2,"name":"src.tar.gz","archive":"tar.gz","entries":4,"input_bytes":900}}),
        json!({"Stream":{"type":"file_download_chunk","download":1,"offset":0,"data":"YWJj"}}),
        json!({"Stream":{"type":"file_download_chunk","download":2,"offset":0,"data":"H4sI","input":512}}),
        json!({"Stream":{"type":"file_download_done","request":"file-1","download":1,"size":3,"hash":hash,"skipped":0,"changed":0}}),
        json!({"Stream":{"type":"file_error","request":"file-2","download":2,"error":{"code":"io","message":"read failed"}}}),
    ] {
        assert!(client.accept(&event).is_ok(), "{event}");
    }
    for event in [
        json!({"Stream":{"type":"file_download_started","request":"file-1","download":1,"name":""}}),
        json!({"Stream":{"type":"file_download_started","request":"file-1","name":"a"}}),
        json!({"Stream":{"type":"file_download_started","request":"file-1","download":1,"name":"a","archive":"zip"}}),
        json!({"Stream":{"type":"file_download_chunk","download":1,"offset":0,"data":""}}),
        json!({"Stream":{"type":"file_download_chunk","download":1,"data":"YWJj"}}),
        json!({"Stream":{"type":"file_download_chunk","download":1,"offset":0,"data":"A".repeat(4 * 1024 * 1024 + 1)}}),
        json!({"Stream":{"type":"file_download_done","request":"file-1","download":1,"size":3,"hash":"zz","skipped":0,"changed":0}}),
        json!({"Stream":{"type":"file_download_done","request":"file-1","download":1,"size":3,"hash":hash}}),
        json!({"Stream":{"type":"file_error","request":"file-2","download":"x","error":{"code":"io","message":"x"}}}),
    ] {
        assert!(client.accept(&event).is_err(), "{event}");
    }
}

// Binary screens (`binary-screens`): the same stream sent as JSON screens to
// one client and as binary records to another leaves both with the same
// screens, and the binary client's events carry the screens as JSON.
mod binary {
    use super::*;
    use kelpclient_protocol::screens::{self, RecordBuilder};
    fn frame(payload: &[u8]) -> Vec<u8> {
        let mut out = (payload.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(payload);
        out
    }
    fn json_frame(value: &Value) -> Vec<u8> {
        frame(&serde_json::to_vec(value).unwrap())
    }
    fn welcome() -> Vec<u8> {
        json_frame(
            &json!({"Stream":{"type":"welcome","version":1,"client":7,"epoch":"e",
            "capabilities":["binary-screens"],"limits":{"subscriptions":10,"subscribed_cells":100000}}}),
        )
    }
    fn update(snapshots: &[PaneSnapshot], patches: &[PanePatch], binary: bool) -> Value {
        let panes: Vec<PaneInfo> = [1, 2]
            .into_iter()
            .map(|id| PaneInfo {
                id,
                ..PaneInfo::default()
            })
            .collect();
        let mut event = json!({"Stream":{"type":"update","epoch":"e","subscribed":[1,2],
            "snapshots":snapshots,"patches":patches,"panes":panes,"leases":[],
            "notifications":[],"vitals":{}}});
        if binary {
            event["Stream"]["binary_screens"] = json!(true);
            event["Stream"]["snapshots"] = json!([]);
            event["Stream"]["patches"] = json!([]);
        }
        event
    }
    fn cell(text: &str, fg: Color, bold: bool) -> Cell {
        Cell {
            text: text.into(),
            fg,
            bold,
            ..Cell::default()
        }
    }
    fn snapshot(pane: PaneId, revision: u64, cols: u16, rows: u16, seed: u8) -> PaneSnapshot {
        let mut cells = vec![Cell::default(); usize::from(cols) * usize::from(rows)];
        for (i, c) in cells.iter_mut().enumerate().take(usize::from(cols) * 2) {
            *c = cell(
                &((b'a' + (i as u8 + seed) % 26) as char).to_string(),
                Color::Indexed(seed.wrapping_add(i as u8)),
                i % 3 == 0,
            );
        }
        cells[usize::from(cols)] = Cell {
            text: "界".into(),
            width: 2,
            ..Cell::default()
        };
        cells[usize::from(cols) + 1] = Cell {
            text: String::new(),
            width: 0,
            ..Cell::default()
        };
        PaneSnapshot {
            pane,
            revision,
            screen: Screen {
                cols,
                rows,
                cells,
                cursor: Some(Cursor {
                    position: Point { x: 1, y: 1 },
                    visible: true,
                }),
                modes: InputModes::default(),
            },
        }
    }
    /// A patch changing two cells of row 1 (and the cursor).
    fn patch(base: &PaneSnapshot) -> (PanePatch, PaneSnapshot) {
        let cols = usize::from(base.screen.cols);
        let patch = PanePatch {
            pane: base.pane,
            base: base.revision,
            revision: base.revision + 1,
            cursor: Some(Cursor {
                position: Point { x: 3, y: 2 },
                visible: false,
            }),
            modes: InputModes {
                bracketed_paste: true,
                ..InputModes::default()
            },
            changes: vec![
                (cols + 4, cell("x", Color::Rgb(1, 2, 3), true)),
                (cols * 2 + 5, cell("y", Color::Default, false)),
            ],
        };
        let next = apply(base, &patch).unwrap();
        (patch, next)
    }
    /// Feeds one update to a JSON client and a binary client and checks they
    /// agree.
    fn both(
        json: &mut Client,
        binary: &mut Client,
        snapshots: &[PaneSnapshot],
        patches: &[(PanePatch, Screen)],
    ) -> (Vec<Value>, Vec<Value>) {
        let plain: Vec<PanePatch> = patches.iter().map(|(p, _)| p.clone()).collect();
        let from_json = json
            .receive(&json_frame(&update(snapshots, &plain, false)))
            .unwrap();
        let mut record = RecordBuilder::new();
        for s in snapshots {
            record.snapshot(s.pane, s.revision, &s.screen);
        }
        for (p, screen) in patches {
            record.patch(p, screen);
        }
        let mut bytes = json_frame(&update(&[], &[], true));
        bytes.extend(frame(&record.finish()));
        let from_binary = binary.receive(&bytes).unwrap();
        assert_eq!(json.screens, binary.screens);
        assert_eq!(from_json.len(), 1);
        assert_eq!(from_binary.len(), 1);
        let event = &from_binary[0]["Stream"];
        assert!(event.get("binary_screens").is_none());
        assert_eq!(event["snapshots"], from_json[0]["Stream"]["snapshots"]);
        (from_json, from_binary)
    }
    fn clients() -> (Client, Client) {
        let mut json = Client::default();
        let mut binary = Client::default();
        json.receive(&welcome()).unwrap();
        binary.receive(&welcome()).unwrap();
        (json, binary)
    }

    #[test]
    fn binary_updates_yield_the_screens_of_json_updates() {
        let (mut json, mut binary) = clients();
        let one = snapshot(1, 1, 20, 4, 0);
        let two = snapshot(2, 1, 12, 3, 5);
        both(&mut json, &mut binary, &[one.clone(), two.clone()], &[]);
        // A patch: the binary event carries every cell of each changed row,
        // which applies to the same screen as the JSON patch.
        let (p, next) = patch(&one);
        let (_, events) = both(&mut json, &mut binary, &[], &[(p, next.screen.clone())]);
        let changes = events[0]["Stream"]["patches"][0]["changes"]
            .as_array()
            .unwrap();
        assert_eq!(changes.len(), 2 * 20);
        assert_eq!(binary.screens[&1], next);
        // A resize starts a new revision chain with a snapshot.
        let resized = snapshot(1, 1, 30, 6, 9);
        let (p2, next2) = patch(&two);
        both(
            &mut json,
            &mut binary,
            std::slice::from_ref(&resized),
            &[(p2, next2.screen.clone())],
        );
        assert_eq!(binary.screens[&1], resized);
        assert_eq!(binary.screens[&2], next2);
        // An empty record (an update without screen changes).
        both(&mut json, &mut binary, &[], &[]);
    }

    #[test]
    fn formats_share_baselines() {
        // A JSON snapshot, then a binary patch on it, then a JSON patch.
        let (_, mut client) = clients();
        let one = snapshot(1, 1, 10, 3, 1);
        let two = snapshot(2, 1, 10, 3, 2);
        client
            .receive(&json_frame(&update(&[one.clone(), two], &[], false)))
            .unwrap();
        let (p, next) = patch(&one);
        let mut bytes = json_frame(&update(&[], &[], true));
        bytes.extend(frame(&screens::encode_patch(&p, &next.screen)));
        client.receive(&bytes).unwrap();
        assert_eq!(client.screens[&1], next);
        let (p, after) = patch(&next);
        client
            .receive(&json_frame(&update(&[], &[p], false)))
            .unwrap();
        assert_eq!(client.screens[&1], after);
    }

    #[test]
    fn an_update_and_its_record_may_arrive_in_any_fragments() {
        let (_, mut client) = clients();
        let one = snapshot(1, 1, 8, 2, 0);
        let two = snapshot(2, 1, 8, 2, 3);
        let mut record = RecordBuilder::new();
        record.snapshot(1, 1, &one.screen);
        record.snapshot(2, 1, &two.screen);
        let mut bytes = json_frame(&update(&[], &[], true));
        bytes.extend(frame(&record.finish()));
        let mut events = Vec::new();
        for byte in bytes {
            events.extend(client.receive(&[byte]).unwrap());
        }
        assert_eq!(events.len(), 1);
        assert_eq!(client.screens[&1], one);
        assert_eq!(client.screens[&2], two);
    }

    #[test]
    fn misplaced_or_invalid_records_are_protocol_errors() {
        let one = snapshot(1, 1, 8, 3, 0);
        let two = snapshot(2, 1, 8, 2, 3);
        let mut record = RecordBuilder::new();
        record.snapshot(1, 1, &one.screen);
        record.snapshot(2, 1, &two.screen);
        let record = frame(&record.finish());
        let announce = json_frame(&update(&[], &[], true));
        // A record without its update.
        let (_, mut client) = clients();
        assert!(client.receive(&record).is_err());
        // A JSON record where the binary record must follow.
        let (_, mut client) = clients();
        let mut bytes = announce.clone();
        bytes.extend(json_frame(&update(&[one.clone(), two.clone()], &[], false)));
        assert!(client.receive(&bytes).is_err());
        // Two updates in a row.
        let (_, mut client) = clients();
        let mut bytes = announce.clone();
        bytes.extend(announce.clone());
        assert!(client.receive(&bytes).is_err());
        // An announcing update that also carries JSON screens.
        let (_, mut client) = clients();
        let mut carrying = update(&[one.clone(), two.clone()], &[], false);
        carrying["Stream"]["binary_screens"] = json!(true);
        assert!(client.receive(&json_frame(&carrying)).is_err());
        // A corrupt record.
        let (_, mut client) = clients();
        let mut bytes = announce.clone();
        let mut corrupt = record.clone();
        corrupt.truncate(corrupt.len() - 1);
        let length = (corrupt.len() - 4) as u32;
        corrupt[..4].copy_from_slice(&length.to_be_bytes());
        bytes.extend(corrupt);
        assert!(client.receive(&bytes).is_err());
        // A record whose screens break the update's subscription.
        let (_, mut client) = clients();
        let mut bytes = announce.clone();
        bytes.extend(frame(&screens::encode_snapshot(&snapshot(3, 1, 8, 2, 0))));
        assert!(client.receive(&bytes).is_err());
        assert!(client.screens.is_empty());
        // A patch without a baseline.
        let (_, mut client) = clients();
        let (p, next) = patch(&one);
        let mut bytes = announce.clone();
        bytes.extend(frame(&screens::encode_patch(&p, &next.screen)));
        assert!(client.receive(&bytes).is_err());
        // `accept` alone cannot take an update whose screens it never sees.
        let (_, mut client) = clients();
        assert!(client.accept(&update(&[], &[], true)).is_err());
        // The record's cell budget is the Welcome's advertised one.
        let mut client = Client::default();
        client
            .receive(&json_frame(&json!({"Stream":{"type":"welcome","version":1,"client":7,
                "epoch":"e","capabilities":[],"limits":{"subscriptions":10,"subscribed_cells":20}}})))
            .unwrap();
        let mut bytes = announce;
        bytes.extend(record);
        assert!(client.receive(&bytes).is_err());
    }
}
