//! Semantic token and folding range handlers

use tower_lsp::lsp_types::*;

use crate::parser::{
    find_inline_comment_start, parse_codeowners_file_with_positions, CodeownersLine,
};

/// Generate semantic tokens for syntax highlighting
pub fn semantic_tokens(content: &str) -> Vec<SemanticToken> {
    // Token types: 0=comment, 1=string(pattern), 2=variable(@user), 3=class(@org/team), 4=operator(glob)
    let mut data: Vec<SemanticToken> = Vec::new();
    let mut prev_line: u32 = 0;
    let mut prev_char: u32 = 0;

    for (line_num, line) in content.lines().enumerate() {
        let line_num = line_num as u32;
        let trimmed = line.trim_start();

        if trimmed.starts_with('#') {
            // Comment - entire line
            let start_char = (line.len() - trimmed.len()) as u32;
            data.push(SemanticToken {
                delta_line: line_num - prev_line,
                delta_start: if line_num == prev_line {
                    start_char - prev_char
                } else {
                    start_char
                },
                length: trimmed.len() as u32,
                token_type: 0, // comment
                token_modifiers_bitset: 0,
            });
            prev_line = line_num;
            prev_char = start_char;
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }

        // Parse rule: pattern owners... (stop at inline comment)
        let all_parts: Vec<&str> = line.split_whitespace().collect();
        let parts: Vec<&str> = all_parts
            .iter()
            .take_while(|part| !part.starts_with('#'))
            .copied()
            .collect();
        if parts.is_empty() {
            continue;
        }

        // Find pattern position
        let pattern = parts[0];
        if let Some(pattern_start) = line.find(pattern) {
            let pattern_start = pattern_start as u32;

            // Highlight glob characters within the pattern
            let mut char_idx = 0;
            let pattern_chars: Vec<char> = pattern.chars().collect();

            while char_idx < pattern_chars.len() {
                let c = pattern_chars[char_idx];
                if c == '*' || c == '?' {
                    // Operator (glob char)
                    let abs_pos = pattern_start + char_idx as u32;
                    data.push(SemanticToken {
                        delta_line: line_num - prev_line,
                        delta_start: if line_num == prev_line {
                            abs_pos - prev_char
                        } else {
                            abs_pos
                        },
                        length: 1,
                        token_type: 4, // operator
                        token_modifiers_bitset: 0,
                    });
                    prev_line = line_num;
                    prev_char = abs_pos;
                }
                char_idx += 1;
            }

            // Highlight the whole pattern as string if no globs
            if !pattern.contains('*') && !pattern.contains('?') {
                data.push(SemanticToken {
                    delta_line: line_num - prev_line,
                    delta_start: if line_num == prev_line {
                        pattern_start - prev_char
                    } else {
                        pattern_start
                    },
                    length: pattern.len() as u32,
                    token_type: 1, // string
                    token_modifiers_bitset: 0,
                });
                prev_line = line_num;
                prev_char = pattern_start;
            }
        }

        // Highlight owners using forward search to handle duplicates correctly
        let mut owner_search_start = 0;
        for &owner in &parts[1..] {
            // Find the next occurrence of this owner at or after owner_search_start
            if let Some(rel_pos) = line[owner_search_start..].find(owner) {
                let owner_start = (owner_search_start + rel_pos) as u32;
                owner_search_start = owner_search_start + rel_pos + owner.len();
                let token_type = if owner.starts_with('@') {
                    if owner.contains('/') {
                        3 // class (team)
                    } else {
                        2 // variable (user)
                    }
                } else {
                    2 // variable (email or other)
                };

                data.push(SemanticToken {
                    delta_line: line_num - prev_line,
                    delta_start: if line_num == prev_line {
                        owner_start - prev_char
                    } else {
                        owner_start
                    },
                    length: owner.len() as u32,
                    token_type,
                    token_modifiers_bitset: 0,
                });
                prev_line = line_num;
                prev_char = owner_start;
            }
        }

        // Highlight inline comment if present
        if let Some(comment_char_off) = find_inline_comment_start(line) {
            let comment_len = line.chars().count() - comment_char_off;
            let comment_start = comment_char_off as u32;
            data.push(SemanticToken {
                delta_line: line_num - prev_line,
                delta_start: if line_num == prev_line {
                    comment_start - prev_char
                } else {
                    comment_start
                },
                length: comment_len as u32,
                token_type: 0, // comment
                token_modifiers_bitset: 0,
            });
            prev_line = line_num;
            prev_char = comment_start;
        }
    }

    data
}

/// Generate folding ranges for CODEOWNERS file
pub fn folding_ranges(content: &str) -> Vec<FoldingRange> {
    let lines = parse_codeowners_file_with_positions(content);
    let mut ranges = Vec::new();
    let mut comment_block_start: Option<u32> = None;

    for (idx, line) in lines.iter().enumerate() {
        let is_comment = matches!(&line.content, CodeownersLine::Comment(_));
        let next_is_comment = lines
            .get(idx + 1)
            .map(|l| matches!(&l.content, CodeownersLine::Comment(_)))
            .unwrap_or(false);

        if is_comment {
            if comment_block_start.is_none() {
                comment_block_start = Some(line.line_number);
            }
            // End of comment block
            if !next_is_comment {
                if let Some(start) = comment_block_start.take() {
                    // Only create fold if block is > 1 line
                    if line.line_number > start {
                        ranges.push(FoldingRange {
                            start_line: start,
                            start_character: None,
                            end_line: line.line_number,
                            end_character: None,
                            kind: Some(FoldingRangeKind::Comment),
                            collapsed_text: Some("# ...".to_string()),
                        });
                    }
                }
            }
        }
    }

    // Also fold sections (comment followed by rules until next section)
    let mut section_start: Option<u32> = None;
    for line in &lines {
        if let CodeownersLine::Comment(text) = &line.content {
            let section_text = text.trim().trim_start_matches('#').trim();
            if !section_text.is_empty()
                && section_text
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
            {
                // New section header - close previous
                if let Some(start) = section_start.take() {
                    let end = if line.line_number > 0 {
                        line.line_number - 1
                    } else {
                        0
                    };
                    if end > start {
                        ranges.push(FoldingRange {
                            start_line: start,
                            start_character: None,
                            end_line: end,
                            end_character: None,
                            kind: Some(FoldingRangeKind::Region),
                            collapsed_text: None,
                        });
                    }
                }
                section_start = Some(line.line_number);
            }
        }
    }

    // Close final section
    if let Some(start) = section_start {
        if let Some(last) = lines.last() {
            if last.line_number > start {
                ranges.push(FoldingRange {
                    start_line: start,
                    start_character: None,
                    end_line: last.line_number,
                    end_character: None,
                    kind: Some(FoldingRangeKind::Region),
                    collapsed_text: None,
                });
            }
        }
    }

    ranges
}
