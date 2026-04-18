//! Shared utilities for LSP handlers

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
