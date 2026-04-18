//! Optimize command - suggests ways to simplify CODEOWNERS patterns.
//!
//! Analyzes existing rules and suggests:
//! - Consolidating multiple file patterns into directory patterns
//! - Removing redundant/shadowed rules
//! - Using more specific globs instead of listing files

use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;
use std::{env, fs};

use colored::Colorize;

use crate::file_cache::FileCache;
use crate::ownership::{find_codeowners, get_repo_root};
use crate::parser::{self, CodeownersLine, ParsedLine};

/// A suggested optimization
#[derive(Debug, Clone)]
pub struct Optimization {
    /// Type of optimization
    pub kind: OptimizationKind,
    /// Lines that would be replaced (0-indexed)
    pub affected_lines: Vec<u32>,
    /// Current patterns being replaced
    pub current_patterns: Vec<String>,
    /// Suggested replacement pattern
    pub suggested_pattern: String,
    /// Owners for the new pattern
    pub owners: Vec<String>,
    /// Explanation of the optimization
    pub reason: String,
    /// Number of files covered
    pub files_covered: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OptimizationKind {
    /// Multiple files in same dir → directory pattern
    ConsolidateToDirectory,
    /// Redundant rule (shadowed by a later rule)
    RemoveRedundant,
}

/// Output format for optimizations
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputFormat {
    /// Human-readable suggestions
    Human,
    /// JSON for CI/tooling
    Json,
}

/// Options for the optimize command
#[derive(Debug, Clone)]
pub struct OptimizeOptions {
    /// Output format (human or json)
    pub format: OutputFormat,
    /// Minimum files to suggest directory consolidation
    pub min_files_for_dir: usize,
    /// Write changes to file (false = preview only)
    pub write: bool,
}

impl Default for OptimizeOptions {
    fn default() -> Self {
        Self {
            format: OutputFormat::Human,
            min_files_for_dir: 3,
            write: false,
        }
    }
}

pub fn optimize(options: OptimizeOptions) -> ExitCode {
    let cwd = env::current_dir().expect("Failed to get current directory");

    let codeowners_path = match find_codeowners(&cwd) {
        Some(p) => p,
        None => {
            eprintln!("{} No CODEOWNERS file found", "Error:".red().bold());
            return ExitCode::from(1);
        }
    };

    let content = match fs::read_to_string(&codeowners_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "{} Failed to read {}: {}",
                "Error:".red().bold(),
                codeowners_path.display(),
                e
            );
            return ExitCode::from(1);
        }
    };

    let repo_root = get_repo_root(&codeowners_path, &cwd);
    let file_cache = FileCache::new(&repo_root);
    let lines = parser::parse_codeowners_file_with_positions(&content);

    // Find optimizations
    let optimizations = find_optimizations(&lines, &file_cache, &options);

    if optimizations.is_empty() {
        match options.format {
            OutputFormat::Human => {
                println!("{} CODEOWNERS file is already optimized!", "✓".green());
            }
            OutputFormat::Json => {
                println!("{{\"optimizations\": [], \"message\": \"Already optimized\"}}");
            }
        }
        return ExitCode::SUCCESS;
    }

    // Output based on format
    match options.format {
        OutputFormat::Human => output_human(&optimizations),
        OutputFormat::Json => output_json(&optimizations),
    }

    // Apply changes if --write
    if options.write {
        let optimized = apply_optimizations(&content, &lines, &optimizations);
        if let Err(e) = fs::write(&codeowners_path, &optimized) {
            eprintln!(
                "{} Failed to write {}: {}",
                "Error:".red().bold(),
                codeowners_path.display(),
                e
            );
            return ExitCode::from(1);
        }
        println!("\n{} Written to {}", "✓".green(), codeowners_path.display());
    }

    ExitCode::SUCCESS
}

fn find_optimizations(
    lines: &[ParsedLine],
    file_cache: &FileCache,
    options: &OptimizeOptions,
) -> Vec<Optimization> {
    let mut optimizations = Vec::new();

    // 1. Find patterns that match no files (dead rules)
    optimizations.extend(find_no_match_rules(lines, file_cache));

    // 2. Find redundant/shadowed rules (also dead code)
    optimizations.extend(find_redundant_rules(lines));

    // 3. Find directories where ALL children have same owners (safe consolidation)
    optimizations.extend(find_directory_consolidations(lines, file_cache, options));

    optimizations
}

