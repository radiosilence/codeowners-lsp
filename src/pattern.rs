/// Pre-processed pattern for fast matching
pub enum CompiledPattern {
    /// Matches everything (unanchored * or **)
    MatchAll,
    /// Anchored /* - matches only root-level files
    RootFilesOnly,
    /// Extension suffix like .rs (from *.rs) - simple ends_with check
    ExtensionSuffix(String),
    /// Single-segment glob like *.rs - needs **/ prefix for matching
    SingleSegmentGlob(String),
    /// Multi-segment glob like src/**/*.rs
    MultiSegmentGlob(String),
    /// Anchored directory pattern like /src/ - matches prefix at root only
    AnchoredDirectory(String),
    /// Unanchored directory pattern like docs/ - matches anywhere
    UnanchoredDirectory(String),
    /// Exact path or directory prefix (always anchored)
    Exact(String),
}

impl CompiledPattern {
    pub fn new(pattern: &str) -> Self {
        let anchored = pattern.starts_with('/');
        let pattern = pattern.trim_start_matches('/');

        // Catch-all patterns
        if pattern == "*" || pattern == "**" {
            if anchored && pattern == "*" {
                return CompiledPattern::RootFilesOnly;
            }
            return CompiledPattern::MatchAll;
        }

        // Patterns with wildcards
        if pattern.contains('*') {
            // Unanchored single-segment like *.rs
            if !anchored && !pattern.contains('/') {
                // Fast path: *.ext patterns use suffix check (no glob needed)
                if let Some(ext) = pattern.strip_prefix('*') {
                    if !ext.contains('*') && !ext.contains('?') {
                        return CompiledPattern::ExtensionSuffix(ext.to_string());
                    }
                }
                return CompiledPattern::SingleSegmentGlob(format!("**/{}", pattern));
            }
            return CompiledPattern::MultiSegmentGlob(pattern.to_string());
        }

        // Directory patterns (trailing /)
        if pattern.ends_with('/') {
            let dir = pattern.trim_end_matches('/').to_string();
            if anchored {
                return CompiledPattern::AnchoredDirectory(dir);
            } else {
                return CompiledPattern::UnanchoredDirectory(dir);
            }
        }

        // Exact patterns (always anchored)
        CompiledPattern::Exact(pattern.to_string())
    }

