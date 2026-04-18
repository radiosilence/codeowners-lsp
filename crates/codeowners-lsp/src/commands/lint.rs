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

pub async fn lint(
    path: Option<PathBuf>,
    json_output: bool,
    fix: bool,
    strict: bool,
    github_actions: bool,
) -> ExitCode {
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

    if github_actions {
        // GitHub Actions annotations only (no human output)
        let file_path = codeowners_path.display();
        for d in &diagnostics {
            let level = match d.severity {
                Some(DiagnosticSeverity::ERROR) => "error",
                Some(DiagnosticSeverity::WARNING) => "warning",
                _ => "notice", // hints and info become notices
            };
            let line = d.range.start.line + 1;
            let col = d.range.start.character + 1;
            let title = d
                .code
                .as_ref()
                .map(|c| match c {
                    NumberOrString::String(s) => s.clone(),
                    NumberOrString::Number(n) => n.to_string(),
                })
                .unwrap_or_default();
            // Escape message for GitHub Actions (newlines become %0A)
            let message = d.message.replace('\n', "%0A").replace('\r', "%0D");
            println!("::{level} file={file_path},line={line},col={col},title={title}::{message}");
        }
    } else if json_output {
        // Parse content to get pattern/owners for each line
        let parsed_lines = parse_codeowners_file_with_positions(&content);
        let line_data: std::collections::HashMap<u32, (&str, &[String])> = parsed_lines
            .iter()
            .filter_map(|l| {
                if let CodeownersLine::Rule { pattern, owners } = &l.content {
                    Some((l.line_number, (pattern.as_str(), owners.as_slice())))
                } else {
                    None
                }
            })
            .collect();

        let json = serde_json::json!({
            "file": codeowners_path.display().to_string(),
            "diagnostics": diagnostics.iter().map(|d| {
                let line_num = d.range.start.line;
                let (pattern, owners) = line_data.get(&line_num).copied().unwrap_or(("", &[]));

                let mut obj = serde_json::json!({
                    "line": line_num + 1,
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
                });

                // Add pattern and owners if this diagnostic relates to a rule
                if !pattern.is_empty() {
                    obj["pattern"] = serde_json::json!(pattern);
                    obj["owners"] = serde_json::json!(owners);
                }

                obj
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

    owner_diagnostics_from_cache(&owners_to_check, &client)
}

/// Generate diagnostics for owners that are definitively Invalid (not Unknown).
fn owner_diagnostics_from_cache(
    owners_to_check: &[(String, u32, u32, u32)],
    client: &GitHubClient,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    for (owner, line_num, char_start, owner_len) in owners_to_check {
        if matches!(
            client.get_owner_info(owner),
            Some(crate::github::OwnerInfo::Invalid)
        ) {
            diagnostics.push(Diagnostic {
                range: Range {
                    start: Position {
                        line: *line_num,
                        character: *char_start,
                    },
                    end: Position {
                        line: *line_num,
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

    // =========================================================================
    // owner_diagnostics_from_cache tests
    // =========================================================================

    use crate::github::{GitHubClient, OwnerInfo, TeamInfo, UserInfo};

    fn make_client_with_cache(entries: Vec<(&str, OwnerInfo)>) -> GitHubClient {
        let client = GitHubClient::new();
        for (owner, info) in entries {
            client.insert_cached(owner, info);
        }
        client
    }

    #[test]
    fn test_invalid_owner_generates_diagnostic() {
        let client = make_client_with_cache(vec![("@ghost", OwnerInfo::Invalid)]);
        let owners = vec![("@ghost".to_string(), 5, 10, 6)];
        let diags = owner_diagnostics_from_cache(&owners, &client);

        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].message, "Owner '@ghost' not found on GitHub");
        assert!(matches!(
            diags[0].severity,
            Some(DiagnosticSeverity::WARNING)
        ));
        assert_eq!(diags[0].range.start.line, 5);
        assert_eq!(diags[0].range.start.character, 10);
    }

    /// Critical regression test: Unknown teams (404 ambiguous) must NOT
    /// generate "not found" diagnostics.
    #[test]
    fn test_unknown_team_does_not_generate_diagnostic() {
        let client = make_client_with_cache(vec![(
            "@org/invisible-team",
            OwnerInfo::Unknown("team not found or token lacks read:org scope".into()),
        )]);
        let owners = vec![("@org/invisible-team".to_string(), 3, 8, 19)];
        let diags = owner_diagnostics_from_cache(&owners, &client);

        assert!(
            diags.is_empty(),
            "Unknown teams must not produce diagnostics"
        );
    }

    #[test]
    fn test_valid_owner_does_not_generate_diagnostic() {
        let client = make_client_with_cache(vec![(
            "@alice",
            OwnerInfo::User(UserInfo {
                login: "alice".to_string(),
                name: None,
                html_url: "https://github.com/alice".to_string(),
                avatar_url: None,
                bio: None,
                company: None,
            }),
        )]);
        let owners = vec![("@alice".to_string(), 1, 5, 6)];
        let diags = owner_diagnostics_from_cache(&owners, &client);

        assert!(diags.is_empty());
    }

    #[test]
    fn test_uncached_owner_does_not_generate_diagnostic() {
        let client = GitHubClient::new(); // empty cache
        let owners = vec![("@org/not-yet-checked".to_string(), 2, 10, 20)];
        let diags = owner_diagnostics_from_cache(&owners, &client);

        assert!(
            diags.is_empty(),
            "Uncached owners should not be flagged as invalid"
        );
    }

    #[test]
    fn test_mixed_cache_states_only_invalid_flagged() {
        let client = make_client_with_cache(vec![
            (
                "@valid-user",
                OwnerInfo::User(UserInfo {
                    login: "valid-user".to_string(),
                    name: None,
                    html_url: "https://github.com/valid-user".to_string(),
                    avatar_url: None,
                    bio: None,
                    company: None,
                }),
            ),
            (
                "@org/valid-team",
                OwnerInfo::Team(TeamInfo {
                    slug: "valid-team".to_string(),
                    name: "Valid Team".to_string(),
                    org: "org".to_string(),
                    description: None,
                    html_url: "https://github.com/orgs/org/teams/valid-team".to_string(),
                    members_count: None,
                    repos_count: None,
                }),
            ),
            (
                "@org/unknown-team",
                OwnerInfo::Unknown("team not found or token lacks read:org scope".into()),
            ),
            ("@truly-gone", OwnerInfo::Invalid),
        ]);

        let owners = vec![
            ("@valid-user".to_string(), 1, 10, 11),
            ("@org/valid-team".to_string(), 2, 10, 15),
            ("@org/unknown-team".to_string(), 3, 10, 17),
            ("@truly-gone".to_string(), 4, 10, 11),
            ("@not-cached".to_string(), 5, 10, 11),
        ];
        let diags = owner_diagnostics_from_cache(&owners, &client);

        assert_eq!(
            diags.len(),
            1,
            "Only the Invalid owner should generate a diagnostic"
        );
        assert_eq!(diags[0].message, "Owner '@truly-gone' not found on GitHub");
        assert_eq!(diags[0].range.start.line, 4);
    }
}
