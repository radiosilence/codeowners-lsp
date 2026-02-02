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
        if covered_dirs
            .iter()
            .any(|d| parent_dir.starts_with(d.trim_end_matches('/')))
        {
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
    suggestions.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());

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

        let count: usize = parts[0].trim().parse().ok()?;
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

    // Determine suggested owner and confidence
    let top_contributor = &contributors[0];

    // Convert email to GitHub username format if possible
    let suggested_owner = email_to_owner(&top_contributor.email, &top_contributor.name);

    // Confidence based on:
    // - Top contributor's percentage of commits
    // - Total number of commits (more commits = more confidence)
    let percentage_factor = top_contributor.percentage / 100.0;
    let volume_factor = (total_commits as f64).min(100.0) / 100.0;
    let confidence = (percentage_factor * 0.7 + volume_factor * 0.3) * 100.0;

    Some(OwnerSuggestion {
        path: path.to_string(),
        suggested_owner,
        confidence,
        contributors,
        total_commits,
    })
}

/// Convert an email to a CODEOWNERS-compatible owner format
fn email_to_owner(email: &str, name: &str) -> String {
    // Check for common GitHub noreply format
    // e.g., "12345678+username@users.noreply.github.com"
    if email.contains("@users.noreply.github.com") {
        if let Some(username) = email
            .split('@')
            .next()
            .and_then(|s| s.split('+').next_back())
        {
            return format!("@{}", username);
        }
    }

    // Check for GitHub email pattern: username@github.com
    if email.ends_with("@github.com") {
        if let Some(username) = email.split('@').next() {
            return format!("@{}", username);
        }
    }

    // For other emails, try to extract a username-like string from the name
    // But ultimately use the email as-is since it's valid in CODEOWNERS
    let clean_name = name
        .to_lowercase()
        .replace(' ', "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>();

    if !clean_name.is_empty() && clean_name.len() >= 2 {
        // Suggest @username format but note it needs verification
        format!("@{}", clean_name)
    } else {
        // Fall back to email
        email.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_email_to_owner_github_noreply() {
        assert_eq!(
            email_to_owner("12345+octocat@users.noreply.github.com", "Octocat"),
            "@octocat"
        );
    }

    #[test]
    fn test_email_to_owner_github() {
        assert_eq!(email_to_owner("octocat@github.com", "Octocat"), "@octocat");
    }

    #[test]
    fn test_email_to_owner_regular() {
        assert_eq!(
            email_to_owner("john.doe@example.com", "John Doe"),
            "@john-doe"
        );
    }

    #[test]
    fn test_parse_shortlog() {
        let output = "    10\tAlice <alice@example.com>\n     5\tBob <bob@example.com>\n";
        let suggestion = parse_shortlog_output(output, "src/main.rs").unwrap();

        assert_eq!(suggestion.total_commits, 15);
        assert_eq!(suggestion.contributors.len(), 2);
        assert_eq!(suggestion.contributors[0].name, "Alice");
        assert_eq!(suggestion.contributors[0].commit_count, 10);
    }
}
