//! Services: the only mutation path. Shared helpers for authorization-to-error mapping,
//! audit actor extraction, trace-context capture, and quotas.

pub mod clusters;
pub mod gateway;
pub mod orgs;
pub mod quota;
pub mod teams;

use crate::authz::{PrincipalCtx, Reason};
use fp_domain::authz::{Action, Resource};
use fp_domain::{DomainError, ErrorCode};
use fp_storage::repos::audit::ActorType;

/// Map an authorization denial to the wire error. Cross-org and no-grant denials on reads
/// render as `not_found` (anti-enumeration, spec/05 §3.2.2); everything else is `forbidden`
/// naming the missing (resource, action) so the caller knows what grant to request.
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
