//! Scope Registry for database-driven scope validation
//!
//! This module provides a cached scope registry that:
//! - Loads scope definitions from the database
//! - Caches them for fast synchronous validation
//! - Provides both sync and async validation methods
//! - Supports team-scoped patterns with wildcards

use crate::errors::{FlowplaneError, Result};
use crate::storage::repositories::{ScopeDefinition, ScopeRepository, SqlxScopeRepository};
use crate::storage::DbPool;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

lazy_static! {
    /// Regex for validating team name format
    /// NOTE: expect() acceptable - pattern is validated by tests
    static ref TEAM_NAME_REGEX: Regex = Regex::new(r"^[a-z0-9-]+$")
        .expect("BUG: TEAM_NAME_REGEX pattern is invalid - validated by tests");

    /// Regex for validating scope format (basic structure check)
    /// NOTE: expect() acceptable - pattern is validated by tests
    static ref SCOPE_FORMAT_REGEX: Regex = Regex::new(
        r"^(team:[a-z0-9-]+:[a-z0-9-]+:[a-z]+|team:[a-z0-9-]+:\*:\*|[a-z0-9-]+:[a-z]+)$"
    ).expect("BUG: SCOPE_FORMAT_REGEX pattern is invalid - validated by tests");
}

/// Cached scope data for fast synchronous validation
#[derive(Debug, Clone, Default)]
struct ScopeCache {
    /// Set of valid scope values (e.g., "tokens:read")
    valid_scopes: HashSet<String>,
    /// Set of valid resources (e.g., "tokens", "clusters")
    valid_resources: HashSet<String>,
    /// Map of resource -> valid actions (e.g., "tokens" -> ["read", "write", "delete"])
    resource_actions: HashMap<String, HashSet<String>>,
    /// Full scope definitions for API responses
    definitions: Vec<ScopeDefinition>,
    /// UI-visible scope definitions
    ui_definitions: Vec<ScopeDefinition>,
}

/// Database-backed scope registry with caching
pub struct ScopeRegistry {
    pool: DbPool,
    cache: Arc<RwLock<ScopeCache>>,
}

impl ScopeRegistry {
    /// Create a new scope registry with database pool
    pub fn new(pool: DbPool) -> Self {
        Self { pool, cache: Arc::new(RwLock::new(ScopeCache::default())) }
    }

    /// Initialize the cache by loading scopes from database
    /// Call this at application startup
    pub async fn init(&self) -> Result<()> {
        self.refresh_cache().await
    }

    /// Refresh the cache from database
    pub async fn refresh_cache(&self) -> Result<()> {
        let repo = SqlxScopeRepository::new(self.pool.clone());

        let all_scopes = repo.find_all_enabled().await?;
        let ui_scopes = repo.find_ui_visible().await?;

        let mut valid_scopes = HashSet::new();
        let mut valid_resources = HashSet::new();
        let mut resource_actions: HashMap<String, HashSet<String>> = HashMap::new();

        for scope in &all_scopes {
            valid_scopes.insert(scope.value.clone());
            valid_resources.insert(scope.resource.clone());

            resource_actions
                .entry(scope.resource.clone())
                .or_default()
                .insert(scope.action.clone());
        }

        let mut cache = self
            .cache
            .write()
            .map_err(|_| FlowplaneError::sync("Scope registry cache lock poisoned"))?;
        cache.valid_scopes = valid_scopes;
        cache.valid_resources = valid_resources;
        cache.resource_actions = resource_actions;
        cache.definitions = all_scopes;
        cache.ui_definitions = ui_scopes;

        Ok(())
    }

    /// Check if a scope is valid (synchronous, uses cache)
    pub fn is_valid_scope(&self, scope: &str) -> bool {
        // First check format
        if !SCOPE_FORMAT_REGEX.is_match(scope) {
            return false;
        }

        // Handle team-scoped patterns
        if scope.starts_with("team:") {
            return self.is_valid_team_scope(scope);
        }

        // Check against cached scopes - fail closed on lock failure
        let cache = match self.cache.read() {
            Ok(guard) => guard,
            Err(_) => {
                tracing::error!("Scope registry cache lock poisoned - denying access");
                return false;
            }
        };
        cache.valid_scopes.contains(scope)
    }

