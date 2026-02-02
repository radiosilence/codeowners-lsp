/// Simple glob pattern matching for CODEOWNERS patterns
pub fn pattern_matches(pattern: &str, path: &str) -> bool {
    pattern_matches_impl(pattern, path)
}

fn pattern_matches_impl(pattern: &str, path: &str) -> bool {
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

/// Check if pattern `a` is subsumed by pattern `b` (i.e., everything `a` matches, `b` also matches).
/// If true, and `b` comes after `a` in CODEOWNERS, then `a` is a dead rule.
pub fn pattern_subsumes(a: &str, b: &str) -> bool {
    let a = a.trim_start_matches('/');
    let b = b.trim_start_matches('/');

    // Identical patterns
    if a == b {
        return true;
    }

    // Universal patterns subsume everything
    if b == "*" || b == "**" {
        return true;
    }

    // Extension patterns: *.rs is subsumed by *
    if let Some(a_ext) = a.strip_prefix('*') {
        if b == "*" || b == "**" {
            return true;
        }
        // *.rs.bak is subsumed by *.bak
        if let Some(b_ext) = b.strip_prefix('*') {
            return a_ext.ends_with(b_ext);
        }
        return false;
    }

    // Directory patterns: /src/lib/ is subsumed by /src/
    let a_dir = a
        .trim_end_matches('/')
        .trim_end_matches("/**")
        .trim_end_matches("/*");
    let b_dir = b
        .trim_end_matches('/')
        .trim_end_matches("/**")
        .trim_end_matches("/*");

    let a_is_dir = a.ends_with('/') || a.ends_with("/**") || a.ends_with("/*");
    let b_is_dir = b.ends_with('/') || b.ends_with("/**") || b.ends_with("/*");

    // /src/lib/ subsumed by /src/ (more specific dir under more general)
    if a_is_dir && b_is_dir {
        return a_dir.starts_with(b_dir)
            && (a_dir == b_dir || a_dir.starts_with(&format!("{}/", b_dir)));
    }

    // Exact file in directory: src/main.rs subsumed by src/ or src/**
    if b_is_dir && !a_is_dir {
        return a.starts_with(b_dir) && (a == b_dir || a.starts_with(&format!("{}/", b_dir)));
    }

    false
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

    // Subsumption tests
    #[test]
    fn test_subsumes_identical() {
        assert!(pattern_subsumes("*.rs", "*.rs"));
        assert!(pattern_subsumes("/src/", "/src/"));
        assert!(pattern_subsumes("Makefile", "Makefile"));
    }

    #[test]
    fn test_subsumes_wildcard() {
        // * subsumes everything
        assert!(pattern_subsumes("*.rs", "*"));
        assert!(pattern_subsumes("*.go", "*"));
        assert!(pattern_subsumes("/src/", "*"));
        assert!(pattern_subsumes("Makefile", "*"));
        assert!(pattern_subsumes("src/main.rs", "*"));

        // ** also subsumes everything
        assert!(pattern_subsumes("*.rs", "**"));
        assert!(pattern_subsumes("/src/lib/", "**"));
    }

    #[test]
    fn test_subsumes_extension() {
        // *.rs.bak subsumed by *.bak
        assert!(pattern_subsumes("*.rs.bak", "*.bak"));
        // but not the other way
        assert!(!pattern_subsumes("*.bak", "*.rs.bak"));
        // *.rs not subsumed by *.go
        assert!(!pattern_subsumes("*.rs", "*.go"));
    }

    #[test]
    fn test_subsumes_directory() {
        // /src/lib/ subsumed by /src/
        assert!(pattern_subsumes("/src/lib/", "/src/"));
        assert!(pattern_subsumes("src/lib/", "src/"));
        // /src/** also subsumes /src/lib/
        assert!(pattern_subsumes("/src/lib/", "/src/**"));
        // but /src/ not subsumed by /src/lib/
        assert!(!pattern_subsumes("/src/", "/src/lib/"));
    }

    #[test]
    fn test_subsumes_file_in_dir() {
        // src/main.rs subsumed by src/
        assert!(pattern_subsumes("src/main.rs", "src/"));
        assert!(pattern_subsumes("src/main.rs", "src/**"));
        // but not by a different dir
        assert!(!pattern_subsumes("src/main.rs", "lib/"));
    }

    #[test]
    fn test_not_subsumed() {
        // Different extensions
        assert!(!pattern_subsumes("*.rs", "*.go"));
        // Different directories
        assert!(!pattern_subsumes("/src/", "/lib/"));
        // Wildcard doesn't subsume specific
        assert!(!pattern_subsumes("*", "*.rs"));
    }
}
