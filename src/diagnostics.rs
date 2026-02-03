use std::collections::{HashMap, HashSet};

use tower_lsp::lsp_types::*;

use crate::file_cache::FileCache;
use crate::github::GitHubClient;
use crate::parser::{parse_codeowners_file_with_positions, CodeownersLine, ParsedLine};
use crate::pattern::pattern_subsumes;
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
    #[allow(dead_code)] // Used by LSP only
    pub const GITHUB_OWNER_NOT_FOUND: &str = "github-owner-not-found";
    #[allow(dead_code)] // Used by LSP only
    pub const FILE_NOT_OWNED: &str = "file-not-owned";
}

/// Configuration for diagnostic severities
/// None means the diagnostic is disabled ("off")
#[derive(Debug, Clone, Default)]
pub struct DiagnosticConfig {
    severities: HashMap<String, Option<DiagnosticSeverity>>,
}

impl DiagnosticConfig {
    /// Create config from a map of code -> severity string
    /// Valid severities: "off", "hint", "info", "warning", "error"
    #[allow(dead_code)] // Used by LSP only
    pub fn from_map(map: &HashMap<String, String>) -> Self {
        let mut severities = HashMap::new();
        for (code, severity_str) in map {
            severities.insert(code.clone(), parse_severity(severity_str));
        }
        Self { severities }
    }

    /// Get severity for a diagnostic code, returning the default if not configured
    pub fn get(&self, code: &str, default: DiagnosticSeverity) -> Option<DiagnosticSeverity> {
        match self.severities.get(code) {
            Some(severity) => *severity, // None means "off"
            None => Some(default),       // Not configured, use default
        }
    }
}

/// Parse a severity string into DiagnosticSeverity
/// Returns None for "off" (disabled)
#[allow(dead_code)] // Used by LSP only
fn parse_severity(s: &str) -> Option<DiagnosticSeverity> {
    match s.to_lowercase().as_str() {
        "off" | "none" | "disable" | "disabled" => None,
        "hint" => Some(DiagnosticSeverity::HINT),
        "info" | "information" => Some(DiagnosticSeverity::INFORMATION),
        "warn" | "warning" => Some(DiagnosticSeverity::WARNING),
        "error" => Some(DiagnosticSeverity::ERROR),
        _ => Some(DiagnosticSeverity::WARNING), // Unknown defaults to warning
    }
}

/// Owner validation info: (line_number, char_offset, owner_string, owner_len)
pub type OwnerValidationInfo = (u32, u32, String, u32);

