//! # Observability Infrastructure
//!
//! This module provides comprehensive observability for the Flowplane control plane,
//! including structured logging, distributed tracing, metrics collection, and
//! health checking.

pub mod health;
pub mod logging;
pub mod metrics;
pub mod tracing;

pub use health::HealthChecker;
pub use logging::init_logging;
pub use metrics::{init_metrics, MetricsRecorder};
pub use tracing::init_tracing;

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
    // Initialize tracing first (if enabled) to set up global tracer provider
    // This MUST come before init_logging() so the OpenTelemetry layer can use it
    let tracer_and_provider = init_tracing(config).await?;
    let tracing_initialized = tracer_and_provider.is_some();

    // Clone the provider to return it later
    let provider = tracer_and_provider.as_ref().map(|(_, p)| p.clone());

    // Initialize logging after tracing so the OpenTelemetry layer is available
    // Pass the tracer to the logging layer for proper integration
    init_logging(config, tracer_and_provider)?;

    // Log tracing status after logging is initialized
    if tracing_initialized && config.otlp_endpoint.is_some() {
        info!(
            otlp_endpoint = config.otlp_endpoint.as_ref().unwrap(),
            "OpenTelemetry tracing initialized with OTLP exporter"
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

        let (health_checker, provider) = init_observability(&config).await.unwrap();
        assert!(!health_checker.is_ready().await);
        assert!(provider.is_none());
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

        let (health_checker, _provider) = init_observability(&config).await.unwrap();
        assert!(!health_checker.is_ready().await);
    }
}
