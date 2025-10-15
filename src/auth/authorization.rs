//! Authorization helpers for resource-level and team-scoped access control.
//!
//! This module provides functions to check permissions using scope patterns:
//! - `admin:all` - Bypass all permission checks
//! - `{resource}:{action}` - Resource-level permissions (e.g., `routes:read`)
//! - `team:{name}:{resource}:{action}` - Team-scoped permissions (e.g., `team:platform:routes:read`)

use crate::auth::models::{AuthContext, AuthError};

/// Admin bypass scope that grants access to all resources across all teams.
pub const ADMIN_ALL_SCOPE: &str = "admin:all";

/// Check if the context has admin bypass privileges.
///
/// Returns `true` if the token has the `admin:all` scope.
///
/// # Examples
///
/// ```rust
/// use flowplane::auth::authorization::has_admin_bypass;
/// use flowplane::auth::models::AuthContext;
/// use flowplane::domain::TokenId;
///
/// let admin_ctx = AuthContext::new(
///     TokenId::from_str_unchecked("token-1"),
///     "admin-token".into(),
///     vec!["admin:all".into()]
/// );
/// assert!(has_admin_bypass(&admin_ctx));
///
/// let normal_ctx = AuthContext::new(
///     TokenId::from_str_unchecked("token-2"),
///     "normal-token".into(),
///     vec!["routes:read".into()]
/// );
/// assert!(!has_admin_bypass(&normal_ctx));
/// ```
pub fn has_admin_bypass(context: &AuthContext) -> bool {
    context.has_scope(ADMIN_ALL_SCOPE)
}

/// Check if the context has access to perform an action on a resource.
///
/// This function checks for permissions in the following order:
/// 1. Admin bypass (`admin:all`)
/// 2. Resource-level permission (`{resource}:{action}`)
/// 3. Team-scoped permission (`team:{team}:{resource}:{action}`)
///
/// # Arguments
///
/// * `context` - The authentication context from the request
/// * `resource` - The resource type (e.g., "routes", "clusters")
/// * `action` - The action being performed (e.g., "read", "write", "delete")
/// * `team` - Optional team name for team-scoped resources
///
/// # Examples
///
/// ```rust
/// use flowplane::auth::authorization::check_resource_access;
/// use flowplane::auth::models::AuthContext;
/// use flowplane::domain::TokenId;
///
/// let ctx = AuthContext::new(
///     TokenId::from_str_unchecked("token-1"),
///     "platform-token".into(),
///     vec!["team:platform:routes:read".into(), "team:platform:routes:write".into()]
/// );
///
/// // Has access to platform team routes
/// assert!(check_resource_access(&ctx, "routes", "read", Some("platform")));
/// assert!(check_resource_access(&ctx, "routes", "write", Some("platform")));
///
/// // No access to engineering team routes
/// assert!(!check_resource_access(&ctx, "routes", "read", Some("engineering")));
///
/// // No delete permission
/// assert!(!check_resource_access(&ctx, "routes", "delete", Some("platform")));
/// ```
pub fn check_resource_access(
    context: &AuthContext,
    resource: &str,
    action: &str,
    team: Option<&str>,
) -> bool {
    // Check admin bypass first
    if has_admin_bypass(context) {
        return true;
    }

    // Check resource-level permission
    let resource_scope = format!("{}:{}", resource, action);
    if context.has_scope(&resource_scope) {
        return true;
    }

    // Check team-scoped permission if team is provided
    if let Some(team_name) = team {
        let team_scope = format!("team:{}:{}:{}", team_name, resource, action);
        if context.has_scope(&team_scope) {
            return true;
        }
    }

    false
}