/// Compute diagnostics for CODEOWNERS content (sync portion only)
pub fn compute_diagnostics_sync(
    content: &str,
    file_cache: Option<&FileCache>,
    config: &DiagnosticConfig,
) -> (Vec<Diagnostic>, Vec<OwnerValidationInfo>) {
    let mut diagnostics = Vec::new();
    let lines = parse_codeowners_file_with_positions(content);

    // Track patterns for dead rule detection
    // Use HashMap for O(1) exact duplicate detection
    let mut exact_patterns: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    // Only store patterns that could subsume others (wildcards, directories)
    let mut subsume_patterns: Vec<(String, u32)> = Vec::new();

    // Collect owners to validate via GitHub (line, offset, owner, len)
    let mut owners_to_validate: Vec<(u32, u32, String, u32)> = Vec::new();

    // Collect patterns for batch matching (pattern, line_number, pattern_start, pattern_end)
    let mut patterns_to_check: Vec<(&str, u32, u32, u32)> = Vec::new();

    for parsed_line in &lines {
        if let CodeownersLine::Rule { pattern, owners } = &parsed_line.content {
            // Check pattern validity
            if let Some(error) = validate_pattern(pattern) {
                if let Some(severity) =
                    config.get(codes::INVALID_PATTERN, DiagnosticSeverity::ERROR)
                {
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
                        severity: Some(severity),
                        code: Some(NumberOrString::String(codes::INVALID_PATTERN.to_string())),
                        source: Some("codeowners".to_string()),
                        message: error,
                        ..Default::default()
                    });
                }
            } else if file_cache.is_some() {
                // Only check valid patterns for file matches
                patterns_to_check.push((
                    pattern.as_str(),
                    parsed_line.line_number,
                    parsed_line.pattern_start,
                    parsed_line.pattern_end,
                ));
            }

            // Check owner validity (format only)
            for (i, owner) in owners.iter().enumerate() {
                let owner_offset = calculate_owner_offset(content, parsed_line, i, owner);

                if let Some(error) = validate_owner(owner) {
                    if let Some(severity) =
                        config.get(codes::INVALID_OWNER, DiagnosticSeverity::ERROR)
                    {
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
                            severity: Some(severity),
                            code: Some(NumberOrString::String(codes::INVALID_OWNER.to_string())),
                            source: Some("codeowners".to_string()),
                            message: error,
                            ..Default::default()
                        });
                    }
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
                    if let Some(severity) =
                        config.get(codes::DUPLICATE_OWNER, DiagnosticSeverity::WARNING)
                    {
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
                            severity: Some(severity),
                            code: Some(NumberOrString::String(codes::DUPLICATE_OWNER.to_string())),
                            source: Some("codeowners".to_string()),
                            message: format!("Duplicate owner '{}' on this line", owner),
                            ..Default::default()
                        });
                    }
                }
            }

            // Check for dead rules (earlier pattern completely shadowed by later)
            let normalized_pattern = pattern.trim_start_matches('/');

            // Fast path: check for exact duplicates via HashMap
            let is_exact_duplicate = exact_patterns.contains_key(normalized_pattern);
            if let Some(&prev_line) = exact_patterns.get(normalized_pattern) {
                if let Some(severity) =
                    config.get(codes::SHADOWED_RULE, DiagnosticSeverity::WARNING)
                {
                    diagnostics.push(Diagnostic {
                        range: Range {
                            start: Position {
                                line: prev_line,
                                character: 0,
                            },
                            end: Position {
                                line: prev_line,
                                character: u32::MAX,
                            },
                        },
                        severity: Some(severity),
                        code: Some(NumberOrString::String(codes::SHADOWED_RULE.to_string())),
                        source: Some("codeowners".to_string()),
                        message: format!(
                            "This rule is shadowed by a later rule on line {} with the same pattern",
                            parsed_line.line_number + 1
                        ),
                        tags: Some(vec![DiagnosticTag::UNNECESSARY]),
                        related_information: Some(vec![DiagnosticRelatedInformation {
                            location: Location {
                                uri: Url::parse("file:///CODEOWNERS").unwrap_or_else(|_| {
                                    Url::parse("file:///").unwrap()
                                }),
                                range: Range {
                                    start: Position {
                                        line: parsed_line.line_number,
                                        character: 0,
                                    },
                                    end: Position {
                                        line: parsed_line.line_number,
                                        character: u32::MAX,
                                    },
                                },
                            },
                            message: "Shadowing rule".to_string(),
                        }]),
                        ..Default::default()
                    });
                }
            }

            // Only check subsumption if current pattern could subsume others
            // (wildcards, directories, catch-alls) - skip exact file patterns for performance
            let could_subsume = pattern.contains('*') || pattern.ends_with('/');

            if could_subsume && !is_exact_duplicate {
                if let Some(severity) =
                    config.get(codes::SHADOWED_RULE, DiagnosticSeverity::WARNING)
                {
                    for (prev_pattern, prev_line) in &subsume_patterns {
                        if pattern_subsumes(prev_pattern, pattern) {
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
                                severity: Some(severity),
                                code: Some(NumberOrString::String(codes::SHADOWED_RULE.to_string())),
                                source: Some("codeowners".to_string()),
                                message: format!(
                                    "This rule is shadowed by a more general pattern '{}' on line {}",
                                    pattern,
                                    parsed_line.line_number + 1
                                ),
                                tags: Some(vec![DiagnosticTag::UNNECESSARY]),
                                ..Default::default()
                            });
                        }
                    }
                }
            }

            // Track this pattern
            exact_patterns.insert(normalized_pattern.to_string(), parsed_line.line_number);

            // Track ALL patterns for shadowing detection - any pattern can be shadowed by * or **
            subsume_patterns.push((pattern.to_string(), parsed_line.line_number));

            // Check for rules without owners
            if owners.is_empty() {
                // This is often intentional (opt-out of ownership), so just a hint
                if let Some(severity) = config.get(codes::NO_OWNERS, DiagnosticSeverity::HINT) {
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
                        severity: Some(severity),
                        code: Some(NumberOrString::String(codes::NO_OWNERS.to_string())),
                        source: Some("codeowners".to_string()),
                        message: "No owners specified (files will have no code owners)".to_string(),
                        ..Default::default()
                    });
                }
            }
        }
    }

    // Batch check for patterns with no matching files
    if let Some(cache) = file_cache {
        if let Some(severity) = config.get(codes::PATTERN_NO_MATCH, DiagnosticSeverity::WARNING) {
            let patterns: Vec<&str> = patterns_to_check.iter().map(|(p, _, _, _)| *p).collect();
            let matched = cache.find_patterns_with_matches(&patterns);

            for (i, (_, line_number, pattern_start, pattern_end)) in
                patterns_to_check.iter().enumerate()
            {
                if !matched.contains(&i) {
                    diagnostics.push(Diagnostic {
                        range: Range {
                            start: Position {
                                line: *line_number,
                                character: *pattern_start,
                            },
                            end: Position {
                                line: *line_number,
                                character: *pattern_end,
                            },
                        },
                        severity: Some(severity),
                        code: Some(NumberOrString::String(codes::PATTERN_NO_MATCH.to_string())),
                        source: Some("codeowners".to_string()),
                        message: "Pattern matches no files in the repository".to_string(),
                        ..Default::default()
                    });
                }
            }
        }
    }

    // Check for unowned files (coverage)
    if let Some(cache) = file_cache {
        if let Some(severity) = config.get(codes::UNOWNED_FILES, DiagnosticSeverity::INFORMATION) {
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
                    severity: Some(severity),
                    code: Some(NumberOrString::String(codes::UNOWNED_FILES.to_string())),
                    source: Some("codeowners".to_string()),
                    message,
                    ..Default::default()
                });
            }
        }
    }

    (diagnostics, owners_to_validate)
}

