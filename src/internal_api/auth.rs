//! Internal API Authentication Context
//!
//! This module provides unified authentication context for internal API operations.
//! It abstracts the differences between REST AuthContext and MCP team strings.

use crate::api::handlers::team_access::{
    get_effective_team_scopes, get_effective_team_scopes_with_org, TeamOwned,
};
use crate::auth::authorization::has_admin_bypass;
use crate::auth::models::AuthContext;
use crate::domain::OrgId;
use crate::internal_api::error::InternalError;
use crate::observability::metrics::record_cross_team_access_attempt;
use crate::storage::repositories::TeamRepository;

/// Internal authentication context for API operations
///
/// This provides a unified view of authentication state that works for both
/// REST handlers (which have full AuthContext) and MCP tools (which have team strings).
#[derive(Debug, Clone)]
pub struct InternalAuthContext {
    /// The primary team for this request (used for resource creation)
    pub team: Option<String>,
    /// Whether this context has admin bypass (empty allowed_teams = admin)
    pub is_admin: bool,
    /// Teams the user can access
    pub allowed_teams: Vec<String>,
    /// Organization ID for this user (if org-scoped)
    pub org_id: Option<OrgId>,
    /// Organization name for this user (if org-scoped)
    pub org_name: Option<String>,
}

impl InternalAuthContext {
    /// Create an internal auth context from a REST AuthContext.
    ///
    /// `is_admin` is set for users with `admin:all` scope. This flag is for
    /// governance operations only (org/user/audit management). It does NOT
    /// grant access to tenant resources — admin users need explicit team scopes.
    pub fn from_rest(context: &AuthContext) -> Self {
        let allowed_teams = get_effective_team_scopes(context);
        let is_admin = has_admin_bypass(context);

        // Admin users get team from their explicit memberships (not special-cased)
        let team = allowed_teams.first().cloned();

        Self {
            team,
            is_admin,
            allowed_teams,
            org_id: context.org_id.clone(),
            org_name: context.org_name.clone(),
        }
    }

    /// Create an internal auth context from a REST AuthContext with org-aware team expansion.
    ///
    /// Unlike `from_rest`, this expands org admin scopes to include ALL teams in their
    /// organization. This ensures org admins can list/access resources across all org teams,
    /// not just teams they're explicitly members of.
    ///
    /// Admin users (admin:all) get only their explicit team memberships —
    /// admin:all is governance-only and does NOT grant implicit access to all teams.
    pub async fn from_rest_with_org(context: &AuthContext, team_repo: &dyn TeamRepository) -> Self {
        let allowed_teams = get_effective_team_scopes_with_org(context, team_repo).await;
        let is_admin = has_admin_bypass(context);

        let team = allowed_teams.first().cloned();

        Self {
            team,
            is_admin,
            allowed_teams,
            org_id: context.org_id.clone(),
            org_name: context.org_name.clone(),
        }
    }

    /// Create an internal auth context from MCP team string with optional org context.
    ///
    /// Two invocation paths:
    /// - **HTTP MCP** (user-facing): `org_id` is extracted from the authenticated user's
    ///   JWT/session context and passed through `McpHandler::with_xds_state`. Team resolution
    ///   and resource access are org-scoped.
    /// - **CLI MCP** (internal): `org_id` is `None` because CLI users have direct machine
    ///   access with `admin:all` scopes. No org isolation needed for local admin.
    pub fn from_mcp(team: &str, org_id: Option<OrgId>, org_name: Option<String>) -> Self {
        let is_admin = team.is_empty();
        let allowed_teams = if is_admin { Vec::new() } else { vec![team.to_string()] };
        let team = if is_admin { None } else { Some(team.to_string()) };

        Self { team, is_admin, allowed_teams, org_id, org_name }
    }

    /// Create an admin context (for governance operations only).
    ///
    /// This context has `is_admin=true` for governance access but no team memberships.
    /// It cannot access tenant resources — use `for_team()` for team-scoped access.
    pub fn admin() -> Self {
        Self { team: None, is_admin: true, allowed_teams: Vec::new(), org_id: None, org_name: None }
    }

    /// Create a team-scoped context
    pub fn for_team(team: impl Into<String>) -> Self {
        let team_str = team.into();
        Self {
            team: Some(team_str.clone()),
            is_admin: false,
            allowed_teams: vec![team_str],
            org_id: None,
            org_name: None,
        }
    }

    /// Create a multi-team context (e.g., for org admins who can access multiple teams).
    ///
    /// The first team in the list becomes the primary team.
    pub fn for_teams(teams: Vec<String>) -> Self {
        let team = teams.first().cloned();
        Self { team, is_admin: false, allowed_teams: teams, org_id: None, org_name: None }
    }

