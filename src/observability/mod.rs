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
pub async fn init_observability(config: &ObservabilityConfig) -> Result<HealthChecker> {
    // Initialize logging first
    init_logging(config)?;

    // Initialize tracing if enabled
    if config.enable_tracing {
        init_tracing(config).await?;
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

    Ok(health_checker)
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

        let health_checker = init_observability(&config).await.unwrap();
        assert!(!health_checker.is_ready().await);
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

        let health_checker = init_observability(&config).await.unwrap();
        assert!(!health_checker.is_ready().await);
    }
}