/// Add GitHub validation diagnostics (async)
#[allow(dead_code)] // Used by LSP only
pub async fn add_github_diagnostics(
    diagnostics: &mut Vec<Diagnostic>,
    owners_to_validate: Vec<OwnerValidationInfo>,
    github_client: &GitHubClient,
    token: &str,
    config: &DiagnosticConfig,
) {
    let Some(severity) = config.get(codes::GITHUB_OWNER_NOT_FOUND, DiagnosticSeverity::WARNING)
    else {
        return; // Disabled
    };

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
                severity: Some(severity),
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

    fn default_config() -> DiagnosticConfig {
        DiagnosticConfig::default()
    }

    #[test]
    fn test_invalid_pattern_diagnostic() {
        let content = "[invalid @owner";
        let (diagnostics, _) = compute_diagnostics_sync(content, None, &default_config());

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
        assert!(diagnostics[0].message.contains("Invalid glob pattern"));
    }

    #[test]
    fn test_invalid_owner_diagnostic() {
        let content = "*.rs invalid-owner";
        let (diagnostics, _) = compute_diagnostics_sync(content, None, &default_config());

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
        assert!(diagnostics[0].message.contains("Invalid owner format"));
    }

    #[test]
    fn test_duplicate_owner_diagnostic() {
        let content = "*.rs @owner @owner";
        let (diagnostics, _) = compute_diagnostics_sync(content, None, &default_config());

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::WARNING));
        assert!(diagnostics[0].message.contains("Duplicate owner"));
    }

    #[test]
    fn test_shadowed_rule_diagnostic() {
        let content = "*.rs @owner1\n*.rs @owner2";
        let (diagnostics, _) = compute_diagnostics_sync(content, None, &default_config());

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::WARNING));
        assert!(diagnostics[0].message.contains("shadowed"));
        assert_eq!(diagnostics[0].range.start.line, 0); // First rule is shadowed
    }

    #[test]
    fn test_catchall_shadows_everything() {
        // Classic footgun: catch-all at end shadows ALL previous rules
        let content = "/src/foo.rs @team1\ndocs/ @team2\n*.rs @team3\n* @default";
        let (diagnostics, _) = compute_diagnostics_sync(content, None, &default_config());

        // All 3 rules before * should be shadowed
        let shadowed: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.message.contains("shadowed"))
            .collect();
        assert_eq!(
            shadowed.len(),
            3,
            "Expected 3 shadowed rules, got: {:?}",
            shadowed
        );

        // Check that lines 0, 1, 2 are marked as shadowed
        let shadowed_lines: Vec<u32> = shadowed.iter().map(|d| d.range.start.line).collect();
        assert!(shadowed_lines.contains(&0), "Line 0 should be shadowed");
        assert!(shadowed_lines.contains(&1), "Line 1 should be shadowed");
        assert!(shadowed_lines.contains(&2), "Line 2 should be shadowed");
    }

    #[test]
    fn test_no_owners_diagnostic() {
        let content = "*.rs";
        let (diagnostics, _) = compute_diagnostics_sync(content, None, &default_config());

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::HINT));
        assert!(diagnostics[0].message.contains("No owners"));
    }

    #[test]
    fn test_valid_content_no_diagnostics() {
        let content = "# Comment\n*.rs @owner\n/src/ @team/name";
        let (diagnostics, owners) = compute_diagnostics_sync(content, None, &default_config());

        assert!(diagnostics.is_empty());
        assert_eq!(owners.len(), 2); // Two valid owners queued for GitHub validation
    }

    #[test]
    fn test_owners_to_validate_collected() {
        let content = "*.rs @user @org/team email@test.com";
        let (_, owners) = compute_diagnostics_sync(content, None, &default_config());

        // All three owners should be queued
        assert_eq!(owners.len(), 3);
        assert_eq!(owners[0].2, "@user");
        assert_eq!(owners[1].2, "@org/team");
        assert_eq!(owners[2].2, "email@test.com");
    }

    #[test]
    fn test_severity_override() {
        let content = "*.rs";
        let mut map = HashMap::new();
        map.insert("no-owners".to_string(), "error".to_string());
        let config = DiagnosticConfig::from_map(&map);

        let (diagnostics, _) = compute_diagnostics_sync(content, None, &config);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
    }

    #[test]
    fn test_severity_off() {
        let content = "*.rs";
        let mut map = HashMap::new();
        map.insert("no-owners".to_string(), "off".to_string());
        let config = DiagnosticConfig::from_map(&map);

        let (diagnostics, _) = compute_diagnostics_sync(content, None, &config);

        assert!(diagnostics.is_empty());
    }
}