    /// Resolve team names to UUIDs using the team repository.
    ///
    /// Converts `allowed_teams` from team names (extracted from auth scopes)
    /// to team UUIDs (used in database queries after FK migration).
    /// Admin contexts (empty allowed_teams) pass through unchanged.
    /// Idempotent: if teams are already UUIDs, returns unchanged.
    pub async fn resolve_teams(
        mut self,
        team_repo: &dyn TeamRepository,
    ) -> Result<Self, InternalError> {
        if !self.allowed_teams.is_empty() {
            // Skip if already resolved (all values are UUIDs)
            if self.allowed_teams.iter().all(|t| uuid::Uuid::parse_str(t).is_ok()) {
                return Ok(self);
            }
            self.allowed_teams = team_repo
                .resolve_team_ids(self.org_id.as_ref(), &self.allowed_teams)
                .await
                .map_err(|e| {
                    InternalError::internal(format!("Failed to resolve team IDs: {}", e))
                })?;
            self.team = self.allowed_teams.first().cloned();
        }
        Ok(self)
    }

    /// Check if this context can access a team-owned resource.
    ///
    /// Admin (`is_admin`) does NOT bypass this check — admin:all is governance-only.
    /// Admin users need explicit team memberships to access tenant resources.
    pub fn can_access_team(&self, resource_team: Option<&str>) -> bool {
        // Global resources (no team) are accessible to all
        let Some(team) = resource_team else {
            return true;
        };

        // Check if user has access to the resource's team
        self.allowed_teams.iter().any(|t| t == team)
    }

    /// Check if this context can create resources for a team.
    ///
    /// Admin (`is_admin`) does NOT bypass this check — admin:all is governance-only.
    /// Admin users need explicit team memberships to create tenant resources.
    pub fn can_create_for_team(&self, target_team: Option<&str>) -> bool {
        // Must have a target team
        let Some(team) = target_team else {
            return false;
        };

        // Check if user has access to the target team
        self.allowed_teams.iter().any(|t| t == team)
    }
}

