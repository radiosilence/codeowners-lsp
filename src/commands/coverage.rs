use std::collections::HashSet;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::process::ExitCode;
use std::{env, fs};

use colored::Colorize;

use crate::file_cache::FileCache;
use crate::ownership::{find_codeowners, get_repo_root};
use crate::parser;

/// Read files from various sources
/// Returns Ok(None) if no filtering requested, Ok(Some(set)) if files specified,
/// Err if --files-from path doesn't exist
pub fn collect_files_to_check(
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

pub fn coverage(files: Option<Vec<String>>, files_from: Option<PathBuf>, stdin: bool) -> ExitCode {
    let cwd = env::current_dir().expect("Failed to get current directory");

    let codeowners_path = match find_codeowners(&cwd) {
        Some(p) => p,
        None => {
            eprintln!("No CODEOWNERS file found");
            return ExitCode::from(1);
        }
    };

    let content = match fs::read_to_string(&codeowners_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to read {}: {}", codeowners_path.display(), e);
            return ExitCode::from(1);
        }
    };

    let repo_root = get_repo_root(&codeowners_path, &cwd);
    let file_cache = FileCache::new(&repo_root);
    let lines = parser::parse_codeowners_file_with_positions(&content);

    // Collect files to check (if specified)
    let files_to_check = match collect_files_to_check(files, files_from, stdin) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::from(1);
        }
    };

    // Get unowned files
    let all_unowned: Vec<&String> = file_cache.get_unowned_files(&lines);

    // Filter to only requested files if specified
    let (unowned, total_files, mode): (Vec<&str>, usize, &str) =
        if let Some(ref filter) = files_to_check {
            let filtered: Vec<&str> = all_unowned
                .into_iter()
                .filter(|f| filter.contains(*f))
                .map(|s| s.as_str())
                .collect();
            (filtered, filter.len(), "checked")
        } else {
            let total = file_cache.count_matches("*");
            (
                all_unowned.into_iter().map(|s| s.as_str()).collect(),
                total,
                "total",
            )
        };

    let owned_count = total_files - unowned.len();
    let coverage_pct = if total_files > 0 {
        (owned_count as f64 / total_files as f64) * 100.0
    } else {
        100.0
    };

    // Color the percentage based on coverage level
    let pct_colored = if coverage_pct >= 90.0 {
        format!("{:.1}%", coverage_pct).green()
    } else if coverage_pct >= 70.0 {
        format!("{:.1}%", coverage_pct).yellow()
    } else {
        format!("{:.1}%", coverage_pct).red()
    };

    println!(
        "{} {} ({}/{} {} files have owners)",
        "Coverage:".bold(),
        pct_colored,
        owned_count.to_string().green(),
        total_files,
        mode
    );

    if unowned.is_empty() {
        println!("\n{} All files have owners!", "✓".green());
        ExitCode::SUCCESS
    } else {
        println!(
            "\n{} ({}):",
            "Files without owners".yellow(),
            unowned.len().to_string().red()
        );
        for file in unowned.iter().take(50) {
            println!("  {}", file.dimmed());
        }
        if unowned.len() > 50 {
            println!("  {} {} more", "...and".dimmed(), unowned.len() - 50);
        }
        ExitCode::from(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_collect_files_none_when_empty() {
        let result = collect_files_to_check(None, None, false).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_collect_files_from_args() {
        let files = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];
        let result = collect_files_to_check(Some(files), None, false).unwrap();
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
        writeln!(temp, "").unwrap(); // empty line
        temp.flush().unwrap();

        let result = collect_files_to_check(None, Some(temp.path().to_path_buf()), false).unwrap();
        assert!(result.is_some());
        let set = result.unwrap();
        assert_eq!(set.len(), 3);
        assert!(set.contains("src/foo.rs"));
        assert!(set.contains("src/bar.rs"));
        assert!(set.contains("src/baz.rs"));
    }

    #[test]
    fn test_collect_files_from_nonexistent_file() {
        let result =
            collect_files_to_check(None, Some(PathBuf::from("/nonexistent/path.txt")), false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to read"));
    }

    #[test]
    fn test_collect_files_combined() {
        let mut temp = NamedTempFile::new().unwrap();
        writeln!(temp, "from_file.rs").unwrap();
        temp.flush().unwrap();

        let files = vec!["from_args.rs".to_string()];
        let result =
            collect_files_to_check(Some(files), Some(temp.path().to_path_buf()), false).unwrap();
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
        let result = collect_files_to_check(Some(files), None, false).unwrap();
        assert!(result.is_some());
        let set = result.unwrap();
        assert_eq!(set.len(), 2);
    }
}
