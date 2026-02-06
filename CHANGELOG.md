# Changelog

## [0.16.0] - 2026-02-06

Performance optimizations across pattern matching, diagnostics, and file cache — driven by the new criterion benchmark suite.

### Added

- **Criterion benchmark suite** — 5 benchmark files covering parsing, pattern matching, diagnostics, file cache, and LSP handlers. Generates a 1000-rule CODEOWNERS and 50k synthetic file paths with seeded RNG for deterministic, reproducible benchmarking.
- **`scripts/bench-summary.sh`** — prints a human-readable table of all benchmark medians from criterion JSON output.

### Performance

- **Extension pattern matching: -95.4%** (10.94ms → 500us for 50k files) — new `ExtensionSuffix` variant in `CompiledPattern` uses `ends_with` instead of glob matching for `*.ext` patterns.
- **Directory pattern matching: -69.7%** (10.67ms → 3.23ms for 50k files) — eliminated `format!("/{}/", dir)` allocation per path in `UnanchoredDirectory` matching, replaced with byte-level segment boundary checks.
- **Diagnostics (1000 rules, no cache): -78.5%** (39.8ms → 8.5ms) — skip already-shadowed patterns in the O(n²) subsumption loop; fast-path catch-all patterns (`*`/`**`) that trivially subsume everything.
- **Diagnostics (1000 rules, with cache): -74.4%** (41.5ms → 10.6ms) — same subsumption improvements.
- **get_unowned_files: -86.4%** (8.07ms → 1.10ms) — `count_matches`/`has_matches`/`get_matches` now compile to `CompiledPattern` once per call instead of re-parsing per file.
- **count_matches cold: -27.3%** (1.73ms → 1.26ms) — same compile-once optimization.
- **check_file_ownership_parsed: -61.0%** (45.6us → 17.8us) — benefits from pattern matching improvements.

### Changed

- Shared code extracted to lib crate (`src/lib.rs`) — enables criterion benchmarks to import internal modules. Both binaries re-export via `pub use`.

## [0.15.5] - 2026-02-06

