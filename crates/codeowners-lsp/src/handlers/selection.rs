//! Selection range handler - smart expand selection

use tower_lsp::lsp_types::*;

use crate::parser::{parse_codeowners_file_with_positions, CodeownersLine};

/// Generate selection ranges for smart expand
/// Hierarchy: word -> owner/pattern -> all owners -> whole rule -> section -> file
pub fn selection_ranges(content: &str, positions: &[Position]) -> Vec<SelectionRange> {
    let lines: Vec<&str> = content.lines().collect();
    let parsed = parse_codeowners_file_with_positions(content);

    positions
        .iter()
        .map(|pos| build_selection_range(&lines, &parsed, *pos))
        .collect()
}

fn build_selection_range(
    lines: &[&str],
    parsed: &[crate::parser::ParsedLine],
    position: Position,
) -> SelectionRange {
    let line_idx = position.line as usize;
    let char_idx = position.character as usize;

    let line = lines.get(line_idx).copied().unwrap_or("");
    let line_len = line.len() as u32;

    // Find the parsed line info
    let parsed_line = parsed.iter().find(|p| p.line_number == position.line);

    // Innermost: current word
    let (word_start, word_end) = find_word_bounds(line, char_idx);
    let word_range = Range {
        start: Position {
            line: position.line,
            character: word_start as u32,
        },
        end: Position {
            line: position.line,
            character: word_end as u32,
        },
    };

    // Build the hierarchy from innermost to outermost
    let mut ranges: Vec<Range> = vec![word_range];

    if let Some(pl) = parsed_line {
        match &pl.content {
            CodeownersLine::Rule { owners, .. } => {
                // If in owner section, add "all owners" range
                if char_idx as u32 >= pl.owners_start && !owners.is_empty() {
                    let owners_end = line_len;
                    ranges.push(Range {
                        start: Position {
                            line: position.line,
                            character: pl.owners_start,
                        },
                        end: Position {
                            line: position.line,
                            character: owners_end,
                        },
                    });
                }

                // Whole rule line
                ranges.push(Range {
                    start: Position {
                        line: position.line,
                        character: 0,
                    },
                    end: Position {
                        line: position.line,
                        character: line_len,
                    },
                });
            }
            CodeownersLine::Comment(_) => {
                // Find comment block bounds
                let (block_start, block_end) = find_comment_block(parsed, position.line);
                if block_end > block_start {
                    ranges.push(Range {
                        start: Position {
                            line: block_start,
                            character: 0,
                        },
                        end: Position {
                            line: block_end,
                            character: lines
                                .get(block_end as usize)
                                .map(|l| l.len() as u32)
                                .unwrap_or(0),
                        },
                    });
                }
            }
            CodeownersLine::Empty => {}
        }
    }

    // Find section bounds (from section header to next section or EOF)
    if let Some((section_start, section_end)) = find_section_bounds(parsed, position.line) {
        ranges.push(Range {
            start: Position {
                line: section_start,
                character: 0,
            },
            end: Position {
                line: section_end,
                character: lines
                    .get(section_end as usize)
                    .map(|l| l.len() as u32)
                    .unwrap_or(0),
            },
        });
    }

    // Whole file
    let last_line = lines.len().saturating_sub(1) as u32;
    ranges.push(Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: last_line,
            character: lines.last().map(|l| l.len() as u32).unwrap_or(0),
        },
    });

    // Deduplicate ranges (sort first so dedup catches non-consecutive duplicates)
    ranges.sort_by(|a, b| {
        a.start
            .line
            .cmp(&b.start.line)
            .then(a.start.character.cmp(&b.start.character))
            .then(a.end.line.cmp(&b.end.line))
            .then(a.end.character.cmp(&b.end.character))
    });
    ranges.dedup();

    // Build nested SelectionRange structure (innermost first)
    let mut result: Option<SelectionRange> = None;
    for range in ranges.into_iter().rev() {
        result = Some(SelectionRange {
            range,
            parent: result.map(Box::new),
        });
    }

    result.unwrap_or(SelectionRange {
        range: word_range,
        parent: None,
    })
}

fn find_word_bounds(line: &str, char_idx: usize) -> (usize, usize) {
    let chars: Vec<char> = line.chars().collect();
    let char_idx = char_idx.min(chars.len());

    // Find word start
    let mut start = char_idx;
    while start > 0 {
        let c = chars[start - 1];
        if c.is_whitespace() {
            break;
        }
        start -= 1;
    }

    // Find word end
    let mut end = char_idx;
    while end < chars.len() {
        let c = chars[end];
        if c.is_whitespace() {
            break;
        }
        end += 1;
    }

    (start, end)
}

fn find_comment_block(parsed: &[crate::parser::ParsedLine], line: u32) -> (u32, u32) {
    let mut start = line;
    let mut end = line;

    // Find start of comment block
    for pl in parsed.iter().rev() {
        if pl.line_number >= line {
            continue;
        }
        if matches!(pl.content, CodeownersLine::Comment(_)) {
            start = pl.line_number;
        } else {
            break;
        }
    }

    // Find end of comment block
    for pl in parsed.iter() {
        if pl.line_number <= line {
            continue;
        }
        if matches!(pl.content, CodeownersLine::Comment(_)) {
            end = pl.line_number;
        } else {
            break;
        }
    }

    (start, end)
}

fn find_section_bounds(parsed: &[crate::parser::ParsedLine], line: u32) -> Option<(u32, u32)> {
    // Find section header (uppercase comment) before this line
    let mut section_start: Option<u32> = None;

    for pl in parsed.iter() {
        if pl.line_number > line {
            break;
        }
        if let CodeownersLine::Comment(text) = &pl.content {
            let section_text = text.trim().trim_start_matches('#').trim();
            if !section_text.is_empty()
                && section_text
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
            {
                section_start = Some(pl.line_number);
            }
        }
    }

    let start = section_start?;

    // Find section end (next section header or EOF)
    let mut end = parsed.last().map(|p| p.line_number).unwrap_or(line);

    for pl in parsed.iter() {
        if pl.line_number <= start {
            continue;
        }
        if let CodeownersLine::Comment(text) = &pl.content {
            let section_text = text.trim().trim_start_matches('#').trim();
            if !section_text.is_empty()
                && section_text
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
            {
                end = pl.line_number.saturating_sub(1);
                break;
            }
        }
    }

    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_word_bounds() {
        assert_eq!(find_word_bounds("hello world", 2), (0, 5));
        assert_eq!(find_word_bounds("hello world", 7), (6, 11));
        assert_eq!(find_word_bounds("@owner", 3), (0, 6));
    }

    #[test]
    fn test_selection_range_basic() {
        let content = "*.rs @owner";
        let ranges = selection_ranges(
            content,
            &[Position {
                line: 0,
                character: 2,
            }],
        );
        assert_eq!(ranges.len(), 1);
        assert!(ranges[0].parent.is_some()); // Should have parent ranges
    }
}
