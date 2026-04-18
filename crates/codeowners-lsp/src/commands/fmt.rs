use std::path::PathBuf;
use std::process::ExitCode;
use std::{env, fs};

use colored::Colorize;

use crate::ownership::find_codeowners;
use crate::parser::format_codeowners;

pub fn fmt(path: Option<PathBuf>, write: bool) -> ExitCode {
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

    let formatted = format_codeowners(&content);

    if formatted == content {
        println!(
            "{} {} is already formatted",
            "✓".green(),
            codeowners_path.display()
        );
        return ExitCode::SUCCESS;
    }

    if write {
        match fs::write(&codeowners_path, &formatted) {
            Ok(_) => {
                println!("{} Formatted {}", "✓".green(), codeowners_path.display());
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
            "Would format {}:\n",
            codeowners_path.display().to_string().bold()
        );
        println!("{}", "--- original".red());
        println!("{}\n", "+++ formatted".green());

        // Simple diff: show lines that differ
        let old_lines: Vec<&str> = content.lines().collect();
        let new_lines: Vec<&str> = formatted.lines().collect();

        let max_lines = old_lines.len().max(new_lines.len());
        for i in 0..max_lines {
            let old = old_lines.get(i).copied().unwrap_or("");
            let new = new_lines.get(i).copied().unwrap_or("");

            if old != new {
                if !old.is_empty() {
                    println!("{}", format!("-{}", old).red());
                }
                if !new.is_empty() {
                    println!("{}", format!("+{}", new).green());
                }
            }
        }

        println!(
            "\nRun with {} or {} to apply changes",
            "--write".cyan(),
            "-w".cyan()
        );
        ExitCode::from(1)
    }
}
