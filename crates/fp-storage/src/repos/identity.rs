//! Identity repositories: users (JIT provisioning), orgs, teams, memberships, grants, and
//! the principal-context loader the auth middleware calls once per request.

use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::{
    DomainError, DomainResult, EntityStatus, OrgId, OrgRole, Organization, Team, TeamId, UserId,
};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Everything the authorization engine needs about a user principal, loaded in one pass.
/// D-014: a human user may belong to multiple orgs, so we return the full membership SET —
/// the auth middleware resolves the single *active* request org from it. No implicit
/// "first membership wins".
#[derive(Debug, Clone)]
pub struct LoadedPrincipal {
    pub user_id: UserId,
    pub platform_admin: bool,
    /// All active org memberships (org + role), unordered. Includes the platform org if the
    /// user is a member of it.
    pub memberships: Vec<(OrgId, OrgRole)>,
    /// The platform org id (from `instance_meta`), so callers can exclude it from tenant
    /// context resolution. `None` on an uninitialized instance.
    pub platform_org_id: Option<OrgId>,
    pub grants: Vec<(Resource, Action, TeamId)>,
}

/// JIT user provisioning (Q-004): first authenticated request creates the user row;
/// later requests refresh email/name drift from the IdP.
pub async fn upsert_user_by_subject(
    pool: &PgPool,
    subject: &str,
    email: &str,
    name: &str,
) -> DomainResult<UserId> {
    let row = sqlx::query(
        "INSERT INTO users (id, subject, email, name) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (subject) DO UPDATE SET email = EXCLUDED.email, name = EXCLUDED.name, \
         updated_at = now() \
         RETURNING id",
    )
    .bind(UserId::generate().as_uuid())
    .bind(subject)
    .bind(email)
    .bind(name)
    .fetch_one(pool)
    .await
    .map_err(|e| DomainError::internal(format!("upsert user: {e}")))?;
    Ok(UserId::from(row.get::<Uuid, _>("id")))
}

/// Load the full principal context for a validated subject. Returns `None` for unknown or
/// suspended users (callers render 401 — a suspended user is indistinguishable from absent).
pub async fn load_principal(pool: &PgPool, subject: &str) -> DomainResult<Option<LoadedPrincipal>> {
    let Some(user_row) = sqlx::query("SELECT id, status FROM users WHERE subject = $1")
        .bind(subject)
        .fetch_optional(pool)
        .await
        .map_err(|e| DomainError::internal(format!("load principal: user: {e}")))?
    else {
        return Ok(None);
    };
    if user_row.get::<String, _>("status") != "active" {
        return Ok(None);
    }
    let user_id = UserId::from(user_row.get::<Uuid, _>("id"));

    // D-014: load the FULL active-membership set (no ORDER BY / LIMIT). The middleware
    // resolves the active request org from this set + the request's org selector.
    let membership_rows = sqlx::query(
        "SELECT m.org_id, m.role FROM org_memberships m \
         JOIN organizations o ON o.id = m.org_id AND o.status = 'active' \
         WHERE m.user_id = $1",
    )
    .bind(user_id.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("load principal: memberships: {e}")))?;

    let mut memberships = Vec::with_capacity(membership_rows.len());
    for row in &membership_rows {
        memberships.push((
            OrgId::from(row.get::<Uuid, _>("org_id")),
            OrgRole::parse(&row.get::<String, _>("role"))?,
        ));
    }

    let platform_org_id: Option<OrgId> = sqlx::query_scalar::<_, String>(
        "SELECT value FROM instance_meta WHERE key = 'platform_org_id'",
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("load principal: platform org: {e}")))?
    .and_then(|v| Uuid::parse_str(&v).ok())
    .map(OrgId::from);

    // Platform admin = owner of the platform org (membership in the set with role Owner).
    let platform_admin = match platform_org_id {
        Some(platform) => memberships
            .iter()
            .any(|(org_id, role)| *org_id == platform && *role == OrgRole::Owner),
        None => false,
    };

    let grant_rows = sqlx::query(
        "SELECT resource, action, team_id FROM grants \
         WHERE principal_type = 'user' AND principal_id = $1",
    )
    .bind(user_id.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("load principal: grants: {e}")))?;

    let mut grants = Vec::with_capacity(grant_rows.len());
    for row in grant_rows {
        // Unknown resource/action strings (from a future or corrupted row) are skipped with
        // a warning rather than failing every request for the principal.
        let resource = Resource::parse(&row.get::<String, _>("resource"));
        let action = Action::parse(&row.get::<String, _>("action"));
        match (resource, action) {
            (Ok(r), Ok(a)) => grants.push((r, a, TeamId::from(row.get::<Uuid, _>("team_id")))),
            _ => tracing::warn!(user = %user_id, "skipping grant with unknown resource/action"),
        }
    }

    Ok(Some(LoadedPrincipal {
        user_id,
        platform_admin,
        memberships,
        platform_org_id,
        grants,
    }))
}

