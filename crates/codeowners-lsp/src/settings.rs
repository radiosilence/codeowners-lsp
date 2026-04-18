use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::diagnostics::DiagnosticConfig;

pub const CONFIG_FILE: &str = ".codeowners-lsp.toml";
pub const CONFIG_FILE_LOCAL: &str = ".codeowners-lsp.local.toml";

/// Settings for the suggest command
#[derive(Debug, Default, Deserialize, Clone)]
pub struct SuggestSettings {
    /// Command to lookup team from email
    /// Use {email} as placeholder, e.g.: "your-tool lookup {email} | jq -r .team"
    pub lookup_cmd: Option<String>,
    /// Prepend / to paths (anchored patterns)
    #[serde(default)]
    pub anchored: bool,
}

/// Shared settings for both LSP and CLI
#[derive(Debug, Default, Deserialize, Clone)]
pub struct Settings {
    /// Custom path to CODEOWNERS file (relative to workspace root)
    pub path: Option<String>,
    /// Individual owner identifier (e.g. @username)
    pub individual: Option<String>,
    /// Team owner identifier (e.g. @org/team-name)
    pub team: Option<String>,
    /// GitHub token for validating owners (reads from env if prefixed with "env:")
    pub github_token: Option<String>,
    /// Whether to validate owners against GitHub API
    #[serde(default)]
    pub validate_owners: bool,
    /// Diagnostic severity overrides (code -> "off"|"hint"|"info"|"warning"|"error")
    #[serde(default)]
    pub diagnostics: HashMap<String, String>,
    /// Suggest command settings
    #[serde(default)]
    pub suggest: SuggestSettings,
}

impl Settings {
    /// Merge another Settings into this one (other takes precedence for set values)
    pub fn merge(&mut self, other: Settings) {
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
        // Merge diagnostics (other overwrites same keys)
        for (k, v) in other.diagnostics {
            self.diagnostics.insert(k, v);
        }
        // Merge suggest settings
        if other.suggest.lookup_cmd.is_some() {
            self.suggest.lookup_cmd = other.suggest.lookup_cmd;
        }
        if other.suggest.anchored {
            self.suggest.anchored = true;
        }
    }

    /// Get DiagnosticConfig from settings
    pub fn diagnostic_config(&self) -> DiagnosticConfig {
        DiagnosticConfig::from_map(&self.diagnostics)
    }

    /// Resolve GitHub token (handles env: prefix)
    pub fn resolve_token(&self) -> Option<String> {
        self.github_token.as_ref().and_then(|token| {
            if let Some(env_var) = token.strip_prefix("env:") {
                std::env::var(env_var).ok()
            } else {
                Some(token.clone())
            }
        })
    }

    /// Get the lookup command template if configured
    #[allow(dead_code)] // Used by CLI only
    pub fn lookup_cmd(&self) -> Option<&str> {
        self.suggest.lookup_cmd.as_deref()
    }

    /// Get whether to use anchored paths in suggestions
    #[allow(dead_code)] // Used by CLI only
    pub fn suggest_anchored(&self) -> bool {
        self.suggest.anchored
    }
}

/// Load settings from config files in a directory
/// Priority: defaults < .codeowners-lsp.toml < .codeowners-lsp.local.toml
pub fn load_settings_from_path(root: &Path) -> Settings {
    let mut settings = Settings::default();

    // Load project config
    let config_path = root.join(CONFIG_FILE);
    if let Ok(content) = fs::read_to_string(&config_path) {
        if let Ok(file_settings) = toml::from_str::<Settings>(&content) {
            settings.merge(file_settings);
        }
    }

    // Load local config (user overrides)
    let local_config_path = root.join(CONFIG_FILE_LOCAL);
    if let Ok(content) = fs::read_to_string(&local_config_path) {
        if let Ok(file_settings) = toml::from_str::<Settings>(&content) {
            settings.merge(file_settings);
        }
    }

    settings
}

