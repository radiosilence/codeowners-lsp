use std::collections::HashMap;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::process::ExitCode;
use std::{env, fs};

use colored::Colorize;
use serde::Serialize;

use crate::ownership::{check_file_ownership, find_codeowners};

#[derive(Serialize)]
struct CheckResultJson {
    rule: Option<String>,
    line: Option<u32>,
    owners: Vec<String>,
}

fn collect_files(
    files: Vec<String>,
    files_from: Option<PathBuf>,
    stdin: bool,
) -> Result<Vec<String>, String> {
    let mut all_files = files;

    if let Some(path) = files_from {
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        all_files.extend(content.lines().filter(|l| !l.is_empty()).map(String::from));
    }

    if stdin {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let line = line.map_err(|e| format!("Failed to read stdin: {}", e))?;
            if !line.is_empty() {
                all_files.push(line);
            }
        }
    }

    Ok(all_files)
}

pub fn check(files: Vec<String>, json: bool, files_from: Option<PathBuf>, stdin: bool) -> ExitCode {
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

    let all_files = match collect_files(files, files_from, stdin) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::from(1);
        }
    };

    if all_files.is_empty() {
        eprintln!("No files specified");
        return ExitCode::from(1);
    }

    if json {
        output_json(&content, &all_files)
    } else {
        output_human(&content, &all_files)
    }
}

fn output_json(content: &str, files: &[String]) -> ExitCode {
    let mut results: HashMap<&str, CheckResultJson> = HashMap::new();

    for file_path in files {
        let result = check_file_ownership(content, file_path);
        results.insert(
            file_path,
            match result {
                Some(r) => CheckResultJson {
                    rule: Some(r.pattern),
                    line: Some(r.line_number + 1),
                    owners: r.owners,
                },
                None => CheckResultJson {
                    rule: None,
                    line: None,
                    owners: vec![],
                },
            },
        );
    }

    println!(
        "{}",
        serde_json::to_string(&results).expect("Failed to serialize JSON")
    );
    ExitCode::SUCCESS
}

fn output_human(content: &str, files: &[String]) -> ExitCode {
    let mut any_unowned = false;

    for (i, file_path) in files.iter().enumerate() {
        if i > 0 {
            println!();
        }

        match check_file_ownership(content, file_path) {
            Some(result) => {
                println!("{} {}", "File:".bold(), file_path);
                println!(
                    "{} {} {}",
                    "Rule:".bold(),
                    result.pattern.cyan(),
                    format!("(line {})", result.line_number + 1).dimmed()
                );
                println!("{} {}", "Owners:".bold(), result.owners.join(" ").green());
            }
            None => {
                any_unowned = true;
                println!("{} {}", "File:".bold(), file_path);
                println!(
                    "{} {}",
                    "✗".red(),
                    "No matching rule - file has no owners".yellow()
                );
            }
        }
    }

    // Return success even if some files are unowned (for multi-file mode)
    // Users can use --strict in lint command if they want to fail on missing owners
    if files.len() == 1 && any_unowned {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