Comprehensive code audit (Opus 4.6 reviewing Opus 4.5's work) plus bulletproof inline comment handling.

### Security

- **Command injection in lookup.rs** - Email from git shortlog was interpolated directly into shell commands. Now sanitized to allow only safe characters before substitution.

### Fixed

- **Inline comment handling** - Inline comments (`*.rs @owner # contact info`) now handled correctly everywhere:
  - Parser tracks comment boundary position on each line
  - Formatter preserves inline comments through roundtrip formatting
  - `find_owner_at_position` no longer returns `@mentions` inside comments
  - Semantic tokens emit proper comment highlighting for inline comment text
  - Diagnostics and rename operations bounded to the non-comment portion of lines

- **Username validation** - Removed `_` and `.` from allowed characters in GitHub username/team regexes. GitHub usernames only allow alphanumeric characters and hyphens.

- **find_nth_owner_position** - Linked editing, find references, and rename all used vec index instead of occurrence count, producing wrong positions when an owner appears multiple times on a line.

- **prepare_rename wrong highlight** - Used `line.find()` which always matched the first occurrence regardless of cursor position. Now scans forward checking word boundaries.

- **rfind in semantic tokens** - `line.rfind(owner)` always found the last occurrence, corrupting delta calculations for duplicate owners. Replaced with forward search.

- **blame.rs crash paths** - `parse().ok()?` aborted all results on a single malformed shortlog line (now skips). `partial_cmp().unwrap()` could panic on NaN (now `total_cmp()`). Directory prefix matching lacked `/` boundary check.

- **Section header detection** - Dead code in symbols, semantic tokens, and selection handlers. Comments include `#` prefix so the check always failed. Fixed to strip `#` first.

- **Unsupported `[...]` syntax** - Removed character class references from signature help docs and semantic token highlighting. CODEOWNERS does not support character classes.

- **selection.rs dedup without sort** - `dedup()` only removes adjacent duplicates; added `sort_by` first.

- **Code lens empty owners** - Trailing separator when rule has no owners.

### Performance

- **Parse-once optimization** - `tree` and `check` commands re-parsed CODEOWNERS for every file. Extracted `check_file_ownership_parsed()` that takes pre-parsed lines.

## [0.15.4] - 2026-02-06

### Fixed

- **Comment support** - End of line comments were breaking the parsing

## [0.15.3] - 2026-02-05

### Fixed

- **LSP lockup on rapid typing** - GitHub owner validation no longer blocks the LSP. Typing `@invalidowner` rapidly won't freeze the editor.

- **`gha` exit code** - Now fails (exit 1) on lint errors. Use `--strict` to also fail on warnings.

## [0.15.2] - 2026-02-05

### Fixed

- **Completions show stale files** - File cache now refreshes on each completion request, so renamed/added/deleted files appear correctly.

## [0.15.1] - 2026-02-05

### Changed

- **`gha` output** - Replaced JSON stdout with human-readable terminal summary. JSON data available via `GITHUB_OUTPUT` variables.

## [0.15.0] - 2026-02-05

### Added

- **`gha` command** - All-in-one GitHub Actions integration. Runs coverage, owner validation, and lint checks in a single command with GHA-native output:

  ```bash
  codeowners-cli gha --changed-files-from changed.txt
  ```

  **Checks performed:**
  - Coverage for changed files (fails if uncovered)
  - Coverage for all files (warns only)
  - Owner validation for changed files (fails if invalid)
  - Owner validation for all files (warns only)
  - Lint checks with inline annotations

  **Output:**
  - Human-readable terminal summary
  - `GITHUB_OUTPUT` variables for downstream steps
  - `GITHUB_STEP_SUMMARY` markdown report
  - `::error::` / `::warning::` annotations on PR files

  **Check flags:** `--no-coverage-changed`, `--no-coverage-all`, `--no-owners-changed`, `--no-owners-all`, `--no-lint`

  **Output flags:** `--no-annotations`, `--no-summary`, `--no-outputs`

## [0.14.3] - 2026-02-05

### Removed

- **`unowned-files` diagnostic** - Coverage info is available via `coverage` command; removed redundant diagnostic that wasn't line-specific.

## [0.14.2] - 2026-02-05

### Added

- **`lint --json` includes `pattern` and `owners`** - Diagnostics now include rule context:

  ```json
  {
    "code": "pattern-no-match",
    "line": 5,
    "message": "Pattern matches no files",
    "pattern": "/src/old/**",
    "owners": ["@org/team"]
  }
  ```

- **`validate-owners --json` includes reasons** - Invalid/unknown owners now include why:
  ```json
  {
    "valid": ["@org/team"],
    "invalid": [{ "owner": "@bad", "reason": "not found on GitHub" }],
    "unknown": [{ "owner": "email@x.com", "reason": "email, can't validate" }]
  }
  ```

## [0.14.1] - 2026-02-05

### Changed

- **`check --json` now includes `owned` field** - Distinguishes between files with no matching rule (`owned: false`) and files matching a rule with no owners (`owned: true, owners: []`):
  ```json
  {"src/main.rs": {"owned": true, "rule": "*", "line": 1, "owners": ["@team"]}}
  {"untracked.rs": {"owned": false, "rule": null, "line": null, "owners": []}}
  ```

## [0.14.0] - 2026-02-05

### Added

- **`coverage --json`** - JSON output for coverage stats:

  ```bash
  codeowners-cli coverage --json
  # {"total":100,"owned":95,"unowned":5,"coverage_percent":95.0,"unowned_files":["src/foo.rs",...]}
  ```

- **`validate-owners --json`** - JSON output for owner validation:
  ```bash
  codeowners-cli validate-owners --json
  # {"valid":["@org/team"],"invalid":["@nonexistent"],"unknown":[{"owner":"email@example.com","reason":"..."}]}
  ```

All commands with meaningful output now support `--json`: `lint`, `check`, `coverage`, `validate-owners`, `suggest`, `optimize`.

## [0.13.1] - 2026-02-05

### Added

- **`check --files`** - Named argument for consistency with `coverage` and `validate-owners`:
  ```bash
  codeowners-cli check --files src/main.rs src/lib.rs
  ```

### Changed

- **`check` now uses shared file collection** - All three commands (`check`, `coverage`, `validate-owners`) now share the same `collect_files` implementation

## [0.13.0] - 2026-02-05

### Added

- **`check` multi-file support** - Check ownership of multiple files at once:

  ```bash
  codeowners-cli check src/main.rs src/lib.rs
  ```

- **`check --json`** - JSON output for CI/scripting:

  ```bash
  codeowners-cli check --json src/main.rs
  # {"src/main.rs":{"rule":"*","line":1,"owners":["@team"]}}
  ```

- **`check --files-from` and `--stdin`** - Bulk file checking for GitHub Actions:
  ```bash
  codeowners-cli check --json --files-from changed_files.txt
  git diff --name-only origin/main | codeowners-cli check --json --stdin
  ```

Files with no owner return `{"rule": null, "line": null, "owners": []}`. Exit code 0 for multi-file mode even with unowned files.

## [0.12.6] - 2026-02-03

### Changed

- **`lint --github-actions` is now exclusive** - Only outputs annotations, no human-readable text (avoids duplication in GitHub Actions logs)

## [0.12.5] - 2026-02-03

### Added

- **`lint --github-actions`** - Output GitHub Actions annotations for inline PR feedback:
  ```bash
  codeowners-cli lint --github-actions
  ```

## [0.12.4] - 2026-02-03

### Added

- **`coverage --tree`** - Show unowned files as a directory tree with per-directory counts:
  ```
  src/                         3 files
  ├── handlers/                2 files
  │   ├── navigation.rs
  │   └── semantic.rs
  └── main.rs
  ```

### Fixed

- **LSP syncs document content on save** - Fixes diagnostics showing stale positions when external tools (formatters, linters) modify files on save

### Changed

- **Coverage summary moved to end** - Stats and progress bar now appear after the file list, not before

## [0.12.3] - 2026-02-03

### Fixed

- **File diagnostics now update with content changes** - Diagnostics for non-CODEOWNERS files (e.g. `file-not-owned`) now update correctly when the file is edited

### Changed

- **Snazzier coverage output** - Added visual progress bar, better formatting, and shows ALL unowned files (no 50 file limit)

## [0.12.1] - 2026-02-03

### Added

- **Linux musl builds** - Static binaries for x86_64 and aarch64 musl targets, useful for Alpine Linux and containers.

## [0.12.0] - 2026-02-03

### Added

- **`coverage` file filtering for CI** - Check ownership of specific files instead of the whole repo:

  ```bash
  codeowners-cli coverage --files src/new.rs src/other.rs
  codeowners-cli coverage --files-from changed_files.txt
  git diff --name-only origin/main | codeowners-cli coverage --stdin
  ```

- **`coverage` now exits non-zero** - Returns exit code 1 if any checked files are unowned. Useful for CI to enforce ownership on new files.

- **`lint --strict`** - Exit non-zero on warnings, not just errors. Useful for stricter CI checks.

- **`validate-owners` file filtering** - Only validate owners for rules matching specific files:
  ```bash
  codeowners-cli validate-owners --files src/new.rs src/other.rs
  codeowners-cli validate-owners --files-from changed_files.txt
  git diff --name-only origin/main | codeowners-cli validate-owners --stdin
  ```

## [0.11.1] - 2026-02-03

### Fixed

- **CLI `lint` now respects diagnostic config** - Diagnostic severity overrides from `.codeowners-lsp.toml` now work in CLI, not just LSP.

- **File discovery now uses `git ls-files`** - Switched from walking filesystem with ignore rules to using git's index. Only git-tracked files are considered for pattern matching and coverage. Fixes `.codeowners-lsp.toml` being incorrectly excluded from ownership checks.

- **`file-not-owned` vs `no-owners` are now distinct** - Previously both cases showed the same error. Now:
  - `file-not-owned` (default: error) - No CODEOWNERS rule matches the file
  - `no-owners` (default: hint) - A rule matches but has no owners specified

  This allows disabling one without the other:

  ```toml
  [diagnostics]
  no-owners = "off"        # Don't warn about catch-all rules with no owners
  file-not-owned = "error" # Still error on files with no matching rule
  ```

### Changed

- **Internal: Unified settings module** - CLI and LSP now share config loading code.

## [0.11.0] - 2026-02-03

### Changed

- **Config: `[suggest]` section** - `lookup_cmd` moved into `[suggest]` section. Update your config:

  ```toml
  # Before
  lookup_cmd = "..."

  # After
  [suggest]
  lookup_cmd = "..."
  anchored = true  # optional: prepend / to paths
  ```

- **`fix` command removed** - Use `lint --fix` instead.

### Added

- **`suggest --anchored`** - Prepend `/` to suggested paths (anchored patterns). Also available via config.
- **`lint --fix`** - Auto-fix safe issues (duplicate owners, shadowed rules, no-match patterns).
- **Remove no-match patterns** - Both `lint --fix` and `optimize` now remove patterns that match no files.

## [0.10.4] - 2026-02-03

### Fixed

- **`optimize --write` now works in single pass** - Fixed bug where directory consolidation would create new patterns that were immediately shadowed by catch-all rules like `*`. Consolidations are now skipped if the resulting pattern would be shadowed, eliminating the need for multiple passes.

## [0.10.3] - 2026-02-03

### Fixed

- **Catch-all `*` now correctly shadows ALL previous rules** - Fixed bug where duplicate catch-all detection prevented subsumption checks, so `* @default` at end of file now properly marks all previous rules as shadowed

## [0.10.2] - 2026-02-03

### Fixed

- **Performance: skip subsumption checks for non-subsuming patterns** - Only check shadowing when current pattern could actually subsume others (wildcards, directories), reducing O(n²) to O(n×m) where m is number of wildcards/directories

## [0.10.1] - 2026-02-03

### Fixed

- **LSP diagnostics now detect all shadowed rules** - Previously only tracked wildcards/directories for shadowing detection, missing exact file patterns like `/src/foo.rs` being shadowed by `*`

## [0.10.0] - 2026-02-03

### Fixed - Critical Pattern Matching Overhaul

Complete rewrite of pattern matching to conform to GitHub CODEOWNERS behavior:

- **Anchored vs unanchored patterns** - Leading `/` now properly anchors to root
  - `/docs/` only matches `docs/` at repository root
  - `docs/` (no leading `/`) matches ANY `docs/` directory anywhere in the tree
  - This was a major bug: `docs/` was incorrectly treated as anchored

- **`/*` now correctly matches only root files** - Previously matched everything

- **Catch-all behavior** - `*` and `**` now correctly match all files at any depth

- **Directory pattern semantics**:
  - `/docs/` = anchored, recursive (matches `docs/**`)
  - `/docs/*` = anchored, direct children only (NOT subdirectories)
  - `docs/` = unanchored, matches anywhere in tree

### Changed - Optimize Command

The `optimize` command now uses the corrected pattern engine:

- **Detects shadowed rules using `pattern_subsumes`** - Properly detects when later rules shadow earlier ones based on "last match wins" semantics
- **Catches common footguns**:
  - `docs/ @team` followed by `* @default` → `docs/` is dead
  - `/src/auth/ @security` followed by `/src/ @backend` → `/src/auth/` is dead
  - Unanchored patterns are NOT shadowed by anchored equivalents

- **Directory consolidation** - Only suggests when ALL files in directory have exact same owners

- **Removed risky glob suggestions** - No longer suggests `*.rs` patterns that might match unintended files

### Added - `suggest` command with `lookup_cmd`

- **`lookup_cmd` config option** - External command to resolve email → team/owner
  ```toml
  lookup_cmd = "your-tool lookup {email} | jq -r .team"
  ```
- **Parallel lookups** - Batch email resolution with progress bar
- **Weighted voting** - Aggregates contributors' teams by commit count
- **Fuzzy matching** - Matches lookup results against existing CODEOWNERS owners
- **`--write` flag** - Adds suggestions directly to CODEOWNERS at smart insertion points

### Added - 51 new pattern conformance tests

Comprehensive test suite covering all GitHub CODEOWNERS behaviors:

- Catch-all patterns, extension patterns, anchored/unanchored directories
- Single vs double star wildcards, case sensitivity
- Dead rule detection scenarios, edge cases

## [0.9.1] - 2026-02-03

### Fixed

- **`optimize --write` no longer causes shadowing** - optimized patterns now replace the original rules in-place instead of being appended at the end of the file, which was causing catch-all rules to shadow the optimizations

## [0.9.0] - 2026-02-02

### Added

- **`suggest` command** - Suggests owners for unowned files based on git commit history
  - Analyzes `git shortlog` to find who commits most to files/directories
  - Converts GitHub emails to @username format automatically
  - Confidence scoring based on commit frequency and volume
  - Multiple output formats: `--format human|codeowners|json`
  - Configurable confidence threshold: `--min-confidence 30`

- **`optimize` command** - Suggests ways to simplify CODEOWNERS patterns
  - Detects multiple file rules that could be a single directory pattern
  - Finds patterns with same extension in same directory → glob pattern
  - Identifies redundant/shadowed rules for removal
  - Multiple output formats: `--format human|codeowners|json`

- **Git blame analysis module** (`src/blame.rs`)
  - Analyzes git history per-file and per-directory
  - Aggregates contributor statistics with commit counts
  - Smart owner format conversion (GitHub noreply emails → @username)

### Example Usage

```bash
# Suggest owners for unowned files
codeowners-cli suggest
codeowners-cli suggest --min-confidence 50 --format codeowners

# Find optimization opportunities
codeowners-cli optimize
codeowners-cli optimize --format json --min-files 5
```

## [0.8.0] - 2026-02-02

### Added

- **Signature Help** - pattern syntax documentation while typing glob characters (`*`, `**`, `?`, `[...]`)
- **Selection Range** - smart expand selection: word → owner → all owners → whole rule → section → file
- **Linked Editing** - edit an @owner and all occurrences in the file update simultaneously
- **Pattern Hover** - hover over patterns to see list of matching files (up to 10 shown)
- **Related Diagnostics** - "shadowed rule" diagnostics now link to the shadowing rule for quick navigation

### Changed

- **New handler modules** - `handlers/signature.rs`, `handlers/selection.rs`, `handlers/linked.rs`

## [0.7.0] - 2026-02-02

### Added

- **Document Symbols** - outline view of CODEOWNERS file with sections and rules (Cmd+Shift+O)
- **Workspace Symbols** - search patterns and owners across the file (Cmd+T)
- **Folding Ranges** - collapse comment blocks and section headers
- **Semantic Tokens** - syntax highlighting for patterns, @users, @org/teams, glob characters, comments
- **Find All References** - find all rules containing a specific owner
- **Rename Symbol** - rename an @owner across all rules in one operation
- **Code Lens** - inline display showing file count and owners above each rule

### Changed

- **Refactored handler code** - LSP handlers now in `src/handlers/` module for cleaner organization:
  - `handlers/symbols.rs` - document and workspace symbols
  - `handlers/navigation.rs` - references, rename
  - `handlers/semantic.rs` - semantic tokens, folding ranges
  - `handlers/lens.rs` - code lens

## [0.6.4] - 2026-02-02

### Added

- **CLI lint validates owners** - `codeowners-cli lint` now validates GitHub owners if `validate_owners = true` in config
- **Respects persistent cache** - CLI uses and updates the same cache as LSP, checks staleness (24h)

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
