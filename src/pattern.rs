/// Pre-processed pattern for fast matching
pub enum CompiledPattern {
    /// Matches everything (* or **)
    MatchAll,
    /// Single-segment glob like *.rs - needs **/ prefix for matching
    SingleSegmentGlob(String),
    /// Multi-segment glob like src/**/*.rs
    MultiSegmentGlob(String),
    /// Directory pattern like src/ - matches prefix
    Directory(String),
    /// Exact path or directory prefix
    Exact(String),
}

impl CompiledPattern {
    pub fn new(pattern: &str) -> Self {
        let pattern = pattern.trim_start_matches('/');

        if pattern == "*" || pattern == "**" {
            return CompiledPattern::MatchAll;
        }

        if pattern.contains('*') {
            if !pattern.contains('/') {
                // Single segment like *.rs -> **/*.rs
                return CompiledPattern::SingleSegmentGlob(format!("**/{}", pattern));
            }
            return CompiledPattern::MultiSegmentGlob(pattern.to_string());
        }

        if pattern.ends_with('/') {
            return CompiledPattern::Directory(pattern.trim_end_matches('/').to_string());
        }

        CompiledPattern::Exact(pattern.to_string())
    }

    #[inline]
    pub fn matches(&self, path: &str) -> bool {
        match self {
            CompiledPattern::MatchAll => true,
            CompiledPattern::SingleSegmentGlob(glob) => fast_glob::glob_match(glob, path),
            CompiledPattern::MultiSegmentGlob(glob) => fast_glob::glob_match(glob, path),
            CompiledPattern::Directory(dir) => {
                path.starts_with(dir.as_str())
                    && (path.len() == dir.len() || path.as_bytes().get(dir.len()) == Some(&b'/'))
            }
            CompiledPattern::Exact(exact) => {
                path == exact
                    || (path.starts_with(exact.as_str())
                        && path.as_bytes().get(exact.len()) == Some(&b'/'))
            }
        }
    }
}

/// Simple glob pattern matching for CODEOWNERS patterns
#[inline]
pub fn pattern_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_start_matches('/');

    // Handle ** (matches everything)
    if pattern == "*" || pattern == "**" {
        return true;
    }

    // Handle complex patterns with * or ** - use fast-glob
    if pattern.contains('*') {
        // CODEOWNERS semantics: single-segment patterns like *.rs match in ANY directory
        // Convert *.rs to **/*.rs for fast-glob
        if !pattern.contains('/') {
            let glob_pattern = format!("**/{}", pattern);
            return fast_glob::glob_match(&glob_pattern, path);
        }
        return fast_glob::glob_match(pattern, path);
    }

    // Handle directory patterns like /dir/ or dir/
    if pattern.ends_with('/') {
        let dir = pattern.trim_end_matches('/');
        return path.starts_with(dir)
            && (path.len() == dir.len() || path[dir.len()..].starts_with('/'));
    }

    // Exact match or prefix match for directories
    path == pattern || path.starts_with(&format!("{}/", pattern))
}

/// Check if pattern `a` is subsumed by pattern `b` (i.e., everything `a` matches, `b` also matches).
/// If true, and `b` comes after `a` in CODEOWNERS, then `a` is a dead rule.
#[inline]
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
        return a_dir == b_dir || starts_with_dir(a_dir, b_dir);
    }

    // Exact file in directory: src/main.rs subsumed by src/ or src/**
    if b_is_dir && !a_is_dir {
        return a == b_dir || starts_with_dir(a, b_dir);
    }

    false
}

/// Check if `path` starts with `dir` followed by `/`
#[inline]
fn starts_with_dir(path: &str, dir: &str) -> bool {
    path.starts_with(dir) && path.as_bytes().get(dir.len()) == Some(&b'/')
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

    #[test]
    fn test_double_star_prefix() {
        // **/foo.txt matches foo.txt in any directory
        assert!(pattern_matches(
            "**/mirrord_config.json",
            "src/mirrord_config.json"
        ));
        assert!(pattern_matches(
            "**/mirrord_config.json",
            "mirrord_config.json"
        ));
        assert!(pattern_matches(
            "**/mirrord_config.json",
            "a/b/c/mirrord_config.json"
        ));
        assert!(!pattern_matches("**/mirrord_config.json", "src/other.json"));

        // Leading slash variant
        assert!(pattern_matches(
            "/**/mirrord_config.json",
            "src/mirrord_config.json"
        ));
        assert!(pattern_matches("/**/foo.txt", "foo.txt"));
        assert!(pattern_matches("/**/foo.txt", "dir/foo.txt"));
    }

    #[test]
    fn test_double_star_middle() {
        // src/**/test.rs matches src/test.rs, src/foo/test.rs, etc.
        assert!(pattern_matches("src/**/test.rs", "src/test.rs"));
        assert!(pattern_matches("src/**/test.rs", "src/foo/test.rs"));
        assert!(pattern_matches("src/**/test.rs", "src/foo/bar/test.rs"));
        assert!(!pattern_matches("src/**/test.rs", "other/test.rs"));
        assert!(!pattern_matches("src/**/test.rs", "src/foo/other.rs"));
    }

    #[test]
    fn test_single_star_in_path() {
        // deployment/*/deploy matches deployment/foo/deploy
        assert!(pattern_matches(
            "deployment/*/deploy/apps/staging/Chart.yaml",
            "deployment/analytics/deploy/apps/staging/Chart.yaml"
        ));
        assert!(pattern_matches(
            "deployment/*/deploy/**",
            "deployment/foo/deploy/bar/baz.yaml"
        ));
        assert!(!pattern_matches(
            "deployment/*/deploy/**",
            "other/foo/deploy/bar.yaml"
        ));
    }

    #[test]
    fn test_star_in_filename() {
        // *crowdin* matches files with crowdin in the name
        assert!(pattern_matches(
            ".github/workflows/*crowdin*",
            ".github/workflows/crowdin-download.yaml"
        ));
        assert!(pattern_matches(
            ".github/workflows/*crowdin*",
            ".github/workflows/upload-crowdin-files.yaml"
        ));
        assert!(!pattern_matches(
            ".github/workflows/*crowdin*",
            ".github/workflows/deploy.yaml"
        ));

        // create_service*.ex
        assert!(pattern_matches(
            "src/apps/platform_rpc/lib/platform_rpc/grpc/action/create_service*.ex",
            "src/apps/platform_rpc/lib/platform_rpc/grpc/action/create_service_foo.ex"
        ));
        assert!(pattern_matches(
            "lib/create_service*.ex",
            "lib/create_service_provider.ex"
        ));
    }

    #[test]
    fn test_star_prefix_suffix() {
        // appointment_review* matches appointment_review.ex, appointment_review_test.ex
        assert!(pattern_matches(
            "src/apps/platform/lib/schemas/appointment_review*",
            "src/apps/platform/lib/schemas/appointment_review.ex"
        ));
        assert!(pattern_matches(
            "src/apps/platform/lib/schemas/appointment_review*",
            "src/apps/platform/lib/schemas/appointment_review_test.ex"
        ));
        assert!(!pattern_matches(
            "src/apps/platform/lib/schemas/appointment_review*",
            "src/apps/platform/lib/schemas/other.ex"
        ));
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
