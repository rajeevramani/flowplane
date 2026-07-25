//! Agent lifecycle for S11 MCP prerequisites.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{actor_of, deny_to_error, record_authz_denial};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::{
    Agent, AgentId, AgentKind, DomainError, DomainResult, ErrorCode, OrgId, RequestId, TeamId,
    UserId,
};
use fp_storage::repos::{audit, identity};
use sqlx::PgPool;

#[derive(Debug, Clone)]
pub struct AgentGrantInput {
    pub team_id: TeamId,
    pub resource: Resource,
    pub action: Action,
}

#[derive(Debug, Clone)]
pub struct AgentWithToken {
    pub agent: Agent,
    pub token: String,
}

fn require_org_admin(ctx: &PrincipalCtx) -> DomainResult<(OrgId, Option<UserId>)> {
    match ctx {
        PrincipalCtx::User {
            user_id,
            org: Some((org_id, role)),
            ..
        } if role.is_org_admin() => Ok((*org_id, Some(*user_id))),
        PrincipalCtx::User {
            org: None,
            org_selector_required: true,
            ..
        } => Err(DomainError::org_selector_required()),
        _ => Err(DomainError::new(
            ErrorCode::Forbidden,
            "agent administration requires an org admin role",
        )
        .with_hint("ask an org owner to grant you the admin role")),
    }
}

fn require_same_org(agent: &Agent, org_id: OrgId) -> DomainResult<()> {
    if agent.org_id == org_id {
        Ok(())
    } else {
        Err(DomainError::not_found("agent", &agent.id.to_string()))
    }
}

fn validate_agent_grant(kind: AgentKind, grant: &AgentGrantInput) -> DomainResult<()> {
    match kind {
        AgentKind::CpTool => {
            if grant.resource.is_governance() {
                Err(DomainError::validation(
                    "cp-tool agents cannot receive governance grants",
                ))
            } else {
                Ok(())
            }
        }
        AgentKind::GatewayTool => {
            if grant.resource == Resource::McpTools
                && matches!(grant.action, Action::Read | Action::Execute)
            {
                Ok(())
            } else {
                Err(DomainError::validation(
                    "gateway-tool agents may only receive mcp-tools:read or mcp-tools:execute grants",
                ))
            }
        }
        AgentKind::ApiConsumer => Err(DomainError::validation(
            "api-consumer agents do not receive MCP grants",
        )),
    }
}

fn mint_agent_token() -> String {
    format!(
        "fpat_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn audit_entry(
    ctx: &PrincipalCtx,
    request_id: RequestId,
    org_id: OrgId,
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
        team_id: None,
        outcome: audit::Outcome::Success,
        detail: serde_json::json!({}),
    }
}

