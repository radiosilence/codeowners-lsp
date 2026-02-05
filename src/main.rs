// Re-export shared modules so `crate::*` paths still resolve within this binary
use codeowners_lsp as lib;
pub use lib::diagnostics;
pub use lib::file_cache;
pub use lib::github;
pub use lib::handlers;
pub use lib::ownership;
pub use lib::parser;
pub use lib::pattern;
pub use lib::settings;
pub use lib::validation;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use codeowners::Owners;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use diagnostics::{compute_diagnostics_sync, DiagnosticConfig};
use file_cache::FileCache;
use github::{GitHubClient, PersistentCache};
use ownership::{apply_safe_fixes, check_file_ownership};
use parser::{
    find_insertion_point_with_owner, find_owner_at_position, format_codeowners,
    parse_codeowners_file, parse_codeowners_file_with_positions, serialize_codeowners,
    CodeownersLine,
};
use pattern::pattern_matches;
use settings::{load_settings_from_path, Settings, CONFIG_FILE, CONFIG_FILE_LOCAL};

struct Backend {
    client: Client,
    workspace_root: RwLock<Option<PathBuf>>,
    codeowners: RwLock<Option<Owners>>,
    codeowners_path: RwLock<Option<PathBuf>>,
    settings: RwLock<Settings>,
    file_cache: RwLock<Option<FileCache>>,
    github_client: Arc<GitHubClient>,
    /// Track open documents to refresh diagnostics when CODEOWNERS changes
    open_documents: RwLock<HashMap<Url, String>>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            workspace_root: RwLock::new(None),
            codeowners: RwLock::new(None),
            codeowners_path: RwLock::new(None),
            settings: RwLock::new(Settings::default()),
            file_cache: RwLock::new(None),
            github_client: Arc::new(GitHubClient::new()),
            open_documents: RwLock::new(HashMap::new()),
        }
    }

    /// Load settings from TOML config files in the workspace
    fn load_config_files(&self) -> Settings {
        let root = self.workspace_root.read().unwrap();
        match root.as_ref() {
            Some(root) => load_settings_from_path(root),
            None => Settings::default(),
        }
    }

    /// Get the GitHub token from settings (resolving env: prefix)
    fn get_github_token(&self) -> Option<String> {
        self.settings.read().unwrap().resolve_token()
    }

    /// Load CODEOWNERS - runs in blocking thread pool
    async fn load_codeowners(&self) -> Option<PathBuf> {
        let root = self.workspace_root.read().unwrap().clone()?;
        let custom_path = self.settings.read().unwrap().path.clone();

        // Heavy work in blocking thread
        let result = tokio::task::spawn_blocking(move || {
            // If custom path is set, use it
            if let Some(custom) = &custom_path {
                let path = root.join(custom);
                if path.exists() {
                    let owners = codeowners::from_path(&path);
                    return Some((Some(owners), Some(path)));
                }
            }

            // Otherwise use the crate's locate function
            if let Some(path) = codeowners::locate(&root) {
                let owners = codeowners::from_path(&path);
                return Some((Some(owners), Some(path)));
            }

            Some((None, None))
        })
        .await
        .ok()
        .flatten();

        // Write results back (fast)
        if let Some((owners, path)) = result {
            *self.codeowners.write().unwrap() = owners;
            *self.codeowners_path.write().unwrap() = path.clone();
            return path;
        }
        None
    }

    /// Load CODEOWNERS rules from buffer content (for unsaved changes)
    fn load_codeowners_from_content(&self, content: &str) {
        use std::io::Cursor;
        let owners = codeowners::from_reader(Cursor::new(content));
        *self.codeowners.write().unwrap() = Some(owners);
    }

    /// Refresh file cache - runs in blocking thread pool
    async fn refresh_file_cache(&self) {
        let Some(root) = self.workspace_root.read().unwrap().clone() else {
            return;
        };

        // Heavy work in blocking thread
        let cache = tokio::task::spawn_blocking(move || FileCache::new(&root))
            .await
            .ok();

        // Write back (fast)
        if let Some(cache) = cache {
            *self.file_cache.write().unwrap() = Some(cache);
        }
    }

    /// Load persistent cache from disk and populate in-memory cache
    fn load_persistent_cache(&self) {
        let root = self.workspace_root.read().unwrap();
        if let Some(root) = root.as_ref() {
            let persistent = PersistentCache::load(root);
            self.github_client.load_from_persistent(&persistent);
        }
    }

    /// Save in-memory cache to disk
    fn save_persistent_cache(&self) {
        let root = self.workspace_root.read().unwrap();
        if let Some(root) = root.as_ref() {
            let persistent = self.github_client.export_to_persistent();
            let _ = persistent.save(root);
        }
    }

    /// Refresh diagnostics for all open documents (call when CODEOWNERS is saved)
    async fn refresh_all_open_documents(&self) {
        let documents: Vec<(Url, String)> = self
            .open_documents
            .read()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        for (uri, text) in documents {
            if self.is_codeowners_file(&uri) {
                let diagnostics = self.compute_diagnostics(&text).await;
                self.client
                    .publish_diagnostics(uri, diagnostics, None)
                    .await;
            } else {
                let line_count = text.lines().count() as u32;
                let diagnostics = self.check_file_not_owned(&uri, line_count);
                self.client
                    .publish_diagnostics(uri, diagnostics, None)
                    .await;
            }
        }
    }

    /// Refresh only file-not-owned diagnostics (cheap, no GitHub validation)
    async fn refresh_file_not_owned_diagnostics(&self) {
        let documents: Vec<_> = self
            .open_documents
            .read()
            .unwrap()
            .iter()
            .filter(|(uri, _)| !self.is_codeowners_file(uri))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        for (uri, text) in documents {
            let line_count = text.lines().count() as u32;
            let diagnostics = self.check_file_not_owned(&uri, line_count);
            self.client
                .publish_diagnostics(uri, diagnostics, None)
                .await;
        }
    }

    /// Validate uncached owners in a spawned task (doesn't block LSP responses)
    async fn validate_owners_task(
        github_client: Arc<GitHubClient>,
        lsp_client: Client,
        settings: Settings,
        uri: Url,
        owners: Vec<diagnostics::OwnerValidationInfo>,
        content: String,
    ) {
        // Check if validation is enabled
        if !settings.validate_owners {
            return;
        }
        let Some(token) = settings.resolve_token() else {
            return;
        };

        // Find owners not already in cache
        let uncached: Vec<_> = owners
            .iter()
            .filter(|(_, _, owner, _)| !github_client.is_cached(owner))
            .map(|(_, _, owner, _)| owner.clone())
            .collect();

        if uncached.is_empty() {
            return;
        }

        // Validate uncached owners and fetch metadata
        let mut any_validated = false;
        for owner in uncached {
            if github_client
                .validate_owner_with_info(&owner, &token)
                .await
                .is_some()
            {
                any_validated = true;
            }
        }

        // If we validated anything, refresh diagnostics
        // (cache save happens on file save or shutdown - skipping here to keep task simple)
        if any_validated {
            let file_cache = None; // Skip pattern matching for speed
            let diag_config = settings.diagnostic_config();
            let (diagnostics, _) = compute_diagnostics_sync(&content, file_cache, &diag_config);
            lsp_client.publish_diagnostics(uri, diagnostics, None).await;
        }
    }

    /// Reload config from TOML files and log the change
    async fn reload_config(&self) {
        let settings = self.load_config_files();
        let settings_info = format!(
            "Config reloaded: individual={:?}, team={:?}, validate_owners={}, diagnostics={} rules",
            settings.individual,
            settings.team,
            settings.validate_owners,
            settings.diagnostics.len()
        );
        *self.settings.write().unwrap() = settings;

        self.client
            .log_message(MessageType::INFO, settings_info)
            .await;
    }

    /// Get CODEOWNERS content from open buffer or disk
    fn get_codeowners_content(&self) -> Option<String> {
        let codeowners_path = self.codeowners_path.read().unwrap();
        let path = codeowners_path.as_ref()?;

        // Try buffer first
        if let Ok(url) = Url::from_file_path(path) {
            let docs = self.open_documents.read().unwrap();
            if let Some(text) = docs.get(&url) {
                return Some(text.clone());
            }
        }

        // Fall back to disk
        fs::read_to_string(path).ok()
    }

    /// Collect all unique owners from CODEOWNERS file
    fn collect_owners_from_codeowners(&self) -> Vec<String> {
        let Some(content) = self.get_codeowners_content() else {
            return Vec::new();
        };

        let lines = parse_codeowners_file_with_positions(&content);
        let mut owners: HashSet<String> = HashSet::new();

        for line in &lines {
            if let CodeownersLine::Rule {
                owners: line_owners,
                ..
            } = &line.content
            {
                for owner in line_owners {
                    if owner.starts_with('@') {
                        owners.insert(owner.clone());
                    }
                }
            }
        }

        owners.into_iter().collect()
    }

    /// Ownership status for a file
    /// - `None` = no CODEOWNERS rule matches this file
    /// - `Some(None)` = a rule matches but has no owners
    /// - `Some(Some(owners))` = a rule matches with owners
    fn get_ownership_status(&self, uri: &Url) -> Option<Option<String>> {
        let root = self.workspace_root.read().unwrap();
        let root = root.as_ref()?;

        let file_path = uri.to_file_path().ok()?;
        let relative_path = file_path.strip_prefix(root).ok()?;

        let codeowners = self.codeowners.read().unwrap();
        let codeowners = codeowners.as_ref()?;

        // .of() returns None if no rule matches, Some(vec) if a rule matches
        let owners = codeowners.of(relative_path)?;
        let owner_strs: Vec<String> = owners.iter().map(|o| o.to_string()).collect();

        if owner_strs.is_empty() {
            Some(None) // Rule matches but no owners
        } else {
            Some(Some(owner_strs.join(" "))) // Rule matches with owners
        }
    }

    fn get_owners_for_file(&self, uri: &Url) -> Option<String> {
        self.get_ownership_status(uri).flatten()
    }

    /// Compute diagnostics for the CODEOWNERS file
    async fn compute_diagnostics(&self, content: &str) -> Vec<Diagnostic> {
        // Check if GitHub validation is enabled and get diagnostic config
        let (validate_owners, token, diag_config) = {
            let settings = self.settings.read().unwrap();
            let enabled = settings.validate_owners;
            let token = self.get_github_token();
            let config = settings.diagnostic_config();
            (enabled && token.is_some(), token, config)
        };

        // Phase 1: Synchronous diagnostics (holds file_cache lock)
        let (mut diagnostics, owners_to_validate) = {
            let file_cache = self.file_cache.read().unwrap();
            compute_diagnostics_sync(content, file_cache.as_ref(), &diag_config)
        };

        // Phase 2: Async GitHub validation (no locks held)
        if validate_owners {
            if let Some(token) = token {
                diagnostics::add_github_diagnostics(
                    &mut diagnostics,
                    owners_to_validate,
                    &self.github_client,
                    &token,
                    &diag_config,
                )
                .await;
            }
        }

        diagnostics
    }

    /// Check if a URI is the CODEOWNERS file
    fn is_codeowners_file(&self, uri: &Url) -> bool {
        let codeowners_path = self.codeowners_path.read().unwrap();
        if let Some(ref path) = *codeowners_path {
            if let Ok(file_path) = uri.to_file_path() {
                return file_path == *path;
            }
        }
        false
    }

    /// Check if a file is owned and publish diagnostic if not
    /// Returns diagnostics for an unowned file (full file error)
    fn check_file_not_owned(&self, uri: &Url, line_count: u32) -> Vec<Diagnostic> {
        // If no CODEOWNERS file exists, don't complain about ownership
        if self.codeowners_path.read().unwrap().is_none() {
            return Vec::new();
        }

        // Skip CODEOWNERS file itself
        if self.is_codeowners_file(uri) {
            return Vec::new();
        }

        // Skip files outside the workspace root
        if let Some(root) = self.workspace_root.read().unwrap().as_ref() {
            if let Ok(path) = uri.to_file_path() {
                if !path.starts_with(root) {
                    return Vec::new();
                }
            }
        }

        let config = {
            let settings = self.settings.read().unwrap();
            settings.diagnostic_config()
        };

        // Check ownership status:
        // - None = no rule matches (file-not-owned)
        // - Some(None) = rule matches but no owners (no-owners)
        // - Some(Some(_)) = rule matches with owners (no diagnostic)
        let ownership = self.get_ownership_status(uri);

        let (code, message, default_severity) = match ownership {
            Some(Some(_)) => return Vec::new(), // Has owners, no diagnostic
            Some(None) => {
                // Rule matches but no owners specified
                (
                    diagnostics::codes::NO_OWNERS,
                    "matched by rule with no owners",
                    DiagnosticSeverity::HINT,
                )
            }
            None => {
                // No rule matches this file
                (
                    diagnostics::codes::FILE_NOT_OWNED,
                    "has no CODEOWNERS entry",
                    DiagnosticSeverity::ERROR,
                )
            }
        };

        // Check if this diagnostic is enabled
        let Some(severity) = config.get(code, default_severity) else {
            return Vec::new(); // Disabled
        };

        // Get relative path for message
        let relative_path = {
            let root = self.workspace_root.read().unwrap();
            if let Some(root) = root.as_ref() {
                uri.to_file_path().ok().and_then(|p| {
                    p.strip_prefix(root)
                        .ok()
                        .map(|r| r.to_string_lossy().to_string())
                })
            } else {
                None
            }
        };

        let path_display = relative_path.unwrap_or_else(|| uri.path().to_string());

        // Full file diagnostic
        vec![Diagnostic {
            range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: line_count.saturating_sub(1),
                    character: u32::MAX, // End of last line
                },
            },
            severity: Some(severity),
            code: Some(NumberOrString::String(code.to_string())),
            source: Some("codeowners".to_string()),
            message: format!("File '{}' {}", path_display, message),
            ..Default::default()
        }]
    }

    /// Generate code actions for CODEOWNERS file diagnostics
    async fn codeowners_code_actions(
        &self,
        params: &CodeActionParams,
    ) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        let Some(content) = self.get_codeowners_content() else {
            return Ok(None);
        };
        let lines: Vec<&str> = content.lines().collect();
        let mut actions = Vec::new();

        // Check each diagnostic in the request
        for diagnostic in &params.context.diagnostics {
            let line_num = diagnostic.range.start.line as usize;

            // Handle "shadowed rule" diagnostics - offer to remove the dead rule
            if diagnostic.message.contains("shadowed by") && line_num < lines.len() {
                let delete_range = Range {
                    start: Position {
                        line: line_num as u32,
                        character: 0,
                    },
                    end: Position {
                        line: (line_num + 1) as u32,
                        character: 0,
                    },
                };

                let mut changes = HashMap::new();
                changes.insert(
                    uri.clone(),
                    vec![TextEdit {
                        range: delete_range,
                        new_text: String::new(),
                    }],
                );

                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Remove shadowed rule".to_string(),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diagnostic.clone()]),
                    edit: Some(WorkspaceEdit {
                        changes: Some(changes),
                        ..Default::default()
                    }),
                    command: None,
                    is_preferred: Some(true),
                    disabled: None,
                    data: None,
                }));
            }

            // Handle "duplicate owner" diagnostics - offer to dedupe
            if diagnostic.message.contains("Duplicate owner") && line_num < lines.len() {
                let line = lines[line_num];
                let parts: Vec<&str> = line.split_whitespace().collect();

                if !parts.is_empty() {
                    let pattern = parts[0];
                    let owners: Vec<&str> = parts[1..].to_vec();

                    // Dedupe owners while preserving order
                    let mut seen = HashSet::new();
                    let deduped: Vec<&str> =
                        owners.into_iter().filter(|o| seen.insert(*o)).collect();

                    let new_line = format!("{} {}", pattern, deduped.join(" "));

                    let edit_range = Range {
                        start: Position {
                            line: line_num as u32,
                            character: 0,
                        },
                        end: Position {
                            line: line_num as u32,
                            character: line.len() as u32,
                        },
                    };

                    let mut changes = HashMap::new();
                    changes.insert(
                        uri.clone(),
                        vec![TextEdit {
                            range: edit_range,
                            new_text: new_line,
                        }],
                    );

                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: "Remove duplicate owners".to_string(),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![diagnostic.clone()]),
                        edit: Some(WorkspaceEdit {
                            changes: Some(changes),
                            ..Default::default()
                        }),
                        command: None,
                        is_preferred: Some(true),
                        disabled: None,
                        data: None,
                    }));
                }
            }

            // Handle "no owners" diagnostics - offer to add configured owners
            if diagnostic.message.contains("No owners specified") && line_num < lines.len() {
                let line = lines[line_num];
                let pattern = line.split_whitespace().next().unwrap_or("");
                let settings = self.settings.read().unwrap();

                if let Some(ref individual) = settings.individual {
                    let new_line = format!("{} {}", pattern, individual);
                    let edit_range = Range {
                        start: Position {
                            line: line_num as u32,
                            character: 0,
                        },
                        end: Position {
                            line: line_num as u32,
                            character: line.len() as u32,
                        },
                    };

                    let mut changes = HashMap::new();
                    changes.insert(
                        uri.clone(),
                        vec![TextEdit {
                            range: edit_range,
                            new_text: new_line,
                        }],
                    );

                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: format!("Add {} as owner", individual),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![diagnostic.clone()]),
                        edit: Some(WorkspaceEdit {
                            changes: Some(changes),
                            ..Default::default()
                        }),
                        command: None,
                        is_preferred: None,
                        disabled: None,
                        data: None,
                    }));
                }

                if let Some(ref team) = settings.team {
                    let new_line = format!("{} {}", pattern, team);
                    let edit_range = Range {
                        start: Position {
                            line: line_num as u32,
                            character: 0,
                        },
                        end: Position {
                            line: line_num as u32,
                            character: line.len() as u32,
                        },
                    };

                    let mut changes = HashMap::new();
                    changes.insert(
                        uri.clone(),
                        vec![TextEdit {
                            range: edit_range,
                            new_text: new_line,
                        }],
                    );

                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: format!("Add {} as owner", team),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![diagnostic.clone()]),
                        edit: Some(WorkspaceEdit {
                            changes: Some(changes),
                            ..Default::default()
                        }),
                        command: None,
                        is_preferred: None,
                        disabled: None,
                        data: None,
                    }));
                }
            }

            // Handle "files have no code owners" (coverage) - offer to add catch-all rule
            if diagnostic.message.contains("files have no code owners") {
                let settings = self.settings.read().unwrap();
                let last_line = lines.len() as u32;

                if let Some(ref individual) = settings.individual {
                    let new_line = format!("* {}\n", individual);
                    let insert_pos = Position {
                        line: last_line,
                        character: 0,
                    };

                    let mut changes = HashMap::new();
                    changes.insert(
                        uri.clone(),
                        vec![TextEdit {
                            range: Range {
                                start: insert_pos,
                                end: insert_pos,
                            },
                            new_text: new_line,
                        }],
                    );

                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: format!("Add catch-all rule: * {}", individual),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![diagnostic.clone()]),
                        edit: Some(WorkspaceEdit {
                            changes: Some(changes),
                            ..Default::default()
                        }),
                        command: None,
                        is_preferred: None,
                        disabled: None,
                        data: None,
                    }));
                }

                if let Some(ref team) = settings.team {
                    let new_line = format!("* {}\n", team);
                    let insert_pos = Position {
                        line: last_line,
                        character: 0,
                    };

                    let mut changes = HashMap::new();
                    changes.insert(
                        uri.clone(),
                        vec![TextEdit {
                            range: Range {
                                start: insert_pos,
                                end: insert_pos,
                            },
                            new_text: new_line,
                        }],
                    );

                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: format!("Add catch-all rule: * {}", team),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![diagnostic.clone()]),
                        edit: Some(WorkspaceEdit {
                            changes: Some(changes),
                            ..Default::default()
                        }),
                        command: None,
                        is_preferred: None,
                        disabled: None,
                        data: None,
                    }));
                }
            }
        }

        // Add "Fix all" source action if there are fixable issues
        let fix_result = apply_safe_fixes(&content, None);
        if !fix_result.fixes.is_empty() {
            let line_count = content.lines().count();
            let last_line_len = content.lines().last().map(|l| l.len()).unwrap_or(0);

            let mut changes = HashMap::new();
            changes.insert(
                uri.clone(),
                vec![TextEdit {
                    range: Range {
                        start: Position {
                            line: 0,
                            character: 0,
                        },
                        end: Position {
                            line: line_count as u32,
                            character: last_line_len as u32,
                        },
                    },
                    new_text: fix_result.content,
                }],
            );

            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Fix all safe issues ({} fixes)", fix_result.fixes.len()),
                kind: Some(CodeActionKind::SOURCE_FIX_ALL),
                diagnostics: None,
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..Default::default()
                }),
                command: None,
                is_preferred: None,
                disabled: None,
                data: None,
            }));
        }

        if actions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(actions))
        }
    }

    /// Publish diagnostics for the CODEOWNERS file
    async fn publish_codeowners_diagnostics(&self) {
        let codeowners_path = self.codeowners_path.read().unwrap().clone();
        if let Some(path) = codeowners_path {
            if let Ok(content) = fs::read_to_string(&path) {
                let diagnostics = self.compute_diagnostics(&content).await;
                if let Ok(uri) = Url::from_file_path(&path) {
                    self.client
                        .publish_diagnostics(uri, diagnostics, None)
                        .await;
                }
            }
        }
    }

    /// Add a new ownership entry to the CODEOWNERS file
    fn add_ownership(&self, pattern: &str, owner: &str) -> std::result::Result<(), String> {
        let codeowners_path = self.codeowners_path.read().unwrap();
        let path = codeowners_path
            .as_ref()
            .ok_or_else(|| "No CODEOWNERS file found".to_string())?
            .clone();
        drop(codeowners_path);

        let content =
            fs::read_to_string(&path).map_err(|e| format!("Failed to read CODEOWNERS: {}", e))?;

        let mut lines = parse_codeowners_file(&content);
        let insertion_point = find_insertion_point_with_owner(&lines, pattern, Some(owner));

        lines.insert(
            insertion_point,
            CodeownersLine::Rule {
                pattern: pattern.to_string(),
                owners: vec![owner.to_string()],
            },
        );

        let new_content = serialize_codeowners(&lines);
        let new_content = if new_content.ends_with('\n') {
            new_content
        } else {
            format!("{}\n", new_content)
        };

        fs::write(&path, new_content).map_err(|e| format!("Failed to write CODEOWNERS: {}", e))?;

        Ok(())
    }

    /// Add an owner to an existing entry that matches the file
    fn add_to_existing(&self, pattern: &str, owner: &str) -> std::result::Result<(), String> {
        let codeowners_path = self.codeowners_path.read().unwrap();
        let path = codeowners_path
            .as_ref()
            .ok_or_else(|| "No CODEOWNERS file found".to_string())?
            .clone();
        drop(codeowners_path);

        let content =
            fs::read_to_string(&path).map_err(|e| format!("Failed to read CODEOWNERS: {}", e))?;

        let mut lines = parse_codeowners_file(&content);

        // Strip leading slash from pattern to get relative path
        let relative_path = pattern.trim_start_matches('/');

        let codeowners = self.codeowners.read().unwrap();
        if codeowners.is_none() {
            return Err("No CODEOWNERS loaded".to_string());
        }

        // Find the matching rule by checking which pattern in our parsed lines
        // would match this file. We iterate in reverse since last match wins.
        let mut matching_idx = None;
        for (idx, line) in lines.iter().enumerate().rev() {
            if let CodeownersLine::Rule {
                pattern: rule_pattern,
                ..
            } = line
            {
                if pattern_matches(rule_pattern, relative_path) {
                    matching_idx = Some(idx);
                    break;
                }
            }
        }

        let idx = matching_idx.ok_or("No matching rule found")?;

        if let CodeownersLine::Rule { owners, .. } = &mut lines[idx] {
            if !owners.contains(&owner.to_string()) {
                owners.push(owner.to_string());
            }
        }

        let new_content = serialize_codeowners(&lines);
        let new_content = if new_content.ends_with('\n') {
            new_content
        } else {
            format!("{}\n", new_content)
        };

        fs::write(&path, new_content).map_err(|e| format!("Failed to write CODEOWNERS: {}", e))?;

        Ok(())
    }

    /// Find the CODEOWNERS rule that matches a given file
    fn find_matching_rule(&self, file_path: &str) -> Option<(u32, String)> {
        let codeowners_path = self.codeowners_path.read().unwrap();
        let path = codeowners_path.as_ref()?;
        let content = fs::read_to_string(path).ok()?;

        check_file_ownership(&content, file_path).map(|r| (r.line_number, r.pattern))
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        if let Some(root_uri) = &params.root_uri {
            if let Ok(path) = root_uri.to_file_path() {
                *self.workspace_root.write().unwrap() = Some(path);
            }
        }

        // Load config: TOML files first, then JSON init options override
        let mut settings = self.load_config_files();
        if let Some(opts) = &params.initialization_options {
            if let Ok(json_settings) = serde_json::from_value::<Settings>(opts.clone()) {
                settings.merge(json_settings);
            }
        }
        // Log loaded config for debugging
        let settings_info = format!(
            "Config loaded: individual={:?}, team={:?}, validate_owners={}, diagnostics={} rules",
            settings.individual,
            settings.team,
            settings.validate_owners,
            settings.diagnostics.len()
        );
        *self.settings.write().unwrap() = settings;

        self.load_codeowners().await;
        self.refresh_file_cache().await;
        self.load_persistent_cache();

        // Check if we should run background validation
        let (should_validate, token) = {
            let settings = self.settings.read().unwrap();
            (settings.validate_owners, self.get_github_token())
        };

        // Collect owners for validation
        let owners_to_validate = self.collect_owners_from_codeowners();

        // Log after init (client not ready during initialize, so spawn task)
        let client = self.client.clone();
        let codeowners_found = self.codeowners_path.read().unwrap().is_some();

        // Background validation task
        if let (true, Some(token)) = (should_validate && !owners_to_validate.is_empty(), token) {
            let github_client = self.github_client.clone();
            let workspace_root = self.workspace_root.read().unwrap().clone();
            let client_clone = client.clone();

            tokio::spawn(async move {
                // Small delay to ensure client is ready
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                client_clone
                    .log_message(
                        MessageType::INFO,
                        format!(
                            "Starting background validation of {} owners...",
                            owners_to_validate.len()
                        ),
                    )
                    .await;

                // Validate in parallel (5 concurrent)
                use futures::stream::{self, StreamExt};
                let results: Vec<_> = stream::iter(owners_to_validate)
                    .map(|owner| {
                        let client = github_client.clone();
                        let token = token.clone();
                        async move {
                            let result = client.validate_owner(&owner, &token).await;
                            (owner, result)
                        }
                    })
                    .buffer_unordered(5)
                    .collect()
                    .await;

                let valid_count = results.iter().filter(|(_, r)| *r == Some(true)).count();
                let invalid_count = results.iter().filter(|(_, r)| *r == Some(false)).count();

                client_clone
                    .log_message(
                        MessageType::INFO,
                        format!(
                            "Background validation complete: {} valid, {} invalid",
                            valid_count, invalid_count
                        ),
                    )
                    .await;

                // Save to persistent cache
                if let Some(root) = workspace_root {
                    let persistent = github_client.export_to_persistent();
                    let _ = persistent.save(&root);
                }
            });
        }

        tokio::spawn(async move {
            // Small delay to ensure client is ready
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            client.log_message(MessageType::INFO, settings_info).await;
            client
                .log_message(
                    MessageType::INFO,
                    format!("CODEOWNERS found: {}", codeowners_found),
                )
                .await;
        });

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(true),
                        })),
                        ..Default::default()
                    },
                )),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![
                        "codeowners.takeOwnership.individual".to_string(),
                        "codeowners.takeOwnership.team".to_string(),
                        "codeowners.takeOwnership.custom".to_string(),
                        "codeowners.addToExisting.individual".to_string(),
                        "codeowners.addToExisting.team".to_string(),
                        "codeowners.addToExisting.custom".to_string(),
                    ],
                    work_done_progress_options: Default::default(),
                }),
                definition_provider: Some(OneOf::Left(false)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["/".to_string(), "@".to_string()]),
                    ..Default::default()
                }),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                references_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: Default::default(),
                })),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: SemanticTokensLegend {
                                token_types: vec![
                                    SemanticTokenType::COMMENT,
                                    SemanticTokenType::STRING,   // pattern
                                    SemanticTokenType::VARIABLE, // @user
                                    SemanticTokenType::CLASS,    // @org/team
                                    SemanticTokenType::OPERATOR, // glob chars: * ? [ ]
                                ],
                                token_modifiers: vec![],
                            },
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            range: Some(false),
                            work_done_progress_options: Default::default(),
                        },
                    ),
                ),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec![
                        "*".to_string(),
                        "?".to_string(),
                        "[".to_string(),
                    ]),
                    retrigger_characters: None,
                    work_done_progress_options: Default::default(),
                }),
                selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
                linked_editing_range_provider: Some(LinkedEditingRangeServerCapabilities::Simple(
                    false,
                )),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "codeowners-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            offset_encoding: None,
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        // Register file watchers for config files and CODEOWNERS
        let registrations = vec![Registration {
            id: "file-watcher".to_string(),
            method: "workspace/didChangeWatchedFiles".to_string(),
            register_options: Some(
                serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                    watchers: vec![
                        // Config files
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String(format!("**/{}", CONFIG_FILE)),
                            kind: Some(WatchKind::all()),
                        },
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String(format!("**/{}", CONFIG_FILE_LOCAL)),
                            kind: Some(WatchKind::all()),
                        },
                        // CODEOWNERS files (all common locations)
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/CODEOWNERS".to_string()),
                            kind: Some(WatchKind::all()),
                        },
                    ],
                })
                .unwrap(),
            ),
        }];

        if let Err(e) = self.client.register_capability(registrations).await {
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!("Failed to register config file watchers: {}", e),
                )
                .await;
        }

        self.publish_codeowners_diagnostics().await;
    }

    async fn shutdown(&self) -> Result<()> {
        // Save GitHub cache to disk before exiting
        self.save_persistent_cache();
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = &params.text_document.uri;
        let text = params.text_document.text;

        // Track open document
        self.open_documents
            .write()
            .unwrap()
            .insert(uri.clone(), text.clone());

        if self.is_codeowners_file(uri) {
            let diagnostics = self.compute_diagnostics(&text).await;
            self.client
                .publish_diagnostics(uri.clone(), diagnostics, None)
                .await;
        } else {
            // Check if file has no CODEOWNERS entry
            let line_count = text.lines().count() as u32;
            let diagnostics = self.check_file_not_owned(uri, line_count);
            if !diagnostics.is_empty() {
                self.client
                    .publish_diagnostics(uri.clone(), diagnostics, None)
                    .await;
            }
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = &params.text_document.uri;
        self.open_documents.write().unwrap().remove(uri);
        // Clear diagnostics for closed file
        self.client
            .publish_diagnostics(uri.clone(), vec![], None)
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = &params.text_document.uri;
        if let Some(change) = params.content_changes.first() {
            // Get previous content for diffing (before we update)
            let previous_content = self.open_documents.read().unwrap().get(uri).cloned();

            // Update tracked content
            self.open_documents
                .write()
                .unwrap()
                .insert(uri.clone(), change.text.clone());

            if self.is_codeowners_file(uri) {
                // Parse CODEOWNERS from buffer content (handles unsaved changes)
                self.load_codeowners_from_content(&change.text);

                // Lightweight diagnostics for CODEOWNERS:
                // - NO file cache (skip expensive pattern-no-match checks)
                // - NO GitHub validation (done async below for uncached owners)
                let diag_config = {
                    let settings = self.settings.read().unwrap();
                    settings.diagnostic_config()
                };
                let (mut diagnostics, owners_to_validate) =
                    compute_diagnostics_sync(&change.text, None, &diag_config);

                // Check changed lines for pattern matches (real-time feedback)
                if let Some(ref prev) = previous_content {
                    let changed_lines = find_changed_lines(prev, &change.text);
                    if !changed_lines.is_empty() {
                        let file_cache = self.file_cache.read().unwrap();
                        if let Some(ref cache) = *file_cache {
                            let extra_diags = check_patterns_for_lines(
                                &change.text,
                                &changed_lines,
                                cache,
                                &diag_config,
                            );
                            diagnostics.extend(extra_diags);
                        }
                    }
                }

                self.client
                    .publish_diagnostics(uri.clone(), diagnostics, None)
                    .await;

                // Refresh file-not-owned diagnostics for other open files (cheap)
                self.refresh_file_not_owned_diagnostics().await;

                // Tell editor to refresh inlay hints (ownership may have changed)
                let _ = self.client.inlay_hint_refresh().await;

                // Spawn background validation for uncached owners (fire-and-forget)
                // Don't await - we don't want to block typing while hitting GitHub API
                {
                    let client = self.github_client.clone();
                    let lsp_client = self.client.clone();
                    let settings = self.settings.read().unwrap().clone();
                    let content = change.text.clone();
                    let uri = uri.clone();
                    tokio::spawn(async move {
                        Self::validate_owners_task(
                            client,
                            lsp_client,
                            settings,
                            uri,
                            owners_to_validate,
                            content,
                        )
                        .await;
                    });
                }
            } else {
                // Non-CODEOWNERS file changed - update its diagnostics with new line count
                let line_count = change.text.lines().count() as u32;
                let diagnostics = self.check_file_not_owned(uri, line_count);
                self.client
                    .publish_diagnostics(uri.clone(), diagnostics, None)
                    .await;
            }
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = &params.text_document.uri;

        // Update open_documents with saved text if provided (ensures sync)
        if let Some(text) = &params.text {
            self.open_documents
                .write()
                .unwrap()
                .insert(uri.clone(), text.clone());
        }

        if self.is_codeowners_file(uri) {
            self.load_codeowners().await;
            self.refresh_file_cache().await;

            // Refresh diagnostics on ALL open files (file-not-owned may have changed)
            self.refresh_all_open_documents().await;

            // Tell editor to refresh inlay hints (ownership may have changed)
            let _ = self.client.inlay_hint_refresh().await;

            // Trigger background validation for any uncached owners (fire-and-forget on save too)
            if let Some(content) = self.get_codeowners_content() {
                let diag_config = {
                    let settings = self.settings.read().unwrap();
                    settings.diagnostic_config()
                };
                let (_, owners_to_validate) =
                    compute_diagnostics_sync(&content, None, &diag_config);

                let client = self.github_client.clone();
                let lsp_client = self.client.clone();
                let settings = self.settings.read().unwrap().clone();
                let uri = uri.clone();
                tokio::spawn(async move {
                    Self::validate_owners_task(
                        client,
                        lsp_client,
                        settings,
                        uri,
                        owners_to_validate,
                        content,
                    )
                    .await;
                });
            }
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        // Special handling for CODEOWNERS file - hover over @owners or patterns
        if self.is_codeowners_file(uri) {
            if let Some(content) = self.get_codeowners_content() {
                let lines: Vec<&str> = content.lines().collect();
                let line_idx = position.line as usize;
                if line_idx < lines.len() {
                    let line = lines[line_idx];
                    let char_idx = position.character as usize;

                    // Find if we're hovering over an @owner
                    if let Some(owner) = find_owner_at_position(line, char_idx) {
                        let info = self.github_client.get_owner_info(&owner);
                        let formatted = format_owner_hover(&owner, info.as_ref());
                        return Ok(Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: formatted,
                            }),
                            range: None,
                        }));
                    }

                    // Check if we're hovering over a pattern (first token before @)
                    let parsed = parse_codeowners_file_with_positions(&content);
                    if let Some(parsed_line) =
                        parsed.iter().find(|p| p.line_number == position.line)
                    {
                        if let CodeownersLine::Rule { pattern, .. } = &parsed_line.content {
                            // Check if cursor is in pattern region
                            if char_idx >= parsed_line.pattern_start as usize
                                && char_idx <= parsed_line.pattern_end as usize
                            {
                                let file_cache = self.file_cache.read().unwrap();
                                if let Some(ref cache) = *file_cache {
                                    let matches = cache.get_matches(pattern);
                                    let count = matches.len();
                                    let formatted = if count == 0 {
                                        format!("**Pattern:** `{}`\n\n*No matching files*", pattern)
                                    } else {
                                        let sample: Vec<&str> =
                                            matches.iter().take(10).map(|s| s.as_str()).collect();
                                        let files_list = sample
                                            .iter()
                                            .map(|f| format!("- `{}`", f))
                                            .collect::<Vec<_>>()
                                            .join("\n");
                                        let more = if count > 10 {
                                            format!("\n\n*...and {} more*", count - 10)
                                        } else {
                                            String::new()
                                        };
                                        format!(
                                            "**Pattern:** `{}`\n\n**Matches {} {}:**\n{}{}",
                                            pattern,
                                            count,
                                            if count == 1 { "file" } else { "files" },
                                            files_list,
                                            more
                                        )
                                    };
                                    return Ok(Some(Hover {
                                        contents: HoverContents::Markup(MarkupContent {
                                            kind: MarkupKind::Markdown,
                                            value: formatted,
                                        }),
                                        range: None,
                                    }));
                                }
                            }
                        }
                    }
                }
            }
            return Ok(None);
        }

        // Get relative path for rule lookup
        let relative_path = {
            let root = self.workspace_root.read().unwrap();
            if let Some(root) = root.as_ref() {
                uri.to_file_path().ok().and_then(|p| {
                    p.strip_prefix(root)
                        .ok()
                        .map(|r| r.to_string_lossy().to_string())
                })
            } else {
                None
            }
        };

        // Get the matching rule info (line number, pattern)
        let rule_info = relative_path
            .as_ref()
            .and_then(|path| self.find_matching_rule(path));

        // Build the rule link if we have CODEOWNERS path and rule info
        let rule_link = if let Some((line_num, pattern)) = &rule_info {
            let codeowners_path = self.codeowners_path.read().unwrap();
            if let Some(path) = codeowners_path.as_ref() {
                if let Ok(codeowners_uri) = Url::from_file_path(path) {
                    // Line numbers in URIs are 1-indexed
                    Some(format!(
                        "\n\n[`{}`]({}#L{}) (line {})",
                        pattern,
                        codeowners_uri,
                        line_num + 1,
                        line_num + 1
                    ))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        let formatted = match self.get_owners_for_file(uri) {
            None => "**Owned by nobody**".to_string(),
            Some(owners) => {
                let owner_list: Vec<&str> = owners.split_whitespace().collect();

                // Look up owner info from cache for rich hover
                let format_with_cache = |owner: &str| -> String {
                    let info = self.github_client.get_owner_info(owner);
                    format_owner_with_info(owner, info.as_ref())
                };

                let owners_text = if owner_list.len() == 1 {
                    format!("**Owner:** {}", format_with_cache(owner_list[0]))
                } else {
                    let list = owner_list
                        .iter()
                        .map(|o| format!("- {}", format_with_cache(o)))
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("**Owners:**\n{}", list)
                };

                // Append rule link if available
                if let Some(link) = rule_link {
                    format!("{}{}", owners_text, link)
                } else {
                    owners_text
                }
            }
        };

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: formatted,
            }),
            range: None,
        }))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;

        if self.is_codeowners_file(uri) {
            return Ok(None);
        }

        let root = self.workspace_root.read().unwrap();
        let root = match root.as_ref() {
            Some(r) => r.clone(),
            None => return Ok(None),
        };
        drop(root);

        let file_path = match uri.to_file_path() {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };

        let relative_path =
            match file_path.strip_prefix(self.workspace_root.read().unwrap().as_ref().unwrap()) {
                Ok(p) => p.to_string_lossy().to_string(),
                Err(_) => return Ok(None),
            };

        if let Some((line_number, _pattern)) = self.find_matching_rule(&relative_path) {
            let codeowners_path = self.codeowners_path.read().unwrap();
            if let Some(path) = codeowners_path.as_ref() {
                if let Ok(codeowners_uri) = Url::from_file_path(path) {
                    return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                        uri: codeowners_uri,
                        range: Range {
                            start: Position {
                                line: line_number,
                                character: 0,
                            },
                            end: Position {
                                line: line_number,
                                character: u32::MAX,
                            },
                        },
                    })));
                }
            }
        }

        Ok(None)
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;

        if self.is_codeowners_file(uri) {
            return self.codeowners_code_actions(&params).await;
        }

        let file_path = match uri.to_file_path() {
            Ok(p) => p,
            Err(_) => {
                self.client
                    .log_message(MessageType::LOG, "code_action: uri.to_file_path() failed")
                    .await;
                return Ok(None);
            }
        };

        let root = {
            let guard = self.workspace_root.read().unwrap();
            guard.as_ref().cloned()
        };
        let root = match root {
            Some(r) => r,
            None => {
                self.client
                    .log_message(MessageType::LOG, "code_action: no workspace root")
                    .await;
                return Ok(None);
            }
        };

        let relative_path = match file_path.strip_prefix(&root) {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => {
                self.client
                    .log_message(
                        MessageType::LOG,
                        format!(
                            "code_action: file {} outside workspace {}",
                            file_path.display(),
                            root.display()
                        ),
                    )
                    .await;
                return Ok(None);
            }
        };

        let has_existing_owners = self.get_owners_for_file(uri).is_some();
        let (individual, team) = {
            let settings = self.settings.read().unwrap();
            (settings.individual.clone(), settings.team.clone())
        };

        let mut actions = Vec::new();

        let make_action = |title: String, command: &str, pattern: &str, owner: Option<&str>| {
            let mut args = vec![
                serde_json::Value::String(uri.to_string()),
                serde_json::Value::String(pattern.to_string()),
            ];
            if let Some(o) = owner {
                args.push(serde_json::Value::String(o.to_string()));
            }

            CodeActionOrCommand::CodeAction(CodeAction {
                title,
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: None,
                edit: None,
                command: Some(Command {
                    title: String::new(),
                    command: command.to_string(),
                    arguments: Some(args),
                }),
                is_preferred: None,
                disabled: None,
                data: None,
            })
        };

        let file_pattern = format!("/{}", relative_path);

        if has_existing_owners {
            if let Some(ref ind) = individual {
                actions.push(make_action(
                    format!("Add {} to existing CODEOWNERS entry", ind),
                    "codeowners.addToExisting.individual",
                    &file_pattern,
                    Some(ind),
                ));
                actions.push(make_action(
                    format!("Take ownership as {} (new specific entry)", ind),
                    "codeowners.takeOwnership.individual",
                    &file_pattern,
                    Some(ind),
                ));
            }
            if let Some(ref t) = team {
                actions.push(make_action(
                    format!("Add {} to existing CODEOWNERS entry", t),
                    "codeowners.addToExisting.team",
                    &file_pattern,
                    Some(t),
                ));
                actions.push(make_action(
                    format!("Take ownership as {} (new specific entry)", t),
                    "codeowners.takeOwnership.team",
                    &file_pattern,
                    Some(t),
                ));
            }
            actions.push(make_action(
                "Add custom owner to existing entry...".to_string(),
                "codeowners.addToExisting.custom",
                &file_pattern,
                None,
            ));
            actions.push(make_action(
                "Take ownership as custom (new specific entry)...".to_string(),
                "codeowners.takeOwnership.custom",
                &file_pattern,
                None,
            ));
        } else {
            if let Some(ref ind) = individual {
                actions.push(make_action(
                    format!("Take ownership as {}", ind),
                    "codeowners.takeOwnership.individual",
                    &file_pattern,
                    Some(ind),
                ));
            }
            if let Some(ref t) = team {
                actions.push(make_action(
                    format!("Take ownership as {}", t),
                    "codeowners.takeOwnership.team",
                    &file_pattern,
                    Some(t),
                ));
            }
            actions.push(make_action(
                "Take ownership as custom...".to_string(),
                "codeowners.takeOwnership.custom",
                &file_pattern,
                None,
            ));
        }

        if actions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(actions))
        }
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = &params.text_document.uri;
        let range = params.range;

        if self.is_codeowners_file(uri) {
            if let Some(content) = self.get_codeowners_content() {
                let lines = parse_codeowners_file_with_positions(&content);
                let file_cache = self.file_cache.read().unwrap();

                if let Some(ref cache) = *file_cache {
                    let hints: Vec<InlayHint> = lines
                        .iter()
                        // Only compute hints for visible range
                        .filter(|line| {
                            line.line_number >= range.start.line
                                && line.line_number <= range.end.line
                        })
                        .filter_map(|line| {
                            if let CodeownersLine::Rule { pattern, owners, .. } = &line.content {
                                // Compute and cache count (blocking, but only for visible ~50 lines)
                                let count = cache.count_matches(pattern);
                                let owners_str = if owners.is_empty() {
                                    "*unowned*".to_string()
                                } else {
                                    owners
                                        .iter()
                                        .map(|o| format!("`{}`", o))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                };
                                Some(InlayHint {
                                    position: Position {
                                        line: line.line_number,
                                        character: line.pattern_end,
                                    },
                                    label: InlayHintLabel::String(format!(
                                        " ({} {})",
                                        count,
                                        if count == 1 { "file" } else { "files" }
                                    )),
                                    kind: None,
                                    text_edits: None,
                                    tooltip: Some(InlayHintTooltip::MarkupContent(MarkupContent {
                                        kind: MarkupKind::Markdown,
                                        value: format!(
                                            "**Pattern:** `{}`\n\n**Matches:** {} {}\n\n**Owners:** {}",
                                            pattern,
                                            count,
                                            if count == 1 { "file" } else { "files" },
                                            owners_str
                                        ),
                                    })),
                                    padding_left: Some(true),
                                    padding_right: Some(false),
                                    data: None,
                                })
                            } else {
                                None
                            }
                        })
                        .collect();

                    return Ok(Some(hints));
                }
            }
            return Ok(None);
        }

        let (label, tooltip) = match self.get_owners_for_file(uri) {
            Some(owners) => {
                let owners_md = owners
                    .split_whitespace()
                    .map(|o| format!("`{}`", o))
                    .collect::<Vec<_>>()
                    .join(", ");
                (
                    format!("Owned by: {}", owners),
                    MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: format!("**Owners:** {}\n\n*From CODEOWNERS*", owners_md),
                    },
                )
            }
            None => (
                "Owned by nobody".to_string(),
                MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: "**No owner**\n\n*No CODEOWNERS rule matches this file*".to_string(),
                },
            ),
        };

        Ok(Some(vec![InlayHint {
            position: Position {
                line: 0,
                character: 0,
            },
            label: InlayHintLabel::String(label),
            kind: None,
            text_edits: None,
            tooltip: Some(InlayHintTooltip::MarkupContent(tooltip)),
            padding_left: Some(false),
            padding_right: Some(true),
            data: None,
        }]))
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let mut config_changed = false;

        for change in &params.changes {
            if let Ok(path) = change.uri.to_file_path() {
                if let Some(filename) = path.file_name().and_then(|s| s.to_str()) {
                    if filename == CONFIG_FILE || filename == CONFIG_FILE_LOCAL {
                        config_changed = true;
                        break;
                    }
                }
            }
        }

        if config_changed {
            self.reload_config().await;
        }

        self.load_codeowners().await;
        self.refresh_file_cache().await;
        self.refresh_all_open_documents().await;
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        // Reload from TOML files and merge with new JSON settings
        let mut settings = self.load_config_files();
        if let Ok(json_settings) = serde_json::from_value::<Settings>(params.settings) {
            settings.merge(json_settings);
        }
        *self.settings.write().unwrap() = settings;
        self.load_codeowners().await;
        self.refresh_file_cache().await;
        self.refresh_all_open_documents().await;
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        let command = &params.command;
        let args = params.arguments;

        if args.first().and_then(|v| v.as_str()).is_none() {
            return Err(tower_lsp::jsonrpc::Error::invalid_params(
                "Missing URI argument",
            ));
        }

        let pattern = args
            .get(1)
            .and_then(|v| v.as_str())
            .ok_or_else(|| tower_lsp::jsonrpc::Error::invalid_params("Missing pattern argument"))?;

        let owner = args.get(2).and_then(|v| v.as_str());

        let is_custom = command.ends_with(".custom");
        if is_custom && owner.is_none() {
            self.client
                .show_message(
                    MessageType::INFO,
                    "Custom owner feature requires editor support for input dialogs. Please manually edit the CODEOWNERS file.",
                )
                .await;
            return Ok(None);
        }

        let owner = if let Some(o) = owner {
            o.to_string()
        } else {
            let settings = self.settings.read().unwrap();
            if command.contains(".individual") {
                settings.individual.clone().ok_or_else(|| {
                    tower_lsp::jsonrpc::Error::invalid_params("No individual owner configured")
                })?
            } else if command.contains(".team") {
                settings.team.clone().ok_or_else(|| {
                    tower_lsp::jsonrpc::Error::invalid_params("No team owner configured")
                })?
            } else {
                return Err(tower_lsp::jsonrpc::Error::invalid_params(
                    "Unknown command type",
                ));
            }
        };

        let result = if command.starts_with("codeowners.addToExisting") {
            self.add_to_existing(pattern, &owner)
        } else if command.starts_with("codeowners.takeOwnership") {
            self.add_ownership(pattern, &owner)
        } else {
            Err(format!("Unknown command: {}", command))
        };

        match result {
            Ok(()) => {
                self.load_codeowners().await;
                self.refresh_file_cache().await;
                self.publish_codeowners_diagnostics().await;
                self.client
                    .show_message(
                        MessageType::INFO,
                        format!("Added {} as owner of {}", owner, pattern),
                    )
                    .await;
                Ok(None)
            }
            Err(e) => {
                self.client.show_message(MessageType::ERROR, &e).await;
                Err(tower_lsp::jsonrpc::Error::internal_error())
            }
        }
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;

        if !self.is_codeowners_file(uri) {
            return Ok(None);
        }

        // Refresh file cache to pick up new/renamed/deleted files
        self.refresh_file_cache().await;

        let position = params.text_document_position.position;

        // Use buffer content if available (for unsaved changes), else read from disk
        let content = {
            let docs = self.open_documents.read().unwrap();
            if let Some(text) = docs.get(uri) {
                text.clone()
            } else {
                drop(docs);
                let codeowners_path = self.codeowners_path.read().unwrap();
                match codeowners_path.as_ref() {
                    Some(p) => fs::read_to_string(p).unwrap_or_default(),
                    None => return Ok(None),
                }
            }
        };

        let lines: Vec<&str> = content.lines().collect();
        let line = match lines.get(position.line as usize) {
            Some(l) => *l,
            None => return Ok(None),
        };

        let col = position.character as usize;
        let text_before_cursor = if col <= line.len() {
            &line[..col]
        } else {
            line
        };

        let last_space = text_before_cursor.rfind(' ').map(|i| i + 1).unwrap_or(0);
        let current_word = &text_before_cursor[last_space..];

        // Range to replace (from start of current word to cursor)
        let replace_range = Range {
            start: Position {
                line: position.line,
                character: last_space as u32,
            },
            end: Position {
                line: position.line,
                character: col as u32,
            },
        };

        let mut items = Vec::new();

        // Helper to create completion item with text_edit
        let make_completion =
            |label: String, kind: CompletionItemKind, detail: String| -> CompletionItem {
                CompletionItem {
                    label: label.clone(),
                    kind: Some(kind),
                    detail: Some(detail),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                        range: replace_range,
                        new_text: label,
                    })),
                    ..Default::default()
                }
            };

        // Helper to create snippet completion with tabstops
        let make_snippet = |label: String,
                            insert_text: String,
                            kind: CompletionItemKind,
                            detail: String|
         -> CompletionItem {
            CompletionItem {
                label,
                kind: Some(kind),
                detail: Some(detail),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: replace_range,
                    new_text: insert_text,
                })),
                ..Default::default()
            }
        };

        // Path completions - trigger on start of line or when typing a path
        // Valid patterns: /src/, src/, ./src/, *.rs, etc.
        let is_path_context = last_space == 0
            || current_word.starts_with('/')
            || current_word.starts_with('.')
            || current_word.contains('/');

        if is_path_context {
            let file_cache = self.file_cache.read().unwrap();
            if let Some(ref cache) = *file_cache {
                // Use the current word as prefix for completion
                let prefix = current_word;
                let paths = cache.complete_path(prefix);
                for path in paths {
                    let is_dir = path.ends_with('/');
                    if is_dir {
                        // Plain directory completion
                        items.push(make_completion(
                            path.clone(),
                            CompletionItemKind::FOLDER,
                            "Directory".to_string(),
                        ));
                        // Snippet: dir/** with owner placeholder
                        items.push(make_snippet(
                            format!("{}** @...", &path),
                            format!("{}** ${{1:@owner}}", &path),
                            CompletionItemKind::SNIPPET,
                            "Directory rule with owner".to_string(),
                        ));
                    } else {
                        // File completion with owner placeholder snippet
                        items.push(make_completion(
                            path.clone(),
                            CompletionItemKind::FILE,
                            "File".to_string(),
                        ));
                        items.push(make_snippet(
                            format!("{} @...", &path),
                            format!("{} ${{1:@owner}}", &path),
                            CompletionItemKind::SNIPPET,
                            "File rule with owner".to_string(),
                        ));
                    }
                }

                // Glob pattern snippets
                if prefix.is_empty() || "*".starts_with(prefix) {
                    items.push(make_snippet(
                        "* @...".to_string(),
                        "* ${1:@owner}".to_string(),
                        CompletionItemKind::SNIPPET,
                        "Catch-all rule".to_string(),
                    ));
                    items.push(make_snippet(
                        "*.rs @...".to_string(),
                        "*.rs ${1:@owner}".to_string(),
                        CompletionItemKind::SNIPPET,
                        "Rust files rule".to_string(),
                    ));
                    items.push(make_snippet(
                        "*.ts @...".to_string(),
                        "*.ts ${1:@owner}".to_string(),
                        CompletionItemKind::SNIPPET,
                        "TypeScript files rule".to_string(),
                    ));
                    items.push(make_snippet(
                        "*.js @...".to_string(),
                        "*.js ${1:@owner}".to_string(),
                        CompletionItemKind::SNIPPET,
                        "JavaScript files rule".to_string(),
                    ));
                }
            }
        }

        // Owner completions
        if current_word.starts_with('@') {
            let (individual, team) = {
                let settings = self.settings.read().unwrap();
                (settings.individual.clone(), settings.team.clone())
            };

            if let Some(ref ind) = individual {
                if ind.starts_with(current_word) {
                    items.push(make_completion(
                        ind.clone(),
                        CompletionItemKind::CONSTANT,
                        "Configured individual".to_string(),
                    ));
                }
            }

            if let Some(ref t) = team {
                if t.starts_with(current_word) {
                    items.push(make_completion(
                        t.clone(),
                        CompletionItemKind::CLASS,
                        "Configured team".to_string(),
                    ));
                }
            }

            // Add validated owners from GitHub cache
            let mut seen_owners: HashSet<String> = HashSet::new();
            for owner in self.github_client.get_cached_owners() {
                if owner.starts_with(current_word) && seen_owners.insert(owner.clone()) {
                    items.push(make_completion(
                        owner.clone(),
                        if owner.contains('/') {
                            CompletionItemKind::CLASS
                        } else {
                            CompletionItemKind::CONSTANT
                        },
                        "Validated on GitHub".to_string(),
                    ));
                }
            }

            // Add owners from current file
            let parsed = parse_codeowners_file_with_positions(&content);
            for line in &parsed {
                if let CodeownersLine::Rule { owners, .. } = &line.content {
                    for owner in owners {
                        if owner.starts_with(current_word) && seen_owners.insert(owner.clone()) {
                            items.push(make_completion(
                                owner.clone(),
                                if owner.contains('/') {
                                    CompletionItemKind::CLASS
                                } else {
                                    CompletionItemKind::CONSTANT
                                },
                                "Used in this file".to_string(),
                            ));
                        }
                    }
                }
            }
        }

        if items.is_empty() {
            Ok(None)
        } else {
            Ok(Some(CompletionResponse::Array(items)))
        }
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document.uri;

        if !self.is_codeowners_file(uri) {
            return Ok(None);
        }

        let Some(content) = self.get_codeowners_content() else {
            return Ok(None);
        };

        let formatted = format_codeowners(&content);

        if formatted == content {
            return Ok(None);
        }

        let line_count = content.lines().count();
        let last_line_len = content.lines().last().map(|l| l.len()).unwrap_or(0);

        Ok(Some(vec![TextEdit {
            range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: line_count as u32,
                    character: last_line_len as u32,
                },
            },
            new_text: formatted,
        }]))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        if !self.is_codeowners_file(&params.text_document.uri) {
            return Ok(None);
        }
        let Some(content) = self.get_codeowners_content() else {
            return Ok(None);
        };
        let symbols = handlers::symbols::document_symbols(&content);
        if symbols.is_empty() {
            Ok(None)
        } else {
            Ok(Some(DocumentSymbolResponse::Nested(symbols)))
        }
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        if !self.is_codeowners_file(&params.text_document.uri) {
            return Ok(None);
        }
        let Some(content) = self.get_codeowners_content() else {
            return Ok(None);
        };
        let ranges = handlers::semantic::folding_ranges(&content);
        if ranges.is_empty() {
            Ok(None)
        } else {
            Ok(Some(ranges))
        }
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        if !self.is_codeowners_file(&params.text_document.uri) {
            return Ok(None);
        }
        let Some(content) = self.get_codeowners_content() else {
            return Ok(None);
        };
        let data = handlers::semantic::semantic_tokens(&content);
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        })))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = &params.text_document_position.text_document.uri;
        if !self.is_codeowners_file(uri) {
            return Ok(None);
        }
        let Some(content) = self.get_codeowners_content() else {
            return Ok(None);
        };
        let codeowners_path = self.codeowners_path.read().unwrap();
        let Some(path) = codeowners_path.as_ref() else {
            return Ok(None);
        };
        let Ok(codeowners_uri) = Url::from_file_path(path) else {
            return Ok(None);
        };
        Ok(handlers::navigation::find_references(
            &content,
            params.text_document_position.position,
            &codeowners_uri,
        ))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        if !self.is_codeowners_file(&params.text_document.uri) {
            return Ok(None);
        }
        let Some(content) = self.get_codeowners_content() else {
            return Ok(None);
        };
        Ok(
            handlers::navigation::prepare_rename(&content, params.position)
                .map(PrepareRenameResponse::Range),
        )
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = &params.text_document_position.text_document.uri;
        if !self.is_codeowners_file(uri) {
            return Ok(None);
        }
        let Some(content) = self.get_codeowners_content() else {
            return Ok(None);
        };
        let codeowners_path = self.codeowners_path.read().unwrap();
        let Some(path) = codeowners_path.as_ref() else {
            return Ok(None);
        };
        let Ok(codeowners_uri) = Url::from_file_path(path) else {
            return Ok(None);
        };
        Ok(handlers::navigation::rename_owner(
            &content,
            params.text_document_position.position,
            &params.new_name,
            &codeowners_uri,
        ))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let codeowners_path = self.codeowners_path.read().unwrap();
        let Some(path) = codeowners_path.as_ref() else {
            return Ok(None);
        };
        let Ok(codeowners_uri) = Url::from_file_path(path) else {
            return Ok(None);
        };
        let Some(content) = self.get_codeowners_content() else {
            return Ok(None);
        };
        let symbols =
            handlers::symbols::workspace_symbols(&content, &params.query, &codeowners_uri);
        if symbols.is_empty() {
            Ok(None)
        } else {
            Ok(Some(symbols))
        }
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        if !self.is_codeowners_file(&params.text_document.uri) {
            return Ok(None);
        }
        let Some(content) = self.get_codeowners_content() else {
            return Ok(None);
        };
        let file_cache = self.file_cache.read().unwrap();
        let Some(ref cache) = *file_cache else {
            return Ok(None);
        };
        let lenses = handlers::lens::code_lenses(&content, cache);
        if lenses.is_empty() {
            Ok(None)
        } else {
            Ok(Some(lenses))
        }
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        if !self.is_codeowners_file(&params.text_document_position_params.text_document.uri) {
            return Ok(None);
        }
        let Some(content) = self.get_codeowners_content() else {
            return Ok(None);
        };
        let position = params.text_document_position_params.position;
        let lines: Vec<&str> = content.lines().collect();
        let line = lines.get(position.line as usize).copied().unwrap_or("");
        Ok(handlers::signature::signature_help(
            line,
            position.character as usize,
        ))
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        if !self.is_codeowners_file(&params.text_document.uri) {
            return Ok(None);
        }
        let Some(content) = self.get_codeowners_content() else {
            return Ok(None);
        };
        let ranges = handlers::selection::selection_ranges(&content, &params.positions);
        if ranges.is_empty() {
            Ok(None)
        } else {
            Ok(Some(ranges))
        }
    }

    async fn linked_editing_range(
        &self,
        params: LinkedEditingRangeParams,
    ) -> Result<Option<LinkedEditingRanges>> {
        let uri = &params.text_document_position_params.text_document.uri;
        if !self.is_codeowners_file(uri) {
            return Ok(None);
        }
        let Some(content) = self.get_codeowners_content() else {
            return Ok(None);
        };
        Ok(handlers::linked::linked_editing_ranges(
            &content,
            params.text_document_position_params.position,
        ))
    }
}

