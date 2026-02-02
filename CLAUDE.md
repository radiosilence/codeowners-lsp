# codeowners-lsp

Rust LSP for CODEOWNERS with diagnostics, navigation, and code actions.

## Build & Test

```bash
cargo build          # dev build
cargo build --release
cargo clippy         # NO warnings allowed
cargo fmt            # always run after changes
```

## Architecture

```
src/
├── main.rs          # LSP entry + Backend struct + LanguageServer trait impl
├── cli.rs           # CLI entry point
├── handlers/        # LSP-only request handlers
│   ├── symbols.rs   # document_symbol, workspace_symbol
│   ├── navigation.rs # references, rename
│   ├── semantic.rs  # semantic_tokens, folding_range
│   ├── lens.rs      # code_lens
│   ├── signature.rs # signature_help
│   ├── selection.rs # selection_range
│   └── linked.rs    # linked_editing_range
├── commands/        # CLI-only commands
│   ├── suggest.rs   # suggest owners from git history
│   ├── optimize.rs  # consolidate patterns
│   └── ...          # lint, fmt, fix, check, coverage, tree, config
├── blame.rs         # Shared: git blame/shortlog analysis
├── parser.rs        # Shared: CODEOWNERS parsing
├── pattern.rs       # Shared: glob matching
├── diagnostics.rs   # Shared: validation logic
├── file_cache.rs    # Shared: file enumeration
└── github.rs        # Shared: GitHub API client
```

Using `tower-lsp`. The `codeowners` crate handles matching (read-only), we handle parsing/validation/writes ourselves.

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
