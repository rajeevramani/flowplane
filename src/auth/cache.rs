//! In-memory TTL permission cache for Zitadel-authenticated users.
//!
//! Keyed by Zitadel `sub` claim. Entries expire after a configurable TTL
//! (default: 60 s, override with `FLOWPLANE_PERMISSION_CACHE_TTL_SECS`).

use std::collections::HashMap;
use std::collections::HashSet;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::auth::models::{AgentContext, Grant};
use crate::domain::{OrgId, UserId};

// ---------------------------------------------------------------------------
// Cache entry
// ---------------------------------------------------------------------------

/// A single cached permission record for one user.
pub struct CachedPermissions {
    pub org_scopes: HashSet<String>,
    pub grants: Vec<Grant>,
    pub user_id: UserId,
    pub email: Option<String>,
    pub org_id: Option<OrgId>,
    pub org_name: Option<String>,
    pub org_role: Option<String>,
    pub cached_at: Instant,
    pub agent_context: Option<AgentContext>,
}

/// Snapshot returned by [`PermissionCache::get`] — avoids a large tuple.
#[derive(Debug, Clone)]
pub struct CachedPermissionSnapshot {
    pub user_id: UserId,
    pub email: Option<String>,
    pub org_scopes: HashSet<String>,
    pub grants: Vec<Grant>,
    pub org_id: Option<OrgId>,
    pub org_name: Option<String>,
    pub org_role: Option<String>,
    pub agent_context: Option<AgentContext>,
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

/// Thread-safe, TTL-based permission cache keyed by Zitadel `sub`.
pub struct PermissionCache {
    entries: RwLock<HashMap<String, CachedPermissions>>,
    ttl: Duration,
}

impl PermissionCache {
    pub fn new(ttl: Duration) -> Self {
        Self { entries: RwLock::new(HashMap::new()), ttl }
    }

    /// Build from the `FLOWPLANE_PERMISSION_CACHE_TTL_SECS` env var (default 60).
    pub fn from_env() -> Self {
        let secs = std::env::var("FLOWPLANE_PERMISSION_CACHE_TTL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(60);
        Self::new(Duration::from_secs(secs))
    }

    /// Return a snapshot of the cached entry if it has not expired.
    pub async fn get(&self, sub: &str) -> Option<CachedPermissionSnapshot> {
        let guard = self.entries.read().await;
        let entry = guard.get(sub)?;
        if entry.cached_at.elapsed() >= self.ttl {
            return None;
        }
        Some(CachedPermissionSnapshot {
            user_id: entry.user_id.clone(),
            email: entry.email.clone(),
            org_scopes: entry.org_scopes.clone(),
            grants: entry.grants.clone(),
            org_id: entry.org_id.clone(),
            org_name: entry.org_name.clone(),
            org_role: entry.org_role.clone(),
            agent_context: entry.agent_context,
        })
    }

    /// Insert or replace the entry for `sub`.
    pub async fn insert(&self, sub: String, permissions: CachedPermissions) {
        self.entries.write().await.insert(sub, permissions);
    }

    /// Remove the entry for `sub`, if any.
    pub async fn evict(&self, sub: &str) {
        self.entries.write().await.remove(sub);
    }

    /// Remove all entries matching `user_id`.
    pub async fn evict_by_user_id(&self, user_id: &UserId) {
        self.entries.write().await.retain(|_, v| &v.user_id != user_id);
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_permissions(user_id: &str, org_scopes: &[&str]) -> CachedPermissions {
        CachedPermissions {
            org_scopes: org_scopes.iter().map(|s| s.to_string()).collect(),
            grants: Vec::new(),
            user_id: UserId::from_string(user_id.to_string()),
            email: Some(format!("{user_id}@example.com")),
            org_id: None,
            org_name: None,
            org_role: None,
            cached_at: Instant::now(),
            agent_context: None,
        }
    }

    #[tokio::test]
    async fn cache_hit_returns_entry() {
        let cache = PermissionCache::new(Duration::from_secs(60));
        cache.insert("sub-1".to_string(), make_permissions("user-1", &["admin:all"])).await;

        let result = cache.get("sub-1").await;
        assert!(result.is_some(), "expected cache hit");
        let snap = result.unwrap();
        assert_eq!(snap.user_id.as_str(), "user-1");
        assert_eq!(snap.email.as_deref(), Some("user-1@example.com"));
        assert!(snap.org_scopes.contains("admin:all"));
    }

    #[tokio::test]
    async fn cache_miss_returns_none_for_unknown_sub() {
        let cache = PermissionCache::new(Duration::from_secs(60));
        assert!(cache.get("unknown-sub").await.is_none());
    }

    #[tokio::test]
    async fn expired_entry_returns_none() {
        // TTL of 0 — entry expires immediately.
        let cache = PermissionCache::new(Duration::from_millis(0));
        cache.insert("sub-exp".to_string(), make_permissions("user-exp", &["admin:all"])).await;

        // Small sleep so elapsed() > 0 ms
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(cache.get("sub-exp").await.is_none(), "entry should have expired");
    }

    #[tokio::test]
    async fn evict_removes_entry() {
        let cache = PermissionCache::new(Duration::from_secs(60));
        cache.insert("sub-2".to_string(), make_permissions("user-2", &["clusters:read"])).await;
        assert!(cache.get("sub-2").await.is_some());

        cache.evict("sub-2").await;
        assert!(cache.get("sub-2").await.is_none(), "entry should be evicted");
    }

    #[tokio::test]
    async fn evict_by_user_id_removes_matching_entries() {
        let cache = PermissionCache::new(Duration::from_secs(60));
        cache.insert("sub-a".to_string(), make_permissions("user-target", &["a:b"])).await;
        cache.insert("sub-b".to_string(), make_permissions("user-target", &["c:d"])).await;
        cache.insert("sub-c".to_string(), make_permissions("user-other", &["e:f"])).await;

        let target_id = UserId::from_string("user-target".to_string());
        cache.evict_by_user_id(&target_id).await;

        assert!(cache.get("sub-a").await.is_none(), "sub-a should be evicted");
        assert!(cache.get("sub-b").await.is_none(), "sub-b should be evicted");
        assert!(cache.get("sub-c").await.is_some(), "sub-c (other user) should remain");
    }

    #[tokio::test]
    async fn insert_overwrites_existing_entry() {
        let cache = PermissionCache::new(Duration::from_secs(60));
        cache.insert("sub-1".to_string(), make_permissions("user-1", &["old:scope"])).await;
        cache.insert("sub-1".to_string(), make_permissions("user-1", &["new:scope"])).await;

        let snap = cache.get("sub-1").await.unwrap();
        assert!(snap.org_scopes.contains("new:scope"));
        assert!(!snap.org_scopes.contains("old:scope"));
    }
}
