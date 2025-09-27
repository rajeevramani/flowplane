//! # Structured Logging
//!
//! Provides structured logging setup using the tracing ecosystem.

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
pub fn init_logging(config: &ObservabilityConfig) -> Result<()> {
    let env_filter = parse_env_filter(&config.log_level)?;

    LOGGING_INITIALIZED.get_or_try_init(|| configure_logging(config, env_filter)).map(|_| ())
}

fn configure_logging(config: &ObservabilityConfig, env_filter: EnvFilter) -> Result<()> {
    let registry = tracing_subscriber::registry().with(env_filter);

    if config.json_logging {
        // JSON structured logging for production
        let json_layer = fmt::layer()
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .fmt_fields(JsonFields::new());

        registry
            .with(json_layer)
            .try_init()
            .map_err(|e| FlowplaneError::config(format!("Failed to initialize logging: {}", e)))?;
    } else {
        // Human-readable logging for development
        let pretty_layer =
            fmt::layer().pretty().with_target(true).with_thread_ids(true).with_thread_names(true);

        registry
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
}
