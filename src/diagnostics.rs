use std::collections::HashSet;

use tower_lsp::lsp_types::*;

use crate::file_cache::FileCache;
use crate::github::GitHubClient;
use crate::parser::{parse_codeowners_file_with_positions, CodeownersLine, ParsedLine};
use crate::validation::{validate_owner, validate_pattern};

/// Diagnostic codes for CODEOWNERS issues
pub mod codes {
    pub const INVALID_PATTERN: &str = "invalid-pattern";
    pub const INVALID_OWNER: &str = "invalid-owner";
    pub const PATTERN_NO_MATCH: &str = "pattern-no-match";
    pub const DUPLICATE_OWNER: &str = "duplicate-owner";
    pub const SHADOWED_RULE: &str = "shadowed-rule";
    pub const NO_OWNERS: &str = "no-owners";
    pub const UNOWNED_FILES: &str = "unowned-files";
    pub const GITHUB_OWNER_NOT_FOUND: &str = "github-owner-not-found";
}

/// Owner validation info: (line_number, char_offset, owner_string, owner_len)
pub type OwnerValidationInfo = (u32, u32, String, u32);

/// Compute diagnostics for CODEOWNERS content (sync portion only)
pub fn compute_diagnostics_sync(
    content: &str,
    file_cache: Option<&FileCache>,
) -> (Vec<Diagnostic>, Vec<OwnerValidationInfo>) {
    let mut diagnostics = Vec::new();
    let lines = parse_codeowners_file_with_positions(content);

    // Track patterns for dead rule detection
    let mut seen_patterns: Vec<(String, u32)> = Vec::new();

    // Collect owners to validate via GitHub (line, offset, owner, len)
    let mut owners_to_validate: Vec<(u32, u32, String, u32)> = Vec::new();

    for parsed_line in &lines {
        if let CodeownersLine::Rule { pattern, owners } = &parsed_line.content {
            // Check pattern validity
            if let Some(error) = validate_pattern(pattern) {
                diagnostics.push(Diagnostic {
                    range: Range {
                        start: Position {
                            line: parsed_line.line_number,
                            character: parsed_line.pattern_start,
                        },
                        end: Position {
                            line: parsed_line.line_number,
                            character: parsed_line.pattern_end,
                        },
                    },
                    severity: Some(DiagnosticSeverity::ERROR),
                    code: Some(NumberOrString::String(codes::INVALID_PATTERN.to_string())),
                    source: Some("codeowners".to_string()),
                    message: error,
                    ..Default::default()
                });
            }

            // Check owner validity (format only)
            for (i, owner) in owners.iter().enumerate() {
                let owner_offset = calculate_owner_offset(content, parsed_line, i, owner);

                if let Some(error) = validate_owner(owner) {
                    diagnostics.push(Diagnostic {
                        range: Range {
                            start: Position {
                                line: parsed_line.line_number,
                                character: owner_offset,
                            },
                            end: Position {
                                line: parsed_line.line_number,
                                character: owner_offset + owner.len() as u32,
                            },
                        },
                        severity: Some(DiagnosticSeverity::ERROR),
                        code: Some(NumberOrString::String(codes::INVALID_OWNER.to_string())),
                        source: Some("codeowners".to_string()),
                        message: error,
                        ..Default::default()
                    });
                } else {
                    // Format valid, queue for GitHub validation
                    owners_to_validate.push((
                        parsed_line.line_number,
                        owner_offset,
                        owner.clone(),
                        owner.len() as u32,
                    ));
                }
            }

            // Check for duplicate owners on same line
            let mut seen_owners: HashSet<&str> = HashSet::new();
            for owner in owners {
                if !seen_owners.insert(owner.as_str()) {
                    diagnostics.push(Diagnostic {
                        range: Range {
                            start: Position {
                                line: parsed_line.line_number,
                                character: parsed_line.owners_start,
                            },
                            end: Position {
                                line: parsed_line.line_number,
                                character: u32::MAX,
                            },
                        },
                        severity: Some(DiagnosticSeverity::WARNING),
                        code: Some(NumberOrString::String(codes::DUPLICATE_OWNER.to_string())),
                        source: Some("codeowners".to_string()),
                        message: format!("Duplicate owner '{}' on this line", owner),
                        ..Default::default()
                    });
                }
            }

            // Check for no matching files
            if let Some(cache) = file_cache {
                let match_count = cache.count_matches(pattern);
                if match_count == 0 {
                    diagnostics.push(Diagnostic {
                        range: Range {
                            start: Position {
                                line: parsed_line.line_number,
                                character: parsed_line.pattern_start,
                            },
                            end: Position {
                                line: parsed_line.line_number,
                                character: parsed_line.pattern_end,
                            },
                        },
                        severity: Some(DiagnosticSeverity::WARNING),
                        code: Some(NumberOrString::String(codes::PATTERN_NO_MATCH.to_string())),
                        source: Some("codeowners".to_string()),
                        message: "Pattern matches no files in the repository".to_string(),
                        ..Default::default()
                    });
                }
            }

            // Check for dead rules (earlier pattern completely shadowed by later)
            for (prev_pattern, prev_line) in &seen_patterns {
                if prev_pattern == pattern {
                    diagnostics.push(Diagnostic {
                        range: Range {
                            start: Position {
                                line: *prev_line,
                                character: 0,
                            },
                            end: Position {
                                line: *prev_line,
                                character: u32::MAX,
                            },
                        },
                        severity: Some(DiagnosticSeverity::HINT),
                        code: Some(NumberOrString::String(codes::SHADOWED_RULE.to_string())),
                        source: Some("codeowners".to_string()),
                        message: format!(
                            "This rule is shadowed by a later rule on line {} with the same pattern",
                            parsed_line.line_number + 1
                        ),
                        tags: Some(vec![DiagnosticTag::UNNECESSARY]),
                        ..Default::default()
                    });
                }
            }

            seen_patterns.push((pattern.clone(), parsed_line.line_number));

            // Check for rules without owners
            if owners.is_empty() {
                diagnostics.push(Diagnostic {
                    range: Range {
                        start: Position {
                            line: parsed_line.line_number,
                            character: parsed_line.pattern_start,
                        },
                        end: Position {
                            line: parsed_line.line_number,
                            character: parsed_line.pattern_end,
                        },
                    },
                    severity: Some(DiagnosticSeverity::WARNING),
                    code: Some(NumberOrString::String(codes::NO_OWNERS.to_string())),
                    source: Some("codeowners".to_string()),
                    message:
                        "Rule has no owners - files matching this pattern will have no code owners"
                            .to_string(),
                    ..Default::default()
                });
            }
        }
    }

    // Check for unowned files (coverage)
    if let Some(cache) = file_cache {
        let unowned = cache.get_unowned_files(&lines);
        if !unowned.is_empty() {
            let last_line = content.lines().count().saturating_sub(1) as u32;
            let sample_files: Vec<&str> = unowned.iter().take(5).map(|s| s.as_str()).collect();
            let message = if unowned.len() > 5 {
                format!(
                    "{} files have no code owners (e.g., {})",
                    unowned.len(),
                    sample_files.join(", ")
                )
            } else {
                format!(
                    "{} files have no code owners: {}",
                    unowned.len(),
                    sample_files.join(", ")
                )
            };

            diagnostics.push(Diagnostic {
                range: Range {
                    start: Position {
                        line: last_line,
                        character: 0,
                    },
                    end: Position {
                        line: last_line,
                        character: 0,
                    },
                },
                severity: Some(DiagnosticSeverity::INFORMATION),
                code: Some(NumberOrString::String(codes::UNOWNED_FILES.to_string())),
                source: Some("codeowners".to_string()),
                message,
                ..Default::default()
            });
        }
    }

    (diagnostics, owners_to_validate)
}

