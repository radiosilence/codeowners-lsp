use std::process::ExitCode;
use std::{env, fs};

use colored::Colorize;

use crate::ownership::{check_file_ownership, find_codeowners};

pub fn check(file_path: &str) -> ExitCode {
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

    match check_file_ownership(&content, file_path) {
        Some(result) => {
            println!("{} {}", "File:".bold(), file_path);
            println!(
                "{} {} {}",
                "Rule:".bold(),
                result.pattern.cyan(),
                format!("(line {})", result.line_number + 1).dimmed()
            );
            println!("{} {}", "Owners:".bold(), result.owners.join(" ").green());
            ExitCode::SUCCESS
        }
        None => {
            println!("{} {}", "File:".bold(), file_path);
            println!(
                "{} {}",
                "✗".red(),
                "No matching rule - file has no owners".yellow()
            );
            ExitCode::from(1)
        }
    }
}
