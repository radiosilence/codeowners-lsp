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
    /// Couldn't validate (no permission, rate limited, network error, etc)
    Unknown(String),
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
    /// Base URL for API requests (allows testing with mock server)
    base_url: String,
}

impl GitHubClient {
    pub fn new() -> Self {
        Self {
            http_client: reqwest::Client::new(),
            cache: RwLock::new(GitHubCache::default()),
            base_url: "https://api.github.com".to_string(),
        }
    }

    /// Create a client with a custom base URL (for testing)
    #[doc(hidden)]
    pub fn with_base_url(base_url: &str) -> Self {
        Self {
            http_client: reqwest::Client::new(),
            cache: RwLock::new(GitHubCache::default()),
            base_url: base_url.to_string(),
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
        let url = format!("{}/users/{}", self.base_url, username);
        let response = match self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "codeowners-lsp")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return Some(OwnerInfo::Unknown(format!("network error: {e}"))),
        };

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
        } else if status.as_u16() == 429 {
            return Some(OwnerInfo::Unknown("rate limit exceeded".to_string()));
        } else if status.as_u16() == 403 {
            return Some(OwnerInfo::Unknown(
                "forbidden - check token permissions".to_string(),
            ));
        }
        Some(OwnerInfo::Unknown(format!("HTTP {status}")))
    }

    /// Fetch GitHub team info
    async fn fetch_team(&self, org: &str, team_slug: &str, token: &str) -> Option<OwnerInfo> {
        let url = format!("{}/orgs/{}/teams/{}", self.base_url, org, team_slug);
        let response = match self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "codeowners-lsp")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return Some(OwnerInfo::Unknown(format!("network error: {e}"))),
        };

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
            // GitHub returns 404 for both "team doesn't exist" AND "team exists
            // but token lacks visibility" (no read:org scope). Unlike /users/
            // which is public, we can't distinguish these cases, so treat as
            // Unknown rather than Invalid to avoid false positives.
            return Some(OwnerInfo::Unknown(
                "team not found or token lacks read:org scope".to_string(),
            ));
        } else if status.as_u16() == 429 {
            return Some(OwnerInfo::Unknown("rate limit exceeded".to_string()));
        } else if status.as_u16() == 403 {
            return Some(OwnerInfo::Unknown(
                "forbidden - check token permissions".to_string(),
            ));
        }
        Some(OwnerInfo::Unknown(format!("HTTP {status}")))
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
            OwnerInfo::Unknown(_) => None,
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

    /// Insert an entry into the cache (for testing)
    #[doc(hidden)]
    pub fn insert_cached(&self, owner: &str, info: OwnerInfo) {
        self.cache
            .write()
            .unwrap()
            .owners
            .insert(owner.to_string(), info);
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
    use tempfile::tempdir;

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

        let unknown = OwnerInfo::Unknown("test".into());
        assert!(!unknown.is_valid());
        assert!(!unknown.is_invalid());
    }

    #[test]
    fn test_persistent_cache_load_missing_file() {
        let dir = tempdir().unwrap();
        let cache = PersistentCache::load(dir.path());

        assert!(cache.owners.is_empty());
        assert_eq!(cache.last_updated, 0);
    }

    #[test]
    fn test_persistent_cache_save_and_load() {
        let dir = tempdir().unwrap();

        let mut cache = PersistentCache::default();
        cache.owners.insert(
            "@user".to_string(),
            OwnerInfo::User(UserInfo {
                login: "user".to_string(),
                name: Some("Test User".to_string()),
                html_url: "https://github.com/user".to_string(),
                avatar_url: Some("https://avatar.url".to_string()),
                bio: Some("A developer".to_string()),
                company: Some("Acme".to_string()),
            }),
        );
        cache.touch();

        // Save
        cache.save(dir.path()).unwrap();

        // Verify directory structure
        assert!(dir.path().join(".codeowners-lsp").exists());
        assert!(dir.path().join(".codeowners-lsp/cache.json").exists());
        assert!(dir.path().join(".codeowners-lsp/.gitignore").exists());

        // Verify .gitignore contents
        let gitignore = fs::read_to_string(dir.path().join(".codeowners-lsp/.gitignore")).unwrap();
        assert_eq!(gitignore, "*\n");

        // Load and verify
        let loaded = PersistentCache::load(dir.path());
        assert_eq!(loaded.owners.len(), 1);
        assert!(loaded.owners.contains_key("@user"));
        assert!(loaded.last_updated > 0);
    }

    #[test]
    fn test_persistent_cache_is_stale() {
        let mut cache = PersistentCache::default();

        // Default timestamp (0) is definitely stale
        assert!(cache.is_stale());

        // Fresh timestamp is not stale
        cache.touch();
        assert!(!cache.is_stale());

        // Old timestamp is stale
        cache.last_updated = 1; // Unix epoch + 1 second
        assert!(cache.is_stale());
    }

    #[test]
    fn test_persistent_cache_touch() {
        let mut cache = PersistentCache::default();
        assert_eq!(cache.last_updated, 0);

        cache.touch();
        assert!(cache.last_updated > 0);

        // Should be recent (within last minute)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(now - cache.last_updated < 60);
    }

    #[test]
    fn test_persistent_cache_load_corrupted_json() {
        let dir = tempdir().unwrap();

        // Create cache dir and write invalid JSON
        let cache_dir = dir.path().join(".codeowners-lsp");
        fs::create_dir_all(&cache_dir).unwrap();
        fs::write(cache_dir.join("cache.json"), "not valid json {{{").unwrap();

        // Should return default on parse error
        let cache = PersistentCache::load(dir.path());
        assert!(cache.owners.is_empty());
    }

    #[test]
    fn test_github_client_default() {
        let client = GitHubClient::default();
        assert!(!client.is_cached("@anyone"));
    }

    #[test]
    fn test_get_cached_owners() {
        let client = GitHubClient::new();

        // Add a valid user
        {
            let mut cache = client.cache.write().unwrap();
            cache.owners.insert(
                "@valid_user".to_string(),
                OwnerInfo::User(UserInfo {
                    login: "valid_user".to_string(),
                    name: None,
                    html_url: "https://github.com/valid_user".to_string(),
                    avatar_url: None,
                    bio: None,
                    company: None,
                }),
            );
            cache.owners.insert(
                "@org/team".to_string(),
                OwnerInfo::Team(TeamInfo {
                    slug: "team".to_string(),
                    name: "Team".to_string(),
                    org: "org".to_string(),
                    description: None,
                    html_url: "https://github.com/orgs/org/teams/team".to_string(),
                    members_count: None,
                    repos_count: None,
                }),
            );
            cache
                .owners
                .insert("@invalid".to_string(), OwnerInfo::Invalid);
            cache
                .owners
                .insert("@unknown".to_string(), OwnerInfo::Unknown("test".into()));
        }

        let cached = client.get_cached_owners();
        assert_eq!(cached.len(), 2);
        assert!(cached.contains(&"@valid_user".to_string()));
        assert!(cached.contains(&"@org/team".to_string()));
        // Invalid and Unknown should not be included
    }

    #[test]
    fn test_load_from_persistent() {
        let client = GitHubClient::new();

        let mut persistent = PersistentCache::default();
        persistent.owners.insert(
            "@persisted_user".to_string(),
            OwnerInfo::User(UserInfo {
                login: "persisted_user".to_string(),
                name: Some("Persisted".to_string()),
                html_url: "https://github.com/persisted_user".to_string(),
                avatar_url: None,
                bio: None,
                company: None,
            }),
        );

        assert!(!client.is_cached("@persisted_user"));
        client.load_from_persistent(&persistent);
        assert!(client.is_cached("@persisted_user"));

        let info = client.get_owner_info("@persisted_user");
        assert!(matches!(info, Some(OwnerInfo::User(ref u)) if u.login == "persisted_user"));
    }

    #[test]
    fn test_export_to_persistent() {
        let client = GitHubClient::new();

        {
            let mut cache = client.cache.write().unwrap();
            cache.owners.insert(
                "@export_user".to_string(),
                OwnerInfo::User(UserInfo {
                    login: "export_user".to_string(),
                    name: None,
                    html_url: "https://github.com/export_user".to_string(),
                    avatar_url: None,
                    bio: None,
                    company: None,
                }),
            );
        }

        let exported = client.export_to_persistent();
        assert!(exported.owners.contains_key("@export_user"));
        assert!(exported.last_updated > 0); // touch() was called
    }

    #[test]
    fn test_get_cached_returns_none_for_uncached() {
        let client = GitHubClient::new();
        assert!(client.get_cached("@nonexistent").is_none());
    }

    #[test]
    fn test_get_cached_returns_false_for_invalid() {
        let client = GitHubClient::new();

        {
            let mut cache = client.cache.write().unwrap();
            cache
                .owners
                .insert("@invalid_owner".to_string(), OwnerInfo::Invalid);
        }

        assert_eq!(client.get_cached("@invalid_owner"), Some(false));
    }

    /// get_cached() is LOSSY: it collapses Unknown into Some(false), same as
    /// Invalid. Code that needs to distinguish Unknown from Invalid MUST use
    /// get_owner_info() instead. This test documents the lossy behavior so
    /// nobody accidentally relies on get_cached() for classification.
    #[test]
    fn test_get_cached_is_lossy_for_unknown() {
        let client = GitHubClient::new();

        {
            let mut cache = client.cache.write().unwrap();
            cache.owners.insert(
                "@org/unknown-team".to_string(),
                OwnerInfo::Unknown("test".into()),
            );
            cache
                .owners
                .insert("@org/invalid-team".to_string(), OwnerInfo::Invalid);
        }

        // Both return Some(false) — INDISTINGUISHABLE via get_cached!
        assert_eq!(client.get_cached("@org/unknown-team"), Some(false));
        assert_eq!(client.get_cached("@org/invalid-team"), Some(false));

        // get_owner_info() preserves the distinction
        assert!(matches!(
            client.get_owner_info("@org/unknown-team"),
            Some(OwnerInfo::Unknown(_))
        ));
        assert!(matches!(
            client.get_owner_info("@org/invalid-team"),
            Some(OwnerInfo::Invalid)
        ));
    }

    #[test]
    fn test_owner_info_serialization() {
        // Test that OwnerInfo can be serialized/deserialized correctly
        let user = OwnerInfo::User(UserInfo {
            login: "test".to_string(),
            name: Some("Test User".to_string()),
            html_url: "https://github.com/test".to_string(),
            avatar_url: Some("https://avatar.url".to_string()),
            bio: Some("Bio".to_string()),
            company: Some("Company".to_string()),
        });

        let json = serde_json::to_string(&user).unwrap();
        let deserialized: OwnerInfo = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, OwnerInfo::User(ref u) if u.login == "test"));
    }

    #[test]
    fn test_team_info_serialization() {
        let team = OwnerInfo::Team(TeamInfo {
            slug: "my-team".to_string(),
            name: "My Team".to_string(),
            org: "my-org".to_string(),
            description: Some("Description".to_string()),
            html_url: "https://github.com/orgs/my-org/teams/my-team".to_string(),
            members_count: Some(10),
            repos_count: Some(5),
        });

        let json = serde_json::to_string(&team).unwrap();
        let deserialized: OwnerInfo = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, OwnerInfo::Team(ref t) if t.slug == "my-team"));
    }

    #[test]
    fn test_invalid_and_unknown_serialization() {
        let invalid = OwnerInfo::Invalid;
        let json = serde_json::to_string(&invalid).unwrap();
        let deserialized: OwnerInfo = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, OwnerInfo::Invalid));

        let unknown = OwnerInfo::Unknown("rate limit exceeded".into());
        let json = serde_json::to_string(&unknown).unwrap();
        let deserialized: OwnerInfo = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, OwnerInfo::Unknown(ref r) if r == "rate limit exceeded"));
    }

    #[test]
    fn test_persistent_cache_save_creates_gitignore_only_once() {
        let dir = tempdir().unwrap();
        let cache = PersistentCache::default();

        // First save creates .gitignore
        cache.save(dir.path()).unwrap();
        let gitignore_path = dir.path().join(".codeowners-lsp/.gitignore");
        assert!(gitignore_path.exists());

        // Modify the gitignore
        fs::write(&gitignore_path, "custom content\n").unwrap();

        // Second save should NOT overwrite existing .gitignore
        cache.save(dir.path()).unwrap();
        let content = fs::read_to_string(&gitignore_path).unwrap();
        assert_eq!(content, "custom content\n");
    }

    // =========================================================================
    // ASYNC TESTS WITH MOCK HTTP SERVER
    // =========================================================================

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_fetch_user_success() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/users/testuser"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "login": "testuser",
                "name": "Test User",
                "html_url": "https://github.com/testuser",
                "avatar_url": "https://avatars.githubusercontent.com/u/123",
                "bio": "A test user",
                "company": "Test Corp"
            })))
            .mount(&mock_server)
            .await;

        let client = GitHubClient::with_base_url(&mock_server.uri());
        let result = client.validate_owner("@testuser", "fake-token").await;

        assert_eq!(result, Some(true));
        // Should be cached now
        assert!(client.is_cached("@testuser"));
        let info = client.get_owner_info("@testuser").unwrap();
        assert!(matches!(info, OwnerInfo::User(ref u) if u.login == "testuser"));
    }

    #[tokio::test]
    async fn test_fetch_user_not_found() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/users/nonexistent"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let client = GitHubClient::with_base_url(&mock_server.uri());
        let result = client.validate_owner("@nonexistent", "fake-token").await;

        assert_eq!(result, Some(false));
        // Invalid should also be cached
        assert!(client.is_cached("@nonexistent"));
        let info = client.get_owner_info("@nonexistent").unwrap();
        assert!(matches!(info, OwnerInfo::Invalid));
    }

    #[tokio::test]
    async fn test_fetch_user_rate_limited() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/users/ratelimited"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;

        let client = GitHubClient::with_base_url(&mock_server.uri());
        let result = client.validate_owner("@ratelimited", "fake-token").await;

        // 403 = Unknown (rate limited or no permission)
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_fetch_team_success() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/orgs/myorg/teams/myteam"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "slug": "myteam",
                "name": "My Team",
                "description": "A test team",
                "html_url": "https://github.com/orgs/myorg/teams/myteam",
                "members_count": 10,
                "repos_count": 5
            })))
            .mount(&mock_server)
            .await;

        let client = GitHubClient::with_base_url(&mock_server.uri());
        let result = client.validate_owner("@myorg/myteam", "fake-token").await;

        assert_eq!(result, Some(true));
        assert!(client.is_cached("@myorg/myteam"));
        let info = client.get_owner_info("@myorg/myteam").unwrap();
        assert!(matches!(info, OwnerInfo::Team(ref t) if t.slug == "myteam"));
    }

    #[tokio::test]
    async fn test_fetch_team_not_found_is_unknown() {
        // Team 404 is ambiguous (could be invisible, not nonexistent),
        // so it should return Unknown (None), not Invalid (Some(false))
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/orgs/myorg/teams/nonexistent"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let client = GitHubClient::with_base_url(&mock_server.uri());
        let result = client
            .validate_owner("@myorg/nonexistent", "fake-token")
            .await;

        // 404 on teams = Unknown, not Invalid
        assert_eq!(result, None);

        // Should be cached as Unknown with reason
        let info = client.get_owner_info("@myorg/nonexistent").unwrap();
        assert!(matches!(info, OwnerInfo::Unknown(ref r) if r.contains("read:org")));
    }

    #[tokio::test]
    async fn test_caching_prevents_duplicate_requests() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/users/cacheduser"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "login": "cacheduser",
                "name": "Cached User",
                "html_url": "https://github.com/cacheduser"
            })))
            .expect(1) // Should only be called once!
            .mount(&mock_server)
            .await;

        let client = GitHubClient::with_base_url(&mock_server.uri());

        // First call
        let result1 = client.validate_owner("@cacheduser", "fake-token").await;
        assert_eq!(result1, Some(true));

        // Second call should use cache, NOT hit the server
        let result2 = client.validate_owner("@cacheduser", "fake-token").await;
        assert_eq!(result2, Some(true));

        // Third call too
        let result3 = client.validate_owner("@cacheduser", "fake-token").await;
        assert_eq!(result3, Some(true));
    }

    #[tokio::test]
    async fn test_validate_owner_email_not_validated() {
        let client = GitHubClient::new();

        // Emails can't be validated via GitHub API
        let result = client
            .validate_owner("user@example.com", "fake-token")
            .await;
        assert_eq!(result, None);

        // Should not be cached since we can't validate
        assert!(!client.is_cached("user@example.com"));
    }

    #[tokio::test]
    async fn test_validate_owner_malformed_team() {
        let client = GitHubClient::new();

        // Malformed team format (too many slashes)
        let result = client.validate_owner("@org/team/extra", "fake-token").await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_validate_owner_with_info_returns_full_metadata() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/users/fulluser"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "login": "fulluser",
                "name": "Full User",
                "html_url": "https://github.com/fulluser",
                "avatar_url": "https://avatars.githubusercontent.com/u/456",
                "bio": "Software engineer",
                "company": "Tech Inc"
            })))
            .mount(&mock_server)
            .await;

        let client = GitHubClient::with_base_url(&mock_server.uri());
        let info = client
            .validate_owner_with_info("@fulluser", "fake-token")
            .await
            .unwrap();

        if let OwnerInfo::User(user) = info {
            assert_eq!(user.login, "fulluser");
            assert_eq!(user.name, Some("Full User".to_string()));
            assert_eq!(user.bio, Some("Software engineer".to_string()));
            assert_eq!(user.company, Some("Tech Inc".to_string()));
        } else {
            panic!("Expected User info");
        }
    }

    #[tokio::test]
    async fn test_cache_cleared_properly() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/users/cleartest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "login": "cleartest",
                "html_url": "https://github.com/cleartest"
            })))
            .mount(&mock_server)
            .await;

        let client = GitHubClient::with_base_url(&mock_server.uri());

        // Populate cache
        client.validate_owner("@cleartest", "fake-token").await;
        assert!(client.is_cached("@cleartest"));

        // Clear cache
        client.clear_cache();
        assert!(!client.is_cached("@cleartest"));
    }

    #[tokio::test]
    async fn test_validate_user_helper() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/users/helperuser"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "login": "helperuser",
                "html_url": "https://github.com/helperuser"
            })))
            .mount(&mock_server)
            .await;

        let client = GitHubClient::with_base_url(&mock_server.uri());
        let result = client.validate_user("helperuser", "fake-token").await;

        assert_eq!(result, Some(true));
    }

    #[tokio::test]
    async fn test_validate_team_helper() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/orgs/helperorg/teams/helperteam"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "slug": "helperteam",
                "name": "Helper Team",
                "html_url": "https://github.com/orgs/helperorg/teams/helperteam"
            })))
            .mount(&mock_server)
            .await;

        let client = GitHubClient::with_base_url(&mock_server.uri());
        let result = client
            .validate_team("helperorg", "helperteam", "fake-token")
            .await;

        assert_eq!(result, Some(true));
    }

    #[tokio::test]
    async fn test_validate_user_not_found() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/users/notfounduser"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let client = GitHubClient::with_base_url(&mock_server.uri());
        let result = client.validate_user("notfounduser", "fake-token").await;

        assert_eq!(result, Some(false));
    }

    #[tokio::test]
    async fn test_validate_user_unknown() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/users/unknownuser"))
            .respond_with(ResponseTemplate::new(403)) // Rate limited
            .mount(&mock_server)
            .await;

        let client = GitHubClient::with_base_url(&mock_server.uri());
        let result = client.validate_user("unknownuser", "fake-token").await;

        assert_eq!(result, None); // Unknown returns None
    }

    #[tokio::test]
    async fn test_validate_team_not_found_is_unknown() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/orgs/someorg/teams/notfoundteam"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let client = GitHubClient::with_base_url(&mock_server.uri());
        let result = client
            .validate_team("someorg", "notfoundteam", "fake-token")
            .await;

        // Team 404 is ambiguous — returns None (Unknown)
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_validate_team_unknown() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/orgs/someorg/teams/privateteam"))
            .respond_with(ResponseTemplate::new(403)) // No permission
            .mount(&mock_server)
            .await;

        let client = GitHubClient::with_base_url(&mock_server.uri());
        let result = client
            .validate_team("someorg", "privateteam", "fake-token")
            .await;

        assert_eq!(result, None); // Unknown returns None
    }

    #[tokio::test]
    async fn test_fetch_team_rate_limited() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/orgs/ratelimitedorg/teams/someteam"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;

        let client = GitHubClient::with_base_url(&mock_server.uri());
        let result = client
            .validate_owner("@ratelimitedorg/someteam", "fake-token")
            .await;

        // 403 on team = Unknown
        assert_eq!(result, None);
    }
}
