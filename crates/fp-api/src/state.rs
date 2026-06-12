//! Shared application state for the API router.

use fp_core::OidcValidator;
use metrics_exporter_prometheus::PrometheusHandle;
use sqlx::PgPool;
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
}
