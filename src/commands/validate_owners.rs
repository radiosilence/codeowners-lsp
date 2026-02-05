use std::collections::HashSet;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::{env, fs};

use colored::Colorize;
use futures::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use serde::Serialize;

use super::files::collect_files;
use crate::github::GitHubClient;
use crate::ownership::find_codeowners;
use crate::parser::{parse_codeowners_file_with_positions, CodeownersLine};
use crate::pattern::pattern_matches;

const CONCURRENCY: usize = 5;

#[derive(Debug)]
enum ValidationResult {
    Valid(String),
    Invalid(String),
    Unknown(String, &'static str),
}

#[derive(Serialize)]
struct ValidateOwnersJson {
    valid: Vec<String>,
    invalid: Vec<String>,
    unknown: Vec<UnknownOwner>,
}

#[derive(Serialize)]
struct UnknownOwner {
    owner: String,
    reason: String,
}

pub async fn validate_owners(
    token: &str,
    files: Option<Vec<String>>,
    files_from: Option<PathBuf>,
    stdin: bool,
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

    // Collect files to filter by (if specified)
    let files_filter = match collect_files(files, files_from, stdin) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::from(1);
        }
    };

    // Collect owners (optionally filtered by which rules match specified files)
    let lines = parse_codeowners_file_with_positions(&content);
    let mut owners: HashSet<String> = HashSet::new();

    for line in &lines {
        if let CodeownersLine::Rule {
            pattern,
            owners: line_owners,
        } = &line.content
        {
            // If file filter specified, only include owners from matching rules
            let should_include = match &files_filter {
                Some(filter) => filter.iter().any(|f| pattern_matches(pattern, f)),
                None => true,
            };

            if should_include {
                for owner in line_owners {
                    owners.insert(owner.clone());
                }
            }
        }
    }

    if owners.is_empty() {
        if files_filter.is_some() {
            println!("{}", "No owners found for the specified files".yellow());
        } else {
            println!("{}", "No owners found in CODEOWNERS".yellow());
        }
        return ExitCode::SUCCESS;
    }

    // Sort for consistent output
    let mut owners_vec: Vec<_> = owners.into_iter().collect();
    owners_vec.sort();

    let total = owners_vec.len();
    let filter_msg = if files_filter.is_some() {
        " (filtered by files)"
    } else {
        ""
    };
    println!(
        "Validating {} unique owners against GitHub{}...\n",
        total.to_string().cyan(),
        filter_msg
    );

    // Progress bar
    let pb = ProgressBar::new(total as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("━╸─"),
    );

    let client = Arc::new(GitHubClient::new());
    let token = token.to_string();

    // Validate in parallel with concurrency limit
    let results: Vec<ValidationResult> = stream::iter(owners_vec)
        .map(|owner| {
            let client = Arc::clone(&client);
            let token = token.clone();
            let pb = pb.clone();
            async move {
                let result = validate_single(&client, &owner, &token).await;
                pb.inc(1);
                result
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;

    pb.finish_and_clear();

    // Sort results for display
    let mut valid: Vec<&str> = Vec::new();
    let mut invalid: Vec<&str> = Vec::new();
    let mut unknown: Vec<(&str, &str)> = Vec::new();

    for result in &results {
        match result {
            ValidationResult::Valid(owner) => valid.push(owner),
            ValidationResult::Invalid(owner) => invalid.push(owner),
            ValidationResult::Unknown(owner, reason) => unknown.push((owner, reason)),
        }
    }

    valid.sort();
    invalid.sort();
    unknown.sort_by_key(|(o, _)| *o);

    // JSON output
    if json {
        let output = ValidateOwnersJson {
            valid: valid.iter().map(|s| s.to_string()).collect(),
            invalid: invalid.iter().map(|s| s.to_string()).collect(),
            unknown: unknown
                .iter()
                .map(|(owner, reason)| UnknownOwner {
                    owner: owner.to_string(),
                    reason: reason.to_string(),
                })
                .collect(),
        };
        println!(
            "{}",
            serde_json::to_string(&output).expect("Failed to serialize JSON")
        );
        return if !invalid.is_empty() {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        };
    }

    // Print results
    for owner in &valid {
        println!("  {} {}", "✓".green(), owner);
    }
    for owner in &invalid {
        println!("  {} {} {}", "✗".red(), owner, "(not found)".dimmed());
    }
    for (owner, reason) in &unknown {
        println!("  {} {} {}", "?".yellow(), owner, reason.dimmed());
    }

    println!("\n{}:", "Summary".bold());
    println!("  {} {}", "Valid:".green(), valid.len());
    println!("  {} {}", "Invalid:".red(), invalid.len());
    println!("  {} {}", "Unknown:".yellow(), unknown.len());

    if !invalid.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

async fn validate_single(client: &GitHubClient, owner: &str, token: &str) -> ValidationResult {
    let result = client.validate_owner(owner, token).await;

    match result {
        Some(true) => ValidationResult::Valid(owner.to_string()),
        Some(false) => ValidationResult::Invalid(owner.to_string()),
        None => {
            let reason = if owner.contains('@') && !owner.starts_with('@') {
                "(email, can't validate)"
            } else {
                "(couldn't validate - check permissions)"
            };
            ValidationResult::Unknown(owner.to_string(), reason)
        }
    }
}
