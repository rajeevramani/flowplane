//! Team, membership, and grant endpoints (D-002: every identity workflow has an API/CLI
//! path in v2 — the UI is gone).

use crate::error::{ApiError, ErrorBody};
use crate::extract::ApiJson;
use crate::resources::resolve_team;
use crate::state::AppState;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::Json;
use fp_core::services::teams as svc;
use fp_core::PrincipalCtx;
use fp_domain::authz::{Action, Resource};
use fp_domain::{Agent, AgentId, AgentKind, DomainError, RequestId, UserId};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct TeamView {
    pub id: uuid::Uuid,
    pub name: String,
    pub display_name: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTeamBody {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
}

#[derive(Serialize, ToSchema)]
pub struct MemberView {
    pub user_id: uuid::Uuid,
    pub email: String,
    pub name: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AddMemberBody {
    pub email: String,
}

#[derive(Serialize, ToSchema)]
pub struct GrantView {
    pub id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub resource: String,
    pub action: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AddGrantBody {
    pub email: String,
    /// Resource kind, e.g. "clusters".
    pub resource: String,
    /// One of: read, create, update, delete, execute.
    pub action: String,
}

#[derive(Serialize, ToSchema)]
pub struct AgentView {
    pub id: uuid::Uuid,
    pub org_id: uuid::Uuid,
    pub name: String,
    pub kind: String,
    pub status: String,
}

#[derive(Serialize, ToSchema)]
pub struct AgentTokenView {
    pub agent: AgentView,
    /// Returned exactly once on create/rotate. Only the SHA-256 hash is stored.
    pub token: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentGrantBody {
    pub team_id: uuid::Uuid,
    pub resource: String,
    pub action: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateAgentBody {
    pub name: String,
    /// One of: cp-tool, gateway-tool, api-consumer.
    pub kind: String,
    #[serde(default)]
    pub grants: Vec<AgentGrantBody>,
}

fn agent_view(agent: Agent) -> AgentView {
    AgentView {
        id: agent.id.as_uuid(),
        org_id: agent.org_id.as_uuid(),
        name: agent.name,
        kind: agent.kind.as_str().into(),
        status: agent.status.as_str().into(),
    }
}

fn parse_agent_id(raw: &str) -> Result<AgentId, DomainError> {
    AgentId::from_str(raw)
}

fn agent_grants(
    body: Vec<AgentGrantBody>,
) -> Result<Vec<fp_core::services::agents::AgentGrantInput>, DomainError> {
    body.into_iter()
        .map(|grant| {
            Ok(fp_core::services::agents::AgentGrantInput {
                team_id: fp_domain::TeamId::from(grant.team_id),
                resource: Resource::parse(&grant.resource)?,
                action: Action::parse(&grant.action)?,
            })
        })
        .collect()
}

#[utoipa::path(get, path = "/api/v1/teams", tag = "Teams",
    responses((status = 200, body = [TeamView]), (status = 401, body = ErrorBody)))]
pub async fn list_teams(
    State(state): State<AppState>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Vec<TeamView>>, ApiError> {
    let teams = svc::list_teams(&state.pool, &ctx)
        .await
        .map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(
        teams
            .into_iter()
            .map(|t| TeamView {
                id: t.id.as_uuid(),
                name: t.name,
                display_name: t.display_name,
            })
            .collect(),
    ))
}

#[utoipa::path(post, path = "/api/v1/teams", tag = "Teams",
    request_body = CreateTeamBody,
    responses((status = 201, body = TeamView), (status = 403, body = ErrorBody),
              (status = 409, body = ErrorBody)))]
pub async fn create_team(
    State(state): State<AppState>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<CreateTeamBody>,
) -> Result<(StatusCode, Json<TeamView>), ApiError> {
    let team = svc::create_team(&state.pool, &ctx, &body.name, &body.display_name, rid)
        .await
        .map_err(|e| ApiError::new(e, rid))?;
    Ok((
        StatusCode::CREATED,
        Json(TeamView {
            id: team.id.as_uuid(),
            name: team.name,
            display_name: team.display_name,
        }),
    ))
}

#[utoipa::path(delete, path = "/api/v1/teams/{team}", tag = "Teams",
    params(("team" = String, Path, description = "Team name or UUID")),
    responses((status = 204), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub async fn delete_team(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<StatusCode, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::delete_team(&state.pool, &ctx, team, rid).await
    };
    run.await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/members", tag = "Teams",
    params(("team" = String, Path, description = "Team name or UUID")),
    responses((status = 200, body = [MemberView]), (status = 404, body = ErrorBody)))]
pub async fn list_members(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Vec<MemberView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::list_members(&state.pool, &ctx, team).await
    };
    let members = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(
        members
            .into_iter()
            .map(|(id, email, name)| MemberView {
                user_id: id.as_uuid(),
                email,
                name,
            })
            .collect(),
    ))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/members", tag = "Teams",
    params(("team" = String, Path, description = "Team name or UUID")),
    request_body = AddMemberBody,
    responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub async fn add_member(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<AddMemberBody>,
) -> Result<StatusCode, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::add_member(&state.pool, &ctx, team, &body.email, rid).await
    };
    run.await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(delete, path = "/api/v1/teams/{team}/members/{user_id}", tag = "Teams",
    params(("team" = String, Path, description = "Team name or UUID"),
           ("user_id" = uuid::Uuid, Path, description = "Member user id")),
    responses((status = 204), (status = 404, body = ErrorBody)))]