fn org_from_row(row: &PgRow) -> DomainResult<Organization> {
    Ok(Organization {
        id: OrgId::from(row.get::<Uuid, _>("id")),
        name: row.get("name"),
        display_name: row.get("display_name"),
        status: parse_status(&row.get::<String, _>("status"))?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn team_from_row(row: &PgRow) -> DomainResult<Team> {
    Ok(Team {
        id: TeamId::from(row.get::<Uuid, _>("id")),
        org_id: OrgId::from(row.get::<Uuid, _>("org_id")),
        name: row.get("name"),
        display_name: row.get("display_name"),
        status: parse_status(&row.get::<String, _>("status"))?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn parse_status(raw: &str) -> DomainResult<EntityStatus> {
    match raw {
        "active" => Ok(EntityStatus::Active),
        "suspended" => Ok(EntityStatus::Suspended),
        other => Err(DomainError::internal(format!(
            "unknown status \"{other}\" in database"
        ))),
    }
}

pub async fn create_org(
    pool: &PgPool,
    name: &str,
    display_name: &str,
) -> DomainResult<Organization> {
    fp_domain::validate_name(name)?;
    let row = sqlx::query(
        "INSERT INTO organizations (id, name, display_name) VALUES ($1, $2, $3) \
         RETURNING id, name, display_name, status, created_at, updated_at",
    )
    .bind(OrgId::generate().as_uuid())
    .bind(name)
    .bind(display_name)
    .fetch_one(pool)
    .await
    .map_err(|e| map_unique_violation(e, "organization", name))?;
    org_from_row(&row)
}

/// Insert a team inside a caller-owned transaction. The service layer uses this so the
/// row, its `TeamCreated` outbox event, and its audit entry commit atomically (the
/// transactional-outbox invariant — a team must never exist without its event/audit).
pub async fn create_team_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org_id: OrgId,
    name: &str,
    display_name: &str,
) -> DomainResult<Team> {
    fp_domain::validate_name(name)?;
    let row = sqlx::query(
        "INSERT INTO teams (id, org_id, name, display_name) VALUES ($1, $2, $3, $4) \
         RETURNING id, org_id, name, display_name, status, created_at, updated_at",
    )
    .bind(TeamId::generate().as_uuid())
    .bind(org_id.as_uuid())
    .bind(name)
    .bind(display_name)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| map_unique_violation(e, "team", name))?;
    team_from_row(&row)
}

/// Standalone create (own transaction). Kept for tests/fixtures; production goes through
/// [`create_team_tx`] so the event + audit share the transaction.
pub async fn create_team(
    pool: &PgPool,
    org_id: OrgId,
    name: &str,
    display_name: &str,
) -> DomainResult<Team> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| DomainError::internal(format!("create team: begin: {e}")))?;
    let team = create_team_tx(&mut tx, org_id, name, display_name).await?;
    tx.commit()
        .await
        .map_err(|e| DomainError::internal(format!("create team: commit: {e}")))?;
    Ok(team)
}

/// Resolve a team id to its org-carrying reference for the authorization engine.
/// Existence is NOT disclosed by this call alone — callers must run the engine's decision
/// and render denials as `not_found`.
pub async fn resolve_team_ref(pool: &PgPool, team_id: TeamId) -> DomainResult<Option<TeamRef>> {
    let row = sqlx::query("SELECT id, org_id FROM teams WHERE id = $1 AND status = 'active'")
        .bind(team_id.as_uuid())
        .fetch_optional(pool)
        .await
        .map_err(|e| DomainError::internal(format!("resolve team: {e}")))?;
    Ok(row.map(|r| TeamRef {
        id: TeamId::from(r.get::<Uuid, _>("id")),
        org_id: OrgId::from(r.get::<Uuid, _>("org_id")),
    }))
}

/// Resolve a team by name within ONE org (tenant callers can never look up by name across
/// orgs — the cross-tenant name oracle from v1 is structurally closed, spec/08a §2.2.2).
pub async fn resolve_team_by_name(
    pool: &PgPool,
    org_id: OrgId,
    name: &str,
) -> DomainResult<Option<TeamRef>> {
    let row = sqlx::query(
        "SELECT id, org_id FROM teams WHERE org_id = $1 AND name = $2 AND status = 'active'",
    )
    .bind(org_id.as_uuid())
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("resolve team by name: {e}")))?;
    Ok(row.map(|r| TeamRef {
        id: TeamId::from(r.get::<Uuid, _>("id")),
        org_id: OrgId::from(r.get::<Uuid, _>("org_id")),
    }))
}

