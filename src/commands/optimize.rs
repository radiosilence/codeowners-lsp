//! Optimize command - suggests ways to simplify CODEOWNERS patterns.
//!
//! Analyzes existing rules and suggests:
//! - Consolidating multiple file patterns into directory patterns
//! - Removing redundant/shadowed rules
//! - Using more specific globs instead of listing files

use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;
use std::{env, fs};

use colored::Colorize;

use crate::file_cache::FileCache;
use crate::ownership::{find_codeowners, get_repo_root};
use crate::parser::{self, CodeownersLine, ParsedLine};

/// A suggested optimization
#[derive(Debug, Clone)]
pub struct Optimization {
    /// Type of optimization
    pub kind: OptimizationKind,
    /// Lines that would be replaced (0-indexed)
    pub affected_lines: Vec<u32>,
    /// Current patterns being replaced
    pub current_patterns: Vec<String>,
    /// Suggested replacement pattern
    pub suggested_pattern: String,
    /// Owners for the new pattern
    pub owners: Vec<String>,
    /// Explanation of the optimization
    pub reason: String,
    /// Number of files covered
    pub files_covered: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OptimizationKind {
    /// Multiple files in same dir → directory pattern
    ConsolidateToDirectory,
    /// Multiple patterns with same owner → combined pattern
    #[allow(dead_code)] // Reserved for future use
    CombinePatterns,
    /// Pattern can use glob instead of listing
    UseGlob,
    /// Redundant rule (shadowed)
    RemoveRedundant,
}

/// Output format for optimizations
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputFormat {
    /// Human-readable suggestions
    Human,
    /// JSON for CI/tooling
    Json,
}

/// Options for the optimize command
#[derive(Debug, Clone)]
pub struct OptimizeOptions {
    /// Output format (human or json)
    pub format: OutputFormat,
    /// Minimum files to suggest directory consolidation
    pub min_files_for_dir: usize,
    /// Write changes to file (false = preview only)
    pub write: bool,
}

impl Default for OptimizeOptions {
    fn default() -> Self {
        Self {
            format: OutputFormat::Human,
            min_files_for_dir: 3,
            write: false,
        }
    }
}

pub fn optimize(options: OptimizeOptions) -> ExitCode {
    let cwd = env::current_dir().expect("Failed to get current directory");

    let codeowners_path = match find_codeowners(&cwd) {
        Some(p) => p,
        None => {
            eprintln!("{} No CODEOWNERS file found", "Error:".red().bold());
            return ExitCode::from(1);
        }
    };

    let content = match fs::read_to_string(&codeowners_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "{} Failed to read {}: {}",
                "Error:".red().bold(),
                codeowners_path.display(),
                e
            );
            return ExitCode::from(1);
        }
    };

    let repo_root = get_repo_root(&codeowners_path, &cwd);
    let file_cache = FileCache::new(&repo_root);
    let lines = parser::parse_codeowners_file_with_positions(&content);

    // Find optimizations
    let optimizations = find_optimizations(&lines, &file_cache, &options);

    if optimizations.is_empty() {
        match options.format {
            OutputFormat::Human => {
                println!("{} CODEOWNERS file is already optimized!", "✓".green());
            }
            OutputFormat::Json => {
                println!("{{\"optimizations\": [], \"message\": \"Already optimized\"}}");
            }
        }
        return ExitCode::SUCCESS;
    }

    // Output based on format
    match options.format {
        OutputFormat::Human => output_human(&optimizations),
        OutputFormat::Json => output_json(&optimizations),
    }

    // Apply changes if --write
    if options.write {
        let optimized = apply_optimizations(&content, &lines, &optimizations);
        if let Err(e) = fs::write(&codeowners_path, &optimized) {
            eprintln!(
                "{} Failed to write {}: {}",
                "Error:".red().bold(),
                codeowners_path.display(),
                e
            );
            return ExitCode::from(1);
        }
        println!("\n{} Written to {}", "✓".green(), codeowners_path.display());
    }

    ExitCode::SUCCESS
}

fn find_optimizations(
    lines: &[ParsedLine],
    file_cache: &FileCache,
    options: &OptimizeOptions,
) -> Vec<Optimization> {
    let mut optimizations = Vec::new();

    // 1. Find rules that could be consolidated into directory patterns
    optimizations.extend(find_directory_consolidations(lines, file_cache, options));

    // 2. Find patterns with same owner that could be combined
    optimizations.extend(find_combinable_patterns(lines, file_cache));

    // 3. Find redundant/shadowed rules
    optimizations.extend(find_redundant_rules(lines));

    optimizations
}

