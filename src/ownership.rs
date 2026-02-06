//! Core CODEOWNERS operations shared between CLI and LSP

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::file_cache::FileCache;
use crate::parser::{parse_codeowners_file_with_positions, CodeownersLine};
use crate::pattern::pattern_matches;

/// Find a CODEOWNERS file starting from the given directory
#[allow(dead_code)] // Used by CLI only
pub fn find_codeowners(start: &Path) -> Option<PathBuf> {
    codeowners::locate(start)
}

/// Get the repository root from a CODEOWNERS file path
#[allow(dead_code)] // Used by CLI only
pub fn get_repo_root(codeowners_path: &Path, fallback: &Path) -> PathBuf {
    codeowners_path
        .parent()
        .and_then(|p| {
            if p.ends_with(".github") || p.ends_with("docs") {
                p.parent()
            } else {
                Some(p)
            }
        })
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| fallback.to_path_buf())
}

/// Result of checking which rule owns a file
#[derive(Debug)]
pub struct OwnershipResult {
    pub line_number: u32,
    pub pattern: String,
    #[allow(dead_code)] // Used by CLI only
    pub owners: Vec<String>,
}

/// Check which rule in a CODEOWNERS file owns a specific file
#[allow(dead_code)] // Used by LSP binary only
pub fn check_file_ownership(content: &str, file_path: &str) -> Option<OwnershipResult> {
    let lines = parse_codeowners_file_with_positions(content);
    check_file_ownership_parsed(&lines, file_path)
}

/// Check ownership against pre-parsed lines (avoids re-parsing in loops)
pub fn check_file_ownership_parsed(
    lines: &[crate::parser::ParsedLine],
    file_path: &str,
) -> Option<OwnershipResult> {
    let file_path = file_path.trim_start_matches("./");

    // Find matching rule (last match wins)
    let mut matching_rule = None;
    for parsed_line in lines {
        if let CodeownersLine::Rule { pattern, owners } = &parsed_line.content {
            if pattern_matches(pattern, file_path) {
                matching_rule = Some(OwnershipResult {
                    line_number: parsed_line.line_number,
                    pattern: pattern.clone(),
                    owners: owners.clone(),
                });
            }
        }
    }

    matching_rule
}

/// Fixes applied to a CODEOWNERS file
pub struct FixResult {
    pub content: String,
    pub fixes: Vec<String>,
}

