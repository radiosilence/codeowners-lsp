# Changelog

## [0.6.3] - 2026-02-02

### Fixed

- **No CODEOWNERS = no complaints** - file-not-owned diagnostics only shown when CODEOWNERS file actually exists

## [0.6.2] - 2026-02-02

### Fixed

- **Release includes both binaries** - `codeowners-cli` now included in release tarballs alongside `codeowners-lsp`

## [0.6.1] - 2026-02-02

### Added

- **Real-time pattern validation** - patterns are checked against file cache as you type, instant feedback on whether a pattern matches any files
- **mise installation** - `mise use -g github:radiosilence/codeowners-lsp@latest`

## [0.6.0] - 2026-02-02

### Performance

- **Non-blocking LSP main loop** - all heavy work (file scanning, pattern matching, GitHub validation) now runs in background threads via `spawn_blocking`, so the editor never lags
- **Pattern match caching** - file match counts are cached per-pattern, making repeated inlay hints/diagnostics instant
- **Inlay hints only compute visible range** - instead of computing all 3000+ lines, only computes hints for lines actually on screen
- **29x faster on large repos** - combined optimizations turn 20+ second delays into sub-second responses

### Added

- **Rich hover for @owners in CODEOWNERS** - hover over any @user or @org/team to see profile info, bio, team description, member counts, with avatars (in supported editors)
- **fzf-style fuzzy path completion** - type `s/m` to match `src/main.rs`, scores: exact prefix > substring > fuzzy
- **Background GitHub validation** - new owners are validated async on save, fetches full metadata for rich hover
- **Internal ignore list** - LSP config files (`.codeowners-lsp.toml`, `.codeowners-lsp/`) excluded from file-not-owned diagnostics
- **Persistent metadata cache** - GitHub user/team info cached to `.codeowners-lsp/cache.json` with auto-gitignore

### Fixed

- **Autocomplete replaces instead of appends** - completion items now use proper `text_edit` with replacement range
- **CODEOWNERS changes detected live** - editing CODEOWNERS buffer updates diagnostics immediately (not just on save)
- **Inlay hints refresh on CODEOWNERS change** - ownership hints in other files update when CODEOWNERS is modified
- **Path completion works without leading `/`** - `src/main` and `/src/main` both work correctly

## [0.5.3] - 2026-02-02

### Added

- **`file-not-owned` diagnostic** - files without CODEOWNERS entry show full-file error (impossible to miss!)
  - Default severity: `error`, configurable via `[diagnostics] file-not-owned = "off"` to disable
  - Existing "Take ownership" code actions work with this diagnostic
- **Smart insertion point** for new CODEOWNERS entries:
  - Inserts near rules with similar directory prefixes
  - Falls back to inserting near other rules by same owner
  - Inserts before catch-all rules (`*`, `/**`)
- **CLI: `config`** - show config file paths and merged settings (like `mise config`)

### Changed

- `validate-owners` now runs requests in parallel (5 concurrent) with progress bar

## [0.5.2] - 2026-02-02

### Added

- **Colorized CLI output** - all commands now use colored output for better readability

### Fixed

- Fixed clippy warnings for CLI-only code (dead_code attributes)

## [0.5.1] - 2026-02-02

### Added

- **Hover: link to CODEOWNERS rule** - hover tooltip now includes clickable link to the exact line in CODEOWNERS
- **CLI: `validate-owners`** - validate all owners against GitHub API, shows report of valid/invalid/unknown
- **CLI: `tree`** - show all files color-coded by owner (uses ANSI colors)
- **CLI: `completions`** - generate shell completions for bash, zsh, fish, powershell, elvish
- **CLI uses clap** - proper argument parsing with help, version, and better error messages

### Fixed

- `.git` directory now excluded from file cache (was causing issues with coverage/completions)

## [0.5.0] - 2026-02-02

### Added

- **Config file support** - `.codeowners-lsp.toml` for project settings, `.codeowners-lsp.local.toml` for user overrides
- **Configurable diagnostic severities** - disable or change severity of any diagnostic rule via `[diagnostics]` section
  - Supports: `off`, `hint`, `info`, `warning`, `error`
  - Example: `no-owners = "off"` to disable the no-owners hint

