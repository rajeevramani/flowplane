//! Team, membership, and grant management.
//!
//! Org admins manage their own org's teams through explicit org-role checks — deliberately
//! OUTSIDE the resource-grant engine, and platform admins are NOT admitted here
//! (spec/05 §3.2 invariant 1: tenant administration belongs to the org).

use crate::authz::PrincipalCtx;
use crate::services::{actor_of, trace_context_json};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::event::{DomainEvent, EventScope};
use fp_domain::{DomainError, DomainResult, ErrorCode, OrgId, RequestId, Team, TeamId, UserId};
use fp_storage::repos::{audit, identity};
use sqlx::PgPool;

/// The caller's org id when (and only when) they hold an org-admin role there. When the
/// active org is unresolved because a selector is needed (D-014), fail closed with
/// `org_selector_required` rather than a generic forbidden.
fn require_org_admin(ctx: &PrincipalCtx) -> DomainResult<OrgId> {
    match ctx {
        PrincipalCtx::User {
            org: Some((org_id, role)),
            ..
        } if role.is_org_admin() => Ok(*org_id),
        PrincipalCtx::User {
            org: None,
            org_selector_required: true,
            ..
        } => Err(DomainError::org_selector_required()),
        _ => Err(DomainError::new(
            ErrorCode::Forbidden,
            "team administration requires an org admin role",
        )
        .with_hint("ask an org owner to grant you the admin role")),
    }
}

fn member_org(ctx: &PrincipalCtx) -> DomainResult<OrgId> {
    match ctx {
        PrincipalCtx::User {
            org: Some((org_id, _)),
            ..
        } => Ok(*org_id),
        PrincipalCtx::Agent { org_id, .. } => Ok(*org_id),
        PrincipalCtx::User {
            org: None,
            org_selector_required: true,
            ..
        } => Err(DomainError::org_selector_required()),
        PrincipalCtx::User { org: None, .. } => Err(DomainError::new(
            ErrorCode::Forbidden,
            "no organization membership",
        )),
    }
}

/// Guard: the team must belong to the caller's org (404 otherwise — anti-enumeration).
fn require_same_org(team: TeamRef, org_id: OrgId) -> DomainResult<()> {
    if team.org_id != org_id {
        return Err(DomainError::new(ErrorCode::NotFound, "team not found"));
    }
    Ok(())
}

fn admin_audit(
    ctx: &PrincipalCtx,
    request_id: RequestId,
    org_id: OrgId,
    team_id: Option<TeamId>,
    action: &str,
    resource: String,
) -> audit::AuditEntry {
    let (actor_type, actor_id) = actor_of(ctx);
    audit::AuditEntry {
        request_id: Some(request_id),
        actor_type,
        actor_id,
        actor_label: String::new(),
        surface: audit::Surface::Rest,
        action: action.into(),
        resource,
        org_id: Some(org_id),
        team_id,
        outcome: audit::Outcome::Success,
        detail: serde_json::json!({}),
    }
}

pub async fn create_team(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    name: &str,
    display_name: &str,
    request_id: RequestId,
) -> DomainResult<Team> {
    let org_id = require_org_admin(ctx)?;
    // Row + event + audit in ONE transaction (transactional-outbox invariant): a team must
    // never exist without its TeamCreated event and audit row, even on a mid-call crash.
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create team: begin"))?;
    let team = identity::create_team_tx(&mut tx, org_id, name, display_name).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::TeamCreated {
            team_id: team.id.as_uuid(),
            name: name.into(),
        },
        EventScope {
            org_id: Some(org_id),
            team_id: Some(team.id),
        },
        trace_context_json(),
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &admin_audit(
            ctx,
            request_id,
            org_id,
            Some(team.id),
            "team.create",
            format!("teams/{name}"),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("create team: commit"))?;
    Ok(team)
}

pub async fn list_teams(pool: &PgPool, ctx: &PrincipalCtx) -> DomainResult<Vec<Team>> {
    let org_id = member_org(ctx)?;
    identity::list_teams_for_org(pool, org_id).await
}

