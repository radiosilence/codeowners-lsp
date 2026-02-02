# codeowners-lsp

Rust LSP providing CODEOWNERS info via hover, inlay hints, and code actions for taking ownership.

## Build & Test

```bash
cargo build          # dev build
cargo build --release
cargo clippy         # NO warnings allowed
cargo fmt            # always run after changes
```

## Architecture

Single-file LSP in `src/main.rs` using `tower-lsp`. The `codeowners` crate handles parsing (read-only), we handle writes manually by parsing/modifying/serializing the file.

Key structs:
- `Backend` - LSP server state, implements `LanguageServer` trait
- `Settings` - config from init options (`path`, `individual`, `team`)
- `CodeownersLine` - parsed line representation for file modification

## LSP Capabilities

- Hover: ownership info on any code
- Inlay hints: ownership at line 0
- Code actions: take ownership (individual/team/custom), add to existing entry
- Execute command: applies the ownership changes to CODEOWNERS file

## Config

Init options:
```json
{
  "path": "custom/CODEOWNERS",
  "individual": "@username",
  "team": "@org/team-name"
}
```