pub async fn add_org_membership(
    pool: &PgPool,
    user_id: UserId,
    org_id: OrgId,
    role: OrgRole,
) -> DomainResult<()> {
    sqlx::query(
        "INSERT INTO org_memberships (id, user_id, org_id, role) VALUES (gen_random_uuid(), $1, $2, $3) \
         ON CONFLICT (user_id, org_id) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(user_id.as_uuid())
    .bind(org_id.as_uuid())
    .bind(role.as_str())
    .execute(pool)
    .await
    .map_err(|e| DomainError::internal(format!("add org membership: {e}")))?;
    Ok(())
}

pub async fn add_grant(
    pool: &PgPool,
    principal_user: UserId,
    org_id: OrgId,
    team_id: TeamId,
    resource: Resource,
    action: Action,
    created_by: Option<UserId>,
) -> DomainResult<()> {
    sqlx::query(
        "INSERT INTO grants (id, principal_type, principal_id, org_id, team_id, resource, action, created_by) \
         VALUES (gen_random_uuid(), 'user', $1, $2, $3, $4, $5, $6) \
         ON CONFLICT (principal_type, principal_id, team_id, resource, action) DO NOTHING",
    )
    .bind(principal_user.as_uuid())
    .bind(org_id.as_uuid())
    .bind(team_id.as_uuid())
    .bind(resource.as_str())
    .bind(action.as_str())
    .bind(created_by.map(|u| u.as_uuid()))
    .execute(pool)
    .await
    .map_err(|e| {
        // The composite (team_id, org_id) FK rejects cross-org grants by construction.
        if is_fk_violation(&e) {
            DomainError::validation("grant references a team outside the given organization")
        } else {
            DomainError::internal(format!("add grant: {e}"))
        }
    })?;
    Ok(())
}

/// Marker used at bootstrap to designate the platform organization.
pub async fn set_platform_org(pool: &PgPool, org_id: OrgId) -> DomainResult<()> {
    sqlx::query(
        "INSERT INTO instance_meta (key, value) VALUES ('platform_org_id', $1) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(org_id.as_uuid().to_string())
    .execute(pool)
    .await
    .map_err(|e| DomainError::internal(format!("set platform org: {e}")))?;
    Ok(())
}

fn map_unique_violation(e: sqlx::Error, kind: &str, name: &str) -> DomainError {
    if let sqlx::Error::Database(db) = &e {
        if db.code().as_deref() == Some("23505") {
            return DomainError::conflict(format!("{kind} \"{name}\" already exists"))
                .with_hint("choose a different name or address the existing resource");
        }
    }
    DomainError::internal(format!("create {kind}: {e}"))
}

fn is_fk_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.code().as_deref() == Some("23503"))
}

