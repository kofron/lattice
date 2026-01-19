//! auth::cache
//!
//! Authorization cache with TTL support.
//!
//! Per SPEC.md Section 8E.0.1, authorization results are cached for 10 minutes
//! at `<common_dir>/lattice/cache/github_auth.json`.
//!
//! # Design
//!
//! The cache is keyed by `host/owner/repo` (case-insensitive). Each entry
//! stores the installation_id, repository_id, and a timestamp for TTL
//! enforcement.
//!
//! Cache invalidation:
//! - Entries expire after 10 minutes (per SPEC.md)
//! - Entries are removed on 403/404 from API
//! - `prune_expired()` removes stale entries
//!
//! # Example
//!
//! ```ignore
//! use latticework::auth::cache::AuthCache;
//! use latticework::auth::installations::RepoAuthResult;
//! use latticework::core::paths::LatticePaths;
//!
//! let paths = LatticePaths::new(git_dir, common_dir);
//! let mut cache = AuthCache::load(&paths);
//!
//! // Check cache
//! if let Some(entry) = cache.get("github.com", "owner", "repo") {
//!     println!("Cached: installation={}", entry.installation_id);
//! } else {
//!     // Query API, then cache result
//!     cache.set("github.com", "owner", "repo", &result);
//!     cache.save(&paths);
//! }
//! ```

use super::installations::RepoAuthResult;
use crate::core::paths::LatticePaths;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Cache TTL in minutes (per SPEC.md: 10 minutes)
const CACHE_TTL_MINUTES: i64 = 10;

/// Cache entry for a repository authorization.
///
/// Stores the authorization result and timestamp for TTL enforcement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCacheEntry {
    /// GitHub App installation ID
    pub installation_id: u64,
    /// Repository ID within the installation
    pub repository_id: u64,
    /// When this entry was cached
    pub cached_at: DateTime<Utc>,
}

impl AuthCacheEntry {
    /// Check if this cache entry has expired.
    ///
    /// Entries expire after 10 minutes per SPEC.md Section 8E.0.1.
    pub fn is_expired(&self) -> bool {
        Utc::now() - self.cached_at > Duration::minutes(CACHE_TTL_MINUTES)
    }

    /// Create a new cache entry from an authorization result.
    pub fn from_result(result: &RepoAuthResult) -> Self {
        Self {
            installation_id: result.installation_id,
            repository_id: result.repository_id,
            cached_at: Utc::now(),
        }
    }

    /// Convert to a RepoAuthResult.
    pub fn to_result(&self) -> RepoAuthResult {
        RepoAuthResult {
            installation_id: self.installation_id,
            repository_id: self.repository_id,
        }
    }
}

/// Authorization cache stored at `<common_dir>/lattice/cache/github_auth.json`.
///
/// The cache uses a case-insensitive key format: `host/owner/repo`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AuthCache {
    /// Map of cache key to entry
    entries: HashMap<String, AuthCacheEntry>,
}

impl AuthCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load cache from disk.
    ///
    /// Returns an empty cache on any error (file not found, parse error, etc.).
    /// This is intentional - cache failures should not block operations.
    pub fn load(paths: &LatticePaths) -> Self {
        let path = Self::cache_path(paths);
        fs::read_to_string(&path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default()
    }

    /// Save cache to disk.
    ///
    /// This is best-effort - errors are ignored. Cache persistence failures
    /// should not block operations.
    pub fn save(&self, paths: &LatticePaths) {
        let path = Self::cache_path(paths);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(content) = serde_json::to_string_pretty(self) {
            let _ = fs::write(&path, content);
        }
    }

    /// Get cached authorization if present and not expired.
    ///
    /// Returns `None` if:
    /// - Entry doesn't exist
    /// - Entry has expired (older than 10 minutes)
    pub fn get(&self, host: &str, owner: &str, repo: &str) -> Option<&AuthCacheEntry> {
        let key = cache_key(host, owner, repo);
        self.entries.get(&key).filter(|e| !e.is_expired())
    }

    /// Store authorization result in cache.
    pub fn set(&mut self, host: &str, owner: &str, repo: &str, result: &RepoAuthResult) {
        let key = cache_key(host, owner, repo);
        self.entries
            .insert(key, AuthCacheEntry::from_result(result));
    }

    /// Remove entry from cache.
    ///
    /// Use this when API returns 403/404 to invalidate stale data.
    pub fn invalidate(&mut self, host: &str, owner: &str, repo: &str) {
        let key = cache_key(host, owner, repo);
        self.entries.remove(&key);
    }

    /// Remove all expired entries from the cache.
    pub fn prune_expired(&mut self) {
        self.entries.retain(|_, entry| !entry.is_expired());
    }

    /// Check if a specific entry exists (regardless of expiry).
    pub fn contains(&self, host: &str, owner: &str, repo: &str) -> bool {
        let key = cache_key(host, owner, repo);
        self.entries.contains_key(&key)
    }

    /// Get the number of entries in the cache.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get the cache file path.
    fn cache_path(paths: &LatticePaths) -> PathBuf {
        paths.repo_cache_dir().join("github_auth.json")
    }
}

