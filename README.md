# codeowners-lsp

Language server for CODEOWNERS files with diagnostics, navigation, and code actions. Also includes a standalone CLI for linting.

## CLI

```zsh
# Lint CODEOWNERS (auto-detects location)
codeowners-cli lint
codeowners-cli lint --json            # Machine-readable output for CI
codeowners-cli lint --fix             # Auto-fix safe issues (dupes, shadowed, no-match)
codeowners-cli lint --strict          # Exit non-zero on warnings (not just errors)
codeowners-cli lint --github-actions  # Output GitHub Actions annotations for PR feedback

# Check who owns a file
codeowners-cli check src/main.rs

# Check multiple files (positional or --files, consistent with coverage)
codeowners-cli check src/main.rs src/lib.rs
codeowners-cli check --files src/main.rs src/lib.rs

# JSON output (for CI/scripting)
codeowners-cli check --json src/main.rs

# Check files from a list (useful for GitHub Actions)
codeowners-cli check --json --files-from changed_files.txt
git diff --name-only origin/main | codeowners-cli check --json --stdin

# Show coverage stats (exits non-zero if uncovered files exist)
codeowners-cli coverage
codeowners-cli coverage --json            # JSON output for CI
codeowners-cli coverage --tree            # Show unowned files as directory tree

# Check coverage for specific files (useful for CI on PRs)
codeowners-cli coverage --files src/new_file.rs src/other.rs
codeowners-cli coverage --files-from changed_files.txt
git diff --name-only origin/main | codeowners-cli coverage --stdin

# Suggest owners for unowned files (requires lookup_cmd config)
# NOTE: Experimental - requires lookup_cmd to resolve emails to teams
codeowners-cli suggest                          # Preview suggestions
codeowners-cli suggest --write                  # Add suggestions to CODEOWNERS
codeowners-cli suggest --format codeowners      # Ready-to-paste CODEOWNERS lines
codeowners-cli suggest --min-confidence 50      # Higher confidence threshold
codeowners-cli suggest --anchored               # Use /path instead of path

# Optimize CODEOWNERS patterns (shadowed rules, no-match, consolidation)
codeowners-cli optimize                         # Preview optimizations
codeowners-cli optimize --write                 # Apply optimizations to file
codeowners-cli optimize --json                  # JSON output for CI
codeowners-cli optimize --min-files 5           # Require 5+ files for dir consolidation

# Format CODEOWNERS file
codeowners-cli fmt                    # Prints formatted output
codeowners-cli fmt --write            # Writes in place

# Validate owners against GitHub API
codeowners-cli validate-owners        # Uses GITHUB_TOKEN env var
codeowners-cli validate-owners --json # JSON output for CI
codeowners-cli validate-owners --token ghp_xxx

# Validate only owners relevant to specific files (useful for CI on PRs)
codeowners-cli validate-owners --files src/new.rs src/other.rs
codeowners-cli validate-owners --files-from changed_files.txt
git diff --name-only origin/main | codeowners-cli validate-owners --stdin

# Show all files color-coded by owner
codeowners-cli tree

# Generate shell completions
codeowners-cli completions zsh       # zsh, bash, fish, powershell, elvish

# GitHub Actions all-in-one command
codeowners-cli gha --changed-files-from changed.txt
# Runs: coverage (changed + all), owner validation (changed + all), lint
# Outputs: JSON to stdout, GITHUB_OUTPUT vars, GITHUB_STEP_SUMMARY markdown
# Fails on: uncovered changed files OR invalid owners for changed files
```

## GitHub Actions

The `gha` command runs all checks in one efficient call with native GHA output:

