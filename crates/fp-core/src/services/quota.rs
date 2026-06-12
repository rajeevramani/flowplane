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
        // Other resources adopt this check as their verticals land.
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
