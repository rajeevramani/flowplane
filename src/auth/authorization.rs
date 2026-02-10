//! Authorization helpers for resource-level and team-scoped access control.
//!
//! This module provides functions to check permissions using scope patterns:
//! - `admin:all` - Bypass all permission checks
//! - `{resource}:{action}` - Resource-level permissions (e.g., `routes:read`)
//! - `team:{name}:{resource}:{action}` - Team-scoped permissions (e.g., `team:platform:routes:read`)

use crate::api::error::ApiError;
use crate::auth::models::{AuthContext, AuthError};
use crate::domain::OrgId;

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

    // Check resource-level permission (exact match)
    // SECURITY: Global (non-team-prefixed) resource scopes like "clusters:read" grant
    // access across ALL teams. Only platform admins should use these. Non-admin users
    // must use team-prefixed scopes like "team:engineering:clusters:read".
    let resource_scope = format!("{}:{}", resource, action);
    if context.has_scope(&resource_scope) && has_admin_bypass(context) {
        return true;
    }

    // Check team-scoped permission if team is provided
    if let Some(team_name) = team {
        // Check exact match first
        let team_scope = format!("team:{}:{}:{}", team_name, resource, action);
        if context.has_scope(&team_scope) {
            return true;
        }

        // Check wildcard patterns: team:{team}:*:* or team:{team}:{resource}:*
        let team_wildcard_all = format!("team:{}:*:*", team_name);
        if context.has_scope(&team_wildcard_all) {
            return true;
        }

        let team_wildcard_action = format!("team:{}:{}:*", team_name, resource);
        if context.has_scope(&team_wildcard_action) {
            return true;
        }

        // Org admins have implicit access to all teams in their org.
        // Defense-in-depth: resolve_team_name (team_access.rs) already validates
        // the team belongs to the caller's org via org-scoped SQL. This check
        // verifies the org admin scope matches the user's actual org_name,
        // preventing a corrupted scope from granting cross-org access.
        let org_scopes = extract_org_scopes(context);
        for (org_name, role) in &org_scopes {
            if role == "admin" {
                if let Some(user_org) = &context.org_name {
                    if org_name == user_org {
                        return true;
                    }
                    tracing::warn!(
                        scope_org = %org_name,
                        user_org = %user_org,
                        "org admin scope doesn't match user's org, denying implicit team access"
                    );
                } else {
                    // No org context on AuthContext (e.g. API token without org binding).
                    // Still grant access for backwards compatibility.
                    return true;
                }
            }
        }
    } else {
        // If no team specified, check if user has ANY team-scoped permission for this resource/action
        // This allows team-scoped users to call handlers that will filter by their teams
        for scope in context.scopes() {
            if let Some(team_name) = parse_team_from_scope(scope) {
                // Check exact match
                let expected_scope = format!("team:{}:{}:{}", team_name, resource, action);
                if *scope == expected_scope {
                    return true;
                }
            }

            // Check wildcard patterns
            if let Some(team_name) = parse_team_wildcard_scope(scope) {
                // User has team:X:*:* - grant access for this team
                let _ = team_name; // Unused but indicates wildcard access exists
                return true;
            }
        }

        // Check if user has any org-level membership (org admin/member get access;
        // handlers do fine-grained team filtering via get_effective_team_scopes_with_org)
        let org_scopes = extract_org_scopes(context);
        if !org_scopes.is_empty() {
            return true;
        }
    }

    false
}

