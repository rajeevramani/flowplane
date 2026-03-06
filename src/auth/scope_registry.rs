//! Scope Registry — code-only constants for scope validation.
//!
//! The `scopes` table has been dropped. All scope definitions live here as
//! compile-time constants. No database queries are needed for scope validation.

use crate::storage::repositories::ScopeDefinition;
use lazy_static::lazy_static;
use regex::Regex;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Valid grants constant
// ---------------------------------------------------------------------------

/// All valid (resource, actions) pairs for CP resource grants.
pub const VALID_GRANTS: &[(&str, &[&str])] = &[
    ("clusters", &["read", "create", "update", "delete"]),
    ("routes", &["read", "create", "update", "delete"]),
    ("listeners", &["read", "create", "update", "delete"]),
    ("filters", &["read", "create", "update", "delete"]),
    ("secrets", &["read", "create", "update", "delete"]),
    ("dataplanes", &["read", "create", "update", "delete"]),
    ("custom-wasm-filters", &["read", "create", "update", "delete"]),
    ("learning-sessions", &["read", "create", "execute", "delete"]),
    ("aggregated-schemas", &["read", "execute"]),
    ("proxy-certificates", &["read", "create", "delete"]),
    ("reports", &["read"]),
    ("audit", &["read"]),
    ("stats", &["read"]),
    ("agents", &["read", "create", "update", "delete"]),
];

lazy_static! {
    /// Regex for validating team name format
    /// NOTE: expect() acceptable - pattern is validated by tests
    static ref TEAM_NAME_REGEX: Regex = Regex::new(r"^[a-z0-9-]+$")
        .expect("BUG: TEAM_NAME_REGEX pattern is invalid - validated by tests");

    /// Regex for validating scope format (basic structure check)
    /// NOTE: expect() acceptable - pattern is validated by tests
    static ref SCOPE_FORMAT_REGEX: Regex = Regex::new(
        r"^(team:[a-z0-9-]+:[a-z0-9-]+:[a-z]+|team:[a-z0-9-]+:\*:\*|org:[a-z0-9-]+:(admin|member)|[a-z0-9-]+:[a-z]+)$"
    ).expect("BUG: SCOPE_FORMAT_REGEX pattern is invalid - validated by tests");
}

// ---------------------------------------------------------------------------
// ScopeRegistry — no longer DB-backed
// ---------------------------------------------------------------------------

/// Code-only scope registry. No pool, no async, no cache.
pub struct ScopeRegistry;

impl ScopeRegistry {
    /// Check if a scope is valid (synchronous, uses constants)
    pub fn is_valid_scope(&self, scope: &str) -> bool {
        // First check format
        if !is_valid_scope_format(scope) {
            return false;
        }

        // Handle team-scoped patterns
        if scope.starts_with("team:") {
            return self.is_valid_team_scope(scope);
        }

        // Handle org-scoped patterns: org:{name}:admin or org:{name}:member
        if scope.starts_with("org:") {
            return self.is_valid_org_scope(scope);
        }

        // Check resource:action against VALID_GRANTS
        let parts: Vec<&str> = scope.splitn(2, ':').collect();
        if parts.len() == 2 {
            return is_valid_resource_action_pair(parts[0], parts[1]);
        }

        // Special case: admin:all
        scope == "admin:all"
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

        // Handle wildcards
        if resource == "*" && action == "*" {
            // team:{name}:*:* is always valid for valid team names
            return true;
        }

        if action == "*" {
            // team:{name}:{resource}:* - check resource exists
            return VALID_GRANTS.iter().any(|(r, _)| *r == resource);
        }

        // Check specific resource:action combination
        is_valid_resource_action_pair(resource, action)
    }

    /// Check if an org-scoped pattern is valid
    fn is_valid_org_scope(&self, scope: &str) -> bool {
        let parts: Vec<&str> = scope.split(':').collect();

        // Org scopes must have exactly 3 parts: org:{name}:{role}
        if parts.len() != 3 || parts[0] != "org" {
            return false;
        }

        let org_name = parts[1];
        let role = parts[2];

        // Validate org name format (same rules as team names)
        if !TEAM_NAME_REGEX.is_match(org_name) {
            return false;
        }

        // Valid org roles are admin and member
        matches!(role, "admin" | "member")
    }

