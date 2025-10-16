//! # Distributed Tracing
//!
//! Provides distributed tracing setup using OpenTelemetry OTLP exporter.
//!
//! This module follows the official OpenTelemetry Rust guide and uses the
//! OpenTelemetry SDK directly (not via tracing-opentelemetry bridge).
//!
//! # Configuration
//!
//! Distributed tracing is configured via environment variables:
//!
//! - `FLOWPLANE_ENABLE_TRACING`: Enable/disable tracing (default: true)
//! - `FLOWPLANE_OTLP_ENDPOINT`: OTLP exporter endpoint (default: http://localhost:4317)
//! - `FLOWPLANE_TRACE_SAMPLING_RATIO`: Sampling ratio 0.0-1.0 (default: 1.0 = 100%)
//! - `FLOWPLANE_SERVICE_NAME`: Service name for traces (default: flowplane)
//!
//! # Usage
//!
//! ```rust
//! use opentelemetry::global;
//! use opentelemetry::trace::{Tracer, SpanKind};
//!
//! // Get the global tracer
//! let tracer = global::tracer("flowplane");
//!
//! // Create a span
//! let span = tracer
//!     .span_builder("operation_name")
//!     .with_kind(SpanKind::Server)
//!     .start(&tracer);
//!
//! // Span exports when dropped
//! drop(span);
//! ```

use crate::config::ObservabilityConfig;
use crate::errors::{FlowplaneError, Result};
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    trace::{RandomIdGenerator, Sampler, SdkTracerProvider},
    Resource,
};
use std::time::Duration;

/// Initialize distributed tracing with OpenTelemetry OTLP exporter
///
/// Returns the SdkTracerProvider which must be kept alive and shutdown before exit.
/// The tracer provider is also set globally, so you can get tracers via global::tracer().
pub async fn init_tracing(config: &ObservabilityConfig) -> Result<Option<SdkTracerProvider>> {
    if !config.enable_tracing {
        tracing::info!("Distributed tracing is disabled");
        return Ok(None);
    }

    // Determine the OTLP endpoint to use
    let otlp_endpoint = match &config.otlp_endpoint {
        Some(endpoint) => endpoint.clone(),
        None => {
            tracing::warn!(
                "No OTLP endpoint configured. Tracing enabled but exporter not initialized. \
                 Set FLOWPLANE_OTLP_ENDPOINT to enable trace export."
            );
            return Ok(None);
        }
    };

    tracing::info!(
        service_name = %config.service_name,
        otlp_endpoint = %otlp_endpoint,
        sampling_ratio = config.trace_sampling_ratio,
        "Initializing OpenTelemetry trace exporter"
    );

    // Create resource with service information
    let resource = Resource::builder_empty()
        .with_attribute(KeyValue::new(
            opentelemetry_semantic_conventions::resource::SERVICE_NAME,
            config.service_name.clone(),
        ))
        .with_attribute(KeyValue::new(
            opentelemetry_semantic_conventions::resource::SERVICE_VERSION,
            env!("CARGO_PKG_VERSION"),
        ))
        .build();

    // Configure OTLP exporter with timeout
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&otlp_endpoint)
        .with_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| FlowplaneError::internal(format!("Failed to create OTLP exporter: {}", e)))?;

    // Create tracer provider with batch exporter (async export in background)
    // This prevents blocking the async runtime when spans are dropped
    let sampler =
        Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(config.trace_sampling_ratio)));

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .with_id_generator(RandomIdGenerator::default())
        .with_sampler(sampler)
        .build();

    // Set the global text map propagator for W3C TraceContext
    global::set_text_map_propagator(opentelemetry_sdk::propagation::TraceContextPropagator::new());

    // Set global tracer provider - this allows using global::tracer() anywhere
    global::set_tracer_provider(provider.clone());

    tracing::info!(
        sampling_ratio = config.trace_sampling_ratio,
        "OpenTelemetry trace exporter initialized successfully"
    );

    Ok(Some(provider))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_init_tracing_disabled() {
        let config = ObservabilityConfig { enable_tracing: false, ..Default::default() };

        let result = init_tracing(&config).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_init_tracing_enabled() {
        let config = ObservabilityConfig {
            enable_tracing: true,
            otlp_endpoint: Some("http://localhost:4317".to_string()),
            ..Default::default()
        };

        let result = init_tracing(&config).await;
        assert!(result.is_ok());
    }
}
