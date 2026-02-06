//! Git blame analysis for suggesting code owners based on commit history.
//!
//! This module analyzes git history to determine who the most frequent
//! contributors are to files and directories, which helps suggest
//! appropriate code owners.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// Statistics about a contributor's involvement with a file or directory
#[derive(Debug, Clone)]
pub struct ContributorStats {
    /// Git author email
    pub email: String,
    /// Git author name
    pub name: String,
    /// Number of commits touching this path
    pub commit_count: usize,
    /// Percentage of total commits (0-100)
    pub percentage: f64,
}

/// Suggested owner for a path based on git history
#[derive(Debug, Clone)]
pub struct OwnerSuggestion {
    /// The file or directory path
    pub path: String,
    /// Suggested owner in CODEOWNERS format (@user or email)
    pub suggested_owner: String,
    /// Confidence score (0-100)
    pub confidence: f64,
    /// Top contributors with their stats
    pub contributors: Vec<ContributorStats>,
    /// Total commits analyzed
    pub total_commits: usize,
}

/// Analyze git blame/log for a single file
pub fn analyze_file(repo_root: &Path, file_path: &str) -> Option<OwnerSuggestion> {
    let full_path = repo_root.join(file_path);
    if !full_path.exists() {
        return None;
    }

    // Use git shortlog to get commit counts per author
    let output = Command::new("git")
        .args(["shortlog", "-sne", "--no-merges", "HEAD", "--", file_path])
        .current_dir(repo_root)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_shortlog_output(&stdout, file_path)
}

/// Analyze git history for a directory (all files within)
pub fn analyze_directory(repo_root: &Path, dir_path: &str) -> Option<OwnerSuggestion> {
    // Normalize directory path
    let dir_pattern = if dir_path.ends_with('/') {
        format!("{}*", dir_path)
    } else {
        format!("{}/*", dir_path)
    };

    let output = Command::new("git")
        .args([
            "shortlog",
            "-sne",
            "--no-merges",
            "HEAD",
            "--",
            &dir_pattern,
        ])
        .current_dir(repo_root)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_shortlog_output(&stdout, dir_path)
}

/// Analyze multiple files and aggregate results by directory
pub fn analyze_files_by_directory(
    repo_root: &Path,
    files: &[String],
) -> HashMap<String, OwnerSuggestion> {
    // Group files by their parent directory
    let mut dir_files: HashMap<String, Vec<String>> = HashMap::new();

    for file in files {
        let dir = Path::new(file)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let dir = if dir.is_empty() { "/".to_string() } else { dir };

        dir_files.entry(dir).or_default().push(file.clone());
    }

    // Analyze each directory
    let mut results = HashMap::new();

    for dir in dir_files.keys() {
        if let Some(suggestion) = analyze_directory(repo_root, dir) {
            results.insert(dir.clone(), suggestion);
        }
    }

    results
}

/// Batch analyze unowned files and suggest owners
pub fn suggest_owners_for_files(
    repo_root: &Path,
    unowned_files: &[String],
    min_confidence: f64,
) -> Vec<OwnerSuggestion> {
    let mut suggestions = Vec::new();

    // First try to get directory-level suggestions
    let dir_suggestions = analyze_files_by_directory(repo_root, unowned_files);

    // For directories with good confidence, use directory suggestion
    let mut covered_dirs: Vec<String> = Vec::new();
    for (dir, suggestion) in &dir_suggestions {
        if suggestion.confidence >= min_confidence {
            let mut dir_suggestion = suggestion.clone();
            // Convert to directory pattern
            dir_suggestion.path = if dir == "/" {
                "*".to_string()
            } else {
                format!("{}/", dir)
            };
            covered_dirs.push(dir_suggestion.path.clone());
            suggestions.push(dir_suggestion);
        }
    }

    // For remaining files not covered by directory suggestions, analyze individually
    for file in unowned_files {
        let parent_dir = Path::new(file)
            .parent()
            .map(|p| format!("{}/", p.to_string_lossy()))
            .unwrap_or_default();

        // Skip if parent directory already has a suggestion
        // Compare with trailing / to avoid "src-extra/" matching "src/"
        if covered_dirs.iter().any(|d| {
            let prefix = d.trim_end_matches('/');
            parent_dir == format!("{}/", prefix) || parent_dir.starts_with(&format!("{}/", prefix))
        }) {
            continue;
        }

        // Analyze individual file
        if let Some(suggestion) = analyze_file(repo_root, file) {
            if suggestion.confidence >= min_confidence {
                suggestions.push(suggestion);
            }
        }
    }

    // Sort by confidence (highest first)
    suggestions.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));

    suggestions
}

