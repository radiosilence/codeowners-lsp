use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::{env, fs};

use colored::Colorize;
use serde::Serialize;

use super::files::collect_files;
use crate::file_cache::FileCache;
use crate::ownership::{find_codeowners, get_repo_root};
use crate::parser;

#[derive(Serialize)]
struct CoverageJson {
    total: usize,
    owned: usize,
    unowned: usize,
    coverage_percent: f64,
    unowned_files: Vec<String>,
}

/// Generate a visual progress bar
fn progress_bar(percentage: f64, width: usize) -> String {
    let filled = ((percentage / 100.0) * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);

    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));

    // Color the bar based on coverage
    if percentage >= 90.0 {
        bar.green().to_string()
    } else if percentage >= 70.0 {
        bar.yellow().to_string()
    } else {
        bar.red().to_string()
    }
}

/// Tree node for directory structure
#[derive(Default)]
struct TreeNode {
    files: Vec<String>,               // Files directly in this dir
    dirs: BTreeMap<String, TreeNode>, // Subdirectories
}

impl TreeNode {
    /// Insert a file path into the tree
    fn insert(&mut self, path: &str) {
        let parts: Vec<&str> = path.split('/').collect();
        self.insert_parts(&parts);
    }

    fn insert_parts(&mut self, parts: &[&str]) {
        match parts.len() {
            0 => {}
            1 => self.files.push(parts[0].to_string()),
            _ => {
                let dir = parts[0].to_string();
                self.dirs.entry(dir).or_default().insert_parts(&parts[1..]);
            }
        }
    }

    /// Count total files in this node and all children
    fn count(&self) -> usize {
        self.files.len() + self.dirs.values().map(|d| d.count()).sum::<usize>()
    }

    /// Render the tree with box-drawing characters
    fn render(&self, prefix: &str, _is_last: bool, is_root: bool) -> Vec<String> {
        let mut lines = Vec::new();

        let entries: Vec<_> = self
            .dirs
            .iter()
            .map(|(name, node)| (name.clone(), true, Some(node)))
            .chain(self.files.iter().map(|f| (f.clone(), false, None)))
            .collect();

        for (i, (name, is_dir, node)) in entries.iter().enumerate() {
            let is_last_entry = i == entries.len() - 1;
            let connector = if is_root {
                ""
            } else if is_last_entry {
                "└── "
            } else {
                "├── "
            };

            if *is_dir {
                let node = node.as_ref().unwrap();
                let count = node.count();
                let count_str = if count == 1 {
                    format!("{} file", count)
                } else {
                    format!("{} files", count)
                };
                lines.push(format!(
                    "{}{}{}/  {}",
                    prefix,
                    connector,
                    name.yellow(),
                    count_str.dimmed()
                ));

                let new_prefix = if is_root {
                    prefix.to_string()
                } else if is_last_entry {
                    format!("{}    ", prefix)
                } else {
                    format!("{}│   ", prefix)
                };
                lines.extend(node.render(&new_prefix, is_last_entry, false));
            } else {
                lines.push(format!("{}{}{}", prefix, connector, name.red()));
            }
        }

        lines
    }
}

/// Build and render tree from unowned files
fn render_tree(unowned: &[&str]) -> Vec<String> {
    let mut root = TreeNode::default();
    for file in unowned {
        root.insert(file);
    }
    root.render("  ", true, true)
}

pub fn coverage(
    files: Option<Vec<String>>,
    files_from: Option<PathBuf>,
    stdin: bool,
    tree: bool,
    json: bool,
) -> ExitCode {
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

    let owned_count = total_files.saturating_sub(unowned.len());
    let coverage_pct = if total_files > 0 {
        (owned_count as f64 / total_files as f64) * 100.0
    } else {
        100.0
    };

    // JSON output
    if json {
        let output = CoverageJson {
            total: total_files,
            owned: owned_count,
            unowned: unowned.len(),
            coverage_percent: (coverage_pct * 10.0).round() / 10.0, // 1 decimal place
            unowned_files: unowned.iter().map(|s| s.to_string()).collect(),
        };
        println!(
            "{}",
            serde_json::to_string(&output).expect("Failed to serialize JSON")
        );
        return if unowned.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        };
    }

    // Color the percentage based on coverage level
    let pct_colored = if coverage_pct >= 90.0 {
        format!("{:.1}%", coverage_pct).green().bold()
    } else if coverage_pct >= 70.0 {
        format!("{:.1}%", coverage_pct).yellow().bold()
    } else {
        format!("{:.1}%", coverage_pct).red().bold()
    };

    // Print unowned files first (if any)
    if !unowned.is_empty() {
        println!();
        println!("  {}:", "Unowned files".yellow().bold());
        println!();

        if tree {
            for line in render_tree(&unowned) {
                println!("{}", line);
            }
        } else {
            for file in &unowned {
                println!("    {} {}", "•".red(), file);
            }
        }
    }

    // Print summary at the end
    println!();
    println!(
        "  {} {}",
        "CODEOWNERS Coverage".bold(),
        format!("({} files)", mode).dimmed()
    );
    println!();

    // Print progress bar
    println!("  {} {}", progress_bar(coverage_pct, 30), pct_colored);
    println!();

    // Print stats
    println!(
        "  {}  {} owned",
        "✓".green(),
        owned_count.to_string().green().bold()
    );
    println!(
        "  {}  {} unowned",
        "✗".red(),
        unowned.len().to_string().red().bold()
    );
    println!(
        "  {}  {} total",
        "•".dimmed(),
        total_files.to_string().dimmed()
    );

    if unowned.is_empty() {
        println!();
        println!("  {} 🎉", "All files have owners!".green().bold());
    }
    println!();

    if unowned.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_node_count() {
        let mut root = TreeNode::default();
        root.insert("src/main.rs");
        root.insert("src/lib.rs");
        root.insert("src/commands/coverage.rs");
        root.insert("README.md");

        assert_eq!(root.count(), 4);
        assert_eq!(root.dirs.get("src").unwrap().count(), 3);
    }

    #[test]
    fn test_render_tree_structure() {
        let files = vec![
            "src/main.rs",
            "src/handlers/symbols.rs",
            "src/handlers/navigation.rs",
            "config/settings.toml",
        ];

        let lines = render_tree(&files);

        // Should have directory entries with counts (ANSI codes between name and /)
        assert!(lines
            .iter()
            .any(|l| l.contains("src") && l.contains("3 files")));
        assert!(lines
            .iter()
            .any(|l| l.contains("handlers") && l.contains("2 files")));
        assert!(lines
            .iter()
            .any(|l| l.contains("config") && l.contains("1 file")));

        // Should have file entries
        assert!(lines.iter().any(|l| l.contains("main.rs")));
        assert!(lines.iter().any(|l| l.contains("symbols.rs")));
        assert!(lines.iter().any(|l| l.contains("settings.toml")));
    }
}
