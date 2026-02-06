//! Navigation handlers: references, rename

use std::collections::HashMap;

use tower_lsp::lsp_types::*;

use crate::parser::{find_owner_at_position, parse_codeowners_file_with_positions, CodeownersLine};

use super::util::find_nth_owner_position;

/// Find all references to an owner in CODEOWNERS file
pub fn find_references(content: &str, position: Position, uri: &Url) -> Option<Vec<Location>> {
    let lines: Vec<&str> = content.lines().collect();
    let line_idx = position.line as usize;
    if line_idx >= lines.len() {
        return None;
    }

    let line = lines[line_idx];
    let char_idx = position.character as usize;

    // Find owner at cursor position
    let owner = find_owner_at_position(line, char_idx)?;

    // Find all lines containing this owner
    let parsed = parse_codeowners_file_with_positions(content);
    let mut locations = Vec::new();

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
                        locations.push(Location {
                            uri: uri.clone(),
                            range: Range {
                                start: Position {
                                    line: parsed_line.line_number,
                                    character: pos as u32,
                                },
                                end: Position {
                                    line: parsed_line.line_number,
                                    character: (pos + owner.len()) as u32,
                                },
                            },
                        });
                    }
                    occurrence += 1;
                }
            }
        }
    }

    if locations.is_empty() {
        None
    } else {
        Some(locations)
    }
}

/// Prepare rename: validate and return the range of the owner to rename
pub fn prepare_rename(content: &str, position: Position) -> Option<Range> {
    let lines: Vec<&str> = content.lines().collect();
    let line_idx = position.line as usize;
    if line_idx >= lines.len() {
        return None;
    }

    let line = lines[line_idx];
    let char_idx = position.character as usize;

    // Only allow renaming @owners
    let owner = find_owner_at_position(line, char_idx)?;

    // Find the occurrence of this owner that contains the cursor.
    // We can't just use line.find() because that always returns the first occurrence.
    let mut start = 0;
    while let Some(pos) = line[start..].find(&owner) {
        let abs_pos = start + pos;
        let end_pos = abs_pos + owner.len();
        // Check word boundary
        let before_ok = abs_pos == 0
            || line
                .as_bytes()
                .get(abs_pos - 1)
                .map(|&b| b == b' ' || b == b'\t')
                .unwrap_or(true);
        let after_ok = end_pos >= line.len()
            || line
                .as_bytes()
                .get(end_pos)
                .map(|&b| b == b' ' || b == b'\t')
                .unwrap_or(true);

        if before_ok && after_ok && char_idx >= abs_pos && char_idx < end_pos {
            return Some(Range {
                start: Position {
                    line: position.line,
                    character: abs_pos as u32,
                },
                end: Position {
                    line: position.line,
                    character: end_pos as u32,
                },
            });
        }
        start = abs_pos + 1;
    }
    None
}

/// Rename an owner across all rules
pub fn rename_owner(
    content: &str,
    position: Position,
    new_name: &str,
    uri: &Url,
) -> Option<WorkspaceEdit> {
    let lines: Vec<&str> = content.lines().collect();
    let line_idx = position.line as usize;
    if line_idx >= lines.len() {
        return None;
    }

    let line = lines[line_idx];
    let char_idx = position.character as usize;

    let old_owner = find_owner_at_position(line, char_idx)?;

    let parsed = parse_codeowners_file_with_positions(content);
    let mut edits = Vec::new();

    for parsed_line in &parsed {
        if let CodeownersLine::Rule {
            owners: line_owners,
            ..
        } = &parsed_line.content
        {
            let line_text = lines.get(parsed_line.line_number as usize).unwrap_or(&"");
            let mut occurrence = 0;
            for o in line_owners.iter() {
                if o == &old_owner {
                    if let Some(pos) = find_nth_owner_position(line_text, &old_owner, occurrence) {
                        edits.push(TextEdit {
                            range: Range {
                                start: Position {
                                    line: parsed_line.line_number,
                                    character: pos as u32,
                                },
                                end: Position {
                                    line: parsed_line.line_number,
                                    character: (pos + old_owner.len()) as u32,
                                },
                            },
                            new_text: new_name.to_string(),
                        });
                    }
                    occurrence += 1;
                }
            }
        }
    }

    if edits.is_empty() {
        None
    } else {
        let mut changes = HashMap::new();
        changes.insert(uri.clone(), edits);
        Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
    }
}
