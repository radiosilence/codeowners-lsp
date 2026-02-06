use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

use rayon::prelude::*;

use crate::parser::{CodeownersLine, ParsedLine};
use crate::pattern::{pattern_matches, CompiledPattern};

/// Check if characters in needle appear in order in haystack (fuzzy match)
fn fuzzy_match(haystack: &str, needle: &str) -> bool {
    let mut needle_chars = needle.chars().peekable();

    for c in haystack.chars() {
        if needle_chars.peek() == Some(&c) {
            needle_chars.next();
        }
        if needle_chars.peek().is_none() {
            return true;
        }
    }

    needle_chars.peek().is_none()
}

/// Cached list of files in the workspace with pattern match caching
pub struct FileCache {
    files: Vec<String>,
    /// Cache of pattern -> match count (lazily populated)
    count_cache: RwLock<HashMap<String, usize>>,
    /// Cache of pattern -> has_match (lazily populated)
    has_match_cache: RwLock<HashSet<String>>,
}

impl FileCache {
    /// Create a new FileCache using git ls-files to get tracked files
    pub fn new(root: &PathBuf) -> Self {
        let files = Command::new("git")
            .args(["ls-files", "--cached", "--others", "--exclude-standard"])
            .current_dir(root)
            .output()
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        Self {
            files,
            count_cache: RwLock::new(HashMap::new()),
            has_match_cache: RwLock::new(HashSet::new()),
        }
    }

    /// Count files matching a pattern (blocking, computes and caches)
    /// For CLI and sync contexts
    #[allow(dead_code)] // Used by CLI, not LSP
    pub fn count_matches(&self, pattern: &str) -> usize {
        // Check cache first
        {
            let cache = self.count_cache.read().unwrap();
            if let Some(&count) = cache.get(pattern) {
                return count;
            }
        }

        // Compute and cache
        let count = self
            .files
            .iter()
            .filter(|f| pattern_matches(pattern, f))
            .count();

        self.count_cache
            .write()
            .unwrap()
            .insert(pattern.to_string(), count);
        count
    }

    /// Count files matching a pattern (non-blocking, returns None if not cached)
    /// For LSP inlay hints - doesn't block if count not yet computed
    #[allow(dead_code)] // Used by LSP, not CLI
    pub fn count_matches_cached(&self, pattern: &str) -> Option<usize> {
        let cache = self.count_cache.read().unwrap();
        cache.get(pattern).copied()
    }

    /// Check if a pattern has any matches (cached, faster than count_matches for existence check)
    #[allow(dead_code)] // Used internally by find_patterns_with_matches
    pub fn has_matches(&self, pattern: &str) -> bool {
        // Check cache first
        {
            let cache = self.has_match_cache.read().unwrap();
            if cache.contains(pattern) {
                return true;
            }
        }

        // Also check count cache (if we already counted, use that)
        {
            let cache = self.count_cache.read().unwrap();
            if let Some(&count) = cache.get(pattern) {
                return count > 0;
            }
        }

        // Compute (early exit on first match)
        let has_match = self.files.iter().any(|f| pattern_matches(pattern, f));

        if has_match {
            self.has_match_cache
                .write()
                .unwrap()
                .insert(pattern.to_string());
        }
        has_match
    }

    /// Find which patterns have at least one matching file.
    /// Returns a set of pattern indices that have matches.
    /// Uses caching for previously checked patterns.
    pub fn find_patterns_with_matches(&self, patterns: &[&str]) -> HashSet<usize> {
        let mut result = HashSet::new();
        let mut uncached_patterns: Vec<(usize, &str)> = Vec::new();

        // First pass: check caches
        {
            let has_match_cache = self.has_match_cache.read().unwrap();
            let count_cache = self.count_cache.read().unwrap();

            for (i, pattern) in patterns.iter().enumerate() {
                if has_match_cache.contains(*pattern) {
                    result.insert(i);
                } else if let Some(&count) = count_cache.get(*pattern) {
                    if count > 0 {
                        result.insert(i);
                    }
                    // If count is 0, we know it has no matches - skip
                } else {
                    uncached_patterns.push((i, pattern));
                }
            }
        }

        // If all patterns were cached, return early
        if uncached_patterns.is_empty() {
            return result;
        }

        // Second pass: compute uncached patterns in parallel
        let compiled: Vec<(usize, CompiledPattern)> = uncached_patterns
            .iter()
            .map(|(i, p)| (*i, CompiledPattern::new(p)))
            .collect();

        let flags: Vec<AtomicBool> = (0..compiled.len())
            .map(|_| AtomicBool::new(false))
            .collect();

        self.files.par_iter().for_each(|file| {
            for (idx, (_, pattern)) in compiled.iter().enumerate() {
                if flags[idx].load(Ordering::Relaxed) {
                    continue;
                }
                if pattern.matches(file) {
                    flags[idx].store(true, Ordering::Relaxed);
                }
            }
        });

        // Update caches and result
        let mut has_match_cache = self.has_match_cache.write().unwrap();
        for (idx, (orig_idx, pattern)) in uncached_patterns.iter().enumerate() {
            if flags[idx].load(Ordering::Relaxed) {
                result.insert(*orig_idx);
                has_match_cache.insert(pattern.to_string());
            }
        }

        result
    }

