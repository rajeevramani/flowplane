//! Team, membership, and grant endpoints (D-002: every identity workflow has an API/CLI
//! path in v2 — the UI is gone).

use crate::error::{ApiError, ErrorBody};
use crate::resources::resolve_team;
use crate::state::AppState;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::Json;
use fp_core::services::teams as svc;
use fp_core::PrincipalCtx;
use fp_domain::authz::{Action, Resource};
use fp_domain::{DomainError, RequestId, UserId};
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
    Json(body): Json<CreateTeamBody>,
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
    Json(body): Json<AddMemberBody>,
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
        svc::list_grants(&state.pool, &ctx, team).await
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
    Json(body): Json<AddGrantBody>,
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