    #[inline]
    pub fn matches(&self, path: &str) -> bool {
        if path.is_empty() {
            return false;
        }

        match self {
            CompiledPattern::MatchAll => true,
            CompiledPattern::RootFilesOnly => !path.contains('/'),
            CompiledPattern::ExtensionSuffix(ext) => {
                path.ends_with(ext.as_str())
                    && (path.len() == ext.len()
                        || path.as_bytes()[path.len() - ext.len() - 1] != b'/')
            }
            CompiledPattern::SingleSegmentGlob(glob) => fast_glob::glob_match(glob, path),
            CompiledPattern::MultiSegmentGlob(glob) => fast_glob::glob_match(glob, path),
            CompiledPattern::AnchoredDirectory(dir) => {
                path.starts_with(dir.as_str())
                    && (path.len() == dir.len() || path.as_bytes().get(dir.len()) == Some(&b'/'))
            }
            CompiledPattern::UnanchoredDirectory(dir) => {
                // Match at root
                if path.starts_with(dir.as_str())
                    && (path.len() == dir.len() || path.as_bytes().get(dir.len()) == Some(&b'/'))
                {
                    return true;
                }
                // Match anywhere: check each path segment boundary
                let dir_bytes = dir.as_bytes();
                let path_bytes = path.as_bytes();
                for (i, _) in path.match_indices('/') {
                    let rest = &path_bytes[i + 1..];
                    if rest.len() >= dir_bytes.len()
                        && &rest[..dir_bytes.len()] == dir_bytes
                        && (rest.len() == dir_bytes.len() || rest[dir_bytes.len()] == b'/')
                    {
                        return true;
                    }
                }
                false
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
///
/// Key rules:
/// - Leading `/` anchors pattern to repository root
/// - No leading `/`:
///   - If ends with `/`: directory pattern, matches anywhere in tree
///   - If contains `/`: implicitly anchored to root
///   - If no `/` and has `*`: matches at any depth (e.g., `*.rs`)
///   - If no `/` and no `*`: anchored to root
/// - `*` matches any characters except `/`
/// - `**` matches zero or more directories
#[inline]
pub fn pattern_matches(pattern: &str, path: &str) -> bool {
    // Empty path never matches (edge case)
    if path.is_empty() {
        return false;
    }

    // Check if pattern is anchored (has leading /)
    let anchored = pattern.starts_with('/');
    let pattern = pattern.trim_start_matches('/');

    // Handle catch-all patterns (matches everything)
    // Only unanchored * or ** are true catch-alls
    if !anchored && (pattern == "*" || pattern == "**") {
        return true;
    }

    // Handle patterns with wildcards
    if pattern.contains('*') {
        // Anchored single-star only matches root level
        // /* should only match files directly in root, not nested
        if anchored && pattern == "*" {
            // Match only if path has no directory separator
            return !path.contains('/');
        }

        // Single-segment patterns like *.rs (unanchored) match at any depth
        if !anchored && !pattern.contains('/') {
            // Fast path: *.ext is just a suffix check
            if let Some(ext) = pattern.strip_prefix('*') {
                return path.ends_with(ext)
                    && (path.len() == ext.len()
                        || path.as_bytes()[path.len() - ext.len() - 1] != b'/');
            }
            let glob_pattern = format!("**/{}", pattern);
            return fast_glob::glob_match(&glob_pattern, path);
        }

        // Multi-segment patterns or anchored patterns: match directly
        return fast_glob::glob_match(pattern, path);
    }

    // Handle directory patterns (trailing /)
    if pattern.ends_with('/') {
        let dir = pattern.trim_end_matches('/');

        if anchored {
            // /docs/ - only matches docs/ at root
            return path.starts_with(dir)
                && (path.len() == dir.len() || path.as_bytes().get(dir.len()) == Some(&b'/'));
        } else {
            // docs/ - matches any directory named "docs" anywhere
            if path.starts_with(dir)
                && (path.len() == dir.len() || path.as_bytes().get(dir.len()) == Some(&b'/'))
            {
                return true;
            }
            // Check each path segment boundary (no allocations)
            let dir_bytes = dir.as_bytes();
            let path_bytes = path.as_bytes();
            for (i, _) in path.match_indices('/') {
                let rest = &path_bytes[i + 1..];
                if rest.len() >= dir_bytes.len()
                    && &rest[..dir_bytes.len()] == dir_bytes
                    && (rest.len() == dir_bytes.len() || rest[dir_bytes.len()] == b'/')
                {
                    return true;
                }
            }
            return false;
        }
    }

    // Handle exact/prefix patterns (no wildcards, no trailing /)
    // These are always anchored (either explicitly or implicitly)
    path == pattern
        || (path.starts_with(pattern) && path.as_bytes().get(pattern.len()) == Some(&b'/'))
}

/// Check if pattern `a` is subsumed by pattern `b` (i.e., everything `a` matches, `b` also matches).
/// If true, and `b` comes after `a` in CODEOWNERS, then `a` is a dead rule.
///
/// Key anchoring rules:
/// - Unanchored patterns (no leading `/`) match MORE files than anchored patterns
/// - `/docs/` (anchored) IS subsumed by `docs/` (unanchored)
/// - `docs/` (unanchored) is NOT subsumed by `/docs/` (anchored)
#[inline]
pub fn pattern_subsumes(a: &str, b: &str) -> bool {
    // Track anchoring before stripping
    let a_anchored = a.starts_with('/');
    let b_anchored = b.starts_with('/');

    let a = a.trim_start_matches('/');
    let b = b.trim_start_matches('/');

    // Identical patterns (both same anchoring and content)
    if a == b && a_anchored == b_anchored {
        return true;
    }

    // For exact paths (no wildcards, no trailing /), patterns containing /
    // are implicitly anchored, so /src/foo and src/foo are equivalent
    let a_is_exact = !a.contains('*') && !a.ends_with('/');
    let b_is_exact = !b.contains('*') && !b.ends_with('/');
    if a_is_exact && b_is_exact && a == b && a.contains('/') {
        // Both are exact paths with /, one has leading / and one doesn't
        // They match the same files, so b subsumes a
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

    // Handle anchored vs unanchored directory patterns
    if a_is_dir && b_is_dir {
        // Unanchored pattern matches MORE files than anchored
        // So: anchored IS subsumed by unanchored (if same path)
        //     unanchored is NOT subsumed by anchored
        if a_anchored && !b_anchored {
            // /docs/ subsumed by docs/ - yes, if same or b is parent
            return a_dir == b_dir || starts_with_dir(a_dir, b_dir);
        }
        if !a_anchored && b_anchored {
            // docs/ subsumed by /docs/ - NO, unanchored matches nested paths
            return false;
        }
        // Both same anchoring - normal rules
        return a_dir == b_dir || starts_with_dir(a_dir, b_dir);
    }

    // Exact file in directory: src/main.rs subsumed by src/ or src/**
    if b_is_dir && !a_is_dir {
        // If b is unanchored, it matches more, so a could be subsumed
        // If b is anchored and a is not a nested path, still works
        if !a_anchored && b_anchored {
            // Unanchored file path not subsumed by anchored dir
            return false;
        }
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

    // =============================================================================
    // GITHUB CODEOWNERS CONFORMANCE TEST SUITE
    // =============================================================================
    // Based on official GitHub documentation and observed behavior.
    // Reference: reports/compass_artifact_wf-*.md
    //
    // Key rules:
    // 1. Last matching pattern wins (not most specific)
    // 2. Leading `/` anchors pattern to repository root
    // 3. No leading `/` = pattern can match anywhere in tree (for certain patterns)
    // 4. Trailing `/` = directory pattern, matches recursively
    // 5. `*` matches any characters EXCEPT `/` (doesn't cross directories)
    // 6. `**` matches zero or more directories (crosses boundaries)
    // 7. Patterns are case-sensitive
    // 8. `[abc]` character ranges NOT supported (unlike gitignore)
    // 9. `!` negation patterns NOT supported (unlike gitignore)
    // =============================================================================

    // ---------------------------------------------------------------------------
    // CATEGORY 1: CATCH-ALL PATTERNS
    // ---------------------------------------------------------------------------
    // `*` and `**` at the root level match every file in the repository.

    #[test]
    fn test_catchall_star() {
        // * matches every file in the repository (not just root)
        assert!(pattern_matches("*", "readme.md"));
        assert!(pattern_matches("*", "src/main.rs"));
        assert!(pattern_matches("*", "a/b/c/d/e/f.txt"));
        assert!(pattern_matches("*", ".hidden"));
        assert!(pattern_matches("*", "deep/nested/path/file.ext"));
    }

    #[test]
    fn test_catchall_double_star() {
        // ** also matches everything
        assert!(pattern_matches("**", "readme.md"));
        assert!(pattern_matches("**", "src/main.rs"));
        assert!(pattern_matches("**", "a/b/c/d/e/f.txt"));
    }

    // ---------------------------------------------------------------------------
    // CATEGORY 2: EXTENSION PATTERNS (no path separator)
    // ---------------------------------------------------------------------------
    // Patterns like `*.rs` without any `/` match files at ANY depth.
    // This is CODEOWNERS-specific behavior.

    #[test]
    fn test_extension_matches_any_depth() {
        // *.rs matches .rs files at any depth
        assert!(pattern_matches("*.rs", "main.rs"));
        assert!(pattern_matches("*.rs", "src/main.rs"));
        assert!(pattern_matches("*.rs", "src/lib/mod.rs"));
        assert!(pattern_matches("*.rs", "a/b/c/d/file.rs"));
    }

    #[test]
    fn test_extension_no_match_wrong_ext() {
        assert!(!pattern_matches("*.rs", "main.go"));
        assert!(!pattern_matches("*.rs", "src/main.go"));
        assert!(!pattern_matches("*.rs", "readme.md"));
        assert!(!pattern_matches("*.rs", "file.rs.bak")); // .rs.bak != .rs
    }

    #[test]
    fn test_extension_complex() {
        // *.config.js should match config.js files
        assert!(pattern_matches("*.config.js", "webpack.config.js"));
        assert!(pattern_matches("*.config.js", "src/babel.config.js"));
        assert!(!pattern_matches("*.config.js", "config.js")); // no prefix before .config.js
    }

    // ---------------------------------------------------------------------------
    // CATEGORY 3: ANCHORED DIRECTORY PATTERNS (leading `/`, trailing `/`)
    // ---------------------------------------------------------------------------
    // `/docs/` = directory at root, matches ALL contents recursively

    #[test]
    fn test_anchored_dir_matches_recursively() {
        // /docs/ matches docs/* and docs/**/* at root only
        assert!(pattern_matches("/docs/", "docs/readme.md"));
        assert!(pattern_matches("/docs/", "docs/api/index.md"));
        assert!(pattern_matches("/docs/", "docs/a/b/c/deep.txt"));
    }

    #[test]
    fn test_anchored_dir_no_match_elsewhere() {
        // /docs/ does NOT match docs/ nested elsewhere
        assert!(!pattern_matches("/docs/", "src/docs/file.txt"));
        assert!(!pattern_matches("/docs/", "other/docs/readme.md"));
        assert!(!pattern_matches("/docs/", "a/b/docs/c.txt"));
    }

    #[test]
    fn test_anchored_dir_no_match_partial_name() {
        // /docs/ should not match /documentation/
        assert!(!pattern_matches("/docs/", "documentation/readme.md"));
        assert!(!pattern_matches("/docs/", "docs-old/readme.md"));
    }

    // ---------------------------------------------------------------------------
    // CATEGORY 4: UNANCHORED DIRECTORY PATTERNS (no leading `/`, trailing `/`)
    // ---------------------------------------------------------------------------
    // `docs/` (no leading slash) = matches ANY directory named `docs` ANYWHERE

    #[test]
    fn test_unanchored_dir_matches_at_root() {
        assert!(pattern_matches("docs/", "docs/readme.md"));
        assert!(pattern_matches("docs/", "docs/api/index.md"));
    }

    #[test]
    fn test_unanchored_dir_matches_nested() {
        // This is the critical difference from anchored patterns
        assert!(pattern_matches("docs/", "src/docs/file.txt"));
        assert!(pattern_matches("docs/", "a/b/docs/deep.txt"));
        assert!(pattern_matches("docs/", "project/src/docs/api/v1/spec.md"));
    }

    #[test]
    fn test_unanchored_dir_no_match_partial_name() {
        // Must be exact directory name
        assert!(!pattern_matches("docs/", "documentation/readme.md"));
        assert!(!pattern_matches("docs/", "mydocs/readme.md"));
        assert!(!pattern_matches("docs/", "src/mydocs/file.txt"));
    }

    // ---------------------------------------------------------------------------
    // CATEGORY 5: ANCHORED PATH PATTERNS (leading `/`, no wildcards)
    // ---------------------------------------------------------------------------
    // `/src/main.rs` = exact file at root
    // `/src` = file or directory named `src` at root

    #[test]
    fn test_anchored_exact_file() {
        assert!(pattern_matches("/Makefile", "Makefile"));
        assert!(pattern_matches("/src/main.rs", "src/main.rs"));
        assert!(!pattern_matches("/Makefile", "build/Makefile"));
        assert!(!pattern_matches("/src/main.rs", "other/src/main.rs"));
    }

    #[test]
    fn test_anchored_name_as_directory() {
        // /docs (no trailing slash) matches file or directory named docs at root
        assert!(pattern_matches("/docs", "docs")); // exact file match
        assert!(pattern_matches("/docs", "docs/anything")); // as directory prefix
        assert!(pattern_matches("/docs", "docs/nested/deep.txt"));
        assert!(!pattern_matches("/docs", "src/docs"));
        assert!(!pattern_matches("/docs", "src/docs/file.txt"));
    }

    // ---------------------------------------------------------------------------
    // CATEGORY 6: SINGLE-SEGMENT UNANCHORED PATTERNS (no `/` at all)
    // ---------------------------------------------------------------------------
    // `Makefile` (no slashes) - behavior depends on interpretation
    // In gitignore: matches anywhere. In CODEOWNERS: typically anchored.

    #[test]
    fn test_single_segment_exact() {
        // Single segment without slashes - anchored to root
        assert!(pattern_matches("Makefile", "Makefile"));
        assert!(pattern_matches("README.md", "README.md"));
    }

    #[test]
    fn test_single_segment_as_directory() {
        // Single segment can also act as directory prefix
        assert!(pattern_matches("src", "src/main.rs"));
        assert!(pattern_matches("src", "src/lib/mod.rs"));
    }

    #[test]
    fn test_single_segment_no_match_nested() {
        // Should NOT match same name nested elsewhere (anchored behavior)
        assert!(!pattern_matches("Makefile", "build/Makefile"));
        assert!(!pattern_matches("src", "project/src/file.rs"));
    }

    // ---------------------------------------------------------------------------
    // CATEGORY 7: MULTI-SEGMENT UNANCHORED PATTERNS (has `/`, no leading `/`)
    // ---------------------------------------------------------------------------
    // `src/main.rs` (contains `/` but no leading `/`) - implicitly anchored

    #[test]
    fn test_multi_segment_implicitly_anchored() {
        assert!(pattern_matches("src/main.rs", "src/main.rs"));
        assert!(pattern_matches("src/lib/mod.rs", "src/lib/mod.rs"));
        // Should NOT match nested (implicitly anchored)
        assert!(!pattern_matches("src/main.rs", "project/src/main.rs"));
    }

    // ---------------------------------------------------------------------------
    // CATEGORY 8: SINGLE STAR PATTERNS (`*` - doesn't cross directories)
    // ---------------------------------------------------------------------------
    // `*` matches any character EXCEPT `/`

    #[test]
    fn test_star_direct_children_only() {
        // /docs/* matches only direct children of docs
        assert!(pattern_matches("/docs/*", "docs/readme.md"));
        assert!(pattern_matches("/docs/*", "docs/index.html"));
        // Does NOT match nested paths
        assert!(!pattern_matches("/docs/*", "docs/api/index.md"));
        assert!(!pattern_matches("/docs/*", "docs/a/b/file.txt"));
    }

    #[test]
    fn test_star_in_filename() {
        // Pattern with * in filename portion
        assert!(pattern_matches("docs/*.md", "docs/readme.md"));
        assert!(pattern_matches("docs/*.md", "docs/CHANGELOG.md"));
        assert!(!pattern_matches("docs/*.md", "docs/readme.txt"));
        assert!(!pattern_matches("docs/*.md", "docs/api/readme.md")); // nested
    }

    #[test]
    fn test_star_prefix_suffix() {
        // *crowdin* matches files with crowdin in the name
        assert!(pattern_matches(
            ".github/workflows/*crowdin*",
            ".github/workflows/crowdin-download.yaml"
        ));
        assert!(pattern_matches(
            ".github/workflows/*crowdin*",
            ".github/workflows/upload-crowdin.yaml"
        ));
        assert!(!pattern_matches(
            ".github/workflows/*crowdin*",
            ".github/workflows/deploy.yaml"
        ));
    }

    #[test]
    fn test_star_middle_of_path() {
        // deployment/*/config.yaml - single directory wildcard
        assert!(pattern_matches(
            "deployment/*/config.yaml",
            "deployment/prod/config.yaml"
        ));
        assert!(pattern_matches(
            "deployment/*/config.yaml",
            "deployment/staging/config.yaml"
        ));
        // Does not match nested
        assert!(!pattern_matches(
            "deployment/*/config.yaml",
            "deployment/env/prod/config.yaml"
        ));
    }

    // ---------------------------------------------------------------------------
    // CATEGORY 9: DOUBLE STAR PATTERNS (`**` - crosses directories)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_double_star_leading() {
        // **/file.txt matches file.txt at any depth
        assert!(pattern_matches("**/config.json", "config.json"));
        assert!(pattern_matches("**/config.json", "src/config.json"));
        assert!(pattern_matches("**/config.json", "a/b/c/config.json"));
        assert!(!pattern_matches("**/config.json", "src/other.json"));
    }

    #[test]
    fn test_double_star_trailing() {
        // docs/** matches everything inside docs recursively
        assert!(pattern_matches("docs/**", "docs/readme.md"));
        assert!(pattern_matches("docs/**", "docs/api/index.md"));
        assert!(pattern_matches("docs/**", "docs/a/b/c/deep.txt"));
        assert!(!pattern_matches("docs/**", "other/readme.md"));
    }

    #[test]
    fn test_double_star_middle() {
        // a/**/b matches a/b, a/x/b, a/x/y/b
        assert!(pattern_matches("a/**/b", "a/b"));
        assert!(pattern_matches("a/**/b", "a/x/b"));
        assert!(pattern_matches("a/**/b", "a/x/y/b"));
        assert!(pattern_matches("a/**/b", "a/x/y/z/b"));
        assert!(!pattern_matches("a/**/b", "a/x/y/c"));
        assert!(!pattern_matches("a/**/b", "x/a/b")); // anchored
    }

    #[test]
    fn test_double_star_with_extension() {
        // docs/**/*.md matches all .md files under docs
        assert!(pattern_matches("docs/**/*.md", "docs/readme.md"));
        assert!(pattern_matches("docs/**/*.md", "docs/api/spec.md"));
        assert!(pattern_matches("docs/**/*.md", "docs/a/b/c/guide.md"));
        assert!(!pattern_matches("docs/**/*.md", "docs/readme.txt"));
        assert!(!pattern_matches("docs/**/*.md", "src/readme.md"));
    }

    // ---------------------------------------------------------------------------
    // CATEGORY 10: CASE SENSITIVITY
    // ---------------------------------------------------------------------------

    #[test]
    fn test_case_sensitive_exact() {
        assert!(pattern_matches("/Docs/", "Docs/readme.md"));
        assert!(!pattern_matches("/Docs/", "docs/readme.md"));
        assert!(!pattern_matches("/docs/", "Docs/readme.md"));
    }

    #[test]
    fn test_case_sensitive_extension() {
        assert!(pattern_matches("*.RS", "main.RS"));
        assert!(!pattern_matches("*.RS", "main.rs"));
        assert!(!pattern_matches("*.rs", "main.RS"));
    }

    // ---------------------------------------------------------------------------
    // CATEGORY 11: UNSUPPORTED FEATURES (should be treated literally or ignored)
    // ---------------------------------------------------------------------------
    // Unlike gitignore, CODEOWNERS does NOT support:
    // - `[abc]` character ranges
    // - `!pattern` negation

    #[test]
    fn test_character_range_not_supported() {
        // [abc] should be treated literally, not as character class
        // A file literally named "[abc].txt" would match
        // But "a.txt", "b.txt", "c.txt" should NOT match
        assert!(!pattern_matches("[abc].txt", "a.txt"));
        assert!(!pattern_matches("[abc].txt", "b.txt"));
        assert!(!pattern_matches("[abc].txt", "c.txt"));
        // Note: matching "[abc].txt" literally is tricky due to glob libs
    }

    #[test]
    fn test_negation_not_supported() {
        // !pattern should NOT work as negation
        // It should either fail to match or be treated literally
        // The key is it shouldn't "un-match" files
        // (Hard to test directly, but we verify it doesn't crash or behave unexpectedly)
        assert!(!pattern_matches("!docs/", "docs/readme.md"));
    }

    // ---------------------------------------------------------------------------
    // CATEGORY 12: COMPOSITE TABLE TEST (from GitHub docs)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_github_docs_table() {
        // From the report:
        // | Pattern      | `/docs/readme.md` | `/docs/api/index.md` | `/src/docs/file.txt` |
        // |--------------|-------------------|----------------------|----------------------|
        // | `/docs/`     | ✓                 | ✓                    | ✗                    |
        // | `/docs/*`    | ✓                 | ✗                    | ✗                    |
        // | `/docs/**`   | ✓                 | ✓                    | ✗                    |
        // | `docs/`      | ✓                 | ✓                    | ✓                    |
        // | `**/docs/**` | ✓                 | ✓                    | ✓                    |

        // /docs/ - anchored, recursive
        assert!(pattern_matches("/docs/", "docs/readme.md"));
        assert!(pattern_matches("/docs/", "docs/api/index.md"));
        assert!(!pattern_matches("/docs/", "src/docs/file.txt"));

        // /docs/* - anchored, direct children only
        assert!(pattern_matches("/docs/*", "docs/readme.md"));
        assert!(!pattern_matches("/docs/*", "docs/api/index.md"));
        assert!(!pattern_matches("/docs/*", "src/docs/file.txt"));

        // /docs/** - anchored, recursive via **
        assert!(pattern_matches("/docs/**", "docs/readme.md"));
        assert!(pattern_matches("/docs/**", "docs/api/index.md"));
        assert!(!pattern_matches("/docs/**", "src/docs/file.txt"));

        // docs/ - unanchored, matches anywhere
        assert!(pattern_matches("docs/", "docs/readme.md"));
        assert!(pattern_matches("docs/", "docs/api/index.md"));
        assert!(pattern_matches("docs/", "src/docs/file.txt"));

        // **/docs/** - explicit anywhere match
        assert!(pattern_matches("**/docs/**", "docs/readme.md"));
        assert!(pattern_matches("**/docs/**", "docs/api/index.md"));
        assert!(pattern_matches("**/docs/**", "src/docs/file.txt"));
    }

    // ---------------------------------------------------------------------------
    // CATEGORY 13: EDGE CASES
    // ---------------------------------------------------------------------------

    #[test]
    fn test_root_files_only() {
        // /* should match only files at root, not nested
        assert!(pattern_matches("/*", "readme.md"));
        assert!(pattern_matches("/*", "Makefile"));
        // Should NOT match nested files
        assert!(!pattern_matches("/*", "src/main.rs"));
        assert!(!pattern_matches("/*", "docs/readme.md"));
    }

    #[test]
    fn test_hidden_files() {
        // Patterns should match hidden files (starting with .)
        assert!(pattern_matches("*", ".gitignore"));
        assert!(pattern_matches(".*", ".gitignore"));
        assert!(pattern_matches(".github/", ".github/workflows/ci.yml"));
    }

    #[test]
    fn test_deeply_nested() {
        let deep = "a/b/c/d/e/f/g/h/i/j/k/file.txt";
        assert!(pattern_matches("*", deep));
        assert!(pattern_matches("**", deep));
        assert!(pattern_matches("a/", deep));
        assert!(pattern_matches("a/**", deep));
        assert!(pattern_matches("**/file.txt", deep));
        assert!(!pattern_matches("a/*", deep)); // * doesn't cross dirs
    }

    #[test]
    fn test_empty_path_segments() {
        // Paths shouldn't have empty segments, but verify no panic
        // This is more of a robustness test
        assert!(!pattern_matches("/docs/", ""));
        assert!(!pattern_matches("*", ""));
    }

    // ---------------------------------------------------------------------------
    // SUBSUMPTION TESTS
    // ---------------------------------------------------------------------------
    // pattern_subsumes(a, b) = true if everything a matches, b also matches
    // Used for detecting dead rules in CODEOWNERS

    #[test]
    fn test_subsumes_identical() {
        assert!(pattern_subsumes("*.rs", "*.rs"));
        assert!(pattern_subsumes("/src/", "/src/"));
        assert!(pattern_subsumes("Makefile", "Makefile"));
    }

    #[test]
    fn test_subsumes_catchall() {
        // * and ** subsume everything - this is the "docs/ @baz" followed by "* @gav" case
        // where docs/ becomes a dead rule because * matches everything and comes later
        assert!(pattern_subsumes("*.rs", "*"));
        assert!(pattern_subsumes("*.go", "*"));
        assert!(pattern_subsumes("/src/", "*")); // anchored dir
        assert!(pattern_subsumes("docs/", "*")); // unanchored dir - critical case!
        assert!(pattern_subsumes("src/lib/", "*")); // unanchored nested dir
        assert!(pattern_subsumes("Makefile", "*"));
        assert!(pattern_subsumes("src/main.rs", "*"));
        assert!(pattern_subsumes("*.rs", "**"));
        assert!(pattern_subsumes("/src/lib/", "**"));
        assert!(pattern_subsumes("docs/", "**")); // unanchored dir subsumed by **
    }

    #[test]
    fn test_subsumes_dead_rule_scenarios() {
        // Real-world CODEOWNERS footguns where earlier rules become dead
        //
        // Scenario 1: Catch-all at end shadows everything
        // ```
        // docs/ @docs-team
        // * @default-team    <- docs/ is now dead, @default-team owns docs/
        // ```
        assert!(pattern_subsumes("docs/", "*"));

        // Scenario 2: Specific before general (WRONG order)
        // ```
        // /src/auth/ @security
        // /src/ @backend       <- /src/auth/ is dead, @backend owns it
        // ```
        assert!(pattern_subsumes("/src/auth/", "/src/"));

        // Scenario 3: Extension pattern before catch-all
        // ```
        // *.rs @rust-team
        // * @default          <- *.rs is dead
        // ```
        assert!(pattern_subsumes("*.rs", "*"));

        // Scenario 4: NOT dead - unanchored survives anchored
        // ```
        // docs/ @docs-team    <- still matches src/docs/, lib/docs/, etc.
        // /docs/ @root-docs   <- only matches root docs/
        // ```
        assert!(!pattern_subsumes("docs/", "/docs/"));
    }

    #[test]
    fn test_subsumes_extension() {
        // *.rs.bak subsumed by *.bak
        assert!(pattern_subsumes("*.rs.bak", "*.bak"));
        assert!(!pattern_subsumes("*.bak", "*.rs.bak"));
        assert!(!pattern_subsumes("*.rs", "*.go"));
    }

    #[test]
    fn test_subsumes_directory() {
        // /src/lib/ subsumed by /src/
        assert!(pattern_subsumes("/src/lib/", "/src/"));
        assert!(pattern_subsumes("src/lib/", "src/"));
        assert!(pattern_subsumes("/src/lib/", "/src/**"));
        assert!(!pattern_subsumes("/src/", "/src/lib/"));
    }

    #[test]
    fn test_subsumes_file_in_dir() {
        assert!(pattern_subsumes("src/main.rs", "src/"));
        assert!(pattern_subsumes("src/main.rs", "src/**"));
        assert!(!pattern_subsumes("src/main.rs", "lib/"));
    }

    #[test]
    fn test_not_subsumed() {
        assert!(!pattern_subsumes("*.rs", "*.go"));
        assert!(!pattern_subsumes("/src/", "/lib/"));
        assert!(!pattern_subsumes("*", "*.rs"));
    }

    // ---------------------------------------------------------------------------
    // ANCHORED VS UNANCHORED SUBSUMPTION
    // ---------------------------------------------------------------------------
    // Unanchored patterns match MORE files than anchored patterns.
    // - /docs/ (anchored) IS subsumed by docs/ (unanchored)
    // - docs/ (unanchored) is NOT subsumed by /docs/ (anchored)

    #[test]
    fn test_subsumes_anchored_by_unanchored() {
        // Anchored pattern IS subsumed by unanchored (unanchored matches more)
        assert!(pattern_subsumes("/docs/", "docs/"));
        assert!(pattern_subsumes("/src/lib/", "src/lib/"));
    }

    #[test]
    fn test_subsumes_unanchored_not_by_anchored() {
        // Unanchored pattern is NOT subsumed by anchored (unanchored matches nested paths)
        assert!(!pattern_subsumes("docs/", "/docs/"));
        assert!(!pattern_subsumes("src/", "/src/"));
    }

    #[test]
    fn test_subsumes_both_anchored() {
        // Both anchored - normal subsumption rules apply
        assert!(pattern_subsumes("/src/lib/", "/src/"));
        assert!(!pattern_subsumes("/src/", "/src/lib/"));
    }

    #[test]
    fn test_subsumes_both_unanchored() {
        // Both unanchored - normal subsumption rules apply
        assert!(pattern_subsumes("src/lib/", "src/"));
        assert!(!pattern_subsumes("src/", "src/lib/"));
    }

    // ---------------------------------------------------------------------------
    // COMPILED PATTERN TESTS
    // ---------------------------------------------------------------------------
    // Tests specifically for the CompiledPattern struct

    #[test]
    fn test_compiled_pattern_match_all() {
        let p = CompiledPattern::new("*");
        assert!(p.matches("anything.txt"));
        assert!(p.matches("a/b/c.txt"));

        let p2 = CompiledPattern::new("**");
        assert!(p2.matches("anything.txt"));
        assert!(p2.matches("a/b/c.txt"));
    }

    #[test]
    fn test_compiled_pattern_root_files_only() {
        let p = CompiledPattern::new("/*");
        assert!(p.matches("readme.md"));
        assert!(p.matches("Makefile"));
        assert!(!p.matches("src/main.rs"));
        assert!(!p.matches("a/b/c.txt"));
    }

    #[test]
    fn test_compiled_pattern_single_segment_glob() {
        let p = CompiledPattern::new("*.rs");
        assert!(p.matches("main.rs"));
        assert!(p.matches("src/main.rs"));
        assert!(p.matches("a/b/c/mod.rs"));
        assert!(!p.matches("main.go"));
    }

    #[test]
    fn test_compiled_pattern_multi_segment_glob() {
        let p = CompiledPattern::new("src/**/*.rs");
        assert!(p.matches("src/main.rs"));
        assert!(p.matches("src/lib/mod.rs"));
        assert!(!p.matches("lib/main.rs"));

        let p2 = CompiledPattern::new("/docs/*.md");
        assert!(p2.matches("docs/readme.md"));
        assert!(!p2.matches("docs/api/readme.md"));
    }

    #[test]
    fn test_compiled_pattern_anchored_directory() {
        let p = CompiledPattern::new("/src/");
        assert!(p.matches("src/main.rs"));
        assert!(p.matches("src/lib/mod.rs"));
        assert!(!p.matches("other/src/file.rs"));
    }

    #[test]
    fn test_compiled_pattern_unanchored_directory() {
        let p = CompiledPattern::new("docs/");
        assert!(p.matches("docs/readme.md"));
        assert!(p.matches("src/docs/file.txt"));
        assert!(p.matches("a/b/docs/deep.txt"));
        assert!(!p.matches("documentation/readme.md"));
    }

    #[test]
    fn test_compiled_pattern_exact() {
        let p = CompiledPattern::new("/Makefile");
        assert!(p.matches("Makefile"));
        assert!(!p.matches("build/Makefile"));

        let p2 = CompiledPattern::new("/src");
        assert!(p2.matches("src"));
        assert!(p2.matches("src/main.rs"));
        assert!(!p2.matches("other/src/file.rs"));
    }

    #[test]
    fn test_compiled_pattern_empty_path() {
        // Empty path should never match anything
        let p = CompiledPattern::new("*");
        assert!(!p.matches(""));

        let p2 = CompiledPattern::new("/src/");
        assert!(!p2.matches(""));
    }

    #[test]
    fn test_compiled_pattern_unanchored_dir_exact_match() {
        // Unanchored dir pattern matching exact dir name
        let p = CompiledPattern::new("docs/");
        // Test where path exactly equals the dir (edge case)
        assert!(p.matches("docs/file.txt"));
    }

    #[test]
    fn test_compiled_pattern_unanchored_dir_slash_search() {
        // Test the /dir/ search path in UnanchoredDirectory
        let p = CompiledPattern::new("lib/");
        assert!(p.matches("src/lib/file.rs"));
        assert!(p.matches("a/b/lib/c/d.rs"));
        assert!(!p.matches("library/file.rs"));
    }

    // ---------------------------------------------------------------------------
    // ADDITIONAL EDGE CASES
    // ---------------------------------------------------------------------------

    #[test]
    fn test_subsumes_exact_paths_with_slash() {
        // Exact paths containing / are implicitly anchored
        // So /src/main.rs and src/main.rs are equivalent
        assert!(pattern_subsumes("src/main.rs", "/src/main.rs"));
        assert!(pattern_subsumes("/src/main.rs", "src/main.rs"));
    }

    #[test]
    fn test_subsumes_unanchored_file_not_by_anchored_dir() {
        // An unanchored file path shouldn't be subsumed by an anchored directory
        // because the unanchored file might be somewhere else
        // (This tests the !a_anchored && b_anchored case in subsumes)
        // Note: "lib/foo.rs" contains / so is implicitly anchored
        // For truly unanchored file, use single segment
        assert!(!pattern_subsumes("foo.rs", "/src/")); // foo.rs is single segment, implicitly anchored
    }

    #[test]
    fn test_compiled_anchored_star_vs_unanchored() {
        // //* (anchored *) should be RootFilesOnly
        // /* matches only root level files
        let p = CompiledPattern::new("/*");
        assert!(matches!(p, CompiledPattern::RootFilesOnly));

        // Unanchored * is MatchAll
        let p2 = CompiledPattern::new("*");
        assert!(matches!(p2, CompiledPattern::MatchAll));
    }

    #[test]
    fn test_anchored_dir_exact_length_match() {
        // Test the exact length boundary in AnchoredDirectory
        let p = CompiledPattern::new("/src/");

        // Path exactly equal to dir (edge case) - matches because it's the dir itself
        assert!(p.matches("src")); // Directory pattern matches the directory itself

        // With content after
        assert!(p.matches("src/file.rs"));

        // But not partial prefix
        assert!(!p.matches("srcfile.rs")); // "srcfile" != "src/"
    }

    #[test]
    fn test_compiled_unanchored_dir_nested_match() {
        // Test the loop in UnanchoredDirectory that checks /dir at various positions (lines 86-89)
        let p = CompiledPattern::new("lib/");

        // Should match lib at various depths
        assert!(p.matches("lib/file.rs")); // root
        assert!(p.matches("src/lib/file.rs")); // one level deep
        assert!(p.matches("a/b/lib/c.rs")); // two levels deep
        assert!(p.matches("x/y/z/lib/deep.txt")); // three levels deep

        // Should not match partial names
        assert!(!p.matches("libfoo/file.rs"));
        assert!(!p.matches("src/libfoo/file.rs"));
    }

    #[test]
    fn test_pattern_matches_unanchored_dir_loop() {
        // Test the loop in pattern_matches for unanchored directories (lines 177-182)
        // This covers the second check branch in the loop
        assert!(pattern_matches("docs/", "project/docs/readme.md"));
        assert!(pattern_matches("docs/", "a/b/docs/file.txt"));
        assert!(pattern_matches("docs/", "x/docs/y/z.md"));
    }

    #[test]
    fn test_subsumes_extension_not_by_non_extension() {
        // Test line 234 - *.rs not subsumed by a non-* pattern
        assert!(!pattern_subsumes("*.rs", "src/"));
        assert!(!pattern_subsumes("*.rs", "/src/main.rs"));
        assert!(!pattern_subsumes("*.js", "docs/"));
    }

    #[test]
    fn test_compiled_exact_as_directory_prefix() {
        // Test Exact pattern matching as directory prefix
        let p = CompiledPattern::new("/build");

        assert!(p.matches("build")); // exact
        assert!(p.matches("build/output")); // as prefix
        assert!(p.matches("build/a/b/c")); // deep prefix
        assert!(!p.matches("builder")); // not prefix with /
        assert!(!p.matches("src/build")); // nested - anchored
    }
}