pub async fn create_agent(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    name: &str,
    kind: AgentKind,
    grants: &[AgentGrantInput],
    request_id: RequestId,
) -> DomainResult<AgentWithToken> {
    let (org_id, created_by) = require_org_admin(ctx)?;
    for grant in grants {
        validate_agent_grant(kind, grant)?;
        let team = identity::resolve_team_ref(pool, grant.team_id)
            .await?
            .ok_or_else(|| DomainError::not_found("team", &grant.team_id.to_string()))?;
        if team.org_id != org_id {
            return Err(DomainError::not_found("team", &grant.team_id.to_string()));
        }
    }

    let token = mint_agent_token();
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create agent: begin"))?;
    let agent = identity::create_agent_tx(
        &mut tx,
        org_id,
        name,
        kind,
        &identity::hash_agent_token(&token),
        created_by,
    )
    .await?;
    for grant in grants {
        identity::add_agent_grant_in_tx(
            &mut tx,
            agent.id,
            org_id,
            grant.team_id,
            grant.resource,
            grant.action,
            created_by,
        )
        .await?;
    }
    audit::record_in_tx(
        &mut tx,
        &audit_entry(
            ctx,
            request_id,
            org_id,
            "agent.create",
            format!("agents/{name}"),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("create agent: commit"))?;

    Ok(AgentWithToken { agent, token })
}

/// The org and row scope an authorized agent read runs under.
struct AgentReadScope {
    org_id: OrgId,
    /// `None` — org admin, reads every agent in the org. `Some(teams)` — non-admin, reads only
    /// agents holding a grant on one of these teams (empty ⇒ nothing).
    team_scope: Option<Vec<TeamId>>,
}

/// Shared read gate for the agent read surface (`list_agents`, `get_agent`, `list_agent_grants`).
/// Unlike the grant surface's `teams::authorize`, this authorizes at `team: None` and returns the
/// caller's row scope instead of unit. `resource` is `Resource::Agents` for the agent reads and
/// `Resource::Grants` for the grants read — it drives both the engine call and the team scope, so
/// a non-admin's visible rows are exactly the teams it holds `<resource>:read` on.
///
/// Order is load-bearing (see the ui-f6b design §3): the `org_selector_required` short-circuit
/// runs **before** the engine so a multi-org caller with no selector gets a client-correctable
/// `400`, never an audited authz denial — the auth middleware only records the flag, and the
/// `require_org_admin` matcher that produced that `400` no longer runs for these reads. After the
/// engine allows, the caller has a resolved active org (the engine denies team-less tenant reads
/// with `org: None`), so row scope comes from the active org role, org-admin first.
async fn authorize_read(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    resource: Resource,
    request_id: RequestId,
) -> DomainResult<AgentReadScope> {
    if let PrincipalCtx::User {
        org: None,
        org_selector_required: true,
        ..
    } = ctx
    {
        return Err(DomainError::org_selector_required());
    }

    match check_resource_access(ctx, resource, Action::Read, None) {
        Decision::Allow(_) => {}
        Decision::Deny(reason) => {
            record_authz_denial(pool, ctx, request_id, resource, Action::Read, None, reason).await;
            return Err(deny_to_error(resource, Action::Read, reason));
        }
    }

    let scope = match ctx {
        PrincipalCtx::User {
            org: Some((org_id, role)),
            grants,
            ..
        } => {
            if role.is_org_admin() {
                AgentReadScope {
                    org_id: *org_id,
                    team_scope: None,
                }
            } else {
                AgentReadScope {
                    org_id: *org_id,
                    team_scope: Some(
                        grants
                            .teams_for_in_org(resource, Action::Read, *org_id)
                            .into_iter()
                            .collect(),
                    ),
                }
            }
        }
        PrincipalCtx::Agent { org_id, grants, .. } => AgentReadScope {
            org_id: *org_id,
            team_scope: Some(
                grants
                    .teams_for_in_org(resource, Action::Read, *org_id)
                    .into_iter()
                    .collect(),
            ),
        },
        // The engine denies a team-less tenant read with no active org, so an Allow here always
        // carries one. Reaching this arm would be an engine/authz contract violation — fail closed.
        PrincipalCtx::User { org: None, .. } => {
            return Err(DomainError::internal(
                "agent read authorized without an active org",
            ));
        }
    };
    Ok(scope)
}

/// Paged agent list, row-scoped and optionally filtered to one team. Returns `(page, total)`.
///
/// `team_filter` is a resolved [`TeamRef`] (the handler resolves `?team=` by name or UUID). The
/// UUID resolver is **global** — it discloses no org and never runs the engine's cross-org branch
/// because this endpoint authorizes at `team: None` — so the cross-org check is made explicit
/// here: a filter team outside the caller's active org renders `404` (same as an unresolvable
/// value), before any row query. `total` is the post-scope, post-filter authorized count.
pub async fn list_agents(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team_filter: Option<TeamRef>,
    limit: i64,
    offset: i64,
    request_id: RequestId,
) -> DomainResult<(Vec<Agent>, i64)> {
    let scope = authorize_read(pool, ctx, Resource::Agents, request_id).await?;

    let filter_team_id = match team_filter {
        Some(team) => {
            if team.org_id != scope.org_id {
                return Err(DomainError::not_found("team", &team.id.to_string()));
            }
            Some(team.id)
        }
        None => None,
    };

    let (is_admin, caller_teams) = match scope.team_scope {
        None => (true, Vec::new()),
        Some(teams) => (false, teams),
    };

    identity::list_agents_paged(
        pool,
        scope.org_id,
        is_admin,
        &caller_teams,
        filter_team_id,
        limit,
        offset,
    )
    .await
}

pub async fn get_agent(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    agent_id: AgentId,
    request_id: RequestId,
) -> DomainResult<Agent> {
    let scope = authorize_read(pool, ctx, Resource::Agents, request_id).await?;
    identity::get_agent_scoped(pool, scope.org_id, agent_id, scope.team_scope.as_deref())
        .await?
        .ok_or_else(|| DomainError::not_found("agent", &agent_id.to_string()))
}

/// Paged listing of one agent's grant rows, authorized on `Resource::Grants` and row-scoped to the
/// caller's `grants:read` teams. Returns `(page, total)`.
///
/// Authorization is the shared read gate on `Resource::Grants` — a caller holding `agents:read`
/// but no `grants:read` is denied `403` here even though it can list agents. The addressed agent
/// must be in the caller's active org (a cross-org or unknown id renders `404`, anti-enumeration),
/// checked before the grant query. Unlike the agents list, this endpoint **rejects a negative
/// `offset` with `400`** (design §1), rather than flooring it.
pub async fn list_agent_grants(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    agent_id: AgentId,
    limit: i64,
    offset: i64,
    request_id: RequestId,
) -> DomainResult<(Vec<identity::AgentGrantRow>, i64)> {
    let scope = authorize_read(pool, ctx, Resource::Grants, request_id).await?;

    if offset < 0 {
        return Err(DomainError::validation("offset must not be negative"));
    }

    // The agent must exist in the caller's active org; otherwise 404 (same for cross-org and
    // unknown — no existence oracle). This runs before the grant query.
    identity::get_agent(pool, scope.org_id, agent_id)
        .await?
        .ok_or_else(|| DomainError::not_found("agent", &agent_id.to_string()))?;

    let (is_admin, caller_teams) = match scope.team_scope {
        None => (true, Vec::new()),
        Some(teams) => (false, teams),
    };

    identity::list_agent_grants_paged(
        pool,
        scope.org_id,
        agent_id,
        is_admin,
        &caller_teams,
        limit,
        offset,
    )
    .await
}

pub async fn rotate_agent_token(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    agent_id: AgentId,
    request_id: RequestId,
) -> DomainResult<AgentWithToken> {
    let (org_id, _) = require_org_admin(ctx)?;
    // Admin path: fetch org-scoped directly (the read-surface `get_agent` now row-scopes and is
    // for readers, not this admin mutation). `require_same_org` stays an explicit invariant.
    let current = identity::get_agent(pool, org_id, agent_id)
        .await?
        .ok_or_else(|| DomainError::not_found("agent", &agent_id.to_string()))?;
    require_same_org(&current, org_id)?;
    let token = mint_agent_token();
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("rotate agent token: begin"))?;
    let agent = identity::rotate_agent_token_tx(
        &mut tx,
        org_id,
        agent_id,
        &identity::hash_agent_token(&token),
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &audit_entry(
            ctx,
            request_id,
            org_id,
            "agent.token.rotate",
            format!("agents/{agent_id}"),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("rotate agent token: commit"))?;
    Ok(AgentWithToken { agent, token })
}

pub async fn disable_agent(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    agent_id: AgentId,
    request_id: RequestId,
) -> DomainResult<Agent> {
    let (org_id, _) = require_org_admin(ctx)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("disable agent: begin"))?;
    let agent = identity::disable_agent_tx(&mut tx, org_id, agent_id).await?;
    audit::record_in_tx(
        &mut tx,
        &audit_entry(
            ctx,
            request_id,
            org_id,
            "agent.disable",
            format!("agents/{agent_id}"),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("disable agent: commit"))?;
    Ok(agent)
}