    /// Get files matching a pattern
    #[allow(dead_code)]
    pub fn get_matches(&self, pattern: &str) -> Vec<&String> {
        self.files
            .iter()
            .filter(|f| pattern_matches(pattern, f))
            .collect()
    }

    /// Get all files for completions
    #[allow(dead_code)]
    pub fn all_files(&self) -> &[String] {
        &self.files
    }

    /// Get files matching a query for path completion (fzf-style)
    ///
    /// Matches files where the query appears anywhere in the path.
    /// - "main" matches "src/main.rs"
    /// - "src/main" matches "src/main.rs"
    /// - "s/m" fuzzy matches "src/main.rs"
    #[allow(dead_code)] // Used by LSP, not CLI
    pub fn complete_path(&self, query: &str) -> Vec<String> {
        let has_leading_slash = query.starts_with('/');
        let normalized = query
            .trim_start_matches('/')
            .trim_start_matches("./")
            .to_lowercase();

        if normalized.is_empty() {
            // Empty query - return top-level items only
            let mut dirs = HashSet::new();
            let mut files = Vec::new();

            for file in &self.files {
                if let Some(slash_pos) = file.find('/') {
                    dirs.insert(format!("{}/", &file[..slash_pos]));
                } else {
                    files.push(file.clone());
                }
            }

            let format_path = |path: String| -> String {
                if has_leading_slash {
                    format!("/{}", path)
                } else {
                    path
                }
            };

            let mut completions: Vec<String> = dirs.into_iter().map(&format_path).collect();
            completions.sort();
            let mut file_completions: Vec<String> = files.into_iter().map(format_path).collect();
            file_completions.sort();
            completions.extend(file_completions);
            completions.truncate(50);
            return completions;
        }

        // Score and collect matching files
        let mut matches: Vec<(String, i32)> = Vec::new();

        for file in &self.files {
            let file_lower = file.to_lowercase();

            // Calculate match score (higher = better)
            let score = if file_lower.starts_with(&normalized) {
                100 // Exact prefix match
            } else if file_lower.contains(&normalized) {
                50 // Substring match
            } else if fuzzy_match(&file_lower, &normalized) {
                25 // Fuzzy match (characters in order)
            } else {
                continue; // No match
            };

            // Bonus for shorter paths (more specific)
            let length_bonus = 20 - (file.len() as i32).min(20);

            matches.push((file.clone(), score + length_bonus));
        }

        // Sort by score (descending), then alphabetically
        matches.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        // Format output
        matches
            .into_iter()
            .take(50)
            .map(|(path, _)| {
                if has_leading_slash {
                    format!("/{}", path)
                } else {
                    path
                }
            })
            .collect()
    }

