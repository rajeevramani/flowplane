//! # Structured Logging
//!
//! Provides structured logging macros and utilities using the tracing ecosystem.
//!
//! # Trace-Log Correlation
//!
//! When OpenTelemetry tracing is enabled, all log entries automatically include
//! trace context (trace ID and span ID) for correlation. This is handled by the
//! `tracing-opentelemetry` layer which bridges `#[instrument]` spans to OpenTelemetry.
//!
//! In JSON logging mode, trace context is included as fields in the JSON output:
//! - `trace_id`: W3C trace ID (32 hex characters)
//! - `span_id`: Span ID (16 hex characters)
//!
//! This enables searching logs by trace ID in your logging system and correlating
//! logs with distributed traces in your tracing backend (Jaeger, Zipkin, etc.).

/// Create a tracing span for request tracking.
///
/// Supports optional `org_id` and `team_id` fields for multi-tenant tracing.
/// Use the 4-arg form to include org context:
///
/// ```rust,ignore
/// let span = request_span!("GET", "/api/clusters", Some("org-1"), Some("team-1"));
/// ```
#[macro_export]
macro_rules! request_span {
    ($method:expr, $path:expr) => {
        tracing::info_span!(
            "http_request",
            method = %$method,
            path = %$path,
            request_id = %uuid::Uuid::new_v4(),
            org_id = tracing::field::Empty,
            team_id = tracing::field::Empty
        )
    };
    ($method:expr, $path:expr, org_id = $org:expr, team_id = $team:expr) => {
        tracing::info_span!(
            "http_request",
            method = %$method,
            path = %$path,
            request_id = %uuid::Uuid::new_v4(),
            org_id = $org,
            team_id = $team
        )
    };
    ($method:expr, $path:expr, $($field:tt)*) => {
        tracing::info_span!(
            "http_request",
            method = %$method,
            path = %$path,
            request_id = %uuid::Uuid::new_v4(),
            org_id = tracing::field::Empty,
            team_id = tracing::field::Empty,
            $($field)*
        )
    };
}

/// Create a tracing span for database operations.
///
/// Supports optional `org_id` and `team_id` fields for multi-tenant tracing.
/// Use the named-field form to include org context:
///
/// ```rust,ignore
/// let span = db_span!("insert_cluster", org_id = "org-1", team_id = "team-1");
/// ```
#[macro_export]
macro_rules! db_span {
    ($operation:expr) => {
        tracing::debug_span!(
            "db_operation",
            operation = %$operation,
            operation_id = %uuid::Uuid::new_v4(),
            org_id = tracing::field::Empty,
            team_id = tracing::field::Empty
        )
    };
    ($operation:expr, org_id = $org:expr, team_id = $team:expr) => {
        tracing::debug_span!(
            "db_operation",
            operation = %$operation,
            operation_id = %uuid::Uuid::new_v4(),
            org_id = $org,
            team_id = $team
        )
    };
    ($operation:expr, $($field:tt)*) => {
        tracing::debug_span!(
            "db_operation",
            operation = %$operation,
            operation_id = %uuid::Uuid::new_v4(),
            org_id = tracing::field::Empty,
            team_id = tracing::field::Empty,
            $($field)*
        )
    };
}

/// Create a tracing span for xDS operations
#[macro_export]
macro_rules! xds_span {
    ($operation:expr, $node_id:expr) => {
        tracing::info_span!(
            "xds_operation",
            operation = %$operation,
            node_id = %$node_id,
            operation_id = %uuid::Uuid::new_v4()
        )
    };
    ($operation:expr, $node_id:expr, $($field:tt)*) => {
        tracing::info_span!(
            "xds_operation",
            operation = %$operation,
            node_id = %$node_id,
            operation_id = %uuid::Uuid::new_v4(),
            $($field)*
        )
    };
}

/// Log configuration at startup
pub fn log_config_info(config: &crate::config::AppConfig) {
    tracing::info!(
        server_address = %config.server.bind_address(),
        xds_address = %config.xds.bind_address(),
        database_type = "postgresql",
        auth_enabled = %config.auth.enable_auth,
        metrics_enabled = %config.observability.enable_metrics,
        tracing_enabled = %config.observability.enable_tracing,
        "Flowplane control plane configuration"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_macros_compile() {
        // Test that our macros compile correctly
        let _span = request_span!("GET", "/api/clusters");
        let _span = request_span!("POST", "/api/clusters", cluster_name = "test");
        let _span =
            request_span!("GET", "/api/orgs/org-1/teams", org_id = "org-1", team_id = "team-1");
        let _span = db_span!("insert_cluster");
        let _span = db_span!("insert_cluster", table = "clusters");
        let _span = db_span!("insert_cluster", org_id = "org-1", team_id = "team-1");
        let _span = xds_span!("stream_clusters", "node-1");
        let _span = xds_span!("stream_clusters", "node-1", version = "v1");
    }

    #[test]
    fn test_log_config_info() {
        let config = crate::config::AppConfig::default();

        // This should not panic
        log_config_info(&config);
    }
}
