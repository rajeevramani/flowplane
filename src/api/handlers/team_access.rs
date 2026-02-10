//! Team Access Verification
//!
//! This module provides unified team-based access control for all resources.
//! It eliminates duplication across handlers by providing a single, generic
//! implementation of team access verification.

use crate::api::error::ApiError;
use crate::api::routes::ApiState;
use crate::auth::authorization::{extract_org_scopes, extract_team_scopes, has_admin_bypass};
use crate::auth::models::AuthContext;
use crate::domain::OrgId;
use crate::storage::repositories::TeamRepository;
use serde::Deserialize;

/// Path parameters for team-scoped operations.
///
/// Shared struct used by secrets, custom_wasm_filters, and mcp_routes handlers
/// to extract the team name from URL path parameters like `/teams/{team}/...`.
#[derive(Debug, Clone, Deserialize)]
pub struct TeamPath {
    pub team: String,
}

/// Trait for resources that belong to a team.
///
/// Implement this trait for any resource type that needs team-based access control.
/// The trait provides the necessary information to verify access and generate
/// appropriate error messages.
pub trait TeamOwned {
    /// Returns the team that owns this resource.
    ///
    /// - `Some(team)` - Resource belongs to a specific team
    /// - `None` - Resource is global (accessible to all teams)
    fn team(&self) -> Option<&str>;

    /// Returns the resource identifier (typically name or id).
    ///
    /// Used in error messages: "Resource with name '{name}' not found"
    fn resource_name(&self) -> &str;

    /// Returns the resource type name for metrics and error messages.
    ///
    /// Examples: "Cluster", "Listener", "Learning session"
    fn resource_type() -> &'static str;

    /// Returns the resource type identifier for metrics tracking.
    ///
    /// Examples: "clusters", "listeners", "learning_sessions"
    fn resource_type_metric() -> &'static str;

    /// Returns the identifier label for error messages.
    ///
    /// Default is "name", override for resources that use "ID" etc.
    fn identifier_label() -> &'static str {
        "name"
    }
}

/// Check if the current context has admin privileges.
///
/// Returns `Ok(())` if the user has `admin:all` scope, `Err(Forbidden)` otherwise.
pub fn require_admin(context: &AuthContext) -> Result<(), ApiError> {
    if !has_admin_bypass(context) {
        return Err(ApiError::forbidden("Admin privileges required"));
    }
    Ok(())
}

/// Default limit for paginated list queries.
///
/// Re-exported from `pagination` module for backward compatibility with
/// existing `#[serde(default = "default_limit")]` usages.
pub use super::pagination::default_limit;

/// Get effective team scopes from auth context.
///
/// This is the single source of truth for extracting team scopes with admin bypass.
/// Admin users (with admin:all scope) get an empty vec, allowing access to all resources.
///
/// # Arguments
/// * `context` - The authentication context from the request
///
/// # Returns
/// * Empty `Vec` for admin users (bypass all team checks)
/// * Team scopes for regular users
pub fn get_effective_team_scopes(context: &AuthContext) -> Vec<String> {
    if has_admin_bypass(context) {
        Vec::new()
    } else {
        extract_team_scopes(context)
    }
}

/// Get effective team IDs (UUIDs) from auth context.
///
/// Like `get_effective_team_scopes()`, but resolves team names to their database UUIDs.
/// This is required after the FK migration where resource tables store team UUIDs
/// instead of team names.
///
/// Admin users still get an empty vec (bypass all team checks).
pub async fn get_effective_team_ids(
    context: &AuthContext,
    team_repo: &dyn TeamRepository,
    org_id: Option<&OrgId>,
) -> Result<Vec<String>, ApiError> {
    let team_names = get_effective_team_scopes(context);
    if team_names.is_empty() {
        return Ok(Vec::new());
    }
    team_repo
        .resolve_team_ids(org_id, &team_names)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to resolve team IDs: {}", e)))
}

/// Resolve a team name to its UUID for database operations.
///
/// After the FK migration, the `team` column stores UUIDs.
/// This converts a user-provided team name to its UUID.
/// If the input is already a UUID, it is returned as-is (idempotent).
pub async fn resolve_team_name(
    state: &ApiState,
    team_name: &str,
    org_id: Option<&OrgId>,
) -> Result<String, ApiError> {
    // If it already looks like a UUID, verify org ownership before passing through
    if uuid::Uuid::parse_str(team_name).is_ok() {
        if let Some(oid) = org_id {
            // Defense-in-depth: verify the UUID belongs to the caller's org
            let team_repo = team_repo_from_state(state)?;
            team_repo
                .resolve_team_names(Some(oid), &[team_name.to_string()])
                .await
                .map_err(|_| ApiError::NotFound(format!("Team '{}' not found", team_name)))?;
        }
        return Ok(team_name.to_string());
    }
    let team_repo = team_repo_from_state(state)?;
    let ids = team_repo
        .resolve_team_ids(org_id, &[team_name.to_string()])
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to resolve team: {}", e)))?;
    ids.into_iter()
        .next()
        .ok_or_else(|| ApiError::NotFound(format!("Team '{}' not found", team_name)))
}

