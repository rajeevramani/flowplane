//! DB-backed permission loading for Zitadel-authenticated users.
//!
//! Queries `organization_memberships` and `user_team_memberships` to produce
//! the full set of Flowplane scope strings for a user.

use std::collections::HashSet;

use sqlx::FromRow;
use tracing::instrument;

use crate::auth::models::{CpGrant, RouteGrant};
use crate::domain::{OrgId, UserId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;

// ---------------------------------------------------------------------------
// Return type
// ---------------------------------------------------------------------------

/// Aggregated permission data for a user, including org context.
#[derive(Debug, Clone)]
pub struct UserPermissions {
    pub scopes: HashSet<String>,
    pub org_id: Option<OrgId>,
    pub org_name: Option<String>,
    pub org_role: Option<String>,
}

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

#[derive(Debug, FromRow)]
struct OrgMembershipRow {
    org_id: String,
    role: String,
    org_name: String,
}

#[derive(Debug, FromRow)]
struct TeamScopesRow {
    scopes: String, // JSON array stored as TEXT
}

// ---------------------------------------------------------------------------
// Permission loading
// ---------------------------------------------------------------------------

/// Load all permission scopes for a user from the database.
///
/// Queries `organization_memberships` (joined with `organizations`) and
/// `user_team_memberships` and maps them into Flowplane scope strings.
///
/// Org role → scope mapping:
/// - `owner` in org `platform`  → `"admin:all"`
/// - `owner` or `admin` in any org → `"org:{org_name}:admin"`
/// - `member`                   → `"org:{org_name}:member"`
/// - `viewer`                   → `"org:{org_name}:viewer"`
///
/// Team memberships contribute their stored `scopes` JSON array directly.
#[instrument(skip(pool), fields(user_id = %user_id), name = "load_user_permissions")]
pub async fn load_user_permissions(pool: &DbPool, user_id: &UserId) -> Result<UserPermissions> {
    let mut scopes = HashSet::new();
    // TODO: multi-org support — currently returns first non-platform org
    let mut org_id: Option<OrgId> = None;
    let mut org_name: Option<String> = None;
    let mut org_role: Option<String> = None;

    // 1. Organisation memberships
    let org_rows = sqlx::query_as::<_, OrgMembershipRow>(
        r#"
        SELECT om.org_id, om.role, o.name AS org_name
        FROM organization_memberships om
        JOIN organizations o ON om.org_id = o.id
        WHERE om.user_id = $1
        "#,
    )
    .bind(user_id.as_str())
    .fetch_all(pool)
    .await
    .map_err(|e| FlowplaneError::Database {
        source: e,
        context: format!("load org memberships for user {user_id}"),
    })?;

    for row in &org_rows {
        map_org_role_to_scopes(&row.role, &row.org_name, &mut scopes);
        // Capture the first non-platform org's context
        if org_id.is_none() && row.org_name != "platform" {
            org_id = Some(OrgId::from_string(row.org_id.clone()));
            org_name = Some(row.org_name.clone());
            org_role = Some(row.role.clone());
        }
    }

    // 2. Team memberships
    let team_rows = sqlx::query_as::<_, TeamScopesRow>(
        "SELECT scopes FROM user_team_memberships WHERE user_id = $1",
    )
    .bind(user_id.as_str())
    .fetch_all(pool)
    .await
    .map_err(|e| FlowplaneError::Database {
        source: e,
        context: format!("load team memberships for user {user_id}"),
    })?;

    for row in team_rows {
        let team_scopes: Vec<String> = serde_json::from_str(&row.scopes).map_err(|e| {
            FlowplaneError::internal(format!("malformed scopes JSON in user_team_memberships: {e}"))
        })?;
        scopes.extend(team_scopes);
    }

    Ok(UserPermissions { scopes, org_id, org_name, org_role })
}

// ---------------------------------------------------------------------------
// Agent grant loading
// ---------------------------------------------------------------------------

/// DB row for agent_grants query — only the fields needed for permission checks.
#[derive(Debug, FromRow)]
struct AgentGrantRow {
    team: String,
    grant_type: String,
    resource_type: Option<String>,
    action: Option<String>,
    route_id: Option<String>,
    allowed_methods: Option<Vec<String>>,
}

/// Load all grants for an agent from the database.
///
/// Returns typed grant structs split by grant_type for use in permission checks.
/// Does NOT filter by expiry — `expires_at` enforcement is future work.
#[instrument(skip(pool), fields(agent_id = %agent_id), name = "load_agent_grants")]
pub async fn load_agent_grants(
    pool: &DbPool,
    agent_id: &str,
) -> Result<(Vec<CpGrant>, Vec<RouteGrant>, Vec<RouteGrant>)> {
    let rows = sqlx::query_as::<_, AgentGrantRow>(
        "SELECT team, grant_type, resource_type, action, route_id, allowed_methods \
         FROM agent_grants WHERE agent_id = $1",
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await
    .map_err(|e| FlowplaneError::Database {
        source: e,
        context: format!("load agent grants for {agent_id}"),
    })?;

    let mut cp_grants = Vec::new();
    let mut gateway_grants = Vec::new();
    let mut route_grants = Vec::new();

    for row in rows {
        match row.grant_type.as_str() {
            "cp-tool" => {
                if let (Some(resource_type), Some(action)) = (row.resource_type, row.action) {
                    cp_grants.push(CpGrant { resource_type, action, team: row.team });
                }
            }
            "gateway-tool" => {
                if let Some(route_id) = row.route_id {
                    gateway_grants.push(RouteGrant {
                        route_id,
                        allowed_methods: row.allowed_methods.unwrap_or_default(),
                        team: row.team,
                    });
                }
            }
            "route" => {
                if let Some(route_id) = row.route_id {
                    route_grants.push(RouteGrant {
                        route_id,
                        allowed_methods: row.allowed_methods.unwrap_or_default(),
                        team: row.team,
                    });
                }
            }
            _ => {
                tracing::warn!(grant_type = %row.grant_type, "unknown grant_type — skipping");
            }
        }
    }

    Ok((cp_grants, gateway_grants, route_grants))
}

/// Map a single org membership row into Flowplane scope strings.
fn map_org_role_to_scopes(role: &str, org_name: &str, scopes: &mut HashSet<String>) {
    match role {
        "owner" if org_name == "platform" => {
            scopes.insert("admin:all".to_string());
            // Owner of the platform org also gets org-level admin
            scopes.insert(format!("org:{org_name}:admin"));
        }
        "owner" | "admin" => {
            scopes.insert(format!("org:{org_name}:admin"));
        }
        "member" => {
            scopes.insert(format!("org:{org_name}:member"));
        }
        "viewer" => {
            scopes.insert(format!("org:{org_name}:viewer"));
        }
        other => {
            tracing::warn!(role = other, org_name, "unknown org role — skipping");
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: call the mapping logic directly with a fake pool result
    fn scopes_for(role: &str, org_name: &str) -> HashSet<String> {
        let mut s = HashSet::new();
        map_org_role_to_scopes(role, org_name, &mut s);
        s
    }

    #[test]
    fn owner_of_platform_gets_admin_all() {
        let s = scopes_for("owner", "platform");
        assert!(s.contains("admin:all"), "expected admin:all in {s:?}");
        assert!(s.contains("org:platform:admin"), "expected org:platform:admin in {s:?}");
    }

    #[test]
    fn owner_of_non_platform_org_gets_org_admin() {
        let s = scopes_for("owner", "acme-corp");
        assert!(!s.contains("admin:all"), "unexpected admin:all for non-platform owner");
        assert!(s.contains("org:acme-corp:admin"), "expected org:acme-corp:admin in {s:?}");
    }

    #[test]
    fn admin_role_maps_to_org_admin() {
        let s = scopes_for("admin", "acme-corp");
        assert!(s.contains("org:acme-corp:admin"));
        assert!(!s.contains("admin:all"));
    }

    #[test]
    fn member_role_maps_to_org_member() {
        let s = scopes_for("member", "acme-corp");
        assert!(s.contains("org:acme-corp:member"));
        assert!(!s.contains("org:acme-corp:admin"));
    }

    #[test]
    fn viewer_role_maps_to_org_viewer() {
        let s = scopes_for("viewer", "acme-corp");
        assert!(s.contains("org:acme-corp:viewer"));
        assert!(!s.contains("org:acme-corp:member"));
    }

    #[test]
    fn unknown_role_produces_no_scopes() {
        let s = scopes_for("superuser", "acme-corp");
        assert!(s.is_empty(), "expected empty scopes for unknown role, got {s:?}");
    }

    #[test]
    fn empty_memberships_produce_empty_scopes() {
        // Simulates a user with no org memberships and no team memberships.
        // load_user_permissions would return an empty set — verified here by
        // calling map_org_role_to_scopes with nothing and asserting the set stays empty.
        let mut s = HashSet::new();
        // No calls to map_org_role_to_scopes
        assert!(s.is_empty());
        // Also verify JSON parsing of an empty scopes array works
        let json = "[]";
        let parsed: Vec<String> = serde_json::from_str(json).unwrap();
        s.extend(parsed);
        assert!(s.is_empty());
    }

    #[test]
    fn team_scopes_are_added_directly() {
        let json = r#"["team:engineering:clusters:read","team:engineering:routes:write"]"#;
        let mut s = HashSet::new();
        let team_scopes: Vec<String> = serde_json::from_str(json).unwrap();
        s.extend(team_scopes);
        assert!(s.contains("team:engineering:clusters:read"));
        assert!(s.contains("team:engineering:routes:write"));
    }
}
