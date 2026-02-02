# Changelog

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