/// Teams of one org (governance read).
pub async fn list_teams_for_org(pool: &PgPool, org_id: OrgId) -> DomainResult<Vec<Team>> {
    let rows = sqlx::query(
        "SELECT id, org_id, name, display_name, status, created_at, updated_at \
         FROM teams WHERE org_id = $1 AND status = 'active' ORDER BY name",
    )
    .bind(org_id.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list teams: {e}")))?;
    rows.iter().map(team_from_row).collect()
}

/// Delete a team inside a caller-owned transaction (resource-count guard, the DELETE, and
/// the caller's event + audit all commit atomically). The count check and DELETE run in the
/// same transaction so a concurrently-created resource cannot slip in between.
pub async fn delete_team_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team_id: TeamId,
) -> DomainResult<()> {
    let (clusters, listeners, rcs): (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM clusters WHERE team_id = $1), \
                (SELECT count(*) FROM listeners WHERE team_id = $1), \
                (SELECT count(*) FROM route_configs WHERE team_id = $1)",
    )
    .bind(team_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("delete team: counts: {e}")))?;
    if clusters + listeners + rcs > 0 {
        return Err(DomainError::conflict(format!(
            "team still owns resources ({clusters} clusters, {listeners} listeners, {rcs} route configs)"
        ))
        .with_hint("delete the team's resources first"));
    }
    let deleted = sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(team_id.as_uuid())
        .execute(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("delete team: {e}")))?;
    if deleted.rows_affected() == 0 {
        return Err(DomainError::new(
            fp_domain::ErrorCode::NotFound,
            "team not found",
        ));
    }
    Ok(())
}

/// Standalone delete (own transaction). Kept for tests/fixtures; production goes through
/// [`delete_team_tx`] so the event + audit share the transaction.
pub async fn delete_team(pool: &PgPool, team_id: TeamId) -> DomainResult<()> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| DomainError::internal(format!("delete team: begin: {e}")))?;
    delete_team_tx(&mut tx, team_id).await?;
    tx.commit()
        .await
        .map_err(|e| DomainError::internal(format!("delete team: commit: {e}")))?;
    Ok(())
}

pub async fn add_team_membership(
    pool: &PgPool,
    user_id: UserId,
    team_id: TeamId,
) -> DomainResult<()> {
    sqlx::query(
        "INSERT INTO team_memberships (id, user_id, team_id) \
         VALUES (gen_random_uuid(), $1, $2) ON CONFLICT (user_id, team_id) DO NOTHING",
    )
    .bind(user_id.as_uuid())
    .bind(team_id.as_uuid())
    .execute(pool)
    .await
    .map_err(|e| {
        if matches!(&e, sqlx::Error::Database(db) if db.code().as_deref() == Some("23503")) {
            DomainError::validation("user or team does not exist")
        } else {
            DomainError::internal(format!("add team membership: {e}"))
        }
    })?;
    Ok(())
}

