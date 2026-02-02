/// Represents a parsed line from a CODEOWNERS file with position info
#[derive(Debug, Clone)]
pub struct ParsedLine {
    pub line_number: u32,
    pub content: CodeownersLine,
    /// Character offset where pattern starts
    pub pattern_start: u32,
    /// Character offset where pattern ends
    pub pattern_end: u32,
    /// Character offset where owners start
    pub owners_start: u32,
}

/// Represents a parsed line from a CODEOWNERS file
#[derive(Debug, Clone, PartialEq)]
pub enum CodeownersLine {
    /// A comment line (starts with #)
    Comment(String),
    /// An empty line
    Empty,
    /// A rule with pattern and owners
    Rule {
        pattern: String,
        owners: Vec<String>,
    },
}

impl std::fmt::Display for CodeownersLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodeownersLine::Comment(c) => write!(f, "{}", c),
            CodeownersLine::Empty => Ok(()),
            CodeownersLine::Rule { pattern, owners } => {
                write!(f, "{} {}", pattern, owners.join(" "))
            }
        }
    }
}

/// Parse a CODEOWNERS file into structured lines with position info
pub fn parse_codeowners_file_with_positions(content: &str) -> Vec<ParsedLine> {
    content
        .lines()
        .enumerate()
        .map(|(line_num, line)| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                ParsedLine {
                    line_number: line_num as u32,
                    content: CodeownersLine::Empty,
                    pattern_start: 0,
                    pattern_end: 0,
                    owners_start: 0,
                }
            } else if trimmed.starts_with('#') {
                ParsedLine {
                    line_number: line_num as u32,
                    content: CodeownersLine::Comment(line.to_string()),
                    pattern_start: 0,
                    pattern_end: 0,
                    owners_start: 0,
                }
            } else {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.is_empty() {
                    ParsedLine {
                        line_number: line_num as u32,
                        content: CodeownersLine::Empty,
                        pattern_start: 0,
                        pattern_end: 0,
                        owners_start: 0,
                    }
                } else {
                    // Find pattern position
                    let pattern_start = line.find(parts[0]).unwrap_or(0) as u32;
                    let pattern_end = pattern_start + parts[0].len() as u32;
                    let owners_start = if parts.len() > 1 {
                        line.find(parts[1]).unwrap_or(pattern_end as usize) as u32
                    } else {
                        pattern_end
                    };

                    ParsedLine {
                        line_number: line_num as u32,
                        content: CodeownersLine::Rule {
                            pattern: parts[0].to_string(),
                            owners: parts[1..].iter().map(|s| s.to_string()).collect(),
                        },
                        pattern_start,
                        pattern_end,
                        owners_start,
                    }
                }
            }
        })
        .collect()
}

/// Parse a CODEOWNERS file into structured lines (without positions)
#[allow(dead_code)]
pub fn parse_codeowners_file(content: &str) -> Vec<CodeownersLine> {
    parse_codeowners_file_with_positions(content)
        .into_iter()
        .map(|p| p.content)
        .collect()
}

