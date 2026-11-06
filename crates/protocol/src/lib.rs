//! Shared wire types; no server, client state, transport, DOM or Unix dependencies.
pub mod model;
pub mod types;
pub use types::*;
pub const LOCAL_PROTOCOL_VERSION: u16 = 58;
pub const STREAM_PROTOCOL_VERSION: u16 = 1;
pub fn cell_valid(cell: &Cell) -> bool {
    if cell.width > 2
        || cell.text.len() > 1024
        || cell.text.chars().any(char::is_control)
        || cell
            .hyperlink
            .as_ref()
            .is_some_and(|uri| !valid_hyperlink(uri))
    {
        return false;
    }
    let mut chars = cell.text.chars();
    if cell.width == 0 {
        return cell.text.is_empty();
    }
    if let Some(first) = chars.next() {
        (unicode_width::UnicodeWidthChar::width(first).unwrap_or(0) == usize::from(cell.width)
            || cell.text.contains('\u{fe0f}')
                && unicode_width::UnicodeWidthStr::width(cell.text.as_str())
                    == usize::from(cell.width))
            && chars.all(|ch| unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) == 0)
    } else {
        true
    }
}