    /// Check if a team-scoped pattern is valid
    fn is_valid_team_scope(&self, scope: &str) -> bool {
        let parts: Vec<&str> = scope.split(':').collect();

        // Team scopes must have exactly 4 parts: team:{name}:{resource}:{action}
        if parts.len() != 4 || parts[0] != "team" {
            return false;
        }

        let team_name = parts[1];
        let resource = parts[2];
        let action = parts[3];

        // Validate team name format
        if !TEAM_NAME_REGEX.is_match(team_name) {
            return false;
        }

        // Fail closed on lock failure
        let cache = match self.cache.read() {
            Ok(guard) => guard,
            Err(_) => {
                tracing::error!("Scope registry cache lock poisoned - denying access");
                return false;
            }
        };

        // Handle wildcards
        if resource == "*" && action == "*" {
            // team:{name}:*:* is always valid for valid team names
            return true;
        }

        if action == "*" {
            // team:{name}:{resource}:* - check resource exists
            return cache.valid_resources.contains(resource);
        }

        // Check specific resource:action combination
        if let Some(actions) = cache.resource_actions.get(resource) {
            return actions.contains(action);
        }

        false
    }

    /// Get all enabled scope definitions (for admin API)
    /// Returns empty Vec if cache is unavailable.
    pub fn get_all_scopes(&self) -> Vec<ScopeDefinition> {
        match self.cache.read() {
            Ok(cache) => cache.definitions.clone(),
            Err(_) => {
                tracing::error!("Scope registry cache lock poisoned - returning empty");
                Vec::new()
            }
        }
    }

    /// Get UI-visible scope definitions (for public API)
    /// Returns empty Vec if cache is unavailable.
    pub fn get_ui_scopes(&self) -> Vec<ScopeDefinition> {
        match self.cache.read() {
            Ok(cache) => cache.ui_definitions.clone(),
            Err(_) => {
                tracing::error!("Scope registry cache lock poisoned - returning empty");
                Vec::new()
            }
        }
    }

    /// Get valid resources
    /// Returns empty Vec if cache is unavailable.
    pub fn get_resources(&self) -> Vec<String> {
        match self.cache.read() {
            Ok(cache) => cache.valid_resources.iter().cloned().collect(),
            Err(_) => {
                tracing::error!("Scope registry cache lock poisoned - returning empty");
                Vec::new()
            }
        }
    }

    /// Validate a scope and return detailed error if invalid
    pub fn validate_scope(&self, scope: &str) -> std::result::Result<(), String> {
        if !SCOPE_FORMAT_REGEX.is_match(scope) {
            return Err(format!(
                "Invalid scope format '{}'. Expected format: 'resource:action' or 'team:name:resource:action'",
                scope
            ));
        }

        if scope.starts_with("team:") {
            if self.is_valid_team_scope(scope) {
                return Ok(());
            }

            let parts: Vec<&str> = scope.split(':').collect();
            if parts.len() == 4 {
                let resource = parts[2];
                let action = parts[3];

                let cache = self
                    .cache
                    .read()
                    .map_err(|_| "Scope registry unavailable - cache lock poisoned".to_string())?;
                if resource != "*" && !cache.valid_resources.contains(resource) {
                    return Err(format!(
                        "Unknown resource '{}' in scope '{}'. Valid resources: {:?}",
                        resource,
                        scope,
                        self.get_resources()
                    ));
                }

                if action != "*" {
                    if let Some(actions) = cache.resource_actions.get(resource) {
                        if !actions.contains(action) {
                            return Err(format!(
                                "Unknown action '{}' for resource '{}'. Valid actions: {:?}",
                                action,
                                resource,
                                actions.iter().collect::<Vec<_>>()
                            ));
                        }
                    }
                }
            }

            return Err(format!("Invalid team scope: {}", scope));
        }

        if self.is_valid_scope(scope) {
            return Ok(());
        }

        Err(format!("Unknown scope '{}'. Use GET /api/v1/scopes to see valid scopes.", scope))
    }

    /// Validate multiple scopes
    pub fn validate_scopes(&self, scopes: &[String]) -> std::result::Result<(), String> {
        for scope in scopes {
            self.validate_scope(scope)?;
        }
        Ok(())
    }

