# Binary screen records (`binary-screens`)

Pane snapshots and patches have a compact binary encoding, negotiated per connection ([ADR 0003](../../../docs/adr/0003-kelpcore-shared-repository.md) section 2). Control messages (pane lists, leases, notifications, requests and replies) stay JSON. A connection that does not opt in receives exactly the JSON it received before. The only implementation of the format is `kelpclient_protocol::screens` in [`crates/protocol`](../../protocol/src/screens.rs), which builds for `wasm32-unknown-unknown` so browsers decode with the same code through the client's WASM core.

In JSON every cell is an object of about 100 bytes, so a 300 × 80 snapshot is about 2.4 MB. Measured with the crate's `size_report_json_versus_binary` test:

| Screen (300 × 80) | JSON | Binary |
| --- | --- | --- |
| Text-heavy shell output (prompts, coloured words, most lines shorter than the terminal) | 2,406,192 bytes | 15,599 bytes (154× smaller) |
| Mostly blank (three prompt lines) | 2,376,742 bytes | 333 bytes |
| Full-screen program with random styles, wide characters and coloured background bars | 2,530,839 bytes | 31,952 bytes (79× smaller) |
| Patch: 20 cells changed in one row | 2,435 bytes | 353 bytes |

## Negotiation

1. The server lists `binary-screens` in Welcome `capabilities`.
2. After Welcome, a client that supports the format sends `{"Stream":{"type":"binary_screens","enabled":true}}`. `enabled: false` returns to JSON screens. Never send this request to a server without the capability: an older server disconnects on an unknown request type.
3. From the next update on, each `update` event to that connection carries `"binary_screens":true` and empty `snapshots` and `patches`, and is immediately followed by exactly one binary record holding that update's screens. The record follows even when the update has no screen change (an empty record is three bytes, `00 01 00`). Updates without `binary_screens` (sent before the opt-in took effect) carry their screens in JSON as before.

The opt-in is a separate request rather than a field of `hello` because `hello` is decoded with unknown fields denied: an older server would reject a `hello` carrying a new field and close the connection. Gating a request on a Welcome capability is how every other optional feature of the session API is negotiated.

Revision rules do not change: a snapshot starts revision 1, a patch applies only to the exact baseline revision of the same pane in the same epoch, and geometry changes, `resync` and backpressure produce snapshots. Switching formats keeps baselines: a binary patch may apply to a screen that arrived as JSON, and the other way round.

## Framing

The framing is unchanged: a four-byte big-endian payload length, then the payload, at most 16 MiB. A binary record's payload starts with the byte `0x00` (`screens::RECORD_MARKER`). JSON payloads start with `{` or `"` and never with a NUL byte, so a reader tells the two apart from the first payload byte (`screens::is_binary_record`). The server sends binary records only to a connection that opted in, only directly after an `update`, and never accepts one from a client.

The `update` and its binary record are written as one packet. The server keeps one in-flight packet and one replaceable update per connection; a later update replaces the pending packet as a whole, so the backpressure replacement never separates an update from its screens and never sends one without the other.

## Bridges

`kelpserver bridge` relays binary records from the server to the client unchanged, without parsing them, once the server's Welcome has been relayed; client-to-server records must still be JSON stream requests. Any other bridge or relay must do the same: forward a record whose payload starts with `0x00` as opaque bytes. The browser `Session` opts in whenever Welcome lists the capability, so a bridge between a browser and the server must forward binary records: KelpMux's `kelpmux web-bridge` does from the KelpMux change that adopted binary screens, while earlier ones decode every server record as JSON and re-serialize it, which fails on a binary record. Applications deploy their web assets and their bridge from the same build.

## Format, version 1

All integers marked *varint* are unsigned LEB128: seven bits per byte, least significant group first, the high bit set on every byte but the last, at most ten bytes and at most 2⁶⁴ − 1. Encoders emit the shortest form. Other integers are single bytes. Lengths are in bytes unless stated otherwise.

```text
record   = 0x00  version:u8 (= 1)  count:varint (≤ 256)  message{count}
message  = kind:u8  pane:varint  revision:varint  [base:varint]  cols:varint  rows:varint
           cursor  modes  styles  body
kind     = 1 (snapshot) | 2 (patch; base present, revision = base + 1)
cursor   = 0x00 (none) | flags:u8 (1 = hidden, 3 = visible)  x:varint  y:varint
modes    = flags:u8  keyboard_flags:u8
styles   = count:varint  style{count}
style    = flags:u8  fg:colour  bg:colour  [underline_colour:colour]  [len:varint  uri:len bytes]
colour   = 0x00 (default) | 0x01 index:u8 | 0x02 r:u8 g:u8 b:u8
body     = row{rows}                                    (snapshot)
         | changed:varint  (row_index:varint  row){changed}  (patch)
row      = runs:varint  run{runs}
run      = header:varint (length << 2 | run_kind)  style_index:varint  payload
```

