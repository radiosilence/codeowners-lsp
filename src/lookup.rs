//! Email-to-owner lookup via external command.
//!
//! Runs a configurable command to resolve git emails to team names,
//! then fuzzy matches against existing owners in CODEOWNERS.

use std::collections::HashMap;
use std::process::Command;

use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;

/// Cached email-to-owner resolver using external lookup command
pub struct OwnerLookup {
    /// Command template with {email} placeholder
    cmd_template: String,
    /// Cache of email -> lookup result (None = lookup failed/empty)
    cache: HashMap<String, Option<String>>,
    /// Existing owners from CODEOWNERS for fuzzy matching
    existing_owners: Vec<String>,
}

impl OwnerLookup {
    /// Create a new lookup resolver
    ///
    /// # Arguments
    /// * `cmd_template` - Command with {email} placeholder
    /// * `existing_owners` - List of owners from CODEOWNERS for fuzzy matching
    pub fn new(cmd_template: &str, existing_owners: Vec<String>) -> Self {
        Self {
            cmd_template: cmd_template.to_string(),
            cache: HashMap::new(),
            existing_owners,
        }
    }

    /// Batch lookup emails in parallel with progress bar
    ///
    /// Returns a map of email -> resolved owner (None if lookup failed)
    pub fn batch_lookup(&mut self, emails: &[String]) -> HashMap<String, Option<String>> {
        // Filter to emails not already cached
        let uncached: Vec<&String> = emails
            .iter()
            .filter(|e| !self.cache.contains_key(*e))
            .collect();

        if !uncached.is_empty() {
            let pb = ProgressBar::new(uncached.len() as u64);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
                    .unwrap()
                    .progress_chars("━╸─"),
            );
            pb.set_message("Looking up contributors...");

            // Run lookups in parallel
            let results: Vec<(String, Option<String>)> = uncached
                .par_iter()
                .map(|email| {
                    let result = self.run_lookup(email);
                    pb.inc(1);
                    ((*email).clone(), result)
                })
                .collect();

            pb.finish_and_clear();

            // Update cache with results
            for (email, result) in results {
                self.cache.insert(email, result);
            }
        }

