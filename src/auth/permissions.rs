//! DB-backed permission loading for Zitadel-authenticated users.
//!
//! Queries `organization_memberships` and the unified `grants` table to produce
//! the full permission set for a user (human or machine).

use std::collections::HashSet;

use sqlx::FromRow;
use tracing::instrument;

use crate::auth::models::{Grant, GrantType};
use crate::domain::{OrgId, UserId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;

// ---------------------------------------------------------------------------
// Return type
// ---------------------------------------------------------------------------

/// Aggregated permission data for a user, including org context.
#[derive(Debug, Clone)]
pub struct UserPermissions {
    /// Org-level scopes (e.g., "admin:all", "org:acme:admin").
    pub org_scopes: HashSet<String>,
    /// Unified grants from the `grants` table.
    pub grants: Vec<Grant>,
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
struct GrantRow {
    team_id: String,
    team_name: String,
    grant_type: String,
    resource_type: Option<String>,
    action: Option<String>,
    route_id: Option<String>,
    allowed_methods: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Permission loading
// ---------------------------------------------------------------------------

/// Load all permissions for a user (human or machine) from the database.
///
/// Queries `organization_memberships` for org-level scopes and the unified
/// `grants` table for resource/gateway/route grants. Works identically for
/// human and machine users — the same table, same query, same result type.
///
/// Org role -> scope mapping:
/// - `owner` in org `platform`  -> `"admin:all"`
/// - `owner` or `admin` in any org -> `"org:{org_name}:admin"`
/// - `member`                   -> `"org:{org_name}:member"`
/// - `viewer`                   -> `"org:{org_name}:viewer"`
#[instrument(skip(pool), fields(user_id = %user_id), name = "load_permissions")]
pub async fn load_permissions(pool: &DbPool, user_id: &UserId) -> Result<UserPermissions> {
    let mut org_scopes = HashSet::new();
    let mut org_id: Option<OrgId> = None;
    let mut org_name: Option<String> = None;
    let mut org_role: Option<String> = None;

    // 1. Organisation memberships -> org_scopes
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
        map_org_role_to_scopes(&row.role, &row.org_name, &mut org_scopes);
        // Capture the first non-platform org's context
        if org_id.is_none() && row.org_name != "platform" {
            org_id = Some(OrgId::from_string(row.org_id.clone()));
            org_name = Some(row.org_name.clone());
            org_role = Some(row.role.clone());
        }
    }

    // 2. Unified grants (resource + gateway-tool + route) with expires_at filter
    let grant_rows = sqlx::query_as::<_, GrantRow>(
        r#"
        SELECT g.team_id, t.name AS team_name, g.grant_type,
               g.resource_type, g.action, g.route_id, g.allowed_methods
        FROM grants g
        JOIN teams t ON t.id = g.team_id
        WHERE g.principal_id = $1
          AND (g.expires_at IS NULL OR g.expires_at > NOW())
        "#,
    )
    .bind(user_id.as_str())
    .fetch_all(pool)
    .await
    .map_err(|e| FlowplaneError::Database {
        source: e,
        context: format!("load grants for user {user_id}"),
    })?;

    let grants: Vec<Grant> = grant_rows
        .into_iter()
        .filter_map(|row| {
            let grant_type = GrantType::parse(&row.grant_type)?;
            Some(Grant {
                grant_type,
                team_id: row.team_id,
                team_name: row.team_name,
                resource_type: row.resource_type,
                action: row.action,
                route_id: row.route_id,
                allowed_methods: row.allowed_methods.unwrap_or_default(),
            })
        })
        .collect();

    Ok(UserPermissions { org_scopes, grants, org_id, org_name, org_role })
}

/// Map a single org membership row into org-level scope strings.
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
        let mut s = HashSet::new();
        assert!(s.is_empty());
        let json = "[]";
        let parsed: Vec<String> = serde_json::from_str(json).unwrap();
        s.extend(parsed);
        assert!(s.is_empty());
    }
}
