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
pub fn parse_codeowners_file(content: &str) -> Vec<CodeownersLine> {
    parse_codeowners_file_with_positions(content)
        .into_iter()
        .map(|p| p.content)
        .collect()
}

/// Write parsed lines back to a string
pub fn serialize_codeowners(lines: &[CodeownersLine]) -> String {
    lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Find the best insertion point for a new pattern (maintains specificity order)
pub fn find_insertion_point(lines: &[CodeownersLine], _pattern: &str) -> usize {
    // CODEOWNERS rules are matched last-match-wins, so more specific patterns
    // should come later in the file. We'll insert at the end by default.
    lines.len()
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
}
