//! xDS health surface (S5.5): per-team NACK/quarantine history. Read-only — what a
//! dataplane rejected, when, and which resources are degraded (serving last-good bytes).

use crate::error::{ApiError, ErrorBody};
use crate::resources::resolve_team;
use crate::state::AppState;
use axum::extract::{Extension, Path, State};
use axum::Json;
use fp_core::PrincipalCtx;
use fp_domain::RequestId;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct NackEventView {
    pub id: uuid::Uuid,
    /// Envoy node id of the rejecting dataplane (attribution only).
    pub node_id: String,
    pub type_url: String,
    pub version_rejected: String,
    pub error_message: String,
    /// Resources quarantined by this NACK (now serving their last-good bytes, or held
    /// out of the snapshot when they were new). Empty when attribution was impossible.
    pub quarantined_resources: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Recent configuration rejections (NACKs) from this team's dataplanes, newest first.
#[utoipa::path(get, path = "/api/v1/teams/{team}/xds/nacks", tag = "XdsStatus",
    params(("team" = String, Path, description = "Team name or UUID")),
    responses((status = 200, body = [NackEventView]), (status = 404, body = ErrorBody)))]
pub async fn list_nacks(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Vec<NackEventView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        fp_core::services::xds_status::list_nack_events(&state.pool, &ctx, team, 100, rid).await
    };
    let events = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(
        events
            .into_iter()
            .map(|e| NackEventView {
                id: e.id,
                node_id: e.node_id,
                type_url: e.type_url,
                version_rejected: e.version_rejected,
                error_message: e.error_message,
                quarantined_resources: e.quarantined_resources,
                created_at: e.created_at,
            })
            .collect(),
    ))
}