/// Create a cache key from host, owner, and repo.
///
/// Keys are case-insensitive (lowercase).
fn cache_key(host: &str, owner: &str, repo: &str) -> String {
    format!(
        "{}/{}/{}",
        host.to_lowercase(),
        owner.to_lowercase(),
        repo.to_lowercase()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_paths() -> LatticePaths {
        LatticePaths::new(PathBuf::from("/repo/.git"), PathBuf::from("/repo/.git"))
    }

    mod cache_key_tests {
        use super::*;

        #[test]
        fn lowercase_normalization() {
            assert_eq!(
                cache_key("GitHub.com", "OWNER", "Repo"),
                "github.com/owner/repo"
            );
        }

        #[test]
        fn already_lowercase() {
            assert_eq!(
                cache_key("github.com", "owner", "repo"),
                "github.com/owner/repo"
            );
        }

        #[test]
        fn mixed_case() {
            assert_eq!(
                cache_key("GitHub.COM", "MyOrg", "My-Repo"),
                "github.com/myorg/my-repo"
            );
        }
    }

    mod auth_cache_entry_tests {
        use super::*;

        #[test]
        fn from_result() {
            let result = RepoAuthResult {
                installation_id: 123,
                repository_id: 456,
            };
            let entry = AuthCacheEntry::from_result(&result);

            assert_eq!(entry.installation_id, 123);
            assert_eq!(entry.repository_id, 456);
            // cached_at should be recent
            assert!(Utc::now() - entry.cached_at < Duration::seconds(1));
        }

        #[test]
        fn to_result() {
            let entry = AuthCacheEntry {
                installation_id: 123,
                repository_id: 456,
                cached_at: Utc::now(),
            };
            let result = entry.to_result();

            assert_eq!(result.installation_id, 123);
            assert_eq!(result.repository_id, 456);
        }

        #[test]
        fn is_expired_fresh() {
            let entry = AuthCacheEntry {
                installation_id: 1,
                repository_id: 1,
                cached_at: Utc::now(),
            };
            assert!(!entry.is_expired());
        }

        #[test]
        fn is_expired_old() {
            let entry = AuthCacheEntry {
                installation_id: 1,
                repository_id: 1,
                cached_at: Utc::now() - Duration::minutes(11),
            };
            assert!(entry.is_expired());
        }

        #[test]
        fn is_expired_at_boundary() {
            // Test just under 10 minutes - should NOT be expired
            let entry = AuthCacheEntry {
                installation_id: 1,
                repository_id: 1,
                cached_at: Utc::now() - Duration::minutes(9) - Duration::seconds(59),
            };
            assert!(!entry.is_expired());
        }

        #[test]
        fn is_expired_just_over() {
            let entry = AuthCacheEntry {
                installation_id: 1,
                repository_id: 1,
                cached_at: Utc::now() - Duration::minutes(10) - Duration::seconds(1),
            };
            assert!(entry.is_expired());
        }
    }

    mod auth_cache_tests {
        use super::*;

        #[test]
        fn new_is_empty() {
            let cache = AuthCache::new();
            assert!(cache.is_empty());
            assert_eq!(cache.len(), 0);
        }

        #[test]
        fn set_and_get() {
            let mut cache = AuthCache::new();
            let result = RepoAuthResult {
                installation_id: 123,
                repository_id: 456,
            };

            cache.set("github.com", "owner", "repo", &result);

            let entry = cache.get("github.com", "owner", "repo");
            assert!(entry.is_some());
            assert_eq!(entry.unwrap().installation_id, 123);
        }

        #[test]
        fn get_case_insensitive() {
            let mut cache = AuthCache::new();
            let result = RepoAuthResult {
                installation_id: 123,
                repository_id: 456,
            };

            cache.set("github.com", "owner", "repo", &result);

            // Different case should still match
            assert!(cache.get("GitHub.com", "OWNER", "REPO").is_some());
            assert!(cache.get("GITHUB.COM", "Owner", "Repo").is_some());
        }

        #[test]
        fn get_expired_returns_none() {
            let mut cache = AuthCache::new();

            // Insert an expired entry directly
            let key = cache_key("github.com", "owner", "repo");
            cache.entries.insert(
                key,
                AuthCacheEntry {
                    installation_id: 123,
                    repository_id: 456,
                    cached_at: Utc::now() - Duration::minutes(15),
                },
            );

            // Should return None because expired
            assert!(cache.get("github.com", "owner", "repo").is_none());
        }

        #[test]
        fn invalidate() {
            let mut cache = AuthCache::new();
            let result = RepoAuthResult {
                installation_id: 123,
                repository_id: 456,
            };

            cache.set("github.com", "owner", "repo", &result);
            assert!(cache.contains("github.com", "owner", "repo"));

            cache.invalidate("github.com", "owner", "repo");
            assert!(!cache.contains("github.com", "owner", "repo"));
        }

        #[test]
        fn invalidate_case_insensitive() {
            let mut cache = AuthCache::new();
            let result = RepoAuthResult {
                installation_id: 123,
                repository_id: 456,
            };

            cache.set("github.com", "owner", "repo", &result);
            cache.invalidate("GitHub.com", "OWNER", "REPO");

            assert!(!cache.contains("github.com", "owner", "repo"));
        }

        #[test]
        fn prune_expired() {
            let mut cache = AuthCache::new();

            // Add a fresh entry
            cache.entries.insert(
                cache_key("github.com", "fresh", "repo"),
                AuthCacheEntry {
                    installation_id: 1,
                    repository_id: 1,
                    cached_at: Utc::now(),
                },
            );

            // Add an expired entry
            cache.entries.insert(
                cache_key("github.com", "stale", "repo"),
                AuthCacheEntry {
                    installation_id: 2,
                    repository_id: 2,
                    cached_at: Utc::now() - Duration::minutes(15),
                },
            );

            assert_eq!(cache.len(), 2);

            cache.prune_expired();

            assert_eq!(cache.len(), 1);
            assert!(cache.contains("github.com", "fresh", "repo"));
            assert!(!cache.contains("github.com", "stale", "repo"));
        }

        #[test]
        fn contains() {
            let mut cache = AuthCache::new();
            let result = RepoAuthResult {
                installation_id: 123,
                repository_id: 456,
            };

            assert!(!cache.contains("github.com", "owner", "repo"));

            cache.set("github.com", "owner", "repo", &result);

            assert!(cache.contains("github.com", "owner", "repo"));
        }

        #[test]
        fn cache_path() {
            let paths = test_paths();
            let path = AuthCache::cache_path(&paths);

            assert_eq!(
                path,
                PathBuf::from("/repo/.git/lattice/cache/github_auth.json")
            );
        }

        #[test]
        fn serialization_roundtrip() {
            let mut cache = AuthCache::new();
            let result = RepoAuthResult {
                installation_id: 123,
                repository_id: 456,
            };
            cache.set("github.com", "owner", "repo", &result);

            // Serialize
            let json = serde_json::to_string(&cache).expect("serialize");

            // Deserialize
            let restored: AuthCache = serde_json::from_str(&json).expect("deserialize");

            assert_eq!(restored.len(), 1);
            let entry = restored.get("github.com", "owner", "repo");
            assert!(entry.is_some());
            assert_eq!(entry.unwrap().installation_id, 123);
        }
    }
}
