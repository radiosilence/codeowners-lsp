use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use colored::Colorize;
use serde::Deserialize;

const CONFIG_FILE: &str = ".codeowners-lsp.toml";
const CONFIG_FILE_LOCAL: &str = ".codeowners-lsp.local.toml";

#[derive(Debug, Default, Deserialize, Clone)]
pub struct Settings {
    path: Option<String>,
    individual: Option<String>,
    team: Option<String>,
    github_token: Option<String>,
    #[serde(default)]
    validate_owners: bool,
    #[serde(default)]
    diagnostics: HashMap<String, String>,
    /// Command to lookup team from email (for suggest command)
    /// Use {email} as placeholder, e.g.: "houston who is {email} --json | jq .team -r"
    lookup_cmd: Option<String>,
}

impl Settings {
    /// Get the lookup command template if configured
    pub fn lookup_cmd(&self) -> Option<&str> {
        self.lookup_cmd.as_deref()
    }

    fn merge(&mut self, other: Settings) {
        if other.path.is_some() {
            self.path = other.path;
        }
        if other.individual.is_some() {
            self.individual = other.individual;
        }
        if other.team.is_some() {
            self.team = other.team;
        }
        if other.github_token.is_some() {
            self.github_token = other.github_token;
        }
        if other.validate_owners {
            self.validate_owners = true;
        }
        for (k, v) in other.diagnostics {
            self.diagnostics.insert(k, v);
        }
        if other.lookup_cmd.is_some() {
            self.lookup_cmd = other.lookup_cmd;
        }
    }
}

struct ConfigSource {
    path: PathBuf,
    exists: bool,
    settings: Option<Settings>,
    error: Option<String>,
}

pub fn config() -> ExitCode {
    let cwd = env::current_dir().unwrap_or_default();

    let config_path = cwd.join(CONFIG_FILE);
    let local_config_path = cwd.join(CONFIG_FILE_LOCAL);

    // Load each config file
    let project_config = load_config(&config_path);
    let local_config = load_config(&local_config_path);

    // Print header
    println!("{}", "Config Files".bold().underline());
    println!();

    // Print project config status
    print_config_source("Project", &project_config);
    print_config_source("Local", &local_config);

    // Compute merged config
    let mut merged = Settings::default();
    if let Some(ref settings) = project_config.settings {
        merged.merge(settings.clone());
    }
    if let Some(ref settings) = local_config.settings {
        merged.merge(settings.clone());
    }

    // Print merged config
    println!();
    println!("{}", "Merged Config".bold().underline());
    println!();
    print_settings(&merged);

    ExitCode::SUCCESS
}

fn load_config(path: &PathBuf) -> ConfigSource {
    if !path.exists() {
        return ConfigSource {
            path: path.clone(),
            exists: false,
            settings: None,
            error: None,
        };
    }

    match fs::read_to_string(path) {
        Ok(content) => match toml::from_str::<Settings>(&content) {
            Ok(settings) => ConfigSource {
                path: path.clone(),
                exists: true,
                settings: Some(settings),
                error: None,
            },
            Err(e) => ConfigSource {
                path: path.clone(),
                exists: true,
                settings: None,
                error: Some(format!("Parse error: {}", e)),
            },
        },
        Err(e) => ConfigSource {
            path: path.clone(),
            exists: true,
            settings: None,
            error: Some(format!("Read error: {}", e)),
        },
    }
}

fn print_config_source(label: &str, source: &ConfigSource) {
    let path_display = source.path.display();

    if !source.exists {
        println!(
            "{:<8} {}  {}",
            format!("{}:", label).dimmed(),
            path_display.to_string().dimmed(),
            "(not found)".dimmed()
        );
        return;
    }

    if let Some(ref err) = source.error {
        println!(
            "{:<8} {}  {}",
            format!("{}:", label).yellow(),
            path_display.to_string().yellow(),
            err.red()
        );
        return;
    }

    println!(
        "{:<8} {}",
        format!("{}:", label).green(),
        path_display.to_string().green()
    );

    // Show what's set in this file
    if let Some(ref settings) = source.settings {
        print_settings_brief(settings);
    }
}

