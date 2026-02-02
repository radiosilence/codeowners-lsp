//! Suggest command - recommends owners for unowned files based on git history.
//!
//! Analyzes git commit history to determine who has been working on unowned
//! files, then suggests appropriate CODEOWNERS entries.

use std::process::ExitCode;
use std::{env, fs};

use colored::Colorize;

use crate::blame::{suggest_owners_for_files, OwnerSuggestion};
use crate::file_cache::FileCache;
use crate::ownership::{find_codeowners, get_repo_root};
use crate::parser;

/// Output format for suggestions
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputFormat {
    /// Human-readable output with explanations
    Human,
    /// CODEOWNERS-compatible lines ready to copy
    Codeowners,
    /// JSON output for tooling
    Json,
}

/// Options for the suggest command
#[derive(Debug, Clone)]
pub struct SuggestOptions {
    /// Minimum confidence threshold (0-100)
    pub min_confidence: f64,
    /// Output format
    pub format: OutputFormat,
    /// Maximum number of suggestions
    pub limit: usize,
    /// Include files that already have owners (for comparison)
    #[allow(dead_code)] // Reserved for --include-owned flag
    pub include_owned: bool,
}

impl Default for SuggestOptions {
    fn default() -> Self {
        Self {
            min_confidence: 30.0,
            format: OutputFormat::Human,
            limit: 50,
            include_owned: false,
        }
    }
}

pub fn suggest(options: SuggestOptions) -> ExitCode {
    let cwd = env::current_dir().expect("Failed to get current directory");

    let codeowners_path = match find_codeowners(&cwd) {
        Some(p) => p,
        None => {
            eprintln!(
                "{} No CODEOWNERS file found. Create one first or run from a repo with CODEOWNERS.",
                "Error:".red().bold()
            );
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

    // Get unowned files
    let unowned: Vec<String> = file_cache
        .get_unowned_files(&lines)
        .iter()
        .map(|s| s.to_string())
        .collect();

    if unowned.is_empty() {
        match options.format {
            OutputFormat::Human => {
                println!("{} All files already have owners!", "✓".green());
            }
            OutputFormat::Json => {
                println!("{{\"suggestions\": [], \"message\": \"All files have owners\"}}");
            }
            OutputFormat::Codeowners => {
                println!("# All files already have owners");
            }
        }
        return ExitCode::SUCCESS;
    }

    // Analyze git history and get suggestions
    let suggestions = suggest_owners_for_files(&repo_root, &unowned, options.min_confidence);

    if suggestions.is_empty() {
        match options.format {
            OutputFormat::Human => {
                println!(
                    "{} No confident suggestions found for {} unowned files.",
                    "!".yellow(),
                    unowned.len()
                );
                println!(
                    "  Try lowering --min-confidence (currently {}%)",
                    options.min_confidence
                );
            }
            OutputFormat::Json => {
                println!(
                    "{{\"suggestions\": [], \"unowned_count\": {}, \"message\": \"No confident suggestions\"}}",
                    unowned.len()
                );
            }
            OutputFormat::Codeowners => {
                println!(
                    "# No confident suggestions for {} unowned files",
                    unowned.len()
                );
            }
        }
        return ExitCode::SUCCESS;
    }

    // Output based on format
    match options.format {
        OutputFormat::Human => output_human(&suggestions, &unowned, options.limit),
        OutputFormat::Codeowners => output_codeowners(&suggestions, options.limit),
        OutputFormat::Json => output_json(&suggestions, &unowned),
    }

    ExitCode::SUCCESS
}

fn output_human(suggestions: &[OwnerSuggestion], unowned: &[String], limit: usize) {
    println!(
        "{} Analyzing {} unowned files...\n",
        "→".blue(),
        unowned.len()
    );

    println!(
        "{} {} {} found:\n",
        "✓".green(),
        suggestions.len().min(limit),
        if suggestions.len() == 1 {
            "suggestion"
        } else {
            "suggestions"
        }
    );

    for (i, suggestion) in suggestions.iter().take(limit).enumerate() {
        let confidence_color = if suggestion.confidence >= 70.0 {
            suggestion.confidence.to_string().green()
        } else if suggestion.confidence >= 50.0 {
            suggestion.confidence.to_string().yellow()
        } else {
            suggestion.confidence.to_string().red()
        };

        println!(
            "{}. {} {} {}",
            (i + 1).to_string().bold(),
            suggestion.path.cyan(),
            suggestion.suggested_owner.green().bold(),
            format!("({}% confidence)", confidence_color).dimmed()
        );

        // Show top contributors
        let top_contribs: Vec<String> = suggestion
            .contributors
            .iter()
            .take(3)
            .map(|c| format!("{} ({}%)", c.name, c.percentage as u32))
            .collect();

        println!(
            "   {} {} from {} commits",
            "Based on:".dimmed(),
            top_contribs.join(", ").dimmed(),
            suggestion.total_commits
        );
        println!();
    }

    if suggestions.len() > limit {
        println!(
            "{} {} more suggestions not shown (use --limit to see more)",
            "...".dimmed(),
            suggestions.len() - limit
        );
    }

    // Print ready-to-use CODEOWNERS lines
    println!("\n{}", "─".repeat(60).dimmed());
    println!("📋 Add to CODEOWNERS:\n");
    for suggestion in suggestions.iter().take(limit) {
        println!("{} {}", suggestion.path, suggestion.suggested_owner);
    }
}

fn output_codeowners(suggestions: &[OwnerSuggestion], limit: usize) {
    println!("# Suggested CODEOWNERS entries (generated from git history)");
    println!("# Review and verify before committing!\n");

    for suggestion in suggestions.iter().take(limit) {
        println!(
            "# Confidence: {:.0}% ({} commits)",
            suggestion.confidence, suggestion.total_commits
        );
        println!("{} {}", suggestion.path, suggestion.suggested_owner);
        println!();
    }
}

fn output_json(suggestions: &[OwnerSuggestion], unowned: &[String]) {
    let json_suggestions: Vec<serde_json::Value> = suggestions
        .iter()
        .map(|s| {
            serde_json::json!({
                "path": s.path,
                "suggested_owner": s.suggested_owner,
                "confidence": s.confidence,
                "total_commits": s.total_commits,
                "contributors": s.contributors.iter().map(|c| {
                    serde_json::json!({
                        "name": c.name,
                        "email": c.email,
                        "commits": c.commit_count,
                        "percentage": c.percentage
                    })
                }).collect::<Vec<_>>()
            })
        })
        .collect();

    let output = serde_json::json!({
        "unowned_count": unowned.len(),
        "suggestion_count": suggestions.len(),
        "suggestions": json_suggestions
    });

    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}