```yaml
- name: Get changed files
  run: |
    gh api "repos/${{ github.repository }}/pulls/${{ github.event.pull_request.number }}/files" \
      --paginate --jq '.[].filename' > changed_files.txt
  env:
    GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}

- name: CODEOWNERS check
  run: codeowners-cli gha --changed-files-from changed_files.txt
  env:
    GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

**What it does:**

- **Coverage** - Checks changed files have owners (fails if not), warns about all uncovered files
- **Owner validation** - Validates owners for changed files exist on GitHub (fails if not), warns about all invalid owners
- **Lint** - Checks CODEOWNERS syntax, emits `::error::` / `::warning::` annotations

**Outputs:**

- JSON results to stdout
- `GITHUB_OUTPUT` vars: `has-coverage-issues`, `coverage-issues`, `has-dead-entries`, `dead-entries`, `has-invalid-teams`, `invalid-teams`
- `GITHUB_STEP_SUMMARY` markdown report

**Flags to skip checks:** `--no-coverage-changed`, `--no-coverage-all`, `--no-owners-changed`, `--no-owners-all`, `--no-lint`

**Flags to control output:** `--no-json`, `--no-annotations`, `--no-summary`, `--no-outputs`

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
- **Code Lens**: Inline file count and owners above each rule
- **Document Symbols**: Outline view with sections and rules (Cmd+Shift+O)
- **Workspace Symbols**: Search patterns and owners across file (Cmd+T)
- **Folding**: Collapse comment blocks and sections
- **Semantic Highlighting**: Syntax colors for patterns, owners, globs, comments
- **Find References**: Find all rules containing an owner
- **Rename**: Rename an owner across all rules
- **Signature Help**: Pattern syntax documentation while typing (`*`, `**`, `?`, `[...]`)
- **Selection Range**: Smart expand selection (word → owner → all owners → rule → section)
- **Linked Editing**: Edit an owner and all occurrences update simultaneously
- **Pattern Hover**: Hover over patterns to see matching files
- **Code Actions**:
  - Remove shadowed rules
  - Remove duplicate owners
  - Add owner to empty rules
  - Add catch-all rule for unowned files
- **GitHub Validation** (optional): Validates users/teams exist on GitHub

All heavy operations (file scanning, pattern matching, GitHub API calls) run in background threads—the LSP never blocks your editor.

## Installation

### mise (recommended)

```zsh
mise use -g github:radiosilence/codeowners-lsp@latest
```

### Manual

Download the latest release from [Releases](https://github.com/radiosilence/codeowners-lsp/releases).

```zsh
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

# Suggest command settings
[suggest]
# Resolve git emails to team names (required for suggest to work)
lookup_cmd = "your-tool lookup {email} | jq -r .team"
# Prepend / to paths (anchored patterns)
anchored = true

# Diagnostic severity overrides
# Values: "off", "hint", "info", "warning", "error"
[diagnostics]
invalid-pattern = "error"        # default: error
invalid-owner = "error"          # default: error
pattern-no-match = "warning"     # default: warning
duplicate-owner = "warning"      # default: warning
shadowed-rule = "warning"        # default: warning
no-owners = "off"                # default: hint
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

| Option               | Description                                                                    |
| -------------------- | ------------------------------------------------------------------------------ |
| `path`               | Custom CODEOWNERS location (relative to workspace root)                        |
| `individual`         | Your GitHub handle for "take ownership" actions                                |
| `team`               | Your team's handle for "take ownership" actions                                |
| `github_token`       | GitHub token for owner validation. Use `env:VAR_NAME` to read from environment |
| `validate_owners`    | Enable GitHub API validation of @user and @org/team (default: false)           |
| `[suggest]`          | Settings for the `suggest` command                                             |
| `suggest.lookup_cmd` | Command to resolve git emails to teams (use `{email}` placeholder)             |
| `suggest.anchored`   | Prepend `/` to paths for anchored patterns (default: false)                    |
| `[diagnostics]`      | Map of diagnostic code to severity override                                    |

## Diagnostics

| Code                     | Default | Description                                                          |
| ------------------------ | ------- | -------------------------------------------------------------------- |
| `invalid-pattern`        | error   | Pattern has invalid glob syntax                                      |
| `invalid-owner`          | error   | Owner format invalid (must be `@user`, `@org/team`, or email)        |
| `pattern-no-match`       | warning | Pattern doesn't match any files in the repository                    |
| `duplicate-owner`        | warning | Same owner listed multiple times on one rule                         |
| `shadowed-rule`          | warning | Rule is shadowed by a later rule (dead code, last match wins)        |
| `no-owners`              | hint    | Rule has a pattern but no owners assigned                            |
| `file-not-owned`         | error   | File has no matching CODEOWNERS rule (shown on non-CODEOWNERS files) |
| `github-owner-not-found` | warning | Owner not found on GitHub (requires `validate_owners = true`)        |

Override severities in config with: `off`, `hint`, `info`, `warning`, `error`

## Feature Status

