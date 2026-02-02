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
codeowners-cli fmt --check            # Check if formatted (for CI)
```

## LSP Features

### In Any File

- **Hover**: Shows file ownership when hovering over any code
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
  - Path completions as you type (trigger: `/`)
  - Owner completions from GitHub API (trigger: `@`, requires `validate_owners`)
- **Inlay Hints**: Shows how many files each pattern matches
- **Code Actions**:
  - Remove shadowed rules
  - Remove duplicate owners
  - Add owner to empty rules
  - Add catch-all rule for unowned files
- **GitHub Validation** (optional): Validates users/teams exist on GitHub

## Installation

Download the latest release from [Releases](https://github.com/radiosilence/codeowners-lsp/releases).

### Zed

Use the [codeowners-zed](https://github.com/radiosilence/codeowners-zed) extension.

### Manual

```bash
codeowners-lsp  # Communicates over stdio
```

## Configuration

The LSP finds CODEOWNERS in standard locations: `.github/CODEOWNERS`, `CODEOWNERS`, `docs/CODEOWNERS`.

### Initialization Options

```json
{
  "path": "custom/CODEOWNERS",
  "individual": "@username",
  "team": "@org/team-name",
  "github_token": "env:GITHUB_TOKEN",
  "validate_owners": false
}
```

| Option            | Description                                                                    |
| ----------------- | ------------------------------------------------------------------------------ |
| `path`            | Custom CODEOWNERS location (relative to workspace root)                        |
| `individual`      | Your GitHub handle for "take ownership" actions                                |
| `team`            | Your team's handle for "take ownership" actions                                |
| `github_token`    | GitHub token for owner validation. Use `env:VAR_NAME` to read from environment |
| `validate_owners` | Enable GitHub API validation of @user and @org/team (default: false)           |

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
| Auto-fix lint issues                  | 🔜     |

## License

MIT
