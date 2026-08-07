//! Shared utilities for LSP handlers

/// Convert an LSP `Position.character` to a byte index into `line`.
///
/// LSP counts characters in UTF-16 code units, not bytes. Slicing a line with
/// the raw value panics the moment anything non-ASCII appears earlier on it,
/// so every cursor-relative slice has to go through here. Out-of-range values
/// clamp to the end of the line.
pub fn utf16_offset_to_byte_index(line: &str, character: usize) -> usize {
    let mut utf16_units = 0;
    for (byte_index, ch) in line.char_indices() {
        if utf16_units >= character {
            return byte_index;
        }
        utf16_units += ch.len_utf16();
    }
    line.len()
}

/// Find the byte position of the nth occurrence of an owner string in a line.
///
/// `n` is the occurrence count (0-indexed) of this specific owner as a
/// whitespace-delimited word. This is NOT the index in the owners vec --
/// callers must track per-owner occurrence counts separately.
pub fn find_nth_owner_position(line: &str, owner: &str, n: usize) -> Option<usize> {
    let mut count = 0;
    let mut start = 0;
    while let Some(pos) = line[start..].find(owner) {
        let abs_pos = start + pos;
        // Verify it's a whole word (not part of pattern)
        let before_ok = abs_pos == 0
            || line
                .as_bytes()
                .get(abs_pos - 1)
                .map(|&b| b == b' ' || b == b'\t')
                .unwrap_or(true);
        let after_ok = abs_pos + owner.len() >= line.len()
            || line
                .as_bytes()
                .get(abs_pos + owner.len())
                .map(|&b| b == b' ' || b == b'\t')
                .unwrap_or(true);

        if before_ok && after_ok {
            if count == n {
                return Some(abs_pos);
            }
            count += 1;
        }
        start = abs_pos + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_nth_owner_basic() {
        let line = "*.rs @alice @bob @charlie";
        assert_eq!(find_nth_owner_position(line, "@alice", 0), Some(5));
        assert_eq!(find_nth_owner_position(line, "@bob", 0), Some(12));
        assert_eq!(find_nth_owner_position(line, "@charlie", 0), Some(17));
    }

    #[test]
    fn test_find_nth_owner_duplicate() {
        let line = "*.rs @alice @bob @alice";
        assert_eq!(find_nth_owner_position(line, "@alice", 0), Some(5));
        assert_eq!(find_nth_owner_position(line, "@alice", 1), Some(17));
        assert_eq!(find_nth_owner_position(line, "@alice", 2), None);
    }

    #[test]
    fn test_find_nth_owner_not_found() {
        let line = "*.rs @alice";
        assert_eq!(find_nth_owner_position(line, "@bob", 0), None);
    }

    #[test]
    fn test_find_nth_owner_word_boundary() {
        // @alice should not match inside @alice-admin
        let line = "*.rs @alice-admin @alice";
        assert_eq!(find_nth_owner_position(line, "@alice", 0), Some(18));
    }

    #[test]
    fn test_find_nth_owner_at_start() {
        let line = "@owner pattern";
        assert_eq!(find_nth_owner_position(line, "@owner", 0), Some(0));
    }

    #[test]
    fn test_find_nth_owner_at_end() {
        let line = "*.rs @owner";
        assert_eq!(find_nth_owner_position(line, "@owner", 0), Some(5));
    }

    #[test]
    fn test_find_nth_owner_triple_duplicate() {
        let line = "*.rs @a @b @a @a";
        assert_eq!(find_nth_owner_position(line, "@a", 0), Some(5));
        assert_eq!(find_nth_owner_position(line, "@a", 1), Some(11));
        assert_eq!(find_nth_owner_position(line, "@a", 2), Some(14));
        assert_eq!(find_nth_owner_position(line, "@a", 3), None);
    }
}

#[cfg(test)]
mod utf16_tests {
    use super::utf16_offset_to_byte_index;

    #[test]
    fn ascii_offsets_are_byte_offsets() {
        let line = "src/*.rs @owner";
        assert_eq!(utf16_offset_to_byte_index(line, 0), 0);
        assert_eq!(utf16_offset_to_byte_index(line, 8), 8);
        assert_eq!(utf16_offset_to_byte_index(line, 999), line.len());
    }

    #[test]
    fn multibyte_offsets_land_on_char_boundaries() {
        // Slicing this line at the raw LSP offset used to panic.
        let line = "日本語.rs @owner";
        for character in 0..=20 {
            let byte = utf16_offset_to_byte_index(line, character);
            assert!(line.is_char_boundary(byte), "offset {character} -> {byte}");
            let _ = &line[..byte];
        }
        assert_eq!(utf16_offset_to_byte_index(line, 1), 3);
        assert_eq!(utf16_offset_to_byte_index(line, 3), 9);
    }

    #[test]
    fn astral_chars_count_as_two_utf16_units() {
        let line = "🦀/main.rs @owner";
        assert_eq!(utf16_offset_to_byte_index(line, 2), 4);
        assert!(line.is_char_boundary(utf16_offset_to_byte_index(line, 1)));
    }
}
