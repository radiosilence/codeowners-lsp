use std::collections::HashMap;
use std::sync::RwLock;

/// Cache for GitHub owner validation results
#[derive(Default)]
pub struct GitHubCache {
    /// Map from owner string to validation result (true = valid, false = invalid)
    pub validated: HashMap<String, bool>,
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
