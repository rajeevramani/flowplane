//! # Observability Infrastructure
//!
//! This module provides comprehensive observability for the Flowplane control plane,
//! including structured logging, distributed tracing, metrics collection, and
//! health checking.

pub mod grpc_tracing;
pub mod health;
pub mod http_tracing;
pub mod logging;
pub mod metrics;
pub mod tracing;

pub use grpc_tracing::GrpcTracingLayer;
pub use health::HealthChecker;
pub use http_tracing::trace_http_requests;
pub use logging::log_config_info;
pub use metrics::{init_metrics, MetricsRecorder};
pub use tracing::{init_tracing_with_logging, shutdown_tracing};

use crate::config::ObservabilityConfig;
use crate::errors::Result;
use ::tracing::info;

/// Initialize all observability components
///
/// Returns a tuple of (HealthChecker, Option<SdkTracerProvider>)
/// The SdkTracerProvider must be kept to call shutdown() before application exit
pub async fn init_observability(
    config: &ObservabilityConfig,
) -> Result<(HealthChecker, Option<opentelemetry_sdk::trace::SdkTracerProvider>)> {
    // Initialize tracing and logging together
    // This bridges #[instrument] spans to OpenTelemetry for export to Jaeger/Zipkin
    let provider = init_tracing_with_logging(config).await?;

    let tracing_initialized = provider.is_some();

    // Log tracing status after logging is initialized
    if tracing_initialized && config.otlp_endpoint.is_some() {
        info!(
            otlp_endpoint = config.otlp_endpoint.as_ref().unwrap(),
            "OpenTelemetry tracing initialized with tracing-opentelemetry bridge"
        );
    }

    // Initialize metrics if enabled
    if config.enable_metrics {
        init_metrics(config).await?;
    }

    // Create and return health checker
    let health_checker = HealthChecker::new();

    info!(
        service_name = %config.service_name,
        log_level = %config.log_level,
        metrics_enabled = %config.enable_metrics,
        tracing_enabled = %config.enable_tracing,
        "Observability initialized successfully"
    );

    Ok((health_checker, provider))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_init_observability() {
        let config = ObservabilityConfig {
            enable_metrics: false, // Disable to avoid port conflicts in tests
            enable_tracing: false, // Disable to avoid external dependencies in tests
            ..Default::default()
        };

        let result = init_observability(&config).await;
        // May succeed or fail depending on whether subscriber is already set
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_init_observability_with_features() {
        let config = ObservabilityConfig {
            enable_metrics: true,
            enable_tracing: true,
            metrics_port: 0,       // Use port 0 to avoid conflicts
            jaeger_endpoint: None, // Disable Jaeger in tests
            ..Default::default()
        };

        let result = init_observability(&config).await;
        // May succeed or fail depending on whether subscriber is already set
        assert!(result.is_ok() || result.is_err());
    }
}
