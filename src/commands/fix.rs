use std::path::PathBuf;
use std::process::ExitCode;
use std::{env, fs};

use colored::Colorize;

use crate::ownership::{apply_safe_fixes, find_codeowners};

pub fn fix(path: Option<PathBuf>, write: bool) -> ExitCode {
    let cwd = env::current_dir().expect("Failed to get current directory");

    let codeowners_path = path.unwrap_or_else(|| {
        find_codeowners(&cwd).unwrap_or_else(|| {
            eprintln!("No CODEOWNERS file found");
            std::process::exit(1);
        })
    });

    if !codeowners_path.exists() {
        eprintln!("File not found: {}", codeowners_path.display());
        return ExitCode::from(1);
    }

    let content = match fs::read_to_string(&codeowners_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to read {}: {}", codeowners_path.display(), e);
            return ExitCode::from(1);
        }
    };

    let result = apply_safe_fixes(&content);

    if result.fixes.is_empty() {
        println!(
            "{} {} - no fixable issues",
            "✓".green(),
            codeowners_path.display()
        );
        return ExitCode::SUCCESS;
    }

    if write {
        match fs::write(&codeowners_path, &result.content) {
            Ok(_) => {
                println!(
                    "{} Fixed {} ({} changes):",
                    "✓".green(),
                    codeowners_path.display(),
                    result.fixes.len().to_string().cyan()
                );
                for fix in &result.fixes {
                    println!("  {} {}", "-".green(), fix);
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!(
                    "{} Failed to write {}: {}",
                    "✗".red(),
                    codeowners_path.display(),
                    e
                );
                ExitCode::from(1)
            }
        }
    } else {
        println!(
            "Would fix {} ({} changes):",
            codeowners_path.display().to_string().bold(),
            result.fixes.len().to_string().cyan()
        );
        for fix in &result.fixes {
            println!("  {} {}", "-".yellow(), fix);
        }
        println!(
            "\nRun with {} or {} to apply fixes",
            "--write".cyan(),
            "-w".cyan()
        );
        ExitCode::from(1)
    }
}
