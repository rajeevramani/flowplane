//! Agent lifecycle for S11 MCP prerequisites.

use crate::authz::PrincipalCtx;
use crate::services::actor_of;
use fp_domain::authz::{Action, Resource};
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

pub async fn list_agents(pool: &PgPool, ctx: &PrincipalCtx) -> DomainResult<Vec<Agent>> {
    let (org_id, _) = require_org_admin(ctx)?;
    identity::list_agents_for_org(pool, org_id).await
}

pub async fn get_agent(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    agent_id: AgentId,
) -> DomainResult<Agent> {
    let (org_id, _) = require_org_admin(ctx)?;
    identity::get_agent(pool, org_id, agent_id)
        .await?
        .ok_or_else(|| DomainError::not_found("agent", &agent_id.to_string()))
}

pub async fn rotate_agent_token(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    agent_id: AgentId,
    request_id: RequestId,
) -> DomainResult<AgentWithToken> {
    let (org_id, _) = require_org_admin(ctx)?;
    let current = get_agent(pool, ctx, agent_id).await?;
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
