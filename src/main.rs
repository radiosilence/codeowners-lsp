mod diagnostics;
mod file_cache;
mod github;
mod parser;
mod pattern;
mod validation;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;

use codeowners::Owners;
use serde::Deserialize;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use diagnostics::compute_diagnostics_sync;
use file_cache::FileCache;
use github::GitHubClient;
use parser::{
    find_insertion_point, parse_codeowners_file, parse_codeowners_file_with_positions,
    serialize_codeowners, CodeownersLine,
};
use pattern::pattern_matches;

#[derive(Debug, Default, Deserialize)]
struct Settings {
    /// Custom path to CODEOWNERS file (relative to workspace root)
    path: Option<String>,
    /// Individual owner identifier (e.g. @username)
    individual: Option<String>,
    /// Team owner identifier (e.g. @org/team-name)
    team: Option<String>,
    /// GitHub token for validating owners (reads from env if prefixed with "env:")
    github_token: Option<String>,
    /// Whether to validate owners against GitHub API
    #[serde(default)]
    validate_owners: bool,
}

struct Backend {
    client: Client,
    workspace_root: RwLock<Option<PathBuf>>,
    codeowners: RwLock<Option<Owners>>,
    codeowners_path: RwLock<Option<PathBuf>>,
    settings: RwLock<Settings>,
    file_cache: RwLock<Option<FileCache>>,
    github_client: GitHubClient,
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
            github_client: GitHubClient::new(),
        }
    }

    /// Get the GitHub token from settings (resolving env: prefix)
    fn get_github_token(&self) -> Option<String> {
        let settings = self.settings.read().unwrap();
        settings.github_token.as_ref().and_then(|token| {
            if let Some(env_var) = token.strip_prefix("env:") {
                std::env::var(env_var).ok()
            } else {
                Some(token.clone())
            }
        })
    }

    fn load_codeowners(&self) -> Option<PathBuf> {
        let root = self.workspace_root.read().unwrap();
        let root = root.as_ref()?;
        let settings = self.settings.read().unwrap();

        // If custom path is set, use it
        if let Some(custom_path) = &settings.path {
            let path = root.join(custom_path);
            if path.exists() {
                let owners = codeowners::from_path(&path);
                *self.codeowners.write().unwrap() = Some(owners);
                *self.codeowners_path.write().unwrap() = Some(path.clone());
                return Some(path);
            }
        }

        // Otherwise use the crate's locate function
        if let Some(path) = codeowners::locate(root) {
            let owners = codeowners::from_path(&path);
            *self.codeowners.write().unwrap() = Some(owners);
            *self.codeowners_path.write().unwrap() = Some(path.clone());
            return Some(path);
        }

        *self.codeowners.write().unwrap() = None;
        *self.codeowners_path.write().unwrap() = None;
        None
    }

    fn refresh_file_cache(&self) {
        let root = self.workspace_root.read().unwrap();
        if let Some(root) = root.as_ref() {
            *self.file_cache.write().unwrap() = Some(FileCache::new(root));
        }
    }

    fn get_owners_for_file(&self, uri: &Url) -> Option<String> {
        let root = self.workspace_root.read().unwrap();
        let root = root.as_ref()?;

        let file_path = uri.to_file_path().ok()?;
        let relative_path = file_path.strip_prefix(root).ok()?;

        let codeowners = self.codeowners.read().unwrap();
        let codeowners = codeowners.as_ref()?;

        let owners = codeowners.of(relative_path)?;
        let owner_strs: Vec<String> = owners.iter().map(|o| o.to_string()).collect();

        if owner_strs.is_empty() {
            None
        } else {
            Some(owner_strs.join(" "))
        }
    }

    /// Compute diagnostics for the CODEOWNERS file
    async fn compute_diagnostics(&self, content: &str) -> Vec<Diagnostic> {
        // Check if GitHub validation is enabled
        let (validate_owners, token) = {
            let settings = self.settings.read().unwrap();
            let enabled = settings.validate_owners;
            let token = self.get_github_token();
            (enabled && token.is_some(), token)
        };

        // Phase 1: Synchronous diagnostics (holds file_cache lock)
        let (mut diagnostics, owners_to_validate) = {
            let file_cache = self.file_cache.read().unwrap();
            compute_diagnostics_sync(content, file_cache.as_ref())
        };

        // Phase 2: Async GitHub validation (no locks held)
        if validate_owners {
            if let Some(token) = token {
                diagnostics::add_github_diagnostics(
                    &mut diagnostics,
                    owners_to_validate,
                    &self.github_client,
                    &token,
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

    /// Generate code actions for CODEOWNERS file diagnostics
    async fn codeowners_code_actions(
        &self,
        params: &CodeActionParams,
    ) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        let codeowners_path = self.codeowners_path.read().unwrap().clone();
        let path = match codeowners_path {
            Some(p) => p,
            None => return Ok(None),
        };

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Ok(None),
        };

        let lines: Vec<&str> = content.lines().collect();
        let mut actions = Vec::new();

        // Check each diagnostic in the request
        for diagnostic in &params.context.diagnostics {
            let line_num = diagnostic.range.start.line as usize;

            // Handle "shadowed rule" diagnostics - offer to remove the dead rule
            if diagnostic.message.contains("shadowed by") && line_num < lines.len() {
                // Create edit to delete this line
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

                // Offer to add catch-all rule with configured owners
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
        let insertion_point = find_insertion_point(&lines, pattern);

        lines.insert(
            insertion_point,
            CodeownersLine::Rule {
                pattern: pattern.to_string(),
                owners: vec![owner.to_string()],
            },
        );

        let new_content = serialize_codeowners(&lines);
        // Ensure file ends with newline
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
        let lines = parse_codeowners_file_with_positions(&content);

        // Iterate in reverse since last match wins
        for parsed_line in lines.iter().rev() {
            if let CodeownersLine::Rule { pattern, .. } = &parsed_line.content {
                if pattern_matches(pattern, file_path) {
                    return Some((parsed_line.line_number, pattern.clone()));
                }
            }
        }

        None
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

        if let Some(opts) = &params.initialization_options {
            if let Ok(settings) = serde_json::from_value::<Settings>(opts.clone()) {
                *self.settings.write().unwrap() = settings;
            }
        }

        self.load_codeowners();
        self.refresh_file_cache();

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
                definition_provider: Some(OneOf::Left(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["/".to_string(), "@".to_string()]),
                    ..Default::default()
                }),
                document_formatting_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "codeowners-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        // Publish initial diagnostics for CODEOWNERS file
        self.publish_codeowners_diagnostics().await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = &params.text_document.uri;
        if self.is_codeowners_file(uri) {
            let diagnostics = self.compute_diagnostics(&params.text_document.text).await;
            self.client
                .publish_diagnostics(uri.clone(), diagnostics, None)
                .await;
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = &params.text_document.uri;
        if self.is_codeowners_file(uri) {
            // Get the full text from the change events (we requested FULL sync)
            if let Some(change) = params.content_changes.first() {
                let diagnostics = self.compute_diagnostics(&change.text).await;
                self.client
                    .publish_diagnostics(uri.clone(), diagnostics, None)
                    .await;
            }
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = &params.text_document.uri;
        if self.is_codeowners_file(uri) {
            // Reload codeowners and refresh cache
            self.load_codeowners();
            self.refresh_file_cache();

            // Publish diagnostics
            if let Some(text) = params.text {
                let diagnostics = self.compute_diagnostics(&text).await;
                self.client
                    .publish_diagnostics(uri.clone(), diagnostics, None)
                    .await;
            } else {
                self.publish_codeowners_diagnostics().await;
            }
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let formatted = match self.get_owners_for_file(uri) {
            None => "**Owned by nobody**".to_string(),
            Some(owners) => {
                let owner_list: Vec<&str> = owners.split_whitespace().collect();
                if owner_list.len() == 1 {
                    format!("**Owner:** {}", format_owner_link(owner_list[0]))
                } else {
                    let list = owner_list
                        .iter()
                        .map(|o| format!("- {}", format_owner_link(o)))
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("**Owners:**\n{}", list)
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

        // Don't navigate from CODEOWNERS file itself
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

        // Find the matching rule
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

        // Handle CODEOWNERS file code actions (fix diagnostics)
        if self.is_codeowners_file(uri) {
            return self.codeowners_code_actions(&params).await;
        }

        // Handle regular file code actions (take ownership)
        let file_path = match uri.to_file_path() {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };

        let root = self.workspace_root.read().unwrap();
        let root = match root.as_ref() {
            Some(r) => r.clone(),
            None => return Ok(None),
        };
        drop(root);

        let relative_path =
            match file_path.strip_prefix(self.workspace_root.read().unwrap().as_ref().unwrap()) {
                Ok(p) => p.to_string_lossy().to_string(),
                Err(_) => return Ok(None),
            };

        let has_existing_owners = self.get_owners_for_file(uri).is_some();
        let settings = self.settings.read().unwrap();

        let mut actions = Vec::new();

        // Helper to create a code action
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
            // Offer to add to existing or create more specific entry
            if let Some(ref individual) = settings.individual {
                actions.push(make_action(
                    format!("Add {} to existing CODEOWNERS entry", individual),
                    "codeowners.addToExisting.individual",
                    &file_pattern,
                    Some(individual),
                ));
                actions.push(make_action(
                    format!("Take ownership as {} (new specific entry)", individual),
                    "codeowners.takeOwnership.individual",
                    &file_pattern,
                    Some(individual),
                ));
            }
            if let Some(ref team) = settings.team {
                actions.push(make_action(
                    format!("Add {} to existing CODEOWNERS entry", team),
                    "codeowners.addToExisting.team",
                    &file_pattern,
                    Some(team),
                ));
                actions.push(make_action(
                    format!("Take ownership as {} (new specific entry)", team),
                    "codeowners.takeOwnership.team",
                    &file_pattern,
                    Some(team),
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
            // No existing owners - offer to take ownership
            if let Some(ref individual) = settings.individual {
                actions.push(make_action(
                    format!("Take ownership as {}", individual),
                    "codeowners.takeOwnership.individual",
                    &file_pattern,
                    Some(individual),
                ));
            }
            if let Some(ref team) = settings.team {
                actions.push(make_action(
                    format!("Take ownership as {}", team),
                    "codeowners.takeOwnership.team",
                    &file_pattern,
                    Some(team),
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

        // For CODEOWNERS file, show file match counts
        if self.is_codeowners_file(uri) {
            let codeowners_path = self.codeowners_path.read().unwrap();
            if let Some(path) = codeowners_path.as_ref() {
                if let Ok(content) = fs::read_to_string(path) {
                    let lines = parse_codeowners_file_with_positions(&content);
                    let file_cache = self.file_cache.read().unwrap();

                    if let Some(ref cache) = *file_cache {
                        let hints: Vec<InlayHint> = lines
                            .iter()
                            .filter_map(|line| {
                                if let CodeownersLine::Rule { pattern, .. } = &line.content {
                                    let count = cache.count_matches(pattern);
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
                                        tooltip: Some(InlayHintTooltip::String(format!(
                                            "This pattern matches {} {} in the repository",
                                            count,
                                            if count == 1 { "file" } else { "files" }
                                        ))),
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
            }
            return Ok(None);
        }

        // For other files, show ownership info
        let (label, tooltip) = match self.get_owners_for_file(uri) {
            Some(owners) => (
                format!("Owned by: {}", owners),
                "File ownership from CODEOWNERS".to_string(),
            ),
            None => (
                "Owned by nobody".to_string(),
                "No CODEOWNERS rule matches this file".to_string(),
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
            tooltip: Some(InlayHintTooltip::String(tooltip)),
            padding_left: Some(false),
            padding_right: Some(true),
            data: None,
        }]))
    }

    async fn did_change_watched_files(&self, _params: DidChangeWatchedFilesParams) {
        self.load_codeowners();
        self.refresh_file_cache();
        self.publish_codeowners_diagnostics().await;
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        if let Ok(settings) = serde_json::from_value::<Settings>(params.settings) {
            *self.settings.write().unwrap() = settings;
            self.load_codeowners();
            self.refresh_file_cache();
        }
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        let command = &params.command;
        let args = params.arguments;

        // Parse arguments: [uri, pattern, owner?]
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

        // For custom commands, we need the owner from the client
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

        // Get owner from settings if not provided (for individual/team commands)
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
                // Reload codeowners after modification
                self.load_codeowners();
                self.refresh_file_cache();
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

        // Only provide completions in CODEOWNERS file
        if !self.is_codeowners_file(uri) {
            return Ok(None);
        }

        let position = params.text_document_position.position;

        // Get current line text
        let codeowners_path = self.codeowners_path.read().unwrap();
        let path = match codeowners_path.as_ref() {
            Some(p) => p.clone(),
            None => return Ok(None),
        };
        drop(codeowners_path);

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Ok(None),
        };

        let lines: Vec<&str> = content.lines().collect();
        let line = match lines.get(position.line as usize) {
            Some(l) => *l,
            None => return Ok(None),
        };

        // Get text up to cursor
        let col = position.character as usize;
        let text_before_cursor = if col <= line.len() {
            &line[..col]
        } else {
            line
        };

        // Find current word being typed
        let last_space = text_before_cursor.rfind(' ').map(|i| i + 1).unwrap_or(0);
        let current_word = &text_before_cursor[last_space..];

        let mut items = Vec::new();

        // Path completions (starts with / or is first word)
        if current_word.starts_with('/') || last_space == 0 {
            let file_cache = self.file_cache.read().unwrap();
            if let Some(ref cache) = *file_cache {
                let prefix = if current_word.starts_with('/') {
                    current_word
                } else {
                    ""
                };
                let paths = cache.complete_path(prefix);
                for path in paths {
                    let is_dir = path.ends_with('/');
                    items.push(CompletionItem {
                        label: path.clone(),
                        kind: Some(if is_dir {
                            CompletionItemKind::FOLDER
                        } else {
                            CompletionItemKind::FILE
                        }),
                        detail: Some(if is_dir {
                            "Directory".to_string()
                        } else {
                            "File".to_string()
                        }),
                        ..Default::default()
                    });
                }

                // Also suggest common patterns
                if prefix.is_empty() || "*".starts_with(prefix) {
                    items.push(CompletionItem {
                        label: "*".to_string(),
                        kind: Some(CompletionItemKind::SNIPPET),
                        detail: Some("Match all files".to_string()),
                        ..Default::default()
                    });
                    items.push(CompletionItem {
                        label: "*.rs".to_string(),
                        kind: Some(CompletionItemKind::SNIPPET),
                        detail: Some("Match Rust files".to_string()),
                        ..Default::default()
                    });
                    items.push(CompletionItem {
                        label: "*.ts".to_string(),
                        kind: Some(CompletionItemKind::SNIPPET),
                        detail: Some("Match TypeScript files".to_string()),
                        ..Default::default()
                    });
                    items.push(CompletionItem {
                        label: "*.js".to_string(),
                        kind: Some(CompletionItemKind::SNIPPET),
                        detail: Some("Match JavaScript files".to_string()),
                        ..Default::default()
                    });
                }
            }
        }

        // Owner completions (starts with @)
        if current_word.starts_with('@') {
            let settings = self.settings.read().unwrap();

            // Suggest configured individual
            if let Some(ref individual) = settings.individual {
                if individual.starts_with(current_word) {
                    items.push(CompletionItem {
                        label: individual.clone(),
                        kind: Some(CompletionItemKind::CONSTANT),
                        detail: Some("Configured individual".to_string()),
                        ..Default::default()
                    });
                }
            }

            // Suggest configured team
            if let Some(ref team) = settings.team {
                if team.starts_with(current_word) {
                    items.push(CompletionItem {
                        label: team.clone(),
                        kind: Some(CompletionItemKind::CLASS),
                        detail: Some("Configured team".to_string()),
                        ..Default::default()
                    });
                }
            }

            // Collect existing owners from the file for suggestions
            let parsed = parse_codeowners_file_with_positions(&content);
            let mut seen_owners: std::collections::HashSet<String> =
                std::collections::HashSet::new();

            for line in &parsed {
                if let CodeownersLine::Rule { owners, .. } = &line.content {
                    for owner in owners {
                        if owner.starts_with(current_word) && seen_owners.insert(owner.clone()) {
                            items.push(CompletionItem {
                                label: owner.clone(),
                                kind: Some(if owner.contains('/') {
                                    CompletionItemKind::CLASS // Team
                                } else {
                                    CompletionItemKind::CONSTANT // User
                                }),
                                detail: Some("Used in this file".to_string()),
                                ..Default::default()
                            });
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

        // Only format CODEOWNERS files
        if !self.is_codeowners_file(uri) {
            return Ok(None);
        }

        let codeowners_path = self.codeowners_path.read().unwrap();
        let path = match codeowners_path.as_ref() {
            Some(p) => p.clone(),
            None => return Ok(None),
        };
        drop(codeowners_path);

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Ok(None),
        };

        let formatted = format_codeowners(&content);

        if formatted == content {
            return Ok(None); // No changes needed
        }

        // Return a single edit that replaces the entire document
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
}

/// Format an owner as a clickable GitHub link
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
        // Email - no link, just format
        return format!("`{}`", owner);
    }
    // Fallback
    format!("`{}`", owner)
}

/// Format CODEOWNERS content (same logic as CLI)
fn format_codeowners(content: &str) -> String {
    let mut result = Vec::new();
    let mut prev_was_empty = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Preserve blank lines but collapse multiple
        if trimmed.is_empty() {
            if !prev_was_empty && !result.is_empty() {
                result.push(String::new());
            }
            prev_was_empty = true;
            continue;
        }
        prev_was_empty = false;

        // Comments: normalize to single space after #
        if let Some(comment_text) = trimmed.strip_prefix('#') {
            let comment_text = comment_text.trim();
            if comment_text.is_empty() {
                result.push("#".to_string());
            } else {
                result.push(format!("# {}", comment_text));
            }
            continue;
        }

        // Rules: normalize spacing between pattern and owners
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        let pattern = parts[0];
        let owners = &parts[1..];

        if owners.is_empty() {
            result.push(pattern.to_string());
        } else {
            result.push(format!("{} {}", pattern, owners.join(" ")));
        }
    }

    // Ensure trailing newline
    let mut output = result.join("\n");
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
