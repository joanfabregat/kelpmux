use kelpmux::native::protocol::*;
use std::io::{self, Cursor, Read};

struct Fragmented(Cursor<Vec<u8>>);
impl Read for Fragmented {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let limit = bytes.len().min(1);
        self.0.read(&mut bytes[..limit])
    }
}

#[test]
fn framed_messages_survive_byte_fragmentation_without_consuming_next_packet() {
    let mut bytes = Vec::new();
    write_message(
        &mut bytes,
        &ClientMessage::Paste(b"a\x02z\x1b[31m".to_vec()),
    )
    .unwrap();
    write_message(&mut bytes, &ClientMessage::Ping).unwrap();
    let mut reader = Fragmented(Cursor::new(bytes));
    assert!(
        matches!(read_message::<ClientMessage>(&mut reader).unwrap(), Some(ClientMessage::Paste(bytes)) if bytes == b"a\x02z\x1b[31m")
    );
    assert!(matches!(
        read_message::<ClientMessage>(&mut reader).unwrap(),
        Some(ClientMessage::Ping)
    ));
    assert!(
        read_message::<ClientMessage>(&mut reader)
            .unwrap()
            .is_none()
    );
}

#[test]
fn malformed_partial_and_oversized_messages_fail_before_allocation_or_dispatch() {
    for bytes in [
        vec![0],
        vec![0, 0, 0, 0],
        u32::MAX.to_be_bytes().to_vec(),
        vec![0, 0, 0, 5, b'{'],
        vec![0, 0, 0, 1, b'!'],
    ] {
        assert!(read_message::<ClientMessage>(&mut Cursor::new(bytes)).is_err());
    }
    for (cols, rows) in [(0, 0), (39, 10), (40, 9), (401, 160), (400, 161)] {
        assert!(validate_size(cols, rows).is_err());
    }
    for (cols, rows) in [(40, 10), (120, 40), (400, 160)] {
        assert!(validate_size(cols, rows).is_ok());
    }
}
