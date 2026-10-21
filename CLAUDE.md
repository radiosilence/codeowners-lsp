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

## CI/CD

`.github/workflows/ci-cd.yml` is one pipeline. Its CI half ends in `Checks`, the only status the ruleset requires; it passes when every other job succeeded or was skipped. The `changes` job works out from the diff which domains a change touches (a pull request against its base, a push against the last main commit whose run passed; a manual run, or a diff it cannot work out, runs everything), and each job runs only for its domain, so a docs-only change skips every Rust job. Markdown never counts, and `.github/` counts for every domain. macOS and Windows runners are scarce or dear, so pull requests use them only when a manifest, the toolchain or CI changed, or a Rust file names that platform in a cfg; main uses them for any Rust change.

To add a domain, give `changes` an output and an allowlist of the paths that reach it, gate the new jobs with `if: needs.changes.outputs.<domain> == 'true'`, and add them to `Checks`'s `needs`.

Releasing: bump `version` in `crates/codeowners-lsp/Cargo.toml` (and the parser's, if it changed) and merge. The push sees the bump, builds the signed macOS binaries alongside the checks, then publishes both crates to crates.io (each skipped if its version is already there) and creates the `v<version>` GitHub release with every binary. A failed release is retried by running the workflow manually on main, which releases whatever version has no GitHub release yet.

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
- GitHub usernames/orgs: alphanumeric and hyphens only — no underscores, no periods (`validation.rs`).
- CODEOWNERS does NOT support `[...]` character classes or `!` negation (unlike gitignore).
- Owner matching in handlers must use forward search with word boundaries, not `find()`/`rfind()`.
- `check_file_ownership_parsed()` exists for hot loops; `check_file_ownership()` re-parses each call. Neither caches compiled patterns — use `CompiledPattern` directly if that matters.
- Go-to-definition and linked editing are deliberately disabled (see CHANGELOG 0.16.1/0.16.2); the handlers still exist. Don't "fix" the capability flags.
- LSP `Position.character` is a UTF-16 offset — never slice a line with it. Use `handlers::util::utf16_offset_to_byte_index`.
