//! # codeowners-parser
//!
//! Parse, match against, and validate GitHub CODEOWNERS files — with the
//! position information editor tooling needs.
//!
//! This crate exists because the Rust ecosystem's original `codeowners` crate
//! was abandoned in 2018 with two open RUSTSEC advisories in its transitive
//! deps, and nothing public replaced it with LSP-grade features. This one is
//! extracted from [`codeowners-lsp`](https://github.com/radiosilence/codeowners-lsp),
//! where it has been hardened against real-world CODEOWNERS files from large
//! monorepos.
//!
//! ## What it does
//!
//! - **Parse** CODEOWNERS into structured lines, with optional character
//!   offsets for every pattern, owner, and inline comment — exactly what an
//!   LSP needs for hover, rename, and go-to-definition.
//! - **Match** file paths against rules using GitHub's glob semantics
//!   (`*`, `**`, `?`, anchored `/prefix`, trailing-slash directories).
//!   The matcher compiles each pattern once and picks a specialized
//!   implementation based on shape — `*.rs` becomes a raw `ends_with` check,
//!   not a regex.
//! - **Resolve ownership** for a path following "last match wins" semantics.
//!   Pre-parse once for hot loops via [`check_file_ownership_parsed`].
//! - **Detect dead rules** via [`pattern_subsumes`] — used by linters to find
//!   patterns that will never match because a later rule shadows them.
//! - **Validate** owner format (`@user`, `@org/team`, `email@host`) and glob
//!   syntax — with the specific subset of globs CODEOWNERS actually supports
//!   (no `[...]` character classes, no `!` negation).
//! - **Locate** the CODEOWNERS file in a repo (`.github/CODEOWNERS`,
//!   `CODEOWNERS`, or `docs/CODEOWNERS`) via [`find_codeowners`].
//!
//! ## What it doesn't do
//!
//! - No file-system enumeration. Matchers take a `file_path: &str`; callers
//!   supply paths however they like (walkdir, git ls-files, HTTP API, etc.).
//! - No GitHub API calls. Validation is syntactic — `@ghost` is accepted
//!   even though GitHub would reject it.
//! - No diagnostic rendering. You get `Option<String>` from validators and
//!   structured results from matchers; turn those into whatever diagnostic
//!   format your tool needs.
//!
//! ## Quick start
//!
//! ```
//! use codeowners_parser::{check_file_ownership, validate_owner, validate_pattern};
//!
//! let codeowners = "\
//! # Default owners
//! *              @core-team
//! /docs/         @docs-team
//! *.rs           @rust-team @reviewer
//! ";
//!
//! // Who owns a file?
//! let result = check_file_ownership(codeowners, "src/lib.rs").unwrap();
//! assert_eq!(result.pattern, "*.rs");
//! assert_eq!(result.owners, vec!["@rust-team", "@reviewer"]);
//!
//! // Validate owner syntax
//! assert!(validate_owner("@rust-team").is_none());
//! assert!(validate_owner("@rust_team").is_some()); // underscore not allowed
//! assert!(validate_owner("user@example.com").is_none());
//!
//! // Validate glob syntax
//! assert!(validate_pattern("*.rs").is_none());
//! assert!(validate_pattern("").is_some());
//! ```
//!
//! ## Parsing with positions (for editor tooling)
//!
//! ```
//! use codeowners_parser::parser::{parse_codeowners_file_with_positions, CodeownersLine};
//!
//! let lines = parse_codeowners_file_with_positions("*.rs @owner\n");
//! let line = &lines[0];
//!
//! assert_eq!(line.line_number, 0);
//! assert_eq!(line.pattern_start, 0);
//! assert_eq!(line.pattern_end, 4);       // points just past "*.rs"
//! assert_eq!(line.owners_start, 5);       // points at "@owner"
//!
//! if let CodeownersLine::Rule { pattern, owners } = &line.content {
//!     assert_eq!(pattern, "*.rs");
//!     assert_eq!(owners, &vec!["@owner".to_string()]);
//! }
//! ```
//!
//! ## Hot-loop ownership checks
//!
//! When you need to check many paths against the same CODEOWNERS content,
//! parse once and reuse the result. The crate's compiled-pattern cache
//! kicks in automatically inside [`check_file_ownership_parsed`].
//!
//! ```
//! use codeowners_parser::{check_file_ownership_parsed, parser::parse_codeowners_file_with_positions};
//!
//! let parsed = parse_codeowners_file_with_positions(
//!     "*.rs @rust\n*.md @docs\n/src/ @core\n",
//! );
//!
//! for path in ["src/main.rs", "README.md", "src/lib.rs"] {
//!     if let Some(result) = check_file_ownership_parsed(&parsed, path) {
//!         println!("{}: {} (from `{}`)", path, result.owners.join(" "), result.pattern);
//!     }
//! }
//! ```
//!
//! ## Detecting dead rules
//!
//! Linters can spot patterns that are shadowed by later rules. The call
//! is `pattern_subsumes(a, b)` — "is everything `a` matches also matched
//! by `b`?". If a later rule subsumes an earlier one, the earlier rule is
//! dead code (last match wins).
//!
//! ```
//! use codeowners_parser::pattern_subsumes;
//!
//! // `*` subsumes everything — any rule before a final `*` is dead.
//! assert!(pattern_subsumes("*.rs", "*"));
//! assert!(pattern_subsumes("/src/", "*"));
//!
//! // `/src/lib/` is subsumed by `/src/` (parent directory contains child).
//! assert!(pattern_subsumes("/src/lib/", "/src/"));
//!
//! // Anchored dir is subsumed by the unanchored version (unanchored matches more).
//! assert!(pattern_subsumes("/docs/", "docs/"));
//! assert!(!pattern_subsumes("docs/", "/docs/"));
//! ```
//!
//! ## Design notes
//!
//! - **Zero-copy is not a goal.** We return `String` / `Vec<String>` for
//!   owners and patterns. The alternative (`&str` tied to input lifetime)
//!   is hostile for interactive tooling that edits CODEOWNERS content.
//! - **Pattern compilation is cached per-call.** If you need pattern reuse
//!   across paths, keep a `CompiledPattern` yourself via
//!   [`pattern::CompiledPattern::compile`].
//! - **Glob semantics match `git check-ignore` closely**, not gitignore's
//!   full spec. CODEOWNERS is a subset: no `[...]` character classes,
//!   no `!` negation. Patterns containing `[` or `!` at the start are
//!   rejected by [`validate_pattern`].

#![deny(missing_docs)]

pub mod ownership;
pub mod parser;
pub mod pattern;
pub mod validation;

pub use ownership::{
    check_file_ownership, check_file_ownership_parsed, find_codeowners, get_repo_root,
    OwnershipResult,
};
pub use parser::{
    find_inline_comment_start, find_insertion_point, find_insertion_point_with_owner,
    find_owner_at_position, format_codeowners, parse_codeowners_file,
    parse_codeowners_file_with_positions, serialize_codeowners, CodeownersLine, ParsedLine,
};
pub use pattern::{pattern_matches, pattern_subsumes, CompiledPattern};
pub use validation::{validate_owner, validate_pattern};
