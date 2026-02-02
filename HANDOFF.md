# Handoff Document for Next Claude Session

## Project Overview
`codeowners-lsp` - A Rust LSP and CLI for CODEOWNERS files. Provides diagnostics, hover info, code actions, completions, and various CLI commands.

## Current State (v0.5.3 + uncommitted work)

### Recent Changes This Session
1. **Parallel `validate-owners` with progress bar** - Uses `indicatif` and `futures` for 5-concurrent validation
2. **`file-not-owned` diagnostic** - Full-file error when a file has no CODEOWNERS entry (configurable severity)
3. **Smart insertion point** - New CODEOWNERS entries placed near similar paths or same owner's rules
4. **CLI `config` command** - Shows config file paths and merged settings
5. **Background GitHub validation** - LSP validates owners on init, saves to `.codeowners-lsp/cache.json`
6. **Cached owners in autocomplete** - Validated owners appear with "Validated on GitHub" label

### Outstanding Issues to Investigate

#### Code Actions Not Showing Individual/Team
User reports that "Take ownership as @username" / "Take ownership as @team" code actions don't appear despite config being set correctly:

```
individual = "@radiosilence"
team = "@surgeventures/team-mint-be"
```

Config is in `.codeowners-lsp.local.toml` and CLI `config` command shows it correctly. LSP now logs config on init - check those logs to verify config is being loaded.

Possible causes:
- LSP might not be finding the config file (workspace root issue?)
- Config might be loaded but code actions filtered by editor
- Need to verify via LSP logs: "Config loaded: individual=..., team=..."

#### CLI `lint` Should Use Cache
The persistent cache (`.codeowners-lsp/cache.json`) is only used by LSP currently. CLI `lint` command should also load and use it for GitHub validation diagnostics.

### Key Files

- `src/main.rs` - LSP server, all handlers
- `src/cli.rs` - CLI entry point and command routing
- `src/commands/` - Individual CLI commands (lint, check, coverage, config, etc.)
- `src/github.rs` - GitHub API client with persistent cache (`PersistentCache`)
- `src/diagnostics.rs` - Diagnostic generation including `DiagnosticConfig`
- `src/parser.rs` - CODEOWNERS parsing, `find_insertion_point_with_owner()`

### Config System

Two TOML files (both optional):
- `.codeowners-lsp.toml` - Project config (commit to repo)
- `.codeowners-lsp.local.toml` - User overrides (gitignore)

Priority: defaults < project config < local config < LSP init options

```toml
individual = "@username"
team = "@org/team"
github_token = "env:GITHUB_TOKEN"
validate_owners = true

[diagnostics]
file-not-owned = "off"  # or hint/info/warning/error
no-owners = "warning"
```

### Build/Test Commands
```bash
cargo build --release
cargo clippy  # NO warnings allowed
cargo fmt
cargo test
```

### Architecture Notes

- LSP uses `tower-lsp` crate
- `Backend` struct holds all state with `RwLock` for thread safety
- `GitHubClient` wrapped in `Arc` for sharing across async tasks
- Background validation spawns tokio task on `initialize`
- Persistent cache saved to `.codeowners-lsp/cache.json` with auto `.gitignore`

### Next Steps

1. **Debug code actions** - Check LSP logs after restart to see if config is loaded
2. **CLI lint + cache** - Load `PersistentCache` in lint command, use for GitHub diagnostics
3. **Consider**: Refresh validation when CODEOWNERS file is saved
4. **Consider**: Show invalid owners differently in autocomplete (strikethrough?)

### Git Status
- Branch: `main`
- Unpushed commits with background validation and cache features
- Version in Cargo.toml: `0.5.3` (but not yet released with these changes)
