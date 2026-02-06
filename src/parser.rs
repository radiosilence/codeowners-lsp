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
    /// Character offset where inline comment starts (the # character), if present
    pub comment_start: Option<u32>,
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
                    comment_start: None,
                }
            } else if trimmed.starts_with('#') {
                ParsedLine {
                    line_number: line_num as u32,
                    content: CodeownersLine::Comment(line.to_string()),
                    pattern_start: 0,
                    pattern_end: 0,
                    owners_start: 0,
                    comment_start: None,
                }
            } else {
                // Split by whitespace, stopping at # (end-of-line comment)
                let parts: Vec<&str> = line
                    .split_whitespace()
                    .take_while(|part| !part.starts_with('#'))
                    .collect();

                // Find the inline comment position (first # that's a separate whitespace-delimited token)
                let comment_start = find_inline_comment_start(line).map(|pos| pos as u32);

                if parts.is_empty() {
                    ParsedLine {
                        line_number: line_num as u32,
                        content: CodeownersLine::Empty,
                        pattern_start: 0,
                        pattern_end: 0,
                        owners_start: 0,
                        comment_start: None,
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
                        comment_start,
                    }
                }
            }
        })
        .collect()
}

/// Find the char offset of an inline comment on a rule line.
/// An inline comment starts with `#` that is preceded by whitespace.
pub fn find_inline_comment_start(line: &str) -> Option<usize> {
    let mut in_whitespace = true;
    for (i, c) in line.chars().enumerate() {
        if c == '#' && in_whitespace {
            return Some(i);
        }
        in_whitespace = c == ' ' || c == '\t';
    }
    None
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

/// Find the @owner at a given character position in a line
#[allow(dead_code)] // Used by LSP only
pub fn find_owner_at_position(line: &str, char_idx: usize) -> Option<String> {
    // Skip comments
    if line.trim_start().starts_with('#') {
        return None;
    }

    // Stop scanning at inline comment boundary
    let scan_end = find_inline_comment_start(line).unwrap_or(line.chars().count());

    // Find all potential owners in the non-comment portion
    let mut owners: Vec<(usize, usize, String)> = Vec::new();
    let mut i = 0;
    let chars: Vec<char> = line.chars().collect();

    while i < chars.len() && i < scan_end {
        if chars[i] == '@' {
            let start = i;
            i += 1;
            // Collect owner chars (alphanumeric, -, /)
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '-' || chars[i] == '/')
            {
                i += 1;
            }
            if i > start + 1 {
                let owner: String = chars[start..i].iter().collect();
                owners.push((start, i, owner));
            }
        } else {
            i += 1;
        }
    }

    // Find which owner the cursor is on
    for (start, end, owner) in owners {
        if char_idx >= start && char_idx < end {
            return Some(owner);
        }
    }

    None
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

        // Rules: normalize spacing between pattern and owners, preserve inline comments
        let parts: Vec<&str> = trimmed
            .split_whitespace()
            .take_while(|part| !part.starts_with('#'))
            .collect();
        if parts.is_empty() {
            continue;
        }

        let pattern = parts[0];
        let owners = &parts[1..];

        // Find inline comment in the original line
        let inline_comment = find_inline_comment_start(line).map(|char_off| {
            line.chars()
                .skip(char_off)
                .collect::<String>()
                .trim_end()
                .to_string()
        });

        let mut formatted = if owners.is_empty() {
            pattern.to_string()
        } else {
            format!("{} {}", pattern, owners.join(" "))
        };

        if let Some(comment) = inline_comment {
            formatted.push(' ');
            formatted.push_str(&comment);
        }

        result.push(formatted);
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
    fn test_parse_rule_single_owner_line_comment() {
        let lines = parse_codeowners_file("*.rs @rustacean # only rust people should touch rust");
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

    #[test]
    fn test_find_owner_at_position_basic() {
        let line = "*.rs @owner1 @org/team";
        assert_eq!(find_owner_at_position(line, 5), Some("@owner1".to_string()));
        assert_eq!(find_owner_at_position(line, 6), Some("@owner1".to_string()));
        assert_eq!(
            find_owner_at_position(line, 11),
            Some("@owner1".to_string())
        );
        assert_eq!(
            find_owner_at_position(line, 13),
            Some("@org/team".to_string())
        );
    }

    #[test]
    fn test_find_owner_at_position_outside_owner() {
        let line = "*.rs @owner";
        // On pattern, not owner
        assert_eq!(find_owner_at_position(line, 0), None);
        assert_eq!(find_owner_at_position(line, 3), None);
        // Past end
        assert_eq!(find_owner_at_position(line, 100), None);
    }

    #[test]
    fn test_find_owner_at_position_comment() {
        let line = "# @mention in comment";
        assert_eq!(find_owner_at_position(line, 2), None);
    }

    #[test]
    fn test_find_owner_at_position_indented_comment() {
        let line = "   # @mention in comment";
        assert_eq!(find_owner_at_position(line, 5), None);
    }

    #[test]
    fn test_find_owner_at_position_no_owners() {
        let line = "/unowned/path/";
        assert_eq!(find_owner_at_position(line, 0), None);
        assert_eq!(find_owner_at_position(line, 5), None);
    }

    #[test]
    fn test_find_owner_at_position_just_at_symbol() {
        let line = "*.rs @ @";
        // @ alone (no following chars) shouldn't match
        assert_eq!(find_owner_at_position(line, 5), None);
        assert_eq!(find_owner_at_position(line, 7), None);
    }

    #[test]
    fn test_find_owner_at_position_special_chars() {
        let line = "*.rs @user-name @org/team";
        assert_eq!(
            find_owner_at_position(line, 5),
            Some("@user-name".to_string())
        );
        assert_eq!(
            find_owner_at_position(line, 16),
            Some("@org/team".to_string())
        );
    }

    #[test]
    fn test_format_codeowners_basic() {
        let input = "# Comment\n*.rs    @owner1   @owner2\n/src/   @team";
        let formatted = format_codeowners(input);
        assert_eq!(formatted, "# Comment\n*.rs @owner1 @owner2\n/src/ @team\n");
    }

    #[test]
    fn test_format_codeowners_collapse_empty_lines() {
        let input = "*.rs @owner\n\n\n\n/src/ @team";
        let formatted = format_codeowners(input);
        assert_eq!(formatted, "*.rs @owner\n\n/src/ @team\n");
    }

    #[test]
    fn test_format_codeowners_preserves_comments() {
        let input = "# This is a special comment    with    spacing\n*.rs @owner";
        let formatted = format_codeowners(input);
        assert!(formatted.contains("# This is a special comment    with    spacing"));
    }

    #[test]
    fn test_format_codeowners_no_owner() {
        let input = "/unowned/";
        let formatted = format_codeowners(input);
        assert_eq!(formatted, "/unowned/\n");
    }

    #[test]
    fn test_format_codeowners_empty() {
        let input = "";
        let formatted = format_codeowners(input);
        assert_eq!(formatted, "");
    }

    #[test]
    fn test_format_codeowners_leading_empty_lines() {
        let input = "\n\n# Header\n*.rs @owner";
        let formatted = format_codeowners(input);
        // Leading blank lines should not produce output before first content
        assert_eq!(formatted, "# Header\n*.rs @owner\n");
    }

    #[test]
    fn test_codeowners_line_display_comment() {
        let line = CodeownersLine::Comment("# test comment".to_string());
        assert_eq!(format!("{}", line), "# test comment");
    }

    #[test]
    fn test_codeowners_line_display_empty() {
        let line = CodeownersLine::Empty;
        assert_eq!(format!("{}", line), "");
    }

    #[test]
    fn test_codeowners_line_display_rule() {
        let line = CodeownersLine::Rule {
            pattern: "*.rs".to_string(),
            owners: vec!["@alice".to_string(), "@bob".to_string()],
        };
        assert_eq!(format!("{}", line), "*.rs @alice @bob");
    }

    #[test]
    fn test_parse_positions_with_leading_whitespace() {
        let lines = parse_codeowners_file_with_positions("  *.rs @owner");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].pattern_start, 2);
        assert_eq!(lines[0].pattern_end, 6);
        assert_eq!(lines[0].owners_start, 7);
    }

    #[test]
    fn test_parse_whitespace_only_line() {
        let lines = parse_codeowners_file("   \t   ");
        assert_eq!(lines.len(), 1);
        assert!(matches!(lines[0], CodeownersLine::Empty));
    }

    #[test]
    fn test_insertion_point_only_comments() {
        let lines = parse_codeowners_file("# Header\n# Another comment");
        let idx = find_insertion_point(&lines, "/foo.rs");
        // Should insert at end when no rules
        assert_eq!(idx, 2);
    }

    #[test]
    fn test_insertion_point_prefers_deeper_path_match() {
        let lines = parse_codeowners_file("/src/ @team1\n/src/api/ @team2\n/src/api/v2/ @team3");
        // Should prefer /src/api/v2 for a file in /src/api/v2/
        let idx = find_insertion_point(&lines, "/src/api/v2/users.rs");
        assert_eq!(idx, 3); // After /src/api/v2/
    }

    #[test]
    fn test_insertion_point_catch_all_variants() {
        // Test with /* catch-all
        let lines = parse_codeowners_file("/src/ @team\n/* @default");
        let idx = find_insertion_point(&lines, "/lib/foo.rs");
        assert_eq!(idx, 1);

        // Test with /** catch-all
        let lines2 = parse_codeowners_file("/src/ @team\n/** @default");
        let idx2 = find_insertion_point(&lines2, "/lib/foo.rs");
        assert_eq!(idx2, 1);
    }

    #[test]
    fn test_directory_prefix_wildcard_variants() {
        assert_eq!(get_directory_prefix("/src/?oo/"), "/src");
        assert_eq!(get_directory_prefix("/src/[abc]/"), "/src");
        assert_eq!(get_directory_prefix("/a/b/*.txt"), "/a/b");
    }

    #[test]
    fn test_find_insertion_point_without_owner() {
        let lines = parse_codeowners_file("/src/ @team\n/lib/ @team");
        // Uses find_insertion_point_with_owner internally
        let idx = find_insertion_point(&lines, "/bin/tool.rs");
        // No path match, no catch-all, so end of file
        assert_eq!(idx, 2);
    }

    #[test]
    fn test_get_directory_prefix_file_with_slash_no_wildcard() {
        // File path with / but no wildcard before it (line 232)
        // This is like "src/foo/bar.txt" where there's no wildcard
        assert_eq!(get_directory_prefix("src/foo/bar.txt"), "/src/foo");
        assert_eq!(get_directory_prefix("a/b/c/file.rs"), "/a/b/c");
    }

    #[test]
    fn test_format_codeowners_whitespace_only_after_trim() {
        // Line that becomes empty after parts split (line 334)
        // This shouldn't really happen since trimmed empty is caught earlier
        // but the code handles it
        let input = "*.rs @owner\n\n/src/ @team";
        let formatted = format_codeowners(input);
        // Should handle gracefully
        assert!(formatted.contains("*.rs @owner"));
        assert!(formatted.contains("/src/ @team"));
    }

    // --- Inline comment tests ---

    #[test]
    fn test_find_inline_comment_start() {
        assert_eq!(find_inline_comment_start("*.rs @owner # comment"), Some(12));
        assert_eq!(find_inline_comment_start("*.rs @owner"), None);
        assert_eq!(find_inline_comment_start("# full line comment"), Some(0));
        assert_eq!(find_inline_comment_start("*.rs @owner #nospace"), Some(12));
    }

    #[test]
    fn test_find_inline_comment_start_hash_in_token() {
        // # attached to a token (no preceding whitespace) is NOT an inline comment
        assert_eq!(find_inline_comment_start("*.rs @owner#attached"), None);
    }

    #[test]
    fn test_parse_inline_comment_stripped_from_owners() {
        let lines = parse_codeowners_file("*.rs @owner # contact @admin for changes");
        assert_eq!(lines.len(), 1);
        match &lines[0] {
            CodeownersLine::Rule { owners, .. } => {
                assert_eq!(owners, &vec!["@owner".to_string()]);
            }
            _ => panic!("Expected Rule"),
        }
    }

    #[test]
    fn test_parse_inline_comment_position_stored() {
        let lines = parse_codeowners_file_with_positions("*.rs @owner # comment");
        assert_eq!(lines[0].comment_start, Some(12));
    }

    #[test]
    fn test_parse_no_inline_comment() {
        let lines = parse_codeowners_file_with_positions("*.rs @owner");
        assert_eq!(lines[0].comment_start, None);
    }

    #[test]
    fn test_find_owner_at_position_inline_comment_ignored() {
        let line = "*.rs @owner # contact @admin";
        // @owner should be found
        assert_eq!(find_owner_at_position(line, 5), Some("@owner".to_string()));
        // @admin is in the comment, should NOT be found
        assert_eq!(find_owner_at_position(line, 22), None);
    }

    #[test]
    fn test_find_owner_at_position_inline_comment_no_space() {
        let line = "*.rs @owner #comment @nope";
        assert_eq!(find_owner_at_position(line, 5), Some("@owner".to_string()));
        assert_eq!(find_owner_at_position(line, 21), None);
    }

    #[test]
    fn test_format_preserves_inline_comments() {
        let input = "*.rs    @owner1   @owner2   # important note";
        let formatted = format_codeowners(input);
        assert_eq!(formatted, "*.rs @owner1 @owner2 # important note\n");
    }

    #[test]
    fn test_format_preserves_inline_comment_no_owners() {
        let input = "/unowned/ # deliberately unowned";
        let formatted = format_codeowners(input);
        assert_eq!(formatted, "/unowned/ # deliberately unowned\n");
    }

    #[test]
    fn test_format_roundtrip_with_inline_comments() {
        let input = "# Header\n*.rs @owner # rust files\n/src/ @team # source dir\n";
        let formatted = format_codeowners(input);
        assert_eq!(
            formatted,
            "# Header\n*.rs @owner # rust files\n/src/ @team # source dir\n"
        );
    }
}