pub async fn remove_member(
    State(state): State<AppState>,
    Path((team, user_id)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<StatusCode, ApiError> {
    let run = async {
        let user_id = UserId::from_str(&user_id)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::remove_member(&state.pool, &ctx, team, user_id, rid).await
    };
    run.await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/grants", tag = "Grants",
    params(("team" = String, Path, description = "Team name or UUID")),
    responses((status = 200, body = [GrantView]), (status = 404, body = ErrorBody)))]
pub async fn list_grants(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Vec<GrantView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::list_grants(&state.pool, &ctx, team, rid).await
    };
    let grants = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(
        grants
            .into_iter()
            .map(|(id, user_id, resource, action)| GrantView {
                id,
                user_id,
                resource,
                action,
            })
            .collect(),
    ))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/grants", tag = "Grants",
    params(("team" = String, Path, description = "Team name or UUID")),
    request_body = AddGrantBody,
    responses((status = 204), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody)))]
pub async fn add_grant(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<AddGrantBody>,
) -> Result<StatusCode, ApiError> {
    let run = async {
        let resource = Resource::parse(&body.resource)?;
        let action = Action::parse(&body.action)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::add_grant(&state.pool, &ctx, team, &body.email, resource, action, rid).await
    };
    run.await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(delete, path = "/api/v1/teams/{team}/grants/{grant_id}", tag = "Grants",
    params(("team" = String, Path, description = "Team name or UUID"),
           ("grant_id" = uuid::Uuid, Path, description = "Grant id")),
    responses((status = 204), (status = 404, body = ErrorBody)))]
pub async fn remove_grant(
    State(state): State<AppState>,
    Path((team, grant_id)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<StatusCode, ApiError> {
    let run = async {
        let grant_id = uuid::Uuid::parse_str(&grant_id)
            .map_err(|_| DomainError::validation("grant id must be a UUID"))?;
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::remove_grant(&state.pool, &ctx, team, grant_id, rid).await
    };
    run.await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(get, path = "/api/v1/agents", tag = "Agents",
    responses((status = 200, body = [AgentView]), (status = 403, body = ErrorBody)))]
pub async fn list_agents(
    State(state): State<AppState>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Vec<AgentView>>, ApiError> {
    let agents = fp_core::services::agents::list_agents(&state.pool, &ctx)
        .await
        .map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(agents.into_iter().map(agent_view).collect()))
}

#[utoipa::path(post, path = "/api/v1/agents", tag = "Agents",
    request_body = CreateAgentBody,
    responses((status = 201, body = AgentTokenView), (status = 400, body = ErrorBody),
              (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub async fn create_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<CreateAgentBody>,
) -> Result<(StatusCode, Json<AgentTokenView>), ApiError> {
    let grants = agent_grants(body.grants).map_err(|e| ApiError::new(e, rid))?;
    let kind = AgentKind::parse(&body.kind).map_err(|e| ApiError::new(e, rid))?;
    let created =
        fp_core::services::agents::create_agent(&state.pool, &ctx, &body.name, kind, &grants, rid)
            .await
            .map_err(|e| ApiError::new(e, rid))?;
    Ok((
        StatusCode::CREATED,
        Json(AgentTokenView {
            agent: agent_view(created.agent),
            token: created.token,
        }),
    ))
}

#[utoipa::path(get, path = "/api/v1/agents/{agent_id}", tag = "Agents",
    params(("agent_id" = uuid::Uuid, Path, description = "Agent id")),
    responses((status = 200, body = AgentView), (status = 404, body = ErrorBody)))]
pub async fn get_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<AgentView>, ApiError> {
    let run = async {
        let agent_id = parse_agent_id(&agent_id)?;
        fp_core::services::agents::get_agent(&state.pool, &ctx, agent_id).await
    };
    run.await
        .map(|agent| Json(agent_view(agent)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(post, path = "/api/v1/agents/{agent_id}/rotate-token", tag = "Agents",
    params(("agent_id" = uuid::Uuid, Path, description = "Agent id")),
    responses((status = 200, body = AgentTokenView), (status = 404, body = ErrorBody)))]
pub async fn rotate_agent_token(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<AgentTokenView>, ApiError> {
    let run = async {
        let agent_id = parse_agent_id(&agent_id)?;
        fp_core::services::agents::rotate_agent_token(&state.pool, &ctx, agent_id, rid).await
    };
    run.await
        .map(|rotated| {
            Json(AgentTokenView {
                agent: agent_view(rotated.agent),
                token: rotated.token,
            })
        })
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(post, path = "/api/v1/agents/{agent_id}/disable", tag = "Agents",
    params(("agent_id" = uuid::Uuid, Path, description = "Agent id")),
    responses((status = 200, body = AgentView), (status = 404, body = ErrorBody)))]
pub async fn disable_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<AgentView>, ApiError> {
    let run = async {
        let agent_id = parse_agent_id(&agent_id)?;
        fp_core::services::agents::disable_agent(&state.pool, &ctx, agent_id, rid).await
    };
    run.await
        .map(|agent| Json(agent_view(agent)))
        .map_err(|e| ApiError::new(e, rid))
}
