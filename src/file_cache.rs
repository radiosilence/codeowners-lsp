use std::path::PathBuf;

use ignore::WalkBuilder;

use crate::parser::{CodeownersLine, ParsedLine};
use crate::pattern::pattern_matches;

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

    /// Get files matching a pattern
    #[allow(dead_code)]
    pub fn get_matches(&self, pattern: &str) -> Vec<&String> {
        self.files
            .iter()
            .filter(|f| pattern_matches(pattern, f))
            .collect()
    }

    /// Get files with no owners according to the given rules
    pub fn get_unowned_files(&self, rules: &[ParsedLine]) -> Vec<&String> {
        self.files
            .iter()
            .filter(|file| {
                !rules.iter().any(|rule| {
                    if let CodeownersLine::Rule { pattern, .. } = &rule.content {
                        pattern_matches(pattern, file)
                    } else {
                        false
                    }
                })
            })
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
