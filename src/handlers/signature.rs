//! Signature help handler - pattern syntax documentation

use tower_lsp::lsp_types::*;

/// Pattern syntax documentation
const PATTERN_DOCS: &[(&str, &str, &str)] = &[
    (
        "*",
        "Match any characters (single segment)",
        "Matches any file in a directory. Example: `*.rs` matches all Rust files.",
    ),
    (
        "**",
        "Match any characters (recursive)",
        "Matches files in any subdirectory. Example: `src/**/*.rs` matches all Rust files under src/.",
    ),
    (
        "?",
        "Match single character",
        "Matches exactly one character. Example: `file?.txt` matches file1.txt, fileA.txt.",
    ),
    (
        "/",
        "Directory separator / anchor",
        "Leading `/` anchors to repo root. Example: `/src/` only matches top-level src directory.",
    ),
];

/// Generate signature help for glob patterns
pub fn signature_help(line: &str, character: usize) -> Option<SignatureHelp> {
    // Only provide help if we're in the pattern part (before first space with @)
    let first_space = line.find(' ');
    let in_pattern = first_space.map(|s| character <= s).unwrap_or(true);

    if !in_pattern {
        return None;
    }

    // Find which glob character we're near
    let before_cursor = &line[..character.min(line.len())];

    // Check what pattern syntax is being used
    let active_parameter = if before_cursor.ends_with("**") {
        Some(1) // **
    } else if before_cursor.ends_with('*') {
        Some(0) // *
    } else if before_cursor.ends_with('?') {
        Some(2) // ?
    } else if before_cursor.starts_with('/') && before_cursor.len() == 1 {
        Some(3) // /
    } else {
        None
    };

    let signatures: Vec<SignatureInformation> = PATTERN_DOCS
        .iter()
        .map(|(label, doc, detail)| SignatureInformation {
            label: format!("{} - {}", label, doc),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: detail.to_string(),
            })),
            parameters: None,
            active_parameter: None,
        })
        .collect();

    Some(SignatureHelp {
        signatures,
        active_signature: active_parameter.map(|p| p as u32),
        active_parameter: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_help_star() {
        let help = signature_help("*.rs", 1);
        assert!(help.is_some());
        assert_eq!(help.unwrap().active_signature, Some(0));
    }

    #[test]
    fn test_signature_help_double_star() {
        let help = signature_help("src/**", 6);
        assert!(help.is_some());
        assert_eq!(help.unwrap().active_signature, Some(1));
    }

    #[test]
    fn test_signature_help_question() {
        let help = signature_help("file?.txt", 5);
        assert!(help.is_some());
        assert_eq!(help.unwrap().active_signature, Some(2));
    }

    #[test]
    fn test_signature_help_slash() {
        let help = signature_help("/", 1);
        assert!(help.is_some());
        assert_eq!(help.unwrap().active_signature, Some(3));
    }

    #[test]
    fn test_signature_help_bracket_not_supported() {
        // Character classes are not supported in CODEOWNERS
        let help = signature_help("[abc", 4);
        assert!(help.is_some());
        // Should NOT match any signature (no character class docs)
        assert!(help.unwrap().active_signature.is_none());
    }

    #[test]
    fn test_signature_help_not_in_pattern() {
        let help = signature_help("*.rs @owner", 8);
        assert!(help.is_none());
    }
}