/// Require resource access or return a 403 Forbidden error.
///
/// This is a convenience function that calls `check_resource_access` and
/// returns an error if access is denied.
///
/// # Arguments
///
/// * `context` - The authentication context from the request
/// * `resource` - The resource type (e.g., "routes", "clusters")
/// * `action` - The action being performed (e.g., "read", "write", "delete")
/// * `team` - Optional team name for team-scoped resources
///
/// # Errors
///
/// Returns `AuthError::Forbidden` if access is denied.
///
/// # Examples
///
/// ```rust
/// use flowplane::auth::authorization::require_resource_access;
/// use flowplane::auth::models::{AuthContext, AuthError};
/// use flowplane::domain::TokenId;
///
/// let ctx = AuthContext::new(
///     TokenId::from_str_unchecked("token-1"),
///     "demo".into(),
///     vec!["routes:read".into()]
/// );
///
/// // Success
/// require_resource_access(&ctx, "routes", "read", None).unwrap();
///
/// // Failure
/// let err = require_resource_access(&ctx, "routes", "write", None).unwrap_err();
/// assert!(matches!(err, AuthError::Forbidden));
/// ```
pub fn require_resource_access(
    context: &AuthContext,
    resource: &str,
    action: &str,
    team: Option<&str>,
) -> Result<(), AuthError> {
    if check_resource_access(context, resource, action, team) {
        Ok(())
    } else {
        Err(AuthError::Forbidden)
    }
}

/// Extract all team names from team-scoped permissions in the context.
///
/// Parses scopes matching the pattern `team:{name}:{resource}:{action}` and
/// returns a list of unique team names.
///
/// # Arguments
///
/// * `context` - The authentication context from the request
///
/// # Returns
///
/// A vector of unique team names found in the scopes.
///
/// # Examples
///
/// ```rust
/// use flowplane::auth::authorization::extract_team_scopes;
/// use flowplane::auth::models::AuthContext;
/// use flowplane::domain::TokenId;
///
/// let ctx = AuthContext::new(
///     TokenId::from_str_unchecked("token-1"),
///     "multi-team".into(),
///     vec![
///         "team:platform:routes:read".into(),
///         "team:platform:clusters:read".into(),
///         "team:engineering:routes:read".into(),
///     ]
/// );
///
/// let teams = extract_team_scopes(&ctx);
/// assert_eq!(teams.len(), 2);
/// assert!(teams.contains(&"platform".to_string()));
/// assert!(teams.contains(&"engineering".to_string()));
/// ```
pub fn extract_team_scopes(context: &AuthContext) -> Vec<String> {
    let mut teams = std::collections::HashSet::new();

    for scope in context.scopes() {
        if let Some(team_name) = parse_team_from_scope(scope) {
            teams.insert(team_name);
        }
    }

    teams.into_iter().collect()
}

/// Parse team name from a scope string if it matches the team pattern.
///
/// Expected pattern: `team:{name}:{resource}:{action}`
///
/// # Arguments
///
/// * `scope` - The scope string to parse
///
/// # Returns
///
/// `Some(team_name)` if the scope matches the team pattern, `None` otherwise.
///
/// # Examples
///
/// ```rust
/// use flowplane::auth::authorization::parse_team_from_scope;
///
/// assert_eq!(
///     parse_team_from_scope("team:platform:routes:read"),
///     Some("platform".to_string())
/// );
///
/// assert_eq!(
///     parse_team_from_scope("routes:read"),
///     None
/// );
///
/// assert_eq!(
///     parse_team_from_scope("team:incomplete"),
///     None
/// );
/// ```
pub fn parse_team_from_scope(scope: &str) -> Option<String> {
    let parts: Vec<&str> = scope.split(':').collect();

    // Pattern: team:{name}:{resource}:{action}
    if parts.len() == 4 && parts[0] == "team" {
        Some(parts[1].to_string())
    } else {
        None
    }
}

