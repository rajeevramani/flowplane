use axum::{extract::State, Extension, Json};
use serde::Serialize;
use tracing::instrument;
use utoipa::ToSchema;

use crate::api::error::ApiError;
use crate::api::routes::ApiState;
use crate::auth::authorization::has_admin_bypass;
use crate::auth::models::AuthContext;
use crate::storage::repositories::AdminSummaryRepository;

/// Summary totals across all tenant orgs.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SummaryTotals {
    pub teams: i64,
    pub clusters: i64,
    pub listeners: i64,
    pub route_configs: i64,
    pub filters: i64,
    pub dataplanes: i64,
    pub secrets: i64,
    pub imports: i64,
}

/// Per-team resource breakdown.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamSummary {
    pub team_name: String,
    pub team_display_name: String,
    pub clusters: i64,
    pub listeners: i64,
    pub route_configs: i64,
    pub filters: i64,
    pub dataplanes: i64,
    pub secrets: i64,
    pub imports: i64,
}

/// Per-org summary with nested teams.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrgSummary {
    pub org_id: Option<String>,
    pub org_name: Option<String>,
    pub teams: Vec<TeamSummary>,
}

/// Top-level admin resource summary.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminResourceSummary {
    pub totals: SummaryTotals,
    pub orgs: Vec<OrgSummary>,
}

/// GET /api/v1/admin/resources/summary
///
/// Returns aggregated resource counts per team and org for the platform admin dashboard.
/// Requires `admin:all` scope.
#[utoipa::path(
    get,
    path = "/api/v1/admin/resources/summary",
    responses(
        (status = 200, description = "Resource summary", body = AdminResourceSummary),
        (status = 403, description = "Admin privileges required"),
        (status = 503, description = "Service unavailable")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "Administration"
)]
#[instrument(skip(state, context), fields(user_id = ?context.user_id))]
pub async fn admin_resource_summary_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
) -> Result<Json<AdminResourceSummary>, ApiError> {
    if !has_admin_bypass(&context) {
        return Err(ApiError::forbidden("Admin privileges required"));
    }

    let pool = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Database unavailable"))?
        .pool()
        .clone();

    let repo = AdminSummaryRepository::new(pool);
    let rows = repo.get_resource_summary().await.map_err(ApiError::from)?;

    // Group by org
    let mut org_map: std::collections::BTreeMap<Option<String>, OrgSummary> =
        std::collections::BTreeMap::new();

    let mut totals = SummaryTotals {
        teams: 0,
        clusters: 0,
        listeners: 0,
        route_configs: 0,
        filters: 0,
        dataplanes: 0,
        secrets: 0,
        imports: 0,
    };

    for row in rows {
        totals.teams += 1;
        totals.clusters += row.cluster_count;
        totals.listeners += row.listener_count;
        totals.route_configs += row.route_config_count;
        totals.filters += row.filter_count;
        totals.dataplanes += row.dataplane_count;
        totals.secrets += row.secret_count;
        totals.imports += row.import_count;

        let entry = org_map.entry(row.org_name.clone()).or_insert_with(|| OrgSummary {
            org_id: row.org_id.clone(),
            org_name: row.org_name.clone(),
            teams: Vec::new(),
        });

        entry.teams.push(TeamSummary {
            team_name: row.team_name,
            team_display_name: row.team_display_name,
            clusters: row.cluster_count,
            listeners: row.listener_count,
            route_configs: row.route_config_count,
            filters: row.filter_count,
            dataplanes: row.dataplane_count,
            secrets: row.secret_count,
            imports: row.import_count,
        });
    }

    let orgs: Vec<OrgSummary> = org_map.into_values().collect();

    Ok(Json(AdminResourceSummary { totals, orgs }))
}
