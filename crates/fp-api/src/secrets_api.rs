//! Secret management endpoints. Values are write-only: create/rotate accept a SecretSpec,
//! but every response is metadata plus `value_redacted = true`.

use crate::error::{ApiError, ErrorBody};
use crate::resources::{resolve_team, ListQuery, Page};
use crate::state::AppState;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use fp_core::services::secrets as svc;
use fp_core::PrincipalCtx;
use fp_domain::{RequestId, Secret, SecretSpec};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct SecretView {
    pub id: uuid::Uuid,
    pub team_id: uuid::Uuid,
    pub name: String,
    pub description: String,
    pub secret_type: String,
    pub revision: i64,
    pub encryption_key_id: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub value_redacted: bool,
}

impl From<Secret> for SecretView {
    fn from(value: Secret) -> Self {
        Self {
            id: value.id.as_uuid(),
            team_id: value.team_id.as_uuid(),
            name: value.name,
            description: value.description,
            secret_type: value.secret_type.as_str().into(),
            revision: value.version,
            encryption_key_id: value.encryption_key_id,
            expires_at: value.expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
            value_redacted: true,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSecretBody {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub spec: serde_json::Value,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RotateSecretBody {
    pub spec: serde_json::Value,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/secrets",
    tag = "Secrets",
    params(("team" = String, Path, description = "Team name or UUID"), ListQuery),
    responses(
        (status = 200, body = Page<SecretView>),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ))]
pub async fn list_secrets(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Query(query): Query<ListQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Page<SecretView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::list_secrets(&state.pool, &ctx, team, query.limit, query.offset, rid).await
    };
    let (items, total) = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(Page {
        items: items.into_iter().map(SecretView::from).collect(),
        total,
        limit: query.limit.clamp(1, 500),
        offset: query.offset.max(0),
    }))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/secrets",
    tag = "Secrets",
    params(("team" = String, Path, description = "Team name or UUID")),
    request_body = CreateSecretBody,
    responses(
        (status = 201, body = SecretView),
        (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 409, body = ErrorBody),
        (status = 503, body = ErrorBody),
    ))]
pub async fn create_secret(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<CreateSecretBody>,
) -> Result<(StatusCode, Json<SecretView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::create_secret(
            &state.pool,
            &ctx,
            team,
            svc::SecretWrite {
                name: &body.name,
                description: &body.description,
                spec: parse_spec(body.spec)?,
                expires_at: body.expires_at,
            },
            rid,
        )
        .await
    };
    let secret = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((StatusCode::CREATED, Json(SecretView::from(secret))))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/secrets/{name}",
    tag = "Secrets",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "Secret name"),
    ),
    responses(
        (status = 200, body = SecretView),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ))]
pub async fn get_secret(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<SecretView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::get_secret(&state.pool, &ctx, team, &name, rid).await
    };
    run.await
        .map(|secret| Json(SecretView::from(secret)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/secrets/{name}/rotate",
    tag = "Secrets",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "Secret name"),
    ),
    request_body = RotateSecretBody,
    responses(
        (status = 200, body = SecretView),
        (status = 400, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 503, body = ErrorBody),
    ))]
pub async fn rotate_secret(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<RotateSecretBody>,
) -> Result<Json<SecretView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::rotate_secret(
            &state.pool,
            &ctx,
            team,
            &name,
            parse_spec(body.spec)?,
            body.expires_at,
            rid,
        )
        .await
    };
    run.await
        .map(|secret| Json(SecretView::from(secret)))
        .map_err(|e| ApiError::new(e, rid))
}

fn parse_spec(value: serde_json::Value) -> Result<SecretSpec, fp_domain::DomainError> {
    serde_json::from_value(value).map_err(|e| {
        fp_domain::DomainError::validation(format!("invalid secret spec: {e}")).with_hint(
            "include a tagged spec object, e.g. {\"type\":\"generic_secret\",\"secret\":\"...\"}",
        )
    })
}
