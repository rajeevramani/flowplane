//! Persisted route generation plans.

use fp_domain::authz::TeamRef;
use fp_domain::{
    DomainError, DomainResult, RouteGenerationPlan, RouteGenerationPlanId, RouteGenerationPlanSpec,
    RouteGenerationPlanStatus, SpecVersionId, TeamId,
};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row, Transaction};

const COLUMNS: &str = "id, team_id, spec_version_id, status, plan, applied_at, created_at";

fn from_row(row: &PgRow) -> DomainResult<RouteGenerationPlan> {
    let status: String = row.get("status");
    Ok(RouteGenerationPlan {
        id: RouteGenerationPlanId::from(row.get::<uuid::Uuid, _>("id")),
        team_id: TeamId::from(row.get::<uuid::Uuid, _>("team_id")),
        spec_version_id: SpecVersionId::from(row.get::<uuid::Uuid, _>("spec_version_id")),
        status: RouteGenerationPlanStatus::parse(&status)?,
        plan: serde_json::from_value(row.get("plan"))
            .map_err(|e| DomainError::internal(format!("decode route generation plan: {e}")))?,
        applied_at: row.get("applied_at"),
        created_at: row.get("created_at"),
    })
}

pub async fn create(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    spec_version_id: SpecVersionId,
    plan: &RouteGenerationPlanSpec,
) -> DomainResult<RouteGenerationPlan> {
    let row = sqlx::query(&format!(
        "INSERT INTO route_generation_plans \
         (id, team_id, org_id, api_definition_id, spec_version_id, status, plan) \
         VALUES ($1, $2, $3, $4, $5, 'dry_run', $6) RETURNING {COLUMNS}"
    ))
    .bind(RouteGenerationPlanId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(plan.api_definition_id.as_uuid())
    .bind(spec_version_id.as_uuid())
    .bind(
        serde_json::to_value(plan)
            .map_err(|e| DomainError::internal(format!("encode route generation plan: {e}")))?,
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("create route generation plan: {e}")))?;
    from_row(&row)
}

pub async fn get(
    pool: &PgPool,
    team_id: TeamId,
    plan_id: RouteGenerationPlanId,
) -> DomainResult<Option<RouteGenerationPlan>> {
    let row = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM route_generation_plans WHERE team_id = $1 AND id = $2"
    ))
    .bind(team_id.as_uuid())
    .bind(plan_id.as_uuid())
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get route generation plan: {e}")))?;
    row.as_ref().map(from_row).transpose()
}

pub async fn mark_applied(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    plan_id: RouteGenerationPlanId,
) -> DomainResult<RouteGenerationPlan> {
    let row = sqlx::query(&format!(
        "UPDATE route_generation_plans \
         SET status = 'applied', applied_at = now() \
         WHERE team_id = $1 AND id = $2 AND status = 'dry_run' \
         RETURNING {COLUMNS}"
    ))
    .bind(team_id.as_uuid())
    .bind(plan_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("mark route generation plan applied: {e}")))?;
    row.as_ref()
        .map(from_row)
        .transpose()?
        .ok_or_else(|| DomainError::conflict("route generation plan is not applicable"))
}
