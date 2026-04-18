//! Linked editing ranges - edit owner in multiple places at once

use tower_lsp::lsp_types::*;

use crate::parser::{find_owner_at_position, parse_codeowners_file_with_positions, CodeownersLine};

use super::util::find_nth_owner_position;

/// Find all ranges where the same owner appears for linked editing
pub fn linked_editing_ranges(content: &str, position: Position) -> Option<LinkedEditingRanges> {
    let lines: Vec<&str> = content.lines().collect();
    let line_idx = position.line as usize;

    if line_idx >= lines.len() {
        return None;
    }

    let line = lines[line_idx];
    let char_idx = position.character as usize;

    // Find owner at cursor position
    let owner = find_owner_at_position(line, char_idx)?;

    // Find all occurrences of this owner
    let parsed = parse_codeowners_file_with_positions(content);
    let mut ranges = Vec::new();

    for parsed_line in &parsed {
        if let CodeownersLine::Rule {
            owners: line_owners,
            ..
        } = &parsed_line.content
        {
            let line_text = lines.get(parsed_line.line_number as usize).unwrap_or(&"");

            let mut occurrence = 0;
            for o in line_owners.iter() {
                if o == &owner {
                    if let Some(pos) = find_nth_owner_position(line_text, &owner, occurrence) {
                        ranges.push(Range {
                            start: Position {
                                line: parsed_line.line_number,
                                character: pos as u32,
                            },
                            end: Position {
                                line: parsed_line.line_number,
                                character: (pos + owner.len()) as u32,
                            },
                        });
                    }
                    occurrence += 1;
                }
            }
        }
    }

    if ranges.len() <= 1 {
        // No point in linked editing if only one occurrence
        return None;
    }

    Some(LinkedEditingRanges {
        ranges,
        word_pattern: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linked_editing_single_occurrence() {
        let content = "*.rs @owner";
        let result = linked_editing_ranges(
            content,
            Position {
                line: 0,
                character: 6,
            },
        );
        // Single occurrence, no linked editing
        assert!(result.is_none());
    }

    #[test]
    fn test_linked_editing_multiple_occurrences() {
        let content = "*.rs @owner\n*.js @owner";
        let result = linked_editing_ranges(
            content,
            Position {
                line: 0,
                character: 6,
            },
        );
        assert!(result.is_some());
        let ranges = result.unwrap().ranges;
        assert_eq!(ranges.len(), 2);
    }

    #[test]
    fn test_linked_editing_not_on_owner() {
        let content = "*.rs @owner";
        let result = linked_editing_ranges(
            content,
            Position {
                line: 0,
                character: 1,
            },
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_linked_editing_duplicate_owners_on_same_line() {
        // Regression: duplicate owners on the same line must all be found
        let content = "*.rs @alice @bob @alice\n*.js @alice";
        let result = linked_editing_ranges(
            content,
            Position {
                line: 0,
                character: 6, // cursor on first @alice
            },
        );
        assert!(result.is_some());
        let ranges = result.unwrap().ranges;
        // Should find: line 0 pos 5, line 0 pos 17, line 1 pos 5
        assert_eq!(ranges.len(), 3, "Expected 3 ranges, got {:?}", ranges);
        assert_eq!(ranges[0].start.character, 5); // first @alice on line 0
        assert_eq!(ranges[1].start.character, 17); // second @alice on line 0
        assert_eq!(ranges[2].start.line, 1); // @alice on line 1
    }
}