/// Load settings from current working directory
#[allow(dead_code)] // Used by CLI only
pub fn load_settings() -> Settings {
    let cwd = std::env::current_dir().unwrap_or_default();
    load_settings_from_path(&cwd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_settings_default() {
        let settings = Settings::default();
        assert!(settings.path.is_none());
        assert!(settings.individual.is_none());
        assert!(settings.team.is_none());
        assert!(settings.github_token.is_none());
        assert!(!settings.validate_owners);
        assert!(settings.diagnostics.is_empty());
        assert!(settings.suggest.lookup_cmd.is_none());
        assert!(!settings.suggest.anchored);
    }

    #[test]
    fn test_settings_merge_basic() {
        let mut base = Settings::default();
        let other = Settings {
            path: Some("custom/CODEOWNERS".to_string()),
            individual: Some("@user".to_string()),
            team: Some("@org/team".to_string()),
            ..Default::default()
        };
        base.merge(other);
        assert_eq!(base.path, Some("custom/CODEOWNERS".to_string()));
        assert_eq!(base.individual, Some("@user".to_string()));
        assert_eq!(base.team, Some("@org/team".to_string()));
    }

    #[test]
    fn test_settings_merge_preserves_base() {
        let mut base = Settings {
            path: Some("base/CODEOWNERS".to_string()),
            individual: Some("@base-user".to_string()),
            ..Default::default()
        };
        let other = Settings {
            team: Some("@org/team".to_string()),
            ..Default::default()
        };
        base.merge(other);
        // Base values preserved when other doesn't set them
        assert_eq!(base.path, Some("base/CODEOWNERS".to_string()));
        assert_eq!(base.individual, Some("@base-user".to_string()));
        assert_eq!(base.team, Some("@org/team".to_string()));
    }

    #[test]
    fn test_settings_merge_validate_owners() {
        let mut base = Settings::default();
        assert!(!base.validate_owners);

        let other = Settings {
            validate_owners: true,
            ..Default::default()
        };
        base.merge(other);
        assert!(base.validate_owners);
    }

    #[test]
    fn test_settings_merge_diagnostics() {
        let mut base = Settings::default();
        base.diagnostics
            .insert("rule-a".to_string(), "error".to_string());

        let mut other = Settings::default();
        other
            .diagnostics
            .insert("rule-a".to_string(), "off".to_string());
        other
            .diagnostics
            .insert("rule-b".to_string(), "warning".to_string());

        base.merge(other);
        assert_eq!(base.diagnostics.get("rule-a"), Some(&"off".to_string()));
        assert_eq!(base.diagnostics.get("rule-b"), Some(&"warning".to_string()));
    }

    #[test]
    fn test_settings_merge_suggest() {
        let mut base = Settings::default();
        let other = Settings {
            suggest: SuggestSettings {
                lookup_cmd: Some("lookup {email}".to_string()),
                anchored: true,
            },
            ..Default::default()
        };
        base.merge(other);
        assert_eq!(base.suggest.lookup_cmd, Some("lookup {email}".to_string()));
        assert!(base.suggest.anchored);
    }

    #[test]
    fn test_resolve_token_direct() {
        let settings = Settings {
            github_token: Some("ghp_direct_token".to_string()),
            ..Default::default()
        };
        assert_eq!(
            settings.resolve_token(),
            Some("ghp_direct_token".to_string())
        );
    }

    #[test]
    fn test_resolve_token_env() {
        std::env::set_var("TEST_GITHUB_TOKEN_12345", "ghp_from_env");
        let settings = Settings {
            github_token: Some("env:TEST_GITHUB_TOKEN_12345".to_string()),
            ..Default::default()
        };
        assert_eq!(settings.resolve_token(), Some("ghp_from_env".to_string()));
        std::env::remove_var("TEST_GITHUB_TOKEN_12345");
    }

    #[test]
    fn test_resolve_token_env_missing() {
        let settings = Settings {
            github_token: Some("env:NONEXISTENT_VAR_XYZ".to_string()),
            ..Default::default()
        };
        assert_eq!(settings.resolve_token(), None);
    }

    #[test]
    fn test_resolve_token_none() {
        let settings = Settings::default();
        assert_eq!(settings.resolve_token(), None);
    }

    #[test]
    fn test_lookup_cmd() {
        let settings = Settings {
            suggest: SuggestSettings {
                lookup_cmd: Some("my-cmd {email}".to_string()),
                anchored: false,
            },
            ..Default::default()
        };
        assert_eq!(settings.lookup_cmd(), Some("my-cmd {email}"));
    }

    #[test]
    fn test_suggest_anchored() {
        let mut settings = Settings::default();
        assert!(!settings.suggest_anchored());
        settings.suggest.anchored = true;
        assert!(settings.suggest_anchored());
    }

    #[test]
    fn test_load_settings_from_path_empty() {
        let dir = TempDir::new().unwrap();
        let settings = load_settings_from_path(dir.path());
        assert!(settings.path.is_none());
    }

    #[test]
    fn test_load_settings_from_path_config_file() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join(CONFIG_FILE);
        let mut file = fs::File::create(&config_path).unwrap();
        writeln!(file, r#"path = "custom/CODEOWNERS""#).unwrap();
        writeln!(file, r#"individual = "@myuser""#).unwrap();

        let settings = load_settings_from_path(dir.path());
        assert_eq!(settings.path, Some("custom/CODEOWNERS".to_string()));
        assert_eq!(settings.individual, Some("@myuser".to_string()));
    }

    #[test]
    fn test_load_settings_from_path_local_overrides() {
        let dir = TempDir::new().unwrap();

        // Base config
        let config_path = dir.path().join(CONFIG_FILE);
        let mut file = fs::File::create(&config_path).unwrap();
        writeln!(file, r#"path = "base/CODEOWNERS""#).unwrap();
        writeln!(file, r#"individual = "@base-user""#).unwrap();

        // Local config (overrides)
        let local_path = dir.path().join(CONFIG_FILE_LOCAL);
        let mut file = fs::File::create(&local_path).unwrap();
        writeln!(file, r#"individual = "@local-user""#).unwrap();
        writeln!(file, r#"team = "@org/my-team""#).unwrap();

        let settings = load_settings_from_path(dir.path());
        assert_eq!(settings.path, Some("base/CODEOWNERS".to_string())); // from base
        assert_eq!(settings.individual, Some("@local-user".to_string())); // overridden
        assert_eq!(settings.team, Some("@org/my-team".to_string())); // from local
    }

    #[test]
    fn test_load_settings_with_diagnostics() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join(CONFIG_FILE);
        let mut file = fs::File::create(&config_path).unwrap();
        writeln!(file, r#"[diagnostics]"#).unwrap();
        writeln!(file, r#"no-owners = "off""#).unwrap();
        writeln!(file, r#"invalid-pattern = "error""#).unwrap();

        let settings = load_settings_from_path(dir.path());
        assert_eq!(
            settings.diagnostics.get("no-owners"),
            Some(&"off".to_string())
        );
        assert_eq!(
            settings.diagnostics.get("invalid-pattern"),
            Some(&"error".to_string())
        );
    }

    #[test]
    fn test_load_settings_with_suggest() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join(CONFIG_FILE);
        let mut file = fs::File::create(&config_path).unwrap();
        writeln!(file, r#"[suggest]"#).unwrap();
        writeln!(file, r#"lookup_cmd = "lookup {{email}}""#).unwrap();
        writeln!(file, r#"anchored = true"#).unwrap();

        let settings = load_settings_from_path(dir.path());
        assert_eq!(
            settings.suggest.lookup_cmd,
            Some("lookup {email}".to_string())
        );
        assert!(settings.suggest.anchored);
    }

    #[test]
    fn test_diagnostic_config() {
        let mut settings = Settings::default();
        settings
            .diagnostics
            .insert("no-owners".to_string(), "off".to_string());
        // Just verify it doesn't panic and returns a config
        let _config = settings.diagnostic_config();
    }

    #[test]
    fn test_settings_merge_github_token() {
        let mut base = Settings {
            github_token: Some("base-token".to_string()),
            ..Default::default()
        };
        let other = Settings {
            github_token: Some("other-token".to_string()),
            ..Default::default()
        };
        base.merge(other);
        // Other overwrites base
        assert_eq!(base.github_token, Some("other-token".to_string()));
    }

    #[test]
    fn test_settings_merge_github_token_none_preserves() {
        let mut base = Settings {
            github_token: Some("base-token".to_string()),
            ..Default::default()
        };
        let other = Settings::default(); // github_token is None
        base.merge(other);
        // Base preserved when other is None
        assert_eq!(base.github_token, Some("base-token".to_string()));
    }
}
