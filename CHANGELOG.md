# Changelog

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
