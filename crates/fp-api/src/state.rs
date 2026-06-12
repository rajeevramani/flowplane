//! Shared application state for the API router.

use metrics_exporter_prometheus::PrometheusHandle;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub prometheus: PrometheusHandle,
    /// Version reported by /healthz, set from the binary's build info.
    pub version: &'static str,
}