/// Find which line numbers changed between two versions of content
fn find_changed_lines(old: &str, new: &str) -> Vec<usize> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut changed = Vec::new();

    // Compare line by line
    let max_len = old_lines.len().max(new_lines.len());
    for i in 0..max_len {
        let old_line = old_lines.get(i).copied();
        let new_line = new_lines.get(i).copied();
        if old_line != new_line {
            changed.push(i);
        }
    }

    // Limit to avoid checking too many lines on large pastes
    if changed.len() > 10 {
        changed.truncate(10);
    }

    changed
}

/// Check patterns on specific lines and return pattern-no-match diagnostics
fn check_patterns_for_lines(
    content: &str,
    line_numbers: &[usize],
    file_cache: &file_cache::FileCache,
    config: &DiagnosticConfig,
) -> Vec<Diagnostic> {
    use parser::CodeownersLine;

    let mut diagnostics = Vec::new();
    let parsed = parse_codeowners_file_with_positions(content);

    for &line_num in line_numbers {
        let line_num_u32 = line_num as u32;
        // Find the parsed line for this line number
        if let Some(parsed_line) = parsed.iter().find(|p| p.line_number == line_num_u32) {
            if let CodeownersLine::Rule { ref pattern, .. } = parsed_line.content {
                // Check if pattern matches any files
                if !file_cache.has_matches(pattern) {
                    if let Some(severity) =
                        config.get("pattern-no-match", DiagnosticSeverity::WARNING)
                    {
                        diagnostics.push(Diagnostic {
                            range: Range {
                                start: Position {
                                    line: line_num_u32,
                                    character: parsed_line.pattern_start,
                                },
                                end: Position {
                                    line: line_num_u32,
                                    character: parsed_line.pattern_end,
                                },
                            },
                            severity: Some(severity),
                            code: Some(NumberOrString::String("pattern-no-match".to_string())),
                            source: Some("codeowners".to_string()),
                            message: format!("Pattern '{}' doesn't match any files", pattern),
                            ..Default::default()
                        });
                    }
                }
            }
        }
    }

    diagnostics
}

