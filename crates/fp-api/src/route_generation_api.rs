//! S9 route generation plan REST endpoints.

use crate::error::ApiError;
use crate::extract::ApiJson;
use crate::resources::resolve_team;
use crate::state::AppState;
use axum::extract::{Extension, Path, State};
use axum::Json;
use fp_core::services::route_generation as svc;
use fp_core::PrincipalCtx;
use fp_domain::{RequestId, RouteGenerationPlan, RouteGenerationPlanId, SpecVersionId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRoutePlanBody {
    pub spec_version_id: uuid::Uuid,
    pub listener_port: u16,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RouteGenerationPlanView {
    pub id: uuid::Uuid,
    pub spec_version_id: uuid::Uuid,
    pub status: String,
    pub plan: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<RouteGenerationPlan> for RouteGenerationPlanView {
    fn from(value: RouteGenerationPlan) -> Self {
        Self {
            id: value.id.as_uuid(),
            spec_version_id: value.spec_version_id.as_uuid(),
            status: value.status.as_str().into(),
            plan: serde_json::to_value(value.plan).unwrap_or_else(|_| serde_json::json!({})),
            applied_at: value.applied_at,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApplyRoutePlanView {
    pub plan: RouteGenerationPlanView,
    pub cluster: String,
    pub route_config: String,
    pub listener: String,
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/route-generation-plans",
    tag = "RouteGeneration",
    params(("team" = String, Path, description = "Team name or UUID")),
    request_body = CreateRoutePlanBody,
    responses(
        (status = 201, body = RouteGenerationPlanView),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    ))]
pub async fn create_route_plan(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<CreateRoutePlanBody>,
) -> Result<(axum::http::StatusCode, Json<RouteGenerationPlanView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::create_plan(
            &state.pool,
            &ctx,
            team,
            svc::CreateRoutePlanInput {
                spec_version_id: SpecVersionId::from(body.spec_version_id),
                listener_port: body.listener_port,
            },
            rid,
        )
        .await
    };
    let plan = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(RouteGenerationPlanView::from(plan)),
    ))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/route-generation-plans/{plan_id}/apply",
    tag = "RouteGeneration",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("plan_id" = uuid::Uuid, Path, description = "Route generation plan ID"),
    ),
    responses(
        (status = 200, body = ApplyRoutePlanView),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    ))]
pub async fn apply_route_plan(
    State(state): State<AppState>,
    Path((team, plan_id)): Path<(String, uuid::Uuid)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<ApplyRoutePlanView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::apply_plan_with_egress_policy(
            &state.pool,
            &ctx,
            team,
            RouteGenerationPlanId::from(plan_id),
            rid,
            &state.egress_policy,
        )
        .await
    };
    let applied = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(ApplyRoutePlanView {
        plan: RouteGenerationPlanView::from(applied.plan),
        cluster: applied.cluster.name,
        route_config: applied.route_config.name,
        listener: applied.listener.name,
    }))
}
