//! Expose shortcut REST surface. The shortcut creates/deletes normal gateway resources; it is
//! operator UX, not a separate product source of truth.

use crate::error::{ApiError, ErrorBody};
use crate::resources::{resolve_team, ClusterView, ListenerView, RouteConfigView};
use crate::state::AppState;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::Json;
use fp_core::services::expose as svc;
use fp_core::PrincipalCtx;
use fp_domain::RequestId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExposeBody {
    pub name: String,
    pub upstream: String,
    #[serde(default = "default_path")]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

fn default_path() -> String {
    "/".into()
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExposeView {
    pub name: String,
    pub upstream: String,
    pub path: String,
    pub port: u16,
    pub curl_url: String,
    pub cluster: ClusterView,
    pub route_config: RouteConfigView,
    pub listener: ListenerView,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UnexposeView {
    pub name: String,
    pub cluster_name: String,
    pub route_config_name: String,
    pub listener_name: String,
}

impl From<svc::ExposedService> for ExposeView {
    fn from(value: svc::ExposedService) -> Self {
        Self {
            name: value.name,
            upstream: value.upstream,
            path: value.path,
            port: value.port,
            curl_url: value.curl_url,
            cluster: ClusterView::from(value.cluster),
            route_config: RouteConfigView::from(value.route_config),
            listener: ListenerView::from(value.listener),
        }
    }
}

impl From<svc::UnexposedService> for UnexposeView {
    fn from(value: svc::UnexposedService) -> Self {
        Self {
            name: value.name,
            cluster_name: value.cluster_name,
            route_config_name: value.route_config_name,
            listener_name: value.listener_name,
        }
    }
}

/// Create a cluster, route config, and listener for one upstream.
#[utoipa::path(post, path = "/api/v1/teams/{team}/expose",
    tag = "Expose",
    params(("team" = String, Path, description = "Team name or UUID")),
    request_body = ExposeBody,
    responses(
        (status = 201, body = ExposeView),
        (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody),
    ))]
pub async fn expose(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<ExposeBody>,
) -> Result<(StatusCode, Json<ExposeView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::expose(
            &state.pool,
            &ctx,
            team,
            svc::ExposeRequest {
                name: body.name,
                upstream: body.upstream,
                path: body.path,
                port: body.port,
            },
            rid,
        )
        .await
    };
    let exposed = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((StatusCode::CREATED, Json(ExposeView::from(exposed))))
}

/// Delete the listener, route config, and cluster created by `expose`.
#[utoipa::path(delete, path = "/api/v1/teams/{team}/expose/{name}",
    tag = "Expose",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "Expose shortcut name"),
    ),
    responses(
        (status = 200, body = UnexposeView),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody),
    ))]
pub async fn unexpose(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<UnexposeView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::unexpose(&state.pool, &ctx, team, &name, rid).await
    };
    run.await
        .map(|removed| Json(UnexposeView::from(removed)))
        .map_err(|e| ApiError::new(e, rid))
}
