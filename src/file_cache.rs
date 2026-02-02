use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use ignore::WalkBuilder;
use rayon::prelude::*;

use crate::parser::{CodeownersLine, ParsedLine};
use crate::pattern::{pattern_matches, CompiledPattern};

/// Cached list of files in the workspace
pub struct FileCache {
    files: Vec<String>,
}

impl FileCache {
    pub fn new(root: &PathBuf) -> Self {
        let mut files = Vec::new();

        let walker = WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build();

        for entry in walker.flatten() {
            if entry.file_type().is_some_and(|ft| ft.is_file()) {
                if let Ok(relative) = entry.path().strip_prefix(root) {
                    files.push(relative.to_string_lossy().to_string());
                }
            }
        }

        Self { files }
    }

    /// Count files matching a pattern
    pub fn count_matches(&self, pattern: &str) -> usize {
        self.files
            .iter()
            .filter(|f| pattern_matches(pattern, f))
            .count()
    }

    /// Find which patterns have at least one matching file.
    /// Returns a set of pattern indices that have matches.
    /// Uses parallel iteration for performance on large repos.
    pub fn find_patterns_with_matches(&self, patterns: &[&str]) -> HashSet<usize> {
        // Pre-compile all patterns once (avoids allocations per match)
        let compiled: Vec<CompiledPattern> =
            patterns.iter().map(|p| CompiledPattern::new(p)).collect();

        // Create atomic flags for each pattern (true = has match)
        let flags: Vec<AtomicBool> = (0..patterns.len())
            .map(|_| AtomicBool::new(false))
            .collect();

        // Process files in parallel
        self.files.par_iter().for_each(|file| {
            for (i, pattern) in compiled.iter().enumerate() {
                // Skip if already matched (relaxed is fine, just an optimization)
                if flags[i].load(Ordering::Relaxed) {
                    continue;
                }
                if pattern.matches(file) {
                    flags[i].store(true, Ordering::Relaxed);
                }
            }
        });

        // Collect results
        flags
            .iter()
            .enumerate()
            .filter(|(_, f)| f.load(Ordering::Relaxed))
            .map(|(i, _)| i)
            .collect()
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

    /// Get files/dirs matching a prefix for path completion
    #[allow(dead_code)] // Used by LSP, not CLI
    pub fn complete_path(&self, prefix: &str) -> Vec<String> {
        let prefix = prefix.trim_start_matches('/');
        let mut seen = std::collections::HashSet::new();
        let mut completions = Vec::new();

        for file in &self.files {
            if let Some(remainder) = file.strip_prefix(prefix) {
                // Add the file itself
                completions.push(format!("/{}", file));
                seen.insert(format!("/{}", file));

                // Also add parent directories
                if let Some(slash_pos) = remainder.find('/') {
                    let dir = format!("/{}{}/", prefix, &remainder[..slash_pos]);
                    if seen.insert(dir.clone()) {
                        completions.push(dir);
                    }
                }
            }
        }

        completions.sort();
        completions.truncate(50); // Limit completions
        completions
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