/// Find rules with patterns that don't match any files in the repository
fn find_no_match_rules(lines: &[ParsedLine], file_cache: &FileCache) -> Vec<Optimization> {
    let mut optimizations = Vec::new();

    for parsed_line in lines {
        if let CodeownersLine::Rule { pattern, owners } = &parsed_line.content {
            if !file_cache.has_matches(pattern) {
                optimizations.push(Optimization {
                    kind: OptimizationKind::RemoveRedundant,
                    affected_lines: vec![parsed_line.line_number],
                    current_patterns: vec![pattern.clone()],
                    suggested_pattern: String::new(),
                    owners: owners.clone(),
                    reason: format!("Pattern '{}' matches no files", pattern),
                    files_covered: 0,
                });
            }
        }
    }

    optimizations
}

/// Find directories where ALL children have the exact same owners.
///
/// Only suggests consolidation when:
/// 1. ALL files in a directory are explicitly listed with rules
/// 2. ALL those rules have the EXACT same owners
/// 3. There are at least `min_files_for_dir` files
/// 4. The consolidated pattern would NOT be shadowed by a later rule
///
/// The consolidated pattern is placed at the FIRST affected line position.
#[allow(clippy::type_complexity)]
fn find_directory_consolidations(
    lines: &[ParsedLine],
    file_cache: &FileCache,
    options: &OptimizeOptions,
) -> Vec<Optimization> {
    use crate::pattern::{pattern_matches, pattern_subsumes};

    let mut optimizations = Vec::new();

    // Collect ALL rules for shadow checking
    let all_rules: Vec<(u32, &str)> = lines
        .iter()
        .filter_map(|line| {
            if let CodeownersLine::Rule { pattern, .. } = &line.content {
                Some((line.line_number, pattern.as_str()))
            } else {
                None
            }
        })
        .collect();

    // Collect all explicit file rules (not globs, not directories)
    let file_rules: Vec<(u32, &str, &[String], bool)> = lines
        .iter()
        .filter_map(|line| {
            if let CodeownersLine::Rule { pattern, owners } = &line.content {
                // Skip patterns that are already directories or globs
                if pattern.ends_with('/') || pattern.contains('*') {
                    return None;
                }
                let anchored = pattern.starts_with('/');
                Some((
                    line.line_number,
                    pattern.as_str(),
                    owners.as_slice(),
                    anchored,
                ))
            } else {
                None
            }
        })
        .collect();

    // Group by parent directory
    let mut dir_to_rules: HashMap<String, Vec<(u32, &str, &[String], bool)>> = HashMap::new();

    for (line_num, pattern, owners, anchored) in &file_rules {
        let clean_pattern = pattern.trim_start_matches('/');
        if let Some(parent) = Path::new(clean_pattern).parent() {
            let dir = parent.to_string_lossy().to_string();
            if !dir.is_empty() {
                dir_to_rules
                    .entry(dir)
                    .or_default()
                    .push((*line_num, *pattern, *owners, *anchored));
            }
        }
    }

    // Check each directory
    for (dir, rules) in dir_to_rules {
        if rules.len() < options.min_files_for_dir {
            continue;
        }

        // Check if ALL rules have the same owners
        let first_owners = rules[0].2;
        let all_same_owners = rules
            .iter()
            .all(|(_, _, owners, _)| *owners == first_owners);
        if !all_same_owners {
            continue;
        }

        // Check if ALL files in directory are covered by these rules
        // Use anchored pattern to get files at this exact directory level
        let dir_glob = format!("/{}/*", dir);
        let all_files_in_dir = file_cache.get_matches(&dir_glob);

        if all_files_in_dir.is_empty() {
            continue;
        }

        // Check each file is matched by exactly one of our rules
        let rule_patterns: Vec<&str> = rules.iter().map(|(_, p, _, _)| *p).collect();
        let all_covered = all_files_in_dir.iter().all(|file| {
            rule_patterns
                .iter()
                .any(|pattern| pattern_matches(pattern, file))
        });

        if !all_covered {
            continue;
        }

        // Check we're not covering MORE than just this directory's direct children
        // (avoid over-consolidation)
        if rules.len() != all_files_in_dir.len() {
            continue;
        }

        // Preserve anchoring from the first pattern
        let first_anchored = rules[0].3;
        let suggested_pattern = if first_anchored {
            format!("/{}/", dir)
        } else {
            format!("{}/", dir)
        };

        // Find the LAST line number of the rules being consolidated
        let last_affected_line = rules.iter().map(|(l, _, _, _)| *l).max().unwrap_or(0);

        // Check if the consolidated pattern would be shadowed by any LATER rule
        let would_be_shadowed = all_rules
            .iter()
            .filter(|(line_num, _)| *line_num > last_affected_line)
            .any(|(_, later_pattern)| pattern_subsumes(&suggested_pattern, later_pattern));

        if would_be_shadowed {
            // Don't suggest consolidation - the individual files will be marked
            // as redundant anyway, and we'd just be creating a new shadowed rule
            continue;
        }

        optimizations.push(Optimization {
            kind: OptimizationKind::ConsolidateToDirectory,
            affected_lines: rules.iter().map(|(l, _, _, _)| *l).collect(),
            current_patterns: rules.iter().map(|(_, p, _, _)| p.to_string()).collect(),
            suggested_pattern,
            owners: first_owners.to_vec(),
            reason: format!("All {} files in {} have same owners", rules.len(), dir),
            files_covered: all_files_in_dir.len(),
        });
    }

    optimizations
}

