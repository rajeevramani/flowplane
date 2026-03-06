//! Authorization helpers for resource-level and team-scoped access control.
//!
//! This module provides functions to check permissions using scope patterns:
//! - `admin:all` - Governance-only access (orgs, users, audit, summary, admin teams)
//! - `{resource}:{action}` - Resource-level permissions (e.g., `routes:read`)
//! - `team:{name}:{resource}:{action}` - Team-scoped permissions (e.g., `team:platform:routes:read`)

use crate::api::error::ApiError;
use crate::auth::models::{AgentContext, AuthContext, AuthError, GrantType};
use crate::domain::OrgId;

/// Admin scope that grants access to governance resources only (orgs, users, audit, summary).
/// Does NOT grant access to tenant resources (clusters, routes, listeners, etc.).
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

/// Returns true if the resource is a governance/admin resource that admin:all should grant access to.
/// Governance resources: org management, user management, audit, summary, admin teams, scopes, apps.
/// Tenant resources (clusters, routes, listeners, filters, etc.) are NOT governance.
pub fn is_governance_resource(resource: &str) -> bool {
    matches!(
        resource,
        "admin"
            | "admin-orgs"
            | "admin-users"
            | "admin-audit"
            | "admin-summary"
            | "admin-teams"
            | "admin-scopes"
            | "admin-apps"
            | "admin-filter-schemas"
            | "organizations"
            | "users"
            | "teams"
            | "stats"
    )
}

