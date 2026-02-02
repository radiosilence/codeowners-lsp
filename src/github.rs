use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

/// In-memory cache for GitHub owner validation results
#[derive(Default)]
pub struct GitHubCache {
    /// Map from owner string to validation result (true = valid, false = invalid)
    pub validated: HashMap<String, bool>,
}

/// Persistent cache stored in .codeowners-lsp/cache.json
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PersistentCache {
    /// Validated owners: owner -> valid (true/false)
    #[serde(default)]
    pub owners: HashMap<String, bool>,
    /// Timestamp of last validation (Unix seconds)
    #[serde(default)]
    pub last_updated: u64,
}

impl PersistentCache {
    /// Load cache from disk
    #[allow(dead_code)] // Used by LSP only
    pub fn load(workspace_root: &Path) -> Self {
        let cache_path = workspace_root.join(".codeowners-lsp").join("cache.json");
        if let Ok(content) = fs::read_to_string(&cache_path) {
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            Self::default()
        }
    }

    /// Save cache to disk
    #[allow(dead_code)] // Used by LSP only
    pub fn save(&self, workspace_root: &Path) -> std::io::Result<()> {
        let cache_dir = workspace_root.join(".codeowners-lsp");
        fs::create_dir_all(&cache_dir)?;

        // Create .gitignore if it doesn't exist
        let gitignore_path = cache_dir.join(".gitignore");
        if !gitignore_path.exists() {
            fs::write(&gitignore_path, "*\n")?;
        }

        let cache_path = cache_dir.join("cache.json");
        let content = serde_json::to_string_pretty(self)?;
        fs::write(cache_path, content)
    }

    /// Check if cache is stale (older than 1 hour)
    #[allow(dead_code)] // May be used later
    pub fn is_stale(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now - self.last_updated > 3600 // 1 hour
    }

    /// Update timestamp
    #[allow(dead_code)] // Used by LSP only
    pub fn touch(&mut self) {
        self.last_updated = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
    }
}

/// GitHub API client for validating owners
pub struct GitHubClient {
    http_client: reqwest::Client,
    cache: RwLock<GitHubCache>,
}

impl GitHubClient {
    pub fn new() -> Self {
        Self {
            http_client: reqwest::Client::new(),
            cache: RwLock::new(GitHubCache::default()),
        }
    }

    /// Load validation results from persistent cache
    #[allow(dead_code)] // Used by LSP only
    pub fn load_from_persistent(&self, persistent: &PersistentCache) {
        let mut cache = self.cache.write().unwrap();
        for (owner, valid) in &persistent.owners {
            cache.validated.insert(owner.clone(), *valid);
        }
    }

    /// Export validation results to persistent cache
    #[allow(dead_code)] // Used by LSP only
    pub fn export_to_persistent(&self) -> PersistentCache {
        let cache = self.cache.read().unwrap();
        let mut persistent = PersistentCache {
            owners: cache.validated.clone(),
            ..Default::default()
        };
        persistent.touch();
        persistent
    }

    /// Get all cached owners (for autocomplete)
    #[allow(dead_code)] // Used by LSP only
    pub fn get_cached_owners(&self) -> Vec<String> {
        let cache = self.cache.read().unwrap();
        cache
            .validated
            .iter()
            .filter(|(_, valid)| **valid)
            .map(|(owner, _)| owner.clone())
            .collect()
    }

    /// Validate a GitHub user exists
    pub async fn validate_user(&self, username: &str, token: &str) -> Option<bool> {
        let url = format!("https://api.github.com/users/{}", username);
        let response = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "codeowners-lsp")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .ok()?;

        Some(response.status().is_success())
    }

    /// Validate a GitHub team exists in the org
    pub async fn validate_team(&self, org: &str, team_slug: &str, token: &str) -> Option<bool> {
        let url = format!("https://api.github.com/orgs/{}/teams/{}", org, team_slug);
        let response = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "codeowners-lsp")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .ok()?;

        // 404 = team doesn't exist, 403 = no permission (treat as unknown, not invalid)
        if response.status().as_u16() == 403 {
            return None; // Can't validate, skip
        }

        Some(response.status().is_success())
    }

    /// Validate an owner against GitHub API (cached)
    pub async fn validate_owner(&self, owner: &str, token: &str) -> Option<bool> {
        // Check cache first
        {
            let cache = self.cache.read().unwrap();
            if let Some(&result) = cache.validated.get(owner) {
                return Some(result);
            }
        }

        let result = if let Some(username) = owner.strip_prefix('@') {
            if username.contains('/') {
                // Team: @org/team
                let parts: Vec<&str> = username.split('/').collect();
                if parts.len() == 2 {
                    let org = parts[0];
                    let team = parts[1];
                    self.validate_team(org, team, token).await
                } else {
                    None
                }
            } else {
                // User: @username
                self.validate_user(username, token).await
            }
        } else {
            // Email - can't validate via GitHub
            None
        };

        // Cache the result
        if let Some(valid) = result {
            let mut cache = self.cache.write().unwrap();
            cache.validated.insert(owner.to_string(), valid);
        }

        result
    }

    /// Check if an owner is cached
    #[cfg(test)]
    pub fn is_cached(&self, owner: &str) -> bool {
        self.cache.read().unwrap().validated.contains_key(owner)
    }

    /// Clear the cache
    #[cfg(test)]
    pub fn clear_cache(&self) {
        self.cache.write().unwrap().validated.clear();
    }
}

impl Default for GitHubClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_operations() {
        let client = GitHubClient::new();

        // Initially not cached
        assert!(!client.is_cached("@user"));

        // Manually insert into cache
        {
            let mut cache = client.cache.write().unwrap();
            cache.validated.insert("@user".to_string(), true);
        }

        // Now cached
        assert!(client.is_cached("@user"));

        // Clear cache
        client.clear_cache();
        assert!(!client.is_cached("@user"));
    }

    // Note: Actual API tests would require mocking or integration testing
    // with a real GitHub token, which is outside the scope of unit tests
}
