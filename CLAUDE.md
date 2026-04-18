# codeowners-lsp

Rust LSP for CODEOWNERS with diagnostics, navigation, and code actions.

## Build & Test

```bash
cargo build          # dev build
cargo build --release
cargo clippy         # NO warnings allowed
cargo fmt            # always run after changes
cargo bench          # run full benchmark suite
cargo bench --bench parsing  # run a single benchmark group
./scripts/bench-summary.sh   # print summary table from last run
```

## Architecture

Cargo workspace with two member crates:

```
crates/
├── codeowners-parser/       # Standalone parser library (publishable)
│   ├── README.md
│   └── src/
│       ├── lib.rs           # Crate docs + public re-exports, #![deny(missing_docs)]
│       ├── parser.rs        # Line parsing with character positions
│       ├── pattern.rs       # CompiledPattern + pattern_matches/pattern_subsumes
│       ├── validation.rs    # Syntactic owner + glob validators
│       └── ownership.rs     # check_file_ownership*, find_codeowners, get_repo_root
└── codeowners-lsp/          # LSP server + CLI binaries (depends on parser)
    ├── benches/             # Criterion benches
    └── src/
        ├── lib.rs           # Shared library crate (re-exports parser modules)
        ├── main.rs          # LSP entry + Backend + LanguageServer impl
        ├── cli.rs           # CLI entry
        ├── handlers/        # LSP-only request handlers
        ├── commands/        # CLI-only commands
        ├── ownership.rs     # apply_safe_fixes (uses FileCache); re-exports parser fns
        ├── diagnostics.rs   # LSP-specific validation + GitHub diagnostics
        ├── file_cache.rs    # File enumeration with compiled-pattern cache
        ├── github.rs        # GitHub API client with persistent cache
        ├── settings.rs      # LSP/CLI config
        ├── blame.rs         # Git blame analysis (CLI suggest)
        └── lookup.rs        # Email → team lookup command
```

Using `tower-lsp`. The `codeowners-parser` crate handles parsing, matching, and validation; this crate adds the LSP plumbing, diagnostic engine, file enumeration, and GitHub integration.

Key structs:

- `Backend` - LSP server state, implements `LanguageServer` trait
- `Settings` - config from init options
- `CodeownersLine` / `ParsedLine` - parsed line representation with positions
- `FileCache` - cached file list for pattern matching
- `GitHubClient` - GitHub API with persistent caching

## LSP Capabilities

**Any file:**

- Hover: ownership info with GitHub metadata
- Inlay hints: ownership at line 0
- Go-to-definition: jump to matching CODEOWNERS rule
- Code actions: take ownership (individual/team/custom)

**CODEOWNERS file:**

- Diagnostics: invalid patterns, invalid owners, no matches, dead rules, coverage
- Inlay hints: file match count per rule
- Code lens: inline file count + owners above rules
- Document symbols: outline view with sections
- Workspace symbols: search patterns/owners
- Folding ranges: collapse comment blocks and sections
- Semantic tokens: syntax highlighting
- Find references: find all rules with an owner
- Rename: rename owner across all rules
- Signature help: pattern syntax docs while typing
- Selection range: smart expand selection
- Linked editing: edit owner in all places at once
- Code actions: remove dead rules, dedupe owners, add catch-all

## Config

```json
{
  "path": "custom/CODEOWNERS",
  "individual": "@username",
  "team": "@org/team-name",
  "github_token": "env:GITHUB_TOKEN",
  "validate_owners": false
}
```

## Key Gotchas

- `codeowners-parser` is `#![deny(missing_docs)]` — every pub item needs a doc comment.
- LSP's `lib.rs` re-exports `parser`/`pattern`/`validation` from parser crate, so `crate::parser::X` still resolves inside LSP code.
- `#[allow(dead_code)]` in LSP is still needed for functions only called from one binary context.
- GitHub usernames: alphanumeric, hyphens, underscores only (NO periods).
- CODEOWNERS does NOT support `[...]` character classes or `!` negation (unlike gitignore).
- Owner matching in handlers must use forward search with word boundaries, not `find()`/`rfind()`.
- `check_file_ownership_parsed()` exists for hot loops; `check_file_ownership()` re-parses each call.
