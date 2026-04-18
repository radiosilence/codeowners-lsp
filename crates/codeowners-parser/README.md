# codeowners-parser

[![Crates.io](https://img.shields.io/crates/v/codeowners-parser.svg)](https://crates.io/crates/codeowners-parser)
[![Docs.rs](https://docs.rs/codeowners-parser/badge.svg)](https://docs.rs/codeowners-parser)
[![MIT licensed](https://img.shields.io/crates/l/codeowners-parser.svg)](./LICENSE)

Parse, match against, and validate GitHub [CODEOWNERS] files — with the
position information editor tooling needs.

[CODEOWNERS]: https://docs.github.com/en/repositories/managing-your-repositories-settings-and-features/customizing-your-repository/about-code-owners

## Why this crate

The Rust ecosystem's original `codeowners` crate was abandoned in 2018
with two open RUSTSEC advisories in its transitive dependencies
([RUSTSEC-2022-0013], [RUSTSEC-2022-0006]) and no LSP-grade features.
This crate is extracted from [`codeowners-lsp`], where it has been
hardened against real-world CODEOWNERS files from large monorepos.

[RUSTSEC-2022-0013]: https://rustsec.org/advisories/RUSTSEC-2022-0013
[RUSTSEC-2022-0006]: https://rustsec.org/advisories/RUSTSEC-2022-0006
[`codeowners-lsp`]: https://github.com/radiosilence/codeowners-lsp

## What it does

- **Parse** CODEOWNERS into structured lines, with optional character
  offsets for every pattern, owner, and inline comment — exactly what an
  LSP needs for hover, rename, go-to-definition, and semantic tokens.
- **Match** file paths against rules using GitHub's glob semantics
  (`*`, `**`, `?`, anchored `/prefix`, trailing-slash directories).
  Each pattern is compiled once and dispatches to a specialized matcher
  based on shape — `*.rs` becomes a raw `ends_with` on bytes, not a regex.
- **Resolve ownership** for a path following "last match wins" semantics.
  Pre-parse once for hot loops via `check_file_ownership_parsed`.
- **Detect dead rules** via `pattern_subsumes` — used by linters to find
  patterns that will never match because a later rule shadows them.
- **Validate** owner format (`@user`, `@org/team`, `email@host`) and glob
  syntax — with the specific subset of globs CODEOWNERS actually supports
  (no `[...]` character classes, no `!` negation).
- **Locate** the CODEOWNERS file in a repo (`.github/CODEOWNERS`,
  `CODEOWNERS`, or `docs/CODEOWNERS`).

## What it doesn't do

- No file-system enumeration. Matchers take a `file_path: &str`; callers
  supply paths however they like (walkdir, `git ls-files`, HTTP API, etc.).
- No GitHub API calls. Validation is syntactic — `@ghost` is accepted
  even though GitHub would reject it.
- No diagnostic rendering. You get `Option<String>` from validators and
  structured results from matchers; turn those into whatever diagnostic
  format your tool needs.

## Quick start

```toml
[dependencies]
codeowners-parser = "0.1"
```

```rust
use codeowners_parser::{check_file_ownership, validate_owner, validate_pattern};

let codeowners = "\
# Default owners
*              @core-team
/docs/         @docs-team
*.rs           @rust-team @reviewer
";

// Who owns a file?
let result = check_file_ownership(codeowners, "src/lib.rs").unwrap();
assert_eq!(result.pattern, "*.rs");
assert_eq!(result.owners, vec!["@rust-team", "@reviewer"]);

// Validate owner syntax
assert!(validate_owner("@rust-team").is_none());
assert!(validate_owner("@rust_team").is_some()); // underscore not allowed
assert!(validate_owner("user@example.com").is_none());

// Validate glob syntax
assert!(validate_pattern("*.rs").is_none());
assert!(validate_pattern("").is_some());
```

## Position-aware parsing (for editor tooling)

```rust
use codeowners_parser::parser::{parse_codeowners_file_with_positions, CodeownersLine};

let lines = parse_codeowners_file_with_positions("*.rs @owner\n");
let line = &lines[0];

assert_eq!(line.line_number, 0);
assert_eq!(line.pattern_start, 0);
assert_eq!(line.pattern_end, 4);  // just past "*.rs"
assert_eq!(line.owners_start, 5); // at "@owner"

if let CodeownersLine::Rule { pattern, owners } = &line.content {
    assert_eq!(pattern, "*.rs");
    assert_eq!(owners, &vec!["@owner".to_string()]);
}
```

## Hot-loop ownership checks

When you need to check many paths against the same CODEOWNERS content,
parse once and reuse:

```rust
use codeowners_parser::{check_file_ownership_parsed, parser::parse_codeowners_file_with_positions};

let parsed = parse_codeowners_file_with_positions(
    "*.rs @rust\n*.md @docs\n/src/ @core\n",
);

for path in ["src/main.rs", "README.md", "src/lib.rs"] {
    if let Some(result) = check_file_ownership_parsed(&parsed, path) {
        println!("{}: {} (from `{}`)", path, result.owners.join(" "), result.pattern);
    }
}
```

## Detecting dead rules

`pattern_subsumes(a, b)` — _is everything `a` matches also matched by `b`_?
If yes, and `b` comes after `a` in the file, `a` is dead code.

```rust
use codeowners_parser::pattern_subsumes;

// `*` subsumes everything — any rule before a final `*` is dead.
assert!(pattern_subsumes("*.rs", "*"));
assert!(pattern_subsumes("/src/", "*"));

// `/src/lib/` is subsumed by `/src/` (parent directory contains child).
assert!(pattern_subsumes("/src/lib/", "/src/"));

// Anchored dir is subsumed by the unanchored version (unanchored matches more).
assert!(pattern_subsumes("/docs/", "docs/"));
assert!(!pattern_subsumes("docs/", "/docs/"));
```

## Design notes

- **Zero-copy is not a goal.** Owners and patterns are returned as `String`
  / `Vec<String>`. The alternative (`&str` tied to the input) is hostile
  for interactive tooling that edits CODEOWNERS content.
- **Pattern compilation caches per call.** If you need pattern reuse
  across paths, hold a `CompiledPattern` yourself via `CompiledPattern::new`.
- **Glob semantics match `git check-ignore` closely**, not gitignore's
  full spec. CODEOWNERS is a subset: no `[...]` character classes, no `!`
  negation. `validate_pattern` rejects those.

## Feature flags

None. The crate has four runtime dependencies: `fast-glob`, `glob`,
`once_cell`, and `regex` — all common and well-maintained.

## Minimum supported Rust version

Rust 1.70 (the current stable-minus-a-while). Bumping MSRV is a minor-version
change; we will not bump it casually.

## Related

- [`codeowners-lsp`](https://github.com/radiosilence/codeowners-lsp) —
  the language server this was extracted from. Uses this crate plus a
  GitHub client, diagnostic engine, file cache, and the full LSP plumbing.
- [`codeowners-cli`](https://github.com/radiosilence/codeowners-lsp) —
  the CLI, shipped alongside the LSP.

## License

MIT
