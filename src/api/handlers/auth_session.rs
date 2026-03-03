//! Auth session endpoint — returns DB-sourced session info for the authenticated user.
//!
//! This replaces the Phase 2 pattern where the frontend parsed Zitadel JWT role claims
//! to determine scopes, teams, and org context. In Auth v3, the JWT is identity-only
//! and all permissions come from the Flowplane DB.

use axum::{Extension, Json};
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::models::AuthContext;

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
    pub scopes: Vec<String>,
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
    let scopes: Vec<String> = context.scopes().cloned().collect();
    let is_platform_admin = scopes.iter().any(|s| s == "admin:all");

    // Extract unique team names from team-scoped permissions.
    // Format: "team:{name}:{resource}:{action}" or "team:{name}:*:*"
    let mut teams: Vec<String> = scopes
        .iter()
        .filter_map(|s| {
            let rest = s.strip_prefix("team:")?;
            let team_name = rest.split(':').next()?;
            Some(team_name.to_string())
        })
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    teams.sort();

    // Org context is populated by the auth middleware from the DB.
    // Derive org_role from scopes using org_name (the middleware sets org_id/org_name
    // but not the role string, so we still parse it from scopes).
    let org_role = context.org_name.as_ref().and_then(|name| {
        scopes.iter().find_map(|s| {
            let rest = s.strip_prefix("org:")?;
            let parts: Vec<&str> = rest.splitn(2, ':').collect();
            if parts.len() == 2 && parts[0] == name {
                Some(parts[1].to_string())
            } else {
                None
            }
        })
    });

    let email = context.user_email.clone().unwrap_or_default();
    // Use email prefix as display name (the OIDC profile name isn't available here,
    // but the middleware stores the JIT-provisioned name in the user row — the email
    // is a reasonable fallback).
    let name = email.split('@').next().unwrap_or("").to_string();

    Json(AuthSessionResponse {
        user_id: context.user_id.as_ref().map(|id| id.to_string()).unwrap_or_default(),
        email,
        name,
        is_admin: is_platform_admin,
        is_platform_admin,
        scopes,
        teams,
        org_id: context.org_id.as_ref().map(|id| id.to_string()),
        org_name: context.org_name.clone(),
        org_role,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{OrgId, TokenId};

    fn make_context(scopes: Vec<&str>) -> AuthContext {
        AuthContext::with_user(
            TokenId::from_string("zitadel:test-sub".to_string()),
            "zitadel/test-sub".to_string(),
            crate::domain::UserId::from_string("user-1".to_string()),
            "admin@flowplane.local".to_string(),
            scopes.into_iter().map(String::from).collect(),
        )
    }

    fn make_org_context(scopes: Vec<&str>, org_id: &str, org_name: &str) -> AuthContext {
        make_context(scopes).with_org(OrgId::from_string(org_id.to_string()), org_name.to_string())
    }

    #[tokio::test]
    async fn platform_admin_session() {
        let ctx = make_context(vec!["admin:all", "org:platform:admin"]);
        let Json(resp) = auth_session_handler(Extension(ctx)).await;

        assert!(resp.is_platform_admin);
        assert!(resp.is_admin);
        assert!(resp.scopes.contains(&"admin:all".to_string()));
        assert_eq!(resp.email, "admin@flowplane.local");
        assert!(resp.teams.is_empty());
        // Platform org is excluded from org context (no with_org call)
        assert!(resp.org_name.is_none());
        assert!(resp.org_id.is_none());
    }

    #[tokio::test]
    async fn org_admin_session() {
        let ctx = make_org_context(
            vec!["org:acme-corp:admin", "team:engineering:*:*", "team:payments:*:*"],
            "org-acme-id",
            "acme-corp",
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
        let ctx = make_org_context(
            vec![
                "org:acme-corp:member",
                "team:engineering:clusters:read",
                "team:engineering:routes:write",
            ],
            "org-acme-id",
            "acme-corp",
        );
        let Json(resp) = auth_session_handler(Extension(ctx)).await;

        assert!(!resp.is_platform_admin);
        assert_eq!(resp.org_id.as_deref(), Some("org-acme-id"));
        assert_eq!(resp.org_name.as_deref(), Some("acme-corp"));
        assert_eq!(resp.org_role.as_deref(), Some("member"));
        assert_eq!(resp.teams, vec!["engineering"]);
    }

    #[tokio::test]
    async fn no_permissions_session() {
        let ctx = make_context(vec![]);
        let Json(resp) = auth_session_handler(Extension(ctx)).await;

        assert!(!resp.is_platform_admin);
        assert!(resp.scopes.is_empty());
        assert!(resp.teams.is_empty());
        assert!(resp.org_name.is_none());
        assert!(resp.org_id.is_none());
    }
}
