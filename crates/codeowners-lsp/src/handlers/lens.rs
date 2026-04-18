//! Code lens handler

use tower_lsp::lsp_types::*;

use crate::file_cache::FileCache;
use crate::parser::{parse_codeowners_file_with_positions, CodeownersLine};

/// Generate code lenses showing file counts for each rule
pub fn code_lenses(content: &str, file_cache: &FileCache) -> Vec<CodeLens> {
    let lines = parse_codeowners_file_with_positions(content);
    let mut lenses = Vec::new();

    for line in &lines {
        if let CodeownersLine::Rule { pattern, owners } = &line.content {
            let count = file_cache.count_matches(pattern);
            let owners_str = owners.join(" ");
            let title = if owners_str.is_empty() {
                format!("{} {}", count, if count == 1 { "file" } else { "files" })
            } else {
                format!(
                    "{} {} · {}",
                    count,
                    if count == 1 { "file" } else { "files" },
                    owners_str
                )
            };

            lenses.push(CodeLens {
                range: Range {
                    start: Position {
                        line: line.line_number,
                        character: 0,
                    },
                    end: Position {
                        line: line.line_number,
                        character: 0,
                    },
                },
                command: Some(Command {
                    title,
                    command: String::new(),
                    arguments: None,
                }),
                data: None,
            });
        }
    }

    lenses
}
