use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

/// Metadata for a GitHub user
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub login: String,
    pub name: Option<String>,
    pub html_url: String,
    pub avatar_url: Option<String>,
    pub bio: Option<String>,
    pub company: Option<String>,
}

/// Metadata for a GitHub team
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamInfo {
    pub slug: String,
    pub name: String,
    pub org: String,
    pub description: Option<String>,
    pub html_url: String,
    pub members_count: Option<u32>,
    pub repos_count: Option<u32>,
}

/// Validation result with optional metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OwnerInfo {
    /// Valid user with metadata
    User(UserInfo),
    /// Valid team with metadata
    Team(TeamInfo),
    /// Invalid owner (doesn't exist)
    Invalid,
    /// Couldn't validate (no permission, rate limited, etc)
    Unknown,
}

impl OwnerInfo {
    pub fn is_valid(&self) -> bool {
        matches!(self, OwnerInfo::User(_) | OwnerInfo::Team(_))
    }

    #[allow(dead_code)] // May be used later
    pub fn is_invalid(&self) -> bool {
        matches!(self, OwnerInfo::Invalid)
    }
}

/// In-memory cache for GitHub owner validation results
#[derive(Default)]
pub struct GitHubCache {
    /// Map from owner string to validation result with metadata
    pub owners: HashMap<String, OwnerInfo>,
}