/// Resolve a team identifier to its name for authorization checks.
///
/// If the input is already a name (not a UUID), returns it as-is.
/// If the input is a UUID, looks up the team and returns its name.
/// This ensures `require_resource_access` always receives team names
/// (matching the `team:<name>:resource:action` scope pattern).
pub async fn resolve_team_id_to_name(
    state: &ApiState,
    team: &str,
    org_id: Option<&OrgId>,
) -> Result<String, ApiError> {
    // If it doesn't look like a UUID, assume it's already a name
    if uuid::Uuid::parse_str(team).is_err() {
        return Ok(team.to_string());
    }
    let team_repo = team_repo_from_state(state)?;
    let names = team_repo
        .resolve_team_names(org_id, &[team.to_string()])
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to resolve team ID to name: {}", e)))?;
    names
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::NotFound(format!("Team with ID '{}' not found", team)))
}

/// Require resource access with automatic UUID-to-name resolution.
///
/// This is the async counterpart of `require_resource_access` that handles
/// the case where the `team` parameter might be a UUID instead of a name.
/// After the FK migration, external callers or bookmarked URLs may send
/// UUIDs, but scopes use team names. This function resolves the mismatch.
pub async fn require_resource_access_resolved(
    state: &ApiState,
    context: &crate::auth::models::AuthContext,
    resource: &str,
    action: &str,
    team: Option<&str>,
    org_id: Option<&OrgId>,
) -> Result<(), ApiError> {
    let resolved_team = match team {
        Some(t) => Some(resolve_team_id_to_name(state, t, org_id).await?),
        None => None,
    };
    crate::auth::authorization::require_resource_access(
        context,
        resource,
        action,
        resolved_team.as_deref(),
    )
    .map_err(|_| ApiError::Forbidden("Access denied".to_string()))
}

/// Get the database pool from ApiState.
pub fn get_db_pool(state: &ApiState) -> Result<std::sync::Arc<crate::storage::DbPool>, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Database not available"))?;
    Ok(std::sync::Arc::new(cluster_repo.pool().clone()))
}

/// Get team repository from ApiState.
pub fn team_repo_from_state(
    state: &ApiState,
) -> Result<&crate::storage::repositories::team::SqlxTeamRepository, ApiError> {
    state
        .xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Team repository unavailable"))
}

/// Get effective team scopes with org admin expansion.
///
/// For org admins (users with `org:{name}:admin` scope), this expands their team list
/// to include ALL teams in their organization. This allows org admins to manage
/// resources across all teams within their org without explicit team memberships.
///
/// Falls back to `get_effective_team_scopes()` for non-org-admin users and platform admins.
pub async fn get_effective_team_scopes_with_org(
    context: &AuthContext,
    team_repo: &dyn TeamRepository,
) -> Vec<String> {
    // Platform admin bypasses all checks
    if has_admin_bypass(context) {
        return Vec::new();
    }

    let mut teams: std::collections::HashSet<String> =
        extract_team_scopes(context).into_iter().collect();

    // Expand org admin scopes to include all teams in their org(s)
    let org_scopes = extract_org_scopes(context);
    for (org_name, role) in &org_scopes {
        if role == "admin" {
            if let Some(org_id) = &context.org_id {
                if let Ok(org_teams) = team_repo.list_teams_by_org(org_id).await {
                    for team in org_teams {
                        teams.insert(team.name);
                    }
                }
            }
            // Even without org_id in context, the org_name match gives us
            // the scope; team names will be picked up from their memberships
            let _ = org_name;
        }
    }

    teams.into_iter().collect()
}

