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
        let cmd = self.cmd_template.replace("{email}", email);

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
}