Messages: `pane` and `revision` are non-zero; a record names each pane at most once; snapshots and patches keep the order the server built them in. `cols` is 1–400 and `rows` 1–160 (the stream's snapshot bounds). The cursor position is inside the screen.

Input modes, `modes.flags` bit 0 to 7: `application_cursor`, `application_keypad`, `bracketed_paste`, `mouse_click`, `mouse_drag`, `mouse_motion`, `mouse_sgr`, `focus`. `keyboard_flags` is copied as is.

Styles: each message has its own table of the distinct cell styles it uses, in order of first use, at most `cols × rows` entries. `style.flags` bits: 1 bold, 2 italic, 4 underline, 8 an underline colour follows, 16 a hyperlink follows; other bits must be zero. A hyperlink is at most 2,048 bytes of UTF-8 and must pass `valid_hyperlink`.

Rows: a snapshot encodes every row in order. A patch encodes the rows that changed since its base, in strictly increasing row order, each whole; the server derives them from the cell changes it computed (`row = index / cols`), so a patch carries the same changes as the JSON patch plus the unchanged cells of those rows. A row has at most `cols` runs and decodes to at most `cols` cells; the cells after the last run are default blanks (text `" "`, width 1, default colours, no attributes, no hyperlink), which is how trailing blanks are omitted.

Runs, `run_kind`:

- **0, text:** `length` bytes of UTF-8. Every character whose width is not zero (`unicode-width` 0.2.2 `UnicodeWidthChar::width`, with no width counted as zero) starts a new cell of that width; the zero-width characters after it (combining marks, variation selectors) belong to it. A cell of width 2 is followed by its spacer, a cell of width 0 with empty text and the same style. The text must start with a character of non-zero width.
- **1, repeat:** `length` cells (at least one), then `len:varint` (at most 1,024) and `len` bytes of text: one character of width 1 followed only by zero-width characters. Every cell has that text, width 1 and the run's style.
- **2, raw:** `length` cells, each `width:u8`, `len:varint` (at most 1,024) and `len` bytes of text. Raw cells represent everything a text run cannot: an empty-text cell of width 1, a spacer that does not follow its glyph or has another style, and a two-column cell whose first character is narrow (an emoji with VS16, such as `❤️`).

Every decoded cell must satisfy `kelpclient_protocol::cell_valid`, and no cell of width 2 may be in the last column. Encoders choose runs freely; the server's encoder uses a repeat run for eight or more identical simple narrow cells (for example a coloured bar), a text run for other cells whose text is one character of exactly the cell's width followed by zero-width characters (and, for a wide cell, a following spacer of the same style), and raw runs otherwise. Decoding reproduces the original `Screen` exactly, cell for cell.

## Decoding untrusted records

`screens::decode(bytes, max_cells)` returns the record's messages or an error and never panics. Pass the Welcome `limits.subscribed_cells` (or `screens::DEFAULT_MAX_CELLS`, 100,000) as `max_cells`. It rejects:

- an unknown marker, format version, message kind, run kind, colour tag or style flag;
- geometry outside 1–400 × 1–160, a cursor outside the screen, a patch whose revision is not `base + 1`, a zero pane or revision, a pane named twice;
- more than 256 messages, or snapshots and changed rows totalling more than `max_cells` cells (checked before allocating them);
- a style table larger than `cols × rows` or than the remaining bytes could hold, a style index outside the table, an invalid hyperlink;
- a row with more than `cols` runs or cells, an empty run, a cell text over 1,024 bytes, invalid UTF-8, a cell failing `cell_valid`, a wide cell in the last column, rows out of order;
- more than 16 MiB of decoded cell text and hyperlinks in one record (a hyperlink counts once per cell it covers), so a small record cannot expand beyond what a 16 MiB JSON record could carry;
- truncated input and trailing bytes.

Decoded snapshots are complete `PaneSnapshot` values. A decoded patch is a `RowPatch`: `RowPatch::apply` produces the next `PaneSnapshot` from the exact baseline, and `RowPatch::into_pane_patch` gives the equivalent cell `PanePatch` (every cell of every changed row) for clients that already apply cell patches.

## Clients

- **Rust clients** (the KelpMux TUI) read records with `kelpserver::native::protocol::read_server_record`, which returns either a JSON `ServerMessage` or the raw bytes of a binary record, then decode with `kelpclient_protocol::screens::decode`.
- **Bridges** forward binary records without parsing them (see [Bridges](#bridges)).
- **Browsers:** the `@kelpcore/web` `Session` (`web/src/connection.ts`) sends the opt-in when Welcome lists `binary-screens`, and the WASM core decodes the records in `kelpclient::Client::receive` with the same crate. The core returns each update with its screens as JSON `snapshots` and `patches` (a binary patch as a cell patch of its whole rows), so the shared worker, the windows and the renderer are unchanged. Laying the screens out as typed arrays transferred to windows (ADR 0003 section 2) is not implemented: on 2026-11-22, switching a browser from a grid tile to a 298-column terminal through a real server and bridge took 186 ms instead of 323 ms (Chromium; WebKit 141 ms instead of 254 ms) with 20 KiB instead of 1.5 MiB on the wire, and the page spent about 25 ms receiving it either way. The remaining per-window cost is the JSON and structured-clone hand-off of per-cell objects between the WASM core, the shared worker and each window, which typed arrays would address.