/// Verify that a resource belongs to one of the user's teams or is global.
///
/// This is the unified access verification function that replaces all the
/// duplicate `verify_*_access` functions across handlers.
///
/// # Behavior
/// - Returns `Ok(resource)` if:
///   - `team_scopes` is empty (admin bypass)
///   - Resource has no team (global resource)
///   - Resource's team is in `team_scopes`
/// - Returns `Err(NotFound)` if resource's team is not in `team_scopes`
/// - Records cross-team access attempts for security monitoring
///
/// # Arguments
/// * `resource` - The resource to verify access for
/// * `team_scopes` - The user's team scopes (empty for admin bypass)
///
/// # Returns
/// * `Ok(resource)` if access is allowed
/// * `Err(ApiError::NotFound)` if access is denied (to avoid leaking existence)
pub async fn verify_team_access<T: TeamOwned>(
    resource: T,
    team_scopes: &[String],
) -> Result<T, ApiError> {
    // Admin:all or resource-level scopes (empty team_scopes) can access everything
    if team_scopes.is_empty() {
        return Ok(resource);
    }

    // Get the resource's team
    match resource.team() {
        // Global resource (team = NULL) - accessible to all
        None => Ok(resource),

        // Team-owned resource - verify membership
        Some(resource_team) => {
            if team_scopes.iter().any(|scope| scope == resource_team) {
                Ok(resource)
            } else {
                // Record cross-team access attempt for security monitoring
                if let Some(from_team) = team_scopes.first() {
                    crate::observability::metrics::record_cross_team_access_attempt(
                        from_team,
                        resource_team,
                        T::resource_type_metric(),
                    )
                    .await;
                }

                // Return 404 to avoid leaking existence of other teams' resources
                Err(ApiError::NotFound(format!(
                    "{} with {} '{}' not found",
                    T::resource_type(),
                    T::identifier_label(),
                    resource.resource_name()
                )))
            }
        }
    }
}