| Feature                                | Status          |
| -------------------------------------- | --------------- |
| Hover (ownership info)                 | ✅              |
| Inlay hints (ownership)                | ✅              |
| Go-to-definition                       | ✅              |
| Take ownership actions                 | ✅              |
| Diagnostics (invalid patterns/owners)  | ✅              |
| Diagnostics (no matching files)        | ✅              |
| Diagnostics (shadowed rules)           | ✅              |
| Diagnostics (duplicate owners)         | ✅              |
| Diagnostics (no owners)                | ✅              |
| Coverage reporting                     | ✅              |
| Path completions                       | ✅              |
| Owner completions (GitHub API)         | ✅              |
| GitHub owner validation                | ✅              |
| CLI: lint                              | ✅              |
| CLI: check                             | ✅              |
| CLI: coverage                          | ✅              |
| CLI: fmt                               | ✅              |
| Code actions: remove shadowed          | ✅              |
| Code actions: remove duplicate owners  | ✅              |
| Code actions: add owner                | ✅              |
| Code actions: add catch-all            | ✅              |
| CLI: lint --fix (auto-fix safe issues) | ✅              |
| LSP: textDocument/formatting           | ✅              |
| Hover: clickable GitHub links          | ✅              |
| Code actions: fix all safe issues      | ✅              |
| Configurable diagnostic severities     | ✅              |
| Hover: link to CODEOWNERS rule         | ✅              |
| CLI: validate-owners                   | ✅              |
| CLI: tree (color-coded by owner)       | ✅              |
| CLI: shell completions                 | ✅              |
| Hover: rich team/user metadata         | ✅              |
| fzf-style fuzzy path completion        | ✅              |
| Background GitHub validation           | ✅              |
| Real-time pattern validation           | ✅              |
| Document symbols (outline view)        | ✅              |
| Workspace symbols (search)             | ✅              |
| Folding ranges                         | ✅              |
| Semantic tokens (syntax highlighting)  | ✅              |
| Find all references                    | ✅              |
| Rename symbol                          | ✅              |
| Code lens                              | ✅              |
| Signature help (pattern syntax)        | ✅              |
| Selection range (smart expand)         | ✅              |
| Linked editing (multi-cursor owners)   | ✅              |
| Pattern hover (show matches)           | ✅              |
| Related diagnostics (shadowed links)   | ✅              |
| CLI: suggest (git-based suggestions)   | ⚠️ experimental |
| CLI: optimize (pattern consolidation)  | ✅              |

## How It Works

### Optimization (`optimize`)

The optimizer detects two types of issues:

**1. Shadowed Rules (Dead Code)**

CODEOWNERS uses "last match wins" semantics. If a later rule matches the same files as an earlier one, the earlier rule is dead code:

```
/src/auth/ @security     # ❌ Dead - shadowed by /src/ below
/src/ @backend           # ✅ This wins for /src/auth/*
```

The optimizer works backwards from the end of the file, tracking which patterns could shadow earlier ones. A pattern is shadowed if any later pattern "subsumes" it (matches everything it matches). The catch-all `*` subsumes everything, so any rule before a final `* @team` is dead.

Key subsumption rules:

- `*` and `**` subsume all patterns
- `/src/` subsumes `/src/lib/` (parent directory contains child)
- `docs/` (unanchored) subsumes `/docs/` (anchored) - unanchored matches more
- `/docs/` does NOT subsume `docs/` - anchored matches fewer paths

**2. Directory Consolidation**

When multiple files in a directory have identical owners:

```
/src/lib/foo.rs @team    # These three lines...
/src/lib/bar.rs @team
/src/lib/baz.rs @team
```

...can become:

```
/src/lib/ @team          # ...this one line
```

Consolidation only triggers when:

- All files in the directory are explicitly listed
- All have exactly the same owners
- At least 3 files (configurable with `--min-files`)
- The resulting pattern wouldn't be immediately shadowed

### Suggestions (`suggest`)

The suggester analyzes git history to recommend owners for unowned files.

**How it works:**

1. **Find unowned files** - Files not matched by any CODEOWNERS rule
2. **Analyze git blame** - For each file, get line-by-line author information
3. **Weight by recency** - Recent changes matter more than ancient history
4. **Aggregate by author** - Sum weighted contributions per author
5. **Resolve to teams** - Use `lookup_cmd` to map emails → team names
6. **Match existing owners** - Fuzzy-match against owners already in CODEOWNERS
7. **Calculate confidence** - Based on contribution concentration and history depth

**The `lookup_cmd` config:**

```toml
lookup_cmd = "your-tool lookup {email} | jq -r .team"
```

The command receives a git email and should output a team/owner identifier. This gets fuzzy-matched against existing CODEOWNERS entries to maintain consistency.

**Confidence scoring:**

- High (70%+): Single dominant contributor, maps cleanly to existing team
- Medium (40-70%): Clear top contributor but some ambiguity
- Low (<40%): Scattered ownership, multiple teams, or unmapped authors

Use `--min-confidence` to filter suggestions.

## License

MIT
