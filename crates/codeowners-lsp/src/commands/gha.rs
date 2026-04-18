//! GitHub Actions integration command - runs all checks and outputs GHA-formatted results.

use std::collections::HashSet;

use std::process::ExitCode;
use std::sync::Arc;
use std::{env, fs};

use colored::Colorize;
use futures::stream::{self, StreamExt};
use serde::Serialize;
use tower_lsp::lsp_types::{DiagnosticSeverity, NumberOrString};

use crate::diagnostics;
use crate::file_cache::FileCache;
use crate::github::{GitHubClient, PersistentCache};
use crate::ownership::{find_codeowners, get_repo_root};
use crate::parser::{parse_codeowners_file_with_positions, CodeownersLine};
use crate::pattern::pattern_matches;
use crate::settings::load_settings_from_path;

const CONCURRENCY: usize = 5;

/// Options for the gha command
pub struct GhaOptions {
    pub token: String,
    pub changed_files: Option<Vec<String>>,
    pub check_coverage_changed: bool,
    pub check_coverage_all: bool,
    pub check_owners_changed: bool,
    pub check_owners_all: bool,
    pub check_lint: bool,
    pub strict: bool, // Fail on warnings too (like lint --strict)
    // Output options
    pub output_annotations: bool,
    pub output_summary: bool,
    pub output_vars: bool,
}

/// Results from all checks
#[derive(Default, Serialize)]
struct GhaResults {
    coverage_changed: Option<CoverageResult>,
    coverage_all: Option<CoverageResult>,
    owners_changed: Option<OwnersResult>,
    owners_all: Option<OwnersResult>,
    lint: Option<LintResult>,
}

#[derive(Serialize)]
struct CoverageResult {
    total: usize,
    owned: usize,
    unowned: usize,
    coverage_percent: f64,
    unowned_files: Vec<String>,
}

#[derive(Serialize)]
struct OwnersResult {
    valid: Vec<String>,
    invalid: Vec<InvalidOwner>,
    unknown: Vec<InvalidOwner>,
}

#[derive(Serialize, Clone)]
struct InvalidOwner {
    owner: String,
    reason: String,
}

#[derive(Serialize)]
struct LintResult {
    file: String,
    diagnostics: Vec<LintDiagnostic>,
}

#[derive(Serialize)]
struct LintDiagnostic {
    line: u32,
    column: u32,
    severity: String,
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owners: Option<Vec<String>>,
}