/// Check if the context has access to perform an action on a resource.
///
/// This function checks for permissions in the following order:
/// 1. Admin governance bypass (`admin:all` for governance resources only)
/// 2. Team-scoped permission (`team:{team}:{resource}:{action}`)
/// 3. Org-admin implicit team access
///
/// # Arguments
///
/// * `context` - The authentication context from the request
/// * `resource` - The resource type (e.g., "routes", "clusters")
/// * `action` - The action being performed (e.g., "read", "create", "update", "delete")
/// * `team` - Optional team name for team-scoped resources
///
/// # Examples
///
/// ```rust
/// use flowplane::auth::authorization::check_resource_access;
/// use flowplane::auth::models::{AuthContext, Grant, GrantType};
/// use flowplane::domain::TokenId;
///
/// let mut ctx = AuthContext::new(
///     TokenId::from_str_unchecked("token-1"),
///     "platform-token".into(),
///     vec![]
/// );
/// ctx.grants = vec![
///     Grant {
///         grant_type: GrantType::Resource,
///         team_id: "t1".into(),
///         team_name: "platform".into(),
///         resource_type: Some("routes".into()),
///         action: Some("read".into()),
///         route_id: None,
///         allowed_methods: vec![],
///     },
///     Grant {
///         grant_type: GrantType::Resource,
///         team_id: "t1".into(),
///         team_name: "platform".into(),
///         resource_type: Some("routes".into()),
///         action: Some("create".into()),
///         route_id: None,
///         allowed_methods: vec![],
///     },
/// ];
///
/// // Has access to platform team routes
/// assert!(check_resource_access(&ctx, "routes", "read", Some("platform")));
/// assert!(check_resource_access(&ctx, "routes", "create", Some("platform")));
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
    // Agent context structural guard (DD-3).
    // GatewayTool/ApiConsumer agents are structurally blocked from CP resources.
    // CpTool agents use the same grant-based path as humans but are restricted
    // to GrantType::Resource grants only.
    if let Some(ref agent_ctx) = context.agent_context {
        return match agent_ctx {
            AgentContext::CpTool => {
                // CpTool agents check resource grants (same table, same logic)
                if let Some(team_name) = team {
                    context.has_grant(resource, action, team_name)
                } else {
                    context.has_any_grant(resource, action)
                }
            }
            // Gateway-tool and api-consumer agents cannot call CP resources
            AgentContext::GatewayTool | AgentContext::ApiConsumer => false,
        };
    }

    // Admin bypass for governance resources only.
    // admin:all grants access to org/user/team management, audit, summary
    // but does NOT grant access to tenant resources (clusters, routes, etc.).
    if has_admin_bypass(context) && is_governance_resource(resource) {
        return true;
    }

    // Check grants (unified for all human users)
    if let Some(team_name) = team {
        // Check exact grant match
        if context.has_grant(resource, action, team_name) {
            return true;
        }

        // Org admins have implicit access to all teams in their org (DD-2).
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
                    // Deny access — tokens must have org binding for team access.
                    tracing::warn!(
                        scope_org = %org_name,
                        "org admin scope without org context on token, denying implicit team access"
                    );
                }
            }
        }
    } else {
        // No team specified — check if user has ANY grant for this resource/action.
        // This allows team-scoped users to call handlers that will filter by their teams.
        if context.has_any_grant(resource, action) {
            return true;
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

/// Require resource access or return a 403 Forbidden error.
///
/// This is a convenience function that calls `check_resource_access` and
/// returns an error if access is denied.
///
/// # Arguments
///
/// * `context` - The authentication context from the request
/// * `resource` - The resource type (e.g., "routes", "clusters")
/// * `action` - The action being performed (e.g., "read", "create", "update", "delete")
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
/// use flowplane::auth::models::{AuthContext, AuthError, Grant, GrantType};
/// use flowplane::domain::TokenId;
///
/// // Team-scoped user can access their team
/// let mut ctx = AuthContext::new(
///     TokenId::from_str_unchecked("token-1"),
///     "demo".into(),
///     vec![]
/// );
/// ctx.grants = vec![Grant {
///     grant_type: GrantType::Resource,
///     team_id: "t1".into(),
///     team_name: "platform".into(),
///     resource_type: Some("routes".into()),
///     action: Some("read".into()),
///     route_id: None,
///     allowed_methods: vec![],
/// }];
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

/// Extract all team names from resource grants in the context.
///
/// Returns a deduplicated, sorted list of team names from resource grants.
///
/// # Arguments
///
/// * `context` - The authentication context from the request
///
/// # Returns
///
/// A vector of unique team names found in the grants.
pub fn extract_team_scopes(context: &AuthContext) -> Vec<String> {
    context.grant_team_names()
}

/// Parse organization name from a scope string if it matches the org pattern.
///
/// Expected pattern: `org:{name}:admin`, `org:{name}:member`, or `org:{name}:viewer`
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
    if parts.len() == 3
        && parts[0] == "org"
        && (parts[2] == "admin" || parts[2] == "member" || parts[2] == "viewer")
    {
        Some((parts[1].to_string(), parts[2].to_string()))
    } else {
        None
    }
}

/// Extract all team names from resource grants in the context.
///
/// Returns a deduplicated, sorted list of team names from resource grants.
///
/// The admin path (`admin:all`) is NOT handled here — callers should check
/// `has_admin_bypass()` separately before calling this.
pub fn extract_team_names(context: &AuthContext) -> Vec<String> {
    context.grant_team_names()
}

/// Extract all organization names and roles from org-scoped permissions in the context.
///
/// Parses org_scopes matching the pattern `org:{name}:admin|member|viewer` and
/// returns a list of (org_name, role) pairs.
pub fn extract_org_scopes(context: &AuthContext) -> Vec<(String, String)> {
    let mut orgs = Vec::new();

    for scope in context.org_scopes() {
        if let Some(pair) = parse_org_from_scope(scope) {
            orgs.push(pair);
        }
    }

    orgs
}

/// Check if the context has org admin privileges for a specific org.
///
/// Returns `true` if the context has `org:{name}:admin` scope OR `admin:all`.
/// Platform admin (`admin:all`) bypasses here for governance operations
/// (e.g., inviting the first org admin during onboarding).
///
/// For operations where platform admin should NOT have access (member management,
/// team management), use `has_org_admin_only()` instead.
pub fn has_org_admin(context: &AuthContext, org_name: &str) -> bool {
    has_admin_bypass(context) || context.has_scope(&format!("org:{}:admin", org_name))
}

/// Require org admin access or return a 403 Forbidden error.
pub fn require_org_admin(context: &AuthContext, org_name: &str) -> Result<(), AuthError> {
    if has_org_admin(context, org_name) {
        Ok(())
    } else {
        Err(AuthError::Forbidden)
    }
}

/// Check if the context has org admin privileges for a specific org.
/// Unlike `has_org_admin`, this does NOT grant access to platform admin.
/// Use this for operations where platform admin should not see into orgs
/// (member management, team management).
pub fn has_org_admin_only(context: &AuthContext, org_name: &str) -> bool {
    context.has_scope(&format!("org:{}:admin", org_name))
}

/// Require org admin access (no platform admin bypass) or return 403.
/// Use this instead of `require_org_admin` for operations where platform admin
/// should NOT have access (member CRUD, team management).
pub fn require_org_admin_only(context: &AuthContext, org_name: &str) -> Result<(), AuthError> {
    if has_org_admin_only(context, org_name) {
        Ok(())
    } else {
        Err(AuthError::Forbidden)
    }
}

/// Check if the context has any org membership (admin or member) for a specific org.
///
/// Returns `true` if the context has `org:{name}:admin`, `org:{name}:member`,
/// `org:{name}:viewer`, or `admin:all`.
/// Platform admin (`admin:all`) bypasses here because org membership checks are governance
/// (viewing org details, listing org members, accessing org-scoped admin pages).
pub fn has_org_membership(context: &AuthContext, org_name: &str) -> bool {
    has_admin_bypass(context)
        || context.has_scope(&format!("org:{}:admin", org_name))
        || context.has_scope(&format!("org:{}:member", org_name))
        || context.has_scope(&format!("org:{}:viewer", org_name))
}

/// Check if the context has any resource grants (i.e., team-scoped permissions).
///
/// Returns `true` if at least one resource grant exists on the context.
pub fn has_team_scopes(context: &AuthContext) -> bool {
    context.grants.iter().any(|g| g.grant_type == GrantType::Resource)
}

/// Derive the required action from an HTTP method.
///
/// Maps HTTP methods to fine-grained RBAC actions:
/// - GET, HEAD, OPTIONS → "read"
/// - POST → "create"
/// - PUT, PATCH → "update"
/// - DELETE → "delete"
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
/// assert_eq!(action_from_http_method("POST"), "create");
/// assert_eq!(action_from_http_method("PUT"), "update");
/// assert_eq!(action_from_http_method("PATCH"), "update");
/// assert_eq!(action_from_http_method("DELETE"), "delete");
/// assert_eq!(action_from_http_method("OPTIONS"), "read");
/// ```
pub fn action_from_http_method(method: &str) -> &'static str {
    match method.to_uppercase().as_str() {
        "GET" | "HEAD" | "OPTIONS" => "read",
        "POST" => "create",
        "PUT" | "PATCH" => "update",
        "DELETE" => "delete",
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
/// The semantic action string ("read", "create", "update", or "delete").
///
/// # Examples
///
/// ```rust
/// use flowplane::auth::authorization::action_from_request;
///
/// // Regular GET → read
/// assert_eq!(action_from_request("GET", "/api/v1/routes"), "read");
///
/// // Regular POST → create
/// assert_eq!(action_from_request("POST", "/api/v1/routes"), "create");
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
        // The scope naming convention uses "openapi-import" (e.g., team:X:openapi-import:create)
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
/// Platform admins (`admin:all`) do NOT bypass this check. All users must
/// have a matching org to access team resources.
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
    use crate::auth::models::Grant;

    // === Grant-based test helpers ===

    fn make_grant(resource: &str, action: &str, team_name: &str) -> Grant {
        Grant {
            grant_type: GrantType::Resource,
            team_id: format!("test-team-id-{}", team_name),
            team_name: team_name.to_string(),
            resource_type: Some(resource.to_string()),
            action: Some(action.to_string()),
            route_id: None,
            allowed_methods: vec![],
        }
    }

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
            vec![],
        )
        .with_grants(
            vec![
                make_grant("routes", "read", "platform"),
                make_grant("routes", "create", "platform"),
                make_grant("clusters", "read", "platform"),
            ],
            None,
        )
    }

    fn no_grants_context() -> AuthContext {
        // User with no org scopes and no grants — should be denied everything
        AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("no-grants-token"),
            "no-grants".into(),
            vec![],
        )
    }

    #[test]
    fn admin_bypass_grants_governance_access_only() {
        let ctx = admin_context();
        assert!(has_admin_bypass(&ctx));

        // Governance resources → allowed
        assert!(check_resource_access(&ctx, "organizations", "read", None));
        assert!(check_resource_access(&ctx, "users", "create", None));
        assert!(check_resource_access(&ctx, "admin-audit", "read", None));
        assert!(check_resource_access(&ctx, "admin-summary", "read", None));
        assert!(check_resource_access(&ctx, "teams", "read", None));

        // Tenant resources → denied (admin:all is governance-only)
        assert!(!check_resource_access(&ctx, "routes", "read", None));
        assert!(!check_resource_access(&ctx, "routes", "create", Some("platform")));
        assert!(!check_resource_access(&ctx, "clusters", "delete", Some("engineering")));
    }

    #[test]
    fn no_grants_user_denied() {
        // User with no grants and no org scopes should be denied everything
        let ctx = no_grants_context();
        assert!(!has_admin_bypass(&ctx));
        assert!(!check_resource_access(&ctx, "routes", "read", None));
        assert!(!check_resource_access(&ctx, "clusters", "read", None));
        assert!(!check_resource_access(&ctx, "routes", "create", None));
        assert!(!check_resource_access(&ctx, "listeners", "read", None));
    }

    #[test]
    fn team_scoped_access_respects_team_boundaries() {
        let ctx = platform_team_context();
        assert!(has_team_scopes(&ctx));

        // Has access to platform team
        assert!(check_resource_access(&ctx, "routes", "read", Some("platform")));
        assert!(check_resource_access(&ctx, "routes", "create", Some("platform")));
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
            vec![],
        )
        .with_grants(
            vec![
                make_grant("routes", "read", "platform"),
                make_grant("clusters", "read", "platform"),
                make_grant("routes", "read", "engineering"),
            ],
            None,
        );

        let teams = extract_team_scopes(&ctx);
        assert_eq!(teams.len(), 2);
        assert!(teams.contains(&"platform".to_string()));
        assert!(teams.contains(&"engineering".to_string()));
    }

    #[test]
    fn extract_team_scopes_returns_empty_for_no_grants() {
        // Users with no grants should get empty team list
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("no-grants-only"),
            "no-grants-user".into(),
            vec![],
        );

        let teams = extract_team_scopes(&ctx);
        assert_eq!(teams.len(), 0, "Users with no grants should NOT bypass team isolation");
        assert!(!has_admin_bypass(&ctx), "No org scopes should not grant admin bypass");
    }

    #[test]
    fn extract_team_scopes_correctly_returns_grant_teams() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("team-scoped"),
            "correct-user".into(),
            vec![],
        )
        .with_grants(
            vec![
                make_grant("listeners", "read", "engineering"),
                make_grant("routes", "read", "engineering"),
                make_grant("clusters", "read", "engineering"),
            ],
            None,
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
        let ctx = no_grants_context();
        let err = require_resource_access(&ctx, "routes", "read", None).unwrap_err();
        assert!(matches!(err, AuthError::Forbidden));
    }

    #[test]
    fn action_from_http_method_maps_correctly() {
        assert_eq!(action_from_http_method("GET"), "read");
        assert_eq!(action_from_http_method("POST"), "create");
        assert_eq!(action_from_http_method("PUT"), "update");
        assert_eq!(action_from_http_method("PATCH"), "update");
        assert_eq!(action_from_http_method("DELETE"), "delete");
        assert_eq!(action_from_http_method("HEAD"), "read");
        assert_eq!(action_from_http_method("OPTIONS"), "read");
        assert_eq!(action_from_http_method("UNKNOWN"), "read");
    }

    #[test]
    fn action_from_request_handles_regular_methods() {
        assert_eq!(action_from_request("GET", "/api/v1/routes"), "read");
        assert_eq!(action_from_request("GET", "/api/v1/clusters/123"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/routes"), "create");
        assert_eq!(action_from_request("POST", "/api/v1/clusters"), "create");
        assert_eq!(action_from_request("PUT", "/api/v1/routes/123"), "update");
        assert_eq!(action_from_request("PATCH", "/api/v1/routes/123"), "update");
        assert_eq!(action_from_request("DELETE", "/api/v1/routes/123"), "delete");
    }

    #[test]
    fn action_from_request_identifies_export_as_read() {
        assert_eq!(action_from_request("POST", "/api/v1/aggregated-schemas/export"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/schemas/export"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/data/export"), "read");
        assert_eq!(action_from_request("PUT", "/api/v1/data/export"), "read");
        assert_eq!(action_from_request("PATCH", "/api/v1/data/export"), "read");
    }

    #[test]
    fn action_from_request_identifies_compare_as_read() {
        assert_eq!(action_from_request("POST", "/api/v1/schemas/compare"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/aggregated-schemas/compare"), "read");
        assert_eq!(action_from_request("PUT", "/api/v1/data/compare"), "read");
    }

    #[test]
    fn action_from_request_identifies_search_as_read() {
        assert_eq!(action_from_request("POST", "/api/v1/resources/search"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/routes/search"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/search"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/search/filters"), "read");
    }

    #[test]
    fn action_from_request_identifies_query_as_read() {
        assert_eq!(action_from_request("POST", "/api/v1/data/query"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/query"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/query/advanced"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/logs/query/recent"), "read");
    }

    #[test]
    fn action_from_request_does_not_false_positive() {
        assert_eq!(action_from_request("POST", "/api/v1/search-configs"), "create");
        assert_eq!(action_from_request("POST", "/api/v1/query-builder"), "create");
        assert_eq!(action_from_request("POST", "/api/v1/export-configs"), "create");
        assert_eq!(action_from_request("POST", "/api/v1/compare-tool"), "create");
    }

    #[test]
    fn action_from_request_case_insensitive_methods() {
        assert_eq!(action_from_request("post", "/api/v1/routes"), "create");
        assert_eq!(action_from_request("get", "/api/v1/routes"), "read");
        assert_eq!(action_from_request("POST", "/api/v1/schemas/export"), "read");
        assert_eq!(action_from_request("post", "/api/v1/schemas/export"), "read");
    }

    #[test]
    fn action_from_request_real_world_examples() {
        assert_eq!(
            action_from_request("POST", "/api/v1/aggregated-schemas/export"),
            "read",
            "aggregated-schemas export should be read operation"
        );
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
        assert_eq!(
            action_from_request("POST", "/api/v1/api-definitions/search"),
            "read",
            "api-definitions search should be read operation"
        );
        assert_eq!(
            action_from_request("POST", "/api/v1/aggregated-schemas"),
            "create",
            "creating aggregated-schema should be create operation"
        );
        assert_eq!(
            action_from_request("PUT", "/api/v1/aggregated-schemas/123"),
            "update",
            "updating aggregated-schema should be update operation"
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
        assert_eq!(parse_org_from_scope("org:acme:viewer"), Some(("acme".into(), "viewer".into())));
        assert_eq!(parse_org_from_scope("team:acme:routes:read"), None);
        assert_eq!(parse_org_from_scope("routes:read"), None);
        assert_eq!(parse_org_from_scope("org:acme"), None);
    }

    // === Team name extraction tests ===

    #[test]
    fn extract_team_names_from_grants() {
        let ctx =
            AuthContext::new(crate::domain::TokenId::from_str_unchecked("t1"), "t1".into(), vec![])
                .with_grants(
                    vec![
                        make_grant("clusters", "read", "platform"),
                        make_grant("routes", "create", "platform"),
                        make_grant("listeners", "read", "sre"),
                    ],
                    None,
                );
        let teams = extract_team_names(&ctx);
        assert_eq!(teams, vec!["platform", "sre"]);
    }

    #[test]
    fn extract_team_names_deduplicates() {
        let ctx =
            AuthContext::new(crate::domain::TokenId::from_str_unchecked("t3"), "t3".into(), vec![])
                .with_grants(
                    vec![
                        make_grant("clusters", "read", "backend"),
                        make_grant("clusters", "create", "backend"),
                        make_grant("routes", "read", "backend"),
                    ],
                    None,
                );
        let teams = extract_team_names(&ctx);
        assert_eq!(teams, vec!["backend"]);
    }

    #[test]
    fn extract_team_names_no_grants() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("t4"),
            "t4".into(),
            vec!["admin:all".into(), "org:acme:admin".into()],
        );
        let teams = extract_team_names(&ctx);
        assert!(teams.is_empty());
    }

    #[test]
    fn extract_org_scopes_returns_org_memberships() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-user"),
            "org-user".into(),
            vec!["org:acme:admin".into(), "org:globex:member".into()],
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
    fn has_org_admin_grants_platform_admin() {
        let ctx = admin_context();
        assert!(has_org_admin(&ctx, "any-org"));
        assert!(has_org_admin(&ctx, "acme"));
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

    #[test]
    fn has_org_membership_grants_platform_admin() {
        let ctx = admin_context();
        assert!(has_org_membership(&ctx, "acme"));
        assert!(has_org_membership(&ctx, "any-org"));
    }

    // === require_org_admin_only tests (no platform admin bypass) ===

    fn org_admin_context_for(org_name: &str) -> AuthContext {
        AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-admin-token"),
            "org-admin".into(),
            vec![format!("org:{}:admin", org_name)],
        )
    }

    fn org_member_context_for(org_name: &str) -> AuthContext {
        AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-member-token"),
            "org-member".into(),
            vec![format!("org:{}:member", org_name)],
        )
    }

    #[test]
    fn require_org_admin_only_allows_matching_org_admin() {
        let ctx = org_admin_context_for("acme");
        assert!(require_org_admin_only(&ctx, "acme").is_ok());
    }

    #[test]
    fn require_org_admin_only_rejects_wrong_org_admin() {
        let ctx = org_admin_context_for("acme");
        assert!(require_org_admin_only(&ctx, "globex").is_err());
    }

    #[test]
    fn require_org_admin_only_rejects_platform_admin() {
        let ctx = admin_context();
        assert!(require_org_admin_only(&ctx, "acme").is_err());
        assert!(require_org_admin_only(&ctx, "any-org").is_err());
    }

    #[test]
    fn require_org_admin_only_rejects_regular_user() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("regular-token"),
            "regular".into(),
            vec![],
        );
        assert!(require_org_admin_only(&ctx, "acme").is_err());
    }

    #[test]
    fn require_org_admin_only_rejects_org_member() {
        let ctx = org_member_context_for("acme");
        assert!(require_org_admin_only(&ctx, "acme").is_err());
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
        assert!(check_resource_access(&ctx, "listeners", "create", None));
    }

    #[test]
    fn org_admin_passes_check_resource_access_without_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-admin"),
            "org-admin".into(),
            vec!["org:acme:admin".into()],
        );

        assert!(check_resource_access(&ctx, "routes", "read", None));
        assert!(check_resource_access(&ctx, "clusters", "create", None));
    }

    #[test]
    fn org_admin_passes_check_resource_access_with_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-admin"),
            "org-admin".into(),
            vec!["org:acme:admin".into()],
        )
        .with_org(crate::domain::OrgId::from_str_unchecked("acme-id"), "acme".into());

        // Org admin should pass for any team (implicit access to all org teams)
        assert!(check_resource_access(&ctx, "routes", "read", Some("engineering")));
        assert!(check_resource_access(&ctx, "clusters", "create", Some("engineering")));
        assert!(check_resource_access(&ctx, "listeners", "read", Some("frontend")));
    }

    #[test]
    fn org_member_fails_check_resource_access_with_wrong_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-member"),
            "org-member".into(),
            vec!["org:acme:member".into()],
        )
        .with_grants(vec![make_grant("routes", "read", "acme-default")], None);

        // Org member should NOT get implicit team access (only admins do)
        assert!(!check_resource_access(&ctx, "routes", "read", Some("engineering")));
        // But should have access to their own team via grant
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
        assert!(check_resource_access(&ctx, "clusters", "create", Some("platform")));
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

        // Scope says "acme" but user's org is "globex" → denied
        assert!(!check_resource_access(&ctx, "routes", "read", Some("engineering")));
        assert!(!check_resource_access(&ctx, "clusters", "create", Some("platform")));
    }

    #[test]
    fn org_admin_without_org_context_denied() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-admin-no-ctx"),
            "org-admin".into(),
            vec!["org:acme:admin".into()],
        );

        // No org_name on context → deny
        assert!(!check_resource_access(&ctx, "routes", "read", Some("engineering")));
    }

    #[test]
    fn no_scopes_no_grants_fails_check_resource_access() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("empty"),
            "no-scopes".into(),
            vec![],
        );

        assert!(!check_resource_access(&ctx, "routes", "read", None));
        assert!(!check_resource_access(&ctx, "routes", "read", Some("platform")));
        assert!(!check_resource_access(&ctx, "clusters", "create", None));
    }

    // === Governance-only admin access tests ===

    #[test]
    fn admin_only_denied_for_tenant_resources() {
        let ctx = admin_context();

        assert!(!check_resource_access(&ctx, "openapi-import", "create", Some("engineering")));
        assert!(!check_resource_access(&ctx, "openapi-import", "read", Some("platform")));
        assert!(!check_resource_access(&ctx, "openapi-import", "delete", Some("random-team")));

        assert!(
            require_resource_access(&ctx, "openapi-import", "create", Some("engineering")).is_err()
        );
    }

    #[test]
    fn admin_with_grants_restricted_to_own_teams() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("admin-with-grants"),
            "admin-with-teams".into(),
            vec!["admin:all".into()],
        )
        .with_grants(
            vec![
                make_grant("routes", "read", "platform-admin"),
                make_grant("clusters", "create", "platform-admin"),
            ],
            None,
        );

        assert!(has_admin_bypass(&ctx));

        // Can access own team's resources via grants
        assert!(check_resource_access(&ctx, "routes", "read", Some("platform-admin")));
        assert!(check_resource_access(&ctx, "clusters", "create", Some("platform-admin")));

        // CANNOT access other teams' tenant resources
        assert!(!check_resource_access(&ctx, "openapi-import", "create", Some("engineering")));
        assert!(!check_resource_access(&ctx, "openapi-import", "read", Some("payments")));
        assert!(!check_resource_access(&ctx, "routes", "create", Some("random-team")));

        // Governance resources still allowed via admin:all
        assert!(check_resource_access(&ctx, "organizations", "read", None));
        assert!(check_resource_access(&ctx, "admin-audit", "read", None));
    }

    #[test]
    fn extract_team_scopes_ignores_admin_all_scope() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("admin-mixed"),
            "admin".into(),
            vec!["admin:all".into()],
        )
        .with_grants(
            vec![
                make_grant("routes", "read", "engineering"),
                make_grant("clusters", "create", "platform"),
            ],
            None,
        );

        let teams = extract_team_scopes(&ctx);
        assert!(teams.contains(&"engineering".to_string()));
        assert!(teams.contains(&"platform".to_string()));
        assert!(!teams.contains(&"admin".to_string()));
        assert!(!teams.contains(&"all".to_string()));
    }

    #[test]
    fn user_with_grant_can_access_own_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("eng-user"),
            "eng-user".into(),
            vec![],
        )
        .with_grants(vec![make_grant("openapi-import", "create", "engineering")], None);

        assert!(check_resource_access(&ctx, "openapi-import", "create", Some("engineering")));
        assert!(
            require_resource_access(&ctx, "openapi-import", "create", Some("engineering")).is_ok()
        );
    }

    #[test]
    fn user_with_grant_cannot_access_other_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("eng-user"),
            "eng-user".into(),
            vec![],
        )
        .with_grants(vec![make_grant("openapi-import", "create", "engineering")], None);

        assert!(!check_resource_access(&ctx, "openapi-import", "create", Some("platform")));
        assert!(
            require_resource_access(&ctx, "openapi-import", "create", Some("platform")).is_err()
        );
    }

    #[test]
    fn user_without_grants_cannot_access_any_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("no-grants-user"),
            "no-grants".into(),
            vec![],
        );

        assert!(!check_resource_access(&ctx, "openapi-import", "create", Some("engineering")));
        assert!(!check_resource_access(&ctx, "openapi-import", "create", Some("platform")));
        assert!(require_resource_access(&ctx, "openapi-import", "create", Some("random")).is_err());
    }

    #[test]
    fn admin_without_grants_denied_for_tenant_team_resources() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("admin-no-grants"),
            "admin-no-grants".into(),
            vec!["admin:all".into()],
        );

        assert!(!check_resource_access(&ctx, "openapi-import", "create", Some("engineering")));
        assert!(!check_resource_access(&ctx, "openapi-import", "create", Some("platform")));
        assert!(require_resource_access(&ctx, "openapi-import", "create", Some("random")).is_err());

        // Governance resources → still allowed
        assert!(check_resource_access(&ctx, "organizations", "create", None));
        assert!(check_resource_access(&ctx, "admin-summary", "read", None));
    }

    #[test]
    fn has_admin_bypass_requires_exact_admin_all_scope() {
        let admin_ctx = admin_context();
        assert!(has_admin_bypass(&admin_ctx));

        let team_ctx = platform_team_context();
        assert!(!has_admin_bypass(&team_ctx));

        let no_grants_ctx = no_grants_context();
        assert!(!has_admin_bypass(&no_grants_ctx));

        let partial_admin_ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("partial"),
            "partial".into(),
            vec!["admin:users".into()],
        );
        assert!(!has_admin_bypass(&partial_admin_ctx));
    }

    #[test]
    fn resource_from_path_extracts_resource_name() {
        assert_eq!(resource_from_path("/api/v1/route-configs"), Some("routes"));
        assert_eq!(resource_from_path("/api/v1/route-configs/123"), Some("routes"));
        assert_eq!(resource_from_path("/api/v1/route-views"), Some("routes"));
        assert_eq!(resource_from_path("/api/v1/route-views/stats"), Some("routes"));
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
        assert_eq!(resource_from_path("/health"), None);
        assert_eq!(resource_from_path("/api/v2/route-configs"), None);
        assert_eq!(
            resource_from_path("/api/v1/teams/payments/bootstrap"),
            Some("generate-envoy-config")
        );
        assert_eq!(
            resource_from_path("/api/v1/teams/engineering/bootstrap"),
            Some("generate-envoy-config")
        );
        assert_eq!(resource_from_path("/api/v1/teams"), None);
        assert_eq!(resource_from_path("/api/v1/teams/payments"), Some("teams"));
        assert_eq!(resource_from_path("/api/v1/teams/engineering/secrets"), Some("secrets"));
        assert_eq!(
            resource_from_path("/api/v1/teams/engineering/secrets/abc-123"),
            Some("secrets")
        );
        assert_eq!(
            resource_from_path("/api/v1/teams/platform/custom-filters"),
            Some("custom-wasm-filters")
        );
        assert_eq!(
            resource_from_path("/api/v1/teams/platform/custom-filters/filter-id"),
            Some("custom-wasm-filters")
        );
        assert_eq!(
            resource_from_path("/api/v1/teams/eng/proxy-certificates"),
            Some("proxy-certificates")
        );
        assert_eq!(resource_from_path("/api/v1/teams/eng/stats"), Some("stats"));
        assert_eq!(resource_from_path("/api/v1/teams/eng/stats/overview"), Some("stats"));
        assert_eq!(resource_from_path("/api/v1/mcp"), None);
        assert_eq!(resource_from_path("/api/v1/mcp/connections"), None);
        assert_eq!(resource_from_path("/api/v1/orgs/current"), None);
        assert_eq!(resource_from_path("/api/v1/orgs/acme/teams"), None);
        assert_eq!(resource_from_path("/api/v1/admin/organizations"), None);
        assert_eq!(resource_from_path("/api/v1/admin/organizations/org-123"), None);
        assert_eq!(resource_from_path("/api/v1/admin/organizations/org-123/members"), None);
        assert_eq!(resource_from_path("/api/v1/openapi/import"), Some("openapi-import"));
        assert_eq!(resource_from_path("/api/v1/openapi/imports"), Some("openapi-import"));
        assert_eq!(resource_from_path("/api/v1/openapi/imports/abc-123"), Some("openapi-import"));
    }

    // === Grant-based access tests (replacing wildcard scope tests) ===

    #[test]
    fn grants_for_all_resources_on_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("full-access-token"),
            "full-access".into(),
            vec![],
        )
        .with_grants(
            vec![
                make_grant("api-definitions", "read", "platform-admin"),
                make_grant("routes", "create", "platform-admin"),
                make_grant("clusters", "delete", "platform-admin"),
                make_grant("listeners", "read", "platform-admin"),
            ],
            None,
        );

        assert!(check_resource_access(&ctx, "api-definitions", "read", Some("platform-admin")));
        assert!(check_resource_access(&ctx, "routes", "create", Some("platform-admin")));
        assert!(check_resource_access(&ctx, "clusters", "delete", Some("platform-admin")));
        assert!(check_resource_access(&ctx, "listeners", "read", Some("platform-admin")));

        // But NOT for other teams
        assert!(!check_resource_access(&ctx, "api-definitions", "read", Some("engineering")));
        assert!(!check_resource_access(&ctx, "routes", "create", Some("other-team")));
    }

    #[test]
    fn grants_allow_access_without_team() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("grants-no-team"),
            "grants".into(),
            vec![],
        )
        .with_grants(
            vec![
                make_grant("api-definitions", "read", "platform-admin"),
                make_grant("routes", "create", "platform-admin"),
            ],
            None,
        );

        // When no team is specified, should allow access if user has any grant
        assert!(check_resource_access(&ctx, "api-definitions", "read", None));
        assert!(check_resource_access(&ctx, "routes", "create", None));
    }

    #[test]
    fn bootstrap_access_with_grants() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("bootstrap-test"),
            "user-with-grants".into(),
            vec![],
        )
        .with_grants(vec![make_grant("generate-envoy-config", "read", "engineering")], None);

        assert!(check_resource_access(&ctx, "generate-envoy-config", "read", Some("engineering")));
        assert!(!check_resource_access(&ctx, "generate-envoy-config", "read", Some("platform")));
    }

    // === Org boundary verification tests ===

    #[test]
    fn verify_org_boundary_admin_no_bypass() {
        let ctx = admin_context();
        let org = OrgId::new();
        assert!(verify_org_boundary(&ctx, &Some(org)).is_err());
        assert!(verify_org_boundary(&ctx, &None).is_ok());
    }

    #[test]
    fn verify_org_boundary_same_org_allowed() {
        let org = OrgId::new();
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-user"),
            "org-user".into(),
            vec![],
        )
        .with_grants(vec![make_grant("routes", "read", "eng")], None)
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
            vec![],
        )
        .with_grants(vec![make_grant("routes", "read", "eng")], None)
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
            vec![],
        )
        .with_grants(vec![make_grant("routes", "read", "eng")], None);
        let result = verify_org_boundary(&ctx, &Some(team_org));
        assert!(result.is_err());
    }

    #[test]
    fn verify_org_boundary_global_team_allowed() {
        let user_org = OrgId::new();
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-user"),
            "org-user".into(),
            vec![],
        )
        .with_grants(vec![make_grant("routes", "read", "eng")], None)
        .with_org(user_org, "acme".into());

        assert!(verify_org_boundary(&ctx, &None).is_ok());
    }

    #[test]
    fn verify_org_boundary_both_no_org_allowed() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("no-org"),
            "no-org".into(),
            vec![],
        );
        assert!(verify_org_boundary(&ctx, &None).is_ok());
    }

    // === Security hardening: governance-only admin access ===

    #[test]
    fn test_admin_denied_for_tenant_resources() {
        let ctx = admin_context();

        let tenant_resources = [
            "clusters",
            "routes",
            "listeners",
            "filters",
            "openapi-import",
            "api-definitions",
            "secrets",
            "proxy-certificates",
            "custom-wasm-filters",
            "generate-envoy-config",
            "dataplanes",
        ];

        for resource in &tenant_resources {
            assert!(
                !check_resource_access(&ctx, resource, "read", None),
                "admin:all should NOT grant read access to tenant resource '{}'",
                resource
            );
            assert!(
                !check_resource_access(&ctx, resource, "create", None),
                "admin:all should NOT grant create access to tenant resource '{}'",
                resource
            );
            assert!(
                !check_resource_access(&ctx, resource, "read", Some("any-team")),
                "admin:all should NOT grant team-scoped read access to tenant resource '{}'",
                resource
            );
            assert!(
                !check_resource_access(&ctx, resource, "create", Some("any-team")),
                "admin:all should NOT grant team-scoped create access to tenant resource '{}'",
                resource
            );
        }
    }

    #[test]
    fn test_admin_allowed_for_governance_resources() {
        let ctx = admin_context();

        let governance_resources = [
            "admin",
            "admin-orgs",
            "admin-users",
            "admin-audit",
            "admin-summary",
            "admin-teams",
            "admin-scopes",
            "admin-apps",
            "admin-filter-schemas",
            "organizations",
            "users",
            "teams",
            "stats",
        ];

        for resource in &governance_resources {
            assert!(
                check_resource_access(&ctx, resource, "read", None),
                "admin:all should grant read access to governance resource '{}'",
                resource
            );
            assert!(
                check_resource_access(&ctx, resource, "create", None),
                "admin:all should grant create access to governance resource '{}'",
                resource
            );
        }
    }

    #[test]
    fn test_is_governance_resource_classification() {
        assert!(is_governance_resource("admin"));
        assert!(is_governance_resource("admin-orgs"));
        assert!(is_governance_resource("admin-users"));
        assert!(is_governance_resource("admin-audit"));
        assert!(is_governance_resource("admin-summary"));
        assert!(is_governance_resource("admin-teams"));
        assert!(is_governance_resource("admin-scopes"));
        assert!(is_governance_resource("admin-apps"));
        assert!(is_governance_resource("admin-filter-schemas"));
        assert!(is_governance_resource("organizations"));
        assert!(is_governance_resource("users"));
        assert!(is_governance_resource("teams"));
        assert!(is_governance_resource("stats"));

        assert!(!is_governance_resource("clusters"));
        assert!(!is_governance_resource("routes"));
        assert!(!is_governance_resource("listeners"));
        assert!(!is_governance_resource("filters"));
        assert!(!is_governance_resource("openapi-import"));
        assert!(!is_governance_resource("api-definitions"));
        assert!(!is_governance_resource("secrets"));
        assert!(!is_governance_resource("proxy-certificates"));
        assert!(!is_governance_resource("custom-wasm-filters"));
        assert!(!is_governance_resource("generate-envoy-config"));
        assert!(!is_governance_resource("dataplanes"));
    }

    // =========================================================
    // Machine-user (agent) branch tests
    // =========================================================

    fn cp_tool_context_with_grants(grants: Vec<Grant>) -> AuthContext {
        AuthContext::with_user(
            crate::domain::TokenId::from_str_unchecked("agent-token"),
            "agent".into(),
            crate::domain::UserId::from_str_unchecked("agent-1"),
            "agent@test.com".into(),
            vec![],
        )
        .with_grants(grants, Some(AgentContext::CpTool))
    }

    #[test]
    fn cp_tool_agent_with_matching_grant_passes() {
        let ctx = cp_tool_context_with_grants(vec![make_grant("clusters", "read", "eng")]);
        assert!(check_resource_access(&ctx, "clusters", "read", Some("eng")));
    }

    #[test]
    fn cp_tool_agent_without_matching_grant_is_denied() {
        let ctx = cp_tool_context_with_grants(vec![make_grant("clusters", "read", "eng")]);
        assert!(!check_resource_access(&ctx, "routes", "read", Some("eng")));
    }

    #[test]
    fn cp_tool_agent_create_grant_does_not_cover_delete() {
        let ctx = cp_tool_context_with_grants(vec![make_grant("clusters", "create", "eng")]);
        assert!(!check_resource_access(&ctx, "clusters", "delete", Some("eng")));
        assert!(check_resource_access(&ctx, "clusters", "create", Some("eng")));
    }

    #[test]
    fn cp_tool_agent_grant_for_wrong_team_is_denied() {
        let ctx = cp_tool_context_with_grants(vec![make_grant("clusters", "read", "eng")]);
        assert!(!check_resource_access(&ctx, "clusters", "read", Some("sales")));
    }

    #[test]
    fn gateway_tool_agent_cannot_access_cp_resources() {
        let ctx = AuthContext::with_user(
            crate::domain::TokenId::from_str_unchecked("gw-agent-token"),
            "gw-agent".into(),
            crate::domain::UserId::from_str_unchecked("gw-1"),
            "gw@test.com".into(),
            vec![],
        )
        .with_grants(vec![], Some(AgentContext::GatewayTool));

        assert!(!check_resource_access(&ctx, "clusters", "read", Some("eng")));
        assert!(!check_resource_access(&ctx, "routes", "create", Some("eng")));
        assert!(!check_resource_access(&ctx, "listeners", "read", None));
    }

    #[test]
    fn api_consumer_agent_cannot_access_cp_resources() {
        let ctx = AuthContext::with_user(
            crate::domain::TokenId::from_str_unchecked("consumer-token"),
            "consumer".into(),
            crate::domain::UserId::from_str_unchecked("consumer-1"),
            "consumer@test.com".into(),
            vec![],
        )
        .with_grants(vec![], Some(AgentContext::ApiConsumer));

        assert!(!check_resource_access(&ctx, "clusters", "read", Some("eng")));
        assert!(!check_resource_access(&ctx, "routes", "read", None));
    }

    #[test]
    fn cp_tool_agent_no_grants_sees_nothing() {
        let ctx = cp_tool_context_with_grants(vec![]);
        assert!(!check_resource_access(&ctx, "clusters", "read", Some("eng")));
        assert!(!check_resource_access(&ctx, "routes", "create", None));
    }

    #[test]
    fn human_user_grant_path() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("human-token"),
            "human".into(),
            vec![],
        )
        .with_grants(vec![make_grant("clusters", "read", "engineering")], None);

        assert!(check_resource_access(&ctx, "clusters", "read", Some("engineering")));
        assert!(!check_resource_access(&ctx, "clusters", "create", Some("engineering")));
    }
}
