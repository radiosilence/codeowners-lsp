//! `pattern_subsumes` drives the "dead rule" diagnostic and its remove-rule
//! code action, so an unsound `true` silently deletes real ownership. The
//! soundness property is the whole contract: if `b` subsumes `a`, then every
//! path matching `a` must also match `b`. This checks that directly against
//! `pattern_matches` rather than against hand-written expectations, which is
//! what caught `/*` being treated as both recursive and universal.

use codeowners_parser::{pattern_matches, pattern_subsumes};

const PATTERNS: &[&str] = &[
    "*",
    "**",
    "/*",
    "/**",
    "*.rs",
    "*.md",
    "src/",
    "/src/",
    "src/*",
    "src/**",
    "docs/",
    "/docs/",
    "docs/*",
    "src/lib/",
    "src/lib/nested.rs",
    "src/main.rs",
    "README.md",
    "/README.md",
    "app/*",
    "src/*.rs",
];

const PATHS: &[&str] = &[
    "README.md",
    "src/main.rs",
    "src/lib/nested.rs",
    "src/lib/deep/x.rs",
    "docs/intro.md",
    "docs/guide/adv.md",
    "app/a.js",
    "app/sub/b.js",
    "vendor/src/main.rs",
    "x/docs/y.md",
    "top.rs",
    "a/b/c/d.rs",
];

#[test]
fn subsumption_never_claims_more_than_matching_delivers() {
    let mut violations = Vec::new();

    for a in PATTERNS {
        for b in PATTERNS {
            if !pattern_subsumes(a, b) {
                continue;
            }
            for path in PATHS {
                if pattern_matches(a, path) && !pattern_matches(b, path) {
                    violations.push(format!(
                        "subsumes({a:?}, {b:?}) is true, but {path:?} matches {a:?} and not {b:?}"
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "unsound subsumption would delete live rules:\n  {}",
        violations.join("\n  ")
    );
}

#[test]
fn single_star_directory_does_not_swallow_nested_rules() {
    // `docs/*` stops at immediate children — GitHub's own docs give this exact
    // example — so a nested rule above it is not dead.
    assert!(!pattern_matches("src/*", "src/lib/nested.rs"));
    assert!(!pattern_subsumes("src/lib/nested.rs", "src/*"));
    assert!(!pattern_subsumes("src/**", "src/*"));
    assert!(!pattern_subsumes("src/lib/", "src/*"));

    // It still subsumes what it genuinely covers.
    assert!(pattern_subsumes("src/main.rs", "src/*"));
    assert!(pattern_subsumes("src/*", "src/*"));
    assert!(pattern_subsumes("src/*", "src/"));
    assert!(pattern_subsumes("src/*", "src/**"));
}

#[test]
fn anchored_star_covers_the_root_only() {
    // `/*` matches root entries, not the whole tree.
    assert!(!pattern_matches("/*", "src/main.rs"));
    assert!(!pattern_subsumes("src/main.rs", "/*"));
    assert!(!pattern_subsumes("*.rs", "/*"));
    assert!(!pattern_subsumes("src/", "/*"));

    assert!(pattern_subsumes("README.md", "/*"));
    assert!(pattern_subsumes("/*", "/*"));
}

#[test]
fn floating_directory_is_not_subsumed_by_a_pinned_one() {
    // `src/` matches a src directory at any depth; `src/**` and `/src/` are
    // pinned to the root, so neither can cover it.
    assert!(pattern_matches("src/", "vendor/src/main.rs"));
    assert!(!pattern_matches("src/**", "vendor/src/main.rs"));
    assert!(!pattern_subsumes("src/", "src/**"));
    assert!(!pattern_subsumes("src/", "/src/"));

    // The reverse direction still holds — pinned is subsumed by floating.
    assert!(pattern_subsumes("/src/", "src/"));
}
