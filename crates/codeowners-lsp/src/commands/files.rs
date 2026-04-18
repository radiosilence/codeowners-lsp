//! File collection utilities for CLI commands that accept file lists.

use std::collections::HashSet;
use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;

/// Collect files from various sources (--files, --files-from, --stdin)
/// Returns Ok(None) if no filtering requested, Ok(Some(set)) if files specified,
/// Err if --files-from path doesn't exist
pub fn collect_files(
    files: Option<Vec<String>>,
    files_from: Option<PathBuf>,
    stdin: bool,
) -> Result<Option<HashSet<String>>, String> {
    let mut result = HashSet::new();

    // From --files argument
    if let Some(f) = files {
        for file in f {
            result.insert(file);
        }
    }

    // From --files-from file
    if let Some(path) = files_from {
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read '{}': {}", path.display(), e))?;
        for line in content.lines() {
            let line = line.trim();
            if !line.is_empty() {
                result.insert(line.to_string());
            }
        }
    }

    // From stdin
    if stdin {
        let stdin_handle = io::stdin();
        for line in stdin_handle.lock().lines().map_while(Result::ok) {
            let line = line.trim();
            if !line.is_empty() {
                result.insert(line.to_string());
            }
        }
    }

    if result.is_empty() {
        Ok(None) // No file filtering
    } else {
        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_collect_files_none_when_empty() {
        let result = collect_files(None, None, false).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_collect_files_from_args() {
        let files = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];
        let result = collect_files(Some(files), None, false).unwrap();
        assert!(result.is_some());
        let set = result.unwrap();
        assert_eq!(set.len(), 2);
        assert!(set.contains("src/main.rs"));
        assert!(set.contains("src/lib.rs"));
    }

    #[test]
    fn test_collect_files_from_file() {
        let mut temp = NamedTempFile::new().unwrap();
        writeln!(temp, "src/foo.rs").unwrap();
        writeln!(temp, "src/bar.rs").unwrap();
        writeln!(temp, "  src/baz.rs  ").unwrap(); // with whitespace
        writeln!(temp).unwrap(); // empty line
        temp.flush().unwrap();

        let result = collect_files(None, Some(temp.path().to_path_buf()), false).unwrap();
        assert!(result.is_some());
        let set = result.unwrap();
        assert_eq!(set.len(), 3);
        assert!(set.contains("src/foo.rs"));
        assert!(set.contains("src/bar.rs"));
        assert!(set.contains("src/baz.rs"));
    }

    #[test]
    fn test_collect_files_from_nonexistent_file() {
        let result = collect_files(None, Some(PathBuf::from("/nonexistent/path.txt")), false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to read"));
    }

    #[test]
    fn test_collect_files_combined() {
        let mut temp = NamedTempFile::new().unwrap();
        writeln!(temp, "from_file.rs").unwrap();
        temp.flush().unwrap();

        let files = vec!["from_args.rs".to_string()];
        let result = collect_files(Some(files), Some(temp.path().to_path_buf()), false).unwrap();
        assert!(result.is_some());
        let set = result.unwrap();
        assert_eq!(set.len(), 2);
        assert!(set.contains("from_args.rs"));
        assert!(set.contains("from_file.rs"));
    }

    #[test]
    fn test_collect_files_dedupes() {
        let files = vec![
            "same.rs".to_string(),
            "same.rs".to_string(),
            "different.rs".to_string(),
        ];
        let result = collect_files(Some(files), None, false).unwrap();
        assert!(result.is_some());
        let set = result.unwrap();
        assert_eq!(set.len(), 2);
    }
}