/// Apply safe fixes to CODEOWNERS content.
/// Safe fixes: duplicate owners, exact duplicate patterns (shadowed rules),
/// and patterns matching no files (when file_cache is provided).
pub fn apply_safe_fixes(content: &str, file_cache: Option<&FileCache>) -> FixResult {
    let lines = parse_codeowners_file_with_positions(content);
    let original_lines: Vec<&str> = content.lines().collect();

    let mut fixes = Vec::new();
    let mut lines_to_delete: HashSet<usize> = HashSet::new();
    let mut line_replacements: HashMap<usize, String> = HashMap::new();

    // Track patterns for shadowed rule detection
    let mut exact_patterns: HashMap<String, usize> = HashMap::new();

    for parsed_line in &lines {
        if let CodeownersLine::Rule { pattern, owners } = &parsed_line.content {
            let line_num = parsed_line.line_number as usize;
            let normalized_pattern = pattern.trim_start_matches('/');

            // Fix 1: Remove duplicate owners
            let mut seen_owners: HashSet<&str> = HashSet::new();
            let deduped: Vec<&str> = owners
                .iter()
                .map(|s| s.as_str())
                .filter(|o| seen_owners.insert(*o))
                .collect();

            if deduped.len() < owners.len() {
                let new_line = if deduped.is_empty() {
                    pattern.clone()
                } else {
                    format!("{} {}", pattern, deduped.join(" "))
                };
                line_replacements.insert(line_num, new_line);
                fixes.push(format!("line {}: removed duplicate owners", line_num + 1));
            }

            // Fix 2: Remove shadowed rules (exact duplicates)
            if let Some(&prev_line) = exact_patterns.get(normalized_pattern) {
                lines_to_delete.insert(prev_line);
                fixes.push(format!(
                    "line {}: removed shadowed rule (duplicated on line {})",
                    prev_line + 1,
                    line_num + 1
                ));
            }
            exact_patterns.insert(normalized_pattern.to_string(), line_num);

            // Fix 3: Remove patterns that match no files
            if let Some(cache) = file_cache {
                if !cache.has_matches(pattern) {
                    lines_to_delete.insert(line_num);
                    fixes.push(format!(
                        "line {}: removed pattern '{}' (matches no files)",
                        line_num + 1,
                        pattern
                    ));
                }
            }
        }
    }

    // Build the fixed content
    let mut result = Vec::new();
    for (i, line) in original_lines.iter().enumerate() {
        if lines_to_delete.contains(&i) {
            continue; // Skip deleted lines
        }
        if let Some(replacement) = line_replacements.get(&i) {
            result.push(replacement.clone());
        } else {
            result.push(line.to_string());
        }
    }

    let mut output = result.join("\n");
    if !content.is_empty() && content.ends_with('\n') && !output.ends_with('\n') {
        output.push('\n');
    }

    FixResult {
        content: output,
        fixes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_file_ownership() {
        let content = "*.rs @rust-team\n/src/ @src-team\n/src/main.rs @main-owner";
        let result = check_file_ownership(content, "src/main.rs").unwrap();
        assert_eq!(result.pattern, "/src/main.rs");
        assert_eq!(result.owners, vec!["@main-owner"]);
    }

    #[test]
    fn test_check_file_no_owner() {
        let content = "*.rs @rust-team";
        let result = check_file_ownership(content, "README.md");
        assert!(result.is_none());
    }

    #[test]
    fn test_apply_safe_fixes_duplicate_owners() {
        let content = "*.rs @owner @owner @other\n";
        let result = apply_safe_fixes(content, None);
        assert_eq!(result.content, "*.rs @owner @other\n");
        assert_eq!(result.fixes.len(), 1);
    }

    #[test]
    fn test_apply_safe_fixes_shadowed_rules() {
        let content = "*.rs @first\n*.rs @second\n";
        let result = apply_safe_fixes(content, None);
        assert_eq!(result.content, "*.rs @second\n");
        assert_eq!(result.fixes.len(), 1);
    }

    #[test]
    fn test_get_repo_root() {
        let path = PathBuf::from("/project/.github/CODEOWNERS");
        let fallback = PathBuf::from("/project");
        assert_eq!(get_repo_root(&path, &fallback), PathBuf::from("/project"));

        let path = PathBuf::from("/project/CODEOWNERS");
        assert_eq!(get_repo_root(&path, &fallback), PathBuf::from("/project"));
    }

    #[test]
    fn test_apply_safe_fixes_all_duplicate_owners_removed() {
        // When all owners are duplicates, pattern should remain alone (line 98)
        let content = "*.rs @owner @owner\n";
        let result = apply_safe_fixes(content, None);
        assert_eq!(result.content, "*.rs @owner\n");
    }

    #[test]
    fn test_get_repo_root_docs_directory() {
        // Test docs/ directory handling
        let path = PathBuf::from("/project/docs/CODEOWNERS");
        let fallback = PathBuf::from("/project");
        assert_eq!(get_repo_root(&path, &fallback), PathBuf::from("/project"));
    }

    #[test]
    fn test_get_repo_root_fallback() {
        // Test fallback when parent is None (root path)
        let path = PathBuf::from("/CODEOWNERS");
        let fallback = PathBuf::from("/fallback");
        // Parent of /CODEOWNERS is /, which is not .github or docs
        assert_eq!(get_repo_root(&path, &fallback), PathBuf::from("/"));
    }

    #[test]
    fn test_check_file_ownership_last_match_wins() {
        // Verify last matching pattern wins
        let content = "* @default\n*.rs @rust";
        let result = check_file_ownership(content, "main.rs").unwrap();
        assert_eq!(result.pattern, "*.rs");
        assert_eq!(result.owners, vec!["@rust"]);
    }
}