/// Find multiple file rules in the same directory with the same owner
#[allow(clippy::type_complexity)]
fn find_directory_consolidations(
    lines: &[ParsedLine],
    file_cache: &FileCache,
    options: &OptimizeOptions,
) -> Vec<Optimization> {
    let mut optimizations = Vec::new();

    // Group rules by (directory, owners)
    let mut dir_rules: HashMap<(String, Vec<String>), Vec<(u32, String)>> = HashMap::new();

    for line in lines {
        if let CodeownersLine::Rule { pattern, owners } = &line.content {
            // Skip patterns that are already directories or globs
            if pattern.ends_with('/') || pattern.contains('*') {
                continue;
            }

            // Get the parent directory
            let dir = Path::new(pattern.trim_start_matches('/'))
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            if dir.is_empty() {
                continue;
            }

            let key = (dir, owners.clone());
            dir_rules
                .entry(key)
                .or_default()
                .push((line.line_number, pattern.clone()));
        }
    }

    // Find directories with enough rules to consolidate
    for ((dir, owners), rules) in dir_rules {
        if rules.len() < options.min_files_for_dir {
            continue;
        }

        // Check if ALL files in this directory are covered
        let dir_pattern = format!("/{}/", dir);
        let all_files_in_dir = file_cache.get_matches(&format!("/{}/*", dir));

        // Count how many files in this dir are covered by the rules
        let covered_files: Vec<_> = rules.iter().map(|(_, p)| p.clone()).collect();

        // Only suggest if the rules cover most files in the directory
        let coverage_ratio = covered_files.len() as f64 / all_files_in_dir.len().max(1) as f64;

        if coverage_ratio >= 0.7 {
            // 70% of files covered = suggest dir pattern
            optimizations.push(Optimization {
                kind: OptimizationKind::ConsolidateToDirectory,
                affected_lines: rules.iter().map(|(l, _)| *l).collect(),
                current_patterns: covered_files,
                suggested_pattern: dir_pattern,
                owners,
                reason: format!(
                    "Consolidate {} file rules into directory pattern ({:.0}% of dir covered)",
                    rules.len(),
                    coverage_ratio * 100.0
                ),
                files_covered: all_files_in_dir.len(),
            });
        }
    }

    optimizations
}

/// Find patterns with same owner that could use a common glob
#[allow(clippy::type_complexity)]
fn find_combinable_patterns(lines: &[ParsedLine], _file_cache: &FileCache) -> Vec<Optimization> {
    let mut optimizations = Vec::new();

    // Group by owner and extension
    let mut ext_rules: HashMap<(Vec<String>, String), Vec<(u32, String)>> = HashMap::new();

    for line in lines {
        if let CodeownersLine::Rule { pattern, owners } = &line.content {
            // Extract extension if it's a file pattern
            if let Some(ext) = Path::new(pattern).extension() {
                let ext = ext.to_string_lossy().to_string();
                let key = (owners.clone(), ext);
                ext_rules
                    .entry(key)
                    .or_default()
                    .push((line.line_number, pattern.clone()));
            }
        }
    }

    // Find extensions with multiple rules
    for ((owners, ext), rules) in ext_rules {
        if rules.len() < 3 {
            continue;
        }

        // Check if patterns are in same directory
        let dirs: Vec<_> = rules
            .iter()
            .filter_map(|(_, p)| {
                Path::new(p.trim_start_matches('/'))
                    .parent()
                    .map(|d| d.to_string_lossy().to_string())
            })
            .collect();

        // If all in same directory, suggest directory glob
        if !dirs.is_empty() && dirs.iter().all(|d| d == &dirs[0]) {
            let dir = &dirs[0];
            optimizations.push(Optimization {
                kind: OptimizationKind::UseGlob,
                affected_lines: rules.iter().map(|(l, _)| *l).collect(),
                current_patterns: rules.iter().map(|(_, p)| p.clone()).collect(),
                suggested_pattern: format!("/{}/*.{}", dir, ext),
                owners,
                reason: format!(
                    "Use glob pattern for {} .{} files in same directory",
                    rules.len(),
                    ext
                ),
                files_covered: rules.len(),
            });
        }
    }

    optimizations
}

