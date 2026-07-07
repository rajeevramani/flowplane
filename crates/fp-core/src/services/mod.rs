//! Services: the only mutation path. Shared helpers for authorization-to-error mapping,
//! audit actor extraction, trace-context capture, and quotas.

pub mod agents;
pub mod ai;
pub mod api_lifecycle;
pub mod clusters;
pub mod dataplanes;
pub mod discovery;
pub mod egress_policy;
pub mod expose;
pub mod filesystem_path_policy;
pub mod gateway;
pub mod learning;
pub mod orgs;
pub mod quota;
pub mod rate_limit;
pub mod rls_sync;
pub mod route_generation;
pub mod secrets;
pub mod teams;
pub mod xds_status;

use crate::authz::{PrincipalCtx, Reason};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::{DomainError, ErrorCode, RequestId};
use fp_storage::repos::audit::{self, ActorType};
use sqlx::PgPool;

/// Map an authorization denial to the wire error. Only an org-boundary (cross-org) denial
/// renders as `not_found` — anti-enumeration across the hard org boundary (spec/05 §3.2.2,
/// §3.3). Every other denial, including a same-org no-grant denial, is `forbidden` naming the
/// missing (resource, action) so the caller knows what grant to request; this is safe because
/// the grant check denies at team level before any per-resource lookup, so the response is
/// identical whether the named resource exists or not (no existence oracle).
pub fn deny_to_error(resource: Resource, action: Action, reason: Reason) -> DomainError {
    match reason {
        Reason::CrossOrg => DomainError::new(ErrorCode::NotFound, "not found"),
        _ => DomainError::new(
            ErrorCode::Forbidden,
            format!(
                "missing permission: {}:{}",
                resource.as_str(),
                action.as_str()
            ),
        )
        .with_hint(format!(
            "ask a team or org admin for a {}:{} grant",
            resource.as_str(),
            action.as_str()
        ))
        .with_details(serde_json::json!({ "reason": reason.as_str() })),
    }
}

pub fn actor_of(ctx: &PrincipalCtx) -> (ActorType, Option<uuid::Uuid>) {
    match ctx {
        PrincipalCtx::User { user_id, .. } => (ActorType::User, Some(user_id.as_uuid())),
        PrincipalCtx::Agent { agent_id, .. } => (ActorType::Agent, Some(agent_id.as_uuid())),
    }
}

pub async fn record_authz_denial(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    request_id: RequestId,
    resource: Resource,
    action: Action,
    team: Option<TeamRef>,
    reason: Reason,
) {
    metrics::counter!(
        "fp_authz_denied_total",
        "resource" => resource.as_str(),
        "action" => action.as_str()
    )
    .increment(1);

    let (actor_type, actor_id) = actor_of(ctx);
    audit::record_best_effort(
        pool,
        &audit::AuditEntry {
            request_id: Some(request_id),
            actor_type,
            actor_id,
            actor_label: String::new(),
            surface: audit::Surface::Rest,
            action: "authz.denied".into(),
            resource: resource.as_str().into(),
            org_id: team.map(|t| t.org_id),
            team_id: team.map(|t| t.id),
            outcome: audit::Outcome::Denied,
            detail: serde_json::json!({
                "resource": resource.as_str(),
                "action": action.as_str(),
                "reason": reason.as_str(),
            }),
        },
    )
    .await;
}

/// Capture the current span's W3C trace context for outbox rows (spec/10 §8a): the async
/// consumer joins the originating request's trace.
pub fn trace_context_json() -> serde_json::Value {
    use opentelemetry::propagation::Injector;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    struct MapInjector(serde_json::Map<String, serde_json::Value>);
    impl Injector for MapInjector {
        fn set(&mut self, key: &str, value: String) {
            self.0
                .insert(key.to_string(), serde_json::Value::String(value));
        }
    }
    let mut injector = MapInjector(serde_json::Map::new());
    let context = tracing::Span::current().context();
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut injector);
    });
    serde_json::Value::Object(injector.0)
}

pub(crate) fn db_err(label: &'static str) -> impl Fn(sqlx::Error) -> DomainError {
    move |e| DomainError::internal(format!("{label}: {e}"))
}