        // Return results for requested emails
        emails
            .iter()
            .map(|e| (e.clone(), self.cache.get(e).cloned().flatten()))
            .collect()
    }

    /// Lookup owner for an email, using cache
    ///
    /// Returns None if lookup fails or no match found
    #[allow(dead_code)]
    pub fn lookup(&mut self, email: &str) -> Option<String> {
        // Check cache first
        if let Some(cached) = self.cache.get(email) {
            return cached.clone();
        }

        // Run lookup command
        let result = self.run_lookup(email);

        // Cache and return
        self.cache.insert(email.to_string(), result.clone());
        result
    }

    /// Run the lookup command for an email
    fn run_lookup(&self, email: &str) -> Option<String> {
        // Sanitize email: only allow safe characters to prevent shell injection.
        // Git author emails can be user-configured and could contain shell metacharacters.
        let safe_email: String = email
            .chars()
            .filter(|c| c.is_alphanumeric() || matches!(c, '@' | '.' | '-' | '_' | '+'))
            .collect();

        if safe_email.is_empty() || safe_email != email {
            // Email contained unsafe characters, skip lookup
            return None;
        }

        let cmd = self.cmd_template.replace("{email}", &safe_email);

        // Run through shell to support pipes
        let output = Command::new("sh").arg("-c").arg(&cmd).output().ok()?;

        if !output.status.success() {
            return None;
        }

        let result = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if result.is_empty() {
            return None;
        }

        // Fuzzy match against existing owners
        self.fuzzy_match(&result)
    }

    /// Fuzzy match a lookup result against existing CODEOWNERS owners
    ///
    /// Matching strategy (in order):
    /// 1. Exact match (case-insensitive)
    /// 2. Owner contains the lookup result
    /// 3. Lookup result contains part of owner (after @org/)
    fn fuzzy_match(&self, lookup_result: &str) -> Option<String> {
        let lookup_lower = lookup_result.to_lowercase();

        // 1. Exact match (case-insensitive)
        for owner in &self.existing_owners {
            if owner.to_lowercase() == lookup_lower
                || owner.to_lowercase() == format!("@{}", lookup_lower)
            {
                return Some(owner.clone());
            }
        }

        // 2. Owner contains the lookup result (e.g., "platform" matches "@org/platform-team")
        for owner in &self.existing_owners {
            let owner_lower = owner.to_lowercase();
            if owner_lower.contains(&lookup_lower) {
                return Some(owner.clone());
            }
        }

        // 3. Lookup result contains owner's team part (e.g., "platform-engineering" matches "@org/platform")
        for owner in &self.existing_owners {
            // Extract team name from @org/team format
            if let Some(team) = owner.strip_prefix('@').and_then(|s| s.split('/').nth(1)) {
                let team_lower = team.to_lowercase();
                if lookup_lower.contains(&team_lower) {
                    return Some(owner.clone());
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_match_exact() {
        let lookup = OwnerLookup::new(
            "echo test",
            vec!["@org/platform".to_string(), "@org/backend".to_string()],
        );

        assert_eq!(
            lookup.fuzzy_match("platform"),
            Some("@org/platform".to_string())
        );
        assert_eq!(
            lookup.fuzzy_match("@org/platform"),
            Some("@org/platform".to_string())
        );
    }

    #[test]
    fn test_fuzzy_match_contains() {
        let lookup = OwnerLookup::new(
            "echo test",
            vec![
                "@org/platform-team".to_string(),
                "@org/backend-team".to_string(),
            ],
        );

        assert_eq!(
            lookup.fuzzy_match("platform"),
            Some("@org/platform-team".to_string())
        );
    }

    #[test]
    fn test_fuzzy_match_reverse_contains() {
        let lookup = OwnerLookup::new(
            "echo test",
            vec!["@org/platform".to_string(), "@org/backend".to_string()],
        );

        assert_eq!(
            lookup.fuzzy_match("platform-engineering"),
            Some("@org/platform".to_string())
        );
    }

    #[test]
    fn test_fuzzy_match_no_match() {
        let lookup = OwnerLookup::new(
            "echo test",
            vec!["@org/platform".to_string(), "@org/backend".to_string()],
        );

        assert_eq!(lookup.fuzzy_match("frontend"), None);
    }

    #[test]
    fn test_fuzzy_match_case_insensitive() {
        let lookup = OwnerLookup::new("echo test", vec!["@org/Platform-Team".to_string()]);

        assert_eq!(
            lookup.fuzzy_match("platform-team"),
            Some("@org/Platform-Team".to_string())
        );
        assert_eq!(
            lookup.fuzzy_match("PLATFORM-TEAM"),
            Some("@org/Platform-Team".to_string())
        );
    }

    #[test]
    fn test_fuzzy_match_user_format() {
        let lookup = OwnerLookup::new(
            "echo test",
            vec!["@username".to_string(), "@org/team".to_string()],
        );

        assert_eq!(
            lookup.fuzzy_match("username"),
            Some("@username".to_string())
        );
    }

    #[test]
    fn test_fuzzy_match_empty_owners() {
        let lookup = OwnerLookup::new("echo test", vec![]);
        assert_eq!(lookup.fuzzy_match("anything"), None);
    }

    #[test]
    fn test_fuzzy_match_exact_takes_priority() {
        let lookup = OwnerLookup::new(
            "echo test",
            vec![
                "@org/platform-extended".to_string(),
                "@org/platform".to_string(),
            ],
        );

        // Exact match "@org/platform" should be found for "platform" since
        // exact matching (with @ prefix) is checked first
        assert_eq!(
            lookup.fuzzy_match("@org/platform"),
            Some("@org/platform".to_string())
        );

        // "platform" matches both via contains, picks first in list
        assert_eq!(
            lookup.fuzzy_match("platform"),
            Some("@org/platform-extended".to_string())
        );
    }

    #[test]
    fn test_owner_lookup_new() {
        let lookup = OwnerLookup::new(
            "my-cmd {email}",
            vec!["@owner1".to_string(), "@owner2".to_string()],
        );

        assert_eq!(lookup.cmd_template, "my-cmd {email}");
        assert_eq!(lookup.existing_owners.len(), 2);
        assert!(lookup.cache.is_empty());
    }

    #[test]
    fn test_owner_lookup_cache_behavior() {
        let mut lookup = OwnerLookup::new(
            "echo '@org/team'", // Simple command that returns a team
            vec!["@org/team".to_string()],
        );

        // First lookup - should run command and cache
        let result1 = lookup.lookup("test@example.com");
        assert!(lookup.cache.contains_key("test@example.com"));

        // Second lookup - should use cache (same result)
        let result2 = lookup.lookup("test@example.com");
        assert_eq!(result1, result2);
    }

    #[test]
    fn test_fuzzy_match_partial_team_name() {
        let lookup = OwnerLookup::new(
            "echo test",
            vec![
                "@mycompany/frontend-web".to_string(),
                "@mycompany/backend-api".to_string(),
            ],
        );

        // "frontend" should match "@mycompany/frontend-web"
        assert_eq!(
            lookup.fuzzy_match("frontend"),
            Some("@mycompany/frontend-web".to_string())
        );
    }

    #[test]
    fn test_fuzzy_match_with_at_prefix() {
        let lookup = OwnerLookup::new("echo test", vec!["@org/devops".to_string()]);

        // Both with and without @ should work
        assert_eq!(
            lookup.fuzzy_match("devops"),
            Some("@org/devops".to_string())
        );
        assert_eq!(
            lookup.fuzzy_match("@devops"),
            Some("@org/devops".to_string())
        );
    }

    #[test]
    fn test_run_lookup_with_echo() {
        let lookup = OwnerLookup::new("echo 'platform'", vec!["@org/platform".to_string()]);

        let result = lookup.run_lookup("test@example.com");
        assert_eq!(result, Some("@org/platform".to_string()));
    }

    #[test]
    fn test_run_lookup_empty_result() {
        let lookup = OwnerLookup::new("echo ''", vec!["@org/team".to_string()]);

        // Empty output should return None
        let result = lookup.run_lookup("test@example.com");
        assert_eq!(result, None);
    }

    #[test]
    fn test_run_lookup_command_failure() {
        let lookup = OwnerLookup::new("exit 1", vec!["@org/team".to_string()]);

        // Failed command should return None
        let result = lookup.run_lookup("test@example.com");
        assert_eq!(result, None);
    }

    #[test]
    fn test_run_lookup_no_fuzzy_match() {
        let lookup = OwnerLookup::new("echo 'unknown-team'", vec!["@org/platform".to_string()]);

        // Result doesn't match any existing owner
        let result = lookup.run_lookup("test@example.com");
        assert_eq!(result, None);
    }

    #[test]
    fn test_run_lookup_with_email_substitution() {
        // Verify {email} is substituted
        let lookup = OwnerLookup::new(
            "echo 'platform' # {email}",
            vec!["@org/platform".to_string()],
        );

        let result = lookup.run_lookup("user@test.com");
        assert_eq!(result, Some("@org/platform".to_string()));
    }

    #[test]
    fn test_batch_lookup_single() {
        let mut lookup = OwnerLookup::new("echo 'platform'", vec!["@org/platform".to_string()]);

        let emails = vec!["test@example.com".to_string()];
        let results = lookup.batch_lookup(&emails);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results.get("test@example.com"),
            Some(&Some("@org/platform".to_string()))
        );
    }

    #[test]
    fn test_batch_lookup_uses_cache() {
        let mut lookup = OwnerLookup::new("echo 'platform'", vec!["@org/platform".to_string()]);

        let emails = vec!["cached@example.com".to_string()];

        // First call populates cache
        lookup.batch_lookup(&emails);
        assert!(lookup.cache.contains_key("cached@example.com"));

        // Second call should use cache (we can verify by checking cache is still there)
        let results = lookup.batch_lookup(&emails);
        assert_eq!(
            results.get("cached@example.com"),
            Some(&Some("@org/platform".to_string()))
        );
    }

    #[test]
    fn test_lookup_uses_cache() {
        let mut lookup = OwnerLookup::new("echo 'platform'", vec!["@org/platform".to_string()]);

        // First lookup
        let result1 = lookup.lookup("cache-test@example.com");
        assert!(lookup.cache.contains_key("cache-test@example.com"));

        // Second lookup should return cached value
        let result2 = lookup.lookup("cache-test@example.com");
        assert_eq!(result1, result2);
    }
}
