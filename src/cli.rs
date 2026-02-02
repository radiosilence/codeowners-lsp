mod blame;
mod commands;
mod diagnostics;
mod file_cache;
mod github;
mod ownership;
mod parser;
mod pattern;
#[allow(dead_code)]
mod validation;

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};

#[derive(Parser)]
#[command(name = "codeowners-cli")]
#[command(about = "Lint and inspect CODEOWNERS files", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check CODEOWNERS for issues
    Lint {
        /// Path to CODEOWNERS file (default: auto-detect)
        path: Option<PathBuf>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Format CODEOWNERS file (normalizes spacing)
    #[command(alias = "format")]
    Fmt {
        /// Path to CODEOWNERS file (default: auto-detect)
        path: Option<PathBuf>,
        /// Write changes to file (default: dry-run)
        #[arg(short, long)]
        write: bool,
    },
    /// Auto-fix safe issues (duplicate owners, shadowed rules)
    Fix {
        /// Path to CODEOWNERS file (default: auto-detect)
        path: Option<PathBuf>,
        /// Write changes to file (default: dry-run)
        #[arg(short, long)]
        write: bool,
    },
    /// Show which rule owns a specific file
    Check {
        /// File path to check ownership of
        file: String,
    },
    /// Show files without owners and coverage percentage
    Coverage,
    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Validate all owners against GitHub API
    ValidateOwners {
        /// GitHub token (or use GITHUB_TOKEN env var)
        #[arg(long, env = "GITHUB_TOKEN")]
        token: String,
    },
    /// Show all files color-coded by owner
    Tree,
    /// Show config file paths and merged settings
    Config,
    /// Suggest owners for unowned files based on git history
    Suggest {
        /// Minimum confidence threshold (0-100)
        #[arg(long, default_value = "30")]
        min_confidence: f64,
        /// Output format (human, codeowners, json)
        #[arg(long, default_value = "human")]
        format: String,
        /// Maximum number of suggestions
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// Suggest optimizations to simplify CODEOWNERS patterns
    Optimize {
        /// Write changes to file (default: preview only)
        #[arg(short, long)]
        write: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Minimum files to suggest directory consolidation
        #[arg(long, default_value = "3")]
        min_files: usize,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Cli::parse();

    match args.command {
        Commands::Lint { path, json } => commands::lint(path, json).await,
        Commands::Fmt { path, write } => commands::fmt(path, write),
        Commands::Fix { path, write } => commands::fix(path, write),
        Commands::Check { file } => commands::check(&file),
        Commands::Coverage => commands::coverage(),
        Commands::Completions { shell } => {
            generate(
                shell,
                &mut Cli::command(),
                "codeowners-cli",
                &mut io::stdout(),
            );
            ExitCode::SUCCESS
        }
        Commands::ValidateOwners { token } => commands::validate_owners(&token).await,
        Commands::Tree => commands::tree(),
        Commands::Config => commands::config(),
        Commands::Suggest {
            min_confidence,
            format,
            limit,
        } => {
            let format = match format.to_lowercase().as_str() {
                "json" => commands::SuggestFormat::Json,
                "codeowners" => commands::SuggestFormat::Codeowners,
                _ => commands::SuggestFormat::Human,
            };
            commands::suggest(commands::SuggestOptions {
                min_confidence,
                format,
                limit,
                include_owned: false,
            })
        }
        Commands::Optimize {
            write,
            json,
            min_files,
        } => commands::optimize(commands::OptimizeOptions {
            format: if json {
                commands::OptimizeFormat::Json
            } else {
                commands::OptimizeFormat::Human
            },
            min_files_for_dir: min_files,
            write,
        }),
    }
}
