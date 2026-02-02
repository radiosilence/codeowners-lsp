# codeowners-lsp

Language server for CODEOWNERS files with diagnostics, navigation, and code actions. Also includes a standalone CLI for linting.

## CLI

```bash
# Lint CODEOWNERS (auto-detects location)
codeowners-cli lint
codeowners-cli lint --json  # Machine-readable output for CI

# Check who owns a file
codeowners-cli check src/main.rs

# Show coverage stats
codeowners-cli coverage

# Format CODEOWNERS file
codeowners-cli fmt                    # Prints formatted output
codeowners-cli fmt --write            # Writes in place

# Auto-fix safe issues
codeowners-cli fix                    # Preview fixes
codeowners-cli fix --write            # Apply fixes

# Validate owners against GitHub API
codeowners-cli validate-owners        # Uses GITHUB_TOKEN env var
codeowners-cli validate-owners --token ghp_xxx

# Show all files color-coded by owner
codeowners-cli tree

# Generate shell completions
codeowners-cli completions bash       # bash, zsh, fish, powershell, elvish
```

## LSP Features

### In Any File

- **Hover**: Shows file ownership with clickable GitHub links and rich metadata (team descriptions, member counts, user bios)
- **Inlay Hints**: Displays ownership at the top of each file
- **Go-to-Definition**: Jump to the CODEOWNERS rule that matches the current file
- **Code Actions**: Take ownership of files directly from your editor

### In CODEOWNERS File

- **Diagnostics**:
  - Invalid glob patterns
  - Invalid owner format (@user, @org/team, or email)
  - Patterns matching no files
  - Duplicate/shadowed rules (dead code)
  - Rules with no owners
  - Coverage: count of files without owners
- **Completions**:
  - fzf-style fuzzy path completions (e.g., `s/m` matches `src/main.rs`)
  - Owner completions from GitHub API with background validation (trigger: `@`)
- **Inlay Hints**: Shows how many files each pattern matches
- **Code Actions**:
  - Remove shadowed rules
  - Remove duplicate owners
  - Add owner to empty rules
  - Add catch-all rule for unowned files
- **GitHub Validation** (optional): Validates users/teams exist on GitHub

All heavy operations (file scanning, pattern matching, GitHub API calls) run in background threads—the LSP never blocks your editor.

## Installation

### mise (recommended)

```bash
mise use -g github:radiosilence/codeowners-lsp@latest
```

### Manual

Download the latest release from [Releases](https://github.com/radiosilence/codeowners-lsp/releases).

```bash
codeowners-lsp  # Communicates over stdio
```

### Zed

Use the [codeowners-zed](https://github.com/radiosilence/codeowners-zed) extension (handles installation automatically).

## Configuration

The LSP finds CODEOWNERS in standard locations: `.github/CODEOWNERS`, `CODEOWNERS`, `docs/CODEOWNERS`.

### Config File

Create `.codeowners-lsp.toml` in your workspace root. For user-specific overrides (gitignore this), use `.codeowners-lsp.local.toml`.

```toml
# CODEOWNERS location (relative to workspace root)
path = "custom/CODEOWNERS"

# Your identifiers for "take ownership" actions
individual = "@username"
team = "@org/team-name"

# GitHub validation (optional)
github_token = "env:GITHUB_TOKEN"
validate_owners = false

# Diagnostic severity overrides
# Values: "off", "hint", "info", "warning", "error"
[diagnostics]
invalid-pattern = "error"        # default: error
invalid-owner = "error"          # default: error
pattern-no-match = "warning"     # default: warning
duplicate-owner = "warning"      # default: warning
shadowed-rule = "warning"        # default: warning
no-owners = "off"                # default: hint
unowned-files = "info"           # default: info
github-owner-not-found = "warning"  # default: warning
```

### LSP Initialization Options

JSON settings can also be passed via LSP init options (these override TOML config):

```json
{
  "path": "custom/CODEOWNERS",
  "individual": "@username",
  "team": "@org/team-name",
  "github_token": "env:GITHUB_TOKEN",
  "validate_owners": false,
  "diagnostics": {
    "no-owners": "off"
  }
}
```

| Option            | Description                                                                    |
| ----------------- | ------------------------------------------------------------------------------ |
| `path`            | Custom CODEOWNERS location (relative to workspace root)                        |
| `individual`      | Your GitHub handle for "take ownership" actions                                |
| `team`            | Your team's handle for "take ownership" actions                                |
| `github_token`    | GitHub token for owner validation. Use `env:VAR_NAME` to read from environment |
| `validate_owners` | Enable GitHub API validation of @user and @org/team (default: false)           |
| `diagnostics`     | Map of diagnostic code to severity override                                    |

## Feature Status

| Feature                               | Status |
| ------------------------------------- | ------ |
| Hover (ownership info)                | ✅     |
| Inlay hints (ownership)               | ✅     |
| Go-to-definition                      | ✅     |
| Take ownership actions                | ✅     |
| Diagnostics (invalid patterns/owners) | ✅     |
| Diagnostics (no matching files)       | ✅     |
| Diagnostics (shadowed rules)          | ✅     |
| Diagnostics (duplicate owners)        | ✅     |
| Diagnostics (no owners)               | ✅     |
| Coverage reporting                    | ✅     |
| Path completions                      | ✅     |
| Owner completions (GitHub API)        | ✅     |
| GitHub owner validation               | ✅     |
| CLI: lint                             | ✅     |
| CLI: check                            | ✅     |
| CLI: coverage                         | ✅     |
| CLI: fmt                              | ✅     |
| Code actions: remove shadowed         | ✅     |
| Code actions: remove duplicate owners | ✅     |
| Code actions: add owner               | ✅     |
| Code actions: add catch-all           | ✅     |
| CLI: fix (auto-fix safe issues)       | ✅     |
| LSP: textDocument/formatting          | ✅     |
| Hover: clickable GitHub links         | ✅     |
| Code actions: fix all safe issues     | ✅     |
| Configurable diagnostic severities    | ✅     |
| Hover: link to CODEOWNERS rule        | ✅     |
| CLI: validate-owners                  | ✅     |
| CLI: tree (color-coded by owner)      | ✅     |
| CLI: shell completions                | ✅     |
| Hover: rich team/user metadata        | ✅     |
| fzf-style fuzzy path completion       | ✅     |
| Background GitHub validation          | ✅     |
| Real-time pattern validation          | ✅     |

## License

MIT