/// Find rules that are shadowed by later rules
///
/// A rule is "dead" if a later rule subsumes it (last match wins).
/// Examples:
/// - `docs/ @a` followed by `* @b` → docs/ is dead
/// - `/src/auth/ @a` followed by `/src/ @b` → /src/auth/ is dead
/// - Duplicate patterns: first occurrence is dead
///
/// Works backwards from end of file. Assumes ALL rules are shadowed by default,
/// then "rescues" rules that are NOT subsumed by any later pattern.
fn find_redundant_rules(lines: &[ParsedLine]) -> Vec<Optimization> {
    use crate::pattern::pattern_subsumes;
    use std::collections::{HashMap, HashSet};

    // Collect all rules with their line numbers
    let rules: Vec<(u32, &str, &[String])> = lines
        .iter()
        .filter_map(|line| {
            if let CodeownersLine::Rule { pattern, owners } = &line.content {
                Some((line.line_number, pattern.as_str(), owners.as_slice()))
            } else {
                None
            }
        })
        .collect();

    // Start with ALL rules marked for deletion
    let mut to_delete: HashSet<u32> = rules.iter().map(|(line_num, _, _)| *line_num).collect();

    // Track which pattern shadows each rule (for error messages)
    let mut shadow_info: HashMap<u32, (u32, String)> = HashMap::new();

    // Work backwards, building up "later_patterns" as we go
    let mut later_patterns: Vec<(u32, &str)> = Vec::new();

    for (line_num, pattern, _) in rules.iter().rev() {
        // Check if this pattern is subsumed by ANY later pattern
        let mut shadowed_by: Option<(u32, &str)> = None;
        for (later_line_num, later_pattern) in &later_patterns {
            if pattern_subsumes(pattern, later_pattern) {
                shadowed_by = Some((*later_line_num, *later_pattern));
                break;
            }
        }

        if let Some((later_line, later_pat)) = shadowed_by {
            // Pattern IS subsumed - keep it marked for deletion, record shadow info
            shadow_info.insert(*line_num, (later_line, later_pat.to_string()));
        } else {
            // Pattern is NOT subsumed - rescue it from deletion
            to_delete.remove(line_num);
        }

        // Always add to later_patterns (even if subsumed, it can still shadow earlier rules)
        later_patterns.push((*line_num, *pattern));
    }

    // Convert to optimizations
    let mut optimizations = Vec::new();
    for (line_num, pattern, owners) in &rules {
        if to_delete.contains(line_num) {
            let reason = if let Some((later_line, later_pat)) = shadow_info.get(line_num) {
                if pattern.trim_start_matches('/') == later_pat.trim_start_matches('/')
                    && pattern.starts_with('/') == later_pat.starts_with('/')
                {
                    format!(
                        "Duplicate pattern - line {} shadows line {}",
                        later_line + 1,
                        line_num + 1
                    )
                } else {
                    format!(
                        "Pattern '{}' on line {} is shadowed by '{}' on line {}",
                        pattern,
                        line_num + 1,
                        later_pat,
                        later_line + 1
                    )
                }
            } else {
                // Shouldn't happen, but fallback
                format!("Pattern '{}' on line {} is shadowed", pattern, line_num + 1)
            };

            optimizations.push(Optimization {
                kind: OptimizationKind::RemoveRedundant,
                affected_lines: vec![*line_num],
                current_patterns: vec![pattern.to_string()],
                suggested_pattern: String::new(),
                owners: owners.to_vec(),
                reason,
                files_covered: 0,
            });
        }
    }

    optimizations
}

