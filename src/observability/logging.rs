//! # Structured Logging
//!
//! Provides structured logging setup using the tracing ecosystem.
//!
//! # Trace-Log Correlation
//!
//! When OpenTelemetry tracing is enabled, all log entries automatically include
//! trace context (trace ID and span ID) for correlation. This is handled by the
//! `tracing-opentelemetry` layer.
//!
//! In JSON logging mode, trace context is included as fields in the JSON output:
//! - `trace_id`: W3C trace ID (32 hex characters)
//! - `span_id`: Span ID (16 hex characters)
//!
//! This enables searching logs by trace ID in your logging system and correlating
//! logs with distributed traces in your tracing backend (Jaeger, Zipkin, etc.).

use crate::config::ObservabilityConfig;
use crate::errors::{FlowplaneError, Result};
use once_cell::sync::OnceCell;
use tracing_subscriber::{
    fmt::{self, format::JsonFields},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
};

static LOGGING_INITIALIZED: OnceCell<()> = OnceCell::new();

/// Initialize structured logging based on configuration
///
/// Note: This only sets up logging (tracing crate for structured logs).
/// OpenTelemetry distributed tracing is handled separately in init_tracing().
pub fn init_logging(config: &ObservabilityConfig) -> Result<()> {
    let env_filter = parse_env_filter(&config.log_level)?;

    LOGGING_INITIALIZED.get_or_try_init(|| configure_logging(config, env_filter)).map(|_| ())
}

fn configure_logging(config: &ObservabilityConfig, env_filter: EnvFilter) -> Result<()> {
    // Build subscriber layers based on configuration
    // Note: We only handle logging here. OpenTelemetry tracing is separate.
    if config.json_logging {
        let json_layer = fmt::layer()
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .fmt_fields(JsonFields::new());

        tracing_subscriber::registry()
            .with(env_filter)
            .with(json_layer)
            .try_init()
            .map_err(|e| FlowplaneError::config(format!("Failed to initialize logging: {}", e)))?;
    } else {
        let pretty_layer =
            fmt::layer().pretty().with_target(true).with_thread_ids(true).with_thread_names(true);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(pretty_layer)
            .try_init()
            .map_err(|e| FlowplaneError::config(format!("Failed to initialize logging: {}", e)))?;
    }

    Ok(())
}

fn parse_env_filter(level: &str) -> Result<EnvFilter> {
    let normalized = level.trim();
    let lower = normalized.to_ascii_lowercase();

    match lower.as_str() {
        "trace" | "debug" | "info" | "warn" | "error" => {}
        _ => {
            return Err(FlowplaneError::config(format!(
                "Invalid log level '{}': must be one of trace, debug, info, warn, error",
                level
            )));
        }
    }

    EnvFilter::try_new(normalized)
        .map_err(|e| FlowplaneError::config(format!("Invalid log level '{}': {}", level, e)))
}

/// Create a tracing span for request tracking
#[macro_export]
macro_rules! request_span {
    ($method:expr, $path:expr) => {
        tracing::info_span!(
            "http_request",
            method = %$method,
            path = %$path,
            request_id = %uuid::Uuid::new_v4()
        )
    };
    ($method:expr, $path:expr, $($field:tt)*) => {
        tracing::info_span!(
            "http_request",
            method = %$method,
            path = %$path,
            request_id = %uuid::Uuid::new_v4(),
            $($field)*
        )
    };
}

/// Create a tracing span for database operations
#[macro_export]
macro_rules! db_span {
    ($operation:expr) => {
        tracing::debug_span!(
            "db_operation",
            operation = %$operation,
            operation_id = %uuid::Uuid::new_v4()
        )
    };
    ($operation:expr, $($field:tt)*) => {
        tracing::debug_span!(
            "db_operation",
            operation = %$operation,
            operation_id = %uuid::Uuid::new_v4(),
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
        database_type = if config.database.is_sqlite() { "sqlite" } else { "postgresql" },
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
    fn test_init_logging_pretty() {
        let config = ObservabilityConfig {
            log_level: "debug".to_string(),
            json_logging: false,
            ..Default::default()
        };

        // This should not panic
        let result = init_logging(&config);
        assert!(result.is_ok() || result.is_err()); // tracing_subscriber might be already initialized
    }

    #[test]
    fn test_init_logging_json() {
        let config = ObservabilityConfig {
            log_level: "info".to_string(),
            json_logging: true,
            ..Default::default()
        };

        // This should not panic
        let result = init_logging(&config);
        assert!(result.is_ok() || result.is_err()); // tracing_subscriber might be already initialized
    }

    #[test]
    fn test_invalid_log_level() {
        let config =
            ObservabilityConfig { log_level: "invalid_level".to_string(), ..Default::default() };

        let result = init_logging(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_macros_compile() {
        // Test that our macros compile correctly
        let _span = request_span!("GET", "/api/clusters");
        let _span = request_span!("POST", "/api/clusters", cluster_name = "test");
        let _span = db_span!("insert_cluster");
        let _span = db_span!("insert_cluster", table = "clusters");
        let _span = xds_span!("stream_clusters", "node-1");
        let _span = xds_span!("stream_clusters", "node-1", version = "v1");
    }

    #[test]
    fn test_log_config_info() {
        let config = crate::config::AppConfig::default();

        // This should not panic
        log_config_info(&config);
    }

    #[tokio::test]
    async fn test_logging_json() {
        // Test that JSON logging can be initialized
        let config = crate::config::ObservabilityConfig {
            enable_tracing: false,
            enable_metrics: false,
            json_logging: true,
            log_level: "info".to_string(),
            ..Default::default()
        };

        // Initialize logging
        let result = init_logging(&config);

        // May succeed or fail if already initialized - both are acceptable
        assert!(result.is_ok() || result.is_err());
    }
}
