use std::collections::HashSet;
use std::process::ExitCode;
use std::{env, fs};

use colored::Colorize;

use crate::github::GitHubClient;
use crate::ownership::find_codeowners;
use crate::parser::{parse_codeowners_file_with_positions, CodeownersLine};

pub async fn validate_owners(token: &str) -> ExitCode {
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

    // Collect unique owners
    let lines = parse_codeowners_file_with_positions(&content);
    let mut owners: HashSet<String> = HashSet::new();

    for line in &lines {
        if let CodeownersLine::Rule {
            owners: line_owners,
            ..
        } = &line.content
        {
            for owner in line_owners {
                owners.insert(owner.clone());
            }
        }
    }

    if owners.is_empty() {
        println!("{}", "No owners found in CODEOWNERS".yellow());
        return ExitCode::SUCCESS;
    }

    println!(
        "Validating {} unique owners against GitHub...\n",
        owners.len().to_string().cyan()
    );

    let client = GitHubClient::new();
    let mut valid_count = 0;
    let mut invalid_count = 0;
    let mut unknown_count = 0;

    // Sort for consistent output
    let mut owners_vec: Vec<_> = owners.into_iter().collect();
    owners_vec.sort();

    for owner in &owners_vec {
        let result = client.validate_owner(owner, token).await;

        match result {
            Some(true) => {
                println!("  {} {}", "✓".green(), owner);
                valid_count += 1;
            }
            Some(false) => {
                println!("  {} {} {}", "✗".red(), owner, "(not found)".dimmed());
                invalid_count += 1;
            }
            None => {
                // Email or couldn't validate (403 permission)
                if owner.contains('@') && !owner.starts_with('@') {
                    println!(
                        "  {} {} {}",
                        "?".yellow(),
                        owner,
                        "(email, can't validate)".dimmed()
                    );
                } else {
                    println!(
                        "  {} {} {}",
                        "?".yellow(),
                        owner,
                        "(couldn't validate - check permissions)".dimmed()
                    );
                }
                unknown_count += 1;
            }
        }
    }

    println!("\n{}:", "Summary".bold());
    println!("  {} {}", "Valid:".green(), valid_count);
    println!("  {} {}", "Invalid:".red(), invalid_count);
    println!("  {} {}", "Unknown:".yellow(), unknown_count);

    if invalid_count > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
