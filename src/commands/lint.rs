use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::{env, fs};

use colored::Colorize;
use futures::stream::{self, StreamExt};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};

use crate::diagnostics;
use crate::file_cache::FileCache;
use crate::github::{GitHubClient, PersistentCache};
use crate::ownership::{apply_safe_fixes, find_codeowners, get_repo_root};
use crate::parser::{parse_codeowners_file_with_positions, CodeownersLine};
use crate::settings::load_settings_from_path;

const CONCURRENCY: usize = 5;

pub async fn lint(path: Option<PathBuf>, json_output: bool, fix: bool, strict: bool) -> ExitCode {
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

    let repo_root = get_repo_root(&codeowners_path, &cwd);
    let file_cache = FileCache::new(&repo_root);

    // If --fix, apply safe fixes and write
    if fix {
        let fix_result = apply_safe_fixes(&content, Some(&file_cache));
        if fix_result.fixes.is_empty() {
            println!(
                "{} {} - no fixable issues",
                "✓".green(),
                codeowners_path.display()
            );
            return ExitCode::SUCCESS;
        }

        match fs::write(&codeowners_path, &fix_result.content) {
            Ok(_) => {
                println!(
                    "{} Fixed {} ({} changes):",
                    "✓".green(),
                    codeowners_path.display(),
                    fix_result.fixes.len().to_string().cyan()
                );
                for fix_msg in &fix_result.fixes {
                    println!("  {} {}", "-".green(), fix_msg);
                }
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                eprintln!(
                    "{} Failed to write {}: {}",
                    "✗".red(),
                    codeowners_path.display(),
                    e
                );
                return ExitCode::from(1);
            }
        }
    }

    // Load config from file
    let settings = load_settings_from_path(&repo_root);
    let diag_config = settings.diagnostic_config();
    let (mut diagnostics, _) =
        diagnostics::compute_diagnostics_sync(&content, Some(&file_cache), &diag_config);

    // Check if validation is enabled
    if settings.validate_owners {
        if let Some(token) = settings.resolve_token() {
            let validation_diags = validate_owners_for_lint(&content, &repo_root, &token).await;
            diagnostics.extend(validation_diags);
        }
    }

    // Sort diagnostics by line number
    diagnostics.sort_by_key(|d| d.range.start.line);

    if json_output {
        let json = serde_json::json!({
            "file": codeowners_path.display().to_string(),
            "diagnostics": diagnostics.iter().map(|d| {
                serde_json::json!({
                    "line": d.range.start.line + 1,
                    "column": d.range.start.character + 1,
                    "severity": match d.severity {
                        Some(DiagnosticSeverity::ERROR) => "error",
                        Some(DiagnosticSeverity::WARNING) => "warning",
                        Some(DiagnosticSeverity::HINT) => "hint",
                        Some(DiagnosticSeverity::INFORMATION) => "info",
                        _ => "unknown",
                    },
                    "code": d.code,
                    "message": d.message,
                })
            }).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&json).unwrap());
    } else {
        if diagnostics.is_empty() {
            println!(
                "{} {} - no issues found",
                "✓".green(),
                codeowners_path.display()
            );
            return ExitCode::SUCCESS;
        }

        println!("{}:", codeowners_path.display().to_string().bold());
        for d in &diagnostics {
            let (severity_label, severity_color) = match d.severity {
                Some(DiagnosticSeverity::ERROR) => ("error", "red"),
                Some(DiagnosticSeverity::WARNING) => ("warning", "yellow"),
                Some(DiagnosticSeverity::HINT) => ("hint", "cyan"),
                Some(DiagnosticSeverity::INFORMATION) => ("info", "blue"),
                _ => ("unknown", "white"),
            };
            let code = d
                .code
                .as_ref()
                .map(|c| match c {
                    NumberOrString::String(s) => s.clone(),
                    NumberOrString::Number(n) => n.to_string(),
                })
                .unwrap_or_default();
            println!(
                "  {} {} {} {}",
                format!("line {}:", d.range.start.line + 1).dimmed(),
                format!("[{}]", severity_label).color(severity_color),
                code.bold(),
                d.message
            );
        }
    }

    if should_fail(&diagnostics, strict) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// Validate owners and return diagnostics for invalid ones
async fn validate_owners_for_lint(
    content: &str,
    repo_root: &std::path::Path,
    token: &str,
) -> Vec<Diagnostic> {
    let lines = parse_codeowners_file_with_positions(content);
    let client = Arc::new(GitHubClient::new());

    // Load persistent cache and check staleness
    let persistent = PersistentCache::load(repo_root);
    let cache_was_stale = persistent.is_stale();

    if !cache_was_stale {
        // Load cached results into client
        client.load_from_persistent(&persistent);
    }

    // Collect owners with their line positions
    let mut owners_to_check: Vec<(String, u32, u32, u32)> = Vec::new(); // (owner, line, start, len)

    for line in &lines {
        if let CodeownersLine::Rule { owners, .. } = &line.content {
            let line_text = content.lines().nth(line.line_number as usize).unwrap_or("");
            let mut search_start = line.owners_start as usize;

            for owner in owners {
                if let Some(pos) = line_text[search_start..].find(owner) {
                    let char_start = (search_start + pos) as u32;
                    owners_to_check.push((
                        owner.clone(),
                        line.line_number,
                        char_start,
                        owner.len() as u32,
                    ));
                    search_start = search_start + pos + owner.len();
                }
            }
        }
    }

    // Dedupe owners for validation (but keep all positions for diagnostics)
    let unique_owners: std::collections::HashSet<_> = owners_to_check
        .iter()
        .map(|(o, _, _, _)| o.clone())
        .collect();

    // Validate uncached owners in parallel
    let uncached: Vec<_> = unique_owners
        .iter()
        .filter(|o| !client.is_cached(o))
        .cloned()
        .collect();

    if !uncached.is_empty() {
        let _: Vec<_> = stream::iter(uncached)
            .map(|owner| {
                let client = Arc::clone(&client);
                let token = token.to_string();
                async move {
                    let _ = client.validate_owner_with_info(&owner, &token).await;
                }
            })
            .buffer_unordered(CONCURRENCY)
            .collect()
            .await;

        // Save updated cache
        let _ = client.export_to_persistent().save(repo_root);
    }

    // Generate diagnostics for invalid owners
    let mut diagnostics = Vec::new();

    for (owner, line_num, char_start, owner_len) in owners_to_check {
        if let Some(false) = client.get_cached(&owner) {
            diagnostics.push(Diagnostic {
                range: Range {
                    start: Position {
                        line: line_num,
                        character: char_start,
                    },
                    end: Position {
                        line: line_num,
                        character: char_start + owner_len,
                    },
                },
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String("github-owner-not-found".to_string())),
                source: Some("codeowners".to_string()),
                message: format!("Owner '{}' not found on GitHub", owner),
                ..Default::default()
            });
        }
    }

    diagnostics
}

