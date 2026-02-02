//! Suggest command - recommends owners for unowned files based on git history.
//!
//! Analyzes git commit history to determine who has been working on unowned
//! files, then suggests appropriate CODEOWNERS entries.
//!
//! Requires `lookup_cmd` config to resolve git emails to team names.

use std::collections::{HashMap, HashSet};
use std::process::ExitCode;
use std::{env, fs};

use colored::Colorize;

use crate::blame::{suggest_owners_for_files, OwnerSuggestion};
use crate::file_cache::FileCache;
use crate::lookup::OwnerLookup;
use crate::ownership::{find_codeowners, get_repo_root};
use crate::parser::{self, find_insertion_point_with_owner, CodeownersLine};

use super::load_settings;

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
    /// Write suggestions to CODEOWNERS file
    pub write: bool,
}

impl Default for SuggestOptions {
    fn default() -> Self {
        Self {
            min_confidence: 30.0,
            format: OutputFormat::Human,
            limit: 50,
            include_owned: false,
            write: false,
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

    // Check for lookup_cmd - required for suggest to work
    let settings = load_settings();
    let lookup_cmd = match settings.lookup_cmd() {
        Some(cmd) => cmd,
        None => {
            eprintln!(
                "{} The suggest command requires lookup_cmd to be configured.",
                "Error:".red().bold()
            );
            eprintln!("  Add to .codeowners-lsp.toml:");
            eprintln!(
                "  {}",
                "lookup_cmd = \"your-tool lookup {email} | jq -r .team\"".dimmed()
            );
            return ExitCode::from(1);
        }
    };

    // Extract existing owners from CODEOWNERS for fuzzy matching
    let existing_owners: Vec<String> = lines
        .iter()
        .filter_map(|line| {
            if let CodeownersLine::Rule { owners, .. } = &line.content {
                Some(owners.clone())
            } else {
                None
            }
        })
        .flatten()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let mut lookup = OwnerLookup::new(lookup_cmd, existing_owners);

    // Analyze git history and get suggestions
    let suggestions = suggest_owners_for_files(&repo_root, &unowned, options.min_confidence);

    // Collect all unique contributor emails for batch lookup
    let all_emails: Vec<String> = suggestions
        .iter()
        .flat_map(|s| s.contributors.iter().map(|c| c.email.clone()))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    // Batch lookup all emails in parallel with progress bar
    let email_to_owner = lookup.batch_lookup(&all_emails);

    // Transform suggestions: use lookup results to pick best team
    let suggestions: Vec<OwnerSuggestion> = suggestions
        .into_iter()
        .filter_map(|mut s| {
            // For each contributor, use cached lookup and accumulate weighted votes
            let mut team_votes: HashMap<String, usize> = HashMap::new();

            for contributor in &s.contributors {
                if let Some(Some(resolved_owner)) = email_to_owner.get(&contributor.email) {
                    *team_votes.entry(resolved_owner.clone()).or_insert(0) +=
                        contributor.commit_count;
                }
            }

            // Pick the team with the most weighted votes
            let best_team = team_votes
                .into_iter()
                .max_by_key(|(_, votes)| *votes)
                .map(|(team, _)| team)?;

            s.suggested_owner = best_team;
            Some(s)
        })
        .collect();

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

    // Limit suggestions
    let suggestions: Vec<_> = suggestions.into_iter().take(options.limit).collect();

    // Output based on format
    match options.format {
        OutputFormat::Human => output_human(&suggestions, &unowned),
        OutputFormat::Codeowners => output_codeowners(&suggestions),
        OutputFormat::Json => output_json(&suggestions, &unowned),
    }

    // Write to file if requested
    if options.write && !suggestions.is_empty() {
        let new_content = apply_suggestions(&content, &suggestions);
        if let Err(e) = fs::write(&codeowners_path, &new_content) {
            eprintln!(
                "{} Failed to write {}: {}",
                "Error:".red().bold(),
                codeowners_path.display(),
                e
            );
            return ExitCode::from(1);
        }
        println!(
            "\n{} Added {} rules to {}",
            "✓".green(),
            suggestions.len(),
            codeowners_path.display()
        );
    }

    ExitCode::SUCCESS
}

/// Apply suggestions to CODEOWNERS content, inserting each rule at the best location
fn apply_suggestions(content: &str, suggestions: &[OwnerSuggestion]) -> String {
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let parsed = parser::parse_codeowners_file(content);

    // Insert suggestions in reverse order of insertion point to avoid index shifting
    let mut insertions: Vec<(usize, String)> = suggestions
        .iter()
        .map(|s| {
            let insert_idx =
                find_insertion_point_with_owner(&parsed, &s.path, Some(&s.suggested_owner));
            let new_line = format!("{} {}", s.path, s.suggested_owner);
            (insert_idx, new_line)
        })
        .collect();

    // Sort by insertion point descending so we insert from bottom to top
    insertions.sort_by(|a, b| b.0.cmp(&a.0));

    for (idx, line) in insertions {
        lines.insert(idx, line);
    }

    lines.join("\n") + "\n"
}

fn output_human(suggestions: &[OwnerSuggestion], unowned: &[String]) {
    println!(
        "{} Analyzing {} unowned files...\n",
        "→".blue(),
        unowned.len()
    );

    println!(
        "{} {} {} found:\n",
        "✓".green(),
        suggestions.len(),
        if suggestions.len() == 1 {
            "suggestion"
        } else {
            "suggestions"
        }
    );

    for (i, suggestion) in suggestions.iter().enumerate() {
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

    // Print ready-to-use CODEOWNERS lines
    println!("{}", "─".repeat(60).dimmed());
    println!("📋 Add to CODEOWNERS:\n");
    for suggestion in suggestions.iter() {
        println!("{} {}", suggestion.path, suggestion.suggested_owner);
    }
}

fn output_codeowners(suggestions: &[OwnerSuggestion]) {
    println!("# Suggested CODEOWNERS entries (generated from git history)");
    println!("# Review and verify before committing!\n");

    for suggestion in suggestions.iter() {
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
