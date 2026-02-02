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

Single-file LSP in `src/main.rs` using `tower-lsp`. The `codeowners` crate handles matching (read-only), we handle parsing/validation/writes ourselves.

Key structs:

- `Backend` - LSP server state, implements `LanguageServer` trait
- `Settings` - config from init options
- `CodeownersLine` / `ParsedLine` - parsed line representation with positions
- `FileCache` - cached file list for pattern matching
- `GitHubCache` - cached GitHub validation results

## LSP Capabilities

**Any file:**

- Hover: ownership info
- Inlay hints: ownership at line 0
- Go-to-definition: jump to matching CODEOWNERS rule
- Code actions: take ownership (individual/team/custom)

**CODEOWNERS file:**

- Diagnostics: invalid patterns, invalid owners, no matches, dead rules, coverage
- Inlay hints: file match count per rule
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