fn print_settings_brief(settings: &Settings) {
    if let Some(ref path) = settings.path {
        println!("           {} {}", "path:".dimmed(), path);
    }
    if let Some(ref individual) = settings.individual {
        println!("           {} {}", "individual:".dimmed(), individual);
    }
    if let Some(ref team) = settings.team {
        println!("           {} {}", "team:".dimmed(), team);
    }
    if settings.github_token.is_some() {
        println!("           {} {}", "github_token:".dimmed(), "(set)".cyan());
    }
    if settings.validate_owners {
        println!("           {} true", "validate_owners:".dimmed());
    }
    if !settings.diagnostics.is_empty() {
        println!(
            "           {} {}",
            "diagnostics:".dimmed(),
            format!("{} rules", settings.diagnostics.len()).cyan()
        );
    }
    if settings.lookup_cmd.is_some() {
        println!("           {} {}", "lookup_cmd:".dimmed(), "(set)".cyan());
    }
}

fn print_settings(settings: &Settings) {
    // path
    print!("  {:<18} ", "path:".cyan());
    match &settings.path {
        Some(p) => println!("{}", p),
        None => println!("{}", "(default)".dimmed()),
    }

    // individual
    print!("  {:<18} ", "individual:".cyan());
    match &settings.individual {
        Some(i) => println!("{}", i.green()),
        None => println!("{}", "(not set)".dimmed()),
    }

    // team
    print!("  {:<18} ", "team:".cyan());
    match &settings.team {
        Some(t) => println!("{}", t.green()),
        None => println!("{}", "(not set)".dimmed()),
    }

    // github_token
    print!("  {:<18} ", "github_token:".cyan());
    match &settings.github_token {
        Some(t) => {
            if t.starts_with("env:") {
                println!("{}", t.yellow());
            } else {
                println!("{}", "(set, hidden)".yellow());
            }
        }
        None => println!("{}", "(not set)".dimmed()),
    }

    // validate_owners
    print!("  {:<18} ", "validate_owners:".cyan());
    if settings.validate_owners {
        println!("{}", "true".green());
    } else {
        println!("{}", "false".dimmed());
    }

    // diagnostics
    print!("  {:<18} ", "diagnostics:".cyan());
    if settings.diagnostics.is_empty() {
        println!("{}", "(defaults)".dimmed());
    } else {
        println!();
        let mut sorted: Vec<_> = settings.diagnostics.iter().collect();
        sorted.sort_by_key(|(k, _)| *k);
        for (code, severity) in sorted {
            let severity_colored = match severity.as_str() {
                "off" | "none" | "disabled" => severity.dimmed(),
                "hint" => severity.cyan(),
                "info" => severity.blue(),
                "warning" | "warn" => severity.yellow(),
                "error" => severity.red(),
                _ => severity.normal(),
            };
            println!("    {:<24} {}", code, severity_colored);
        }
    }

    // lookup_cmd
    print!("  {:<18} ", "lookup_cmd:".cyan());
    match &settings.lookup_cmd {
        Some(cmd) => println!("{}", cmd.yellow()),
        None => println!("{}", "(not set)".dimmed()),
    }
}

/// Load merged settings from config files
pub fn load_settings() -> Settings {
    let cwd = env::current_dir().unwrap_or_default();
    let config_path = cwd.join(CONFIG_FILE);
    let local_config_path = cwd.join(CONFIG_FILE_LOCAL);

    let mut merged = Settings::default();

    if let Some(settings) = load_config(&config_path).settings {
        merged.merge(settings);
    }
    if let Some(settings) = load_config(&local_config_path).settings {
        merged.merge(settings);
    }

    merged
}
