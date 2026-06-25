//! Line/column <-> UTF-8 byte offset conversion.
//!
//! We advertise `PositionEncodingKind::UTF8`, so an LSP `character` is a UTF-8
//! byte offset from the start of the line (not a UTF-16 code unit). This makes
//! positions correct for Guile source that contains Chinese comments/strings.

use tower_lsp::lsp_types::Position;

/// Convert an LSP position to a byte offset within `text`.
pub fn position_to_offset(text: &str, pos: &Position) -> usize {
    let target_line = pos.line as usize;

    // Locate the byte offset where the target line starts.
    let mut line_idx = 0usize;
    let mut line_start = 0usize;
    for (i, b) in text.bytes().enumerate() {
        if line_idx == target_line {
            break;
        }
        if b == b'\n' {
            line_idx += 1;
            line_start = i + 1;
        }
    }
    if line_idx < target_line {
        return text.len();
    }

    // Clamp the in-line byte offset to the line's bounds.
    let rest = &text[line_start..];
    let line_end = rest
        .find('\n')
        .map(|n| line_start + n)
        .unwrap_or(text.len());
    let line_len = line_end - line_start;
    let char_off = (pos.character as usize).min(line_len);
    line_start + char_off
}

/// Convert a byte offset within `text` back to an LSP position.
pub fn offset_to_position(text: &str, offset: usize) -> Position {
    let offset = offset.min(text.len());
    let mut line = 0u32;
    let mut last_line_start = 0usize;
    for (i, b) in text.bytes().enumerate() {
        if i >= offset {
            break;
        }
        if b == b'\n' {
            line += 1;
            last_line_start = i + 1;
        }
    }
    let character = (offset - last_line_start) as u32;
    Position { line, character }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_roundtrip() {
        let text = "abc\ndefgh\nij";
        // line 1 "defgh" starts at offset 4; char 3 -> offset 7 ('g').
        assert_eq!(position_to_offset(text, &Position::new(1, 3)), 7);
        assert_eq!(offset_to_position(text, 7), Position::new(1, 3));
    }

    #[test]
    fn utf8_byte_offsets() {
        // "中" is 3 UTF-8 bytes; positions count bytes, not characters.
        let text = "中a\nbc";
        // line 0, char 3 lands just after "中" (3 bytes), at 'a'.
        assert_eq!(position_to_offset(text, &Position::new(0, 3)), 3);
        assert_eq!(offset_to_position(text, 3), Position::new(0, 3));
    }

    #[test]
    fn clamps_past_end() {
        let text = "ab\ncd";
        assert_eq!(position_to_offset(text, &Position::new(5, 0)), 5);
    }
}