/// Format rich hover content for an owner in CODEOWNERS file
fn format_owner_hover(owner: &str, info: Option<&github::OwnerInfo>) -> String {
    match info {
        Some(github::OwnerInfo::User(user)) => {
            let mut lines = Vec::new();

            // Avatar + header (some editors render images in hover, invisible if not)
            if let Some(ref avatar) = user.avatar_url {
                lines.push(format!(
                    "![]({}&s=64) [`{}`]({})",
                    avatar, owner, user.html_url
                ));
            } else {
                lines.push(format!("[`{}`]({})", owner, user.html_url));
            }

            if let Some(ref name) = user.name {
                lines.push(format!("**{}**", name));
            }

            if let Some(ref company) = user.company {
                lines.push(format!("📍 {}", company));
            }

            if let Some(ref bio) = user.bio {
                lines.push(String::new());
                lines.push(format!("*{}*", bio));
            }

            lines.join("\n")
        }
        Some(github::OwnerInfo::Team(team)) => {
            let mut lines = vec![format!("## [`{}`]({})", owner, team.html_url)];

            if team.name != team.slug {
                lines.push(format!("**{}**", team.name));
            }

            let mut stats = Vec::new();
            if let Some(members) = team.members_count {
                stats.push(format!("👥 {} members", members));
            }
            if let Some(repos) = team.repos_count {
                stats.push(format!("📁 {} repos", repos));
            }
            if !stats.is_empty() {
                lines.push(stats.join(" · "));
            }

            if let Some(ref desc) = team.description {
                if !desc.is_empty() {
                    lines.push(String::new());
                    lines.push(format!("*{}*", desc));
                }
            }

            lines.join("\n")
        }
        Some(github::OwnerInfo::Invalid) => {
            format!("## {}\n\n⚠️ **Owner not found on GitHub**", owner)
        }
        Some(github::OwnerInfo::Unknown) | None => {
            // Build basic link even without cached info
            let url = if let Some(username) = owner.strip_prefix('@') {
                if username.contains('/') {
                    let parts: Vec<&str> = username.splitn(2, '/').collect();
                    if parts.len() == 2 {
                        format!("https://github.com/orgs/{}/teams/{}", parts[0], parts[1])
                    } else {
                        format!("https://github.com/{}", username)
                    }
                } else {
                    format!("https://github.com/{}", username)
                }
            } else {
                String::new()
            };

            if url.is_empty() {
                format!("## {}", owner)
            } else {
                format!("## [`{}`]({})\n\n*Not validated yet*", owner, url)
            }
        }
    }
}

