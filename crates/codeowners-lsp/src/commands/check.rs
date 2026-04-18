use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::{env, fs};

use colored::Colorize;
use serde::Serialize;

use super::files::collect_files;
use crate::ownership::{check_file_ownership_parsed, find_codeowners};
use crate::parser::parse_codeowners_file_with_positions;

#[derive(Serialize)]
struct CheckResultJson {
    owned: bool,
    rule: Option<String>,
    line: Option<u32>,
    owners: Vec<String>,
}

pub fn check(
    paths: Vec<String>,
    files: Option<Vec<String>>,
    json: bool,
    files_from: Option<PathBuf>,
    stdin: bool,
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

    // Merge positional paths with --files for consistent interface
    let files_arg = if paths.is_empty() {
        files
    } else {
        let mut merged = paths;
        if let Some(f) = files {
            merged.extend(f);
        }
        Some(merged)
    };

    let all_files: Vec<String> = match collect_files(files_arg, files_from, stdin) {
        Ok(Some(set)) => set.into_iter().collect(),
        Ok(None) => {
            eprintln!("No files specified");
            return ExitCode::from(1);
        }
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::from(1);
        }
    };

    if json {
        output_json(&content, &all_files)
    } else {
        output_human(&content, &all_files)
    }
}

fn output_json(content: &str, files: &[String]) -> ExitCode {
    let parsed = parse_codeowners_file_with_positions(content);
    let mut results: HashMap<&str, CheckResultJson> = HashMap::new();

    for file_path in files {
        let result = check_file_ownership_parsed(&parsed, file_path);
        results.insert(
            file_path,
            match result {
                Some(r) => CheckResultJson {
                    owned: true,
                    rule: Some(r.pattern),
                    line: Some(r.line_number + 1),
                    owners: r.owners,
                },
                None => CheckResultJson {
                    owned: false,
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
    let parsed = parse_codeowners_file_with_positions(content);
    let mut any_unowned = false;

    for (i, file_path) in files.iter().enumerate() {
        if i > 0 {
            println!();
        }

        match check_file_ownership_parsed(&parsed, file_path) {
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
