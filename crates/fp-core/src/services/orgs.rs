//! Organization administration. Org CRUD is platform governance (decided by the engine on
//! Resource::Organizations). Member management within an org is org-admin territory —
//! platform admins are NOT admitted (spec/05 §3.2 invariant 1), with one carve-out: the
//! platform admin may add the FIRST owner to an org they just created (provisioning).

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{actor_of, deny_to_error, record_authz_denial};
use fp_domain::authz::{Action, Resource};
use fp_domain::{
    DomainError, DomainResult, ErrorCode, OrgId, OrgRole, Organization, RequestId, UserId,
};
use fp_storage::repos::{audit, identity};
use sqlx::PgPool;

async fn authorize_governance(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    action: Action,
    request_id: RequestId,
) -> DomainResult<()> {
    match check_resource_access(ctx, Resource::Organizations, action, None) {
        Decision::Allow(_) => Ok(()),
        Decision::Deny(reason) => {
            record_authz_denial(
                pool,
                ctx,
                request_id,
                Resource::Organizations,
                action,
                None,
                reason,
            )
            .await;
            Err(deny_to_error(Resource::Organizations, action, reason))
        }
    }
}

fn org_audit(
    ctx: &PrincipalCtx,
    request_id: RequestId,
    org_id: Option<OrgId>,
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
        org_id,
        team_id: None,
        outcome: audit::Outcome::Success,
        detail: serde_json::json!({}),
    }
}

pub async fn create_org(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    name: &str,
    display_name: &str,
    request_id: RequestId,
) -> DomainResult<Organization> {
    authorize_governance(pool, ctx, Action::Create, request_id).await?;
    let org = identity::create_org(pool, name, display_name).await?;
    audit::record_best_effort(
        pool,
        &org_audit(
            ctx,
            request_id,
            Some(org.id),
            "org.create",
            format!("organizations/{name}"),
        ),
    )
    .await;
    Ok(org)
}

pub async fn list_orgs(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    request_id: RequestId,
) -> DomainResult<Vec<Organization>> {
    authorize_governance(pool, ctx, Action::Read, request_id).await?;
    identity::list_orgs(pool).await
}

pub async fn get_org(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    org_id: OrgId,
    request_id: RequestId,
) -> DomainResult<Organization> {
    authorize_governance(pool, ctx, Action::Read, request_id).await?;
    identity::get_org(pool, org_id)
        .await?
        .ok_or_else(|| DomainError::new(ErrorCode::NotFound, "organization not found"))
}

pub async fn delete_org(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    org_id: OrgId,
    request_id: RequestId,
) -> DomainResult<()> {
    authorize_governance(pool, ctx, Action::Delete, request_id).await?;
    identity::delete_org(pool, org_id).await?;
    audit::record_best_effort(
        pool,
        &org_audit(
            ctx,
            request_id,
            Some(org_id),
            "org.delete",
            format!("organizations/{org_id}"),
        ),
    )
    .await;
    Ok(())
}

/// Who may manage members of `org_id`: an org admin of that same org, or — only while the
/// org has no owner yet — the platform admin (first-owner provisioning).
async fn authorize_member_admin(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    org_id: OrgId,
) -> DomainResult<()> {
    if let PrincipalCtx::User { user_id, .. } = ctx {
        if identity::get_org_membership_role(pool, *user_id, org_id)
            .await?
            .is_some_and(|role| role.is_org_admin())
        {
            return Ok(());
        }
    }
    if matches!(
        ctx,
        PrincipalCtx::User {
            platform_admin: true,
            ..
        }
    ) && identity::count_org_owners(pool, org_id).await? == 0
    {
        return Ok(());
    }
    Err(DomainError::new(
        ErrorCode::Forbidden,
        "member administration requires an org admin role in this organization",
    ))
}