fn output_human(optimizations: &[Optimization]) {
    println!(
        "{} Found {} optimization{}:\n",
        "✓".green(),
        optimizations.len(),
        if optimizations.len() == 1 { "" } else { "s" }
    );

    for (i, opt) in optimizations.iter().enumerate() {
        let kind_str = match opt.kind {
            OptimizationKind::ConsolidateToDirectory => "📁 Directory consolidation",
            OptimizationKind::RemoveRedundant => "🗑️  Remove shadowed rule",
        };

        println!("{}. {}", (i + 1).to_string().bold(), kind_str.cyan());
        println!("   {}", opt.reason.dimmed());

        if !opt.current_patterns.is_empty() && opt.current_patterns.len() <= 5 {
            println!("   {}:", "Current".yellow());
            for pattern in &opt.current_patterns {
                println!("     {}", pattern.dimmed());
            }
        } else if !opt.current_patterns.is_empty() {
            println!(
                "   {}: {} patterns on lines {:?}",
                "Current".yellow(),
                opt.current_patterns.len(),
                opt.affected_lines.iter().map(|l| l + 1).collect::<Vec<_>>()
            );
        }

        if !opt.suggested_pattern.is_empty() {
            println!(
                "   {}: {} {}",
                "Suggested".green(),
                opt.suggested_pattern.green().bold(),
                opt.owners.join(" ").green()
            );
        }

        println!();
    }
}

/// Apply optimizations to generate new file content
/// Replaces the first affected line with the optimized pattern, deletes the rest
fn apply_optimizations(
    content: &str,
    _lines: &[ParsedLine],
    optimizations: &[Optimization],
) -> String {
    use std::collections::HashMap;

    // Build a map of line_num -> replacement (if any)
    // For each optimization, the FIRST affected line gets the replacement,
    // other affected lines are deleted (mapped to None)
    let mut line_actions: HashMap<u32, Option<String>> = HashMap::new();

    for opt in optimizations {
        if opt.affected_lines.is_empty() {
            continue;
        }

        // Sort affected lines to find the first one
        let mut sorted_lines = opt.affected_lines.clone();
        sorted_lines.sort();

        // First affected line gets the replacement (if there is one)
        if !opt.suggested_pattern.is_empty() {
            let replacement = format!("{} {}", opt.suggested_pattern, opt.owners.join(" "));
            line_actions.insert(sorted_lines[0], Some(replacement));
        } else {
            // No replacement (e.g., RemoveRedundant) - just delete
            line_actions.insert(sorted_lines[0], None);
        }

        // Rest of affected lines are deleted
        for &line_num in &sorted_lines[1..] {
            line_actions.insert(line_num, None);
        }
    }

    // Build result
    let mut result = String::new();
    for (i, line) in content.lines().enumerate() {
        let line_num = i as u32;

        if let Some(action) = line_actions.get(&line_num) {
            // This line is affected by an optimization
            if let Some(replacement) = action {
                result.push_str(replacement);
                result.push('\n');
            }
            // else: line is deleted, skip it
        } else {
            // Keep original line
            result.push_str(line);
            result.push('\n');
        }
    }

    result
}

