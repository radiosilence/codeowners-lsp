use std::path::PathBuf;
use std::process::ExitCode;
use std::{env, fs};

use colored::Colorize;

use super::files::collect_files;
use crate::file_cache::FileCache;
use crate::ownership::{find_codeowners, get_repo_root};
use crate::parser;

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
    let files_to_check = match collect_files(files, files_from, stdin) {
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