pub async fn gha(opts: GhaOptions) -> ExitCode {
    let cwd = env::current_dir().expect("Failed to get current directory");

    let codeowners_path = match find_codeowners(&cwd) {
        Some(p) => p,
        None => {
            eprintln!("::error::No CODEOWNERS file found");
            return ExitCode::from(1);
        }
    };

    let content = match fs::read_to_string(&codeowners_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "::error::Failed to read {}: {}",
                codeowners_path.display(),
                e
            );
            return ExitCode::from(1);
        }
    };

    let repo_root = get_repo_root(&codeowners_path, &cwd);
    let file_cache = FileCache::new(&repo_root);
    let lines = parse_codeowners_file_with_positions(&content);

    let mut results = GhaResults::default();
    let mut failed = false;

    // === Coverage checks ===
    if opts.check_coverage_changed || opts.check_coverage_all {
        let all_unowned: Vec<String> = file_cache
            .get_unowned_files(&lines)
            .into_iter()
            .cloned()
            .collect();
        let total_files = file_cache.count_matches("*");

        // Changed files coverage (enforced)
        if opts.check_coverage_changed {
            if let Some(ref changed) = opts.changed_files {
                let changed_set: HashSet<&str> = changed.iter().map(|s| s.as_str()).collect();
                let unowned: Vec<String> = all_unowned
                    .iter()
                    .filter(|f| changed_set.contains(f.as_str()))
                    .cloned()
                    .collect();

                let owned = changed.len() - unowned.len();
                let pct = if changed.is_empty() {
                    100.0
                } else {
                    (owned as f64 / changed.len() as f64) * 100.0
                };

                if !unowned.is_empty() {
                    if opts.output_annotations {
                        eprintln!("::error::Changed files lack CODEOWNERS coverage");
                    }
                    failed = true;
                }

                results.coverage_changed = Some(CoverageResult {
                    total: changed.len(),
                    owned,
                    unowned: unowned.len(),
                    coverage_percent: (pct * 10.0).round() / 10.0,
                    unowned_files: unowned,
                });
            }
        }

        // All files coverage (warning only)
        if opts.check_coverage_all {
            let owned = total_files - all_unowned.len();
            let pct = if total_files == 0 {
                100.0
            } else {
                (owned as f64 / total_files as f64) * 100.0
            };

            if !all_unowned.is_empty() && opts.output_annotations {
                eprintln!(
                    "::warning::Some files lack CODEOWNERS coverage ({} unowned)",
                    all_unowned.len()
                );
            }

            results.coverage_all = Some(CoverageResult {
                total: total_files,
                owned,
                unowned: all_unowned.len(),
                coverage_percent: (pct * 10.0).round() / 10.0,
                unowned_files: all_unowned,
            });
        }
    }

    // === Owner validation ===
    if opts.check_owners_changed || opts.check_owners_all {
        let client = Arc::new(GitHubClient::new());

        // Load persistent cache
        let persistent = PersistentCache::load(&repo_root);
        if !persistent.is_stale() {
            client.load_from_persistent(&persistent);
        }

        // Collect all owners
        let mut all_owners: HashSet<String> = HashSet::new();
        let mut changed_owners: HashSet<String> = HashSet::new();

        for line in &lines {
            if let CodeownersLine::Rule {
                pattern,
                owners: line_owners,
            } = &line.content
            {
                for owner in line_owners {
                    all_owners.insert(owner.clone());

                    // Check if this rule matches any changed file
                    if let Some(ref changed) = opts.changed_files {
                        if changed.iter().any(|f| pattern_matches(pattern, f)) {
                            changed_owners.insert(owner.clone());
                        }
                    }
                }
            }
        }

        // Validate all unique owners
        let owners_to_validate: Vec<_> = all_owners.iter().cloned().collect();
        let uncached: Vec<_> = owners_to_validate
            .iter()
            .filter(|o| !client.is_cached(o))
            .cloned()
            .collect();

        if !uncached.is_empty() {
            let token = opts.token.clone();
            let _: Vec<_> = stream::iter(uncached)
                .map(|owner| {
                    let client = Arc::clone(&client);
                    let token = token.clone();
                    async move {
                        let _ = client.validate_owner(&owner, &token).await;
                    }
                })
                .buffer_unordered(CONCURRENCY)
                .collect()
                .await;

            // Save updated cache
            let _ = client.export_to_persistent().save(&repo_root);
        }

        let build_owners_result =
            |owners: &HashSet<String>| -> OwnersResult { classify_owners(owners, &client) };

        // Changed files owners (enforced)
        if opts.check_owners_changed && opts.changed_files.is_some() {
            let result = build_owners_result(&changed_owners);
            if !result.invalid.is_empty() {
                if opts.output_annotations {
                    eprintln!("::error::Invalid teams found in CODEOWNERS for changed files");
                }
                failed = true;
            } else if !result.unknown.is_empty() && opts.output_annotations {
                eprintln!(
                    "::warning::Could not verify {} team(s) in CODEOWNERS for changed files - check token permissions",
                    result.unknown.len()
                );
            }
            results.owners_changed = Some(result);
        }

        // All owners (warning only)
        if opts.check_owners_all {
            let result = build_owners_result(&all_owners);
            if !result.invalid.is_empty() && opts.output_annotations {
                eprintln!(
                    "::warning::Invalid teams found in CODEOWNERS ({} invalid)",
                    result.invalid.len()
                );
            } else if !result.unknown.is_empty() && opts.output_annotations {
                eprintln!(
                    "::warning::Could not verify {} team(s) in CODEOWNERS - check token permissions",
                    result.unknown.len()
                );
            }
            results.owners_all = Some(result);
        }
    }

    // === Lint check ===
    if opts.check_lint {
        let settings = load_settings_from_path(&repo_root);
        let diag_config = settings.diagnostic_config();
        let (mut diagnostics, _) =
            diagnostics::compute_diagnostics_sync(&content, Some(&file_cache), &diag_config);

        diagnostics.sort_by_key(|d| d.range.start.line);

        // Parse content for pattern/owners info
        let line_data: std::collections::HashMap<u32, (&str, &[String])> = lines
            .iter()
            .filter_map(|l| {
                if let CodeownersLine::Rule { pattern, owners } = &l.content {
                    Some((l.line_number, (pattern.as_str(), owners.as_slice())))
                } else {
                    None
                }
            })
            .collect();

        // Output GHA annotations
        if opts.output_annotations {
            let file_path = codeowners_path.display();
            for d in &diagnostics {
                let level = match d.severity {
                    Some(DiagnosticSeverity::ERROR) => "error",
                    Some(DiagnosticSeverity::WARNING) => "warning",
                    _ => "notice",
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
                let message = d.message.replace('\n', "%0A").replace('\r', "%0D");
                println!(
                    "::{level} file={file_path},line={line},col={col},title={title}::{message}"
                );
            }
        }

        // Check if lint should cause failure (errors always, warnings if strict)
        let has_errors = diagnostics
            .iter()
            .any(|d| matches!(d.severity, Some(DiagnosticSeverity::ERROR)));
        let has_warnings = diagnostics
            .iter()
            .any(|d| matches!(d.severity, Some(DiagnosticSeverity::WARNING)));

        if has_errors || (opts.strict && has_warnings) {
            failed = true;
        }

        let lint_diagnostics: Vec<LintDiagnostic> = diagnostics
            .iter()
            .map(|d| {
                let line_num = d.range.start.line;
                let (pattern, owners) = line_data.get(&line_num).copied().unwrap_or(("", &[]));

                LintDiagnostic {
                    line: line_num + 1,
                    column: d.range.start.character + 1,
                    severity: match d.severity {
                        Some(DiagnosticSeverity::ERROR) => "error",
                        Some(DiagnosticSeverity::WARNING) => "warning",
                        Some(DiagnosticSeverity::HINT) => "hint",
                        Some(DiagnosticSeverity::INFORMATION) => "info",
                        _ => "unknown",
                    }
                    .to_string(),
                    code: d
                        .code
                        .as_ref()
                        .map(|c| match c {
                            NumberOrString::String(s) => s.clone(),
                            NumberOrString::Number(n) => n.to_string(),
                        })
                        .unwrap_or_default(),
                    message: d.message.clone(),
                    pattern: if pattern.is_empty() {
                        None
                    } else {
                        Some(pattern.to_string())
                    },
                    owners: if owners.is_empty() {
                        None
                    } else {
                        Some(owners.to_vec())
                    },
                }
            })
            .collect();

        results.lint = Some(LintResult {
            file: codeowners_path.display().to_string(),
            diagnostics: lint_diagnostics,
        });
    }

    // === Output results ===
    output_results(&results, &opts, failed);

    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn output_results(results: &GhaResults, opts: &GhaOptions, failed: bool) {
    // Human-readable terminal output
    print_human_summary(results);

    // Write GitHub Actions outputs
    if opts.output_vars {
        if let Ok(output_file) = env::var("GITHUB_OUTPUT") {
            let mut outputs = Vec::new();

            // Coverage changed
            if let Some(ref cov) = results.coverage_changed {
                outputs.push(format!(
                    "has-coverage-issues={}",
                    if cov.unowned > 0 { "true" } else { "false" }
                ));
                outputs.push(format!(
                    "coverage-issues={}",
                    serde_json::to_string(&cov.unowned_files).unwrap()
                ));
            }

            // Lint
            if let Some(ref lint) = results.lint {
                outputs.push(format!(
                    "has-dead-entries={}",
                    if lint.diagnostics.is_empty() {
                        "false"
                    } else {
                        "true"
                    }
                ));
                outputs.push(format!(
                    "dead-entries={}",
                    serde_json::to_string(&lint.diagnostics).unwrap()
                ));
            }

            // Invalid teams (changed)
            if let Some(ref owners) = results.owners_changed {
                let has_issues = !owners.invalid.is_empty() || !owners.unknown.is_empty();
                outputs.push(format!(
                    "has-invalid-teams={}",
                    if has_issues { "true" } else { "false" }
                ));
                let issues: Vec<_> = owners.invalid.iter().chain(owners.unknown.iter()).collect();
                outputs.push(format!(
                    "invalid-teams={}",
                    serde_json::to_string(&issues).unwrap()
                ));
            }

            if let Ok(mut file) = fs::OpenOptions::new().append(true).open(&output_file) {
                use std::io::Write;
                for output in outputs {
                    let _ = writeln!(file, "{}", output);
                }
            }
        }
    }

    // Write GitHub Actions step summary
    if opts.output_summary {
        if let Ok(summary_file) = env::var("GITHUB_STEP_SUMMARY") {
            let summary = build_step_summary(results, failed);
            let _ = fs::write(&summary_file, summary);
        }
    }
}

fn print_human_summary(results: &GhaResults) {
    println!();
    println!("{}", "CODEOWNERS Check".bold());
    println!();

    // Coverage (changed files)
    if let Some(ref cov) = results.coverage_changed {
        if cov.unowned > 0 {
            println!(
                "  {} Coverage (changed): {}/{} files owned",
                "✗".red(),
                cov.owned.to_string().red(),
                cov.total
            );
            for file in cov.unowned_files.iter().take(5) {
                println!("      {} {}", "•".red(), file);
            }
            if cov.unowned_files.len() > 5 {
                println!(
                    "      {} ...and {} more",
                    "•".dimmed(),
                    cov.unowned_files.len() - 5
                );
            }
        } else {
            println!(
                "  {} Coverage (changed): {}/{} files owned",
                "✓".green(),
                cov.owned.to_string().green(),
                cov.total
            );
        }
    }

    // Coverage (all files)
    if let Some(ref cov) = results.coverage_all {
        if cov.unowned > 0 {
            println!(
                "  {} Coverage (all): {}/{} files owned ({} unowned)",
                "⚠".yellow(),
                cov.owned,
                cov.total,
                cov.unowned.to_string().yellow()
            );
        } else {
            println!(
                "  {} Coverage (all): {}/{} files owned",
                "✓".green(),
                cov.owned.to_string().green(),
                cov.total
            );
        }
    }

    // Owners (changed files)
    if let Some(ref owners) = results.owners_changed {
        if !owners.invalid.is_empty() {
            println!(
                "  {} Owners (changed): {} invalid",
                "✗".red(),
                owners.invalid.len().to_string().red()
            );
            for inv in &owners.invalid {
                println!(
                    "      {} {} ({})",
                    "•".red(),
                    inv.owner,
                    inv.reason.dimmed()
                );
            }
        } else if !owners.unknown.is_empty() {
            println!(
                "  {} Owners (changed): {} unverified",
                "⚠".yellow(),
                owners.unknown.len().to_string().yellow()
            );
            for inv in owners.unknown.iter().take(5) {
                println!(
                    "      {} {} ({})",
                    "•".yellow(),
                    inv.owner,
                    inv.reason.dimmed()
                );
            }
            if owners.unknown.len() > 5 {
                println!(
                    "      {} ...and {} more",
                    "•".dimmed(),
                    owners.unknown.len() - 5
                );
            }
        } else {
            println!(
                "  {} Owners (changed): {} valid",
                "✓".green(),
                owners.valid.len().to_string().green()
            );
        }
    }

    // Owners (all)
    if let Some(ref owners) = results.owners_all {
        if !owners.invalid.is_empty() {
            println!(
                "  {} Owners (all): {} invalid",
                "⚠".yellow(),
                owners.invalid.len().to_string().yellow()
            );
        } else if !owners.unknown.is_empty() {
            println!(
                "  {} Owners (all): {} unverified",
                "⚠".yellow(),
                owners.unknown.len().to_string().yellow()
            );
        } else {
            println!(
                "  {} Owners (all): {} valid",
                "✓".green(),
                owners.valid.len().to_string().green()
            );
        }
    }

    // Lint
    if let Some(ref lint) = results.lint {
        if lint.diagnostics.is_empty() {
            println!("  {} Lint: no issues", "✓".green());
        } else {
            let errors = lint
                .diagnostics
                .iter()
                .filter(|d| d.severity == "error")
                .count();
            let warnings = lint
                .diagnostics
                .iter()
                .filter(|d| d.severity == "warning")
                .count();
            if errors > 0 {
                println!(
                    "  {} Lint: {} errors, {} warnings",
                    "✗".red(),
                    errors.to_string().red(),
                    warnings
                );
            } else {
                println!(
                    "  {} Lint: {} warnings",
                    "⚠".yellow(),
                    warnings.to_string().yellow()
                );
            }
        }
    }

    println!();
}

fn build_step_summary(results: &GhaResults, failed: bool) -> String {
    let mut md = String::new();

    md.push_str("## CODEOWNERS Check Results\n\n");

    // Coverage (changed files)
    if let Some(ref cov) = results.coverage_changed {
        if cov.unowned > 0 {
            md.push_str("### :x: Uncovered Files\n");
            md.push_str("The following changed files need CODEOWNERS entries:\n\n");
            for file in &cov.unowned_files {
                md.push_str(&format!("- `{}`\n", file));
            }
            md.push('\n');
        } else {
            md.push_str("### :white_check_mark: Coverage\n");
            md.push_str("All changed files have CODEOWNERS coverage.\n\n");
        }
    }

    // Coverage (all files - warning only)
    if let Some(ref cov) = results.coverage_all {
        if cov.unowned > 0 {
            md.push_str("### :warning: Uncovered Files (all)\n");
            md.push_str("The following files in the repo lack CODEOWNERS coverage:\n\n");
            md.push_str("<details><summary>Show files</summary>\n\n");
            for file in &cov.unowned_files {
                md.push_str(&format!("- `{}`\n", file));
            }
            md.push_str("\n</details>\n\n");
        }
    }

    // Lint diagnostics
    if let Some(ref lint) = results.lint {
        if !lint.diagnostics.is_empty() {
            md.push_str("### :warning: Lint Issues\n");
            md.push_str("The following issues were found in CODEOWNERS:\n\n");
            md.push_str("| Severity | Line | Pattern | Owners | Issue |\n");
            md.push_str("|----------|------|---------|--------|-------|\n");
            for d in &lint.diagnostics {
                let pattern = d.pattern.as_deref().unwrap_or("-");
                let owners = d
                    .owners
                    .as_ref()
                    .map(|o| o.join(", "))
                    .unwrap_or_else(|| "-".to_string());
                md.push_str(&format!(
                    "| {} | {} | `{}` | {} | {} |\n",
                    d.severity, d.line, pattern, owners, d.code
                ));
            }
            md.push('\n');
        } else {
            md.push_str("### :white_check_mark: No Lint Issues\n");
            md.push_str("CODEOWNERS file is clean.\n\n");
        }
    }

    // Invalid teams (changed files)
    if let Some(ref owners) = results.owners_changed {
        let issues: Vec<_> = owners.invalid.iter().chain(owners.unknown.iter()).collect();
        if !issues.is_empty() {
            md.push_str("### :x: Invalid Teams (changed files)\n");
            md.push_str(
                "The following teams/users for changed files are invalid or lack repository access:\n\n",
            );
            md.push_str("| Team | Reason |\n");
            md.push_str("|------|--------|\n");
            for issue in &issues {
                md.push_str(&format!("| `{}` | {} |\n", issue.owner, issue.reason));
            }
            md.push('\n');
        } else {
            md.push_str("### :white_check_mark: Teams Valid (changed files)\n");
            md.push_str("All teams/users for changed files exist and have repository access.\n\n");
        }
    }

    // Invalid teams (all - warning only)
    if let Some(ref owners) = results.owners_all {
        let issues: Vec<_> = owners.invalid.iter().chain(owners.unknown.iter()).collect();
        if !issues.is_empty() {
            md.push_str("### :warning: Invalid Teams (all)\n");
            md.push_str(
                "The following teams/users in CODEOWNERS are invalid or lack repository access:\n\n",
            );
            md.push_str("<details><summary>Show teams</summary>\n\n");
            md.push_str("| Team | Reason |\n");
            md.push_str("|------|--------|\n");
            for issue in &issues {
                md.push_str(&format!("| `{}` | {} |\n", issue.owner, issue.reason));
            }
            md.push_str("\n</details>\n\n");
        }
    }

    // Final status
    if failed {
        md.push_str("---\n\n:x: **Check failed**\n");
    } else {
        md.push_str("---\n\n:white_check_mark: **All checks passed!**\n");
    }

    md
}

/// Classify a set of owners into valid/invalid/unknown based on cached GitHub info.
fn classify_owners(owners: &HashSet<String>, client: &GitHubClient) -> OwnersResult {
    let mut valid = Vec::new();
    let mut invalid = Vec::new();
    let mut unknown = Vec::new();

    for owner in owners {
        match client.get_owner_info(owner) {
            Some(crate::github::OwnerInfo::User(_)) | Some(crate::github::OwnerInfo::Team(_)) => {
                valid.push(owner.clone())
            }
            Some(crate::github::OwnerInfo::Invalid) => invalid.push(InvalidOwner {
                owner: owner.clone(),
                reason: "not found on GitHub".to_string(),
            }),
            Some(crate::github::OwnerInfo::Unknown(ref reason)) => {
                unknown.push(InvalidOwner {
                    owner: owner.clone(),
                    reason: reason.clone(),
                });
            }
            None => {
                let reason = if owner.contains('@') && !owner.starts_with('@') {
                    "email, can't validate".to_string()
                } else {
                    "owner not validated".to_string()
                };
                unknown.push(InvalidOwner {
                    owner: owner.clone(),
                    reason,
                });
            }
        }
    }

    valid.sort();
    invalid.sort_by(|a, b| a.owner.cmp(&b.owner));
    unknown.sort_by(|a, b| a.owner.cmp(&b.owner));

    OwnersResult {
        valid,
        invalid,
        unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{GitHubClient, OwnerInfo, TeamInfo, UserInfo};

    fn make_client_with_cache(entries: Vec<(&str, OwnerInfo)>) -> GitHubClient {
        let client = GitHubClient::new();
        for (owner, info) in entries {
            client.insert_cached(owner, info);
        }
        client
    }

    fn user_info(login: &str) -> OwnerInfo {
        OwnerInfo::User(UserInfo {
            login: login.to_string(),
            name: None,
            html_url: format!("https://github.com/{login}"),
            avatar_url: None,
            bio: None,
            company: None,
        })
    }

    fn team_info(org: &str, slug: &str) -> OwnerInfo {
        OwnerInfo::Team(TeamInfo {
            slug: slug.to_string(),
            name: slug.to_string(),
            org: org.to_string(),
            description: None,
            html_url: format!("https://github.com/orgs/{org}/teams/{slug}"),
            members_count: None,
            repos_count: None,
        })
    }

    #[test]
    fn test_classify_valid_user() {
        let client = make_client_with_cache(vec![("@alice", user_info("alice"))]);
        let owners: HashSet<String> = ["@alice".to_string()].into();
        let result = classify_owners(&owners, &client);

        assert_eq!(result.valid, vec!["@alice"]);
        assert!(result.invalid.is_empty());
        assert!(result.unknown.is_empty());
    }

    #[test]
    fn test_classify_valid_team() {
        let client = make_client_with_cache(vec![("@myorg/myteam", team_info("myorg", "myteam"))]);
        let owners: HashSet<String> = ["@myorg/myteam".to_string()].into();
        let result = classify_owners(&owners, &client);

        assert_eq!(result.valid, vec!["@myorg/myteam"]);
        assert!(result.invalid.is_empty());
        assert!(result.unknown.is_empty());
    }

    #[test]
    fn test_classify_invalid_user() {
        let client = make_client_with_cache(vec![("@ghost", OwnerInfo::Invalid)]);
        let owners: HashSet<String> = ["@ghost".to_string()].into();
        let result = classify_owners(&owners, &client);

        assert!(result.valid.is_empty());
        assert_eq!(result.invalid.len(), 1);
        assert_eq!(result.invalid[0].owner, "@ghost");
        assert_eq!(result.invalid[0].reason, "not found on GitHub");
        assert!(result.unknown.is_empty());
    }

    /// This is THE critical test: team 404s are Unknown (ambiguous), not Invalid.
    /// They must NOT appear in the invalid bucket or cause CI failures.
    #[test]
    fn test_classify_unknown_team_not_treated_as_invalid() {
        let client = make_client_with_cache(vec![
            (
                "@surgeventures/team-zen",
                OwnerInfo::Unknown("team not found or token lacks read:org scope".into()),
            ),
            (
                "@surgeventures/tribe-growth",
                OwnerInfo::Unknown("team not found or token lacks read:org scope".into()),
            ),
        ]);
        let owners: HashSet<String> = [
            "@surgeventures/team-zen".to_string(),
            "@surgeventures/tribe-growth".to_string(),
        ]
        .into();
        let result = classify_owners(&owners, &client);

        // MUST be in unknown, NOT invalid
        assert!(result.valid.is_empty());
        assert!(
            result.invalid.is_empty(),
            "Unknown teams must not be classified as invalid"
        );
        assert_eq!(result.unknown.len(), 2);
        for u in &result.unknown {
            assert_eq!(u.reason, "team not found or token lacks read:org scope");
        }
    }

    #[test]
    fn test_classify_uncached_owner_goes_to_unknown() {
        let client = GitHubClient::new(); // empty cache
        let owners: HashSet<String> = ["@org/uncached-team".to_string()].into();
        let result = classify_owners(&owners, &client);

        assert!(result.valid.is_empty());
        assert!(result.invalid.is_empty());
        assert_eq!(result.unknown.len(), 1);
        assert_eq!(result.unknown[0].owner, "@org/uncached-team");
        assert_eq!(result.unknown[0].reason, "owner not validated");
    }

    #[test]
    fn test_classify_email_gets_email_reason() {
        let client = GitHubClient::new(); // emails can't be validated
        let owners: HashSet<String> = ["user@example.com".to_string()].into();
        let result = classify_owners(&owners, &client);

        assert!(result.valid.is_empty());
        assert!(result.invalid.is_empty());
        assert_eq!(result.unknown.len(), 1);
        assert_eq!(result.unknown[0].reason, "email, can't validate");
    }

    #[test]
    fn test_classify_mixed_states() {
        let client = make_client_with_cache(vec![
            ("@valid-user", user_info("valid-user")),
            ("@org/valid-team", team_info("org", "valid-team")),
            (
                "@org/unknown-team",
                OwnerInfo::Unknown("team not found or token lacks read:org scope".into()),
            ),
            ("@nonexistent", OwnerInfo::Invalid),
        ]);
        let owners: HashSet<String> = [
            "@valid-user".to_string(),
            "@org/valid-team".to_string(),
            "@org/unknown-team".to_string(),
            "@nonexistent".to_string(),
            "uncached@email.com".to_string(),
        ]
        .into();
        let result = classify_owners(&owners, &client);

        assert_eq!(result.valid.len(), 2);
        assert!(result.valid.contains(&"@valid-user".to_string()));
        assert!(result.valid.contains(&"@org/valid-team".to_string()));

        assert_eq!(result.invalid.len(), 1);
        assert_eq!(result.invalid[0].owner, "@nonexistent");

        assert_eq!(result.unknown.len(), 2);
        let unknown_owners: Vec<&str> = result.unknown.iter().map(|u| u.owner.as_str()).collect();
        assert!(unknown_owners.contains(&"@org/unknown-team"));
        assert!(unknown_owners.contains(&"uncached@email.com"));
    }

    /// Regression test: only invalid owners should cause CI failure, not unknown.
    #[test]
    fn test_only_invalid_causes_failure_not_unknown() {
        // Scenario: all owners are Unknown (team 404s) — should NOT fail
        let client = make_client_with_cache(vec![
            (
                "@org/team-a",
                OwnerInfo::Unknown("team not found or token lacks read:org scope".into()),
            ),
            (
                "@org/team-b",
                OwnerInfo::Unknown("team not found or token lacks read:org scope".into()),
            ),
        ]);
        let owners: HashSet<String> = ["@org/team-a".to_string(), "@org/team-b".to_string()].into();
        let result = classify_owners(&owners, &client);
        assert!(
            result.invalid.is_empty(),
            "No invalid owners means no CI failure"
        );

        // Scenario: mix of unknown and one truly invalid — should fail
        let client = make_client_with_cache(vec![
            (
                "@org/team-a",
                OwnerInfo::Unknown("team not found or token lacks read:org scope".into()),
            ),
            ("@definitely-fake", OwnerInfo::Invalid),
        ]);
        let owners: HashSet<String> =
            ["@org/team-a".to_string(), "@definitely-fake".to_string()].into();
        let result = classify_owners(&owners, &client);
        assert_eq!(
            result.invalid.len(),
            1,
            "One invalid should trigger failure"
        );
        assert_eq!(result.unknown.len(), 1, "Unknown should be separate");
    }
}