fn output_json(optimizations: &[Optimization]) {
    let json_opts: Vec<serde_json::Value> = optimizations
        .iter()
        .map(|o| {
            serde_json::json!({
                "kind": format!("{:?}", o.kind),
                "affected_lines": o.affected_lines,
                "current_patterns": o.current_patterns,
                "suggested_pattern": o.suggested_pattern,
                "owners": o.owners,
                "reason": o.reason,
                "files_covered": o.files_covered
            })
        })
        .collect();

    let output = serde_json::json!({
        "optimization_count": optimizations.len(),
        "optimizations": json_opts
    });

    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== apply_optimizations tests ====================

    #[test]
    fn test_apply_optimizations_replaces_in_place() {
        let content = "src/a.rs @alice\nsrc/b.rs @alice\nsrc/c.rs @alice\n* @default\n";

        let opt = Optimization {
            kind: OptimizationKind::ConsolidateToDirectory,
            affected_lines: vec![0, 1, 2],
            current_patterns: vec![
                "src/a.rs".to_string(),
                "src/b.rs".to_string(),
                "src/c.rs".to_string(),
            ],
            suggested_pattern: "src/".to_string(),
            owners: vec!["@alice".to_string()],
            reason: "test".to_string(),
            files_covered: 3,
        };

        let result = apply_optimizations(content, &[], &[opt]);

        // Should replace first line, delete others, keep catch-all at end
        assert_eq!(result, "src/ @alice\n* @default\n");
    }

    #[test]
    fn test_apply_optimizations_preserves_order_with_catchall() {
        let content = "# Header\nlib/x.rs @bob\nlib/y.rs @bob\n* @default\n# Footer\n";

        let opt = Optimization {
            kind: OptimizationKind::ConsolidateToDirectory,
            affected_lines: vec![1, 2],
            current_patterns: vec!["lib/x.rs".to_string(), "lib/y.rs".to_string()],
            suggested_pattern: "lib/".to_string(),
            owners: vec!["@bob".to_string()],
            reason: "test".to_string(),
            files_covered: 2,
        };

        let result = apply_optimizations(content, &[], &[opt]);

        // Optimized pattern should be where lib/x.rs was, before catch-all
        assert_eq!(result, "# Header\nlib/ @bob\n* @default\n# Footer\n");
    }

    #[test]
    fn test_apply_optimizations_remove_redundant() {
        let content = "*.rs @rust\n* @default\n*.rs @maintainer\n";

        let opt = Optimization {
            kind: OptimizationKind::RemoveRedundant,
            affected_lines: vec![0], // First *.rs is shadowed
            current_patterns: vec!["*.rs".to_string()],
            suggested_pattern: String::new(), // No replacement, just delete
            owners: vec!["@rust".to_string()],
            reason: "shadowed".to_string(),
            files_covered: 0,
        };

        let result = apply_optimizations(content, &[], &[opt]);

        // Should just remove the shadowed line
        assert_eq!(result, "* @default\n*.rs @maintainer\n");
    }

    #[test]
    fn test_apply_optimizations_multiple() {
        let content = "src/a.rs @a\nsrc/b.rs @a\nlib/c.rs @b\nlib/d.rs @b\n";

        let opts = vec![
            Optimization {
                kind: OptimizationKind::ConsolidateToDirectory,
                affected_lines: vec![0, 1],
                current_patterns: vec!["src/a.rs".to_string(), "src/b.rs".to_string()],
                suggested_pattern: "src/".to_string(),
                owners: vec!["@a".to_string()],
                reason: "test".to_string(),
                files_covered: 2,
            },
            Optimization {
                kind: OptimizationKind::ConsolidateToDirectory,
                affected_lines: vec![2, 3],
                current_patterns: vec!["lib/c.rs".to_string(), "lib/d.rs".to_string()],
                suggested_pattern: "lib/".to_string(),
                owners: vec!["@b".to_string()],
                reason: "test".to_string(),
                files_covered: 2,
            },
        ];

        let result = apply_optimizations(content, &[], &opts);

        // Each group replaced in place
        assert_eq!(result, "src/ @a\nlib/ @b\n");
    }

    #[test]
    fn test_apply_optimizations_non_contiguous_lines() {
        // Lines 0, 2, 4 are affected (with comments/other rules in between)
        let content =
            "src/a.rs @alice\n# comment\nsrc/b.rs @alice\nother.rs @bob\nsrc/c.rs @alice\n";

        let opt = Optimization {
            kind: OptimizationKind::ConsolidateToDirectory,
            affected_lines: vec![0, 2, 4],
            current_patterns: vec![
                "src/a.rs".to_string(),
                "src/b.rs".to_string(),
                "src/c.rs".to_string(),
            ],
            suggested_pattern: "src/".to_string(),
            owners: vec!["@alice".to_string()],
            reason: "test".to_string(),
            files_covered: 3,
        };

        let result = apply_optimizations(content, &[], &[opt]);

        // First affected line (0) gets replacement, others deleted, unaffected lines preserved
        assert_eq!(result, "src/ @alice\n# comment\nother.rs @bob\n");
    }

    #[test]
    fn test_apply_optimizations_unsorted_affected_lines() {
        // affected_lines not in order - should still use the lowest line number
        let content = "line0 @a\nline1 @a\nline2 @a\n";

        let opt = Optimization {
            kind: OptimizationKind::ConsolidateToDirectory,
            affected_lines: vec![2, 0, 1], // Deliberately unsorted
            current_patterns: vec![],
            suggested_pattern: "combined/".to_string(),
            owners: vec!["@a".to_string()],
            reason: "test".to_string(),
            files_covered: 3,
        };

        let result = apply_optimizations(content, &[], &[opt]);

        // Should replace line 0 (the lowest), delete 1 and 2
        assert_eq!(result, "combined/ @a\n");
    }

    #[test]
    fn test_apply_optimizations_empty_file() {
        let content = "";
        let result = apply_optimizations(content, &[], &[]);
        assert_eq!(result, "");
    }

    #[test]
    fn test_apply_optimizations_no_optimizations() {
        let content = "src/ @alice\n* @default\n";
        let result = apply_optimizations(content, &[], &[]);
        assert_eq!(result, "src/ @alice\n* @default\n");
    }

    #[test]
    fn test_apply_optimizations_preserves_multiple_owners() {
        let content = "src/a.rs @alice @bob\nsrc/b.rs @alice @bob\n";

        let opt = Optimization {
            kind: OptimizationKind::ConsolidateToDirectory,
            affected_lines: vec![0, 1],
            current_patterns: vec!["src/a.rs".to_string(), "src/b.rs".to_string()],
            suggested_pattern: "src/".to_string(),
            owners: vec!["@alice".to_string(), "@bob".to_string()],
            reason: "test".to_string(),
            files_covered: 2,
        };

        let result = apply_optimizations(content, &[], &[opt]);
        assert_eq!(result, "src/ @alice @bob\n");
    }

    #[test]
    fn test_apply_optimizations_complex_real_world() {
        // Simulates a real CODEOWNERS with sections, comments, catch-all
        let content = r#"# Frontend
/src/components/Button.tsx @frontend
/src/components/Modal.tsx @frontend
/src/components/Form.tsx @frontend

# Backend
/api/users.rs @backend
/api/auth.rs @backend

# Catch-all
* @maintainers
"#;

        let opts = vec![
            Optimization {
                kind: OptimizationKind::ConsolidateToDirectory,
                affected_lines: vec![1, 2, 3],
                current_patterns: vec![
                    "/src/components/Button.tsx".to_string(),
                    "/src/components/Modal.tsx".to_string(),
                    "/src/components/Form.tsx".to_string(),
                ],
                suggested_pattern: "/src/components/".to_string(),
                owners: vec!["@frontend".to_string()],
                reason: "test".to_string(),
                files_covered: 3,
            },
            Optimization {
                kind: OptimizationKind::ConsolidateToDirectory,
                affected_lines: vec![6, 7],
                current_patterns: vec!["/api/users.rs".to_string(), "/api/auth.rs".to_string()],
                suggested_pattern: "/api/".to_string(),
                owners: vec!["@backend".to_string()],
                reason: "test".to_string(),
                files_covered: 2,
            },
        ];

        let result = apply_optimizations(content, &[], &opts);

        let expected = r#"# Frontend
/src/components/ @frontend

# Backend
/api/ @backend

# Catch-all
* @maintainers
"#;
        assert_eq!(result, expected);
    }

    #[test]
    fn test_apply_optimizations_single_line_delete() {
        let content = "keep.rs @a\ndelete.rs @b\nkeep2.rs @c\n";

        let opt = Optimization {
            kind: OptimizationKind::RemoveRedundant,
            affected_lines: vec![1],
            current_patterns: vec!["delete.rs".to_string()],
            suggested_pattern: String::new(),
            owners: vec!["@b".to_string()],
            reason: "test".to_string(),
            files_covered: 0,
        };

        let result = apply_optimizations(content, &[], &[opt]);
        assert_eq!(result, "keep.rs @a\nkeep2.rs @c\n");
    }

    // ==================== find_redundant_rules tests ====================

    fn make_parsed_line(line_number: u32, pattern: &str, owners: Vec<&str>) -> ParsedLine {
        ParsedLine {
            line_number,
            content: CodeownersLine::Rule {
                pattern: pattern.to_string(),
                owners: owners.into_iter().map(|s| s.to_string()).collect(),
            },
            pattern_start: 0,
            pattern_end: pattern.len() as u32,
            owners_start: pattern.len() as u32 + 1,
            comment_start: None,
        }
    }

    #[test]
    fn test_find_redundant_rules_duplicate_pattern() {
        let lines = vec![
            make_parsed_line(0, "*.rs", vec!["@first"]),
            make_parsed_line(1, "*.rs", vec!["@second"]),
        ];

        let result = find_redundant_rules(&lines);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].affected_lines, vec![0]); // First one is shadowed
        assert_eq!(result[0].kind, OptimizationKind::RemoveRedundant);
    }

    #[test]
    fn test_find_redundant_rules_anchored_not_same_as_unanchored() {
        // /src/foo and src/foo are NOT the same pattern!
        // /src/foo is anchored to root, src/foo (without trailing /) is also anchored
        // but they have different semantics for directory patterns
        let lines = vec![
            make_parsed_line(0, "/src/foo", vec!["@first"]),
            make_parsed_line(1, "src/foo", vec!["@second"]),
        ];

        // Both are exact path patterns (no trailing /), both anchored to root
        // so src/foo subsumes /src/foo (identical after normalization)
        let result = find_redundant_rules(&lines);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_find_redundant_rules_unanchored_dir_not_shadowed_by_anchored() {
        // docs/ (unanchored) matches MORE than /docs/ (anchored)
        // So /docs/ does NOT shadow docs/
        let lines = vec![
            make_parsed_line(0, "docs/", vec!["@team"]), // matches anywhere
            make_parsed_line(1, "/docs/", vec!["@root"]), // only matches root
        ];

        let result = find_redundant_rules(&lines);
        assert!(result.is_empty()); // docs/ is NOT shadowed
    }

    #[test]
    fn test_find_redundant_rules_anchored_shadowed_by_unanchored() {
        // /docs/ (anchored) IS shadowed by docs/ (unanchored)
        // because docs/ matches everything /docs/ matches (and more)
        let lines = vec![
            make_parsed_line(0, "/docs/", vec!["@root"]), // only matches root
            make_parsed_line(1, "docs/", vec!["@team"]),  // matches anywhere, including root
        ];

        let result = find_redundant_rules(&lines);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].affected_lines, vec![0]); // /docs/ is dead
    }

    #[test]
    fn test_find_redundant_rules_shadowed_by_catchall() {
        // Classic footgun: specific patterns before catch-all
        let lines = vec![
            make_parsed_line(0, "docs/", vec!["@docs"]),
            make_parsed_line(1, "*.rs", vec!["@rust"]),
            make_parsed_line(2, "*", vec!["@default"]),
        ];

        let result = find_redundant_rules(&lines);
        assert_eq!(result.len(), 2); // Both docs/ and *.rs are shadowed by *
        assert!(result.iter().any(|o| o.affected_lines == vec![0]));
        assert!(result.iter().any(|o| o.affected_lines == vec![1]));
    }

    #[test]
    fn test_find_redundant_rules_nested_dir_shadowed() {
        // /src/auth/ is shadowed by /src/
        let lines = vec![
            make_parsed_line(0, "/src/auth/", vec!["@security"]),
            make_parsed_line(1, "/src/", vec!["@backend"]),
        ];

        let result = find_redundant_rules(&lines);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].affected_lines, vec![0]);
    }

    #[test]
    fn test_find_redundant_rules_no_duplicates() {
        let lines = vec![
            make_parsed_line(0, "*.rs", vec!["@rust"]),
            make_parsed_line(1, "*.ts", vec!["@ts"]),
        ];

        let result = find_redundant_rules(&lines);
        assert!(result.is_empty());
    }

    #[test]
    fn test_find_redundant_rules_correct_order_not_redundant() {
        // Correct order: general first, specific last
        let lines = vec![
            make_parsed_line(0, "*", vec!["@default"]),
            make_parsed_line(1, "docs/", vec!["@docs"]),
            make_parsed_line(2, "*.rs", vec!["@rust"]),
        ];

        let result = find_redundant_rules(&lines);
        assert!(result.is_empty()); // Nothing is shadowed when ordered correctly
    }

    #[test]
    fn test_find_redundant_rules_complex_with_catchall() {
        // Simulate real-world CODEOWNERS with many diverse patterns ending in catch-all
        // This tests the scenario where optimize --write needed 2 passes
        let lines = vec![
            // Patterns without owners (ignored/generated files)
            make_parsed_line(0, "**/.env.*", vec![]),
            make_parsed_line(1, "*.mo", vec![]),
            make_parsed_line(2, "*.po", vec![]),
            // Extension patterns
            make_parsed_line(3, "*.jpg", vec!["@design"]),
            make_parsed_line(4, "*.png", vec!["@design"]),
            // Anchored directories
            make_parsed_line(5, "/.github/workflows/", vec!["@devops"]),
            make_parsed_line(6, "/src/apps/helpers/", vec!["@platform"]),
            make_parsed_line(7, "/src/apps/platform/", vec!["@platform"]),
            // Anchored exact paths
            make_parsed_line(8, "/README.md", vec!["@docs"]),
            make_parsed_line(9, "/src/mix.exs", vec!["@core"]),
            // Unanchored directories
            make_parsed_line(10, "docs/", vec!["@docs"]),
            make_parsed_line(11, "test/", vec!["@qa"]),
            // Nested anchored directories
            make_parsed_line(12, "/src/apps/helpers/lib/", vec!["@helpers"]),
            make_parsed_line(13, "/src/apps/platform/lib/", vec!["@platform-lib"]),
            // Catch-all at the end - shadows EVERYTHING above
            make_parsed_line(14, "*", vec!["@default"]),
        ];

        let result = find_redundant_rules(&lines);

        // ALL 14 patterns before * should be detected as shadowed
        assert_eq!(
            result.len(),
            14,
            "Expected 14 shadowed rules, got: {} - lines: {:?}",
            result.len(),
            result.iter().map(|o| &o.affected_lines).collect::<Vec<_>>()
        );

        // Verify each line is marked
        for line in 0..14 {
            assert!(
                result.iter().any(|o| o.affected_lines == vec![line]),
                "Line {} should be marked as shadowed",
                line
            );
        }
    }

    #[test]
    fn test_find_redundant_rules_intermediate_shadowing() {
        // Test case where A is shadowed by B, and B is also shadowed by C
        // Both A and B should be detected in a single pass
        let lines = vec![
            make_parsed_line(0, "/src/apps/helpers/lib/config/", vec!["@config"]),
            make_parsed_line(1, "/src/apps/helpers/lib/", vec!["@lib"]),
            make_parsed_line(2, "/src/apps/helpers/", vec!["@helpers"]),
            make_parsed_line(3, "/src/apps/", vec!["@apps"]),
            make_parsed_line(4, "/src/", vec!["@src"]),
            make_parsed_line(5, "*", vec!["@default"]),
        ];

        let result = find_redundant_rules(&lines);

        // All 5 patterns before * should be detected
        // Even though there's a chain of subsumption
        assert_eq!(
            result.len(),
            5,
            "Expected 5 shadowed rules, got: {} - reasons: {:?}",
            result.len(),
            result.iter().map(|o| &o.reason).collect::<Vec<_>>()
        );
    }
}