/// Check if the context has any team-scoped permissions.
///
/// Returns `true` if at least one scope matches the `team:{name}:{resource}:{action}` pattern.
///
/// # Arguments
///
/// * `context` - The authentication context from the request
///
/// # Examples
///
/// ```rust
/// use flowplane::auth::authorization::has_team_scopes;
/// use flowplane::auth::models::AuthContext;
/// use flowplane::domain::TokenId;
///
/// let team_ctx = AuthContext::new(
///     TokenId::from_str_unchecked("token-1"),
///     "team-token".into(),
///     vec!["team:platform:routes:read".into()]
/// );
/// assert!(has_team_scopes(&team_ctx));
///
/// let global_ctx = AuthContext::new(
///     TokenId::from_str_unchecked("token-2"),
///     "global-token".into(),
///     vec!["routes:read".into()]
/// );
/// assert!(!has_team_scopes(&global_ctx));
/// ```
pub fn has_team_scopes(context: &AuthContext) -> bool {
    context.scopes().any(|scope| parse_team_from_scope(scope).is_some())
}

/// Derive the required action from an HTTP method.
///
/// Maps HTTP methods to RBAC actions:
/// - GET → "read"
/// - POST → "write"
/// - PUT, PATCH, DELETE → "write"
///
/// Note: DELETE requires "write" permission to maintain backward compatibility
/// with existing scope configurations. A separate "delete" action may be
/// introduced in a future version for finer-grained control.
///
/// # Arguments
///
/// * `method` - The HTTP method string
///
/// # Returns
///
/// The corresponding action string.
///
/// # Examples
///
/// ```rust
/// use flowplane::auth::authorization::action_from_http_method;
///
/// assert_eq!(action_from_http_method("GET"), "read");
/// assert_eq!(action_from_http_method("POST"), "write");
/// assert_eq!(action_from_http_method("PUT"), "write");
/// assert_eq!(action_from_http_method("PATCH"), "write");
/// assert_eq!(action_from_http_method("DELETE"), "write");
/// assert_eq!(action_from_http_method("OPTIONS"), "read");
/// ```
pub fn action_from_http_method(method: &str) -> &'static str {
    match method.to_uppercase().as_str() {
        "GET" | "HEAD" | "OPTIONS" => "read",
        "POST" | "PUT" | "PATCH" | "DELETE" => "write",
        _ => "read", // Default to read for unknown methods
    }
}