/// Format an owner as a clickable GitHub link (LSP-specific, uses markdown)
fn format_owner_link(owner: &str) -> String {
    if let Some(user) = owner.strip_prefix('@') {
        if user.contains('/') {
            // Team: @org/team -> https://github.com/orgs/org/teams/team
            let parts: Vec<&str> = user.splitn(2, '/').collect();
            if parts.len() == 2 {
                let url = format!("https://github.com/orgs/{}/teams/{}", parts[0], parts[1]);
                return format!("[`{}`]({})", owner, url);
            }
        } else {
            // User: @user -> https://github.com/user
            let url = format!("https://github.com/{}", user);
            return format!("[`{}`]({})", owner, url);
        }
    } else if owner.contains('@') {
        // Email - no link
        return format!("`{}`", owner);
    }
    format!("`{}`", owner)
}

/// Format an owner with rich metadata if available
fn format_owner_with_info(owner: &str, info: Option<&github::OwnerInfo>) -> String {
    match info {
        Some(github::OwnerInfo::User(user)) => {
            let mut parts = vec![format!("[`{}`]({})", owner, user.html_url)];

            // Add display name if different from login
            if let Some(ref name) = user.name {
                if name != &user.login {
                    parts.push(format!("*{}*", name));
                }
            }

            // Add company
            if let Some(ref company) = user.company {
                parts.push(format!("📍 {}", company));
            }

            // Add bio (truncated)
            if let Some(ref bio) = user.bio {
                let truncated = if bio.len() > 60 {
                    format!("{}...", &bio[..60])
                } else {
                    bio.clone()
                };
                parts.push(format!("*\"{}\"*", truncated));
            }

            parts.join(" — ")
        }
        Some(github::OwnerInfo::Team(team)) => {
            let mut parts = vec![format!("[`{}`]({})", owner, team.html_url)];

            // Add team display name if different from slug
            if team.name != team.slug {
                parts.push(format!("*{}*", team.name));
            }

            // Add member/repo count if available
            let mut stats = Vec::new();
            if let Some(members) = team.members_count {
                stats.push(format!("{} members", members));
            }
            if let Some(repos) = team.repos_count {
                stats.push(format!("{} repos", repos));
            }
            if !stats.is_empty() {
                parts.push(format!("({})", stats.join(", ")));
            }

            // Add description (truncated)
            if let Some(ref desc) = team.description {
                if !desc.is_empty() {
                    let truncated = if desc.len() > 60 {
                        format!("{}...", &desc[..60])
                    } else {
                        desc.clone()
                    };
                    parts.push(format!("*\"{}\"*", truncated));
                }
            }

            parts.join(" — ")
        }
        _ => format_owner_link(owner),
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
