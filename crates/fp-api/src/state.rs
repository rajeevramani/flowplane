//! Shared application state for the API router.

use fp_core::services::discovery::DiscoveryForwardingPolicy;
use fp_core::services::egress_advisory::EgressAdvisoryPolicy;
use fp_core::OidcValidator;
use metrics_exporter_prometheus::PrometheusHandle;
use sqlx::PgPool;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

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