/// Write parsed lines back to a string
#[allow(dead_code)]
pub fn serialize_codeowners(lines: &[CodeownersLine]) -> String {
    lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Find the best insertion point for a new pattern
/// Uses heuristics to find a sensible location:
/// 1. Path similarity: near rules with similar directory prefixes
/// 2. Before catch-all rules (*, /**)
/// 3. End of file as fallback
#[allow(dead_code)]
pub fn find_insertion_point(lines: &[CodeownersLine], pattern: &str) -> usize {
    find_insertion_point_with_owner(lines, pattern, None)
}

/// Find the best insertion point considering both path and owner
#[allow(dead_code)]
pub fn find_insertion_point_with_owner(
    lines: &[CodeownersLine],
    pattern: &str,
    owner: Option<&str>,
) -> usize {
    if lines.is_empty() {
        return 0;
    }

    // Extract directory prefix from the pattern (e.g., "/src/foo/bar.rs" -> "/src/foo")
    let pattern_dir = get_directory_prefix(pattern);

    // Track best match by path similarity
    let mut best_path_match: Option<(usize, usize)> = None; // (index, depth)

    // Track owner clusters
    let mut owner_lines: Vec<usize> = Vec::new();

    // Track catch-all position
    let mut catch_all_idx: Option<usize> = None;

    for (idx, line) in lines.iter().enumerate() {
        if let CodeownersLine::Rule {
            pattern: rule_pattern,
            owners,
        } = line
        {
            // Check for catch-all patterns
            if rule_pattern == "*" || rule_pattern == "/*" || rule_pattern == "/**" {
                if catch_all_idx.is_none() {
                    catch_all_idx = Some(idx);
                }
                continue;
            }

            // Check path similarity
            let rule_dir = get_directory_prefix(rule_pattern);
            let common_depth = common_prefix_depth(&pattern_dir, &rule_dir);

            if common_depth > 0 {
                if let Some((_, best_depth)) = best_path_match {
                    if common_depth > best_depth {
                        best_path_match = Some((idx, common_depth));
                    }
                } else {
                    best_path_match = Some((idx, common_depth));
                }
            }

            // Track owner occurrences
            if let Some(target_owner) = owner {
                if owners.iter().any(|o| o == target_owner) {
                    owner_lines.push(idx);
                }
            }
        }
    }

    // Priority 1: Insert after the best path match (similar directory)
    if let Some((idx, _)) = best_path_match {
        // Insert after the matched rule
        return idx + 1;
    }

    // Priority 2: Insert near owner cluster (after last occurrence)
    if let Some(&last_owner_idx) = owner_lines.last() {
        return last_owner_idx + 1;
    }

    // Priority 3: Insert before catch-all rules
    if let Some(idx) = catch_all_idx {
        return idx;
    }

    // Fallback: end of file
    lines.len()
}

/// Extract directory prefix from a pattern
/// "/src/foo/bar.rs" -> "/src/foo"
/// "/src/**/*.rs" -> "/src"
/// "*.js" -> ""
fn get_directory_prefix(pattern: &str) -> String {
    // Remove leading / for comparison
    let p = pattern.strip_prefix('/').unwrap_or(pattern);

    // Find the last path separator before any wildcard
    let mut last_slash = None;
    for (i, c) in p.char_indices() {
        if c == '*' || c == '?' || c == '[' {
            break;
        }
        if c == '/' {
            last_slash = Some(i);
        }
    }

    match last_slash {
        Some(idx) => format!("/{}", &p[..idx]),
        None => {
            // Check if it's a full path to a file (contains /)
            if let Some(idx) = p.rfind('/') {
                format!("/{}", &p[..idx])
            } else {
                String::new()
            }
        }
    }
}

/// Count common directory depth between two paths
/// "/src/foo" and "/src/foo/bar" -> 2 (src, foo)
/// "/src/foo" and "/lib/bar" -> 0
fn common_prefix_depth(a: &str, b: &str) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }

    let a_parts: Vec<&str> = a.split('/').filter(|s| !s.is_empty()).collect();
    let b_parts: Vec<&str> = b.split('/').filter(|s| !s.is_empty()).collect();

    let mut depth = 0;
    for (ap, bp) in a_parts.iter().zip(b_parts.iter()) {
        if ap == bp {
            depth += 1;
        } else {
            break;
        }
    }
    depth
}