/// Persistent cache stored in .codeowners-lsp/cache.json
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PersistentCache {
    /// Validated owners with metadata
    #[serde(default)]
    pub owners: HashMap<String, OwnerInfo>,
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

    /// Check if cache is stale (older than 24 hours)
    #[allow(dead_code)] // May be used later
    pub fn is_stale(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now - self.last_updated > 86400 // 24 hours
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

/// Response from GitHub user API (subset of fields we care about)
#[derive(Debug, Deserialize)]
struct GitHubUserResponse {
    login: String,
    name: Option<String>,
    html_url: String,
    avatar_url: Option<String>,
    bio: Option<String>,
    company: Option<String>,
}

/// Response from GitHub team API (subset of fields we care about)
#[derive(Debug, Deserialize)]
struct GitHubTeamResponse {
    slug: String,
    name: String,
    description: Option<String>,
    html_url: String,
    members_count: Option<u32>,
    repos_count: Option<u32>,
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
        for (owner, info) in &persistent.owners {
            cache.owners.insert(owner.clone(), info.clone());
        }
    }

    /// Export validation results to persistent cache
    #[allow(dead_code)] // Used by LSP only
    pub fn export_to_persistent(&self) -> PersistentCache {
        let cache = self.cache.read().unwrap();
        let mut persistent = PersistentCache {
            owners: cache.owners.clone(),
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
            .owners
            .iter()
            .filter(|(_, info)| info.is_valid())
            .map(|(owner, _)| owner.clone())
            .collect()
    }

    /// Fetch GitHub user info
    async fn fetch_user(&self, username: &str, token: &str) -> Option<OwnerInfo> {
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

        let status = response.status();
        if status.is_success() {
            if let Ok(user) = response.json::<GitHubUserResponse>().await {
                return Some(OwnerInfo::User(UserInfo {
                    login: user.login,
                    name: user.name,
                    html_url: user.html_url,
                    avatar_url: user.avatar_url,
                    bio: user.bio,
                    company: user.company,
                }));
            }
        } else if status.as_u16() == 404 {
            return Some(OwnerInfo::Invalid);
        }
        // 403, rate limit, network error -> Unknown
        Some(OwnerInfo::Unknown)
    }

    /// Fetch GitHub team info
    async fn fetch_team(&self, org: &str, team_slug: &str, token: &str) -> Option<OwnerInfo> {
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

        let status = response.status();
        if status.is_success() {
            if let Ok(team) = response.json::<GitHubTeamResponse>().await {
                return Some(OwnerInfo::Team(TeamInfo {
                    slug: team.slug,
                    name: team.name,
                    org: org.to_string(),
                    description: team.description,
                    html_url: team.html_url,
                    members_count: team.members_count,
                    repos_count: team.repos_count,
                }));
            }
        } else if status.as_u16() == 404 {
            return Some(OwnerInfo::Invalid);
        }
        // 403 = no permission, treat as unknown (might be valid, just can't see)
        Some(OwnerInfo::Unknown)
    }

    /// Validate a GitHub user exists (returns bool for backwards compat)
    #[allow(dead_code)] // Used by CLI
    pub async fn validate_user(&self, username: &str, token: &str) -> Option<bool> {
        match self.fetch_user(username, token).await {
            Some(OwnerInfo::User(_)) => Some(true),
            Some(OwnerInfo::Invalid) => Some(false),
            _ => None,
        }
    }

    /// Validate a GitHub team exists in the org (returns bool for backwards compat)
    #[allow(dead_code)] // Used by CLI
    pub async fn validate_team(&self, org: &str, team_slug: &str, token: &str) -> Option<bool> {
        match self.fetch_team(org, team_slug, token).await {
            Some(OwnerInfo::Team(_)) => Some(true),
            Some(OwnerInfo::Invalid) => Some(false),
            _ => None,
        }
    }

    /// Validate an owner and fetch metadata (cached)
    pub async fn validate_owner_with_info(&self, owner: &str, token: &str) -> Option<OwnerInfo> {
        // Check cache first
        {
            let cache = self.cache.read().unwrap();
            if let Some(info) = cache.owners.get(owner) {
                return Some(info.clone());
            }
        }

        let result = if let Some(username) = owner.strip_prefix('@') {
            if username.contains('/') {
                // Team: @org/team
                let parts: Vec<&str> = username.split('/').collect();
                if parts.len() == 2 {
                    let org = parts[0];
                    let team = parts[1];
                    self.fetch_team(org, team, token).await
                } else {
                    None
                }
            } else {
                // User: @username
                self.fetch_user(username, token).await
            }
        } else {
            // Email - can't validate via GitHub
            None
        };

        // Cache the result
        if let Some(ref info) = result {
            let mut cache = self.cache.write().unwrap();
            cache.owners.insert(owner.to_string(), info.clone());
        }

        result
    }

    /// Validate an owner against GitHub API (cached, returns bool for backwards compat)
    pub async fn validate_owner(&self, owner: &str, token: &str) -> Option<bool> {
        let info = self.validate_owner_with_info(owner, token).await?;
        match info {
            OwnerInfo::User(_) | OwnerInfo::Team(_) => Some(true),
            OwnerInfo::Invalid => Some(false),
            OwnerInfo::Unknown => None,
        }
    }

    /// Check if an owner is cached
    #[allow(dead_code)] // Used by LSP, not CLI
    pub fn is_cached(&self, owner: &str) -> bool {
        self.cache.read().unwrap().owners.contains_key(owner)
    }

    /// Get validation result from cache (None if not cached)
    #[allow(dead_code)] // Used by LSP, not CLI
    pub fn get_cached(&self, owner: &str) -> Option<bool> {
        self.cache
            .read()
            .unwrap()
            .owners
            .get(owner)
            .map(|info| matches!(info, OwnerInfo::User(_) | OwnerInfo::Team(_)))
    }

    /// Get owner info from cache (None if not cached)
    #[allow(dead_code)] // Used by LSP, not CLI
    pub fn get_owner_info(&self, owner: &str) -> Option<OwnerInfo> {
        self.cache.read().unwrap().owners.get(owner).cloned()
    }

    /// Clear the cache
    #[cfg(test)]
    pub fn clear_cache(&self) {
        self.cache.write().unwrap().owners.clear();
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
            cache.owners.insert(
                "@user".to_string(),
                OwnerInfo::User(UserInfo {
                    login: "user".to_string(),
                    name: Some("Test User".to_string()),
                    html_url: "https://github.com/user".to_string(),
                    avatar_url: None,
                    bio: None,
                    company: None,
                }),
            );
        }

        // Now cached
        assert!(client.is_cached("@user"));
        assert_eq!(client.get_cached("@user"), Some(true));

        // Check owner info
        let info = client.get_owner_info("@user");
        assert!(matches!(info, Some(OwnerInfo::User(_))));

        // Clear cache
        client.clear_cache();
        assert!(!client.is_cached("@user"));
    }

    #[test]
    fn test_owner_info_validity() {
        let user = OwnerInfo::User(UserInfo {
            login: "test".to_string(),
            name: None,
            html_url: "https://github.com/test".to_string(),
            avatar_url: None,
            bio: None,
            company: None,
        });
        assert!(user.is_valid());
        assert!(!user.is_invalid());

        let team = OwnerInfo::Team(TeamInfo {
            slug: "team".to_string(),
            name: "Team".to_string(),
            org: "org".to_string(),
            description: None,
            html_url: "https://github.com/orgs/org/teams/team".to_string(),
            members_count: None,
            repos_count: None,
        });
        assert!(team.is_valid());
        assert!(!team.is_invalid());

        let invalid = OwnerInfo::Invalid;
        assert!(!invalid.is_valid());
        assert!(invalid.is_invalid());

        let unknown = OwnerInfo::Unknown;
        assert!(!unknown.is_valid());
        assert!(!unknown.is_invalid());
    }
}
