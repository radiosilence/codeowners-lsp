use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;

use codeowners::Owners;
use serde::Deserialize;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

/// Represents a parsed line from a CODEOWNERS file
#[derive(Debug, Clone)]
enum CodeownersLine {
    /// A comment line (starts with #)
    Comment(String),
    /// An empty line
    Empty,
    /// A rule with pattern and owners
    Rule {
        pattern: String,
        owners: Vec<String>,
    },
}

impl std::fmt::Display for CodeownersLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodeownersLine::Comment(c) => write!(f, "{}", c),
            CodeownersLine::Empty => Ok(()),
            CodeownersLine::Rule { pattern, owners } => {
                write!(f, "{} {}", pattern, owners.join(" "))
            }
        }
    }
}

/// Parse a CODEOWNERS file into structured lines
fn parse_codeowners_file(content: &str) -> Vec<CodeownersLine> {
    content
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                CodeownersLine::Empty
            } else if trimmed.starts_with('#') {
                CodeownersLine::Comment(line.to_string())
            } else {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.is_empty() {
                    CodeownersLine::Empty
                } else {
                    CodeownersLine::Rule {
                        pattern: parts[0].to_string(),
                        owners: parts[1..].iter().map(|s| s.to_string()).collect(),
                    }
                }
            }
        })
        .collect()
}

/// Write parsed lines back to a string
fn serialize_codeowners(lines: &[CodeownersLine]) -> String {
    lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Find the best insertion point for a new pattern (maintains specificity order)
fn find_insertion_point(lines: &[CodeownersLine], _pattern: &str) -> usize {
    // CODEOWNERS rules are matched last-match-wins, so more specific patterns
    // should come later in the file. We'll insert at the end by default.
    // A smarter implementation could analyze pattern specificity.
    lines.len()
}

#[derive(Debug, Default, Deserialize)]
struct Settings {
    /// Custom path to CODEOWNERS file (relative to workspace root)
    path: Option<String>,
    /// Individual owner identifier (e.g. @username)
    individual: Option<String>,
    /// Team owner identifier (e.g. @org/team-name)
    team: Option<String>,
}

#[allow(dead_code)]
struct Backend {
    client: Client,
    workspace_root: RwLock<Option<PathBuf>>,
    codeowners: RwLock<Option<Owners>>,
    codeowners_path: RwLock<Option<PathBuf>>,
    settings: RwLock<Settings>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            workspace_root: RwLock::new(None),
            codeowners: RwLock::new(None),
            codeowners_path: RwLock::new(None),
            settings: RwLock::new(Settings::default()),
        }
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

    fn get_owners_for_file(&self, uri: &Url) -> Option<String> {
        let root = self.workspace_root.read().unwrap();
        let root = root.as_ref()?;

        let file_path = uri.to_file_path().ok()?;
        let relative_path = file_path.strip_prefix(root).ok()?;

        let codeowners = self.codeowners.read().unwrap();
        let codeowners = codeowners.as_ref()?;

        let owners = codeowners.of(relative_path)?;
        let owner_strs: Vec<String> = owners.iter().map(|o| o.to_string()).collect();

        Some(owner_strs.join(", "))
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
                // Simple glob matching - this is approximate
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
}

/// Simple glob pattern matching for CODEOWNERS patterns
fn pattern_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_start_matches('/');

    // Handle ** (matches everything)
    if pattern == "*" || pattern == "**" {
        return true;
    }

    // Handle directory patterns like /dir/ or dir/
    if pattern.ends_with('/') {
        let dir = pattern.trim_end_matches('/');
        return path.starts_with(dir);
    }

    // Handle patterns ending with /* or /**
    if pattern.ends_with("/**") || pattern.ends_with("/*") {
        let dir = pattern.trim_end_matches("/**").trim_end_matches("/*");
        return path.starts_with(dir);
    }

    // Handle extension patterns like *.rs
    if let Some(suffix) = pattern.strip_prefix('*') {
        return path.ends_with(suffix);
    }

    // Exact match or prefix match for directories
    path == pattern || path.starts_with(&format!("{}/", pattern))
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

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::NONE,
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
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "codeowners-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {}

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let Some(owners) = self.get_owners_for_file(uri) else {
            return Ok(None);
        };

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("**CODEOWNERS:** {}", owners),
            }),
            range: None,
        }))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
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
        let Some(owners) = self.get_owners_for_file(uri) else {
            return Ok(None);
        };

        Ok(Some(vec![InlayHint {
            position: Position {
                line: 0,
                character: 0,
            },
            label: InlayHintLabel::String(format!("Owned by: {}", owners)),
            kind: None,
            text_edits: None,
            tooltip: Some(InlayHintTooltip::String(
                "File ownership from CODEOWNERS".to_string(),
            )),
            padding_left: Some(false),
            padding_right: Some(true),
            data: None,
        }]))
    }

    async fn did_change_watched_files(&self, _params: DidChangeWatchedFilesParams) {
        self.load_codeowners();
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        if let Ok(settings) = serde_json::from_value::<Settings>(params.settings) {
            *self.settings.write().unwrap() = settings;
            self.load_codeowners();
        }
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        let command = &params.command;
        let args = params.arguments;

        // Parse arguments: [uri, pattern, owner?]
        // uri is passed but not currently used - we operate on the pattern
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
        // The LSP client should prompt and re-invoke with the owner
        let is_custom = command.ends_with(".custom");
        if is_custom && owner.is_none() {
            // Return a special response indicating we need user input
            // The client should show an input dialog and re-invoke
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
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