pub async fn add_org_member(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    org_id: OrgId,
    email: &str,
    role: OrgRole,
    request_id: RequestId,
) -> DomainResult<()> {
    authorize_member_admin(pool, ctx, org_id).await?;
    let user = identity::find_user_by_email(pool, email)
        .await?
        .ok_or_else(|| {
            DomainError::not_found("user", email)
                .with_hint("the user must sign in once before being added to an organization")
        })?;
    add_org_member_resolved(pool, ctx, org_id, user, email, role, request_id).await
}

pub async fn add_org_member_by_subject(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    org_id: OrgId,
    subject: &str,
    role: OrgRole,
    request_id: RequestId,
) -> DomainResult<()> {
    authorize_member_admin(pool, ctx, org_id).await?;
    let user = identity::find_user_by_subject(pool, subject)
        .await?
        .ok_or_else(|| {
            DomainError::not_found("user", subject)
                .with_hint("the user must sign in once before being added to an organization")
        })?;
    add_org_member_resolved(pool, ctx, org_id, user, subject, role, request_id).await
}

pub async fn add_org_member_by_user_id(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    org_id: OrgId,
    user: UserId,
    role: OrgRole,
    request_id: RequestId,
) -> DomainResult<()> {
    authorize_member_admin(pool, ctx, org_id).await?;
    if !identity::active_user_exists(pool, user).await? {
        return Err(DomainError::not_found("user", &user.to_string())
            .with_hint("the user must sign in once before being added to an organization"));
    }
    let label = user.to_string();
    add_org_member_resolved(pool, ctx, org_id, user, &label, role, request_id).await
}

async fn add_org_member_resolved(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    org_id: OrgId,
    user: UserId,
    actor_label: &str,
    role: OrgRole,
    request_id: RequestId,
) -> DomainResult<()> {
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("add org member: begin"))?;
    identity::add_org_membership_in_tx(&mut tx, user, org_id, role).await?;
    audit::record_in_tx(
        &mut tx,
        &org_audit(
            ctx,
            request_id,
            Some(org_id),
            "org.member.add",
            format!("users/{actor_label}:{}", role.as_str()),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("add org member: commit"))?;
    Ok(())
}

pub async fn list_org_members(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    org_id: OrgId,
) -> DomainResult<Vec<(UserId, String, String, String)>> {
    // Own-org members may read the roster; platform admin may read any (governance read).
    let allowed = match ctx {
        PrincipalCtx::User {
            platform_admin: true,
            ..
        } => true,
        PrincipalCtx::User { user_id, .. } => {
            identity::get_org_membership_role(pool, *user_id, org_id)
                .await?
                .is_some()
        }
        _ => false,
    };
    if !allowed {
        return Err(DomainError::new(
            ErrorCode::NotFound,
            "organization not found",
        ));
    }
    identity::list_org_members(pool, org_id).await
}

pub async fn remove_org_member(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    org_id: OrgId,
    user_id: UserId,
    request_id: RequestId,
) -> DomainResult<()> {
    authorize_member_admin(pool, ctx, org_id).await?;
    // Never orphan an org: the last owner cannot be removed.
    let members = identity::list_org_members(pool, org_id).await?;
    let is_owner = members
        .iter()
        .any(|(id, _, _, role)| *id == user_id && role == "owner");
    if is_owner && identity::count_org_owners(pool, org_id).await? <= 1 {
        return Err(
            DomainError::conflict("cannot remove the last owner of an organization")
                .with_hint("promote another member to owner first"),
        );
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("remove org member: begin"))?;
    if !identity::remove_org_membership_in_tx(&mut tx, user_id, org_id).await? {
        return Err(DomainError::new(
            ErrorCode::NotFound,
            "membership not found",
        ));
    }
    audit::record_in_tx(
        &mut tx,
        &org_audit(
            ctx,
            request_id,
            Some(org_id),
            "org.member.remove",
            format!("users/{user_id}"),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("remove org member: commit"))?;
    Ok(())
}