/// Add GitHub validation diagnostics (async)
pub async fn add_github_diagnostics(
    diagnostics: &mut Vec<Diagnostic>,
    owners_to_validate: Vec<OwnerValidationInfo>,
    github_client: &GitHubClient,
    token: &str,
) {
    for (line_number, owner_offset, owner, owner_len) in owners_to_validate {
        if let Some(false) = github_client.validate_owner(&owner, token).await {
            diagnostics.push(Diagnostic {
                range: Range {
                    start: Position {
                        line: line_number,
                        character: owner_offset,
                    },
                    end: Position {
                        line: line_number,
                        character: owner_offset + owner_len,
                    },
                },
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String(
                    codes::GITHUB_OWNER_NOT_FOUND.to_string(),
                )),
                source: Some("codeowners".to_string()),
                message: format!("Owner '{}' not found on GitHub", owner),
                ..Default::default()
            });
        }
    }
}

fn calculate_owner_offset(
    content: &str,
    parsed_line: &ParsedLine,
    index: usize,
    owner: &str,
) -> u32 {
    if index == 0 {
        parsed_line.owners_start
    } else {
        let line_text = content.lines().nth(parsed_line.line_number as usize);
        if let Some(text) = line_text {
            text.find(owner).unwrap_or(0) as u32
        } else {
            parsed_line.owners_start
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_pattern_diagnostic() {
        let content = "[invalid @owner";
        let (diagnostics, _) = compute_diagnostics_sync(content, None);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
        assert!(diagnostics[0].message.contains("Invalid glob pattern"));
    }

    #[test]
    fn test_invalid_owner_diagnostic() {
        let content = "*.rs invalid-owner";
        let (diagnostics, _) = compute_diagnostics_sync(content, None);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
        assert!(diagnostics[0].message.contains("Invalid owner format"));
    }

    #[test]
    fn test_duplicate_owner_diagnostic() {
        let content = "*.rs @owner @owner";
        let (diagnostics, _) = compute_diagnostics_sync(content, None);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::WARNING));
        assert!(diagnostics[0].message.contains("Duplicate owner"));
    }

    #[test]
    fn test_shadowed_rule_diagnostic() {
        let content = "*.rs @owner1\n*.rs @owner2";
        let (diagnostics, _) = compute_diagnostics_sync(content, None);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::HINT));
        assert!(diagnostics[0].message.contains("shadowed"));
        assert_eq!(diagnostics[0].range.start.line, 0); // First rule is shadowed
    }

    #[test]
    fn test_no_owners_diagnostic() {
        let content = "*.rs";
        let (diagnostics, _) = compute_diagnostics_sync(content, None);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::WARNING));
        assert!(diagnostics[0].message.contains("no owners"));
    }

    #[test]
    fn test_valid_content_no_diagnostics() {
        let content = "# Comment\n*.rs @owner\n/src/ @team/name";
        let (diagnostics, owners) = compute_diagnostics_sync(content, None);

        assert!(diagnostics.is_empty());
        assert_eq!(owners.len(), 2); // Two valid owners queued for GitHub validation
    }

    #[test]
    fn test_owners_to_validate_collected() {
        let content = "*.rs @user @org/team email@test.com";
        let (_, owners) = compute_diagnostics_sync(content, None);

        // All three owners should be queued
        assert_eq!(owners.len(), 3);
        assert_eq!(owners[0].2, "@user");
        assert_eq!(owners[1].2, "@org/team");
        assert_eq!(owners[2].2, "email@test.com");
    }
}
