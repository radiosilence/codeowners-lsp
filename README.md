# codeowners-lsp

Language server for CODEOWNERS files with diagnostics, navigation, and code actions.

## Features

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

## License

MIT
