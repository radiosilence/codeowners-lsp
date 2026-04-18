//! LSP/CLI-side ownership helpers that depend on `FileCache`.
//!
//! Pure parsing and pattern-based ownership resolution live in
//! `codeowners_parser::ownership`. This module adds the fix engine that
//! consults the file system via [`FileCache`], and re-exports the parser's
//! ownership helpers for call-site ergonomics.

use std::collections::{HashMap, HashSet};

use codeowners_parser::parser::{parse_codeowners_file_with_positions, CodeownersLine};

pub use codeowners_parser::ownership::{
    check_file_ownership, check_file_ownership_parsed, find_codeowners, get_repo_root,
    OwnershipResult,
};

use crate::file_cache::FileCache;

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

    let mut exact_patterns: HashMap<String, usize> = HashMap::new();

    for parsed_line in &lines {
        if let CodeownersLine::Rule { pattern, owners } = &parsed_line.content {
            let line_num = parsed_line.line_number as usize;
            let normalized_pattern = pattern.trim_start_matches('/');

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

            if let Some(&prev_line) = exact_patterns.get(normalized_pattern) {
                lines_to_delete.insert(prev_line);
                fixes.push(format!(
                    "line {}: removed shadowed rule (duplicated on line {})",
                    prev_line + 1,
                    line_num + 1
                ));
            }
            exact_patterns.insert(normalized_pattern.to_string(), line_num);

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

    let mut result = Vec::new();
    for (i, line) in original_lines.iter().enumerate() {
        if lines_to_delete.contains(&i) {
            continue;
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
    fn test_apply_safe_fixes_all_duplicate_owners_removed() {
        let content = "*.rs @owner @owner\n";
        let result = apply_safe_fixes(content, None);
        assert_eq!(result.content, "*.rs @owner\n");
    }
}
