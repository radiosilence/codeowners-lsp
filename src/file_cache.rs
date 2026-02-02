use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

use ignore::WalkBuilder;
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

/// Files/patterns to ignore internally (LSP's own config, not subject to CODEOWNERS rules)
pub const INTERNAL_IGNORE: &[&str] = &[
    ".codeowners-lsp.toml",
    ".codeowners-lsp.local.toml",
    ".codeowners-lsp/",
];

/// Check if a path should be internally ignored
pub fn is_internally_ignored(path: &str) -> bool {
    for pattern in INTERNAL_IGNORE {
        if pattern.ends_with('/') {
            // Directory pattern
            let dir_name = pattern.trim_end_matches('/');
            if path.starts_with(&format!("{}/", dir_name))
                || path.contains(&format!("/{}/", dir_name))
            {
                return true;
            }
        } else {
            // File pattern
            if path == *pattern || path.ends_with(&format!("/{}", pattern)) {
                return true;
            }
        }
    }
    false
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
    pub fn new(root: &PathBuf) -> Self {
        let mut files = Vec::new();

        let walker = WalkBuilder::new(root)
            .hidden(false) // Include dotfiles like .github/
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                // Exclude .git directory and LSP internal directories
                name != ".git" && name != ".codeowners-lsp"
            })
            .build();

        for entry in walker.flatten() {
            if entry.file_type().is_some_and(|ft| ft.is_file()) {
                if let Ok(relative) = entry.path().strip_prefix(root) {
                    let path = relative.to_string_lossy().to_string();
                    // Skip internally ignored files
                    if !is_internally_ignored(&path) {
                        files.push(path);
                    }
                }
            }
        }

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
    use tempfile::tempdir;

    fn create_test_files(dir: &std::path::Path) {
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("docs")).unwrap();
        File::create(dir.join("src/main.rs")).unwrap();
        File::create(dir.join("src/lib.rs")).unwrap();
        File::create(dir.join("docs/readme.md")).unwrap();
        File::create(dir.join("Cargo.toml")).unwrap();
    }

    #[test]
    fn test_file_cache_creation() {
        let dir = tempdir().unwrap();
        create_test_files(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());
        assert_eq!(cache.files.len(), 4);
    }

    #[test]
    fn test_count_matches() {
        let dir = tempdir().unwrap();
        create_test_files(dir.path());

        let cache = FileCache::new(&dir.path().to_path_buf());
        assert_eq!(cache.count_matches("*.rs"), 2);
        assert_eq!(cache.count_matches("*.md"), 1);
        assert_eq!(cache.count_matches("src/**"), 2);
        assert_eq!(cache.count_matches("*"), 4);
    }

    #[test]
    fn test_get_unowned_files() {
        let dir = tempdir().unwrap();
        create_test_files(dir.path());

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
        }];

        let unowned = cache.get_unowned_files(&rules);
        assert_eq!(unowned.len(), 2); // docs/readme.md and Cargo.toml
    }

    #[test]
    fn test_all_files_owned() {
        let dir = tempdir().unwrap();
        create_test_files(dir.path());

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
        }];

        let unowned = cache.get_unowned_files(&rules);
        assert!(unowned.is_empty());
    }
}