    /// Get files with no owners according to the given rules
    #[allow(dead_code)] // Used by CLI binary
    pub fn get_unowned_files(&self, rules: &[ParsedLine]) -> Vec<&String> {
        // Extract and compile patterns once
        let compiled: Vec<CompiledPattern> = rules
            .iter()
            .filter_map(|rule| {
                if let CodeownersLine::Rule { pattern, .. } = &rule.content {
                    Some(CompiledPattern::new(pattern))
                } else {
                    None
                }
            })
            .collect();

        // Check files in parallel
        self.files
            .par_iter()
            .filter(|file| !compiled.iter().any(|pattern| pattern.matches(file)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::process::Command;
    use tempfile::tempdir;

    fn create_test_repo(dir: &std::path::Path) {
        // Init git repo
        Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .output()
            .unwrap();

        // Create files
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("docs")).unwrap();
        File::create(dir.join("src/main.rs")).unwrap();
        File::create(dir.join("src/lib.rs")).unwrap();
        File::create(dir.join("docs/readme.md")).unwrap();
        File::create(dir.join("Cargo.toml")).unwrap();

        // Add to git
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    #[test]
    fn test_file_cache_creation() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());
        assert_eq!(cache.files.len(), 4);
    }

    #[test]
    fn test_count_matches() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());
        assert_eq!(cache.count_matches("*.rs"), 2);
        assert_eq!(cache.count_matches("*.md"), 1);
        assert_eq!(cache.count_matches("src/**"), 2);
        assert_eq!(cache.count_matches("*"), 4);
    }

    #[test]
    fn test_get_unowned_files() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());

        // Rule that covers only Rust files
        let rules = vec![ParsedLine {
            line_number: 0,
            content: CodeownersLine::Rule {
                pattern: "*.rs".to_string(),
                owners: vec!["@owner".to_string()],
            },
            pattern_start: 0,
            pattern_end: 4,
            owners_start: 5,
            comment_start: None,
        }];

        let unowned = cache.get_unowned_files(&rules);
        assert_eq!(unowned.len(), 2); // docs/readme.md and Cargo.toml
    }

    #[test]
    fn test_all_files_owned() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());

        // Catch-all rule
        let rules = vec![ParsedLine {
            line_number: 0,
            content: CodeownersLine::Rule {
                pattern: "*".to_string(),
                owners: vec!["@owner".to_string()],
            },
            pattern_start: 0,
            pattern_end: 1,
            owners_start: 2,
            comment_start: None,
        }];

        let unowned = cache.get_unowned_files(&rules);
        assert!(unowned.is_empty());
    }

    #[test]
    fn test_fuzzy_match_basic() {
        assert!(fuzzy_match("src/main.rs", "src"));
        assert!(fuzzy_match("src/main.rs", "main"));
        assert!(fuzzy_match("src/main.rs", "s/m"));
        assert!(fuzzy_match("src/main.rs", "smr")); // s...m...r
        assert!(!fuzzy_match("src/main.rs", "xyz"));
        assert!(!fuzzy_match("src/main.rs", "zyx")); // none exist
                                                     // Note: "rms" DOES match: s[r]c/[m]ain.r[s] - chars appear in order
        assert!(fuzzy_match("src/main.rs", "rms"));
    }

    #[test]
    fn test_fuzzy_match_empty() {
        assert!(fuzzy_match("anything", ""));
        assert!(fuzzy_match("", ""));
        assert!(!fuzzy_match("", "a"));
    }

    #[test]
    fn test_count_matches_caching() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());

        // First call computes and caches
        assert_eq!(cache.count_matches("*.rs"), 2);

        // Verify it's cached
        assert_eq!(cache.count_matches_cached("*.rs"), Some(2));

        // Non-cached pattern returns None
        assert_eq!(cache.count_matches_cached("*.md"), None);

        // Now cache it
        assert_eq!(cache.count_matches("*.md"), 1);
        assert_eq!(cache.count_matches_cached("*.md"), Some(1));
    }

    #[test]
    fn test_has_matches() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());

        assert!(cache.has_matches("*.rs"));
        assert!(cache.has_matches("*.md"));
        assert!(!cache.has_matches("*.xyz"));
        assert!(cache.has_matches("src/**"));
    }

    #[test]
    fn test_has_matches_uses_count_cache() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());

        // Pre-populate count cache
        cache.count_matches("*.rs");

        // has_matches should use the count cache
        assert!(cache.has_matches("*.rs"));
    }

    #[test]
    fn test_find_patterns_with_matches() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());
        let patterns = vec!["*.rs", "*.md", "*.xyz", "src/**"];
        let result = cache.find_patterns_with_matches(&patterns);

        assert!(result.contains(&0)); // *.rs
        assert!(result.contains(&1)); // *.md
        assert!(!result.contains(&2)); // *.xyz - no match
        assert!(result.contains(&3)); // src/**
    }

    #[test]
    fn test_find_patterns_with_matches_uses_cache() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());

        // Pre-cache one pattern
        cache.has_matches("*.rs");

        let patterns = vec!["*.rs", "*.md"];
        let result = cache.find_patterns_with_matches(&patterns);

        assert!(result.contains(&0));
        assert!(result.contains(&1));
    }

    #[test]
    fn test_get_matches() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());
        let matches = cache.get_matches("*.rs");

        assert_eq!(matches.len(), 2);
        assert!(matches.iter().any(|f| f.ends_with("main.rs")));
        assert!(matches.iter().any(|f| f.ends_with("lib.rs")));
    }

    #[test]
    fn test_all_files() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());
        let files = cache.all_files();

        assert_eq!(files.len(), 4);
    }

    #[test]
    fn test_complete_path_empty_query() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());
        let completions = cache.complete_path("");

        // Should return top-level items
        assert!(completions.contains(&"src/".to_string()));
        assert!(completions.contains(&"docs/".to_string()));
        assert!(completions.contains(&"Cargo.toml".to_string()));
    }

    #[test]
    fn test_complete_path_with_leading_slash() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());
        let completions = cache.complete_path("/");

        // Should preserve leading slash
        assert!(completions.iter().all(|c| c.starts_with('/')));
    }

    #[test]
    fn test_complete_path_prefix_match() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());
        let completions = cache.complete_path("src");

        // Should match files in src/
        assert!(completions.iter().any(|c| c.contains("main.rs")));
        assert!(completions.iter().any(|c| c.contains("lib.rs")));
    }

    #[test]
    fn test_complete_path_substring_match() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());
        let completions = cache.complete_path("main");

        assert!(completions.iter().any(|c| c.contains("main.rs")));
    }

    #[test]
    fn test_complete_path_fuzzy_match() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());
        let completions = cache.complete_path("smr"); // s...m...r

        // Should fuzzy match src/main.rs
        assert!(completions.iter().any(|c| c.contains("main.rs")));
    }

    #[test]
    fn test_complete_path_strips_dot_slash() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());
        let completions = cache.complete_path("./src");

        assert!(completions.iter().any(|c| c.contains("main.rs")));
    }

    #[test]
    fn test_unowned_with_comment_rules() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());

        // Mix of comments and rules
        let rules = vec![
            ParsedLine {
                line_number: 0,
                content: CodeownersLine::Comment("# Section".to_string()),
                pattern_start: 0,
                pattern_end: 0,
                owners_start: 0,
                comment_start: None,
            },
            ParsedLine {
                line_number: 1,
                content: CodeownersLine::Rule {
                    pattern: "*.rs".to_string(),
                    owners: vec!["@rust-team".to_string()],
                },
                pattern_start: 0,
                pattern_end: 4,
                owners_start: 5,
                comment_start: None,
            },
            ParsedLine {
                line_number: 2,
                content: CodeownersLine::Empty,
                pattern_start: 0,
                pattern_end: 0,
                owners_start: 0,
                comment_start: None,
            },
        ];

        let unowned = cache.get_unowned_files(&rules);
        // docs/readme.md and Cargo.toml are unowned
        assert_eq!(unowned.len(), 2);
    }

    #[test]
    fn test_empty_repo() {
        let dir = tempdir().unwrap();

        // Init git repo without any files
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        let cache = FileCache::new(&dir.path().to_path_buf());
        assert!(cache.files.is_empty());
        assert_eq!(cache.count_matches("*"), 0);
        assert!(!cache.has_matches("*.rs"));
    }

    #[test]
    fn test_non_git_directory() {
        let dir = tempdir().unwrap();
        // No git init - just a regular directory

        let cache = FileCache::new(&dir.path().to_path_buf());
        assert!(cache.files.is_empty());
    }

    #[test]
    fn test_count_matches_cache_hit() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());

        // First call populates cache
        let count1 = cache.count_matches("*.rs");
        // Second call should hit cache (line 67)
        let count2 = cache.count_matches("*.rs");

        assert_eq!(count1, count2);
        assert_eq!(count1, 2);
    }

    #[test]
    fn test_has_matches_count_cache_zero() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());

        // Pre-populate count cache with 0 for a pattern that doesn't match
        cache.count_matches("*.xyz");

        // has_matches should use count cache and return false (line 100)
        assert!(!cache.has_matches("*.xyz"));
    }

    #[test]
    fn test_find_patterns_all_cached() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());

        // Pre-cache all patterns
        cache.has_matches("*.rs");
        cache.has_matches("*.md");

        // Now call find_patterns_with_matches - should hit early return (lines 140-141)
        let patterns = vec!["*.rs", "*.md"];
        let result = cache.find_patterns_with_matches(&patterns);

        assert!(result.contains(&0));
        assert!(result.contains(&1));
    }

    #[test]
    fn test_find_patterns_count_cache_zero_skip() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());

        // Pre-populate count cache with 0 (no matches)
        cache.count_matches("*.xyz");

        // Pattern should be skipped, not added to result
        let patterns = vec!["*.xyz", "*.rs"];
        let result = cache.find_patterns_with_matches(&patterns);

        assert!(!result.contains(&0)); // *.xyz has 0 matches
        assert!(result.contains(&1)); // *.rs has matches
    }

    #[test]
    fn test_complete_path_with_query_and_leading_slash() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());

        // Query with leading slash and actual content (line 279)
        let completions = cache.complete_path("/src");

        // Results should have leading slash preserved
        assert!(completions.iter().all(|c| c.starts_with('/')));
        assert!(completions.iter().any(|c| c.contains("main.rs")));
    }

    #[test]
    fn test_find_patterns_count_cache_positive_hit() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());

        // Pre-populate count cache with positive count (line 141)
        cache.count_matches("*.rs"); // This will cache count=2

        // Now find_patterns_with_matches should use the count cache
        let patterns = vec!["*.rs"];
        let result = cache.find_patterns_with_matches(&patterns);

        // Should use the count cache (count > 0) and add to result
        assert!(result.contains(&0));
    }

    #[test]
    fn test_has_matches_uses_has_match_cache() {
        let dir = tempdir().unwrap();
        create_test_repo(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());

        // First call to has_matches populates has_match_cache
        assert!(cache.has_matches("*.rs"));

        // Second call should hit the has_match_cache (line 100)
        assert!(cache.has_matches("*.rs"));
    }
}
