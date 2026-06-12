//! Organization endpoints (platform governance + org-admin member management).

use crate::error::{ApiError, ErrorBody};
use crate::state::AppState;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::Json;
use fp_core::services::orgs as svc;
use fp_core::PrincipalCtx;
use fp_domain::{DomainError, DomainResult, OrgId, OrgRole, RequestId, UserId};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct OrgView {
    pub id: uuid::Uuid,
    pub name: String,
    pub display_name: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateOrgBody {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
}

#[derive(Serialize, ToSchema)]
pub struct OrgMemberView {
    pub user_id: uuid::Uuid,
    pub email: String,
    pub name: String,
    pub role: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AddOrgMemberBody {
    pub email: String,
    /// One of: viewer, member, admin, owner.
    pub role: String,
}

async fn resolve_org(state: &AppState, raw: &str) -> DomainResult<OrgId> {
    if let Ok(id) = OrgId::from_str(raw) {
        return Ok(id);
    }
    fp_storage::repos::identity::resolve_org_by_name(&state.pool, raw)
        .await?
        .ok_or_else(|| DomainError::not_found("organization", raw))
}

fn view(org: fp_domain::Organization) -> OrgView {
    OrgView {
        id: org.id.as_uuid(),
        name: org.name,
        display_name: org.display_name,
    }
}

#[utoipa::path(get, path = "/api/v1/orgs", tag = "Organizations",
    responses((status = 200, body = [OrgView]), (status = 403, body = ErrorBody)))]
pub async fn list_orgs(
    State(state): State<AppState>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Vec<OrgView>>, ApiError> {
    let orgs = svc::list_orgs(&state.pool, &ctx)
        .await
        .map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(orgs.into_iter().map(view).collect()))
}

#[utoipa::path(post, path = "/api/v1/orgs", tag = "Organizations",
    request_body = CreateOrgBody,
    responses((status = 201, body = OrgView), (status = 403, body = ErrorBody),
              (status = 409, body = ErrorBody)))]
pub async fn create_org(
    State(state): State<AppState>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<CreateOrgBody>,
) -> Result<(StatusCode, Json<OrgView>), ApiError> {
    let org = svc::create_org(&state.pool, &ctx, &body.name, &body.display_name, rid)
        .await
        .map_err(|e| ApiError::new(e, rid))?;
    Ok((StatusCode::CREATED, Json(view(org))))
}

#[utoipa::path(get, path = "/api/v1/orgs/{org}", tag = "Organizations",
    params(("org" = String, Path, description = "Organization name or UUID")),
    responses((status = 200, body = OrgView), (status = 404, body = ErrorBody)))]
pub async fn get_org(
    State(state): State<AppState>,
    Path(org): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<OrgView>, ApiError> {
    let run = async {
        let org_id = resolve_org(&state, &org).await?;
        svc::get_org(&state.pool, &ctx, org_id).await
    };
    run.await
        .map(|o| Json(view(o)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(delete, path = "/api/v1/orgs/{org}", tag = "Organizations",
    params(("org" = String, Path, description = "Organization name or UUID")),
    responses((status = 204), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub async fn delete_org(
    State(state): State<AppState>,
    Path(org): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<StatusCode, ApiError> {
    let run = async {
        let org_id = resolve_org(&state, &org).await?;
        svc::delete_org(&state.pool, &ctx, org_id, rid).await
    };
    run.await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(get, path = "/api/v1/orgs/{org}/members", tag = "Organizations",
    params(("org" = String, Path, description = "Organization name or UUID")),
    responses((status = 200, body = [OrgMemberView]), (status = 404, body = ErrorBody)))]
pub async fn list_members(
    State(state): State<AppState>,
    Path(org): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Vec<OrgMemberView>>, ApiError> {
    let run = async {
        let org_id = resolve_org(&state, &org).await?;
        svc::list_org_members(&state.pool, &ctx, org_id).await
    };
    let members = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(
        members
            .into_iter()
            .map(|(id, email, name, role)| OrgMemberView {
                user_id: id.as_uuid(),
                email,
                name,
                role,
            })
            .collect(),
    ))
}

#[utoipa::path(post, path = "/api/v1/orgs/{org}/members", tag = "Organizations",
    params(("org" = String, Path, description = "Organization name or UUID")),
    request_body = AddOrgMemberBody,
    responses((status = 204), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody)))]
pub async fn add_member(
    State(state): State<AppState>,
    Path(org): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<AddOrgMemberBody>,
) -> Result<StatusCode, ApiError> {
    let run = async {
        let role = OrgRole::parse(&body.role)?;
        let org_id = resolve_org(&state, &org).await?;
        svc::add_org_member(&state.pool, &ctx, org_id, &body.email, role, rid).await
    };
    run.await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(delete, path = "/api/v1/orgs/{org}/members/{user_id}", tag = "Organizations",
    params(("org" = String, Path, description = "Organization name or UUID"),
           ("user_id" = uuid::Uuid, Path, description = "Member user id")),
    responses((status = 204), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub async fn remove_member(
    State(state): State<AppState>,
    Path((org, user_id)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<StatusCode, ApiError> {
    let run = async {
        let user_id = UserId::from_str(&user_id)?;
        let org_id = resolve_org(&state, &org).await?;
        svc::remove_org_member(&state.pool, &ctx, org_id, user_id, rid).await
    };
    run.await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::new(e, rid))
}