/// Parse team name from a wildcard scope string.
///
/// Expected pattern: `team:{name}:*:*` or `team:{name}:{resource}:*`
///
/// # Arguments
///
/// * `scope` - The scope string to parse
///
/// # Returns
///
/// `Some(team_name)` if the scope is a wildcard team scope, `None` otherwise.
pub fn parse_team_wildcard_scope(scope: &str) -> Option<String> {
    let parts: Vec<&str> = scope.split(':').collect();

    // Pattern: team:{name}:*:* (full wildcard)
    if parts.len() == 4 && parts[0] == "team" && parts[2] == "*" && parts[3] == "*" {
        return Some(parts[1].to_string());
    }

    // Pattern: team:{name}:{resource}:* (action wildcard)
    if parts.len() == 4 && parts[0] == "team" && parts[3] == "*" {
        return Some(parts[1].to_string());
    }

    None
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
/// // Team-scoped user can access their team
/// let ctx = AuthContext::new(
///     TokenId::from_str_unchecked("token-1"),
///     "demo".into(),
///     vec!["team:platform:routes:read".into()]
/// );
/// require_resource_access(&ctx, "routes", "read", Some("platform")).unwrap();
///
/// // But not other teams
/// let err = require_resource_access(&ctx, "routes", "read", Some("engineering")).unwrap_err();
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

/// Parse organization name from a scope string if it matches the org pattern.
///
/// Expected pattern: `org:{name}:admin` or `org:{name}:member`
///
/// # Arguments
///
/// * `scope` - The scope string to parse
///
/// # Returns
///
/// `Some((org_name, role))` if the scope matches the org pattern, `None` otherwise.
pub fn parse_org_from_scope(scope: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = scope.split(':').collect();

    // Pattern: org:{name}:{role}
    if parts.len() == 3 && parts[0] == "org" && (parts[2] == "admin" || parts[2] == "member") {
        Some((parts[1].to_string(), parts[2].to_string()))
    } else {
        None
    }
}

/// Extract all organization names and roles from org-scoped permissions in the context.
///
/// Parses scopes matching the pattern `org:{name}:admin|member` and
/// returns a list of (org_name, role) pairs.
pub fn extract_org_scopes(context: &AuthContext) -> Vec<(String, String)> {
    let mut orgs = Vec::new();

    for scope in context.scopes() {
        if let Some(pair) = parse_org_from_scope(scope) {
            orgs.push(pair);
        }
    }

    orgs
}

/// Check if the context has org admin privileges for a specific org.
pub fn has_org_admin(context: &AuthContext, org_name: &str) -> bool {
    if has_admin_bypass(context) {
        return true;
    }

    let expected = format!("org:{}:admin", org_name);
    context.has_scope(&expected)
}

/// Require org admin access or return a 403 Forbidden error.
pub fn require_org_admin(context: &AuthContext, org_name: &str) -> Result<(), AuthError> {
    if has_org_admin(context, org_name) {
        Ok(())
    } else {
        Err(AuthError::Forbidden)
    }
}

/// Check if the context has any org membership (admin or member) for a specific org.
pub fn has_org_membership(context: &AuthContext, org_name: &str) -> bool {
    if has_admin_bypass(context) {
        return true;
    }

    let admin_scope = format!("org:{}:admin", org_name);
    let member_scope = format!("org:{}:member", org_name);
    context.has_scope(&admin_scope) || context.has_scope(&member_scope)
}

/// Check if a scope is a global (non-team-prefixed) resource scope.
///
/// Global resource scopes have the pattern `{resource}:{action}` (e.g., `clusters:read`).
/// These grant access across ALL teams and should only be held by platform admins.
/// Non-admin users should use team-prefixed scopes like `team:X:clusters:read`.
///
/// Returns `false` for:
/// - `admin:all` (admin bypass, not a resource scope)
/// - `team:X:resource:action` (team-prefixed, safe)
/// - `org:X:role` (org scope, handled separately)
///
/// Returns `true` for:
/// - `clusters:read`, `routes:write`, `listeners:read` etc.
pub fn is_global_resource_scope(scope: &str) -> bool {
    let parts: Vec<&str> = scope.split(':').collect();

    // Pattern: {resource}:{action} — exactly 2 parts
    // Exclude admin:all (admin bypass), team:* (team-prefixed), org:* (org scope)
    parts.len() == 2 && parts[0] != "admin" && parts[0] != "team" && parts[0] != "org"
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

/// Determine the semantic action from both HTTP method and request path.
///
/// Some POST endpoints are semantically read operations (they use POST to send
/// request bodies, not to modify data). This function identifies such endpoints
/// by analyzing the path and returns the correct semantic action.
///
/// # Semantic Read Operations
///
/// The following patterns are treated as "read" regardless of HTTP method:
/// - Paths ending in `/export` (e.g., `/api/v1/aggregated-schemas/export`)
/// - Paths ending in `/compare` (e.g., `/api/v1/schemas/compare`)
/// - Paths containing `/search` (e.g., `/api/v1/resources/search`)
/// - Paths containing `/query` (e.g., `/api/v1/data/query`)
///
/// # Arguments
///
/// * `method` - The HTTP method string
/// * `path` - The request path
///
/// # Returns
///
/// The semantic action string ("read" or "write").
///
/// # Examples
///
/// ```rust
/// use flowplane::auth::authorization::action_from_request;
///
/// // Regular GET → read
/// assert_eq!(action_from_request("GET", "/api/v1/routes"), "read");
///
/// // Regular POST → write
/// assert_eq!(action_from_request("POST", "/api/v1/routes"), "write");
///
/// // Export endpoint uses POST but is semantically read
/// assert_eq!(action_from_request("POST", "/api/v1/aggregated-schemas/export"), "read");
///
/// // Compare endpoint
/// assert_eq!(action_from_request("POST", "/api/v1/schemas/compare"), "read");
///
/// // Search endpoint
/// assert_eq!(action_from_request("POST", "/api/v1/resources/search"), "read");
///
/// // Query endpoint
/// assert_eq!(action_from_request("POST", "/api/v1/data/query"), "read");
/// ```
pub fn action_from_request(method: &str, path: &str) -> &'static str {
    // Check if path indicates a semantic read operation
    // Export and compare must be at the end of the path
    if path.ends_with("/export") || path.ends_with("/compare") {
        return "read";
    }

    // Search and query must be a complete path segment (preceded by /)
    // This prevents false positives like "search-configs" or "query-builder"
    // Split path into segments and check if any segment is exactly "search" or "query"
    let segments: Vec<&str> = path.split('/').collect();
    for segment in segments {
        if segment == "search" || segment == "query" {
            return "read";
        }
    }

    // Otherwise, delegate to HTTP method-based detection
    action_from_http_method(method)
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
/// assert_eq!(resource_from_path("/api/v1/route-configs"), Some("routes"));
/// assert_eq!(resource_from_path("/api/v1/route-configs/123"), Some("routes"));
/// assert_eq!(resource_from_path("/api/v1/clusters/my-cluster"), Some("clusters"));
/// assert_eq!(resource_from_path("/api/v1/listeners"), Some("listeners"));
/// assert_eq!(resource_from_path("/health"), None);
/// ```
pub fn resource_from_path(path: &str) -> Option<&str> {
    // Expected pattern: /api/v1/{resource} or /api/v1/{resource}/{id}
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    if parts.len() >= 3 && parts[0] == "api" && parts[1] == "v1" {
        // Special case: /api/v1/teams (list teams) should be accessible to all authenticated users
        // This endpoint returns different results based on admin status but doesn't require specific scopes
        if parts[2] == "teams" && parts.len() == 3 {
            return None;
        }

        // Special case: /api/v1/teams/{team}/bootstrap uses generate-envoy-config resource
        // This allows tokens with generate-envoy-config:read to access team bootstrap
        if parts[2] == "teams" && parts.len() >= 4 && parts.last() == Some(&"bootstrap") {
            return Some("generate-envoy-config");
        }

        // Special case: /api/v1/teams/{team}/{resource} - team-scoped resources
        // These paths have the pattern: ["api", "v1", "teams", "{team}", "{resource}"]
        // We need to extract the sub-resource (parts[4]) not "teams" (parts[2])
        // Examples:
        //   /api/v1/teams/engineering/secrets → "secrets"
        //   /api/v1/teams/engineering/proxy-certificates → "proxy-certificates"
        //   /api/v1/teams/engineering/custom-filters → "custom-wasm-filters"
        if parts[2] == "teams" && parts.len() >= 5 {
            let sub_resource = parts[4];

            // Handle resource name mappings for team-scoped resources
            // URL path uses "custom-filters" but scope uses "custom-wasm-filters"
            if sub_resource == "custom-filters" {
                return Some("custom-wasm-filters");
            }

            return Some(sub_resource);
        }

        // Special case: /api/v1/orgs - Org-scoped endpoints use handler-level authorization
        // via org membership scopes (org:{name}:admin|member), not resource-level scopes.
        // Returning None skips dynamic scope checks and lets the handler enforce access.
        if parts[2] == "orgs" {
            return None;
        }

        // Special case: /api/v1/admin/organizations - Admin org management endpoints
        // enforce admin:all or org:X:admin in handlers. Skip dynamic scope middleware.
        if parts[2] == "admin" && parts.len() >= 4 && parts[3] == "organizations" {
            return None;
        }

        // Special case: /api/v1/mcp - MCP endpoints implement method-level authorization
        // The HTTP method is always POST (JSON-RPC), but the actual operation is in the request body.
        // The MCP streamable HTTP handlers have their own comprehensive authorization based on the method field.
        if parts[2] == "mcp" {
            return None;
        }

        // Special case: /api/v1/openapi/* routes use "openapi-import" resource
        // The scope naming convention uses "openapi-import" (e.g., team:X:openapi-import:write)
        // but the URL structure is /api/v1/openapi/import, /api/v1/openapi/imports, etc.
        if parts[2] == "openapi" {
            return Some("openapi-import");
        }

        // Special case: /api/v1/route-configs/* uses "routes" resource
        // The API path follows Envoy terminology (RouteConfiguration) but the scope
        // remains "routes" for backwards compatibility and consistency
        if parts[2] == "route-configs" {
            return Some("routes");
        }

        // Special case: /api/v1/route-views/* uses "routes" resource
        // This endpoint provides flattened route views for UI consumption
        // but uses the same authorization scope as route-configs
        if parts[2] == "route-views" {
            return Some("routes");
        }

        // Special case: /api/v1/filter-types/* uses "filters" resource
        // Filter types are metadata about available filter schemas
        // and share the same authorization scope as filters management
        if parts[2] == "filter-types" {
            return Some("filters");
        }

        Some(parts[2])
    } else {
        None
    }
}

/// Verifies that the requesting user's org matches the target team's org.
///
/// Platform admins (with `admin:all` scope) bypass this check.
/// Returns 404 (not 403) for cross-org access to prevent enumeration.
///
/// # Arguments
///
/// * `context` - The authentication context from the request
/// * `team_org_id` - The org_id of the team that owns the resource
///
/// # Returns
///
/// * `Ok(())` if access is allowed
/// * `Err(ApiError::NotFound)` if the user's org doesn't match the team's org
pub fn verify_org_boundary(
    context: &AuthContext,
    team_org_id: &Option<OrgId>,
) -> Result<(), ApiError> {
    // Platform admins can access any org
    if has_admin_bypass(context) {
        return Ok(());
    }

    match (&context.org_id, team_org_id) {
        // User is in a different org than the team
        (Some(user_org), Some(team_org)) if user_org != team_org => {
            Err(ApiError::NotFound("Resource not found".to_string()))
        }
        // User has no org but team does -- deny
        (None, Some(_)) => Err(ApiError::NotFound("Resource not found".to_string())),
        // All other cases: org matches, team has no org (global), or both None
        _ => Ok(()),
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
    fn global_scope_without_admin_denied() {
        // SECURITY FIX: Non-admin users with global resource scopes should be denied.
        // Global scopes like "routes:read" are now restricted to platform admins only.
        let ctx = global_read_context();
        assert!(!has_admin_bypass(&ctx));
        assert!(!check_resource_access(&ctx, "routes", "read", None));
        assert!(!check_resource_access(&ctx, "clusters", "read", None));
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
    fn extract_team_scopes_returns_empty_for_global_scopes() {
        // This test verifies Bug 12 fix: users with only global scopes
        // (not team-scoped) should get empty team list, preventing them
        // from seeing all resources via the admin bypass logic
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("global-scopes-only"),
            "bug-12-user".into(),
            vec!["listeners:read".into(), "routes:read".into(), "clusters:read".into()],
        );

        let teams = extract_team_scopes(&ctx);
        assert_eq!(teams.len(), 0, "Users with global scopes should NOT bypass team isolation");
        assert!(!has_admin_bypass(&ctx), "Global scopes should not grant admin bypass");
    }

    #[test]
    fn extract_team_scopes_correctly_parses_team_scoped_permissions() {
        // This test verifies correct behavior: users with team-scoped permissions
        // get their team list extracted properly for resource filtering
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("team-scoped"),
            "correct-user".into(),
            vec![
                "team:engineering:listeners:read".into(),
                "team:engineering:routes:read".into(),
                "team:engineering:clusters:read".into(),
            ],
        );

        let teams = extract_team_scopes(&ctx);
        assert_eq!(teams.len(), 1);
        assert!(teams.contains(&"engineering".to_string()));
        assert!(!has_admin_bypass(&ctx));
    }

    #[test]
    fn require_resource_access_returns_ok_for_team_scoped_user() {
        let ctx = platform_team_context();
        assert!(require_resource_access(&ctx, "routes", "read", Some("platform")).is_ok());
    }

    #[test]
    fn require_resource_access_returns_forbidden_when_denied() {
        let ctx = global_read_context();
        // Non-admin with global scopes should be forbidden
        let err = require_resource_access(&ctx, "routes", "read", None).unwrap_err();
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
    fn action_from_request_handles_regular_methods() {
        // Regular GET request → read
        assert_eq!(action_from_request("GET", "/api/v1/routes"), "read");
        assert_eq!(action_from_request("GET", "/api/v1/clusters/123"), "read");

        // Regular POST request → write
        assert_eq!(action_from_request("POST", "/api/v1/routes"), "write");
        assert_eq!(action_from_request("POST", "/api/v1/clusters"), "write");

        // Regular PUT/PATCH/DELETE → write
        assert_eq!(action_from_request("PUT", "/api/v1/routes/123"), "write");
        assert_eq!(action_from_request("PATCH", "/api/v1/routes/123"), "write");
        assert_eq!(action_from_request("DELETE", "/api/v1/routes/123"), "write");
    }

    #[test]
    fn action_from_request_identifies_export_as_read() {
        // Export endpoints are semantic reads regardless of method
        assert_eq!(action_from_request("POST", "/api/v1/aggregated-schemas/export"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/schemas/export"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/data/export"), "read");

        // Even if someone uses PUT or other methods, still read
        assert_eq!(action_from_request("PUT", "/api/v1/data/export"), "read");
        assert_eq!(action_from_request("PATCH", "/api/v1/data/export"), "read");
    }

    #[test]
    fn action_from_request_identifies_compare_as_read() {
        // Compare endpoints are semantic reads
        assert_eq!(action_from_request("POST", "/api/v1/schemas/compare"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/aggregated-schemas/compare"), "read");
        assert_eq!(action_from_request("PUT", "/api/v1/data/compare"), "read");
    }

    #[test]
    fn action_from_request_identifies_search_as_read() {
        // Search endpoints are semantic reads
        assert_eq!(action_from_request("POST", "/api/v1/resources/search"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/routes/search"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/search"), "read");

        // Search in the middle of path
        assert_eq!(action_from_request("POST", "/api/v1/search/filters"), "read");
    }

    #[test]
    fn action_from_request_identifies_query_as_read() {
        // Query endpoints are semantic reads
        assert_eq!(action_from_request("POST", "/api/v1/data/query"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/query"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/query/advanced"), "read");

        // Query in the middle of path
        assert_eq!(action_from_request("POST", "/api/v1/logs/query/recent"), "read");
    }

    #[test]
    fn action_from_request_does_not_false_positive() {
        // Paths containing "search" or "query" as part of resource names should still work
        // but paths with these as actual operations should be read

        // These are normal write operations (resource creation/modification)
        assert_eq!(action_from_request("POST", "/api/v1/search-configs"), "write");
        assert_eq!(action_from_request("POST", "/api/v1/query-builder"), "write");

        // Export/compare must be at the END of the path
        assert_eq!(action_from_request("POST", "/api/v1/export-configs"), "write");
        assert_eq!(action_from_request("POST", "/api/v1/compare-tool"), "write");
    }

    #[test]
    fn action_from_request_case_insensitive_methods() {
        // HTTP methods should be case-insensitive
        assert_eq!(action_from_request("post", "/api/v1/routes"), "write");
        assert_eq!(action_from_request("get", "/api/v1/routes"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/schemas/export"), "read");
        assert_eq!(action_from_request("post", "/api/v1/schemas/export"), "read");
    }

    #[test]
    fn action_from_request_real_world_examples() {
        // Real-world bug fix: aggregated-schemas export
        assert_eq!(
            action_from_request("POST", "/api/v1/aggregated-schemas/export"),
            "read",
            "aggregated-schemas export should be read operation"
        );

        // Other potential export endpoints
        assert_eq!(
            action_from_request("POST", "/api/v1/routes/export"),
            "read",
            "routes export should be read operation"
        );
        assert_eq!(
            action_from_request("POST", "/api/v1/clusters/export"),
            "read",
            "clusters export should be read operation"
        );

        // Search endpoints
        assert_eq!(
            action_from_request("POST", "/api/v1/api-definitions/search"),
            "read",
            "api-definitions search should be read operation"
        );

        // Regular write operations should remain unchanged
        assert_eq!(
            action_from_request("POST", "/api/v1/aggregated-schemas"),
            "write",
            "creating aggregated-schema should be write operation"
        );
        assert_eq!(
            action_from_request("PUT", "/api/v1/aggregated-schemas/123"),
            "write",
            "updating aggregated-schema should be write operation"
        );
    }

    // === Organization scope tests ===

    #[test]
    fn parse_org_from_scope_extracts_org_and_role() {
        assert_eq!(parse_org_from_scope("org:acme:admin"), Some(("acme".into(), "admin".into())));
        assert_eq!(
            parse_org_from_scope("org:my-org:member"),
            Some(("my-org".into(), "member".into()))
        );
        assert_eq!(parse_org_from_scope("org:acme:viewer"), None);
        assert_eq!(parse_org_from_scope("team:acme:routes:read"), None);
        assert_eq!(parse_org_from_scope("routes:read"), None);
        assert_eq!(parse_org_from_scope("org:acme"), None);
    }

    #[test]
    fn extract_org_scopes_returns_org_memberships() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-user"),
            "org-user".into(),
            vec![
                "org:acme:admin".into(),
                "org:globex:member".into(),
                "team:platform:routes:read".into(),
                "routes:read".into(),
            ],
        );

        let orgs = extract_org_scopes(&ctx);
        assert_eq!(orgs.len(), 2);
        assert!(orgs.contains(&("acme".into(), "admin".into())));
        assert!(orgs.contains(&("globex".into(), "member".into())));
    }

    #[test]
    fn has_org_admin_checks_org_scope() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-admin"),
            "org-admin".into(),
            vec!["org:acme:admin".into(), "org:globex:member".into()],
        );

        assert!(has_org_admin(&ctx, "acme"));
        assert!(!has_org_admin(&ctx, "globex")); // member, not admin
        assert!(!has_org_admin(&ctx, "unknown"));
    }

    #[test]
    fn has_org_admin_respects_platform_admin() {
        let ctx = admin_context();
        assert!(has_org_admin(&ctx, "any-org"));
    }

    #[test]
    fn require_org_admin_returns_forbidden() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("member"),
            "member".into(),
            vec!["org:acme:member".into()],
        );

        assert!(require_org_admin(&ctx, "acme").is_err());
        assert!(require_org_admin(&ctx, "unknown").is_err());
    }

    #[test]
    fn has_org_membership_checks_admin_or_member() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-user"),
            "org-user".into(),
            vec!["org:acme:admin".into(), "org:globex:member".into()],
        );

        assert!(has_org_membership(&ctx, "acme"));
        assert!(has_org_membership(&ctx, "globex"));
        assert!(!has_org_membership(&ctx, "unknown"));
    }

    // === Org scope access in check_resource_access (team=None) ===

    #[test]
    fn org_member_passes_check_resource_access_without_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-member"),
            "org-member".into(),
            vec!["org:acme:member".into()],
        );

        // User with org:acme:member should pass when team=None
        // (handlers will do fine-grained team filtering)
        assert!(check_resource_access(&ctx, "routes", "read", None));
        assert!(check_resource_access(&ctx, "clusters", "read", None));
        assert!(check_resource_access(&ctx, "listeners", "write", None));
    }

    #[test]
    fn org_admin_passes_check_resource_access_without_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-admin"),
            "org-admin".into(),
            vec!["org:acme:admin".into()],
        );

        // User with org:acme:admin should pass when team=None
        assert!(check_resource_access(&ctx, "routes", "read", None));
        assert!(check_resource_access(&ctx, "clusters", "write", None));
    }

    #[test]
    fn org_admin_passes_check_resource_access_with_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-admin"),
            "org-admin".into(),
            vec!["org:acme:admin".into(), "team:acme-default:*:*".into()],
        );

        // Org admin should pass for any team (implicit access to all org teams)
        assert!(check_resource_access(&ctx, "routes", "read", Some("engineering")));
        assert!(check_resource_access(&ctx, "clusters", "write", Some("engineering")));
        assert!(check_resource_access(&ctx, "listeners", "read", Some("frontend")));
    }

    #[test]
    fn org_member_fails_check_resource_access_with_wrong_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-member"),
            "org-member".into(),
            vec!["org:acme:member".into(), "team:acme-default:routes:read".into()],
        );

        // Org member should NOT get implicit team access (only admins do)
        assert!(!check_resource_access(&ctx, "routes", "read", Some("engineering")));
        // But should have access to their own team
        assert!(check_resource_access(&ctx, "routes", "read", Some("acme-default")));
    }

    // === Defense-in-depth: org admin scope must match user's org_name ===

    #[test]
    fn org_admin_with_matching_org_passes() {
        let org_id = crate::domain::OrgId::new();
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-admin"),
            "org-admin".into(),
            vec!["org:acme:admin".into()],
        )
        .with_org(org_id, "acme".into());

        // Scope org matches context org_name → granted
        assert!(check_resource_access(&ctx, "routes", "read", Some("engineering")));
        assert!(check_resource_access(&ctx, "clusters", "write", Some("platform")));
    }

    #[test]
    fn org_admin_with_mismatched_org_denied() {
        let org_id = crate::domain::OrgId::new();
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-admin"),
            "org-admin".into(),
            vec!["org:acme:admin".into()],
        )
        .with_org(org_id, "globex".into());

        // Scope says "acme" but user's org is "globex" → denied (scope corruption defense)
        assert!(!check_resource_access(&ctx, "routes", "read", Some("engineering")));
        assert!(!check_resource_access(&ctx, "clusters", "write", Some("platform")));
    }

    #[test]
    fn org_admin_without_org_context_still_passes() {
        // Backwards compat: AuthContext without org_name (e.g. API token)
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-admin-no-ctx"),
            "org-admin".into(),
            vec!["org:acme:admin".into()],
        );

        // No org_name on context → allow (backward compatible)
        assert!(check_resource_access(&ctx, "routes", "read", Some("engineering")));
    }

    #[test]
    fn no_scopes_fails_check_resource_access() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("empty"),
            "no-scopes".into(),
            vec![],
        );

        // User with NO scopes should fail
        assert!(!check_resource_access(&ctx, "routes", "read", None));
        assert!(!check_resource_access(&ctx, "routes", "read", Some("platform")));
        assert!(!check_resource_access(&ctx, "clusters", "write", None));
    }

    // === Regression tests for admin-with-team-memberships bug ===
    // These tests ensure admin bypass works correctly even when the admin
    // also has team-scoped permissions from their team memberships.

    /// Test that admin with ONLY admin:all scope can access any team's resources
    #[test]
    fn admin_only_can_access_any_team_resource() {
        let ctx = admin_context(); // Only has admin:all

        // Can access any team
        assert!(check_resource_access(&ctx, "openapi-import", "write", Some("engineering")));
        assert!(check_resource_access(&ctx, "openapi-import", "read", Some("platform")));
        assert!(check_resource_access(&ctx, "openapi-import", "delete", Some("random-team")));

        // require_resource_access also works
        assert!(
            require_resource_access(&ctx, "openapi-import", "write", Some("engineering")).is_ok()
        );
    }

    /// Test that admin with admin:all AND team memberships can still access any team
    /// This was the root cause of the bug - admins with team memberships were being
    /// restricted to only their membership teams.
    #[test]
    fn admin_with_team_membership_can_access_other_teams() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("admin-with-membership"),
            "admin-with-teams".into(),
            vec![
                "admin:all".into(),
                "team:platform-admin:routes:read".into(),
                "team:platform-admin:clusters:write".into(),
            ],
        );

        // Admin bypass should still work
        assert!(has_admin_bypass(&ctx));

        // Can access ANY team, not just platform-admin
        assert!(check_resource_access(&ctx, "openapi-import", "write", Some("engineering")));
        assert!(check_resource_access(&ctx, "openapi-import", "read", Some("payments")));
        assert!(check_resource_access(&ctx, "routes", "write", Some("random-team")));

        // require_resource_access also works for any team
        assert!(
            require_resource_access(&ctx, "openapi-import", "write", Some("engineering")).is_ok()
        );
        assert!(
            require_resource_access(&ctx, "openapi-import", "read", Some("platform-admin")).is_ok()
        );
    }

    /// Test that extract_team_scopes correctly extracts teams but ignores admin:all
    #[test]
    fn extract_team_scopes_ignores_admin_all_scope() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("admin-mixed"),
            "admin".into(),
            vec![
                "admin:all".into(),
                "team:engineering:routes:read".into(),
                "team:platform:clusters:write".into(),
            ],
        );

        let teams = extract_team_scopes(&ctx);

        // Should contain the team names from team-scoped permissions
        assert!(teams.contains(&"engineering".to_string()));
        assert!(teams.contains(&"platform".to_string()));

        // Should NOT contain "admin" - admin:all is not a team scope
        assert!(!teams.contains(&"admin".to_string()));
        assert!(!teams.contains(&"all".to_string()));
    }

    /// Test that user with team-scoped permission can access their team
    #[test]
    fn user_with_team_scope_can_access_own_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("eng-user"),
            "eng-user".into(),
            vec!["team:engineering:openapi-import:write".into()],
        );

        assert!(check_resource_access(&ctx, "openapi-import", "write", Some("engineering")));
        assert!(
            require_resource_access(&ctx, "openapi-import", "write", Some("engineering")).is_ok()
        );
    }

    /// Test that user with team-scoped permission CANNOT access other teams
    #[test]
    fn user_with_team_scope_cannot_access_other_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("eng-user"),
            "eng-user".into(),
            vec!["team:engineering:openapi-import:write".into()],
        );

        // Cannot access platform team
        assert!(!check_resource_access(&ctx, "openapi-import", "write", Some("platform")));
        assert!(require_resource_access(&ctx, "openapi-import", "write", Some("platform")).is_err());
    }

    /// SECURITY FIX: Non-admin user with global resource scope CANNOT access any team's resources.
    /// Global scopes are now restricted to platform admins only.
    #[test]
    fn non_admin_with_global_scope_cannot_access_any_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("global-user"),
            "global".into(),
            vec!["openapi-import:write".into()],
        );

        // Non-admin with global scope should be denied access to all teams
        assert!(!check_resource_access(&ctx, "openapi-import", "write", Some("engineering")));
        assert!(!check_resource_access(&ctx, "openapi-import", "write", Some("platform")));
        assert!(require_resource_access(&ctx, "openapi-import", "write", Some("random")).is_err());
    }

    /// Platform admin with global resource scope CAN access any team's resources.
    #[test]
    fn admin_with_global_scope_can_access_any_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("admin-global"),
            "admin-global".into(),
            vec!["admin:all".into(), "openapi-import:write".into()],
        );

        assert!(check_resource_access(&ctx, "openapi-import", "write", Some("engineering")));
        assert!(check_resource_access(&ctx, "openapi-import", "write", Some("platform")));
        assert!(require_resource_access(&ctx, "openapi-import", "write", Some("random")).is_ok());
    }

    /// Test is_global_resource_scope classification
    #[test]
    fn is_global_resource_scope_classification() {
        // Global resource scopes (should be true)
        assert!(is_global_resource_scope("clusters:read"));
        assert!(is_global_resource_scope("routes:write"));
        assert!(is_global_resource_scope("listeners:read"));
        assert!(is_global_resource_scope("openapi-import:write"));

        // NOT global resource scopes (should be false)
        assert!(!is_global_resource_scope("admin:all"));
        assert!(!is_global_resource_scope("team:eng:routes:read"));
        assert!(!is_global_resource_scope("org:acme:admin"));
        assert!(!is_global_resource_scope("team:platform:*:*"));
    }

    /// Test has_admin_bypass returns true only for admin:all scope
    #[test]
    fn has_admin_bypass_requires_exact_admin_all_scope() {
        // Context with admin:all should bypass
        let admin_ctx = admin_context();
        assert!(has_admin_bypass(&admin_ctx));

        // Context with only team scopes should NOT bypass
        let team_ctx = platform_team_context();
        assert!(!has_admin_bypass(&team_ctx));

        // Context with global resource scopes should NOT bypass
        let global_ctx = global_read_context();
        assert!(!has_admin_bypass(&global_ctx));

        // Context with "admin:" prefix but not "admin:all" should NOT bypass
        let partial_admin_ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("partial"),
            "partial".into(),
            vec!["admin:users".into()],
        );
        assert!(!has_admin_bypass(&partial_admin_ctx));
    }

    #[test]
    fn resource_from_path_extracts_resource_name() {
        // route-configs API path maps to "routes" scope
        assert_eq!(resource_from_path("/api/v1/route-configs"), Some("routes"));
        assert_eq!(resource_from_path("/api/v1/route-configs/123"), Some("routes"));

        // route-views API path also maps to "routes" scope
        assert_eq!(resource_from_path("/api/v1/route-views"), Some("routes"));
        assert_eq!(resource_from_path("/api/v1/route-views/stats"), Some("routes"));

        // filter-types API path maps to "filters" scope
        assert_eq!(
            resource_from_path("/api/v1/filter-types"),
            Some("filters"),
            "filter-types list endpoint should map to filters resource"
        );
        assert_eq!(
            resource_from_path("/api/v1/filter-types/header_mutation"),
            Some("filters"),
            "filter-types detail endpoint should map to filters resource"
        );

        assert_eq!(resource_from_path("/api/v1/clusters/my-cluster"), Some("clusters"));
        assert_eq!(resource_from_path("/api/v1/listeners"), Some("listeners"));
        assert_eq!(resource_from_path("/api/v1/api-definitions"), Some("api-definitions"));
        assert_eq!(resource_from_path("/api/v1/tokens/revoke"), Some("tokens"));
        assert_eq!(resource_from_path("/health"), None);
        assert_eq!(resource_from_path("/api/v2/route-configs"), None); // Wrong version

        // Special case: team bootstrap endpoint uses generate-envoy-config resource
        assert_eq!(
            resource_from_path("/api/v1/teams/payments/bootstrap"),
            Some("generate-envoy-config"),
            "team bootstrap should use generate-envoy-config resource"
        );
        assert_eq!(
            resource_from_path("/api/v1/teams/engineering/bootstrap"),
            Some("generate-envoy-config"),
            "team bootstrap should use generate-envoy-config resource"
        );

        // List teams endpoint should not require specific scope (accessible to all authenticated users)
        assert_eq!(
            resource_from_path("/api/v1/teams"),
            None,
            "list teams endpoint should be accessible to all authenticated users"
        );

        // Single team detail endpoint should return "teams"
        assert_eq!(
            resource_from_path("/api/v1/teams/payments"),
            Some("teams"),
            "single team detail endpoint should return teams as resource"
        );

        // Team-scoped resources should extract the sub-resource (parts[4])
        // Pattern: /api/v1/teams/{team}/{resource}
        assert_eq!(
            resource_from_path("/api/v1/teams/engineering/secrets"),
            Some("secrets"),
            "team-scoped secrets endpoint should return secrets"
        );
        assert_eq!(
            resource_from_path("/api/v1/teams/engineering/secrets/abc-123"),
            Some("secrets"),
            "team-scoped secrets detail endpoint should return secrets"
        );
        assert_eq!(
            resource_from_path("/api/v1/teams/platform/custom-filters"),
            Some("custom-wasm-filters"),
            "team-scoped custom-filters should map to custom-wasm-filters scope"
        );
        assert_eq!(
            resource_from_path("/api/v1/teams/platform/custom-filters/filter-id"),
            Some("custom-wasm-filters"),
            "team-scoped custom-filters detail should map to custom-wasm-filters scope"
        );
        assert_eq!(
            resource_from_path("/api/v1/teams/eng/proxy-certificates"),
            Some("proxy-certificates"),
            "team-scoped proxy-certificates endpoint should return proxy-certificates"
        );
        assert_eq!(
            resource_from_path("/api/v1/teams/eng/stats"),
            Some("stats"),
            "team-scoped stats endpoint should return stats"
        );
        assert_eq!(
            resource_from_path("/api/v1/teams/eng/stats/overview"),
            Some("stats"),
            "team-scoped stats sub-path should return stats"
        );

        // MCP endpoints use method-level authorization (JSON-RPC style)
        // The HTTP method is always POST but the actual operation is in the request body
        // The handler implements its own authorization based on the JSON-RPC method field
        assert_eq!(
            resource_from_path("/api/v1/mcp/cp"),
            None,
            "MCP CP JSON-RPC endpoint should bypass resource-level auth (method-level auth inside handler)"
        );
        assert_eq!(
            resource_from_path("/api/v1/mcp/cp/connections"),
            None,
            "MCP CP connections endpoint should bypass resource-level auth"
        );
        assert_eq!(
            resource_from_path("/api/v1/mcp/api"),
            None,
            "MCP API tools endpoint should bypass resource-level auth"
        );

        // Org-scoped endpoints use handler-level auth, not resource-level scopes
        assert_eq!(
            resource_from_path("/api/v1/orgs/current"),
            None,
            "orgs/current should bypass resource-level auth (handler auth via org scopes)"
        );
        assert_eq!(
            resource_from_path("/api/v1/orgs/acme/teams"),
            None,
            "org teams endpoint should bypass resource-level auth"
        );

        // Admin organization management endpoints also bypass resource-level auth
        assert_eq!(
            resource_from_path("/api/v1/admin/organizations"),
            None,
            "admin organizations list should bypass resource-level auth"
        );
        assert_eq!(
            resource_from_path("/api/v1/admin/organizations/org-123"),
            None,
            "admin organizations detail should bypass resource-level auth"
        );
        assert_eq!(
            resource_from_path("/api/v1/admin/organizations/org-123/members"),
            None,
            "admin org members should bypass resource-level auth"
        );

        // OpenAPI import routes should map to "openapi-import" resource
        // URL structure: /api/v1/openapi/import, /api/v1/openapi/imports
        // Scope naming: team:X:openapi-import:write, openapi-import:read
        assert_eq!(
            resource_from_path("/api/v1/openapi/import"),
            Some("openapi-import"),
            "openapi import endpoint should use openapi-import resource"
        );
        assert_eq!(
            resource_from_path("/api/v1/openapi/imports"),
            Some("openapi-import"),
            "openapi imports list endpoint should use openapi-import resource"
        );
        assert_eq!(
            resource_from_path("/api/v1/openapi/imports/abc-123"),
            Some("openapi-import"),
            "openapi import detail endpoint should use openapi-import resource"
        );
    }

    // === Wildcard scope matching tests ===

    /// Test that team:X:*:* wildcard grants access to all resources
    #[test]
    fn wildcard_scope_grants_all_team_resources() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("wildcard-token"),
            "wildcard".into(),
            vec!["team:platform-admin:*:*".into()],
        );

        // Should have access to any resource with any action for this team
        assert!(check_resource_access(&ctx, "api-definitions", "read", Some("platform-admin")));
        assert!(check_resource_access(&ctx, "routes", "write", Some("platform-admin")));
        assert!(check_resource_access(&ctx, "clusters", "delete", Some("platform-admin")));
        assert!(check_resource_access(&ctx, "listeners", "read", Some("platform-admin")));

        // But NOT for other teams
        assert!(!check_resource_access(&ctx, "api-definitions", "read", Some("engineering")));
        assert!(!check_resource_access(&ctx, "routes", "write", Some("other-team")));
    }

    /// Test that team:X:{resource}:* wildcard grants access to all actions on that resource
    #[test]
    fn wildcard_action_scope_grants_all_actions() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("action-wildcard"),
            "action-wildcard".into(),
            vec!["team:engineering:routes:*".into()],
        );

        // Should have access to all actions on routes for engineering team
        assert!(check_resource_access(&ctx, "routes", "read", Some("engineering")));
        assert!(check_resource_access(&ctx, "routes", "write", Some("engineering")));
        assert!(check_resource_access(&ctx, "routes", "delete", Some("engineering")));

        // But NOT for other resources
        assert!(!check_resource_access(&ctx, "clusters", "read", Some("engineering")));
        assert!(!check_resource_access(&ctx, "api-definitions", "read", Some("engineering")));

        // And NOT for other teams
        assert!(!check_resource_access(&ctx, "routes", "read", Some("platform")));
    }

    /// Test parse_team_wildcard_scope correctly identifies wildcard scopes
    #[test]
    fn parse_team_wildcard_scope_extracts_team() {
        // Full wildcard: team:X:*:*
        assert_eq!(
            parse_team_wildcard_scope("team:platform-admin:*:*"),
            Some("platform-admin".to_string())
        );
        assert_eq!(
            parse_team_wildcard_scope("team:engineering:*:*"),
            Some("engineering".to_string())
        );

        // Action wildcard: team:X:resource:*
        assert_eq!(
            parse_team_wildcard_scope("team:platform:routes:*"),
            Some("platform".to_string())
        );

        // Non-wildcard scopes should return None
        assert_eq!(parse_team_wildcard_scope("team:platform:routes:read"), None);
        assert_eq!(parse_team_wildcard_scope("routes:read"), None);
        assert_eq!(parse_team_wildcard_scope("admin:all"), None);
    }

    /// Test that wildcard scope allows access without specifying team
    #[test]
    fn wildcard_scope_allows_access_without_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("wildcard-no-team"),
            "wildcard".into(),
            vec!["team:platform-admin:*:*".into()],
        );

        // When no team is specified, should allow access if user has any team wildcard
        assert!(check_resource_access(&ctx, "api-definitions", "read", None));
        assert!(check_resource_access(&ctx, "routes", "write", None));
    }

    /// Test bootstrap endpoint access with wildcard scope
    #[test]
    fn bootstrap_access_with_wildcard_scope() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("bootstrap-test"),
            "user-with-wildcard".into(),
            vec!["team:engineering:*:*".into()],
        );

        // User should be able to access bootstrap for their team
        // The bootstrap endpoint uses resource="generate-envoy-config", action="read"
        assert!(check_resource_access(&ctx, "generate-envoy-config", "read", Some("engineering")));

        // But not for other teams
        assert!(!check_resource_access(&ctx, "generate-envoy-config", "read", Some("platform")));
    }

    // === Org boundary verification tests ===

    #[test]
    fn verify_org_boundary_admin_bypasses() {
        let ctx = admin_context();
        let org = OrgId::new();
        // Admin can access any org
        assert!(verify_org_boundary(&ctx, &Some(org)).is_ok());
        assert!(verify_org_boundary(&ctx, &None).is_ok());
    }

    #[test]
    fn verify_org_boundary_same_org_allowed() {
        let org = OrgId::new();
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-user"),
            "org-user".into(),
            vec!["team:eng:routes:read".into()],
        )
        .with_org(org.clone(), "acme".into());

        assert!(verify_org_boundary(&ctx, &Some(org)).is_ok());
    }

    #[test]
    fn verify_org_boundary_different_org_returns_not_found() {
        let user_org = OrgId::new();
        let team_org = OrgId::new();
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-user"),
            "org-user".into(),
            vec!["team:eng:routes:read".into()],
        )
        .with_org(user_org, "acme".into());

        let result = verify_org_boundary(&ctx, &Some(team_org));
        assert!(result.is_err());
        if let Err(ApiError::NotFound(msg)) = result {
            assert_eq!(msg, "Resource not found");
        } else {
            panic!("Expected NotFound error");
        }
    }

    #[test]
    fn verify_org_boundary_user_no_org_team_has_org_denied() {
        let team_org = OrgId::new();
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("no-org-user"),
            "no-org".into(),
            vec!["team:eng:routes:read".into()],
        );
        // User has no org, team has org -- deny
        let result = verify_org_boundary(&ctx, &Some(team_org));
        assert!(result.is_err());
    }

    #[test]
    fn verify_org_boundary_global_team_allowed() {
        let user_org = OrgId::new();
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-user"),
            "org-user".into(),
            vec!["team:eng:routes:read".into()],
        )
        .with_org(user_org, "acme".into());

        // Team has no org (global) -- allow
        assert!(verify_org_boundary(&ctx, &None).is_ok());
    }

    #[test]
    fn verify_org_boundary_both_no_org_allowed() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("no-org"),
            "no-org".into(),
            vec!["routes:read".into()],
        );
        // Both have no org -- allow
        assert!(verify_org_boundary(&ctx, &None).is_ok());
    }
}