/// Determine exit code based on diagnostics and strict mode
fn should_fail(diagnostics: &[Diagnostic], strict: bool) -> bool {
    let has_errors = diagnostics
        .iter()
        .any(|d| matches!(d.severity, Some(DiagnosticSeverity::ERROR)));

    let has_warnings = diagnostics
        .iter()
        .any(|d| matches!(d.severity, Some(DiagnosticSeverity::WARNING)));

    has_errors || (strict && has_warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_diag(severity: DiagnosticSeverity) -> Diagnostic {
        Diagnostic {
            severity: Some(severity),
            ..Default::default()
        }
    }

    #[test]
    fn test_should_fail_no_diagnostics() {
        assert!(!should_fail(&[], false));
        assert!(!should_fail(&[], true));
    }

    #[test]
    fn test_should_fail_errors_always_fail() {
        let diags = vec![make_diag(DiagnosticSeverity::ERROR)];
        assert!(should_fail(&diags, false));
        assert!(should_fail(&diags, true));
    }

    #[test]
    fn test_should_fail_warnings_only_in_strict() {
        let diags = vec![make_diag(DiagnosticSeverity::WARNING)];
        assert!(!should_fail(&diags, false)); // Not strict, warnings OK
        assert!(should_fail(&diags, true)); // Strict, warnings fail
    }

    #[test]
    fn test_should_fail_hints_never_fail() {
        let diags = vec![make_diag(DiagnosticSeverity::HINT)];
        assert!(!should_fail(&diags, false));
        assert!(!should_fail(&diags, true));
    }

    #[test]
    fn test_should_fail_info_never_fail() {
        let diags = vec![make_diag(DiagnosticSeverity::INFORMATION)];
        assert!(!should_fail(&diags, false));
        assert!(!should_fail(&diags, true));
    }

    #[test]
    fn test_should_fail_mixed_severities() {
        // Error + warning = fail
        let diags = vec![
            make_diag(DiagnosticSeverity::ERROR),
            make_diag(DiagnosticSeverity::WARNING),
        ];
        assert!(should_fail(&diags, false));
        assert!(should_fail(&diags, true));

        // Warning + hint, non-strict = pass
        let diags = vec![
            make_diag(DiagnosticSeverity::WARNING),
            make_diag(DiagnosticSeverity::HINT),
        ];
        assert!(!should_fail(&diags, false));
        assert!(should_fail(&diags, true));
    }
}