    /// Async validation that bypasses cache (for when freshest data is needed)
    pub async fn validate_scope_async(&self, scope: &str) -> Result<bool> {
        // First check format
        if !SCOPE_FORMAT_REGEX.is_match(scope) {
            return Ok(false);
        }

        // For team scopes, use cached validation (team names don't change scope validity)
        if scope.starts_with("team:") {
            return Ok(self.is_valid_team_scope(scope));
        }

        // For direct scopes, query database for freshest data
        let repo = SqlxScopeRepository::new(self.pool.clone());
        repo.is_valid_scope(scope).await
    }
}

/// Global scope registry instance
/// Must be initialized with `init_scope_registry()` before use
static SCOPE_REGISTRY: std::sync::OnceLock<Arc<ScopeRegistry>> = std::sync::OnceLock::new();

/// Initialize the global scope registry
/// Call this once at application startup after database connection is established
pub async fn init_scope_registry(pool: DbPool) -> Result<()> {
    let registry = ScopeRegistry::new(pool);
    registry.init().await?;

    SCOPE_REGISTRY
        .set(Arc::new(registry))
        .map_err(|_| crate::errors::FlowplaneError::config("Scope registry already initialized"))?;

    Ok(())
}

/// Get the global scope registry
///
/// # Panics
/// Panics if not initialized - call `init_scope_registry()` first.
/// This is a programming error, not a runtime failure.
pub fn get_scope_registry() -> &'static Arc<ScopeRegistry> {
    SCOPE_REGISTRY
        .get()
        .expect("BUG: Scope registry not initialized - call init_scope_registry() first")
}

/// Check if scope registry is initialized
pub fn is_scope_registry_initialized() -> bool {
    SCOPE_REGISTRY.get().is_some()
}

/// Synchronous scope validation using the global registry
/// Falls back to format-only validation if registry not initialized
pub fn validate_scope_sync(scope: &str) -> bool {
    if let Some(registry) = SCOPE_REGISTRY.get() {
        registry.is_valid_scope(scope)
    } else {
        // Fallback to format validation only if registry not initialized
        // This allows validation to work during tests without full setup
        SCOPE_FORMAT_REGEX.is_match(scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_format_regex() {
        // Valid formats
        assert!(SCOPE_FORMAT_REGEX.is_match("tokens:read"));
        assert!(SCOPE_FORMAT_REGEX.is_match("clusters:write"));
        assert!(SCOPE_FORMAT_REGEX.is_match("admin:all"));
        assert!(SCOPE_FORMAT_REGEX.is_match("api-definitions:read"));
        assert!(SCOPE_FORMAT_REGEX.is_match("custom-wasm-filters:read"));
        assert!(SCOPE_FORMAT_REGEX.is_match("team:platform:routes:read"));
        assert!(SCOPE_FORMAT_REGEX.is_match("team:eng-team:api-definitions:write"));
        assert!(SCOPE_FORMAT_REGEX.is_match("team:team-test-1:clusters:read"));
        assert!(SCOPE_FORMAT_REGEX.is_match("team:engineering:custom-wasm-filters:read"));
        assert!(SCOPE_FORMAT_REGEX.is_match("team:platform:*:*"));

        // Invalid formats
        assert!(!SCOPE_FORMAT_REGEX.is_match("bad_scope"));
        assert!(!SCOPE_FORMAT_REGEX.is_match("UPPERCASE:READ"));
        assert!(!SCOPE_FORMAT_REGEX.is_match("team:only-two"));
        assert!(!SCOPE_FORMAT_REGEX.is_match(""));
    }

    #[test]
    fn test_team_name_regex() {
        assert!(TEAM_NAME_REGEX.is_match("platform"));
        assert!(TEAM_NAME_REGEX.is_match("eng-team"));
        assert!(TEAM_NAME_REGEX.is_match("team123"));
        assert!(TEAM_NAME_REGEX.is_match("team-test-1"));

        assert!(!TEAM_NAME_REGEX.is_match(""));
        assert!(!TEAM_NAME_REGEX.is_match("UPPERCASE"));
        assert!(!TEAM_NAME_REGEX.is_match("has space"));
        assert!(!TEAM_NAME_REGEX.is_match("has_underscore"));
    }

    #[test]
    fn test_validate_scope_sync_fallback() {
        // When registry is not initialized, falls back to format validation
        assert!(validate_scope_sync("tokens:read"));
        assert!(validate_scope_sync("team:platform:routes:read"));
        assert!(!validate_scope_sync("invalid"));
    }
}