pub async fn list_team_members(
    pool: &PgPool,
    team_id: TeamId,
) -> DomainResult<Vec<(UserId, String, String)>> {
    let rows = sqlx::query(
        "SELECT u.id, u.email, u.name FROM users u \
         JOIN team_memberships m ON m.user_id = u.id WHERE m.team_id = $1 ORDER BY u.email",
    )
    .bind(team_id.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list team members: {e}")))?;
    Ok(rows
        .iter()
        .map(|r| {
            (
                UserId::from(r.get::<Uuid, _>("id")),
                r.get("email"),
                r.get("name"),
            )
        })
        .collect())
}

pub async fn remove_team_membership(
    pool: &PgPool,
    user_id: UserId,
    team_id: TeamId,
) -> DomainResult<bool> {
    let deleted = sqlx::query("DELETE FROM team_memberships WHERE user_id = $1 AND team_id = $2")
        .bind(user_id.as_uuid())
        .bind(team_id.as_uuid())
        .execute(pool)
        .await
        .map_err(|e| DomainError::internal(format!("remove team membership: {e}")))?;
    Ok(deleted.rows_affected() > 0)
}

/// Grants on one team (governance read for the grant surface).
pub async fn list_grants_for_team(
    pool: &PgPool,
    team_id: TeamId,
) -> DomainResult<Vec<(Uuid, Uuid, String, String)>> {
    let rows = sqlx::query(
        "SELECT id, principal_id, resource, action FROM grants \
         WHERE team_id = $1 AND principal_type = 'user' ORDER BY resource, action",
    )
    .bind(team_id.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list grants: {e}")))?;
    Ok(rows
        .iter()
        .map(|r| {
            (
                r.get("id"),
                r.get("principal_id"),
                r.get("resource"),
                r.get("action"),
            )
        })
        .collect())
}

pub async fn delete_grant(pool: &PgPool, team_id: TeamId, grant_id: Uuid) -> DomainResult<bool> {
    let deleted = sqlx::query("DELETE FROM grants WHERE id = $1 AND team_id = $2")
        .bind(grant_id)
        .bind(team_id.as_uuid())
        .execute(pool)
        .await
        .map_err(|e| DomainError::internal(format!("delete grant: {e}")))?;
    Ok(deleted.rows_affected() > 0)
}

/// Resolve an active user by **immutable subject** (the preferred, unambiguous path — a
/// subject is globally unique by IdP construction).
pub async fn find_user_by_subject(pool: &PgPool, subject: &str) -> DomainResult<Option<UserId>> {
    let id: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE subject = $1 AND status = 'active'")
            .bind(subject)
            .fetch_optional(pool)
            .await
            .map_err(|e| DomainError::internal(format!("find user by subject: {e}")))?;
    Ok(id.map(UserId::from))
}

/// Resolve an active user by email (a UX affordance only — D-014/R6). Email is NOT a unique
/// or tenant-isolating key, so an ambiguous match (more than one active user) is REJECTED
/// rather than silently picking one with `LIMIT 1`. Callers should prefer
/// [`find_user_by_subject`].
pub async fn find_user_by_email(pool: &PgPool, email: &str) -> DomainResult<Option<UserId>> {
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE email = $1 AND status = 'active'")
            .bind(email)
            .fetch_all(pool)
            .await
            .map_err(|e| DomainError::internal(format!("find user: {e}")))?;
    match ids.len() {
        0 => Ok(None),
        1 => Ok(Some(UserId::from(ids[0]))),
        _ => Err(DomainError::conflict(format!(
            "email \"{email}\" matches multiple users; identify the user by subject or id instead"
        ))
        .with_hint("email is not a unique identifier; use the immutable subject/user-id")),
    }
}

/// Resolve an active org id by name (for the `X-Flowplane-Org` selector when it is a name
/// rather than a UUID). Returns `None` for unknown/suspended orgs — the caller then fails
/// closed without disclosing existence.
pub async fn find_active_org_id_by_name(pool: &PgPool, name: &str) -> DomainResult<Option<OrgId>> {
    let id: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM organizations WHERE name = $1 AND status = 'active'")
            .bind(name)
            .fetch_optional(pool)
            .await
            .map_err(|e| DomainError::internal(format!("resolve org by name: {e}")))?;
    Ok(id.map(OrgId::from))
}

pub async fn list_orgs(pool: &PgPool) -> DomainResult<Vec<Organization>> {
    let rows = sqlx::query(
        "SELECT id, name, display_name, status, created_at, updated_at \
         FROM organizations WHERE status = 'active' ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list orgs: {e}")))?;
    rows.iter().map(org_from_row).collect()
}

pub async fn get_org(pool: &PgPool, org_id: OrgId) -> DomainResult<Option<Organization>> {
    let row = sqlx::query(
        "SELECT id, name, display_name, status, created_at, updated_at \
         FROM organizations WHERE id = $1",
    )
    .bind(org_id.as_uuid())
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get org: {e}")))?;
    row.as_ref().map(org_from_row).transpose()
}

pub async fn resolve_org_by_name(pool: &PgPool, name: &str) -> DomainResult<Option<OrgId>> {
    let id: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM organizations WHERE name = $1 AND status = 'active'")
            .bind(name)
            .fetch_optional(pool)
            .await
            .map_err(|e| DomainError::internal(format!("resolve org: {e}")))?;
    Ok(id.map(OrgId::from))
}

/// Delete an org. Refuses while teams exist (RESTRICT semantics with a helpful error).
pub async fn delete_org(pool: &PgPool, org_id: OrgId) -> DomainResult<()> {
    let teams: i64 = sqlx::query_scalar("SELECT count(*) FROM teams WHERE org_id = $1")
        .bind(org_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("delete org: count: {e}")))?;
    if teams > 0 {
        return Err(
            DomainError::conflict(format!("organization still has {teams} team(s)"))
                .with_hint("delete the org's teams first"),
        );
    }
    let deleted = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org_id.as_uuid())
        .execute(pool)
        .await
        .map_err(|e| DomainError::internal(format!("delete org: {e}")))?;
    if deleted.rows_affected() == 0 {
        return Err(DomainError::new(
            fp_domain::ErrorCode::NotFound,
            "organization not found",
        ));
    }
    Ok(())
}

