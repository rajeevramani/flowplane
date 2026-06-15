//! Per-tenant resource quotas (spec/10 §4): defaults here, per-team overrides land with
//! the platform-admin surface. Enforced in the service layer before any write.

use fp_domain::authz::Resource;
use fp_domain::{DomainError, DomainResult, ErrorCode, TeamId};
use sqlx::PgPool;

/// Conservative defaults (Q-003 founder defaults); platform-admin overrides come with S4.
pub fn default_limit(resource: Resource) -> i64 {
    match resource {
        Resource::Clusters => 50,
        Resource::Listeners => 25,
        Resource::RouteConfigs => 100,
        Resource::LearningSessions => 5,
        Resource::ApiDefinitions | Resource::Secrets | Resource::Dataplanes => 200,
        _ => 200,
    }
}

pub async fn check_team_resource_quota(
    pool: &PgPool,
    team_id: TeamId,
    resource: Resource,
) -> DomainResult<()> {
    let used = match resource {
        Resource::Clusters => fp_storage::repos::clusters::count_for_team(pool, team_id).await?,
        Resource::RouteConfigs => {
            fp_storage::repos::gateway::count_route_configs(pool, team_id).await?
        }
        Resource::Listeners => fp_storage::repos::gateway::count_listeners(pool, team_id).await?,
        Resource::ApiDefinitions => {
            fp_storage::repos::api_lifecycle::count_api_definitions_for_team(pool, team_id).await?
        }
        Resource::LearningSessions => {
            fp_storage::repos::api_lifecycle::count_capture_sessions_for_team(pool, team_id).await?
        }
        Resource::Secrets => fp_storage::repos::secrets::count_for_team(pool, team_id).await?,
        Resource::AiProviders => fp_storage::repos::ai::count_for_team(pool, team_id).await?,
        Resource::AiRoutes => fp_storage::repos::ai::count_routes_for_team(pool, team_id).await?,
        Resource::Dataplanes => {
            fp_storage::repos::dataplanes::count_for_team(pool, team_id).await?
        }
        _ => return Ok(()),
    };
    let limit = default_limit(resource);
    if used >= limit {
        return Err(DomainError::new(
            ErrorCode::QuotaExceeded,
            format!("team quota reached: {used}/{limit} {}", resource.as_str()),
        )
        .with_hint("delete unused resources or ask a platform admin to raise the quota"));
    }
    Ok(())
}
