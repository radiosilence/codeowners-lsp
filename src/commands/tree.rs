use std::collections::HashSet;
use std::process::ExitCode;
use std::{env, fs};

use colored::{Color, Colorize};

use crate::file_cache::FileCache;
use crate::ownership::{check_file_ownership_parsed, find_codeowners, get_repo_root};
use crate::parser::parse_codeowners_file_with_positions;

/// Generate a consistent color from a string
fn owner_color(owner: &str) -> Color {
    let mut hash: u32 = 0;
    for byte in owner.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(byte as u32);
    }
    // Pick from a set of distinct, readable colors
    let colors = [
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::BrightRed,
        Color::BrightGreen,
        Color::BrightYellow,
        Color::BrightBlue,
        Color::BrightMagenta,
        Color::BrightCyan,
    ];
    colors[(hash as usize) % colors.len()]
}

pub fn tree() -> ExitCode {
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

    let repo_root = get_repo_root(&codeowners_path, &cwd);
    let file_cache = FileCache::new(&repo_root);
    let parsed_lines = parse_codeowners_file_with_positions(&content);

    // Collect all files with their owners
    let mut files_with_owners: Vec<(String, Option<String>)> = Vec::new();

    for file in file_cache.all_files() {
        let owners = check_file_ownership_parsed(&parsed_lines, file).map(|r| r.owners.join(" "));
        files_with_owners.push((file.clone(), owners));
    }

    // Sort by path
    files_with_owners.sort_by(|a, b| a.0.cmp(&b.0));

    // Print legend first
    let mut seen_owners: HashSet<String> = HashSet::new();
    for (_, owners) in &files_with_owners {
        if let Some(o) = owners {
            seen_owners.insert(o.clone());
        }
    }
    let mut owners_list: Vec<_> = seen_owners.into_iter().collect();
    owners_list.sort();

    println!("{}:", "Legend".bold());
    for owner in &owners_list {
        let color = owner_color(owner);
        println!("  {} {}", "██".color(color), owner);
    }
    println!("  {} {}\n", "██".dimmed(), "(no owner)".dimmed());

    // Print files
    for (file, owners) in &files_with_owners {
        match owners {
            Some(owner) => {
                let color = owner_color(owner);
                println!("{}", file.color(color));
            }
            None => {
                println!("{}", file.dimmed());
            }
        }
    }

    ExitCode::SUCCESS
}
