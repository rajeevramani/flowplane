//! Auth session endpoint — returns DB-sourced session info for the authenticated user.
//!
//! This replaces the Phase 2 pattern where the frontend parsed Zitadel JWT role claims
//! to determine scopes, teams, and org context. In Auth v3, the JWT is identity-only
//! and all permissions come from the Flowplane DB.

use axum::{Extension, Json};
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::models::AuthContext;

/// A single grant summarized for the session response.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrantSummary {
    pub team_name: String,
    pub resource_type: String,
    pub action: String,
}

/// Response from `GET /api/v1/auth/session`.
///
/// Returns the authenticated user's DB-sourced permissions and identity info.
/// The frontend uses this to determine menu visibility, role badges, and org context.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuthSessionResponse {
    pub user_id: String,
    pub email: String,
    pub name: String,
    pub is_admin: bool,
    pub is_platform_admin: bool,
    pub org_scopes: Vec<String>,
    pub grants: Vec<GrantSummary>,
    pub teams: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_role: Option<String>,
}

/// `GET /api/v1/auth/session` — return the authenticated user's session info.
///
/// The auth middleware has already validated the JWT, JIT-provisioned the user,
/// loaded permissions from the DB, and built the `AuthContext`. This handler
/// just serializes it into the shape the frontend expects.
#[utoipa::path(
    get,
    path = "/api/v1/auth/session",
    tag = "auth",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Session info", body = AuthSessionResponse),
        (status = 401, description = "Not authenticated"),
    )
)]
pub async fn auth_session_handler(
    Extension(context): Extension<AuthContext>,
) -> Json<AuthSessionResponse> {
    let org_scopes: Vec<String> = context.org_scopes().cloned().collect();
    let is_platform_admin = context.has_scope("admin:all");

    // Teams come from resource grants.
    let teams = context.grant_team_names();

    // Derive org_role from org_scopes using org_name.
    let org_role = context.org_name.as_ref().and_then(|name| {
        org_scopes.iter().find_map(|s| {
            let rest = s.strip_prefix("org:")?;
            let parts: Vec<&str> = rest.splitn(2, ':').collect();
            if parts.len() == 2 && parts[0] == name {
                Some(parts[1].to_string())
            } else {
                None
            }
        })
    });

    // Build grant summaries for resource grants.
    let grants: Vec<GrantSummary> = context
        .grants
        .iter()
        .filter(|g| g.grant_type == crate::auth::models::GrantType::Resource)
        .filter_map(|g| {
            Some(GrantSummary {
                team_name: g.team_name.clone(),
                resource_type: g.resource_type.clone()?,
                action: g.action.clone()?,
            })
        })
        .collect();

    let email = context.user_email.clone().unwrap_or_default();
    // Use OIDC profile name if available, fall back to email prefix.
    let name = context
        .user_name
        .clone()
        .unwrap_or_else(|| email.split('@').next().unwrap_or("").to_string());

    Json(AuthSessionResponse {
        user_id: context.user_id.as_ref().map(|id| id.to_string()).unwrap_or_default(),
        email,
        name,
        is_admin: is_platform_admin,
        is_platform_admin,
        org_scopes,
        grants,
        teams,
        org_id: context.org_id.as_ref().map(|id| id.to_string()),
        org_name: context.org_name.clone(),
        org_role,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::models::{Grant, GrantType};
    use crate::domain::{OrgId, TokenId};

    fn make_context(org_scopes: Vec<&str>) -> AuthContext {
        AuthContext::with_user(
            TokenId::from_string("zitadel:test-sub".to_string()),
            "zitadel/test-sub".to_string(),
            crate::domain::UserId::from_string("user-1".to_string()),
            "admin@flowplane.local".to_string(),
            org_scopes.into_iter().map(String::from).collect(),
        )
    }

    fn make_org_context(org_scopes: Vec<&str>, org_id: &str, org_name: &str) -> AuthContext {
        make_context(org_scopes)
            .with_org(OrgId::from_string(org_id.to_string()), org_name.to_string())
    }

    fn team_grant(team_name: &str, resource: &str, action: &str) -> Grant {
        Grant {
            grant_type: GrantType::Resource,
            team_id: format!("team-{}", team_name),
            team_name: team_name.to_string(),
            resource_type: Some(resource.to_string()),
            action: Some(action.to_string()),
            route_id: None,
            allowed_methods: vec![],
        }
    }

    #[tokio::test]
    async fn platform_admin_session() {
        let ctx = make_context(vec!["admin:all", "org:platform:admin"]);
        let Json(resp) = auth_session_handler(Extension(ctx)).await;

        assert!(resp.is_platform_admin);
        assert!(resp.is_admin);
        assert!(resp.org_scopes.contains(&"admin:all".to_string()));
        assert_eq!(resp.email, "admin@flowplane.local");
        assert!(resp.teams.is_empty());
        // Platform org is excluded from org context (no with_org call)
        assert!(resp.org_name.is_none());
        assert!(resp.org_id.is_none());
    }

    #[tokio::test]
    async fn org_admin_session() {
        let ctx = make_org_context(vec!["org:acme-corp:admin"], "org-acme-id", "acme-corp")
            .with_grants(
                vec![team_grant("engineering", "*", "*"), team_grant("payments", "*", "*")],
                None,
            );
        let Json(resp) = auth_session_handler(Extension(ctx)).await;

        assert!(!resp.is_platform_admin);
        assert_eq!(resp.org_id.as_deref(), Some("org-acme-id"));
        assert_eq!(resp.org_name.as_deref(), Some("acme-corp"));
        assert_eq!(resp.org_role.as_deref(), Some("admin"));
        assert_eq!(resp.teams, vec!["engineering", "payments"]);
    }

    #[tokio::test]
    async fn team_member_session() {
        let ctx = make_org_context(vec!["org:acme-corp:member"], "org-acme-id", "acme-corp")
            .with_grants(
                vec![
                    team_grant("engineering", "clusters", "read"),
                    team_grant("engineering", "routes", "create"),
                ],
                None,
            );
        let Json(resp) = auth_session_handler(Extension(ctx)).await;

        assert!(!resp.is_platform_admin);
        assert_eq!(resp.org_id.as_deref(), Some("org-acme-id"));
        assert_eq!(resp.org_name.as_deref(), Some("acme-corp"));
        assert_eq!(resp.org_role.as_deref(), Some("member"));
        assert_eq!(resp.teams, vec!["engineering"]);
    }

    #[tokio::test]
    async fn session_uses_oidc_name_when_available() {
        let ctx = make_context(vec![]).with_user_name("Jane Doe".to_string());
        let Json(resp) = auth_session_handler(Extension(ctx)).await;

        assert_eq!(resp.name, "Jane Doe");
        assert_eq!(resp.email, "admin@flowplane.local");
    }

    #[tokio::test]
    async fn session_falls_back_to_email_prefix_when_no_name() {
        let ctx = make_context(vec![]);
        let Json(resp) = auth_session_handler(Extension(ctx)).await;

        // No user_name set, should fall back to email prefix
        assert_eq!(resp.name, "admin");
        assert_eq!(resp.email, "admin@flowplane.local");
    }

    #[tokio::test]
    async fn no_permissions_session() {
        let ctx = make_context(vec![]);
        let Json(resp) = auth_session_handler(Extension(ctx)).await;

        assert!(!resp.is_platform_admin);
        assert!(resp.org_scopes.is_empty());
        assert!(resp.teams.is_empty());
        assert!(resp.org_name.is_none());
        assert!(resp.org_id.is_none());
    }
}