/// Extract resource name from a URL path.
///
/// Parses paths like `/api/v1/routes/{id}` and extracts "routes".
///
/// # Arguments
///
/// * `path` - The URL path to parse
///
/// # Returns
///
/// `Some(resource)` if a resource can be extracted, `None` otherwise.
///
/// # Examples
///
/// ```rust
/// use flowplane::auth::authorization::resource_from_path;
///
/// assert_eq!(resource_from_path("/api/v1/routes"), Some("routes"));
/// assert_eq!(resource_from_path("/api/v1/routes/123"), Some("routes"));
/// assert_eq!(resource_from_path("/api/v1/clusters/my-cluster"), Some("clusters"));
/// assert_eq!(resource_from_path("/api/v1/listeners"), Some("listeners"));
/// assert_eq!(resource_from_path("/health"), None);
/// ```
pub fn resource_from_path(path: &str) -> Option<&str> {
    // Expected pattern: /api/v1/{resource} or /api/v1/{resource}/{id}
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    if parts.len() >= 3 && parts[0] == "api" && parts[1] == "v1" {
        Some(parts[2])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admin_context() -> AuthContext {
        AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("admin-token"),
            "admin".into(),
            vec!["admin:all".into()],
        )
    }

    fn platform_team_context() -> AuthContext {
        AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("platform-token"),
            "platform".into(),
            vec![
                "team:platform:routes:read".into(),
                "team:platform:routes:write".into(),
                "team:platform:clusters:read".into(),
            ],
        )
    }

    fn global_read_context() -> AuthContext {
        AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("read-token"),
            "readonly".into(),
            vec!["routes:read".into(), "clusters:read".into()],
        )
    }

    #[test]
    fn admin_bypass_grants_all_access() {
        let ctx = admin_context();
        assert!(has_admin_bypass(&ctx));
        assert!(check_resource_access(&ctx, "routes", "read", None));
        assert!(check_resource_access(&ctx, "routes", "write", Some("platform")));
        assert!(check_resource_access(&ctx, "clusters", "delete", Some("engineering")));
    }

    #[test]
    fn resource_level_scope_works() {
        let ctx = global_read_context();
        assert!(!has_admin_bypass(&ctx));
        assert!(check_resource_access(&ctx, "routes", "read", None));
        assert!(check_resource_access(&ctx, "clusters", "read", None));
        assert!(!check_resource_access(&ctx, "routes", "write", None));
        assert!(!check_resource_access(&ctx, "listeners", "read", None));
    }

    #[test]
    fn team_scoped_access_respects_team_boundaries() {
        let ctx = platform_team_context();
        assert!(has_team_scopes(&ctx));

        // Has access to platform team
        assert!(check_resource_access(&ctx, "routes", "read", Some("platform")));
        assert!(check_resource_access(&ctx, "routes", "write", Some("platform")));
        assert!(check_resource_access(&ctx, "clusters", "read", Some("platform")));

        // No access to engineering team
        assert!(!check_resource_access(&ctx, "routes", "read", Some("engineering")));
        assert!(!check_resource_access(&ctx, "clusters", "read", Some("engineering")));

        // No delete permission
        assert!(!check_resource_access(&ctx, "routes", "delete", Some("platform")));
    }

    #[test]
    fn extract_team_scopes_returns_unique_teams() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("multi-team"),
            "demo".into(),
            vec![
                "team:platform:routes:read".into(),
                "team:platform:clusters:read".into(),
                "team:engineering:routes:read".into(),
                "routes:read".into(), // Not a team scope
            ],
        );

        let teams = extract_team_scopes(&ctx);
        assert_eq!(teams.len(), 2);
        assert!(teams.contains(&"platform".to_string()));
        assert!(teams.contains(&"engineering".to_string()));
    }

    #[test]
    fn parse_team_from_scope_extracts_team_name() {
        assert_eq!(parse_team_from_scope("team:platform:routes:read"), Some("platform".into()));
        assert_eq!(
            parse_team_from_scope("team:engineering:clusters:write"),
            Some("engineering".into())
        );
        assert_eq!(parse_team_from_scope("routes:read"), None);
        assert_eq!(parse_team_from_scope("team:incomplete"), None);
        assert_eq!(parse_team_from_scope("team:too:many:parts:here"), None);
    }

    #[test]
    fn require_resource_access_returns_ok_when_allowed() {
        let ctx = global_read_context();
        assert!(require_resource_access(&ctx, "routes", "read", None).is_ok());
    }

    #[test]
    fn require_resource_access_returns_forbidden_when_denied() {
        let ctx = global_read_context();
        let err = require_resource_access(&ctx, "routes", "write", None).unwrap_err();
        assert!(matches!(err, AuthError::Forbidden));
    }

    #[test]
    fn action_from_http_method_maps_correctly() {
        assert_eq!(action_from_http_method("GET"), "read");
        assert_eq!(action_from_http_method("POST"), "write");
        assert_eq!(action_from_http_method("PUT"), "write");
        assert_eq!(action_from_http_method("PATCH"), "write");
        assert_eq!(action_from_http_method("DELETE"), "write");
        assert_eq!(action_from_http_method("HEAD"), "read");
        assert_eq!(action_from_http_method("OPTIONS"), "read");
        assert_eq!(action_from_http_method("UNKNOWN"), "read");
    }

    #[test]
    fn resource_from_path_extracts_resource_name() {
        assert_eq!(resource_from_path("/api/v1/routes"), Some("routes"));
        assert_eq!(resource_from_path("/api/v1/routes/123"), Some("routes"));
        assert_eq!(resource_from_path("/api/v1/clusters/my-cluster"), Some("clusters"));
        assert_eq!(resource_from_path("/api/v1/listeners"), Some("listeners"));
        assert_eq!(resource_from_path("/api/v1/api-definitions"), Some("api-definitions"));
        assert_eq!(resource_from_path("/api/v1/tokens/revoke"), Some("tokens"));
        assert_eq!(resource_from_path("/health"), None);
        assert_eq!(resource_from_path("/api/v2/routes"), None); // Wrong version
    }
}