pub async fn list_org_members(
    pool: &PgPool,
    org_id: OrgId,
) -> DomainResult<Vec<(UserId, String, String, String)>> {
    let rows = sqlx::query(
        "SELECT u.id, u.email, u.name, m.role FROM users u \
         JOIN org_memberships m ON m.user_id = u.id WHERE m.org_id = $1 ORDER BY u.email",
    )
    .bind(org_id.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list org members: {e}")))?;
    Ok(rows
        .iter()
        .map(|r| {
            (
                UserId::from(r.get::<Uuid, _>("id")),
                r.get("email"),
                r.get("name"),
                r.get("role"),
            )
        })
        .collect())
}

pub async fn remove_org_membership(
    pool: &PgPool,
    user_id: UserId,
    org_id: OrgId,
) -> DomainResult<bool> {
    let deleted = sqlx::query("DELETE FROM org_memberships WHERE user_id = $1 AND org_id = $2")
        .bind(user_id.as_uuid())
        .bind(org_id.as_uuid())
        .execute(pool)
        .await
        .map_err(|e| DomainError::internal(format!("remove org membership: {e}")))?;
    Ok(deleted.rows_affected() > 0)
}

/// Owners of an org (used to prevent removing the last owner).
pub async fn count_org_owners(pool: &PgPool, org_id: OrgId) -> DomainResult<i64> {
    sqlx::query_scalar("SELECT count(*) FROM org_memberships WHERE org_id = $1 AND role = 'owner'")
        .bind(org_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count owners: {e}")))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn unique(prefix: &str) -> String {
        format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
    }

    async fn pool() -> Option<PgPool> {
        let url = std::env::var("FLOWPLANE_TEST_DATABASE_URL").ok()?;
        let pool = crate::connect(&url, 4).await.expect("connect");
        crate::migrate(&pool).await.expect("migrate");
        Some(pool)
    }

    #[tokio::test]
    async fn loader_returns_full_membership_set_no_implicit_pick() {
        let Some(pool) = pool().await else { return };
        // A user in TWO orgs: the loader must return both, in no implied order, and must
        // NOT collapse to one (D-014 — no "first membership wins").
        let org_a = create_org(&pool, &unique("orga"), "").await.expect("a");
        let org_b = create_org(&pool, &unique("orgb"), "").await.expect("b");
        let subject = unique("sub");
        let user = upsert_user_by_subject(&pool, &subject, "m@x.test", "M")
            .await
            .expect("u");
        add_org_membership(&pool, user, org_a.id, OrgRole::Admin)
            .await
            .expect("ma");
        add_org_membership(&pool, user, org_b.id, OrgRole::Member)
            .await
            .expect("mb");

        let loaded = load_principal(&pool, &subject)
            .await
            .expect("load")
            .expect("exists");
        assert_eq!(loaded.memberships.len(), 2, "both memberships returned");
        let ids: std::collections::HashSet<_> =
            loaded.memberships.iter().map(|(o, _)| *o).collect();
        assert!(ids.contains(&org_a.id) && ids.contains(&org_b.id));
        // Not a platform admin (neither org is the platform org here).
        assert!(!loaded.platform_admin);
    }

    #[tokio::test]
    async fn find_user_by_email_rejects_ambiguous_matches() {
        let Some(pool) = pool().await else { return };
        // Two distinct users sharing one email (email is not unique — R6).
        let email = format!("{}@dup.test", unique("e"));
        let u1 = upsert_user_by_subject(&pool, &unique("s1"), &email, "One")
            .await
            .expect("u1");
        let _u2 = upsert_user_by_subject(&pool, &unique("s2"), &email, "Two")
            .await
            .expect("u2");

        let err = find_user_by_email(&pool, &email)
            .await
            .expect_err("ambiguous email must be rejected, not silently picked");
        assert_eq!(err.code, fp_domain::ErrorCode::Conflict);

        // A unique email still resolves; subject is always unambiguous.
        let unique_email = format!("{}@one.test", unique("e"));
        let u3 = upsert_user_by_subject(&pool, &unique("s3"), &unique_email, "Three")
            .await
            .expect("u3");
        assert_eq!(
            find_user_by_email(&pool, &unique_email).await.expect("ok"),
            Some(u3)
        );
        assert_eq!(
            find_user_by_subject(&pool, &format!("ghost-{}", unique("g")))
                .await
                .expect("ok"),
            None
        );
        let _ = u1;
    }
}
