/// Simple glob pattern matching for CODEOWNERS patterns
pub fn pattern_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_start_matches('/');

    // Handle ** (matches everything)
    if pattern == "*" || pattern == "**" {
        return true;
    }

    // Handle directory patterns like /dir/ or dir/
    if pattern.ends_with('/') {
        let dir = pattern.trim_end_matches('/');
        return path.starts_with(dir);
    }

    // Handle patterns ending with /* or /**
    if pattern.ends_with("/**") || pattern.ends_with("/*") {
        let dir = pattern.trim_end_matches("/**").trim_end_matches("/*");
        return path.starts_with(dir);
    }

    // Handle extension patterns like *.rs
    if let Some(suffix) = pattern.strip_prefix('*') {
        return path.ends_with(suffix);
    }

    // Exact match or prefix match for directories
    path == pattern || path.starts_with(&format!("{}/", pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wildcard_all() {
        assert!(pattern_matches("*", "any/file.rs"));
        assert!(pattern_matches("**", "any/nested/file.rs"));
    }

    #[test]
    fn test_extension_pattern() {
        assert!(pattern_matches("*.rs", "src/main.rs"));
        assert!(pattern_matches("*.rs", "lib.rs"));
        assert!(!pattern_matches("*.rs", "src/main.go"));
        assert!(!pattern_matches("*.rs", "readme.md"));
    }

    #[test]
    fn test_directory_pattern_trailing_slash() {
        assert!(pattern_matches("/src/", "src/main.rs"));
        assert!(pattern_matches("/src/", "src/lib/mod.rs"));
        assert!(pattern_matches("src/", "src/main.rs"));
        assert!(!pattern_matches("/src/", "other/file.rs"));
    }

    #[test]
    fn test_directory_pattern_glob() {
        assert!(pattern_matches("/src/**", "src/main.rs"));
        assert!(pattern_matches("/src/**", "src/nested/deep/file.rs"));
        assert!(pattern_matches("/src/*", "src/main.rs"));
        assert!(!pattern_matches("/src/**", "other/file.rs"));
    }

    #[test]
    fn test_exact_match() {
        assert!(pattern_matches("Makefile", "Makefile"));
        assert!(pattern_matches("/Makefile", "Makefile"));
        assert!(!pattern_matches("Makefile", "other/Makefile"));
    }

    #[test]
    fn test_directory_prefix() {
        assert!(pattern_matches("src", "src/main.rs"));
        assert!(pattern_matches("src", "src/nested/file.rs"));
        assert!(!pattern_matches("src", "other/src/file.rs"));
    }

    #[test]
    fn test_leading_slash_stripped() {
        assert!(pattern_matches("/src/main.rs", "src/main.rs"));
        assert!(pattern_matches("src/main.rs", "src/main.rs"));
    }

    #[test]
    fn test_nested_directory() {
        assert!(pattern_matches("src/lib/", "src/lib/mod.rs"));
        assert!(pattern_matches("/src/lib/", "src/lib/nested.rs"));
        assert!(!pattern_matches("src/lib/", "src/other/file.rs"));
    }
}
