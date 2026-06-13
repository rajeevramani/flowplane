//! Dataplane management endpoints. The certificate registry and xDS binding internals
//! shipped in S5.4; S6 exposes the operator-facing REST surface.

use crate::error::{ApiError, ErrorBody};
use crate::resources::{resolve_team, ListQuery, Page};
use crate::state::AppState;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use fp_core::services::dataplanes as svc;
use fp_core::PrincipalCtx;
use fp_domain::dataplane::{Dataplane, ProxyCertificate};
use fp_domain::RequestId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct DataplaneView {
    pub id: uuid::Uuid,
    pub team_id: uuid::Uuid,
    pub name: String,
    pub description: String,
    pub revision: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<Dataplane> for DataplaneView {
    fn from(value: Dataplane) -> Self {
        Self {
            id: value.id.as_uuid(),
            team_id: value.team_id.as_uuid(),
            name: value.name,
            description: value.description,
            revision: value.version,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateDataplaneBody {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProxyCertificateView {
    pub id: uuid::Uuid,
    pub team_id: uuid::Uuid,
    pub dataplane_id: uuid::Uuid,
    pub spiffe_uri: String,
    pub serial_number: String,
    pub issued_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub revoked_reason: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<ProxyCertificate> for ProxyCertificateView {
    fn from(value: ProxyCertificate) -> Self {
        Self {
            id: value.id.as_uuid(),
            team_id: value.team_id.as_uuid(),
            dataplane_id: value.dataplane_id.as_uuid(),
            spiffe_uri: value.spiffe_uri,
            serial_number: value.serial_number,
            issued_at: value.issued_at,
            expires_at: value.expires_at,
            revoked_at: value.revoked_at,
            revoked_reason: value.revoked_reason,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterProxyCertificateBody {
    pub dataplane: String,
    pub spiffe_uri: String,
    pub serial_number: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeProxyCertificateBody {
    pub reason: String,
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/dataplanes",
    tag = "Dataplanes",
    params(("team" = String, Path, description = "Team name or UUID"), ListQuery),
    responses(
        (status = 200, body = Page<DataplaneView>),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ))]
pub async fn list_dataplanes(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Query(query): Query<ListQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Page<DataplaneView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::list_dataplanes(&state.pool, &ctx, team, query.limit, query.offset, rid).await
    };
    let (items, total) = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(Page {
        items: items.into_iter().map(DataplaneView::from).collect(),
        total,
        limit: query.limit.clamp(1, 500),
        offset: query.offset.max(0),
    }))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/dataplanes",
    tag = "Dataplanes",
    params(("team" = String, Path, description = "Team name or UUID")),
    request_body = CreateDataplaneBody,
    responses(
        (status = 201, body = DataplaneView),
        (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 409, body = ErrorBody),
    ))]
pub async fn create_dataplane(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<CreateDataplaneBody>,
) -> Result<(StatusCode, Json<DataplaneView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::create_dataplane(&state.pool, &ctx, team, &body.name, &body.description, rid).await
    };
    let created = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((StatusCode::CREATED, Json(DataplaneView::from(created))))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/dataplanes/{name}",
    tag = "Dataplanes",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "Dataplane name"),
    ),
    responses(
        (status = 200, body = DataplaneView),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ))]
pub async fn get_dataplane(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<DataplaneView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::get_dataplane(&state.pool, &ctx, team, &name, rid).await
    };
    run.await
        .map(|v| Json(DataplaneView::from(v)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/proxy-certificates",
    tag = "Dataplanes",
    params(("team" = String, Path, description = "Team name or UUID")),
    responses(
        (status = 200, body = Vec<ProxyCertificateView>),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ))]
pub async fn list_proxy_certificates(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Vec<ProxyCertificateView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::list_certificates(&state.pool, &ctx, team, rid).await
    };
    run.await
        .map(|items| Json(items.into_iter().map(ProxyCertificateView::from).collect()))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/proxy-certificates",
    tag = "Dataplanes",
    params(("team" = String, Path, description = "Team name or UUID")),
    request_body = RegisterProxyCertificateBody,
    responses(
        (status = 201, body = ProxyCertificateView),
        (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody),
    ))]
pub async fn register_proxy_certificate(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<RegisterProxyCertificateBody>,
) -> Result<(StatusCode, Json<ProxyCertificateView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::register_certificate(
            &state.pool,
            &ctx,
            team,
            svc::CertificateRegistration {
                dataplane: &body.dataplane,
                spiffe_uri: &body.spiffe_uri,
                serial_number: &body.serial_number,
                expires_at: body.expires_at,
            },
            rid,
        )
        .await
    };
    let cert = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((StatusCode::CREATED, Json(ProxyCertificateView::from(cert))))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/proxy-certificates/{serial_number}/revoke",
    tag = "Dataplanes",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("serial_number" = String, Path, description = "Certificate serial number"),
    ),
    request_body = RevokeProxyCertificateBody,
    responses(
        (status = 200, body = ProxyCertificateView),
        (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody),
    ))]
pub async fn revoke_proxy_certificate(
    State(state): State<AppState>,
    Path((team, serial_number)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<RevokeProxyCertificateBody>,
) -> Result<Json<ProxyCertificateView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::revoke_certificate(&state.pool, &ctx, team, &serial_number, &body.reason, rid).await
    };
    run.await
        .map(|cert| Json(ProxyCertificateView::from(cert)))
        .map_err(|e| ApiError::new(e, rid))
}