pub async fn delete_team(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    request_id: RequestId,
) -> DomainResult<()> {
    let org_id = require_org_admin(ctx)?;
    require_same_org(team, org_id)?;
    // Guard + DELETE + event + audit in ONE transaction (see create_team).
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("delete team: begin"))?;
    identity::delete_team_tx(&mut tx, team.id).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::TeamDeleted {
            team_id: team.id.as_uuid(),
            name: String::new(),
        },
        EventScope {
            org_id: Some(org_id),
            team_id: Some(team.id),
        },
        trace_context_json(),
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &admin_audit(
            ctx,
            request_id,
            org_id,
            Some(team.id),
            "team.delete",
            format!("teams/{}", team.id),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("delete team: commit"))?;
    Ok(())
}

pub async fn add_member(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    email: &str,
    request_id: RequestId,
) -> DomainResult<()> {
    let org_id = require_org_admin(ctx)?;
    require_same_org(team, org_id)?;
    let user = identity::find_user_by_email(pool, email)
        .await?
        .ok_or_else(|| {
            DomainError::not_found("user", email)
                .with_hint("the user must sign in once before being added to a team")
        })?;
    identity::add_team_membership(pool, user, team.id).await?;
    audit::record_best_effort(
        pool,
        &admin_audit(
            ctx,
            request_id,
            org_id,
            Some(team.id),
            "team.member.add",
            format!("users/{email}"),
        ),
    )
    .await;
    Ok(())
}

pub async fn list_members(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
) -> DomainResult<Vec<(UserId, String, String)>> {
    let org_id = member_org(ctx)?;
    require_same_org(team, org_id)?;
    identity::list_team_members(pool, team.id).await
}

pub async fn remove_member(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    user_id: UserId,
    request_id: RequestId,
) -> DomainResult<()> {
    let org_id = require_org_admin(ctx)?;
    require_same_org(team, org_id)?;
    if !identity::remove_team_membership(pool, user_id, team.id).await? {
        return Err(DomainError::new(
            ErrorCode::NotFound,
            "membership not found",
        ));
    }
    audit::record_best_effort(
        pool,
        &admin_audit(
            ctx,
            request_id,
            org_id,
            Some(team.id),
            "team.member.remove",
            format!("users/{user_id}"),
        ),
    )
    .await;
    Ok(())
}

pub async fn add_grant(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    email: &str,
    resource: Resource,
    action: Action,
    request_id: RequestId,
) -> DomainResult<()> {
    let org_id = require_org_admin(ctx)?;
    require_same_org(team, org_id)?;
    if resource.is_governance() {
        return Err(DomainError::validation(
            "governance resources cannot be granted at team scope",
        ));
    }
    let user = identity::find_user_by_email(pool, email)
        .await?
        .ok_or_else(|| {
            DomainError::not_found("user", email)
                .with_hint("the user must sign in once before receiving grants")
        })?;
    let granter = match ctx {
        PrincipalCtx::User { user_id, .. } => Some(*user_id),
        PrincipalCtx::Agent { .. } => None,
    };
    identity::add_grant(pool, user, org_id, team.id, resource, action, granter).await?;
    audit::record_best_effort(
        pool,
        &admin_audit(
            ctx,
            request_id,
            org_id,
            Some(team.id),
            "grant.add",
            format!("users/{email}:{}:{}", resource.as_str(), action.as_str()),
        ),
    )
    .await;
    Ok(())
}

pub async fn list_grants(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
) -> DomainResult<Vec<(uuid::Uuid, uuid::Uuid, String, String)>> {
    let org_id = member_org(ctx)?;
    require_same_org(team, org_id)?;
    identity::list_grants_for_team(pool, team.id).await
}

pub async fn remove_grant(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    grant_id: uuid::Uuid,
    request_id: RequestId,
) -> DomainResult<()> {
    let org_id = require_org_admin(ctx)?;
    require_same_org(team, org_id)?;
    if !identity::delete_grant(pool, team.id, grant_id).await? {
        return Err(DomainError::new(ErrorCode::NotFound, "grant not found"));
    }
    audit::record_best_effort(
        pool,
        &admin_audit(
            ctx,
            request_id,
            org_id,
            Some(team.id),
            "grant.remove",
            format!("grants/{grant_id}"),
        ),
    )
    .await;
    Ok(())
}
