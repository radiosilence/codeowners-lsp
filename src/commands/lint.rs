use std::path::PathBuf;
use std::process::ExitCode;
use std::{env, fs};

use colored::Colorize;

use crate::diagnostics;
use crate::file_cache::FileCache;
use crate::ownership::{find_codeowners, get_repo_root};

pub fn lint(path: Option<PathBuf>, json_output: bool) -> ExitCode {
    let cwd = env::current_dir().expect("Failed to get current directory");

    let codeowners_path = path.unwrap_or_else(|| {
        find_codeowners(&cwd).unwrap_or_else(|| {
            eprintln!("No CODEOWNERS file found");
            std::process::exit(1);
        })
    });

    if !codeowners_path.exists() {
        eprintln!("File not found: {}", codeowners_path.display());
        return ExitCode::from(1);
    }

    let content = match fs::read_to_string(&codeowners_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to read {}: {}", codeowners_path.display(), e);
            return ExitCode::from(1);
        }
    };

    let repo_root = get_repo_root(&codeowners_path, &cwd);
    let file_cache = FileCache::new(&repo_root);
    let diag_config = diagnostics::DiagnosticConfig::default();
    let (diagnostics, _) =
        diagnostics::compute_diagnostics_sync(&content, Some(&file_cache), &diag_config);

    if json_output {
        let json = serde_json::json!({
            "file": codeowners_path.display().to_string(),
            "diagnostics": diagnostics.iter().map(|d| {
                serde_json::json!({
                    "line": d.range.start.line + 1,
                    "column": d.range.start.character + 1,
                    "severity": match d.severity {
                        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR) => "error",
                        Some(tower_lsp::lsp_types::DiagnosticSeverity::WARNING) => "warning",
                        Some(tower_lsp::lsp_types::DiagnosticSeverity::HINT) => "hint",
                        Some(tower_lsp::lsp_types::DiagnosticSeverity::INFORMATION) => "info",
                        _ => "unknown",
                    },
                    "code": d.code,
                    "message": d.message,
                })
            }).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&json).unwrap());
    } else {
        if diagnostics.is_empty() {
            println!(
                "{} {} - no issues found",
                "✓".green(),
                codeowners_path.display()
            );
            return ExitCode::SUCCESS;
        }

        println!("{}:", codeowners_path.display().to_string().bold());
        for d in &diagnostics {
            let (severity_label, severity_color) = match d.severity {
                Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR) => ("error", "red"),
                Some(tower_lsp::lsp_types::DiagnosticSeverity::WARNING) => ("warning", "yellow"),
                Some(tower_lsp::lsp_types::DiagnosticSeverity::HINT) => ("hint", "cyan"),
                Some(tower_lsp::lsp_types::DiagnosticSeverity::INFORMATION) => ("info", "blue"),
                _ => ("unknown", "white"),
            };
            let code = d
                .code
                .as_ref()
                .map(|c| match c {
                    tower_lsp::lsp_types::NumberOrString::String(s) => s.clone(),
                    tower_lsp::lsp_types::NumberOrString::Number(n) => n.to_string(),
                })
                .unwrap_or_default();
            println!(
                "  {} {} {} {}",
                format!("line {}:", d.range.start.line + 1).dimmed(),
                format!("[{}]", severity_label).color(severity_color),
                code.bold(),
                d.message
            );
        }
    }

    let has_errors = diagnostics.iter().any(|d| {
        matches!(
            d.severity,
            Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
        )
    });

    if has_errors {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