## [0.4.3] - 2026-02-02

### Added

- **"Fix all safe issues"** code action in LSP - one-click fix for all auto-fixable issues

### Changed

- Refactored shared logic into `ownership.rs` module (cleaner codebase)
- Formatter now preserves comment formatting exactly (no normalization)

## [0.4.2] - 2026-02-02

### Added

- **`fix` command** in CLI - auto-fix safe issues (duplicate owners, shadowed rules)
- **`textDocument/formatting`** in LSP - editors like Zed can now use CODEOWNERS-LSP as formatter
- **Clickable owner links** in hover - `@user` and `@org/team` now link to GitHub profiles/teams

### Changed

- `no-owners` diagnostic is now a hint (not a warning) - often intentional for opt-out

## [0.4.1] - 2026-02-02

### Changed

- **29x faster linting** via compiled patterns, fast-glob crate, and optimized algorithms (4.8s → 165ms on 3800-rule repos)

## [0.4.0] - 2026-02-02

### Added

- **Path completions** in CODEOWNERS file - autocomplete file paths as you type
- **Owner completions** - autocomplete owners from GitHub API (when `validate_owners` enabled)
- **`fmt` command** in CLI - format/normalize CODEOWNERS files

### Changed

- **Shadowed rules now show as warnings** instead of hints (visible in editors by default)

### Fixed

- Glob patterns like `/**/file.json`, `deployment/*/deploy/**`, and `*crowdin*` now match correctly

## [0.3.2] - 2026-02-02

### Fixed

- Use rustls instead of OpenSSL for cross-platform builds

## [0.3.1] - 2026-02-02

### Fixed

- Show "Owned by nobody" instead of empty output when file has no owners
- CI now recreates tags/releases instead of failing if they exist

## [0.3.0] - 2026-02-02

### Added

- **`codeowners-cli`** standalone CLI binary:
  - `lint [path]` - Check CODEOWNERS for issues (`--json` for CI)
  - `check <file>` - Show which rule owns a specific file
  - `coverage` - Show files without owners and coverage percentage
- **Pattern subsumption detection** - detects when rules are shadowed by more general patterns (e.g., `*.rs` shadowed by `*`)
- **Diagnostics** for CODEOWNERS file:
  - Invalid glob pattern syntax (error)
  - Invalid owner format (error)
  - Patterns matching no files (warning)
  - Duplicate patterns / dead rules (hint with "unnecessary" tag)
  - Duplicate owners on same line (warning)
  - Rules with no owners (warning)
  - Coverage: shows count of unowned files (info)
- **Inlay hints** in CODEOWNERS file showing file match count per rule
- **Go-to-definition** from any file jumps to its matching CODEOWNERS rule
- **Code actions** for CODEOWNERS diagnostics:
  - Remove shadowed/dead rules
  - Remove duplicate owners
  - Add owner to rules with no owners
  - Add catch-all rule for unowned files
- **GitHub validation** (optional): validates @user and @org/team against GitHub API
  - Requires `github_token` and `validate_owners: true` in settings

### Changed

- Refactored codebase into modules with comprehensive test coverage (39 tests)

## [0.2.2] - 2026-02-02

### Fixed

- Fixed trailing comma on owner names in hover list when multiple owners exist

## [0.2.1] - 2026-02-02

### Changed

- Improved hover formatting: single owner shows inline, multiple owners display as a markdown list with code formatting

## [0.2.0] - 2026-02-02

### Added

- Code actions for taking ownership of files
  - "Take ownership as individual" - uses configured `individual` setting
  - "Take ownership as team" - uses configured `team` setting
  - "Take ownership as custom" - for ad-hoc owners
  - "Add to existing entry" variants for files with existing ownership
- New settings: `individual` and `team` for configuring default owners

## [0.1.1] - 2026-02-02

Initial release.

### Features

- Hover info showing CODEOWNERS for current file
- Inlay hints displaying ownership at top of file
- Custom CODEOWNERS path via initialization options
- Automatic detection of CODEOWNERS in standard locations (`.github/CODEOWNERS`, `CODEOWNERS`, `docs/CODEOWNERS`)
