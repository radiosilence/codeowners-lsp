//! Resolve which rule in a CODEOWNERS file owns a given path, and locate
//! the CODEOWNERS file itself within a repository.
//!
//! GitHub's CODEOWNERS semantics are "last match wins": later rules override
//! earlier ones. The functions here encode that rule.

use std::path::{Path, PathBuf};

use crate::parser::{parse_codeowners_file_with_positions, CodeownersLine, ParsedLine};
use crate::pattern::pattern_matches;

/// Locate a CODEOWNERS file by walking up from `start`, checking
/// `.github/CODEOWNERS`, `CODEOWNERS`, and `docs/CODEOWNERS` at each level.
///
/// These are the three locations GitHub recognizes, checked in the same
/// priority order as `git`'s own CODEOWNERS resolution.
///
/// # Example
///
/// ```no_run
/// use std::path::Path;
/// use codeowners_parser::find_codeowners;
///
/// if let Some(path) = find_codeowners(Path::new(".")) {
///     println!("Found CODEOWNERS at {}", path.display());
/// }
/// ```
pub fn find_codeowners(start: &Path) -> Option<PathBuf> {
    const CANDIDATES: [&str; 3] = [".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"];
    let mut current = Some(start);
    while let Some(dir) = current {
        for candidate in CANDIDATES {
            let path = dir.join(candidate);
            if path.is_file() {
                return Some(path);
            }
        }
        current = dir.parent();
    }
    None
}

/// Get the repository root from a CODEOWNERS file path.
///
/// `.github/CODEOWNERS` and `docs/CODEOWNERS` live one directory below
/// the repo root; a top-level `CODEOWNERS` sits at the root itself.
/// If the parent cannot be determined, `fallback` is returned.
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

/// The rule that owns a file: which line matched, with what pattern, and
/// which owners were assigned.
///
/// Returned by [`check_file_ownership`] and [`check_file_ownership_parsed`].
#[derive(Debug, Clone)]
pub struct OwnershipResult {
    /// 0-indexed line number of the matching rule in the CODEOWNERS file.
    pub line_number: u32,
    /// The pattern text, as written in CODEOWNERS.
    pub pattern: String,
    /// Owners assigned to the matching rule. May be empty if the rule
    /// declares a pattern without any owners (a legal way of "unassigning"
    /// ownership via later rules).
    pub owners: Vec<String>,
}

/// Find which CODEOWNERS rule owns `file_path`, parsing `content` each call.
///
/// For hot loops where the same CODEOWNERS content is checked against many
/// paths, parse once and use [`check_file_ownership_parsed`] instead.
///
/// Returns `None` if no rule matches.
///
/// # Example
///
/// ```
/// use codeowners_parser::check_file_ownership;
///
/// let content = "*.rs @rust-team\n/src/main.rs @main-owner\n";
/// let result = check_file_ownership(content, "src/main.rs").unwrap();
/// assert_eq!(result.pattern, "/src/main.rs");
/// assert_eq!(result.owners, vec!["@main-owner"]);
/// ```
pub fn check_file_ownership(content: &str, file_path: &str) -> Option<OwnershipResult> {
    let lines = parse_codeowners_file_with_positions(content);
    check_file_ownership_parsed(&lines, file_path)
}

/// Find which CODEOWNERS rule owns `file_path` against pre-parsed lines.
///
/// Follows CODEOWNERS "last match wins" semantics: the returned result is
/// the last rule in `lines` whose pattern matches `file_path`.
///
/// Use this in hot loops where you'd otherwise call [`check_file_ownership`]
/// repeatedly against the same content — parse once with
/// [`parse_codeowners_file_with_positions`](crate::parser::parse_codeowners_file_with_positions)
/// and reuse the result.
///
/// Leading `./` is stripped from `file_path` before matching.
pub fn check_file_ownership_parsed(
    lines: &[ParsedLine],
    file_path: &str,
) -> Option<OwnershipResult> {
    let file_path = file_path.trim_start_matches("./");

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
    fn test_check_file_ownership_last_match_wins() {
        let content = "* @default\n*.rs @rust";
        let result = check_file_ownership(content, "main.rs").unwrap();
        assert_eq!(result.pattern, "*.rs");
        assert_eq!(result.owners, vec!["@rust"]);
    }

    #[test]
    fn test_check_file_ownership_strips_leading_dot_slash() {
        let content = "*.rs @rust";
        let result = check_file_ownership(content, "./src/main.rs").unwrap();
        assert_eq!(result.owners, vec!["@rust"]);
    }

    #[test]
    fn test_get_repo_root_github_subdir() {
        let path = PathBuf::from("/project/.github/CODEOWNERS");
        let fallback = PathBuf::from("/project");
        assert_eq!(get_repo_root(&path, &fallback), PathBuf::from("/project"));
    }

    #[test]
    fn test_get_repo_root_top_level() {
        let path = PathBuf::from("/project/CODEOWNERS");
        let fallback = PathBuf::from("/project");
        assert_eq!(get_repo_root(&path, &fallback), PathBuf::from("/project"));
    }

    #[test]
    fn test_get_repo_root_docs_directory() {
        let path = PathBuf::from("/project/docs/CODEOWNERS");
        let fallback = PathBuf::from("/project");
        assert_eq!(get_repo_root(&path, &fallback), PathBuf::from("/project"));
    }

    #[test]
    fn test_get_repo_root_fallback() {
        let path = PathBuf::from("/CODEOWNERS");
        let fallback = PathBuf::from("/fallback");
        assert_eq!(get_repo_root(&path, &fallback), PathBuf::from("/"));
    }
}