/// Format a CODEOWNERS file: normalize rule spacing, preserve comments exactly
pub fn format_codeowners(content: &str) -> String {
    let mut result = Vec::new();
    let mut prev_was_empty = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Preserve blank lines but collapse multiple
        if trimmed.is_empty() {
            if !prev_was_empty && !result.is_empty() {
                result.push(String::new());
            }
            prev_was_empty = true;
            continue;
        }
        prev_was_empty = false;

        // Comments: preserve exactly as-is (people use specific formatting)
        if trimmed.starts_with('#') {
            result.push(line.to_string());
            continue;
        }

        // Rules: normalize spacing between pattern and owners
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        let pattern = parts[0];
        let owners = &parts[1..];

        if owners.is_empty() {
            result.push(pattern.to_string());
        } else {
            result.push(format!("{} {}", pattern, owners.join(" ")));
        }
    }

    // Ensure trailing newline
    let mut output = result.join("\n");
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_file() {
        let lines = parse_codeowners_file("");
        assert!(lines.is_empty());
    }

    #[test]
    fn test_parse_comment() {
        let lines = parse_codeowners_file("# This is a comment");
        assert_eq!(lines.len(), 1);
        assert!(matches!(&lines[0], CodeownersLine::Comment(c) if c == "# This is a comment"));
    }

    #[test]
    fn test_parse_empty_line() {
        let lines = parse_codeowners_file("\n\n");
        assert_eq!(lines.len(), 2);
        assert!(matches!(lines[0], CodeownersLine::Empty));
        assert!(matches!(lines[1], CodeownersLine::Empty));
    }

    #[test]
    fn test_parse_rule_single_owner() {
        let lines = parse_codeowners_file("*.rs @rustacean");
        assert_eq!(lines.len(), 1);
        match &lines[0] {
            CodeownersLine::Rule { pattern, owners } => {
                assert_eq!(pattern, "*.rs");
                assert_eq!(owners, &vec!["@rustacean".to_string()]);
            }
            _ => panic!("Expected Rule"),
        }
    }

    #[test]
    fn test_parse_rule_multiple_owners() {
        let lines = parse_codeowners_file("/src/ @user1 @org/team email@test.com");
        assert_eq!(lines.len(), 1);
        match &lines[0] {
            CodeownersLine::Rule { pattern, owners } => {
                assert_eq!(pattern, "/src/");
                assert_eq!(
                    owners,
                    &vec![
                        "@user1".to_string(),
                        "@org/team".to_string(),
                        "email@test.com".to_string()
                    ]
                );
            }
            _ => panic!("Expected Rule"),
        }
    }

    #[test]
    fn test_parse_rule_no_owners() {
        let lines = parse_codeowners_file("/unowned/");
        assert_eq!(lines.len(), 1);
        match &lines[0] {
            CodeownersLine::Rule { pattern, owners } => {
                assert_eq!(pattern, "/unowned/");
                assert!(owners.is_empty());
            }
            _ => panic!("Expected Rule"),
        }
    }

    #[test]
    fn test_parse_with_positions() {
        let lines = parse_codeowners_file_with_positions("*.rs @owner");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].line_number, 0);
        assert_eq!(lines[0].pattern_start, 0);
        assert_eq!(lines[0].pattern_end, 4);
        assert_eq!(lines[0].owners_start, 5);
    }

    #[test]
    fn test_serialize_roundtrip() {
        let original = "# Comment\n*.rs @owner\n/src/ @team";
        let lines = parse_codeowners_file(original);
        let serialized = serialize_codeowners(&lines);
        assert_eq!(serialized, original);
    }

    #[test]
    fn test_serialize_empty_rule() {
        let lines = vec![CodeownersLine::Rule {
            pattern: "/path/".to_string(),
            owners: vec![],
        }];
        let serialized = serialize_codeowners(&lines);
        assert_eq!(serialized, "/path/ ");
    }

    #[test]
    fn test_insertion_point_path_similarity() {
        let lines = parse_codeowners_file(
            "/src/api/ @api-team\n/src/api/auth/ @auth-team\n/lib/ @lib-team",
        );
        // New file in /src/api should go after first best path match (/src/api/)
        let idx = find_insertion_point_with_owner(&lines, "/src/api/users.rs", None);
        assert_eq!(idx, 1); // After /src/api/
    }

    #[test]
    fn test_insertion_point_before_catch_all() {
        let lines = parse_codeowners_file("/src/ @team\n* @default");
        // New pattern should go before catch-all
        let idx = find_insertion_point_with_owner(&lines, "/lib/foo.rs", None);
        assert_eq!(idx, 1); // Before *
    }

    #[test]
    fn test_insertion_point_owner_cluster() {
        let lines =
            parse_codeowners_file("/src/ @alice\n/lib/ @bob\n/tests/ @alice\n/docs/ @carol");
        // When owner is specified, prefer inserting near their other rules
        let idx = find_insertion_point_with_owner(&lines, "/bin/tool.rs", Some("@alice"));
        assert_eq!(idx, 3); // After /tests/ @alice
    }

    #[test]
    fn test_insertion_point_empty() {
        let lines: Vec<CodeownersLine> = vec![];
        let idx = find_insertion_point_with_owner(&lines, "/foo/bar.rs", None);
        assert_eq!(idx, 0);
    }

    #[test]
    fn test_get_directory_prefix() {
        assert_eq!(get_directory_prefix("/src/foo/bar.rs"), "/src/foo");
        assert_eq!(get_directory_prefix("/src/**/*.rs"), "/src");
        assert_eq!(get_directory_prefix("*.js"), "");
        assert_eq!(get_directory_prefix("/single.rs"), "");
        assert_eq!(get_directory_prefix("/a/b/c/d.txt"), "/a/b/c");
        // Directory patterns (trailing slash)
        assert_eq!(get_directory_prefix("/src/api/"), "/src/api");
        assert_eq!(get_directory_prefix("/src/api/auth/"), "/src/api/auth");
    }

    #[test]
    fn test_common_prefix_depth() {
        assert_eq!(common_prefix_depth("/src/foo", "/src/foo/bar"), 2);
        assert_eq!(common_prefix_depth("/src/foo", "/src/bar"), 1);
        assert_eq!(common_prefix_depth("/src/foo", "/lib/bar"), 0);
        assert_eq!(common_prefix_depth("", "/src"), 0);
    }
}