    /// Get all enabled scope definitions (for admin API).
    pub fn get_all_scopes(&self) -> Vec<ScopeDefinition> {
        build_scope_definitions(false)
    }

    /// Get UI-visible scope definitions (for public API).
    pub fn get_ui_scopes(&self) -> Vec<ScopeDefinition> {
        build_scope_definitions(true)
    }

    /// Get valid resources
    pub fn get_resources(&self) -> Vec<String> {
        VALID_GRANTS.iter().map(|(r, _)| r.to_string()).collect()
    }

    /// Validate a scope and return detailed error if invalid
    pub fn validate_scope(&self, scope: &str) -> std::result::Result<(), String> {
        if !is_valid_scope_format(scope) {
            return Err(format!(
                "Invalid scope format '{}'. Expected format: 'resource:action' or 'team:name:resource:action'",
                scope
            ));
        }

        if scope.starts_with("org:") {
            if self.is_valid_org_scope(scope) {
                return Ok(());
            }
            return Err(format!(
                "Invalid org scope '{}'. Expected format: 'org:{{name}}:admin' or 'org:{{name}}:member'",
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

                if resource != "*" && !VALID_GRANTS.iter().any(|(r, _)| *r == resource) {
                    return Err(format!(
                        "Unknown resource '{}' in scope '{}'. Valid resources: {:?}",
                        resource,
                        scope,
                        self.get_resources()
                    ));
                }

                if action != "*" {
                    if let Some((_, actions)) = VALID_GRANTS.iter().find(|(r, _)| *r == resource) {
                        if !actions.contains(&action) {
                            return Err(format!(
                                "Unknown action '{}' for resource '{}'. Valid actions: {:?}",
                                action, resource, actions
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
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Check if a (resource, action) pair is valid according to VALID_GRANTS.
pub fn is_valid_resource_action_pair(resource: &str, action: &str) -> bool {
    VALID_GRANTS.iter().any(|(r, actions)| *r == resource && actions.contains(&action))
}

/// Build ScopeDefinition Vec from VALID_GRANTS constants.
///
/// `ui_only` — if true, excludes governance-only scopes (admin:all etc.)
fn build_scope_definitions(ui_only: bool) -> Vec<ScopeDefinition> {
    use crate::domain::ScopeId;
    use chrono::Utc;

    let mut defs = Vec::new();

    for (resource, actions) in VALID_GRANTS {
        for action in *actions {
            let value = format!("{}:{}", resource, action);
            let label = format!("{} {}", capitalize_first(resource), capitalize_first(action));
            let category = capitalize_first(resource);

            defs.push(ScopeDefinition {
                id: ScopeId::from_string(format!("scope-{}-{}", resource, action)),
                value,
                resource: resource.to_string(),
                action: action.to_string(),
                label,
                description: None,
                category,
                visible_in_ui: true,
                enabled: true,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            });
        }
    }

    // Add admin:all (not UI-visible)
    if !ui_only {
        defs.push(ScopeDefinition {
            id: ScopeId::from_string("scope-admin-all".to_string()),
            value: "admin:all".to_string(),
            resource: "admin".to_string(),
            action: "all".to_string(),
            label: "Platform Admin".to_string(),
            description: Some("Full platform governance access".to_string()),
            category: "Admin".to_string(),
            visible_in_ui: false,
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        });
    }

    defs
}

/// Capitalize the first character of a string.
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

// ---------------------------------------------------------------------------
// Global registry
// ---------------------------------------------------------------------------

/// Global scope registry instance
static SCOPE_REGISTRY: OnceLock<std::sync::Arc<ScopeRegistry>> = OnceLock::new();

/// Initialize the global scope registry.
/// Call this once at application startup. No pool or async needed.
pub fn init_scope_registry() {
    SCOPE_REGISTRY.get_or_init(|| std::sync::Arc::new(ScopeRegistry));
}

/// Get the global scope registry.
///
/// # Panics
/// Panics if not initialized — call `init_scope_registry()` first.
pub fn get_scope_registry() -> &'static std::sync::Arc<ScopeRegistry> {
    SCOPE_REGISTRY
        .get()
        .expect("BUG: Scope registry not initialized - call init_scope_registry() first")
}

/// Check if scope registry is initialized
pub fn is_scope_registry_initialized() -> bool {
    SCOPE_REGISTRY.get().is_some()
}

/// Synchronous scope validation using the global registry.
/// Falls back to format-only validation if registry not initialized.
pub fn validate_scope_sync(scope: &str) -> bool {
    if let Some(registry) = SCOPE_REGISTRY.get() {
        registry.is_valid_scope(scope)
    } else {
        // Fallback to format validation only if registry not initialized
        is_valid_scope_format(scope)
    }
}

/// Check if a scope string has valid format.
fn is_valid_scope_format(scope: &str) -> bool {
    if !SCOPE_FORMAT_REGEX.is_match(scope) {
        return false;
    }
    // Reject two-part scopes starting with "org:" or "team:" (these need 3 or 4 parts)
    let parts: Vec<&str> = scope.split(':').collect();
    if parts.len() == 2 && (parts[0] == "org" || parts[0] == "team") {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_format_validation() {
        // Valid formats
        assert!(is_valid_scope_format("clusters:read"));
        assert!(is_valid_scope_format("admin:all"));
        assert!(is_valid_scope_format("custom-wasm-filters:read"));
        assert!(is_valid_scope_format("team:platform:routes:read"));
        assert!(is_valid_scope_format("team:eng-team:clusters:create"));
        assert!(is_valid_scope_format("team:team-test-1:clusters:read"));
        assert!(is_valid_scope_format("team:engineering:custom-wasm-filters:read"));
        assert!(is_valid_scope_format("team:platform:*:*"));

        // Org scope formats
        assert!(is_valid_scope_format("org:acme:admin"));
        assert!(is_valid_scope_format("org:acme:member"));
        assert!(is_valid_scope_format("org:my-org-1:admin"));

        // Invalid org formats
        assert!(!is_valid_scope_format("org:acme:viewer"));
        assert!(!is_valid_scope_format("org:ACME:admin"));
        assert!(!is_valid_scope_format("org:acme"));

        // Invalid formats
        assert!(!is_valid_scope_format("bad_scope"));
        assert!(!is_valid_scope_format("UPPERCASE:READ"));
        assert!(!is_valid_scope_format(""));
        assert!(!is_valid_scope_format("team:platform"));
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
        assert!(validate_scope_sync("clusters:read"));
        assert!(validate_scope_sync("team:platform:routes:read"));
        assert!(validate_scope_sync("org:acme:admin"));
        assert!(validate_scope_sync("org:acme:member"));
        assert!(!validate_scope_sync("org:acme:viewer"));
        assert!(!validate_scope_sync("invalid"));
    }

    #[test]
    fn test_valid_grants_all_pairs() {
        let registry = ScopeRegistry;

        // All VALID_GRANTS entries should be valid
        for (resource, actions) in VALID_GRANTS {
            for action in *actions {
                let scope = format!("{}:{}", resource, action);
                assert!(registry.is_valid_scope(&scope), "Expected valid scope: {}", scope);
            }
        }
    }

    #[test]
    fn test_is_valid_resource_action_pair() {
        assert!(is_valid_resource_action_pair("clusters", "read"));
        assert!(is_valid_resource_action_pair("clusters", "delete"));
        assert!(is_valid_resource_action_pair("learning-sessions", "execute"));
        assert!(!is_valid_resource_action_pair("clusters", "execute"));
        assert!(!is_valid_resource_action_pair("foo", "read"));
    }

    #[test]
    fn test_get_all_scopes_includes_admin() {
        let registry = ScopeRegistry;
        let all = registry.get_all_scopes();
        assert!(all.iter().any(|s| s.value == "admin:all"), "admin:all must be in get_all_scopes");
    }

    #[test]
    fn test_get_ui_scopes_excludes_admin() {
        let registry = ScopeRegistry;
        let ui = registry.get_ui_scopes();
        assert!(!ui.iter().any(|s| s.value == "admin:all"), "admin:all must not be in UI scopes");
        assert!(
            ui.iter().any(|s| s.value == "clusters:read"),
            "clusters:read must be in UI scopes"
        );
    }
}