/// Parse git shortlog output into contributor stats
fn parse_shortlog_output(output: &str, path: &str) -> Option<OwnerSuggestion> {
    let mut contributors: Vec<ContributorStats> = Vec::new();
    let mut total_commits = 0usize;

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Format: "   123\tName <email>"
        let parts: Vec<&str> = line.splitn(2, '\t').collect();
        if parts.len() != 2 {
            continue;
        }

        let count: usize = match parts[0].trim().parse() {
            Ok(c) => c,
            Err(_) => continue, // skip malformed lines instead of aborting all results
        };
        let author = parts[1].trim();

        // Parse "Name <email>"
        let (name, email) = if let Some(start) = author.find('<') {
            if let Some(end) = author.find('>') {
                let name = author[..start].trim().to_string();
                let email = author[start + 1..end].to_string();
                (name, email)
            } else {
                (author.to_string(), String::new())
            }
        } else {
            (author.to_string(), String::new())
        };

        total_commits += count;
        contributors.push(ContributorStats {
            email,
            name,
            commit_count: count,
            percentage: 0.0, // Will calculate after
        });
    }

    if contributors.is_empty() {
        return None;
    }

    // Calculate percentages
    for contrib in &mut contributors {
        contrib.percentage = (contrib.commit_count as f64 / total_commits as f64) * 100.0;
    }

    // Sort by commit count (highest first)
    contributors.sort_by(|a, b| b.commit_count.cmp(&a.commit_count));

    // Determine confidence
    let top_contributor = &contributors[0];

    // Confidence based on:
    // - Top contributor's percentage of commits
    // - Total number of commits (more commits = more confidence)
    let percentage_factor = top_contributor.percentage / 100.0;
    let volume_factor = (total_commits as f64).min(100.0) / 100.0;
    let confidence = (percentage_factor * 0.7 + volume_factor * 0.3) * 100.0;

    Some(OwnerSuggestion {
        path: path.to_string(),
        suggested_owner: String::new(), // Filled by suggest command using lookup
        confidence,
        contributors,
        total_commits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_shortlog() {
        let output = "    10\tAlice <alice@example.com>\n     5\tBob <bob@example.com>\n";
        let suggestion = parse_shortlog_output(output, "src/main.rs").unwrap();

        assert_eq!(suggestion.total_commits, 15);
        assert_eq!(suggestion.contributors.len(), 2);
        assert_eq!(suggestion.contributors[0].name, "Alice");
        assert_eq!(suggestion.contributors[0].commit_count, 10);
    }

    #[test]
    fn test_parse_shortlog_single_contributor() {
        let output = "   100\tSolo Dev <solo@example.com>\n";
        let suggestion = parse_shortlog_output(output, "file.rs").unwrap();

        assert_eq!(suggestion.total_commits, 100);
        assert_eq!(suggestion.contributors.len(), 1);
        assert_eq!(suggestion.contributors[0].email, "solo@example.com");
        assert_eq!(suggestion.contributors[0].percentage, 100.0);
        // High confidence with 100% ownership and 100 commits
        assert!(suggestion.confidence > 90.0);
    }

    #[test]
    fn test_parse_shortlog_empty() {
        let output = "";
        let result = parse_shortlog_output(output, "file.rs");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_shortlog_whitespace_only() {
        let output = "   \n   \n";
        let result = parse_shortlog_output(output, "file.rs");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_shortlog_malformed_line() {
        // Missing tab separator
        let output = "10 Alice <alice@example.com>\n";
        let result = parse_shortlog_output(output, "file.rs");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_shortlog_no_email() {
        let output = "    5\tJust A Name\n";
        let suggestion = parse_shortlog_output(output, "file.rs").unwrap();

        assert_eq!(suggestion.contributors.len(), 1);
        assert_eq!(suggestion.contributors[0].name, "Just A Name");
        assert_eq!(suggestion.contributors[0].email, "");
    }

    #[test]
    fn test_parse_shortlog_sorting() {
        // Bob has more commits, should be first
        let output = "     5\tAlice <alice@example.com>\n    20\tBob <bob@example.com>\n     3\tCharlie <charlie@example.com>\n";
        let suggestion = parse_shortlog_output(output, "file.rs").unwrap();

        assert_eq!(suggestion.contributors[0].name, "Bob");
        assert_eq!(suggestion.contributors[0].commit_count, 20);
        assert_eq!(suggestion.contributors[1].name, "Alice");
        assert_eq!(suggestion.contributors[2].name, "Charlie");
    }

    #[test]
    fn test_parse_shortlog_percentage_calculation() {
        let output = "    75\tMajority <maj@example.com>\n    25\tMinority <min@example.com>\n";
        let suggestion = parse_shortlog_output(output, "file.rs").unwrap();

        assert_eq!(suggestion.contributors[0].percentage, 75.0);
        assert_eq!(suggestion.contributors[1].percentage, 25.0);
    }

    #[test]
    fn test_parse_shortlog_confidence_low_commits() {
        // Only 2 commits - low volume factor
        let output = "     2\tDev <dev@example.com>\n";
        let suggestion = parse_shortlog_output(output, "file.rs").unwrap();

        // 100% ownership but only 2 commits, confidence should be moderate
        assert!(suggestion.confidence < 80.0);
    }

    #[test]
    fn test_parse_shortlog_confidence_split_ownership() {
        // 50/50 split - lower confidence
        let output = "    50\tAlice <alice@example.com>\n    50\tBob <bob@example.com>\n";
        let suggestion = parse_shortlog_output(output, "file.rs").unwrap();

        // Only 50% top contributor, confidence should be lower
        assert!(suggestion.confidence < 70.0);
    }

    #[test]
    fn test_parse_shortlog_path_preserved() {
        let output = "    10\tDev <dev@example.com>\n";
        let suggestion = parse_shortlog_output(output, "src/deep/nested/file.rs").unwrap();

        assert_eq!(suggestion.path, "src/deep/nested/file.rs");
    }

    #[test]
    fn test_contributor_stats_fields() {
        let output = "   42\tTest User <test.user@company.com>\n";
        let suggestion = parse_shortlog_output(output, "test.rs").unwrap();

        let contrib = &suggestion.contributors[0];
        assert_eq!(contrib.email, "test.user@company.com");
        assert_eq!(contrib.name, "Test User");
        assert_eq!(contrib.commit_count, 42);
        assert_eq!(contrib.percentage, 100.0);
    }

    #[test]
    fn test_parse_shortlog_unclosed_bracket() {
        // Email with only opening bracket
        let output = "    5\tUser <email@test.com\n";
        let suggestion = parse_shortlog_output(output, "file.rs").unwrap();

        // Should treat whole thing as name
        assert_eq!(suggestion.contributors[0].name, "User <email@test.com");
        assert_eq!(suggestion.contributors[0].email, "");
    }

    #[test]
    fn test_parse_shortlog_invalid_count() {
        // Non-numeric commit count - skipped, no valid lines remain
        let output = "   abc\tUser <user@test.com>\n";
        let result = parse_shortlog_output(output, "file.rs");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_shortlog_mixed_valid_invalid() {
        // First line valid, second has invalid count - should skip bad line, keep good one
        let output = "    10\tAlice <alice@test.com>\n   bad\tBob <bob@test.com>\n";
        let result = parse_shortlog_output(output, "file.rs");
        assert!(result.is_some());
        let suggestion = result.unwrap();
        assert_eq!(suggestion.contributors.len(), 1);
        assert_eq!(suggestion.contributors[0].name, "Alice");
        assert_eq!(suggestion.total_commits, 10);
    }

    #[test]
    fn test_owner_suggestion_default_suggested_owner() {
        let output = "    10\tDev <dev@test.com>\n";
        let suggestion = parse_shortlog_output(output, "file.rs").unwrap();

        // suggested_owner is empty by default, filled later by suggest command
        assert_eq!(suggestion.suggested_owner, "");
    }

    #[test]
    fn test_parse_shortlog_all_invalid_counts() {
        // Every line has bad count - should return None
        let output = "   abc\tAlice <a@test.com>\n   xyz\tBob <b@test.com>\n";
        assert!(parse_shortlog_output(output, "file.rs").is_none());
    }

    #[test]
    fn test_parse_shortlog_invalid_between_valid() {
        // Bad line sandwiched between good ones - only good lines kept
        let output =
            "    10\tAlice <a@test.com>\n   bad\tEvil <e@test.com>\n     5\tBob <b@test.com>\n";
        let suggestion = parse_shortlog_output(output, "file.rs").unwrap();
        assert_eq!(suggestion.contributors.len(), 2);
        assert_eq!(suggestion.total_commits, 15);
        // Alice has more commits, should be first after sort
        assert_eq!(suggestion.contributors[0].name, "Alice");
        assert_eq!(suggestion.contributors[1].name, "Bob");
    }

    #[test]
    fn test_confidence_calculation_max_volume() {
        // 100+ commits maxes out volume factor at 1.0
        let output = "   150\tDev <dev@test.com>\n";
        let suggestion = parse_shortlog_output(output, "file.rs").unwrap();

        // 100% ownership (0.7) + max volume (0.3) = 100%
        assert!(suggestion.confidence >= 99.0);
    }
}