/// Find rules that are shadowed by later rules
fn find_redundant_rules(lines: &[ParsedLine]) -> Vec<Optimization> {
    let mut optimizations = Vec::new();
    let mut seen_patterns: HashMap<String, u32> = HashMap::new();

    for line in lines {
        if let CodeownersLine::Rule { pattern, owners } = &line.content {
            let normalized = pattern.trim_start_matches('/').to_string();

            if let Some(&first_line) = seen_patterns.get(&normalized) {
                optimizations.push(Optimization {
                    kind: OptimizationKind::RemoveRedundant,
                    affected_lines: vec![first_line],
                    current_patterns: vec![pattern.clone()],
                    suggested_pattern: String::new(),
                    owners: owners.clone(),
                    reason: format!(
                        "Duplicate pattern - line {} shadows line {}",
                        line.line_number + 1,
                        first_line + 1
                    ),
                    files_covered: 0,
                });
            }

            seen_patterns.insert(normalized, line.line_number);
        }
    }

    optimizations
}

fn output_human(optimizations: &[Optimization]) {
    println!(
        "{} Found {} optimization{}:\n",
        "✓".green(),
        optimizations.len(),
        if optimizations.len() == 1 { "" } else { "s" }
    );

    for (i, opt) in optimizations.iter().enumerate() {
        let kind_str = match opt.kind {
            OptimizationKind::ConsolidateToDirectory => "📁 Directory consolidation",
            OptimizationKind::CombinePatterns => "🔗 Combine patterns",
            OptimizationKind::UseGlob => "✨ Use glob",
            OptimizationKind::RemoveRedundant => "🗑️  Remove redundant",
        };

        println!("{}. {}", (i + 1).to_string().bold(), kind_str.cyan());
        println!("   {}", opt.reason.dimmed());

        if !opt.current_patterns.is_empty() && opt.current_patterns.len() <= 5 {
            println!("   {}:", "Current".yellow());
            for pattern in &opt.current_patterns {
                println!("     {}", pattern.dimmed());
            }
        } else if !opt.current_patterns.is_empty() {
            println!(
                "   {}: {} patterns on lines {:?}",
                "Current".yellow(),
                opt.current_patterns.len(),
                opt.affected_lines.iter().map(|l| l + 1).collect::<Vec<_>>()
            );
        }

        if !opt.suggested_pattern.is_empty() {
            println!(
                "   {}: {} {}",
                "Suggested".green(),
                opt.suggested_pattern.green().bold(),
                opt.owners.join(" ").green()
            );
        }

        println!();
    }
}

/// Apply optimizations to generate new file content
fn apply_optimizations(
    content: &str,
    _lines: &[ParsedLine],
    optimizations: &[Optimization],
) -> String {
    // Track which lines to skip (affected by optimizations)
    let lines_to_skip: std::collections::HashSet<u32> = optimizations
        .iter()
        .flat_map(|o| o.affected_lines.clone())
        .collect();

    let mut result = String::new();
    let mut added_optimizations = false;

    // Output original lines, skipping affected ones
    for (i, line) in content.lines().enumerate() {
        let line_num = i as u32;
        if lines_to_skip.contains(&line_num) {
            continue;
        }
        result.push_str(line);
        result.push('\n');
    }

    // Add new optimized patterns at the end
    for opt in optimizations {
        if !opt.suggested_pattern.is_empty() {
            if !added_optimizations {
                result.push_str("\n# Optimized patterns\n");
                added_optimizations = true;
            }
            result.push_str(&format!(
                "{} {}\n",
                opt.suggested_pattern,
                opt.owners.join(" ")
            ));
        }
    }

    result
}

fn output_json(optimizations: &[Optimization]) {
    let json_opts: Vec<serde_json::Value> = optimizations
        .iter()
        .map(|o| {
            serde_json::json!({
                "kind": format!("{:?}", o.kind),
                "affected_lines": o.affected_lines,
                "current_patterns": o.current_patterns,
                "suggested_pattern": o.suggested_pattern,
                "owners": o.owners,
                "reason": o.reason,
                "files_covered": o.files_covered
            })
        })
        .collect();

    let output = serde_json::json!({
        "optimization_count": optimizations.len(),
        "optimizations": json_opts
    });

    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}
