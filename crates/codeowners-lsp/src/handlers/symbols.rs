//! Document and workspace symbol handlers

use std::collections::HashSet;

use tower_lsp::lsp_types::*;

use crate::parser::{parse_codeowners_file_with_positions, CodeownersLine};

/// Generate document symbols (outline) for CODEOWNERS file
pub fn document_symbols(content: &str) -> Vec<DocumentSymbol> {
    let lines = parse_codeowners_file_with_positions(content);
    let mut symbols = Vec::new();
    let mut current_section: Option<(String, u32, Vec<DocumentSymbol>)> = None;

    for line in &lines {
        match &line.content {
            CodeownersLine::Comment(text) => {
                // Check if this is a section header (e.g., "# Section Name")
                let section_text = text.trim().trim_start_matches('#').trim();
                if !section_text.is_empty()
                    && section_text
                        .chars()
                        .next()
                        .map(|c| c.is_uppercase())
                        .unwrap_or(false)
                {
                    // Finish previous section
                    if let Some((name, start_line, children)) = current_section.take() {
                        let end_line = if line.line_number > 0 {
                            line.line_number - 1
                        } else {
                            0
                        };
                        #[allow(deprecated)]
                        symbols.push(DocumentSymbol {
                            name,
                            detail: None,
                            kind: SymbolKind::NAMESPACE,
                            tags: None,
                            deprecated: None,
                            range: Range {
                                start: Position {
                                    line: start_line,
                                    character: 0,
                                },
                                end: Position {
                                    line: end_line,
                                    character: u32::MAX,
                                },
                            },
                            selection_range: Range {
                                start: Position {
                                    line: start_line,
                                    character: 0,
                                },
                                end: Position {
                                    line: start_line,
                                    character: u32::MAX,
                                },
                            },
                            children: if children.is_empty() {
                                None
                            } else {
                                Some(children)
                            },
                        });
                    }
                    // Start new section
                    current_section =
                        Some((section_text.to_string(), line.line_number, Vec::new()));
                }
            }
            CodeownersLine::Rule { pattern, owners } => {
                let owners_str = owners.join(" ");
                #[allow(deprecated)]
                let symbol = DocumentSymbol {
                    name: pattern.clone(),
                    detail: Some(owners_str),
                    kind: SymbolKind::FILE,
                    tags: None,
                    deprecated: None,
                    range: Range {
                        start: Position {
                            line: line.line_number,
                            character: 0,
                        },
                        end: Position {
                            line: line.line_number,
                            character: u32::MAX,
                        },
                    },
                    selection_range: Range {
                        start: Position {
                            line: line.line_number,
                            character: line.pattern_start,
                        },
                        end: Position {
                            line: line.line_number,
                            character: line.pattern_end,
                        },
                    },
                    children: None,
                };

                if let Some((_, _, ref mut children)) = current_section {
                    children.push(symbol);
                } else {
                    symbols.push(symbol);
                }
            }
            CodeownersLine::Empty => {}
        }
    }

    // Finish last section
    if let Some((name, start_line, children)) = current_section {
        let end_line = lines.last().map(|l| l.line_number).unwrap_or(start_line);
        #[allow(deprecated)]
        symbols.push(DocumentSymbol {
            name,
            detail: None,
            kind: SymbolKind::NAMESPACE,
            tags: None,
            deprecated: None,
            range: Range {
                start: Position {
                    line: start_line,
                    character: 0,
                },
                end: Position {
                    line: end_line,
                    character: u32::MAX,
                },
            },
            selection_range: Range {
                start: Position {
                    line: start_line,
                    character: 0,
                },
                end: Position {
                    line: start_line,
                    character: u32::MAX,
                },
            },
            children: if children.is_empty() {
                None
            } else {
                Some(children)
            },
        });
    }

    symbols
}

/// Generate workspace symbols for CODEOWNERS file
pub fn workspace_symbols(content: &str, query: &str, uri: &Url) -> Vec<SymbolInformation> {
    let query = query.to_lowercase();
    let parsed = parse_codeowners_file_with_positions(content);
    let lines: Vec<&str> = content.lines().collect();
    let mut symbols = Vec::new();

    for line in &parsed {
        if let CodeownersLine::Rule { pattern, owners } = &line.content {
            // Match against pattern
            if query.is_empty() || pattern.to_lowercase().contains(&query) {
                #[allow(deprecated)]
                symbols.push(SymbolInformation {
                    name: pattern.clone(),
                    kind: SymbolKind::FILE,
                    tags: None,
                    deprecated: None,
                    location: Location {
                        uri: uri.clone(),
                        range: Range {
                            start: Position {
                                line: line.line_number,
                                character: line.pattern_start,
                            },
                            end: Position {
                                line: line.line_number,
                                character: line.pattern_end,
                            },
                        },
                    },
                    container_name: Some(owners.join(" ")),
                });
            }

            // Match against owners
            for owner in owners {
                if query.is_empty() || owner.to_lowercase().contains(&query) {
                    let line_text = lines.get(line.line_number as usize).unwrap_or(&"");
                    if let Some(pos) = line_text.find(owner.as_str()) {
                        #[allow(deprecated)]
                        symbols.push(SymbolInformation {
                            name: owner.clone(),
                            kind: if owner.contains('/') {
                                SymbolKind::CLASS
                            } else {
                                SymbolKind::CONSTANT
                            },
                            tags: None,
                            deprecated: None,
                            location: Location {
                                uri: uri.clone(),
                                range: Range {
                                    start: Position {
                                        line: line.line_number,
                                        character: pos as u32,
                                    },
                                    end: Position {
                                        line: line.line_number,
                                        character: (pos + owner.len()) as u32,
                                    },
                                },
                            },
                            container_name: Some(pattern.clone()),
                        });
                    }
                }
            }
        }
    }

    // Deduplicate owners (only show unique)
    let mut seen_owners: HashSet<String> = HashSet::new();
    symbols.retain(|s| {
        if s.kind == SymbolKind::FILE {
            true // Keep all patterns
        } else {
            seen_owners.insert(s.name.clone())
        }
    });

    symbols
}
