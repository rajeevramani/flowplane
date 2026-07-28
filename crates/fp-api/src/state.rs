//! Shared application state for the API router.

use fp_core::services::discovery::DiscoveryForwardingPolicy;
use fp_core::services::egress_advisory::EgressAdvisoryPolicy;
use fp_core::OidcValidator;
use fp_domain::TeamId;
use metrics_exporter_prometheus::PrometheusHandle;
use serde::Serialize;
use sqlx::PgPool;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub prometheus: PrometheusHandle,
    /// Version reported by /healthz, set from the binary's build info.
    pub version: &'static str,
    /// `None` = auth not configured: authenticated endpoints answer 503 (degraded mode).
    pub validator: Option<Arc<OidcValidator>>,
    /// Per-tenant write throttle (spec/10 §4a edge hardening).
    pub write_throttle: Arc<crate::throttle::WriteThrottle>,
    /// Optional xDS outbox consumer readiness. API-only tests and deployments can leave this off.
    pub xds_readiness: Option<XdsReadiness>,
    /// Optional read-only handle over the live xDS snapshot cache's degraded set, so
    /// `GET /xds/status` can report the per-team withdrawn resources. `None` (API-only tests /
    /// deployments without the cache wired) → the endpoint reports an empty `withdrawn` list.
    /// Mirrors the `xds_readiness` seam; kept dependency-free (implemented in the binary, so
    /// fp-api need not depend on fp-xds).
    pub xds_degraded: Option<Arc<dyn XdsDegradedSource>>,
    /// Runtime deny policy for S9 discovery forwarding.
    pub discovery_forwarding_policy: DiscoveryForwardingPolicy,
    /// Write-time egress advisory (FP-DEC-0008, fpv2-1hp): built once at boot from
    /// `ServerConfig`; consumed by the mutation paths that accept tenant-authored upstream
    /// hosts. `Default` = disabled (tests that don't exercise the advisory).
    pub egress_advisory: EgressAdvisoryPolicy,
    /// Kicks the rate-limit `rls_sync` worker for an immediate reconcile (force-repush).
    /// `None` when the RLS admin URL is unconfigured (the worker is not running).
    pub rls_repush: Option<Arc<tokio::sync::Notify>>,
    /// `true` when `FLOWPLANE_RLS_GRPC_URL` is set, i.e. the CP injects the built-in
    /// `rate_limit_cluster` into CDS (S6). The listener service reads this to fail closed when a
    /// `global_rate_limit` filter points at the built-in cluster but injection is off (S7).
    pub rls_grpc_configured: bool,
}

#[derive(Clone)]
pub struct XdsReadiness {
    pub consumer: &'static str,
    pub max_lag: i64,
    pub failed: Arc<AtomicBool>,
}

/// One xDS resource whose latest revision was withdrawn from a team's served snapshot: either
/// quarantined after a dataplane NACK — in which case its **last-good** bytes are still served,
/// only the rejected revision is held back — or dropped because it failed translation. `error`
/// carries the reason. fpv2-xni.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WithdrawnResource {
    /// The xDS type URL (cluster/endpoint/route/secret/listener).
    pub type_url: String,
    /// The resource name.
    pub name: String,
    /// Why the latest revision was withdrawn (NACK detail or translation error).
    pub error: String,
}

/// Narrow read-only handle over the live xDS snapshot cache's degraded set. Defined here (not in
/// fp-xds) so fp-api stays decoupled from fp-xds; the binary implements it over `SnapshotCache`.
/// Dependency-free async via a boxed future (the workspace has no `async-trait`).
pub trait XdsDegradedSource: Send + Sync {
    fn withdrawn<'a>(
        &'a self,
        team_id: TeamId,
    ) -> Pin<Box<dyn Future<Output = Vec<WithdrawnResource>> + Send + 'a>>;
}
