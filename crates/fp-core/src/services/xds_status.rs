//! xDS health surfacing (S5.5): per-team NACK history. Read-only; quarantine state lives
//! in the xDS snapshot cache and reaches operators through these persisted events.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{deny_to_error, record_authz_denial};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::{DomainResult, RequestId};
use fp_storage::repos::xds_nacks::NackEvent;
use sqlx::PgPool;

pub async fn list_nack_events(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    limit: i64,
    request_id: RequestId,
) -> DomainResult<Vec<NackEvent>> {
    match check_resource_access(ctx, Resource::Stats, Action::Read, Some(team)) {
        Decision::Allow(_) => {}
        Decision::Deny(reason) => {
            record_authz_denial(
                pool,
                ctx,
                request_id,
                Resource::Stats,
                Action::Read,
                Some(team),
                reason,
            )
            .await;
            return Err(deny_to_error(Resource::Stats, Action::Read, reason));
        }
    }
    fp_storage::repos::xds_nacks::list(pool, team.id, limit).await
}
