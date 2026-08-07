//! Syntactic validation for CODEOWNERS owners and glob patterns.
//!
//! These checks are _syntactic only_ — they confirm the shape of an owner
//! (`@user`, `@org/team`, or `email@host`) or the well-formedness of a glob
//! pattern. They do not verify that the user/team exists on GitHub or that
//! the pattern actually matches any files in the repository.

use glob::Pattern;
use once_cell::sync::Lazy;
use regex::Regex;

static TEAM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^@[a-zA-Z0-9-]+/[a-zA-Z0-9-]+$").unwrap());
static USER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^@[a-zA-Z0-9-]+$").unwrap());
static EMAIL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[^\s@]+@[^\s@]+\.[^\s@]+$").unwrap());

/// Validate an owner format - returns error message if invalid
pub fn validate_owner(owner: &str) -> Option<String> {
    if TEAM_RE.is_match(owner) || USER_RE.is_match(owner) || EMAIL_RE.is_match(owner) {
        None
    } else {
        Some(format!(
            "Invalid owner format '{}'. Expected @user, @org/team, or email@domain.com",
            owner
        ))
    }
}

/// Validate a glob pattern - returns error message if invalid
pub fn validate_pattern(pattern: &str) -> Option<String> {
    // Strip leading slash for glob validation
    let pattern_for_glob = pattern.trim_start_matches('/');

    // Empty pattern is invalid
    if pattern_for_glob.is_empty() {
        return Some("Empty pattern".to_string());
    }

    // These are valid general-purpose glob syntax, so the glob crate accepts
    // them — but GitHub documents both as unsupported in CODEOWNERS, where
    // they are treated as inert literals that silently match nothing.
    if pattern.starts_with('!') {
        return Some(
            "CODEOWNERS does not support '!' negation; the rule will never match".to_string(),
        );
    }
    if pattern.contains('[') {
        return Some(
            "CODEOWNERS does not support '[...]' character ranges; the rule will never match"
                .to_string(),
        );
    }

    // Try to compile as glob pattern
    if let Err(e) = Pattern::new(pattern_for_glob) {
        return Some(format!("Invalid glob pattern: {}", e));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Owner validation tests
    #[test]
    fn test_valid_user() {
        assert!(validate_owner("@username").is_none());
        assert!(validate_owner("@user-name").is_none());
        assert!(validate_owner("@user123").is_none());
    }

    #[test]
    fn test_valid_team() {
        assert!(validate_owner("@org/team").is_none());
        assert!(validate_owner("@my-org/my-team").is_none());
    }

    #[test]
    fn test_invalid_owner_with_underscore() {
        // GitHub usernames/orgs don't allow underscores
        assert!(validate_owner("@user_name").is_some());
        assert!(validate_owner("@org_name/team_name").is_some());
    }

    #[test]
    fn test_valid_email() {
        assert!(validate_owner("user@example.com").is_none());
        assert!(validate_owner("user.name@domain.co.uk").is_none());
        assert!(validate_owner("user+tag@example.org").is_none());
    }

    #[test]
    fn test_invalid_owner_with_period() {
        // GitHub usernames/orgs don't allow periods
        assert!(validate_owner("@user.name").is_some());
        assert!(validate_owner("@org.name/team").is_some());
    }

    #[test]
    fn test_invalid_owner_no_at() {
        assert!(validate_owner("username").is_some());
    }

    #[test]
    fn test_invalid_owner_empty() {
        assert!(validate_owner("").is_some());
        assert!(validate_owner("@").is_some());
    }

    #[test]
    fn test_invalid_owner_spaces() {
        assert!(validate_owner("@user name").is_some());
        assert!(validate_owner("user @name").is_some());
    }

    #[test]
    fn test_invalid_team_format() {
        assert!(validate_owner("@org/").is_some());
        assert!(validate_owner("@/team").is_some());
        assert!(validate_owner("@org//team").is_some());
    }

    // Pattern validation tests
    #[test]
    fn test_valid_patterns() {
        assert!(validate_pattern("*").is_none());
        assert!(validate_pattern("**").is_none());
        assert!(validate_pattern("*.rs").is_none());
        assert!(validate_pattern("/src/").is_none());
        assert!(validate_pattern("/src/**").is_none());
        assert!(validate_pattern("docs/*.md").is_none());
    }

    #[test]
    fn test_invalid_empty_pattern() {
        assert!(validate_pattern("").is_some());
        assert!(validate_pattern("/").is_some());
    }

    #[test]
    fn test_invalid_glob_syntax() {
        // Unclosed bracket
        assert!(validate_pattern("[invalid").is_some());
    }

    #[test]
    fn test_rejects_syntax_codeowners_does_not_support() {
        // Valid general-purpose glob, but GitHub treats both as inert literals
        // that silently match nothing — so they must not pass validation.
        assert!(validate_pattern("[abc].txt").is_some());
        assert!(validate_pattern("src/[a-z]*.rs").is_some());
        assert!(validate_pattern("!docs/").is_some());
        assert!(validate_pattern("!/src/main.rs").is_some());

        // A `!` that is not leading is just an ordinary filename character.
        assert!(validate_pattern("docs/hello!.md").is_none());
    }
}
