//! Stats caching layer with TTL-based expiration
//!
//! This module provides a simple in-memory cache for stats snapshots,
//! reducing load on Envoy admin endpoints and improving response times.

use crate::domain::StatsSnapshot;
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Configuration for the stats cache
#[derive(Debug, Clone)]
pub struct StatsCacheConfig {
    /// Time-to-live for cached entries
    pub ttl: Duration,
    /// Maximum number of entries to cache
    pub max_entries: usize,
}

impl Default for StatsCacheConfig {
    fn default() -> Self {
        Self { ttl: Duration::from_secs(10), max_entries: 100 }
    }
}

/// A cached stats entry with expiration time
#[derive(Debug, Clone)]
struct CacheEntry {
    snapshot: StatsSnapshot,
    expires_at: Instant,
}

impl CacheEntry {
    fn new(snapshot: StatsSnapshot, ttl: Duration) -> Self {
        Self { snapshot, expires_at: Instant::now() + ttl }
    }

    fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

/// Thread-safe in-memory cache for stats snapshots
pub struct StatsCache {
    entries: RwLock<HashMap<String, CacheEntry>>,
    config: StatsCacheConfig,
}

impl StatsCache {
    /// Create a new cache with the given configuration
    pub fn new(config: StatsCacheConfig) -> Self {
        Self { entries: RwLock::new(HashMap::new()), config }
    }

    /// Create a cache with default configuration
    pub fn with_defaults() -> Self {
        Self::new(StatsCacheConfig::default())
    }

    /// Get a cached snapshot for the given team, if present and not expired
    pub fn get(&self, team_id: &str) -> Option<StatsSnapshot> {
        let entries = self.entries.read().ok()?;
        let entry = entries.get(team_id)?;

        if entry.is_expired() {
            None
        } else {
            Some(entry.snapshot.clone())
        }
    }

    /// Store a snapshot in the cache
    pub fn set(&self, team_id: &str, snapshot: StatsSnapshot) {
        if let Ok(mut entries) = self.entries.write() {
            // Evict expired entries if we're at capacity
            if entries.len() >= self.config.max_entries {
                self.evict_expired(&mut entries);
            }

            // If still at capacity, remove oldest entry
            if entries.len() >= self.config.max_entries {
                if let Some(oldest_key) =
                    entries.iter().min_by_key(|(_, v)| v.expires_at).map(|(k, _)| k.clone())
                {
                    entries.remove(&oldest_key);
                }
            }

            entries.insert(team_id.to_string(), CacheEntry::new(snapshot, self.config.ttl));
        }
    }

    /// Remove a cached entry for the given team
    pub fn invalidate(&self, team_id: &str) {
        if let Ok(mut entries) = self.entries.write() {
            entries.remove(team_id);
        }
    }

    /// Clear all cached entries
    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.write() {
            entries.clear();
        }
    }

    /// Get the number of cached entries
    pub fn len(&self) -> usize {
        self.entries.read().map(|e| e.len()).unwrap_or(0)
    }

    /// Check if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Remove all expired entries
    fn evict_expired(&self, entries: &mut HashMap<String, CacheEntry>) {
        entries.retain(|_, v| !v.is_expired());
    }

    /// Perform maintenance on the cache (remove expired entries)
    pub fn cleanup(&self) {
        if let Ok(mut entries) = self.entries.write() {
            self.evict_expired(&mut entries);
        }
    }
}

impl Default for StatsCache {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    fn create_test_snapshot(team_id: &str) -> StatsSnapshot {
        StatsSnapshot::new(team_id.to_string())
    }

    #[test]
    fn test_cache_get_set() {
        let cache = StatsCache::with_defaults();

        let snapshot = create_test_snapshot("team-1");
        cache.set("team-1", snapshot.clone());

        let cached = cache.get("team-1");
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().team_id, "team-1");
    }

    #[test]
    fn test_cache_miss() {
        let cache = StatsCache::with_defaults();

        let cached = cache.get("nonexistent");
        assert!(cached.is_none());
    }

    #[test]
    fn test_cache_expiration() {
        let config = StatsCacheConfig { ttl: Duration::from_millis(50), max_entries: 100 };
        let cache = StatsCache::new(config);

        let snapshot = create_test_snapshot("team-1");
        cache.set("team-1", snapshot);

        // Should be present immediately
        assert!(cache.get("team-1").is_some());

        // Wait for expiration
        sleep(Duration::from_millis(60));

        // Should be expired
        assert!(cache.get("team-1").is_none());
    }

    #[test]
    fn test_cache_invalidate() {
        let cache = StatsCache::with_defaults();

        let snapshot = create_test_snapshot("team-1");
        cache.set("team-1", snapshot);

        assert!(cache.get("team-1").is_some());

        cache.invalidate("team-1");

        assert!(cache.get("team-1").is_none());
    }

    #[test]
    fn test_cache_clear() {
        let cache = StatsCache::with_defaults();

        cache.set("team-1", create_test_snapshot("team-1"));
        cache.set("team-2", create_test_snapshot("team-2"));

        assert_eq!(cache.len(), 2);

        cache.clear();

        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_max_entries() {
        let config = StatsCacheConfig { ttl: Duration::from_secs(60), max_entries: 2 };
        let cache = StatsCache::new(config);

        cache.set("team-1", create_test_snapshot("team-1"));
        cache.set("team-2", create_test_snapshot("team-2"));
        cache.set("team-3", create_test_snapshot("team-3"));

        // Should have evicted oldest entry
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_cache_update_existing() {
        let cache = StatsCache::with_defaults();

        let snapshot1 = create_test_snapshot("team-1");
        cache.set("team-1", snapshot1);

        let mut snapshot2 = create_test_snapshot("team-1");
        snapshot2.server.uptime_seconds = 12345;
        cache.set("team-1", snapshot2);

        let cached = cache.get("team-1").unwrap();
        assert_eq!(cached.server.uptime_seconds, 12345);
    }

    #[test]
    fn test_cache_cleanup() {
        let config = StatsCacheConfig { ttl: Duration::from_millis(50), max_entries: 100 };
        let cache = StatsCache::new(config);

        cache.set("team-1", create_test_snapshot("team-1"));
        cache.set("team-2", create_test_snapshot("team-2"));

        assert_eq!(cache.len(), 2);

        // Wait for expiration
        sleep(Duration::from_millis(60));

        cache.cleanup();

        assert_eq!(cache.len(), 0);
    }
}