/// Verify that a resource belongs to one of the user's teams or is global.
///
/// This is the internal API equivalent of the REST `verify_team_access` function.
/// Returns `Ok(resource)` if access is allowed, or `Err(NotFound)` to hide existence.
///
/// Admin (`is_admin`) does NOT bypass this check — admin:all is governance-only.
pub async fn verify_team_access<T: TeamOwned>(
    resource: T,
    auth: &InternalAuthContext,
) -> Result<T, InternalError> {
    // Get the resource's team
    match resource.team() {
        // Global resource (team = NULL) - accessible to all
        None => Ok(resource),

        // Team-owned resource - verify membership
        Some(resource_team) => {
            if auth.allowed_teams.iter().any(|scope| scope == resource_team) {
                Ok(resource)
            } else {
                // Record cross-team access attempt for security monitoring
                if let Some(from_team) = auth.allowed_teams.first() {
                    record_cross_team_access_attempt(
                        from_team,
                        resource_team,
                        T::resource_type_metric(),
                    )
                    .await;
                }

                // Return 404 to avoid leaking existence of other teams' resources
                Err(InternalError::not_found(T::resource_type(), resource.resource_name()))
            }
        }
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

    #[test]
    fn test_admin_context() {
        let ctx = InternalAuthContext::admin();
        assert!(ctx.is_admin);
        assert!(ctx.team.is_none());
        assert!(ctx.allowed_teams.is_empty());
    }

    #[test]
    fn test_team_context() {
        let ctx = InternalAuthContext::for_team("team-a");
        assert!(!ctx.is_admin);
        assert_eq!(ctx.team, Some("team-a".to_string()));
        assert_eq!(ctx.allowed_teams, vec!["team-a".to_string()]);
    }

    #[test]
    fn test_for_teams_context() {
        let ctx = InternalAuthContext::for_teams(vec!["team-a".into(), "team-b".into()]);
        assert!(!ctx.is_admin);
        assert_eq!(ctx.team, Some("team-a".to_string()));
        assert_eq!(ctx.allowed_teams, vec!["team-a".to_string(), "team-b".to_string()]);
        assert!(ctx.can_access_team(Some("team-a")));
        assert!(ctx.can_access_team(Some("team-b")));
        assert!(!ctx.can_access_team(Some("team-c")));
    }

    #[test]
    fn test_from_mcp_empty_team() {
        let ctx = InternalAuthContext::from_mcp("", None, None);
        assert!(ctx.is_admin);
        assert!(ctx.team.is_none());
    }

    #[test]
    fn test_from_mcp_with_team() {
        let ctx = InternalAuthContext::from_mcp("team-b", None, None);
        assert!(!ctx.is_admin);
        assert_eq!(ctx.team, Some("team-b".to_string()));
    }

    #[test]
    fn test_from_mcp_with_org_context() {
        let org_id = OrgId::new();
        let ctx =
            InternalAuthContext::from_mcp("team-c", Some(org_id.clone()), Some("acme".into()));
        assert!(!ctx.is_admin);
        assert_eq!(ctx.org_id, Some(org_id));
        assert_eq!(ctx.org_name, Some("acme".into()));
    }

    #[test]
    fn test_can_access_team_admin_denied_for_resources() {
        let ctx = InternalAuthContext::admin();
        // Admin (governance-only) cannot access team resources without explicit membership
        assert!(!ctx.can_access_team(Some("any-team")));
        // Global resources (no team) are still accessible
        assert!(ctx.can_access_team(None));
    }

    #[test]
    fn test_can_access_team_global_resource() {
        let ctx = InternalAuthContext::for_team("team-a");
        assert!(ctx.can_access_team(None)); // Global resource
    }

    #[test]
    fn test_can_access_team_same_team() {
        let ctx = InternalAuthContext::for_team("team-a");
        assert!(ctx.can_access_team(Some("team-a")));
    }

    #[test]
    fn test_can_access_team_different_team() {
        let ctx = InternalAuthContext::for_team("team-a");
        assert!(!ctx.can_access_team(Some("team-b")));
    }

    #[test]
    fn test_can_create_for_team_admin_denied() {
        let ctx = InternalAuthContext::admin();
        // Admin (governance-only) cannot create resources for any team
        assert!(!ctx.can_create_for_team(Some("any-team")));
        assert!(!ctx.can_create_for_team(None));
    }

    #[test]
    fn test_can_create_for_team_same_team() {
        let ctx = InternalAuthContext::for_team("team-a");
        assert!(ctx.can_create_for_team(Some("team-a")));
    }

    #[test]
    fn test_can_create_for_team_different_team() {
        let ctx = InternalAuthContext::for_team("team-a");
        assert!(!ctx.can_create_for_team(Some("team-b")));
    }

    #[test]
    fn test_can_create_for_team_no_team() {
        // Non-admin users cannot create global resources
        let ctx = InternalAuthContext::for_team("team-a");
        assert!(!ctx.can_create_for_team(None));
    }

    #[test]
    fn test_from_rest_non_admin_with_empty_teams_is_not_admin() {
        // An org member with no team scopes should NOT be treated as admin
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("org-member"),
            "org-member".into(),
            vec!["org:acme:member".into()],
        );

        let internal = InternalAuthContext::from_rest(&ctx);
        assert!(!internal.is_admin, "org member with no team scopes must not be admin");
        assert!(internal.allowed_teams.is_empty());
    }

    #[test]
    fn test_from_rest_admin_is_admin() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("admin"),
            "admin".into(),
            vec!["admin:all".into()],
        );

        let internal = InternalAuthContext::from_rest(&ctx);
        assert!(internal.is_admin, "admin:all user must be admin (governance flag)");
        assert!(internal.allowed_teams.is_empty(), "admin with no team memberships gets no teams");
        assert!(internal.team.is_none(), "admin with no team memberships has no primary team");
    }

    #[test]
    fn test_from_rest_admin_with_teams() {
        let ctx = AuthContext::new(
            crate::domain::TokenId::from_str_unchecked("admin-with-teams"),
            "admin".into(),
            vec!["admin:all".into(), "team:platform-admin:*:*".into()],
        );

        let internal = InternalAuthContext::from_rest(&ctx);
        assert!(internal.is_admin);
        // Admin gets their team scopes extracted (not special-cased)
        assert_eq!(internal.allowed_teams, vec!["platform-admin".to_string()]);
        assert_eq!(internal.team, Some("platform-admin".to_string()));
        // Admin can access their explicit team membership
        assert!(internal.can_access_team(Some("platform-admin")));
        // Admin cannot access other teams (governance-only, no bypass)
        assert!(!internal.can_access_team(Some("other-team")));
    }

    #[tokio::test]
    async fn test_verify_team_access_admin_denied_for_resources() {
        let resource = TestResource {
            name: "test-resource".to_string(),
            team: Some("other-team".to_string()),
        };
        let auth = InternalAuthContext::admin();

        // Admin (governance-only) cannot access team resources without explicit membership
        let result = verify_team_access(resource, &auth).await;
        assert!(result.is_err(), "admin should be denied access to team resources");
    }

    #[tokio::test]
    async fn test_verify_team_access_admin_can_access_global() {
        let resource = TestResource { name: "global-resource".to_string(), team: None };
        let auth = InternalAuthContext::admin();

        // Admin can access global (no team) resources
        let result = verify_team_access(resource, &auth).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_verify_team_access_global_resource() {
        let resource = TestResource { name: "global-resource".to_string(), team: None };
        let auth = InternalAuthContext::for_team("team-a");

        let result = verify_team_access(resource, &auth).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_verify_team_access_same_team() {
        let resource =
            TestResource { name: "team-resource".to_string(), team: Some("team-a".to_string()) };
        let auth = InternalAuthContext::for_team("team-a");

        let result = verify_team_access(resource, &auth).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_verify_team_access_different_team() {
        let resource =
            TestResource { name: "secret-resource".to_string(), team: Some("team-b".to_string()) };
        let auth = InternalAuthContext::for_team("team-a");

        let result = verify_team_access(resource, &auth).await;
        assert!(result.is_err());

        let err = result.unwrap_err();
        match err {
            InternalError::NotFound { resource, id } => {
                assert_eq!(resource, "Test resource");
                assert_eq!(id, "secret-resource");
            }
            _ => panic!("Expected NotFound error"),
        }
    }
}