/// Verify that a team belongs to the same org as the user for cross-org isolation.
///
/// This prevents users from one org from adding memberships to teams in another org.
/// Platform admins bypass this check.
///
/// # Arguments
/// * `team_org_id` - The org_id of the target team (None for global teams)
/// * `user_org_id` - The org_id of the current user (None for unscoped users)
/// * `is_admin` - Whether the user is a platform admin
///
/// # Returns
/// * `Ok(())` if access is allowed
/// * `Err(ApiError::Forbidden)` if the user is trying to access a team in a different org
pub fn verify_same_org(
    team_org_id: Option<&crate::domain::OrgId>,
    user_org_id: Option<&crate::domain::OrgId>,
    is_admin: bool,
) -> Result<(), ApiError> {
    // Platform admin can access any org's teams
    if is_admin {
        return Ok(());
    }

    match (team_org_id, user_org_id) {
        // Both have orgs - must match
        (Some(team_org), Some(user_org)) => {
            if team_org == user_org {
                Ok(())
            } else {
                tracing::warn!(
                    attempted_org = %team_org,
                    user_org = %user_org,
                    "cross-org access violation detected"
                );
                Err(ApiError::Forbidden(format!(
                    "Cross-organization access denied: user org '{}' cannot access resources in org '{}'",
                    user_org, team_org
                )))
            }
        }
        // Team has org but user doesn't - deny
        (Some(team_org), None) => {
            tracing::warn!(
                attempted_org = %team_org,
                "cross-org access violation: unscoped user accessing org-scoped team"
            );
            Err(ApiError::Forbidden(format!(
                "Cross-organization access denied: unscoped user cannot access resources in org '{}'",
                team_org
            )))
        }
        // Team has no org (global) or user has no org - allow
        // Global teams are accessible to all; unscoped users accessing unscoped teams is fine
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test resource for unit tests
    #[derive(Debug, Clone)]
    struct TestResource {
        name: String,
        team: Option<String>,
    }

    impl TeamOwned for TestResource {
        fn team(&self) -> Option<&str> {
            self.team.as_deref()
        }

        fn resource_name(&self) -> &str {
            &self.name
        }

        fn resource_type() -> &'static str {
            "Test resource"
        }

        fn resource_type_metric() -> &'static str {
            "test_resources"
        }
    }

    /// Test resource with non-optional team (like SecretData)
    #[derive(Debug, Clone)]
    struct TestSecretResource {
        name: String,
        team: String,
    }

    impl TeamOwned for TestSecretResource {
        fn team(&self) -> Option<&str> {
            Some(&self.team)
        }

        fn resource_name(&self) -> &str {
            &self.name
        }

        fn resource_type() -> &'static str {
            "Secret"
        }

        fn resource_type_metric() -> &'static str {
            "secrets"
        }
    }

    /// Test resource with ID instead of name (like LearningSessionData)
    #[derive(Debug, Clone)]
    struct TestSessionResource {
        id: String,
        team: String,
    }

    impl TeamOwned for TestSessionResource {
        fn team(&self) -> Option<&str> {
            Some(&self.team)
        }

        fn resource_name(&self) -> &str {
            &self.id
        }

        fn resource_type() -> &'static str {
            "Learning session"
        }

        fn resource_type_metric() -> &'static str {
            "learning_sessions"
        }

        fn identifier_label() -> &'static str {
            "ID"
        }
    }

    #[tokio::test]
    async fn test_admin_bypass_allows_access() {
        let resource =
            TestResource { name: "test-cluster".to_string(), team: Some("other-team".to_string()) };

        // Empty team_scopes = admin bypass
        let result = verify_team_access(resource, &[]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_global_resource_accessible_to_all() {
        let resource = TestResource {
            name: "global-cluster".to_string(),
            team: None, // Global resource
        };

        let team_scopes = vec!["team-a".to_string()];
        let result = verify_team_access(resource, &team_scopes).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_same_team_allows_access() {
        let resource =
            TestResource { name: "team-cluster".to_string(), team: Some("team-a".to_string()) };

        let team_scopes = vec!["team-a".to_string(), "team-b".to_string()];
        let result = verify_team_access(resource, &team_scopes).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_different_team_denies_access() {
        let resource =
            TestResource { name: "secret-cluster".to_string(), team: Some("team-x".to_string()) };

        let team_scopes = vec!["team-a".to_string()];
        let result = verify_team_access(resource, &team_scopes).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_error_message_format() {
        let resource =
            TestResource { name: "my-cluster".to_string(), team: Some("team-x".to_string()) };

        let team_scopes = vec!["team-a".to_string()];
        let result = verify_team_access(resource, &team_scopes).await;

        let err = result.unwrap_err();
        if let ApiError::NotFound(msg) = err {
            assert_eq!(msg, "Test resource with name 'my-cluster' not found");
        } else {
            panic!("Expected NotFound error");
        }
    }

    #[tokio::test]
    async fn test_secret_resource_with_required_team() {
        let resource =
            TestSecretResource { name: "my-secret".to_string(), team: "team-a".to_string() };

        let team_scopes = vec!["team-a".to_string()];
        let result = verify_team_access(resource, &team_scopes).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_session_resource_with_id_label() {
        let resource =
            TestSessionResource { id: "session-123".to_string(), team: "team-x".to_string() };

        let team_scopes = vec!["team-a".to_string()];
        let result = verify_team_access(resource, &team_scopes).await;

        let err = result.unwrap_err();
        if let ApiError::NotFound(msg) = err {
            assert_eq!(msg, "Learning session with ID 'session-123' not found");
        } else {
            panic!("Expected NotFound error");
        }
    }

    #[tokio::test]
    async fn test_multiple_team_scopes() {
        let resource = TestResource {
            name: "multi-team-cluster".to_string(),
            team: Some("team-c".to_string()),
        };

        // User has access to multiple teams, including team-c
        let team_scopes = vec!["team-a".to_string(), "team-b".to_string(), "team-c".to_string()];
        let result = verify_team_access(resource, &team_scopes).await;
        assert!(result.is_ok());
    }

    // === Cross-org isolation tests ===

    #[test]
    fn test_verify_same_org_admin_bypass() {
        let org_a = crate::domain::OrgId::new();
        let org_b = crate::domain::OrgId::new();
        // Admin can access any org's teams
        assert!(verify_same_org(Some(&org_a), Some(&org_b), true).is_ok());
    }

    #[test]
    fn test_verify_same_org_matching_orgs() {
        let org = crate::domain::OrgId::new();
        assert!(verify_same_org(Some(&org), Some(&org), false).is_ok());
    }

    #[test]
    fn test_verify_same_org_different_orgs() {
        let org_a = crate::domain::OrgId::new();
        let org_b = crate::domain::OrgId::new();
        let result = verify_same_org(Some(&org_a), Some(&org_b), false);
        assert!(result.is_err());
        if let Err(ApiError::Forbidden(msg)) = result {
            assert!(msg.contains("Cross-organization"));
        } else {
            panic!("Expected Forbidden error");
        }
    }

    #[test]
    fn test_verify_same_org_team_has_org_user_doesnt() {
        let org = crate::domain::OrgId::new();
        let result = verify_same_org(Some(&org), None, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_same_org_global_team() {
        let user_org = crate::domain::OrgId::new();
        // Global team (no org) is accessible to all
        assert!(verify_same_org(None, Some(&user_org), false).is_ok());
    }

    #[test]
    fn test_verify_same_org_both_none() {
        // Unscoped user accessing unscoped team - fine
        assert!(verify_same_org(None, None, false).is_ok());
    }
}
